use crate::*;
use std::{
    fs::{self, File, OpenOptions},
    io::{SeekFrom, Write},
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Clone, Copy, Debug, Default)]
pub struct LocalFilesystem;

fn metadata(value: &fs::Metadata) -> Metadata {
    let kind = if value.is_file() {
        EntryKind::File
    } else if value.is_dir() {
        EntryKind::Directory
    } else if value.file_type().is_symlink() {
        EntryKind::Symlink
    } else {
        EntryKind::Other
    };
    let modified = value.modified().ok();
    let mut token = format!("{:?}:{}:{modified:?}", kind, value.len()).into_bytes();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        for field in [
            value.dev(),
            value.ino(),
            value.ctime() as u64,
            value.ctime_nsec() as u64,
        ] {
            token.extend_from_slice(&field.to_le_bytes());
        }
    }
    Metadata {
        kind,
        len: value.len(),
        modified,
        readonly: value.permissions().readonly(),
        version: FileVersion::new(token),
    }
}

struct LocalReader {
    file: File,
    path: PathBuf,
    observed: Metadata,
}
impl Read for LocalReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.file.read(bytes)
    }
}
impl Seek for LocalReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.file.seek(position)
    }
}
impl FileReader for LocalReader {
    fn metadata(&self) -> &Metadata {
        &self.observed
    }
    fn verify(&self) -> Result<()> {
        if metadata(&self.file.metadata()?).version != self.observed.version
            || metadata(&fs::metadata(&self.path)?).version != self.observed.version
        {
            return Err(Error::Changed);
        }
        Ok(())
    }
}

impl FilesystemBackend for LocalFilesystem {
    fn canonicalize(&self, path: &Path) -> Result<PathBuf> {
        Ok(fs::canonicalize(path)?)
    }
    fn metadata(&self, path: &Path) -> Result<Metadata> {
        Ok(metadata(&fs::metadata(path)?))
    }
    fn entry_metadata(&self, path: &Path) -> Result<Metadata> {
        Ok(metadata(&fs::symlink_metadata(path)?))
    }
    fn read_directory(
        &self,
        path: &Path,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<DirectoryEntry>> {
        // Filetree passes resolved directories. Reject replaced symlinks and
        // symlink ancestors before enumeration, preserving its root boundary.
        let value = fs::symlink_metadata(path)?;
        if !value.is_dir() || value.file_type().is_symlink() || fs::canonicalize(path)? != path {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "Directory was replaced by a non-directory or symbolic link",
            )
            .into());
        }
        let mut entries = Vec::new();
        for entry in fs::read_dir(path)? {
            if cancelled() {
                return Err(
                    io::Error::new(io::ErrorKind::Interrupted, "Directory read cancelled").into(),
                );
            }
            let entry = entry?;
            let kind = entry.file_type()?;
            let kind = if kind.is_symlink() {
                EntryKind::Symlink
            } else if kind.is_dir() {
                EntryKind::Directory
            } else if kind.is_file() {
                EntryKind::File
            } else {
                EntryKind::Other
            };
            let name = entry.file_name();
            let hidden = name.as_encoded_bytes().first() == Some(&b'.');
            #[cfg(windows)]
            let hidden = {
                use std::os::windows::fs::MetadataExt;
                hidden || entry.metadata()?.file_attributes() & 0x2 != 0
            };
            entries.push(DirectoryEntry {
                path: entry.path(),
                name,
                kind,
                hidden,
            });
        }
        Ok(entries)
    }
    fn open(&self, path: &Path) -> Result<Box<dyn FileReader>> {
        // Reject special files before open, and use a nonblocking descriptor so
        // replacement with a FIFO/device cannot block a worker during opening.
        if !fs::metadata(path)?.is_file() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Not a regular file").into());
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = options.open(path)?;
        let observed = metadata(&file.metadata()?);
        if !observed.is_file() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Not a regular file").into());
        }
        let reader = LocalReader {
            file,
            path: path.to_owned(),
            observed,
        };
        reader.verify()?;
        Ok(Box::new(reader))
    }
    fn prepare_write(&self, path: &Path, content: &mut dyn Read) -> Result<Box<dyn PendingWrite>> {
        let target = match fs::canonicalize(path) {
            Ok(target) => target,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let name = path.file_name().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "Missing filename")
                })?;
                fs::canonicalize(parent_directory(path))?.join(name)
            }
            Err(error) => return Err(error.into()),
        };
        let original = match fs::metadata(&target) {
            Ok(value) => {
                if !value.is_file() || value.permissions().readonly() {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "File is read-only or is not a regular file",
                    )
                    .into());
                }
                Some(value)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let (temporary, mut file) = loop {
            let temporary = target.parent().unwrap().join(format!(
                ".bedtime-write-{}-{}.tmp",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                Ok(file) => break (temporary, file),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        };
        let mut pending = LocalWrite {
            path: path.to_owned(),
            target,
            temporary,
            identity: file.metadata()?,
            version: None,
        };
        if let Some(original) = original {
            file.set_permissions(original.permissions())?;
        }
        io::copy(content, &mut file)?;
        file.flush()?;
        file.sync_all()?;
        // ctime changes with writes/permissions. Record identity after staging.
        pending.version = Some(metadata(&file.metadata()?).version);
        drop(file);
        Ok(Box::new(pending))
    }
}

struct LocalWrite {
    path: PathBuf,
    target: PathBuf,
    temporary: PathBuf,
    identity: fs::Metadata,
    version: Option<FileVersion>,
}
impl PendingWrite for LocalWrite {
    fn commit(self: Box<Self>, expected: WriteExpectation<'_>) -> Result<Metadata> {
        if Some(metadata(&fs::symlink_metadata(&self.temporary)?).version) != self.version {
            return Err(Error::Changed);
        }
        match expected {
            WriteExpectation::Contents(expected) => {
                if fs::canonicalize(&self.path)? != self.target {
                    return Err(Error::Changed);
                }
                let mut reader = LocalFilesystem.open(&self.path)?;
                if reader.metadata().readonly {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "File is read-only",
                    )
                    .into());
                }
                if !equal_bytes(&mut reader, expected)? {
                    return Err(Error::Changed);
                }
                reader.verify()?;
                if fs::canonicalize(&self.path)? != self.target {
                    return Err(Error::Changed);
                }
                if Some(metadata(&fs::symlink_metadata(&self.temporary)?).version) != self.version {
                    return Err(Error::Changed);
                }
                // Ordinary filesystems do not provide content CAS. Another
                // process can still write between this comparison and rename.
                fs::rename(&self.temporary, &self.target)?;
            }
            WriteExpectation::Missing => {
                let parent = fs::canonicalize(parent_directory(&self.path))?;
                if parent.join(self.path.file_name().unwrap()) != self.target {
                    return Err(Error::Changed);
                }
                if Some(metadata(&fs::symlink_metadata(&self.temporary)?).version) != self.version {
                    return Err(Error::Changed);
                }
                // Share the native no-replace primitive with directory operations.
                // Rename retires staging before the receipt is observed, avoiding
                // a later temporary unlink changing the returned source version.
                crate::operations::rename_no_replace(&self.temporary, &self.target)?;
            }
        }
        // Observe the committed object via its target. Failure here follows a
        // possible commit; callers must refresh before retrying any failed save.
        let current = fs::metadata(&self.target).map_err(Error::CommitUncertain)?;
        if !same_object(&current, &self.identity) {
            return Err(Error::CommitUncertain(io::Error::other(
                "Committed destination changed",
            )));
        }
        Ok(metadata(&current))
    }
}
impl Drop for LocalWrite {
    fn drop(&mut self) {
        // Never remove an unrelated file adopted under the staging name. Compare
        // inode identity, not version: writes legitimately changed the ctime.
        if let Ok(current) = fs::symlink_metadata(&self.temporary) {
            if same_object(&current, &self.identity) {
                let _ = fs::remove_file(&self.temporary);
            }
        }
    }
}

fn same_object(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        left.dev() == right.dev() && left.ino() == right.ino()
    }
    #[cfg(not(unix))]
    {
        left.created()
            .ok()
            .zip(right.created().ok())
            .is_some_and(|(a, b)| a == b)
    }
}

fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}
