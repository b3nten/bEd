use crate::*;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::UNIX_EPOCH,
};

type ServiceResult<T> = Result<T, RemoteError>;

/// Shared local service implementation, also used by the headless process.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalBackend;

impl LocalBackend {
    pub fn call(&self, request: Request) -> ServiceResult<Response> {
        match request {
            Request::Hello { version } => {
                if version != PROTOCOL_VERSION {
                    return Err(RemoteError::new(
                        ErrorKind::UnsupportedVersion,
                        format!(
                            "protocol version {version} unsupported; expected {PROTOCOL_VERSION}"
                        ),
                    ));
                }
                Ok(Response::Hello { version })
            }
            Request::Canonicalize {
                root,
                path,
                allow_missing,
            } => {
                let root = canonical_root(&root)?;
                Ok(Response::Path {
                    path: path_string(&resolve(&root, &path, allow_missing)?)?,
                })
            }
            Request::ReadFile { root, path } => {
                let root = canonical_root(&root)?;
                let path = resolve(&root, &path, false)?;
                let (bytes, baseline) = read_file(&path)?;
                Ok(Response::File {
                    path: path_string(&path)?,
                    bytes,
                    baseline,
                })
            }
            Request::ReadDirectory {
                root,
                path,
                classify_gitignored,
            } => {
                let root = canonical_root(&root)?;
                let path = resolve(&root, &path, false)?;
                let mut entries = Vec::new();
                for entry in fs::read_dir(&path)? {
                    let entry = entry?;
                    let kind = entry.file_type()?;
                    entries.push(DirectoryEntry {
                        path: path_string(&entry.path())?,
                        name: entry.file_name().into_string().map_err(|_| {
                            RemoteError::new(
                                ErrorKind::InvalidInput,
                                "remote filename is not valid UTF-8",
                            )
                        })?,
                        is_directory: entry.path().is_dir(),
                        is_symlink: kind.is_symlink(),
                        is_gitignored: false,
                    });
                }
                entries.sort_by(|a, b| {
                    b.is_directory
                        .cmp(&a.is_directory)
                        .then_with(|| a.name.cmp(&b.name))
                });
                let warning = if classify_gitignored {
                    classify_tree_entries(&path, &mut entries)
                        .err()
                        .map(|error| error.to_string())
                } else {
                    None
                };
                Ok(Response::Directory { entries, warning })
            }
            Request::ListFiles { root } => {
                let root = canonical_root(&root)?;
                let paths = walk_files(&root, true)?
                    .iter()
                    .map(|path| path_string(path))
                    .collect::<ServiceResult<_>>()?;
                Ok(Response::Files { paths })
            }
            Request::WriteFile {
                root,
                path,
                bytes,
                baseline,
            } => {
                let root = canonical_root(&root)?;
                let path = resolve(&root, &path, true)?;
                let baseline = write_file(&path, &bytes, baseline.as_ref())?;
                Ok(Response::Written { baseline })
            }
            Request::CreateFile { root, path } => {
                let root = canonical_root(&root)?;
                let path = resolve(&root, &path, true)?;
                OpenOptions::new().write(true).create_new(true).open(path)?;
                Ok(Response::Unit)
            }
            Request::CreateDirectory { root, path } => {
                let root = canonical_root(&root)?;
                fs::create_dir(resolve(&root, &path, true)?)?;
                Ok(Response::Unit)
            }
            Request::Rename { root, from, to } => {
                let root = canonical_root(&root)?;
                let from = resolve_entry(&root, &from)?;
                let to = resolve_entry(&root, &to)?;
                protect_root(&root, &from)?;
                protect_root(&root, &to)?;
                if fs::symlink_metadata(&to).is_ok() {
                    return Err(RemoteError::new(
                        ErrorKind::AlreadyExists,
                        "rename destination already exists",
                    ));
                }
                rename_no_replace(&from, &to)?;
                Ok(Response::Unit)
            }
            Request::Remove {
                root,
                path,
                is_directory,
            } => {
                let root = canonical_root(&root)?;
                let path = resolve_entry(&root, &path)?;
                protect_root(&root, &path)?;
                let metadata = fs::symlink_metadata(&path)?;
                if metadata.file_type().is_symlink() || !is_directory {
                    fs::remove_file(path)?;
                } else {
                    fs::remove_dir_all(path)?;
                }
                Ok(Response::Unit)
            }
            Request::Search {
                root,
                query,
                case_sensitive,
                include_ignored,
                max_results,
            } => search(
                &canonical_root(&root)?,
                &query,
                case_sensitive,
                include_ignored,
                max_results,
            ),
            Request::GitStatus { root } => git_status(&canonical_root(&root)?),
            Request::GitBaseline { root, path } => git_baseline(&canonical_root(&root)?, &path),
        }
    }
}

pub fn serve(input: impl Read, output: impl Write) -> io::Result<()> {
    serve_with(input, output, |request| LocalBackend.call(request))
}

pub fn serve_with(
    mut input: impl Read,
    mut output: impl Write,
    mut dispatch: impl FnMut(Request) -> Result<Response, RemoteError>,
) -> io::Result<()> {
    let mut negotiated = false;
    while let Some(frame) = read_frame::<_, RequestFrame>(&mut input)? {
        let response = if !negotiated && !matches!(&frame.request, Request::Hello { .. }) {
            Err(RemoteError::new(
                ErrorKind::UnsupportedVersion,
                "protocol handshake required",
            ))
        } else {
            let response = dispatch(frame.request);
            if matches!(
                &response,
                Ok(Response::Hello {
                    version: PROTOCOL_VERSION
                })
            ) {
                negotiated = true;
            }
            response
        };
        write_frame(
            &mut output,
            &ResponseFrame {
                id: frame.id,
                response,
            },
        )?;
    }
    Ok(())
}

fn canonical_root(root: &str) -> ServiceResult<PathBuf> {
    let root = fs::canonicalize(if root.is_empty() { "." } else { root })?;
    if !root.is_dir() {
        return Err(RemoteError::new(
            ErrorKind::InvalidInput,
            "workspace root is not a directory",
        ));
    }
    Ok(root)
}

fn within(root: &Path, path: PathBuf) -> ServiceResult<PathBuf> {
    if !path.starts_with(root) {
        return Err(RemoteError::new(
            ErrorKind::PermissionDenied,
            "path escapes workspace root",
        ));
    }
    Ok(path)
}

fn resolve(root: &Path, path: &str, allow_missing: bool) -> ServiceResult<PathBuf> {
    let path = root.join(path);
    match fs::canonicalize(&path) {
        Ok(path) => within(root, path),
        Err(error) if error.kind() == io::ErrorKind::NotFound && allow_missing => resolve_entry(
            root,
            path.to_str().ok_or_else(|| {
                RemoteError::new(ErrorKind::InvalidInput, "remote path is not valid UTF-8")
            })?,
        ),
        Err(error) => Err(error.into()),
    }
}

// File actions operate on the entry, preserving symlinks rather than renaming
// or removing their targets. The parent must resolve within the capability root.
fn resolve_entry(root: &Path, path: &str) -> ServiceResult<PathBuf> {
    let path = root.join(path);
    if path == root {
        return Ok(root.to_path_buf());
    }
    let name = path
        .file_name()
        .ok_or_else(|| RemoteError::new(ErrorKind::InvalidInput, "path requires a filename"))?;
    let parent = path.parent().ok_or_else(|| {
        RemoteError::new(ErrorKind::InvalidInput, "path requires a parent directory")
    })?;
    Ok(within(root, fs::canonicalize(parent)?)?.join(name))
}

fn protect_root(root: &Path, path: &Path) -> ServiceResult<()> {
    if path == root {
        return Err(RemoteError::new(
            ErrorKind::PermissionDenied,
            "cannot remove or rename workspace root",
        ));
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    #[cfg(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    unsafe extern "C" {
        fn syscall(number: std::ffi::c_long, ...) -> std::ffi::c_long;
    }
    #[cfg(all(
        target_os = "linux",
        not(any(target_arch = "x86_64", target_arch = "aarch64"))
    ))]
    unsafe extern "C" {
        fn renameat2(
            old_directory: std::ffi::c_int,
            old_path: *const std::ffi::c_char,
            new_directory: std::ffi::c_int,
            new_path: *const std::ffi::c_char,
            flags: std::ffi::c_uint,
        ) -> std::ffi::c_int;
    }
    #[cfg(target_os = "macos")]
    unsafe extern "C" {
        fn renamex_np(
            old_path: *const std::ffi::c_char,
            new_path: *const std::ffi::c_char,
            flags: std::ffi::c_uint,
        ) -> std::ffi::c_int;
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    const SYS_RENAMEAT2: std::ffi::c_long = 316;
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    const SYS_RENAMEAT2: std::ffi::c_long = 276;
    #[cfg(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    // SAFETY: Both CString pointers remain live. These Linux ABI syscall
    // numbers are architecture-specific; using syscall also supports musl
    // versions which do not export the renameat2 libc convenience function.
    let result = unsafe {
        syscall(
            SYS_RENAMEAT2,
            -100_i32,
            source.as_ptr(),
            -100_i32,
            destination.as_ptr(),
            1_u32,
        )
    };
    #[cfg(all(
        target_os = "linux",
        not(any(target_arch = "x86_64", target_arch = "aarch64"))
    ))]
    // SAFETY: CString pointers remain live for the call. AT_FDCWD=-100 and
    // RENAME_NOREPLACE=1 request an atomic operation without replacing an entry.
    let result = unsafe { renameat2(-100, source.as_ptr(), -100, destination.as_ptr(), 1) };
    #[cfg(target_os = "macos")]
    // SAFETY: CString pointers remain live; RENAME_EXCL=4 forbids replacement.
    let result = unsafe { renamex_np(source.as_ptr(), destination.as_ptr(), 4) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(source: *const u16, destination: *const u16, flags: u32) -> i32;
    }
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: Both strings are NUL-terminated and live throughout the call;
    // flags=0 forbids replacing an existing destination.
    if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 0) } != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn rename_no_replace(_source: &Path, _destination: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic no-replace rename is unsupported on this host",
    ))
}

fn path_string(path: &Path) -> ServiceResult<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| RemoteError::new(ErrorKind::InvalidInput, "remote path is not valid UTF-8"))
}

fn modified_ns(metadata: &fs::Metadata) -> Option<u64> {
    metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos()
        .try_into()
        .ok()
}

fn baseline(bytes: &[u8], metadata: &fs::Metadata) -> FileBaseline {
    // Fixed FNV-1a; unlike DefaultHasher this is stable across processes/versions.
    let fingerprint = bytes.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    });
    FileBaseline {
        fingerprint,
        len: bytes.len() as u64,
        modified_ns: modified_ns(metadata),
    }
}

fn file_too_large(path: &Path, actual: u64) -> RemoteError {
    RemoteError::new(
        ErrorKind::TooLarge,
        format!(
            "Cannot edit '{}': file is {actual} bytes; Bed's limit is {} MiB ({MAX_FILE_BYTES} bytes). Open it in another editor or split it into smaller files.",
            path.display(),
            MAX_FILE_BYTES / (1024 * 1024),
        ),
    )
}

fn read_file(path: &Path) -> ServiceResult<(Vec<u8>, FileBaseline)> {
    let mut file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(RemoteError::new(
            ErrorKind::InvalidInput,
            "path is not a regular file",
        ));
    }
    if metadata.len() > MAX_FILE_BYTES as u64 {
        return Err(file_too_large(path, metadata.len()));
    }
    let mut bytes = Vec::with_capacity((metadata.len() as usize).min(1024));
    Read::by_ref(&mut file).take(1024).read_to_end(&mut bytes)?;
    Read::by_ref(&mut file)
        .take((MAX_FILE_BYTES + 1) as u64 - bytes.len() as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(file_too_large(
            path,
            file.metadata()?.len().max(bytes.len() as u64),
        ));
    }
    let after = file.metadata()?;
    if metadata.len() != after.len()
        || modified_ns(&metadata) != modified_ns(&after)
        || after.len() != bytes.len() as u64
    {
        return Err(RemoteError::new(
            ErrorKind::Conflict,
            "file changed while being read; retry",
        ));
    }
    let baseline = baseline(&bytes, &after);
    Ok((bytes, baseline))
}

fn check_baseline(path: &Path, expected: Option<&FileBaseline>) -> ServiceResult<()> {
    match (fs::symlink_metadata(path), expected) {
        (Err(error), None) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        (Ok(_), Some(expected)) => {
            if &read_file(path)?.1 == expected {
                Ok(())
            } else {
                Err(conflict())
            }
        }
        (Ok(_), None) => Err(conflict()),
        (Err(error), Some(_)) if error.kind() == io::ErrorKind::NotFound => Err(conflict()),
        (Err(error), _) => Err(error.into()),
    }
}

fn conflict() -> RemoteError {
    RemoteError::new(
        ErrorKind::Conflict,
        "file changed on disk; reload before saving",
    )
}

fn write_file(
    path: &Path,
    bytes: &[u8],
    expected: Option<&FileBaseline>,
) -> ServiceResult<FileBaseline> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(file_too_large(path, bytes.len() as u64));
    }
    check_baseline(path, expected)?;
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().ok_or_else(|| {
        RemoteError::new(
            ErrorKind::InvalidInput,
            "save path requires a parent directory",
        )
    })?;
    let (temp, mut file) = loop {
        let temp = parent.join(format!(
            ".bed-save-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => break (temp, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    };
    let result = (|| {
        if let Ok(metadata) = fs::metadata(path) {
            file.set_permissions(metadata.permissions())?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        check_baseline(path, expected)?;
        if expected.is_none() {
            // Atomically reserve a new filename: a file appearing after the
            // baseline check must never be silently overwritten.
            fs::hard_link(&temp, path).map_err(|error| {
                if error.kind() == io::ErrorKind::AlreadyExists {
                    conflict()
                } else {
                    error.into()
                }
            })?;
            fs::remove_file(&temp)?;
        } else {
            fs::rename(&temp, path)?;
        }
        Ok(baseline(bytes, &fs::metadata(path)?))
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

fn git(root: &Path, arguments: &[&str]) -> io::Result<std::process::Output> {
    Command::new("git")
        .env("LC_ALL", "C")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .stdin(Stdio::null())
        .output()
}

// One batch per directory, never one subprocess per entry. Git owns ignore semantics.
fn classify_tree_entries(directory: &Path, entries: &mut [DirectoryEntry]) -> io::Result<()> {
    let repository = git(directory, &["rev-parse", "--is-inside-work-tree"])?;
    if !repository.status.success() {
        // A plain folder is expected; other Git failures should be visible to the caller.
        if String::from_utf8_lossy(&repository.stderr).contains("not a git repository") {
            return Ok(());
        }
        return Err(io::Error::other(
            String::from_utf8_lossy(&repository.stderr).into_owned(),
        ));
    }
    if repository.stdout != b"true\n" || entries.is_empty() {
        return Ok(());
    }
    let tracked = git(directory, &["ls-files", "-z", "--cached", "--", "."])?;
    if !tracked.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&tracked.stderr).into_owned(),
        ));
    }
    let paths: Vec<_> = entries.iter().map(|entry| entry.name.clone()).collect();
    let ignored = git_ignored_paths(directory, &paths)?;
    let tracked: std::collections::HashSet<_> = tracked
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .filter_map(|path| path.split(|byte| *byte == b'/').next())
        .collect();
    for entry in entries {
        entry.is_gitignored =
            ignored.contains(&entry.name) && !tracked.contains(entry.name.as_bytes());
    }
    Ok(())
}

/// Batch Git's ignore rules for paths relative to `directory`; tracked files are excluded.
/// Input/output use NUL delimiters so spaces, newlines and literal glob characters are safe.
pub fn git_ignored_paths(
    directory: &Path,
    paths: &[String],
) -> io::Result<std::collections::HashSet<String>> {
    if paths.is_empty() {
        return Ok(Default::default());
    }
    let mut input = Vec::new();
    for path in paths {
        input.extend_from_slice(path.as_bytes());
        input.push(0);
    }
    let mut child = Command::new("git")
        .env("LC_ALL", "C")
        .arg("-C")
        .arg(directory)
        .args(["check-ignore", "-z", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().unwrap();
    // Drain output concurrently with feeding input to avoid pipe-capacity deadlocks.
    let output = std::thread::scope(|scope| {
        let writer = scope.spawn(move || stdin.write_all(&input));
        let output = child.wait_with_output();
        writer
            .join()
            .map_err(|_| io::Error::other("Git-ignore input writer failed"))??;
        output
    })?;
    if !matches!(output.status.code(), Some(0 | 1)) {
        return Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8(path.to_vec()).map_err(io::Error::other))
        .collect()
}

fn discover_files(root: &Path) -> ServiceResult<Vec<PathBuf>> {
    // Git performs ignore handling, including parent worktrees and nested rules.
    if let Ok(repository) = git(root, &["rev-parse", "--is-inside-work-tree"])
        && repository.status.success()
    {
        let output = git(
            root,
            &[
                "ls-files",
                "-z",
                "--cached",
                "--others",
                "--exclude-standard",
                "--",
                ".",
            ],
        )?;
        if !output.status.success() {
            return Err(RemoteError::new(
                ErrorKind::Other,
                String::from_utf8_lossy(&output.stderr),
            ));
        }
        let mut files = Vec::new();
        for name in output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|name| !name.is_empty())
        {
            let name = std::str::from_utf8(name).map_err(|_| {
                RemoteError::new(
                    ErrorKind::InvalidInput,
                    "remote filename is not valid UTF-8",
                )
            })?;
            let path = root.join(name);
            if path.is_file()
                && let Ok(path) = resolve(root, name, false)
            {
                files.push(path);
            }
        }
        files.sort_by_key(|path| path.as_os_str().len());
        files.dedup();
        return Ok(files);
    }
    walk_files(root, false)
}

fn walk_files(root: &Path, include_git: bool) -> ServiceResult<Vec<PathBuf>> {
    let mut stack = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            if !include_git && entry.file_name() == ".git" {
                continue;
            }
            let kind = entry.file_type()?;
            if kind.is_dir() {
                stack.push(entry.path());
            } else if (kind.is_file() || kind.is_symlink())
                && entry.path().is_file()
                && let Ok(path) = resolve(root, &path_string(&entry.path())?, false)
            {
                files.push(path);
            }
        }
    }
    files.sort_by_key(|path| path.as_os_str().len());
    files.dedup();
    Ok(files)
}

fn document_lines(text: &str) -> impl Iterator<Item = &str> {
    let mut remaining = Some(text.strip_prefix('\u{feff}').unwrap_or(text));
    std::iter::from_fn(move || {
        let text = remaining.take()?;
        if let Some(end) = text.find(['\r', '\n']) {
            let separator = if text[end..].starts_with("\r\n") {
                2
            } else {
                1
            };
            remaining = Some(&text[end + separator..]);
            Some(&text[..end])
        } else {
            Some(text)
        }
    })
}

fn search(
    root: &Path,
    query: &str,
    case_sensitive: bool,
    include_ignored: bool,
    max_results: usize,
) -> ServiceResult<Response> {
    let mut matches = Vec::new();
    let maximum = max_results.min(10_000);
    if query.is_empty() || maximum == 0 {
        return Ok(Response::Search {
            matches,
            truncated: false,
            scanned_files: 0,
            discovered_files: 0,
            ignored_paths: 0,
            skipped_files: 0,
        });
    }
    let needle = if case_sensitive {
        query.to_owned()
    } else {
        query.to_ascii_lowercase()
    };
    let paths = if include_ignored {
        walk_files(root, false)?
    } else {
        discover_files(root)?
    };
    let discovered_files = paths.len();
    let mut scanned_files = 0;
    let mut skipped_files = 0;
    let mut budget = 0;
    for path in paths {
        scanned_files += 1;
        let Ok((bytes, _)) = read_file(&path) else {
            skipped_files += 1;
            continue;
        };
        let Ok(text) = std::str::from_utf8(&bytes) else {
            skipped_files += 1;
            continue;
        };
        for (line_index, line) in document_lines(text).enumerate() {
            let mut byte = 0;
            while byte <= line.len() {
                let found = if case_sensitive {
                    line[byte..].find(&needle).map(|offset| byte + offset)
                } else {
                    // ASCII folding preserves UTF-8 bytes and byte columns.
                    // Compare only the needle-sized window: lowercasing each
                    // remaining suffix would copy a long line quadratically.
                    line.as_bytes()[byte..]
                        .windows(needle.len())
                        .position(|candidate| candidate.eq_ignore_ascii_case(needle.as_bytes()))
                        .map(|offset| byte + offset)
                };
                let Some(found) = found else {
                    break;
                };
                budget +=
                    line.len().saturating_mul(12) + path.as_os_str().len().saturating_mul(6) + 512;
                if matches.len() == maximum || budget > MAX_FRAME_BYTES / 2 {
                    return Ok(Response::Search {
                        matches,
                        truncated: true,
                        scanned_files,
                        discovered_files,
                        ignored_paths: 0,
                        skipped_files,
                    });
                }
                matches.push(SearchMatch {
                    path: path_string(&path)?,
                    line: line_index + 1,
                    column: found + 1,
                    editor_row: line_index + 1,
                    line_bytes: line.as_bytes().to_vec(),
                    text: line.to_owned(),
                });
                byte = found + line[found..].chars().next().map_or(1, char::len_utf8);
            }
        }
    }
    Ok(Response::Search {
        matches,
        truncated: false,
        scanned_files,
        discovered_files,
        ignored_paths: 0,
        skipped_files,
    })
}

fn git_status(root: &Path) -> ServiceResult<Response> {
    let repository = git(root, &["rev-parse", "--show-toplevel"])?;
    if !repository.status.success() {
        return Ok(Response::GitStatus {
            entries: Vec::new(),
        });
    }
    let worktree = PathBuf::from(
        String::from_utf8(repository.stdout)
            .map_err(|_| RemoteError::new(ErrorKind::InvalidInput, "Git root is not valid UTF-8"))?
            .trim_end_matches(['\r', '\n']),
    );
    let output = git(
        root,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--",
            ".",
        ],
    )?;
    if !output.status.success() {
        return Err(RemoteError::new(
            ErrorKind::Other,
            String::from_utf8_lossy(&output.stderr),
        ));
    }
    let mut records = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty());
    let mut entries = Vec::new();
    while let Some(record) = records.next() {
        if record.len() < 4 {
            return Err(RemoteError::new(
                ErrorKind::Other,
                "malformed Git status record",
            ));
        }
        let path = std::str::from_utf8(&record[3..]).map_err(|_| {
            RemoteError::new(ErrorKind::InvalidInput, "Git filename is not valid UTF-8")
        })?;
        entries.push(GitStatusEntry {
            path: path_string(&worktree.join(path))?,
            index_status: record[0] as char,
            worktree_status: record[1] as char,
        });
        if record[0] == b'R' || record[0] == b'C' || record[1] == b'R' || record[1] == b'C' {
            records.next();
        }
    }
    Ok(Response::GitStatus { entries })
}

fn git_baseline(root: &Path, path: &str) -> ServiceResult<Response> {
    let path = resolve(root, path, true)?;
    let repository = git(root, &["rev-parse", "--show-toplevel"])?;
    if !repository.status.success() {
        return Ok(Response::GitBaseline { bytes: None });
    }
    let worktree = PathBuf::from(
        String::from_utf8(repository.stdout)
            .map_err(|_| RemoteError::new(ErrorKind::InvalidInput, "Git root is not valid UTF-8"))?
            .trim_end_matches(['\r', '\n']),
    );
    let relative = path
        .strip_prefix(&worktree)
        .map_err(|_| RemoteError::new(ErrorKind::PermissionDenied, "Git path outside worktree"))?;
    let object = format!("HEAD:{}", path_string(relative)?);
    let size = git(root, &["cat-file", "-s", &object])?;
    if !size.status.success() {
        return Ok(Response::GitBaseline { bytes: None });
    }
    let size = String::from_utf8_lossy(&size.stdout)
        .trim()
        .parse::<u64>()
        .map_err(|_| RemoteError::new(ErrorKind::Other, "invalid Git object size"))?;
    if size > MAX_FILE_BYTES as u64 {
        return Err(file_too_large(&path, size));
    }
    let output = git(root, &["show", &object])?;
    if !output.status.success() {
        return Ok(Response::GitBaseline { bytes: None });
    }
    if output.stdout.len() > MAX_FILE_BYTES {
        return Err(file_too_large(&path, output.stdout.len() as u64));
    }
    Ok(Response::GitBaseline {
        bytes: Some(output.stdout),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "bed-remote-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(fs::canonicalize(path).unwrap())
        }
        fn root(&self) -> String {
            self.0.to_str().unwrap().into()
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn directory(temp: &Temp, path: &str) -> (Vec<DirectoryEntry>, Option<String>) {
        let Response::Directory { entries, warning } = LocalBackend
            .call(Request::ReadDirectory {
                root: temp.root(),
                path: path.into(),
                classify_gitignored: true,
            })
            .unwrap()
        else {
            panic!("expected directory listing")
        };
        (entries, warning)
    }

    #[test]
    fn tree_listings_classify_git_rules_but_preserve_tracked_descendants() {
        let temp = Temp::new();
        assert!(git(&temp.0, &["init", "-q"]).unwrap().status.success());
        fs::create_dir(temp.0.join("src")).unwrap();
        fs::write(temp.0.join("src/tracked.log"), b"keep").unwrap();
        assert!(
            git(&temp.0, &["add", "--", "src/tracked.log"])
                .unwrap()
                .status
                .success()
        );
        fs::write(temp.0.join(".gitignore"), b"*.log\nignored/\n").unwrap();
        fs::write(temp.0.join("src/.gitignore"), b"!keep.log\n").unwrap();
        fs::write(temp.0.join(".git/info/exclude"), b"excluded\n").unwrap();
        fs::write(temp.0.join("excluded"), b"").unwrap();
        fs::create_dir(temp.0.join("ignored")).unwrap();
        for name in ["drop.log", "keep.log", "line break.log"] {
            fs::write(temp.0.join("src").join(name), b"").unwrap();
        }
        let (root, warning) = directory(&temp, ".");
        assert_eq!(warning, None);
        assert_eq!(directory(&temp, ".git").1, None);
        assert!(
            root.iter()
                .find(|entry| entry.name == "ignored")
                .unwrap()
                .is_gitignored
        );
        assert!(
            root.iter()
                .find(|entry| entry.name == "excluded")
                .unwrap()
                .is_gitignored
        );
        let (entries, warning) = directory(&temp, "src");
        assert_eq!(warning, None);
        for entry in entries {
            assert_eq!(
                entry.is_gitignored,
                matches!(entry.name.as_str(), "drop.log" | "line break.log"),
                "{}",
                entry.name
            );
        }
        fs::write(temp.0.join(".gitignore"), b"src/\n").unwrap();
        let (root, _) = directory(&temp, ".");
        assert!(
            !root
                .iter()
                .find(|entry| entry.name == "src")
                .unwrap()
                .is_gitignored
        );
        let (entries, _) = directory(&temp, "src");
        assert!(
            !entries
                .iter()
                .find(|entry| entry.name == "tracked.log")
                .unwrap()
                .is_gitignored
        );
    }

    #[test]
    fn tree_plain_folders_and_git_failures_keep_the_listing_visible() {
        let temp = Temp::new();
        fs::write(temp.0.join("file"), b"").unwrap();
        fs::write(temp.0.join(".DS_Store"), b"").unwrap();
        let (entries, warning) = directory(&temp, ".");
        assert_eq!(warning, None);
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|entry| !entry.is_gitignored));
        assert!(git(&temp.0, &["init", "-q"]).unwrap().status.success());
        fs::write(temp.0.join(".git/config"), b"invalid git config\n").unwrap();
        let (entries, warning) = directory(&temp, ".");
        assert!(warning.is_some());
        assert!(entries.iter().any(|entry| entry.name == "file"));
        assert!(entries.iter().all(|entry| !entry.is_gitignored));
        let Response::Directory { warning, .. } = LocalBackend
            .call(Request::ReadDirectory {
                root: temp.root(),
                path: ".".into(),
                classify_gitignored: false,
            })
            .unwrap()
        else {
            panic!("expected directory listing")
        };
        assert_eq!(warning, None, "disabled filters must not probe Git");
    }

    #[cfg(unix)]
    #[test]
    fn git_ignore_batch_preserves_literal_newlines_spaces_and_glob_characters() {
        let temp = Temp::new();
        assert!(git(&temp.0, &["init", "-q"]).unwrap().status.success());
        fs::write(temp.0.join(".gitignore"), b"*.log\n").unwrap();
        let paths = [
            "line\nbreak.log",
            "space name.log",
            "[literal].log",
            "é.log",
        ]
        .map(str::to_owned);
        let ignored = git_ignored_paths(&temp.0, &paths).unwrap();
        assert_eq!(ignored, paths.into_iter().collect());
    }

    #[test]
    fn large_git_ignore_batches_drain_output_while_writing_input() {
        let temp = Temp::new();
        assert!(git(&temp.0, &["init", "-q"]).unwrap().status.success());
        fs::write(temp.0.join(".gitignore"), b"*.log\n").unwrap();
        let mut entries: Vec<_> = (0..2000)
            .map(|index| {
                let name = format!("{index}-{}.log", "long".repeat(30));
                DirectoryEntry {
                    path: temp.0.join(&name).to_str().unwrap().into(),
                    name,
                    is_directory: false,
                    is_symlink: false,
                    is_gitignored: false,
                }
            })
            .collect();
        classify_tree_entries(&temp.0, &mut entries).unwrap();
        assert!(entries.iter().all(|entry| entry.is_gitignored));
    }

    #[test]
    fn reads_preserve_bom_crlf_and_conflicting_saves_leave_disk_untouched() {
        let temp = Temp::new();
        let path = temp.0.join("text");
        let original = b"\xef\xbb\xbfhello\r\n";
        fs::write(&path, original).unwrap();
        let Response::File {
            bytes, baseline, ..
        } = LocalBackend
            .call(Request::ReadFile {
                root: temp.root(),
                path: "text".into(),
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(bytes, original);
        fs::write(&path, b"external").unwrap();
        let error = LocalBackend
            .call(Request::WriteFile {
                root: temp.root(),
                path: "text".into(),
                bytes: b"local".to_vec(),
                baseline: Some(baseline),
            })
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Conflict);
        assert_eq!(fs::read(path).unwrap(), b"external");
    }

    #[test]
    fn writes_require_missing_baseline_and_roundtrip_new_baseline() {
        let temp = Temp::new();
        let Response::Written { baseline } = LocalBackend
            .call(Request::WriteFile {
                root: temp.root(),
                path: "text".into(),
                bytes: b"first".to_vec(),
                baseline: None,
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(
            LocalBackend
                .call(Request::WriteFile {
                    root: temp.root(),
                    path: "text".into(),
                    bytes: b"clobber".to_vec(),
                    baseline: None
                })
                .unwrap_err()
                .kind,
            ErrorKind::Conflict
        );
        LocalBackend
            .call(Request::WriteFile {
                root: temp.root(),
                path: "text".into(),
                bytes: b"second".to_vec(),
                baseline: Some(baseline),
            })
            .unwrap();
        assert_eq!(fs::read(temp.0.join("text")).unwrap(), b"second");
        assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 1);
    }

    #[test]
    fn editable_limit_accepts_full_files_and_rejects_oversized_writes_without_clobbering() {
        let temp = Temp::new();
        let path = temp.0.join("maximum.txt");
        let mut original = vec![b'a'; MAX_FILE_BYTES];
        original[..3].copy_from_slice(&[0xef, 0xbb, 0xbf]);
        original[MAX_FILE_BYTES - 2..].copy_from_slice(b"\r\n");
        fs::write(&path, &original).unwrap();
        let (mut bytes, baseline) = read_file(&path).unwrap();
        assert_eq!(bytes, original);
        bytes[3] = b'z';
        write_file(&path, &bytes, Some(&baseline)).unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes);
        bytes.push(b'x');
        let error = write_file(&path, &bytes, None).unwrap_err();
        assert_eq!(error.kind, ErrorKind::TooLarge);
        assert!(error.message.contains("maximum.txt"));
        assert!(error.message.contains(&(MAX_FILE_BYTES + 1).to_string()));
        assert!(error.message.contains("128 MiB"));
        assert!(error.message.contains("another editor"));
        assert_eq!(fs::metadata(&path).unwrap().len(), MAX_FILE_BYTES as u64);
        assert_eq!(fs::read(&path).unwrap(), &bytes[..MAX_FILE_BYTES]);
        assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 1);
    }

    #[test]
    fn raw_binary_reads_succeed_while_oversized_and_root_escape_are_rejected() {
        let temp = Temp::new();
        File::create(temp.0.join("large"))
            .unwrap()
            .set_len(MAX_FILE_BYTES as u64 + 1)
            .unwrap();
        fs::write(temp.0.join("binary"), vec![0; 1024]).unwrap();
        let error = LocalBackend
            .call(Request::ReadFile {
                root: temp.root(),
                path: "large".into(),
            })
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::TooLarge);
        assert!(error.message.contains("large"));
        assert!(error.message.contains(&(MAX_FILE_BYTES + 1).to_string()));
        assert!(error.message.contains("128 MiB"));
        assert!(matches!(
            LocalBackend.call(Request::ReadFile { root: temp.root(), path: "binary".into() }).unwrap(),
            Response::File { bytes, .. } if bytes == vec![0; 1024]
        ));
        assert_eq!(
            LocalBackend
                .call(Request::Canonicalize {
                    root: temp.root(),
                    path: "..".into(),
                    allow_missing: false
                })
                .unwrap_err()
                .kind,
            ErrorKind::PermissionDenied
        );
        assert_eq!(
            LocalBackend
                .call(Request::Remove {
                    root: temp.root(),
                    path: ".".into(),
                    is_directory: true
                })
                .unwrap_err()
                .kind,
            ErrorKind::PermissionDenied
        );
    }

    #[cfg(unix)]
    #[test]
    fn internal_file_symlinks_work_and_external_traversal_is_rejected() {
        use std::os::unix::fs::symlink;
        let temp = Temp::new();
        let outside = Temp::new();
        fs::write(temp.0.join("real"), b"text").unwrap();
        symlink(temp.0.join("real"), temp.0.join("alias")).unwrap();
        symlink(&outside.0, temp.0.join("external")).unwrap();
        assert!(
            LocalBackend
                .call(Request::ReadFile {
                    root: temp.root(),
                    path: "alias".into()
                })
                .is_ok()
        );
        assert_eq!(
            LocalBackend
                .call(Request::CreateFile {
                    root: temp.root(),
                    path: "external/escape".into()
                })
                .unwrap_err()
                .kind,
            ErrorKind::PermissionDenied
        );
        LocalBackend
            .call(Request::Remove {
                root: temp.root(),
                path: "alias".into(),
                is_directory: false,
            })
            .unwrap();
        assert_eq!(fs::read(temp.0.join("real")).unwrap(), b"text");
    }

    #[test]
    fn search_beyond_one_mib_keeps_editor_positions_and_reports_oversized_skips() {
        let temp = Temp::new();
        let mut bytes = b"\xef\xbb\xbfneedle\r\nneedle\rneedle\n".to_vec();
        bytes.resize(2 * 1024 * 1024, b'x');
        fs::write(temp.0.join("large.txt"), &bytes).unwrap();
        File::create(temp.0.join("oversized.txt"))
            .unwrap()
            .set_len(MAX_FILE_BYTES as u64 + 1)
            .unwrap();
        let Response::Search {
            matches,
            skipped_files,
            ..
        } = search(&temp.0, "NEEDLE", false, true, 100).unwrap()
        else {
            panic!()
        };
        assert_eq!(skipped_files, 1);
        assert_eq!(
            matches
                .iter()
                .map(|found| (found.line, found.editor_row, found.column))
                .collect::<Vec<_>>(),
            vec![(1, 1, 1), (2, 2, 1), (3, 3, 1)],
        );
        assert!(matches.iter().all(|found| found.line_bytes == b"needle"));
    }

    #[test]
    fn search_returns_original_byte_columns_for_unicode() {
        let temp = Temp::new();
        fs::write(temp.0.join("text"), "é MATCH\r\nmatch\n").unwrap();
        let Response::Search {
            matches, truncated, ..
        } = LocalBackend
            .call(Request::Search {
                root: temp.root(),
                query: "match".into(),
                case_sensitive: false,
                include_ignored: false,
                max_results: 10,
            })
            .unwrap()
        else {
            panic!()
        };
        assert!(!truncated);
        assert_eq!((matches[0].line, matches[0].column), (1, 4));
        assert_eq!((matches[1].line, matches[1].column), (2, 1));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn atomic_rename_preserves_a_destination_created_after_validation() {
        let temp = Temp::new();
        let source = temp.0.join("source");
        let target = temp.0.join("target");
        fs::write(&source, b"source").unwrap();
        assert!(!target.exists());
        // Simulate another process creating the destination after validation.
        fs::write(&target, b"preserve").unwrap();
        assert_eq!(
            rename_no_replace(&source, &target).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(&target).unwrap(), b"preserve");
        assert_eq!(fs::read(&source).unwrap(), b"source");
    }

    #[test]
    fn non_git_workspaces_have_empty_status_without_errors() {
        let temp = Temp::new();
        assert_eq!(
            LocalBackend
                .call(Request::GitStatus { root: temp.root() })
                .unwrap(),
            Response::GitStatus {
                entries: Vec::new()
            }
        );
    }
}
