use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

pub const PROTOCOL_VERSION: u32 = 2;
pub const MAX_FRAME_BYTES: usize = 80 * 1024 * 1024;
pub const MAX_FILE_BYTES: usize = 1024 * 1024;

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
pub struct SearchMatch {
    pub path: String,
    /// One-based line and byte column, matching Bed document indexing.
    pub line: usize,
    pub column: usize,
    pub editor_row: usize,
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
    ReadDirectory {
        root: String,
        path: String,
        classify_gitignored: bool,
    },
    ListFiles {
        root: String,
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
        case_sensitive: bool,
        include_ignored: bool,
        max_results: usize,
    },
    GitStatus {
        root: String,
    },
    GitBaseline {
        root: String,
        path: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Response {
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
    Directory {
        entries: Vec<DirectoryEntry>,
        /// Classification failures preserve the listing and leave entries visible.
        warning: Option<String>,
    },
    Files {
        paths: Vec<String>,
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
