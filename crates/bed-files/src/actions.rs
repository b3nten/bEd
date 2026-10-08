//! Filesystem actions for standalone Bed. UI and open-document rebinding are callers' responsibilities.
use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};

pub fn validate_name(name: &str) -> io::Result<()> {
    if name.trim().is_empty()
        || name.contains(['/', '\\', '\0'])
        || !matches!(
            Path::new(name).components().next(),
            Some(Component::Normal(_))
        )
        || Path::new(name).components().count() != 1
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Enter one file or folder name",
        ));
    }
    Ok(())
}
pub fn create_file(directory: &Path, name: &str) -> io::Result<PathBuf> {
    validate_name(name)?;
    let path = directory.join(name);
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    Ok(path)
}
pub fn create_folder(directory: &Path, name: &str) -> io::Result<PathBuf> {
    validate_name(name)?;
    let path = directory.join(name);
    fs::create_dir(&path)?;
    Ok(path)
}
/// Resolve the parent only: destructive operations must affect a symlink entry, not its target.
pub fn validate_project_entry(root: &Path, path: &Path) -> io::Result<PathBuf> {
    let root = fs::canonicalize(root)?;
    let parent =
        fs::canonicalize(path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Missing parent directory")
        })?)?;
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "The project root cannot be changed",
        )
    })?;
    let entry = parent.join(name);
    if entry == root || !entry.starts_with(&root) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Choose an entry inside the project",
        ));
    }
    fs::symlink_metadata(&entry)?;
    Ok(entry)
}
pub fn rename_entry(source: &Path, name: &str) -> io::Result<PathBuf> {
    validate_name(name)?;
    let target = source.with_file_name(name);
    if source == target {
        return Ok(target);
    }
    // On case-insensitive filesystems a differently cased spelling can name
    // the same entry. Use a private intermediate name, preserving no-replace
    // semantics at both steps and rolling back a failed second rename.
    if case_alias(source, &target) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_RENAME: AtomicU64 = AtomicU64::new(1);
        let temporary = source.with_file_name(format!(
            ".bed-rename-{}-{}",
            std::process::id(),
            NEXT_RENAME.fetch_add(1, Ordering::Relaxed)
        ));
        rename_no_replace(source, &temporary)?;
        if let Err(error) = rename_no_replace(&temporary, &target) {
            rename_no_replace(&temporary, source).map_err(|rollback| {
                io::Error::other(format!(
                    "Rename failed: {error}; restoring {} failed: {rollback}. File remains at {}",
                    source.display(),
                    temporary.display()
                ))
            })?;
            return Err(error);
        }
    } else {
        rename_no_replace(source, &target)?;
    }
    Ok(target)
}
fn case_alias(source: &Path, target: &Path) -> bool {
    let (Some(from), Some(to)) = (
        source.file_name().and_then(|s| s.to_str()),
        target.file_name().and_then(|s| s.to_str()),
    ) else {
        return false;
    };
    if from == to {
        return false;
    }
    // A distinct hard link with a case-only name must remain an existing entry.
    // Reading the directory's actual spelling distinguishes it from an alias.
    if std::fs::read_dir(source.parent().unwrap_or_else(|| Path::new("."))).is_ok_and(|entries| {
        entries
            .filter_map(Result::ok)
            .any(|entry| entry.file_name() == to)
    }) {
        return false;
    }
    use std::os::unix::fs::MetadataExt;
    match (fs::symlink_metadata(source), fs::symlink_metadata(target)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}
pub fn rename_no_replace(source: &Path, target: &Path) -> io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let from = CString::new(source.as_os_str().as_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let to = CString::new(target.as_os_str().as_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    #[cfg(target_os = "macos")]
    // SAFETY: Both paths are valid nul-terminated strings; no handles escape.
    let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
    #[cfg(target_os = "linux")]
    // SAFETY: renameat2 receives valid strings and the no-replace flag.
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            1_u32,
        ) as i32
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    #[test]
    fn create_and_rename_never_replace_existing_entries() {
        let temp = TempDir::new();
        let source = create_file(temp.root(), "source.rs").unwrap();
        temp.write("target.rs", b"preserve");
        assert!(create_file(temp.root(), "target.rs").is_err());
        assert!(rename_entry(&source, "target.rs").is_err());
        assert_eq!(fs::read(temp.path("target.rs")).unwrap(), b"preserve");
        let folder = create_folder(temp.root(), "folder").unwrap();
        assert!(folder.is_dir());
        assert!(create_folder(temp.root(), "folder").is_err());
        assert!(rename_entry(&source, "renamed.rs").unwrap().is_file());
    }
    #[test]
    fn rejects_traversal_and_root_mutations() {
        for name in ["", " ", ".", "..", "../outside", "a/b", "a\\b"] {
            assert!(validate_name(name).is_err());
        }
        assert!(validate_name("日本語.rs").is_ok());
        let temp = TempDir::new();
        assert!(validate_project_entry(temp.root(), temp.root()).is_err());
    }
    #[test]
    fn case_only_rename_keeps_contents_and_requested_directory_spelling() {
        let temp = TempDir::new();
        let source = temp.write("Mixed.rs", b"preserve");
        let renamed = rename_entry(&source, "mixed.rs").unwrap();
        assert_eq!(fs::read(&renamed).unwrap(), b"preserve");
        assert!(
            fs::read_dir(temp.root())
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| entry.file_name() == "mixed.rs")
        );
    }
    #[cfg(unix)]
    #[test]
    fn differently_cased_distinct_hard_link_is_never_replaced() {
        let temp = TempDir::new();
        let source = temp.write("File", b"preserve");
        let target = temp.path("file");
        if fs::hard_link(&source, &target).is_ok() {
            assert!(rename_entry(&source, "file").is_err());
            assert!(source.exists());
            assert!(target.exists());
        }
    }
    #[cfg(unix)]
    #[test]
    fn symlink_entry_validation_keeps_the_target_untouched() {
        let temp = TempDir::new();
        let outside = TempDir::new();
        outside.write("target", b"keep");
        std::os::unix::fs::symlink(outside.path("target"), temp.path("link")).unwrap();
        let entry = validate_project_entry(temp.root(), &temp.path("link")).unwrap();
        assert!(
            fs::symlink_metadata(&entry)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        rename_entry(&entry, "renamed-link").unwrap();
        assert_eq!(fs::read(outside.path("target")).unwrap(), b"keep");
    }
}
