//! Bounded transfer primitives. Paths are capabilities rooted at a workspace;
//! final symlink entries are inspected/replaced without following their targets.
use crate::*;
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
    time::UNIX_EPOCH,
};

fn entry(root: &str, path: &str) -> Result<PathBuf, RemoteError> {
    let root = fs::canonicalize(root)?;
    let path = root.join(path);
    if path == root {
        return Ok(root);
    }
    let name = path
        .file_name()
        .ok_or_else(|| RemoteError::new(ErrorKind::InvalidInput, "Missing filename"))?;
    let parent = fs::canonicalize(
        path.parent()
            .ok_or_else(|| RemoteError::new(ErrorKind::InvalidInput, "Missing parent"))?,
    )?;
    if !parent.starts_with(&root) {
        return Err(RemoteError::new(
            ErrorKind::PermissionDenied,
            "Transfer path escapes workspace",
        ));
    }
    Ok(parent.join(name))
}

fn writable(root: &str, path: &str) -> Result<PathBuf, RemoteError> {
    let resolved = entry(root, path)?;
    if resolved == fs::canonicalize(root)? {
        return Err(RemoteError::new(
            ErrorKind::PermissionDenied,
            "Cannot replace workspace root",
        ));
    }
    Ok(resolved)
}

pub(crate) fn call(request: Request) -> Result<Response, RemoteError> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    match request {
        Request::TransferStat { root, path } => {
            let path = entry(&root, &path)?;
            let metadata = fs::symlink_metadata(&path)?;
            let is_symlink = metadata.file_type().is_symlink();
            if !metadata.is_file() && !metadata.is_dir() && !is_symlink {
                return Err(RemoteError::new(
                    ErrorKind::InvalidInput,
                    "Only regular files, folders, and symbolic links can be transferred",
                ));
            }
            Ok(Response::TransferStat {
                entry: TransferEntry {
                    len: metadata.len(),
                    is_directory: metadata.is_dir(),
                    is_symlink,
                    symlink_target: if is_symlink {
                        Some(
                            fs::read_link(path)?
                                .to_str()
                                .ok_or_else(|| {
                                    RemoteError::new(
                                        ErrorKind::InvalidInput,
                                        "Link target is not UTF-8",
                                    )
                                })?
                                .into(),
                        )
                    } else {
                        None
                    },
                    modified_ns: metadata
                        .modified()
                        .ok()
                        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                        .map(|time| time.as_nanos().min(u64::MAX as u128) as u64),
                    mode: metadata.permissions().mode(),
                },
            })
        }
        Request::ReadFileChunk {
            root,
            path,
            offset,
            max_bytes,
        } => {
            let path = entry(&root, &path)?;
            let mut file = fs::OpenOptions::new()
                .read(true)
                .custom_flags(no_follow() | non_blocking())
                .open(path)?;
            if !file.metadata()?.is_file() {
                return Err(RemoteError::new(
                    ErrorKind::InvalidInput,
                    "Transfer source is not a regular file",
                ));
            }
            file.seek(SeekFrom::Start(offset))?;
            let mut bytes = Vec::new();
            file.take(max_bytes.min(TRANSFER_CHUNK_BYTES) as u64)
                .read_to_end(&mut bytes)?;
            Ok(Response::FileChunk { bytes })
        }
        Request::WriteFileChunk {
            root,
            path,
            offset,
            bytes,
            create,
            mode,
        } => {
            if bytes.len() > TRANSFER_CHUNK_BYTES {
                return Err(RemoteError::new(
                    ErrorKind::TooLarge,
                    "Transfer chunk exceeds limit",
                ));
            }
            if create && offset != 0 {
                return Err(RemoteError::new(
                    ErrorKind::InvalidInput,
                    "A transfer must start at offset zero",
                ));
            }
            let path = writable(&root, &path)?;
            let mut options = fs::OpenOptions::new();
            options
                .write(true)
                .custom_flags(no_follow() | non_blocking());
            if create {
                options.create_new(true);
            }
            let mut file = options.open(path)?;
            if !file.metadata()?.is_file() {
                return Err(RemoteError::new(
                    ErrorKind::InvalidInput,
                    "Transfer destination is not a regular file",
                ));
            }
            // A transfer cannot silently create holes or overwrite an earlier chunk.
            if file.metadata()?.len() != offset {
                return Err(RemoteError::new(
                    ErrorKind::Conflict,
                    "Transfer offset changed",
                ));
            }
            file.seek(SeekFrom::Start(offset))?;
            file.write_all(&bytes)?;
            if let Some(mode) = mode {
                file.set_permissions(fs::Permissions::from_mode(mode & 0o777))?;
            }
            Ok(Response::Unit)
        }
        Request::CommitFileTransfer {
            root,
            temporary,
            path,
            replace,
        } => {
            let temporary = writable(&root, &temporary)?;
            let path = writable(&root, &path)?;
            if temporary.parent() != path.parent() || temporary == path {
                return Err(RemoteError::new(
                    ErrorKind::InvalidInput,
                    "Transfer must commit a separate sibling entry",
                ));
            }
            if replace {
                fs::rename(temporary, path)?;
            } else {
                super::filesystem::rename_no_replace(&temporary, &path)?;
            }
            Ok(Response::Unit)
        }
        Request::CreateSymlink { root, path, target } => {
            std::os::unix::fs::symlink(target, writable(&root, &path)?)?;
            Ok(Response::Unit)
        }
        Request::RemoveEmptyDirectory { root, path } => {
            fs::remove_dir(writable(&root, &path)?)?;
            Ok(Response::Unit)
        }
        _ => Err(RemoteError::new(
            ErrorKind::InvalidInput,
            "Not a transfer request",
        )),
    }
}

#[cfg(target_os = "macos")]
fn no_follow() -> i32 {
    0x100
}
#[cfg(target_os = "linux")]
fn no_follow() -> i32 {
    0x20000
}

#[cfg(target_os = "macos")]
fn non_blocking() -> i32 {
    0x4
}
#[cfg(target_os = "linux")]
fn non_blocking() -> i32 {
    0x800
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "bed-transfers-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self(root.canonicalize().unwrap())
        }
        fn root(&self) -> String {
            self.0.to_str().unwrap().into()
        }
        fn path(&self, name: &str) -> String {
            self.0.join(name).to_str().unwrap().into()
        }
        fn write(
            &self,
            name: &str,
            offset: u64,
            bytes: Vec<u8>,
            create: bool,
        ) -> Result<Response, RemoteError> {
            call(Request::WriteFileChunk {
                root: self.root(),
                path: self.path(name),
                offset,
                bytes,
                create,
                mode: Some(0o640),
            })
        }
        fn read(&self, name: &str, offset: u64, max_bytes: usize) -> Result<Vec<u8>, RemoteError> {
            match call(Request::ReadFileChunk {
                root: self.root(),
                path: self.path(name),
                offset,
                max_bytes,
            })? {
                Response::FileChunk { bytes } => Ok(bytes),
                response => panic!("unexpected response: {response:?}"),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn chunks_are_bounded_and_do_not_use_the_editor_file_size_limit() {
        let fx = Fixture::new();
        let mut file = fs::File::create(fx.path("large")).unwrap();
        file.set_len(MAX_FILE_BYTES as u64 + 4).unwrap();
        file.seek(SeekFrom::End(-4)).unwrap();
        file.write_all(b"tail").unwrap();
        assert_eq!(
            fx.read("large", 0, usize::MAX).unwrap().len(),
            TRANSFER_CHUNK_BYTES
        );
        assert_eq!(
            fx.read("large", MAX_FILE_BYTES as u64, usize::MAX).unwrap(),
            b"tail"
        );
        assert!(
            fx.read("large", MAX_FILE_BYTES as u64 + 4, 10)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            fx.write("oversized", 0, vec![0; TRANSFER_CHUNK_BYTES + 1], true)
                .unwrap_err()
                .kind,
            ErrorKind::TooLarge
        );
        assert!(!PathBuf::from(fx.path("oversized")).exists());
    }

    #[test]
    fn writes_reject_holes_repeated_chunks_and_existing_files() {
        let fx = Fixture::new();
        assert_eq!(
            fx.write("invalid", 1, vec![1], true).unwrap_err().kind,
            ErrorKind::InvalidInput
        );
        assert!(!PathBuf::from(fx.path("invalid")).exists());
        fx.write("temporary", 0, b"first".to_vec(), true).unwrap();
        assert_eq!(
            fx.write("temporary", 0, b"overwrite".to_vec(), true)
                .unwrap_err()
                .kind,
            ErrorKind::AlreadyExists
        );
        assert_eq!(
            fx.write("temporary", 0, b"repeat".to_vec(), false)
                .unwrap_err()
                .kind,
            ErrorKind::Conflict
        );
        assert_eq!(
            fx.write("temporary", 8, b"hole".to_vec(), false)
                .unwrap_err()
                .kind,
            ErrorKind::Conflict
        );
        assert_eq!(fs::read(fx.path("temporary")).unwrap(), b"first");
        fx.write("temporary", 5, b" second".to_vec(), false)
            .unwrap();
        assert_eq!(fs::read(fx.path("temporary")).unwrap(), b"first second");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(fx.path("temporary"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
    }

    #[test]
    fn committing_without_replace_never_overwrites_and_replace_is_explicit() {
        let fx = Fixture::new();
        fs::write(fx.path("destination"), b"old").unwrap();
        fx.write("temporary", 0, b"new".to_vec(), true).unwrap();
        let commit = |replace| {
            call(Request::CommitFileTransfer {
                root: fx.root(),
                temporary: fx.path("temporary"),
                path: fx.path("destination"),
                replace,
            })
        };
        assert_eq!(commit(false).unwrap_err().kind, ErrorKind::AlreadyExists);
        assert_eq!(fs::read(fx.path("temporary")).unwrap(), b"new");
        assert_eq!(fs::read(fx.path("destination")).unwrap(), b"old");
        commit(true).unwrap();
        assert_eq!(fs::read(fx.path("destination")).unwrap(), b"new");
        assert!(!PathBuf::from(fx.path("temporary")).exists());
    }

    #[test]
    fn transfer_paths_are_confined_and_links_are_never_followed() {
        let fx = Fixture::new();
        let outside = Fixture::new();
        fs::write(outside.path("protected"), b"keep").unwrap();
        std::os::unix::fs::symlink(&outside.0, fx.path("escape")).unwrap();
        std::os::unix::fs::symlink(outside.path("protected"), fx.path("link")).unwrap();
        assert_eq!(
            fx.write("escape/new", 0, vec![1], true).unwrap_err().kind,
            ErrorKind::PermissionDenied
        );
        assert_eq!(
            fx.read("escape/protected", 0, 10).unwrap_err().kind,
            ErrorKind::PermissionDenied
        );
        assert!(fx.write("link", 0, vec![1], false).is_err());
        assert!(fx.read("link", 0, 10).is_err());
        assert_eq!(fs::read(outside.path("protected")).unwrap(), b"keep");
        let response = call(Request::TransferStat {
            root: fx.root(),
            path: fx.path("link"),
        })
        .unwrap();
        let Response::TransferStat { entry } = response else {
            panic!("wrong stat response")
        };
        assert!(entry.is_symlink);
        assert!(!entry.is_directory);
        assert_eq!(
            entry.symlink_target.as_deref(),
            Some(outside.path("protected").as_str())
        );
        assert_eq!(
            call(Request::WriteFileChunk {
                root: fx.root(),
                path: fx.root(),
                offset: 0,
                bytes: vec![],
                create: true,
                mode: None
            })
            .unwrap_err()
            .kind,
            ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn empty_directory_cleanup_cannot_remove_new_contents_or_the_root() {
        let fx = Fixture::new();
        fs::create_dir(fx.path("directory")).unwrap();
        fs::write(fx.path("directory/new"), b"keep").unwrap();
        let remove = |path| {
            call(Request::RemoveEmptyDirectory {
                root: fx.root(),
                path,
            })
        };
        assert!(remove(fx.path("directory")).is_err());
        assert_eq!(fs::read(fx.path("directory/new")).unwrap(), b"keep");
        assert_eq!(
            remove(fx.root()).unwrap_err().kind,
            ErrorKind::PermissionDenied
        );
        fs::remove_file(fx.path("directory/new")).unwrap();
        remove(fx.path("directory")).unwrap();
        assert!(!PathBuf::from(fx.path("directory")).exists());
    }

    #[test]
    fn commits_require_separate_siblings_and_links_preserve_the_target() {
        let fx = Fixture::new();
        fs::create_dir(fx.path("other")).unwrap();
        fx.write("temporary", 0, b"new".to_vec(), true).unwrap();
        assert_eq!(
            call(Request::CommitFileTransfer {
                root: fx.root(),
                temporary: fx.path("temporary"),
                path: fx.path("other/destination"),
                replace: false
            })
            .unwrap_err()
            .kind,
            ErrorKind::InvalidInput
        );
        assert_eq!(
            call(Request::CommitFileTransfer {
                root: fx.root(),
                temporary: fx.path("temporary"),
                path: fx.path("temporary"),
                replace: true
            })
            .unwrap_err()
            .kind,
            ErrorKind::InvalidInput
        );
        call(Request::CreateSymlink {
            root: fx.root(),
            path: fx.path("dangling"),
            target: "../absent".into(),
        })
        .unwrap();
        assert_eq!(
            fs::read_link(fx.path("dangling")).unwrap(),
            PathBuf::from("../absent")
        );
        assert!(
            fs::symlink_metadata(fx.path("dangling"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn cross_device_errors_survive_the_remote_protocol() {
        let remote = RemoteError::from(std::io::Error::from(std::io::ErrorKind::CrossesDevices));
        let bytes = serde_json::to_vec(&remote).unwrap();
        let decoded: RemoteError = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded.kind, ErrorKind::CrossesDevices);
        assert_eq!(decoded.into_io().kind(), std::io::ErrorKind::CrossesDevices);
    }
}
