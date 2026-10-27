//! Worker-only local filesystem mutations. Paths stay native; only error messages
//! use display formatting. Canonical parents establish the root boundary, while
//! final symlinks are entries to copy/move/trash, never directories to traverse.
//!
//! This is not a sandbox against another process concurrently replacing ancestor
//! directories. In particular, `trash` owns its own path-based implementation.
//! Source identities/versions and staging ownership are revalidated before copy
//! commit, and cleanup refuses a replaced temporary. Validation is not a locked
//! filesystem snapshot: writes after the final check can still race with commit,
//! and externally relocated staging directories may be left for manual cleanup.
//! Copies are staged and atomically committed without replacement. Cross-device
//! moves deliberately fail rather than implementing a lossy copy-and-delete.
use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    fs::{self, Metadata, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Clone, Debug)]
pub enum Operation {
    CreateFile {
        directory: PathBuf,
        name: OsString,
    },
    CreateDirectory {
        directory: PathBuf,
        name: OsString,
    },
    Rename {
        path: PathBuf,
        name: OsString,
    },
    Copy {
        sources: Vec<PathBuf>,
        directory: PathBuf,
    },
    Move {
        sources: Vec<PathBuf>,
        directory: PathBuf,
    },
    Duplicate {
        sources: Vec<PathBuf>,
    },
    Trash {
        sources: Vec<PathBuf>,
    },
}

#[derive(Clone, Debug)]
pub struct OperationRequest {
    pub id: u64,
    pub root: PathBuf,
    pub operation: Operation,
}

#[derive(Clone, Debug)]
pub enum Change {
    Created {
        path: PathBuf,
    },
    Copied {
        source: PathBuf,
        destination: PathBuf,
    },
    Moved {
        source: PathBuf,
        destination: PathBuf,
    },
    Trashed {
        path: PathBuf,
    },
}

#[derive(Clone, Debug)]
pub struct OperationError {
    pub path: PathBuf,
    /// Portable classification for human-facing messages; message retains the
    /// original diagnostic rather than forcing UI code to parse OS error text.
    pub kind: io::ErrorKind,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct OperationResult {
    pub id: u64,
    pub completed: Vec<Change>,
    pub errors: Vec<OperationError>,
    pub cancelled: bool,
}

/// Items count committed top-level changes. Bytes count streamed copy data,
/// including data subsequently discarded if that copy fails or is cancelled.
#[derive(Clone, Debug, Default)]
pub struct OperationProgress {
    pub completed_items: usize,
    pub bytes: u64,
    pub current_path: Option<PathBuf>,
}

pub fn execute(
    request: OperationRequest,
    cancelled: impl Fn() -> bool,
    mut progress: impl FnMut(OperationProgress),
) -> OperationResult {
    let mut result = OperationResult {
        id: request.id,
        completed: Vec::new(),
        errors: Vec::new(),
        cancelled: false,
    };
    if cancelled() {
        result.cancelled = true;
        return result;
    }
    let root = match fs::canonicalize(&request.root).and_then(|root| {
        if root.is_dir() {
            Ok(root)
        } else {
            Err(invalid("The file tree root must be an existing directory"))
        }
    }) {
        Ok(root) => root,
        Err(error) => {
            result.errors.push(OperationError {
                path: request.root,
                kind: error.kind(),
                message: error.to_string(),
            });
            return result;
        }
    };
    let _lease=match crate::WriteAccess::local().repository(&root) {Ok(lease)=>lease,Err(message)=>{result.errors.push(OperationError {path:root,kind:io::ErrorKind::WouldBlock,message});return result;}};
    let mut execution = Execution {
        root,
        cancelled: &cancelled,
        report: &mut progress,
        progress: OperationProgress::default(),
        result,
    };
    execution.run(request.operation);
    execution.result.cancelled |= cancelled();
    for change in &execution.result.completed {match change {Change::Moved {source,destination}=>crate::Filesystem::local().move_document_sources(source,destination),Change::Trashed {path}=>crate::Filesystem::local().delete_document_sources(path),_=>{}}}
    execution.result
}

/// Lexical ancestor/duplicate elimination, preserving input order and the first
/// spelling of duplicate entries. No I/O: a final symlink must remain a link.
/// Sorted component order puts each ancestor immediately before its descendants,
/// so filtering needs one prefix check per unique path, not a pairwise scan:
/// O(n log n) ordered-set work plus a linear pass over path components.
pub fn top_level_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
    let sorted: BTreeSet<&Path> = paths.iter().map(PathBuf::as_path).collect();
    let mut roots = BTreeSet::new();
    let mut previous: Option<&Path> = None;
    for path in sorted {
        if previous.is_some_and(|ancestor| path.starts_with(ancestor)) {
            continue;
        }
        roots.insert(path);
        previous = Some(path);
    }
    paths
        .iter()
        .filter(|path| roots.remove(path.as_path()))
        .cloned()
        .collect()
}

struct Execution<'a> {
    root: PathBuf,
    cancelled: &'a dyn Fn() -> bool,
    report: &'a mut dyn FnMut(OperationProgress),
    progress: OperationProgress,
    result: OperationResult,
}

impl Execution<'_> {
    fn check_cancelled(&self) -> io::Result<()> {
        if (self.cancelled)() {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Operation cancelled",
            ))
        } else {
            Ok(())
        }
    }

    fn current(&mut self, path: &Path) {
        self.progress.current_path = Some(path.to_path_buf());
        (self.report)(self.progress.clone());
    }

    fn completed(&mut self, change: Change) {
        self.result.completed.push(change);
        self.progress.completed_items = self.result.completed.len();
        (self.report)(self.progress.clone());
    }

    fn error(&mut self, path: &Path, error: io::Error) {
        if error.kind() == io::ErrorKind::Interrupted && (self.cancelled)() {
            self.result.cancelled = true;
        } else {
            self.result.errors.push(OperationError {
                path: path.to_path_buf(),
                kind: error.kind(),
                message: error.to_string(),
            });
        }
    }

    fn absolute(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        }
    }

    fn directory(&self, path: &Path) -> io::Result<PathBuf> {
        let directory = fs::canonicalize(self.absolute(path))?;
        if !directory.starts_with(&self.root) {
            return Err(invalid(
                "Destination directory is outside the file tree root",
            ));
        }
        if !fs::metadata(&directory)?.is_dir() {
            return Err(invalid("Destination must be an existing directory"));
        }
        Ok(directory)
    }

    /// Canonicalize only the parent, preserving dangling and outward symlinks.
    fn source(&self, path: &Path, external: bool) -> io::Result<PathBuf> {
        let absolute = self.absolute(path);
        let name = absolute
            .file_name()
            .ok_or_else(|| invalid("Cannot operate on a filesystem root"))?;
        let parent = absolute
            .parent()
            .ok_or_else(|| invalid("Source has no parent"))?;
        let path = fs::canonicalize(parent)?.join(name);
        if path == self.root {
            return Err(invalid("Cannot operate on the file tree root"));
        }
        if !external && !path.starts_with(&self.root) {
            return Err(invalid("Source is outside the file tree root"));
        }
        fs::symlink_metadata(&path)?;
        Ok(path)
    }

    fn run(&mut self, operation: Operation) {
        match operation {
            Operation::CreateFile { directory, name } => {
                let outcome = self.create(&directory, &name, false);
                self.finish_one(&directory.join(&name), outcome);
            }
            Operation::CreateDirectory { directory, name } => {
                let outcome = self.create(&directory, &name, true);
                self.finish_one(&directory.join(&name), outcome);
            }
            Operation::Rename { path, name } => {
                let outcome = self.rename(&path, &name);
                self.finish_one(&path, outcome);
            }
            Operation::Copy { sources, directory } => {
                self.transfer(sources, Some(directory), Transfer::Copy);
            }
            Operation::Move { sources, directory } => {
                self.transfer(sources, Some(directory), Transfer::Move);
            }
            Operation::Duplicate { sources } => {
                self.transfer(sources, None, Transfer::Copy);
            }
            Operation::Trash { sources } => {
                for path in top_level_paths(&sources) {
                    if (self.cancelled)() {
                        break;
                    }
                    let outcome = (|| {
                        let path = self.source(&path, false)?;
                        self.current(&path);
                        self.check_cancelled()?;
                        self.source(&path, false)?;
                        // Do not canonicalize the final component: deleting a link
                        // to an outside directory must only trash the link itself.
                        trash::delete(&path).map_err(io::Error::other)?;
                        Ok(Some(Change::Trashed { path }))
                    })();
                    self.finish_one(&path, outcome);
                }
            }
        }
    }

    fn finish_one(&mut self, path: &Path, outcome: io::Result<Option<Change>>) {
        match outcome {
            Ok(Some(change)) => self.completed(change),
            Ok(None) => {}
            Err(error) => self.error(path, error),
        }
    }

    fn create(
        &mut self,
        directory: &Path,
        name: &OsStr,
        is_directory: bool,
    ) -> io::Result<Option<Change>> {
        validate_name(name)?;
        let path = self.directory(directory)?.join(name);
        self.current(&path);
        self.check_cancelled()?;
        self.directory(path.parent().expect("destination parent"))?;
        if is_directory {
            fs::create_dir(&path)?;
        } else {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
        }
        Ok(Some(Change::Created { path }))
    }

    fn rename(&mut self, path: &Path, name: &OsStr) -> io::Result<Option<Change>> {
        validate_name(name)?;
        let source = self.source(path, false)?;
        let parent = self.directory(source.parent().expect("validated source parent"))?;
        let destination = parent.join(name);
        if source == destination {
            return Ok(None);
        }
        self.current(&source);
        self.check_cancelled()?;
        self.relocate(&source, &destination)?;
        Ok(Some(Change::Moved {
            source,
            destination,
        }))
    }

    fn transfer(&mut self, sources: Vec<PathBuf>, directory: Option<PathBuf>, kind: Transfer) {
        let destination = match directory
            .as_deref()
            .map(|directory| self.directory(directory))
            .transpose()
        {
            Ok(destination) => destination,
            Err(error) => {
                self.error(
                    directory.as_deref().expect("destination was supplied"),
                    error,
                );
                return;
            }
        };
        for path in top_level_paths(&sources) {
            if (self.cancelled)() {
                break;
            }
            let outcome = (|| {
                // Only Copy with an explicit destination accepts external sources.
                let source = self.source(&path, kind == Transfer::Copy && destination.is_some())?;
                let parent = source.parent().expect("validated source parent");
                let directory = match &destination {
                    Some(directory) => self.directory(directory)?,
                    None => self.directory(parent)?,
                };
                let metadata = fs::symlink_metadata(&source)?;
                if metadata.is_dir() && directory.starts_with(fs::canonicalize(&source)?) {
                    return Err(invalid(
                        "Cannot copy or move a directory into itself or a descendant",
                    ));
                }
                self.current(&source);
                self.check_cancelled()?;
                match kind {
                    Transfer::Copy => {
                        let keep_both = parent == directory.as_path();
                        let destination =
                            self.copy(&source, &directory, keep_both, metadata.is_dir())?;
                        Ok(Some(Change::Copied {
                            source,
                            destination,
                        }))
                    }
                    Transfer::Move => {
                        let destination =
                            directory.join(source.file_name().expect("validated source name"));
                        if source == destination {
                            return Ok(None);
                        }
                        self.relocate(&source, &destination)?;
                        Ok(Some(Change::Moved {
                            source,
                            destination,
                        }))
                    }
                }
            })();
            self.finish_one(&path, outcome);
        }
    }

    fn copy(
        &mut self,
        source: &Path,
        directory: &Path,
        keep_both: bool,
        is_directory: bool,
    ) -> io::Result<PathBuf> {
        if !cfg!(any(target_os = "macos", target_os = "linux")) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Atomic no-replace copy commit is unsupported on this platform",
            ));
        }
        self.directory(directory)?;
        let name = source.file_name().expect("validated source name");
        let destination = directory.join(name);
        if !keep_both && entry_exists(&destination)? {
            return Err(conflict());
        }
        let mut staging = Staging::new(directory)?;
        let payload = staging.path.join("item");
        let outcome = (|| {
            // Keep observations until commit: an open file handle can remain
            // unchanged even after its source pathname is replaced or removed.
            let mut observations = Vec::new();
            self.copy_entry(source, &payload, 0, &mut observations)?;
            let mut number = 1u64;
            loop {
                self.check_cancelled()?;
                // Revalidate both boundaries and every observed source entry
                // immediately before commit, including directory membership
                // changes (mtime/ctime), removed files and replaced symlinks.
                if self.directory(directory)? != directory || self.source(source, true)? != source {
                    return Err(invalid("Source or destination parent changed during copy"));
                }
                staging.validate()?;
                for (path, before) in &observations {
                    self.check_cancelled()?;
                    if !same_version(before, &fs::symlink_metadata(path)?)? {
                        return Err(invalid(
                            "Source changed before commit; staged data was discarded",
                        ));
                    }
                }
                self.check_cancelled()?;
                let destination = if keep_both {
                    directory.join(copy_name(name, is_directory, number))
                } else {
                    destination.clone()
                };
                match rename_no_replace(&payload, &destination) {
                    Ok(()) => return Ok(destination),
                    Err(error) if keep_both && error.kind() == io::ErrorKind::AlreadyExists => {
                        number = number.checked_add(1).ok_or_else(conflict)?;
                    }
                    Err(error) => return Err(error),
                }
            }
        })();
        // Cleanup failure is independent of whether the payload was committed.
        if let Err(error) = staging.cleanup() {
            self.error(&staging.path, error);
        }
        outcome
    }

    fn copy_entry(
        &mut self,
        source: &Path,
        destination: &Path,
        depth: usize,
        observations: &mut Vec<(PathBuf, Metadata)>,
    ) -> io::Result<()> {
        self.check_cancelled()?;
        if depth > 256 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Copy nesting exceeds 256 directories",
            ));
        }
        self.current(source);
        self.check_cancelled()?;
        let metadata = fs::symlink_metadata(source)?;
        observations.push((source.to_path_buf(), metadata.clone()));
        if metadata.file_type().is_symlink() {
            return copy_symlink(source, destination);
        }
        if metadata.is_dir() {
            fs::create_dir(destination)?;
            for entry in fs::read_dir(source)? {
                self.check_cancelled()?;
                let entry = entry?;
                self.copy_entry(
                    &entry.path(),
                    &destination.join(entry.file_name()),
                    depth + 1,
                    observations,
                )?;
            }
            // Directory modes/ACLs/timestamps are not copied: staged directories
            // must remain writable for cleanup if a later item fails.
            return Ok(());
        }
        if !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Only regular files, directories and symbolic links can be copied",
            ));
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // A final-component swap must not redirect the read or block on a FIFO.
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let mut input = options.open(source)?;
        let before = input.metadata()?;
        if !before.is_file() || !same_version(&metadata, &before)? {
            return Err(invalid("Source changed while opening it for copying"));
        }
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?;
        let mut buffer = [0u8; 64 * 1024];
        let mut remaining = before.len();
        while remaining > 0 {
            self.check_cancelled()?;
            let length = remaining.min(buffer.len() as u64) as usize;
            let read = match input.read(&mut buffer[..length]) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                other => other?,
            };
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Source shrank during copy",
                ));
            }
            output.write_all(&buffer[..read])?;
            remaining -= read as u64;
            self.progress.bytes = self.progress.bytes.saturating_add(read as u64);
            (self.report)(self.progress.clone());
        }
        self.check_cancelled()?;
        let after = input.metadata()?;
        if !same_version(&before, &after)? {
            return Err(invalid(
                "Source changed during copy; staged data was discarded",
            ));
        }
        output.set_permissions(before.permissions())?;
        output.sync_all()?;
        Ok(())
    }

    fn relocate(&mut self, source: &Path, destination: &Path) -> io::Result<()> {
        self.source(source, false)?;
        self.directory(destination.parent().expect("destination parent"))?;
        // Case-insensitive filesystems can resolve two spellings to one entry.
        // Check actual directory names as well as inode identity: two hard links
        // are a conflict, not permission to overwrite one of them.
        if case_alias(source, destination)? {
            return self.rename_case_alias(source, destination);
        }
        rename_no_replace(source, destination).map_err(move_error)
    }

    fn rename_case_alias(&mut self, source: &Path, destination: &Path) -> io::Result<()> {
        let mut staging = Staging::new(source.parent().expect("source parent"))?;
        let payload = staging.path.join("item");
        self.check_cancelled()?;
        rename_no_replace(source, &payload).map_err(move_error)?;
        // Once the source has moved, finish or roll back even if cancellation
        // arrives. Never abandon a user entry inside a disposable temporary.
        match rename_no_replace(&payload, destination) {
            Ok(()) => {
                if let Err(error) = staging.cleanup() {
                    self.error(&staging.path, error);
                }
                Ok(())
            }
            Err(error) => match rename_no_replace(&payload, source) {
                Ok(()) => {
                    if let Err(cleanup) = staging.cleanup() {
                        self.error(&staging.path, cleanup);
                    }
                    Err(error)
                }
                Err(rollback) => {
                    // A concurrent creator can block rollback. Keep the source,
                    // report its actual location, and do NOT recursively clean it.
                    let location = payload.clone();
                    staging.preserve();
                    self.completed(Change::Moved {
                        source: source.to_path_buf(),
                        destination: location.clone(),
                    });
                    Err(io::Error::other(format!(
                        "Rename failed: {error}; rollback failed: {rollback}. Source preserved at {}",
                        location.display()
                    )))
                }
            },
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Transfer {
    Copy,
    Move,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
fn conflict() -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        "Destination already exists; nothing was overwritten",
    )
}

fn validate_name(name: &OsStr) -> io::Result<()> {
    let mut components = Path::new(name).components();
    if name.is_empty()
        || !matches!(components.next(), Some(Component::Normal(component)) if component == name)
        || components.next().is_some()
        || name
            .as_encoded_bytes()
            .iter()
            .any(|byte| matches!(byte, 0 | b'/' | b'\\'))
    {
        return Err(invalid(
            "Name must be one component, not empty, '.', '..', or contain NUL or slashes",
        ));
    }
    Ok(())
}

fn entry_exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn copy_name(name: &OsStr, directory: bool, number: u64) -> OsString {
    let path = Path::new(name);
    let (stem, extension) = if directory {
        (name, None)
    } else {
        (path.file_stem().unwrap_or(name), path.extension())
    };
    let mut result = stem.to_os_string();
    result.push(" copy");
    if number > 1 {
        result.push(format!(" {number}"));
    }
    if let Some(extension) = extension {
        result.push(".");
        result.push(extension);
    }
    result
}

fn same_identity(first: &Metadata, second: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        first.dev() == second.dev() && first.ino() == second.ino()
    }
    #[cfg(not(unix))]
    {
        // No destructive use on platforms without atomic no-replace support.
        let _ = (first, second);
        false
    }
}

/// Identity and observable content/membership changes, deliberately excluding
/// access time (the copy itself can update it). This is validation, not a source
/// snapshot: another writer can still change an entry after its final check.
fn same_version(first: &Metadata, second: &Metadata) -> io::Result<bool> {
    if !same_identity(first, second)
        || first.file_type() != second.file_type()
        || first.len() != second.len()
        || first.modified()? != second.modified()?
    {
        return Ok(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        return Ok(first.ctime() == second.ctime()
            && first.ctime_nsec() == second.ctime_nsec()
            && first.mode() == second.mode());
    }
    #[cfg(not(unix))]
    Ok(true)
}

fn case_alias(source: &Path, destination: &Path) -> io::Result<bool> {
    if source.parent() != destination.parent() || source == destination {
        return Ok(false);
    }
    let destination_metadata = match fs::symlink_metadata(destination) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if !same_identity(&fs::symlink_metadata(source)?, &destination_metadata) {
        return Ok(false);
    }
    let mut source_present = false;
    for entry in fs::read_dir(source.parent().expect("source parent"))? {
        let name = entry?.file_name();
        if Some(name.as_os_str()) == destination.file_name() {
            return Ok(false);
        }
        source_present |= Some(name.as_os_str()) == source.file_name();
    }
    Ok(source_present)
}

fn copy_symlink(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(fs::read_link(source)?, destination)
    }
    #[cfg(not(unix))]
    {
        let _ = (source, destination);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Preserving symbolic links is unsupported on this platform",
        ))
    }
}

fn move_error(error: io::Error) -> io::Error {
    #[cfg(unix)]
    if error.raw_os_error() == Some(libc::EXDEV) {
        return io::Error::new(
            io::ErrorKind::Unsupported,
            "Cross-device moves are unsupported; source was not removed",
        );
    }
    error
}

/// The existence check is never the authority for no-overwrite. Only the OS
/// primitive commits entries. Unsupported kernels/filesystems fail closed.
pub(crate) fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let source = CString::new(source.as_os_str().as_bytes())
            .map_err(|_| invalid("Path contains NUL"))?;
        let destination = CString::new(destination.as_os_str().as_bytes())
            .map_err(|_| invalid("Path contains NUL"))?;
        #[cfg(target_os = "macos")]
        // SAFETY: both C strings live for the call; RENAME_EXCL forbids replacement.
        let status =
            unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
        #[cfg(target_os = "linux")]
        // SAFETY: valid C strings; AT_FDCWD resolves these absolute paths; the
        // kernel, not a prior existence check, enforces RENAME_NOREPLACE.
        let status = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                destination.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if status == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if matches!(
            error.raw_os_error(),
            Some(libc::ENOSYS | libc::EINVAL | libc::EOPNOTSUPP)
        ) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("Atomic no-replace rename is unavailable: {error}"),
            ));
        }
        Err(error)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (source, destination);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Atomic no-replace rename is unsupported on this platform",
        ))
    }
}

static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(1);

/// Exclusively-created sibling container; even a preexisting dangling symlink
/// cannot be adopted as a temporary. The payload shares the destination device.
struct Staging {
    path: PathBuf,
    identity: Metadata,
    cleanup_on_drop: bool,
}
impl Staging {
    fn new(parent: &Path) -> io::Result<Self> {
        for _ in 0..128 {
            let number = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(".bedtime-filetree-{}-{number}", std::process::id()));
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => {
                    let identity = fs::symlink_metadata(&path)?;
                    return Ok(Self {
                        path,
                        identity,
                        cleanup_on_drop: true,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Could not reserve a staging directory",
        ))
    }
    fn validate(&self) -> io::Result<()> {
        let current = fs::symlink_metadata(&self.path)?;
        if !current.is_dir() || !same_identity(&self.identity, &current) {
            return Err(invalid(
                "Staging directory was replaced; replacement left untouched",
            ));
        }
        Ok(())
    }
    fn cleanup(&mut self) -> io::Result<()> {
        // Never remove this name twice, or remove a replacement adopted under
        // our old name. Both commit and cleanup validate the reserved identity.
        self.cleanup_on_drop = false;
        self.validate()?;
        fs::remove_dir_all(&self.path)
    }
    fn preserve(mut self) {
        self.cleanup_on_drop = false;
    }
}
impl Drop for Staging {
    fn drop(&mut self) {
        if self.cleanup_on_drop {
            let _ = self.cleanup();
        }
    }
}

#[cfg(all(test, unix))]
mod native_path_tests {
    use super::*;

    #[test]
    fn native_name_validation_and_keep_both_naming_are_lossless() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let name = OsString::from_vec(b"native-\xff.bin".to_vec());
        validate_name(&name).unwrap();
        assert_eq!(
            PathBuf::from(&name).file_name().unwrap().as_bytes(),
            b"native-\xff.bin"
        );
        assert_eq!(
            copy_name(&name, false, 1).as_bytes(),
            b"native-\xff copy.bin"
        );
        assert_eq!(
            copy_name(&name, false, 2).as_bytes(),
            b"native-\xff copy 2.bin"
        );
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            Self::in_directory(&std::env::temp_dir()).unwrap()
        }
        fn in_directory(parent: &Path) -> io::Result<Self> {
            for _ in 0..128 {
                let number = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
                let path = parent.join(format!(
                    "bedtime-filetree-test-{}-{number}",
                    std::process::id()
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Ok(Self(fs::canonicalize(path)?)),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            }
            Err(conflict())
        }
        fn path(&self, name: impl AsRef<Path>) -> PathBuf {
            self.0.join(name)
        }
        fn write(&self, name: impl AsRef<Path>, bytes: &[u8]) -> PathBuf {
            let path = self.path(name);
            fs::write(&path, bytes).unwrap();
            path
        }
        fn directory(&self, name: &str) -> PathBuf {
            let path = self.path(name);
            fs::create_dir(&path).unwrap();
            path
        }
        fn run(&self, operation: Operation) -> OperationResult {
            execute(self.request(operation), || false, |_| {})
        }
        fn request(&self, operation: Operation) -> OperationRequest {
            OperationRequest {
                id: 42,
                root: self.0.clone(),
                operation,
            }
        }
        fn assert_no_staging(&self) {
            for entry in fs::read_dir(&self.0).unwrap() {
                assert!(
                    !entry
                        .unwrap()
                        .file_name()
                        .as_encoded_bytes()
                        .starts_with(b".bedtime-filetree-")
                );
            }
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn success(result: &OperationResult, count: usize) {
        assert!(result.errors.is_empty(), "{result:?}");
        assert!(!result.cancelled, "{result:?}");
        assert_eq!(result.id, 42);
        assert_eq!(result.completed.len(), count, "{result:?}");
    }
    fn rejected(result: &OperationResult) {
        assert!(!result.errors.is_empty(), "{result:?}");
        assert!(result.completed.is_empty(), "{result:?}");
    }

    #[test]
    fn create_validates_names_and_never_replaces_entries() {
        let temp = Temp::new();
        for name in ["", ".", "..", "a/b", "a\\b", "/absolute", "nul\0name"] {
            rejected(&temp.run(Operation::CreateFile {
                directory: temp.0.clone(),
                name: name.into(),
            }));
        }
        success(
            &temp.run(Operation::CreateFile {
                directory: temp.0.clone(),
                name: "file".into(),
            }),
            1,
        );
        assert_eq!(fs::read(temp.path("file")).unwrap(), b"");
        fs::write(temp.path("file"), b"preserve").unwrap();
        rejected(&temp.run(Operation::CreateFile {
            directory: temp.0.clone(),
            name: "file".into(),
        }));
        rejected(&temp.run(Operation::CreateDirectory {
            directory: temp.0.clone(),
            name: "file".into(),
        }));
        success(
            &temp.run(Operation::CreateDirectory {
                directory: temp.0.clone(),
                name: "folder".into(),
            }),
            1,
        );
        rejected(&temp.run(Operation::CreateDirectory {
            directory: temp.0.clone(),
            name: "folder".into(),
        }));
        assert_eq!(fs::read(temp.path("file")).unwrap(), b"preserve");
    }

    #[test]
    fn root_is_required_and_protected_for_every_source_operation() {
        let temp = Temp::new();
        let target = temp.directory("target");
        let file = temp.write("file", b"data");
        for root in [temp.path("missing"), file] {
            let result = execute(
                OperationRequest {
                    id: 42,
                    root,
                    operation: Operation::CreateFile {
                        directory: temp.0.clone(),
                        name: "uncreated".into(),
                    },
                },
                || false,
                |_| {},
            );
            rejected(&result);
        }
        for operation in [
            Operation::Rename {
                path: temp.0.clone(),
                name: "renamed-root".into(),
            },
            Operation::Copy {
                sources: vec![temp.0.clone()],
                directory: target.clone(),
            },
            Operation::Move {
                sources: vec![temp.0.clone()],
                directory: target.clone(),
            },
            Operation::Duplicate {
                sources: vec![temp.0.clone()],
            },
            Operation::Trash {
                sources: vec![temp.0.clone()],
            },
        ] {
            rejected(&temp.run(operation));
        }
        assert!(temp.0.is_dir());
        assert!(!temp.path("uncreated").exists());
    }

    #[test]
    fn rename_copy_and_move_collisions_preserve_both_entries() {
        let temp = Temp::new();
        let source = temp.write("source.bin", b"source\0\xff");
        let target = temp.directory("target");
        let destination = target.join("source.bin");
        fs::write(&destination, b"target").unwrap();
        let existing = temp.write("existing.bin", b"existing");
        rejected(&temp.run(Operation::Rename {
            path: source.clone(),
            name: "existing.bin".into(),
        }));
        rejected(&temp.run(Operation::Copy {
            sources: vec![source.clone()],
            directory: target.clone(),
        }));
        rejected(&temp.run(Operation::Move {
            sources: vec![source.clone()],
            directory: target,
        }));
        assert_eq!(fs::read(&source).unwrap(), b"source\0\xff");
        assert_eq!(fs::read(destination).unwrap(), b"target");
        assert_eq!(fs::read(existing).unwrap(), b"existing");
        temp.assert_no_staging();
    }

    #[test]
    fn duplicate_and_same_parent_copy_choose_deterministic_keep_both_names() {
        let temp = Temp::new();
        let file = temp.write("image.bin", b"\0\xff\xfe\x80");
        let existing = temp.write("image copy.bin", b"preserve");
        success(
            &temp.run(Operation::Duplicate {
                sources: vec![file.clone()],
            }),
            1,
        );
        success(
            &temp.run(Operation::Copy {
                sources: vec![file],
                directory: temp.0.clone(),
            }),
            1,
        );
        assert_eq!(fs::read(existing).unwrap(), b"preserve");
        assert_eq!(
            fs::read(temp.path("image copy 2.bin")).unwrap(),
            b"\0\xff\xfe\x80"
        );
        assert_eq!(
            fs::read(temp.path("image copy 3.bin")).unwrap(),
            b"\0\xff\xfe\x80"
        );
        let directory = temp.directory("folder.with.dots");
        fs::write(directory.join("child"), b"child").unwrap();
        success(
            &temp.run(Operation::Duplicate {
                sources: vec![directory],
            }),
            1,
        );
        assert_eq!(
            fs::read(temp.path("folder.with.dots copy/child")).unwrap(),
            b"child"
        );
        let hidden = temp.write(".hidden", b"hidden");
        success(
            &temp.run(Operation::Duplicate {
                sources: vec![hidden],
            }),
            1,
        );
        assert_eq!(fs::read(temp.path(".hidden copy")).unwrap(), b"hidden");
        temp.assert_no_staging();
    }

    #[test]
    fn only_copy_accepts_external_sources_and_all_destinations_stay_scoped() {
        let temp = Temp::new();
        let external = Temp::new();
        let source = external.write("incoming", b"incoming");
        success(
            &temp.run(Operation::Copy {
                sources: vec![source.clone()],
                directory: temp.0.clone(),
            }),
            1,
        );
        for operation in [
            Operation::Move {
                sources: vec![source.clone()],
                directory: temp.0.clone(),
            },
            Operation::Duplicate {
                sources: vec![source.clone()],
            },
            Operation::Rename {
                path: source.clone(),
                name: "other".into(),
            },
            Operation::Trash {
                sources: vec![source.clone()],
            },
            Operation::CreateFile {
                directory: external.0.clone(),
                name: "outside".into(),
            },
            Operation::Copy {
                sources: vec![temp.path("incoming")],
                directory: external.0.clone(),
            },
        ] {
            rejected(&temp.run(operation));
        }
        assert_eq!(fs::read(source).unwrap(), b"incoming");
        assert_eq!(fs::read(temp.path("incoming")).unwrap(), b"incoming");
        assert!(!external.path("outside").exists());
    }

    #[test]
    fn descendants_are_deduplicated_even_when_selected_before_their_parent() {
        let temp = Temp::new();
        let directory = temp.directory("directory");
        let child = directory.join("child");
        fs::write(&child, b"child").unwrap();
        let other = temp.write("other", b"other");
        assert_eq!(
            top_level_paths(&[
                child.clone(),
                other.clone(),
                directory.clone(),
                directory.clone()
            ]),
            vec![other, directory.clone()]
        );
        let target = temp.directory("target");
        success(
            &temp.run(Operation::Move {
                sources: vec![child, directory.clone(), directory.clone()],
                directory: target.clone(),
            }),
            1,
        );
        assert!(!directory.exists());
        assert_eq!(fs::read(target.join("directory/child")).unwrap(), b"child");
    }

    #[test]
    fn wide_selection_preserves_order_and_first_duplicate_with_late_ancestors() {
        let mut paths = Vec::new();
        let mut expected = Vec::new();
        // Wide Select All must not do a pairwise scan on the UI thread. Descendants
        // precede their selected ancestors; duplicates follow in another spelling.
        for index in (0..20_000).rev() {
            let parent = PathBuf::from(format!("wide/item-{index:05}"));
            paths.push(parent.join("nested/leaf"));
            let first = if index == 42 {
                parent.join(".")
            } else {
                parent.clone()
            };
            paths.push(first.clone());
            paths.push(parent);
            expected.push(first);
        }
        // A textual prefix is not a component ancestor.
        paths.push(PathBuf::from("wide/item-00042-sibling"));
        expected.push(PathBuf::from("wide/item-00042-sibling"));
        let actual = top_level_paths(&paths);
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(&expected) {
            assert_eq!(actual.as_os_str(), expected.as_os_str());
        }
    }

    #[test]
    fn directory_cannot_be_copied_or_moved_into_itself_or_a_descendant() {
        let temp = Temp::new();
        let directory = temp.directory("directory");
        let child = directory.join("child");
        fs::create_dir(&child).unwrap();
        fs::write(directory.join("data"), b"preserve").unwrap();
        for destination in [directory.clone(), child] {
            rejected(&temp.run(Operation::Copy {
                sources: vec![directory.clone()],
                directory: destination.clone(),
            }));
            rejected(&temp.run(Operation::Move {
                sources: vec![directory.clone()],
                directory: destination,
            }));
        }
        assert_eq!(fs::read(directory.join("data")).unwrap(), b"preserve");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 2);
    }

    #[test]
    fn recursive_copy_streams_binary_data_and_reports_committed_items() {
        let temp = Temp::new();
        let source = temp.directory("source");
        fs::create_dir(source.join("nested")).unwrap();
        let bytes: Vec<_> = (0..200_000).map(|value| (value % 256) as u8).collect();
        fs::write(source.join("nested/data.bin"), &bytes).unwrap();
        fs::write(source.join("empty"), []).unwrap();
        let target = temp.directory("target");
        let mut updates = Vec::new();
        let result = execute(
            temp.request(Operation::Copy {
                sources: vec![source],
                directory: target.clone(),
            }),
            || false,
            |progress| updates.push(progress),
        );
        success(&result, 1);
        assert_eq!(
            fs::read(target.join("source/nested/data.bin")).unwrap(),
            bytes
        );
        assert_eq!(fs::read(target.join("source/empty")).unwrap(), b"");
        assert_eq!(updates.last().unwrap().completed_items, 1);
        assert_eq!(updates.last().unwrap().bytes, 200_000);
        assert!(
            updates
                .windows(2)
                .all(|updates| updates[0].bytes <= updates[1].bytes)
        );
        temp.assert_no_staging();
    }

    #[test]
    fn partial_failure_keeps_successful_outcomes_and_continues() {
        let temp = Temp::new();
        let first = temp.write("first", b"first");
        let last = temp.write("last", b"last");
        let target = temp.directory("target");
        let result = temp.run(Operation::Copy {
            sources: vec![first, temp.path("missing"), last],
            directory: target.clone(),
        });
        assert_eq!(result.completed.len(), 2, "{result:?}");
        assert_eq!(result.errors.len(), 1, "{result:?}");
        assert!(!result.cancelled);
        assert_eq!(fs::read(target.join("first")).unwrap(), b"first");
        assert_eq!(fs::read(target.join("last")).unwrap(), b"last");
    }

    #[test]
    fn cancellation_discards_the_staged_file_and_preserves_the_source() {
        let temp = Temp::new();
        let bytes = vec![0xff; 200_000];
        let source = temp.write("large.bin", &bytes);
        let target = temp.directory("target");
        let cancelled = Cell::new(false);
        let mut copied = 0;
        let result = execute(
            temp.request(Operation::Copy {
                sources: vec![source.clone()],
                directory: target.clone(),
            }),
            || cancelled.get(),
            |progress| {
                if progress.bytes > 0 {
                    copied = progress.bytes;
                    cancelled.set(true);
                }
            },
        );
        assert!(result.cancelled, "{result:?}");
        assert!(result.completed.is_empty());
        assert!(result.errors.is_empty(), "{result:?}");
        assert!(copied > 0 && copied <= 64 * 1024);
        assert_eq!(fs::read(source).unwrap(), bytes);
        assert_eq!(fs::read_dir(target).unwrap().count(), 0);
    }

    #[test]
    fn cancellation_before_mutation_and_after_a_commit_retains_actual_effects() {
        let temp = Temp::new();
        let source = temp.write("source", b"source");
        let second = temp.write("second", b"second");
        let target = temp.directory("target");
        let result = execute(
            temp.request(Operation::Rename {
                path: source.clone(),
                name: "renamed".into(),
            }),
            || true,
            |_| panic!("no progress after initial cancellation"),
        );
        assert!(result.cancelled);
        assert!(result.completed.is_empty());
        assert!(source.exists());
        let cancelled = Cell::new(false);
        let result = execute(
            temp.request(Operation::Copy {
                sources: vec![source, second],
                directory: target.clone(),
            }),
            || cancelled.get(),
            |progress| {
                if progress.completed_items == 1 {
                    cancelled.set(true);
                }
            },
        );
        assert!(result.cancelled);
        assert_eq!(result.completed.len(), 1, "{result:?}");
        assert!(result.errors.is_empty(), "{result:?}");
        assert_eq!(fs::read(target.join("source")).unwrap(), b"source");
        assert!(!target.join("second").exists());
    }

    #[test]
    fn cancellation_after_the_last_chunk_prevents_commit_and_rename_checks_before_mutation() {
        let temp = Temp::new();
        let source = temp.write("source", b"complete");
        let target = temp.directory("target");
        let cancelled = Cell::new(false);
        let result = execute(
            temp.request(Operation::Copy {
                sources: vec![source.clone()],
                directory: target.clone(),
            }),
            || cancelled.get(),
            |progress| {
                if progress.bytes == 8 {
                    cancelled.set(true);
                }
            },
        );
        assert!(result.cancelled, "{result:?}");
        assert!(result.errors.is_empty(), "{result:?}");
        assert!(result.completed.is_empty());
        assert_eq!(fs::read_dir(&target).unwrap().count(), 0);
        cancelled.set(false);
        let result = execute(
            temp.request(Operation::Rename {
                path: source.clone(),
                name: "renamed".into(),
            }),
            || cancelled.get(),
            |_| cancelled.set(true),
        );
        assert!(result.cancelled, "{result:?}");
        assert!(result.completed.is_empty());
        assert_eq!(fs::read(source).unwrap(), b"complete");
        assert!(!temp.path("renamed").exists());
    }

    #[test]
    fn a_destination_created_during_copy_is_never_overwritten_at_commit() {
        let temp = Temp::new();
        let source = temp.write("source", b"source");
        let target = temp.directory("target");
        let destination = target.join("source");
        let injected = Cell::new(false);
        let result = execute(
            temp.request(Operation::Copy {
                sources: vec![source.clone()],
                directory: target.clone(),
            }),
            || false,
            |progress| {
                if progress.bytes > 0 && !injected.replace(true) {
                    fs::write(&destination, b"concurrent entry").unwrap();
                }
            },
        );
        rejected(&result);
        assert_eq!(fs::read(source).unwrap(), b"source");
        assert_eq!(fs::read(destination).unwrap(), b"concurrent entry");
        assert_eq!(fs::read_dir(target).unwrap().count(), 1);
    }

    #[test]
    fn keep_both_commit_retries_a_concurrent_collision_without_recopying() {
        let temp = Temp::new();
        let source = temp.write("data.bin", b"\xff\0");
        let injected = Cell::new(false);
        let result = execute(
            temp.request(Operation::Duplicate {
                sources: vec![source],
            }),
            || false,
            |progress| {
                if progress.bytes > 0 && !injected.replace(true) {
                    fs::write(temp.path("data copy.bin"), b"concurrent entry").unwrap();
                }
            },
        );
        success(&result, 1);
        assert_eq!(
            fs::read(temp.path("data copy.bin")).unwrap(),
            b"concurrent entry"
        );
        assert_eq!(fs::read(temp.path("data copy 2.bin")).unwrap(), b"\xff\0");
        temp.assert_no_staging();
    }

    #[test]
    fn a_changing_source_is_not_committed() {
        let temp = Temp::new();
        let source = temp.write("source", b"original");
        let target = temp.directory("target");
        let changed = Cell::new(false);
        let result = execute(
            temp.request(Operation::Copy {
                sources: vec![source.clone()],
                directory: target.clone(),
            }),
            || false,
            |progress| {
                if progress.bytes > 0 && !changed.replace(true) {
                    fs::write(&source, b"different length").unwrap();
                }
            },
        );
        rejected(&result);
        assert_eq!(fs::read(&source).unwrap(), b"different length");
        assert_eq!(fs::read_dir(target).unwrap().count(), 0);
    }

    #[test]
    fn replacing_a_source_path_after_reading_cannot_commit_the_old_handle() {
        let temp = Temp::new();
        let source = temp.write("source", b"original");
        let target = temp.directory("target");
        let replaced = Cell::new(false);
        let result = execute(
            temp.request(Operation::Copy {
                sources: vec![source.clone()],
                directory: target.clone(),
            }),
            || false,
            |progress| {
                if progress.bytes > 0 && !replaced.replace(true) {
                    fs::rename(&source, temp.path("old-source")).unwrap();
                    fs::write(&source, b"replaced").unwrap();
                }
            },
        );
        rejected(&result);
        assert_eq!(fs::read(source).unwrap(), b"replaced");
        assert_eq!(fs::read(temp.path("old-source")).unwrap(), b"original");
        assert_eq!(fs::read_dir(target).unwrap().count(), 0);
    }

    #[test]
    fn an_already_copied_sibling_is_revalidated_before_the_tree_is_committed() {
        let temp = Temp::new();
        let source = temp.directory("source");
        fs::write(source.join("one"), b"original").unwrap();
        fs::write(source.join("two"), b"original").unwrap();
        let target = temp.directory("target");
        let mut first_copied: Option<PathBuf> = None;
        let mut replaced = false;
        let result = execute(
            temp.request(Operation::Copy {
                sources: vec![source.clone()],
                directory: target.clone(),
            }),
            || false,
            |progress| {
                if progress.bytes == 0 {
                    return;
                }
                let Some(current) = progress.current_path else {
                    return;
                };
                if let Some(first) = &first_copied {
                    if !replaced && first != &current {
                        // The first file's handle has already been checked and closed.
                        fs::rename(first, temp.path("saved-original")).unwrap();
                        fs::write(first, b"replaced").unwrap();
                        replaced = true;
                    }
                } else {
                    first_copied = Some(current);
                }
            },
        );
        assert!(replaced);
        rejected(&result);
        assert_eq!(fs::read(first_copied.unwrap()).unwrap(), b"replaced");
        assert_eq!(fs::read(temp.path("saved-original")).unwrap(), b"original");
        assert_eq!(fs::read_dir(target).unwrap().count(), 0);
    }

    #[test]
    fn directory_membership_changes_are_rejected_before_commit() {
        let temp = Temp::new();
        let source = temp.directory("source");
        fs::write(source.join("original"), b"original").unwrap();
        let target = temp.directory("target");
        let changed = Cell::new(false);
        let result = execute(
            temp.request(Operation::Copy {
                sources: vec![source.clone()],
                directory: target.clone(),
            }),
            || false,
            |progress| {
                if progress.bytes > 0 && !changed.replace(true) {
                    fs::write(source.join("added-during-copy"), b"new entry").unwrap();
                }
            },
        );
        rejected(&result);
        assert_eq!(fs::read(source.join("original")).unwrap(), b"original");
        assert_eq!(
            fs::read(source.join("added-during-copy")).unwrap(),
            b"new entry"
        );
        assert_eq!(fs::read_dir(target).unwrap().count(), 0);
    }

    #[test]
    fn replaced_staging_directory_is_neither_committed_nor_cleaned_up_as_ours() {
        let temp = Temp::new();
        let source = temp.write("source", b"original");
        let target = temp.directory("target");
        let mut replacement = None;
        let result = execute(
            temp.request(Operation::Copy {
                sources: vec![source.clone()],
                directory: target.clone(),
            }),
            || false,
            |progress| {
                if progress.bytes > 0 && replacement.is_none() {
                    let staging = fs::read_dir(&target)
                        .unwrap()
                        .map(|entry| entry.unwrap().path())
                        .find(|path| {
                            path.file_name()
                                .unwrap()
                                .as_encoded_bytes()
                                .starts_with(b".bedtime-filetree-")
                        })
                        .unwrap();
                    fs::rename(&staging, target.join("externally-relocated-staging")).unwrap();
                    fs::create_dir(&staging).unwrap();
                    fs::write(staging.join("unrelated-entry"), b"preserve replacement").unwrap();
                    replacement = Some(staging);
                }
            },
        );
        rejected(&result);
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.message.contains("Staging directory was replaced")),
            "{result:?}"
        );
        assert_eq!(
            fs::read(replacement.unwrap().join("unrelated-entry")).unwrap(),
            b"preserve replacement"
        );
        assert_eq!(fs::read(source).unwrap(), b"original");
        assert!(!target.join("source").exists());
        assert_eq!(
            fs::read(target.join("externally-relocated-staging/item")).unwrap(),
            b"original"
        );
    }

    #[test]
    fn symlinks_are_preserved_and_outward_parent_symlinks_cannot_escape_the_root() {
        use std::os::unix::fs::symlink;
        let temp = Temp::new();
        let external = Temp::new();
        let outside = external.write("outside", b"outside");
        symlink(&external.0, temp.path("outward")).unwrap();
        symlink("missing-target", temp.path("dangling")).unwrap();
        for operation in [
            Operation::CreateFile {
                directory: temp.path("outward"),
                name: "new".into(),
            },
            Operation::Move {
                sources: vec![temp.path("outward/outside")],
                directory: temp.0.clone(),
            },
            Operation::Rename {
                path: temp.path("outward/outside"),
                name: "other".into(),
            },
            Operation::Trash {
                sources: vec![temp.path("outward/outside")],
            },
        ] {
            rejected(&temp.run(operation));
        }
        success(
            &temp.run(Operation::Duplicate {
                sources: vec![temp.path("outward"), temp.path("dangling")],
            }),
            2,
        );
        assert!(
            fs::symlink_metadata(temp.path("outward copy"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_link(temp.path("outward copy")).unwrap(),
            external.0
        );
        assert_eq!(
            fs::read_link(temp.path("dangling copy")).unwrap(),
            PathBuf::from("missing-target")
        );
        success(
            &temp.run(Operation::Rename {
                path: temp.path("outward"),
                name: "renamed-link".into(),
            }),
            1,
        );
        let target = temp.directory("target");
        success(
            &temp.run(Operation::Move {
                sources: vec![temp.path("renamed-link")],
                directory: target.clone(),
            }),
            1,
        );
        assert_eq!(
            fs::read_link(target.join("renamed-link")).unwrap(),
            external.0
        );
        assert_eq!(fs::read(outside).unwrap(), b"outside");
        assert!(!external.path("new").exists());
    }

    #[test]
    fn canonical_root_and_in_root_directory_aliases_are_accepted() {
        use std::os::unix::fs::symlink;
        let temp = Temp::new();
        let directory = temp.directory("directory");
        symlink(&directory, temp.path("directory-alias")).unwrap();
        let alias_parent = Temp::new();
        let root_alias = alias_parent.path("root-alias");
        symlink(&temp.0, &root_alias).unwrap();
        let result = execute(
            OperationRequest {
                id: 42,
                root: root_alias,
                operation: Operation::CreateFile {
                    directory: temp.path("directory-alias"),
                    name: "inside".into(),
                },
            },
            || false,
            |_| {},
        );
        success(&result, 1);
        assert!(directory.join("inside").is_file());
        assert!(
            matches!(&result.completed[0], Change::Created { path } if path == &directory.join("inside"))
        );
    }

    #[test]
    fn a_dangling_destination_link_is_a_conflict_not_a_missing_file() {
        use std::os::unix::fs::symlink;
        let temp = Temp::new();
        let source = temp.write("source", b"source");
        let target = temp.directory("target");
        symlink("missing", target.join("source")).unwrap();
        rejected(&temp.run(Operation::Copy {
            sources: vec![source.clone()],
            directory: target.clone(),
        }));
        rejected(&temp.run(Operation::Move {
            sources: vec![source],
            directory: target.clone(),
        }));
        rejected(&temp.run(Operation::CreateFile {
            directory: target.clone(),
            name: "source".into(),
        }));
        assert_eq!(
            fs::read_link(target.join("source")).unwrap(),
            PathBuf::from("missing")
        );
    }

    #[test]
    fn native_non_utf8_names_survive_create_rename_and_duplicate() {
        use std::os::unix::ffi::OsStringExt;
        let temp = Temp::new();
        let name = OsString::from_vec(b"native-\xff.bin".to_vec());
        let renamed = OsString::from_vec(b"renamed-\xfe.bin".to_vec());
        // APFS can reject invalid UTF-8 natively with EILSEQ. Probe the actual
        // filesystem independently of the operation implementation; do not turn
        // unrelated I/O failures into skips or weaken the lossless test above.
        for candidate in [&name, &renamed] {
            let path = temp.path(candidate);
            match fs::write(&path, b"filesystem capability probe") {
                Ok(()) => fs::remove_file(path).unwrap(),
                Err(error)
                    if cfg!(target_os = "macos")
                        && (error.kind() == io::ErrorKind::InvalidInput
                            || error.raw_os_error() == Some(libc::EILSEQ)) =>
                {
                    return;
                }
                Err(error) => panic!("Unexpected non-UTF-8 filesystem probe failure: {error}"),
            }
        }
        success(
            &temp.run(Operation::CreateFile {
                directory: temp.0.clone(),
                name: name.clone(),
            }),
            1,
        );
        fs::write(temp.path(&name), b"\0\xff").unwrap();
        success(
            &temp.run(Operation::Rename {
                path: temp.path(&name),
                name: renamed.clone(),
            }),
            1,
        );
        success(
            &temp.run(Operation::Duplicate {
                sources: vec![temp.path(&renamed)],
            }),
            1,
        );
        let copy = OsString::from_vec(b"renamed-\xfe copy.bin".to_vec());
        assert_eq!(fs::read(temp.path(copy)).unwrap(), b"\0\xff");
        assert!(!temp.path(name).exists());
    }

    #[test]
    fn live_case_only_rename_changes_the_directory_entry_spelling() {
        let temp = Temp::new();
        let source = temp.write("MixedCase.txt", b"preserve");
        let destination = temp.path("mixedcase.txt");
        // On case-insensitive volumes this exercises the staged alias path; on
        // case-sensitive volumes the same public operation is an ordinary rename.
        let case_insensitive = destination.exists();
        success(
            &temp.run(Operation::Rename {
                path: source,
                name: "mixedcase.txt".into(),
            }),
            1,
        );
        let names: Vec<_> = fs::read_dir(&temp.0)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert!(
            names.contains(&OsString::from("mixedcase.txt")),
            "case-insensitive={case_insensitive}: {names:?}"
        );
        assert!(
            !names.contains(&OsString::from("MixedCase.txt")),
            "{names:?}"
        );
        assert_eq!(fs::read(destination).unwrap(), b"preserve");
        temp.assert_no_staging();
    }

    #[test]
    fn a_distinct_hard_link_is_not_mistaken_for_a_case_alias() {
        let temp = Temp::new();
        let source = temp.write("source", b"preserve");
        let target = temp.path("TARGET");
        fs::hard_link(&source, &target).unwrap();
        rejected(&temp.run(Operation::Rename {
            path: source.clone(),
            name: "TARGET".into(),
        }));
        assert_eq!(fs::read(source).unwrap(), b"preserve");
        assert_eq!(fs::read(target).unwrap(), b"preserve");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cross_device_move_reports_unsupported_and_preserves_source_when_available() {
        use std::os::unix::fs::MetadataExt;
        let source_directory = Temp::new();
        let Ok(target) = Temp::in_directory(Path::new("/dev/shm")) else {
            return;
        };
        if fs::metadata(&source_directory.0).unwrap().dev()
            == fs::metadata(&target.0).unwrap().dev()
        {
            return;
        }
        let source = source_directory.write("source", b"preserve");
        let result = execute(
            OperationRequest {
                id: 42,
                root: PathBuf::from("/"),
                operation: Operation::Move {
                    sources: vec![source.clone()],
                    directory: target.0.clone(),
                },
            },
            || false,
            |_| {},
        );
        rejected(&result);
        assert!(
            result.errors[0]
                .message
                .contains("Cross-device moves are unsupported"),
            "{result:?}"
        );
        assert_eq!(fs::read(source).unwrap(), b"preserve");
        assert!(!target.path("source").exists());
    }
}
