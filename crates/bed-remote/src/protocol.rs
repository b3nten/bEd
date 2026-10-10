pub use bed_editing::text_search::SearchOptions;
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

pub const PROTOCOL_VERSION: u32 = 9;
pub const TRANSFER_CHUNK_BYTES: usize = 1024 * 1024;
pub const MAX_FILE_PREFIX_BYTES: usize = 1024;
// JSON arrays of u8 require at most four bytes per input byte. Keep headroom
// for a full file and its request metadata.
pub const MAX_FILE_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_FRAME_BYTES: usize = 4 * MAX_FILE_BYTES + 16 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct FileBaseline {
    pub fingerprint: u64,
    pub len: u64,
    pub modified_ns: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub path: String,
    pub name: String,
    pub is_directory: bool,
    pub is_symlink: bool,
    pub is_gitignored: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct TransferEntry {
    pub len: u64,
    pub is_directory: bool,
    pub is_symlink: bool,
    pub symlink_target: Option<String>,
    pub modified_ns: Option<u64>,
    pub mode: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FilesystemChange {
    Created { path: String },
    Modified { path: String },
    Removed { path: String },
    Renamed { from: String, to: String },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DirectoryListing {
    pub path: String,
    pub entries: Vec<DirectoryEntry>,
    pub warning: Option<String>,
}

/// A monotonic filesystem generation; full snapshots occur only at startup or
/// explicit/overflow recovery. Ordinary notifications contain index deltas.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct WorkspaceUpdate {
    pub root: String,
    pub generation: u64,
    pub changes: Vec<FilesystemChange>,
    pub dirty_directories: Vec<String>,
    pub directories: Vec<DirectoryListing>,
    pub indexed_files: Option<Vec<String>>,
    pub indexed_added: Vec<String>,
    pub indexed_removed: Vec<String>,
    pub degraded: Option<String>,
    pub ready: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct FileInfo {
    pub file_type: String,
    #[serde(default)]
    pub mime_type: Option<String>,
    pub size: u64,
    pub modified_unix_seconds: Option<i64>,
    pub is_directory: bool,
    pub symlink_target: Option<String>,
    pub readonly: bool,
    pub git: Option<String>,
    pub binary: Option<BinaryInfo>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BinaryInfo {
    pub format: String,
    pub architecture: String,
    pub debug_symbols: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SearchMatch {
    pub path: String,
    /// One-based line and byte column, matching Bed document indexing.
    pub line: usize,
    pub column: usize,
    pub editor_row: usize,
    /// Zero-based exclusive byte range within line_bytes.
    pub range: std::ops::Range<usize>,
    pub line_bytes: Vec<u8>,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct GitStatusEntry {
    pub path: String,
    pub index_status: char,
    pub worktree_status: char,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum Request {
    Hello {
        version: u32,
    },
    Canonicalize {
        root: String,
        path: String,
        allow_missing: bool,
    },
    ReadFile {
        root: String,
        path: String,
    },
    /// Read a bounded sample without loading a complete file.
    ReadFilePrefix {
        root: String,
        path: String,
    },
    ReadDirectory {
        root: String,
        path: String,
        classify_gitignored: bool,
    },
    FileInfo {
        root: String,
        path: String,
    },
    ListFiles {
        root: String,
    },
    WatchWorkspace {
        root: String,
        include_ignored: bool,
    },
    PollWorkspace {
        watch_id: u64,
    },
    RefreshWorkspace {
        watch_id: u64,
        include_ignored: bool,
    },
    WatchDirectory {
        watch_id: u64,
        path: String,
    },
    RefreshWorkspaceDirectory {
        watch_id: u64,
        path: String,
    },
    UnwatchWorkspace {
        watch_id: u64,
    },
    TransferStat {
        root: String,
        path: String,
    },
    ReadFileChunk {
        root: String,
        path: String,
        offset: u64,
        max_bytes: usize,
    },
    WriteFileChunk {
        root: String,
        path: String,
        offset: u64,
        bytes: Vec<u8>,
        create: bool,
        mode: Option<u32>,
    },
    CommitFileTransfer {
        root: String,
        temporary: String,
        path: String,
        replace: bool,
    },
    CreateSymlink {
        root: String,
        path: String,
        target: String,
    },
    RemoveEmptyDirectory {
        root: String,
        path: String,
    },
    WriteFile {
        root: String,
        path: String,
        bytes: Vec<u8>,
        baseline: Option<FileBaseline>,
    },
    CreateFile {
        root: String,
        path: String,
    },
    CreateDirectory {
        root: String,
        path: String,
    },
    Rename {
        root: String,
        from: String,
        to: String,
    },
    Remove {
        root: String,
        path: String,
        is_directory: bool,
    },
    Search {
        root: String,
        query: String,
        options: SearchOptions,
        buffer_paths: Vec<String>,
        max_results: usize,
    },
    GitStatus {
        root: String,
    },
    GitBaseline {
        root: String,
        path: String,
    },
    /// Git commands run in bounded jobs so hooks/network I/O never block saves.
    GitStart {
        root: String,
        #[serde(rename = "git_operation")]
        operation: bed_git::Operation,
    },
    GitPoll {
        job_id: u64,
    },
    GitCancel {
        job_id: u64,
    },
    GitRelease {
        job_id: u64,
    },
    CheckStart {
        root: String,
        program: String,
        arguments: Vec<String>,
        directory: String,
    },
    CheckPoll {
        job_id: u64,
    },
    CheckCancel {
        job_id: u64,
    },
    CheckRelease {
        job_id: u64,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Response {
    CheckStarted {
        job_id: u64,
    },
    CheckJob {
        #[serde(rename = "completion")]
        result: Option<std::result::Result<crate::CheckOutput, String>>,
    },
    GitStarted {
        job_id: u64,
    },
    /// Completion remains available until explicitly released.
    GitJob {
        #[serde(rename = "completion")]
        result: Option<std::result::Result<bed_git::Output, bed_git::Error>>,
    },
    Hello {
        version: u32,
    },
    Path {
        path: String,
    },
    File {
        path: String,
        bytes: Vec<u8>,
        baseline: FileBaseline,
    },
    FilePrefix {
        /// Canonical path, matching `File` and preserving resolved symlink identity.
        path: String,
        bytes: Vec<u8>,
    },
    Directory {
        entries: Vec<DirectoryEntry>,
        /// Classification failures preserve the listing and leave entries visible.
        warning: Option<String>,
    },
    FileInfo {
        info: FileInfo,
    },
    Files {
        paths: Vec<String>,
    },
    WorkspaceWatch {
        watch_id: u64,
    },
    WorkspaceUpdates {
        updates: Vec<WorkspaceUpdate>,
    },
    TransferStat {
        entry: TransferEntry,
    },
    FileChunk {
        bytes: Vec<u8>,
    },
    Written {
        baseline: FileBaseline,
    },
    Unit,
    Search {
        matches: Vec<SearchMatch>,
        truncated: bool,
        scanned_files: usize,
        discovered_files: usize,
        ignored_paths: usize,
        skipped_files: usize,
        eligible_buffer_paths: Vec<String>,
    },
    GitStatus {
        entries: Vec<GitStatusEntry>,
    },
    GitBaseline {
        bytes: Option<Vec<u8>>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    NotFound,
    PermissionDenied,
    AlreadyExists,
    CrossesDevices,
    InvalidInput,
    Conflict,
    UnsupportedVersion,
    TooLarge,
    Other,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RemoteError {
    pub kind: ErrorKind,
    pub message: String,
}

impl RemoteError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn into_io(self) -> io::Error {
        let kind = match self.kind {
            ErrorKind::NotFound => io::ErrorKind::NotFound,
            ErrorKind::PermissionDenied => io::ErrorKind::PermissionDenied,
            ErrorKind::AlreadyExists => io::ErrorKind::AlreadyExists,
            ErrorKind::CrossesDevices => io::ErrorKind::CrossesDevices,
            ErrorKind::InvalidInput | ErrorKind::UnsupportedVersion | ErrorKind::TooLarge => {
                io::ErrorKind::InvalidInput
            }
            ErrorKind::Conflict => io::ErrorKind::WouldBlock,
            ErrorKind::Other => io::ErrorKind::Other,
        };
        io::Error::new(kind, self.message)
    }
}

impl From<io::Error> for RemoteError {
    fn from(error: io::Error) -> Self {
        let kind = match error.kind() {
            io::ErrorKind::NotFound => ErrorKind::NotFound,
            io::ErrorKind::PermissionDenied => ErrorKind::PermissionDenied,
            io::ErrorKind::AlreadyExists => ErrorKind::AlreadyExists,
            io::ErrorKind::CrossesDevices => ErrorKind::CrossesDevices,
            io::ErrorKind::InvalidInput | io::ErrorKind::InvalidData => ErrorKind::InvalidInput,
            _ => ErrorKind::Other,
        };
        Self::new(kind, error.to_string())
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RequestFrame {
    pub id: u64,
    pub request: Request,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ResponseFrame {
    pub id: u64,
    pub response: Result<Response, RemoteError>,
}

pub fn write_frame<W: Write, T: Serialize>(writer: &mut W, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "remote protocol frame exceeds limit",
        ));
    }
    writer.write_all(&(bytes.len() as u32).to_be_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()
}

/// EOF before a new frame is normal; a partial header or payload is an error.
pub fn read_frame<R: Read, T: serde::de::DeserializeOwned>(
    reader: &mut R,
) -> io::Result<Option<T>> {
    let mut header = [0_u8; 4];
    loop {
        match reader.read(&mut header[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    reader.read_exact(&mut header[1..])?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid remote protocol frame length",
        ));
    }
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload)?;
    serde_json::from_slice(&payload)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_roundtrip_preserves_binary_and_request_id() {
        let frame = RequestFrame {
            id: 19,
            request: Request::WriteFile {
                root: "/project".into(),
                path: "binary".into(),
                bytes: vec![0, 255, 13, 10],
                baseline: None,
            },
        };
        let mut encoded = Vec::new();
        write_frame(&mut encoded, &frame).unwrap();
        let decoded: RequestFrame = read_frame(&mut &encoded[..]).unwrap().unwrap();
        assert_eq!(decoded.id, 19);
        assert_eq!(decoded.request, frame.request);
    }

    #[test]
    fn file_information_roundtrips_mime_and_accepts_older_responses() {
        let mut value = serde_json::json!({
            "result": "file_info",
            "info": {
                "file_type": "PNG image",
                "mime_type": "image/png",
                "size": 13,
                "modified_unix_seconds": null,
                "is_directory": false,
                "symlink_target": null,
                "readonly": false,
                "git": null,
                "binary": null
            }
        });
        for mime in [Some("image/png"), None] {
            if mime.is_none() {
                value["info"].as_object_mut().unwrap().remove("mime_type");
            }
            let response: Response = serde_json::from_value(value.clone()).unwrap();
            let frame = ResponseFrame {
                id: 23,
                response: Ok(response.clone()),
            };
            let mut encoded = Vec::new();
            write_frame(&mut encoded, &frame).unwrap();
            let decoded: ResponseFrame = read_frame(&mut &encoded[..]).unwrap().unwrap();
            assert_eq!(decoded.id, 23);
            assert_eq!(decoded.response.as_ref().unwrap(), &response);
            let Response::FileInfo { info } = decoded.response.unwrap() else {
                panic!("expected file information");
            };
            assert_eq!(info.mime_type.as_deref(), mime);
        }
    }

    #[test]
    fn full_editable_file_fits_the_frame_even_with_worst_case_json_bytes() {
        let frame = RequestFrame {
            id: u64::MAX,
            request: Request::WriteFile {
                root: "/project".into(),
                path: "maximum-size.bin".into(),
                bytes: vec![255; MAX_FILE_BYTES],
                baseline: Some(FileBaseline {
                    fingerprint: u64::MAX,
                    len: MAX_FILE_BYTES as u64,
                    modified_ns: Some(u64::MAX),
                }),
            },
        };
        write_frame(&mut io::sink(), &frame).unwrap();
    }

    #[test]
    fn protocol_rejects_truncation_oversize_and_malformed_json() {
        assert!(read_frame::<_, RequestFrame>(&mut &[0, 0][..]).is_err());
        assert!(
            read_frame::<_, RequestFrame>(&mut &((MAX_FRAME_BYTES as u32) + 1).to_be_bytes()[..])
                .is_err()
        );
        assert!(read_frame::<_, RequestFrame>(&mut &[0, 0, 0, 1, b'{'][..]).is_err());
        assert!(
            read_frame::<_, RequestFrame>(&mut &[][..])
                .unwrap()
                .is_none()
        );
    }
}
