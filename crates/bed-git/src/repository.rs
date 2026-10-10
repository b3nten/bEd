use crate::{
    ConflictChoice, Diff, DiffLine, DiffSide, Error, ErrorKind, Hunk, LineKind, Operation, Output,
    RepositoryOperation, Result, Status, StatusEntry,
    process::{self, Run},
};
use std::{
    fs,
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

fn locks() -> &'static Mutex<std::collections::HashSet<PathBuf>> {
    static LOCKS: OnceLock<Mutex<std::collections::HashSet<PathBuf>>> = OnceLock::new();
    LOCKS.get_or_init(Mutex::default)
}
struct RepositoryLock(PathBuf);
impl RepositoryLock {
    fn acquire(key: PathBuf) -> Result<Self> {
        if !locks().lock().unwrap().insert(key.clone()) {
            return Err(Error::new(
                ErrorKind::Busy,
                "Another Git operation is running for this repository",
            ));
        }
        Ok(Self(key))
    }
}
impl Drop for RepositoryLock {
    fn drop(&mut self) {
        locks().lock().unwrap().remove(&self.0);
    }
}

struct Repository {
    root: PathBuf,
    workspace: PathBuf,
    canceled: Arc<AtomicBool>,
}
impl Repository {
    fn open(root: &Path, canceled: Arc<AtomicBool>) -> Result<Option<Self>> {
        let workspace = fs::canonicalize(root)?;
        let run = process::git(
            &workspace,
            &["rev-parse", "--show-toplevel"],
            None,
            &canceled,
        )?;
        if !run.status.success() {
            if String::from_utf8_lossy(&run.stderr).contains("not a git repository") {
                return Ok(None);
            }
            return run.checked().map(|_| None);
        }
        let root = fs::canonicalize(text_line(&run.stdout)?)?;
        Ok(Some(Self {
            root,
            workspace,
            canceled,
        }))
    }
    fn run(&self, args: &[&str], input: Option<&[u8]>) -> Result<Run> {
        process::git(&self.root, args, input, &self.canceled)
    }
    fn checked(&self, args: &[&str], input: Option<&[u8]>) -> Result<Run> {
        self.run(args, input)?.checked()
    }
    fn writable(&self) -> Result<()> {
        if self.workspace != self.root {
            return Err(Error::new(
                ErrorKind::PermissionDenied,
                "Open the repository root as the workspace before changing Git state",
            ));
        }
        Ok(())
    }
    fn path(&self, path: &str) -> Result<PathBuf> {
        let relative = Path::new(path);
        if path.is_empty()
            || path.contains('\0')
            || relative.is_absolute()
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
            || relative.components().any(|part| part.as_os_str() == ".git")
        {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "Git paths must be literal repository-relative filenames",
            ));
        }
        let absolute = self.root.join(relative);
        // Follow parents, never the final entry: staging a symlink stages the
        // link itself, including one whose target is outside the repository.
        let parent = absolute.parent().unwrap();
        let ancestor = parent
            .ancestors()
            .find(|ancestor| ancestor.exists())
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::PermissionDenied,
                    "Git path has no repository parent",
                )
            })?;
        let parent = fs::canonicalize(ancestor)?;
        if !parent.starts_with(&self.workspace) {
            return Err(Error::new(
                ErrorKind::PermissionDenied,
                "Git path escapes workspace",
            ));
        }
        Ok(absolute)
    }
    fn metadata(&self, name: &str) -> Result<PathBuf> {
        Ok(PathBuf::from(text_line(
            &self
                .checked(
                    &["rev-parse", "--path-format=absolute", "--git-path", name],
                    None,
                )?
                .stdout,
        )?))
    }
    fn key(&self) -> Result<PathBuf> {
        let key = self.checked(
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            None,
        )?;
        Ok(fs::canonicalize(text_line(&key.stdout)?)?)
    }
    fn operation(&self) -> Result<RepositoryOperation> {
        for (name, operation) in [
            ("rebase-merge", RepositoryOperation::Rebase),
            ("rebase-apply", RepositoryOperation::Rebase),
            ("MERGE_HEAD", RepositoryOperation::Merge),
            ("CHERRY_PICK_HEAD", RepositoryOperation::CherryPick),
            ("REVERT_HEAD", RepositoryOperation::Revert),
        ] {
            if self.metadata(name)?.exists() {
                return Ok(operation);
            }
        }
        Ok(RepositoryOperation::None)
    }
    fn head(&self) -> Result<Option<String>> {
        let run = self.run(&["rev-parse", "--verify", "HEAD"], None)?;
        if run.status.success() {
            Ok(Some(text_line(&run.stdout)?.to_owned()))
        } else {
            Ok(None)
        }
    }
    fn status(&self) -> Result<Status> {
        let run = self.checked(
            &[
                "status",
                "--porcelain=v2",
                "-z",
                "--branch",
                "--untracked-files=all",
            ],
            None,
        )?;
        let mut status = Status {
            root: path_text(&self.root)?,
            writable: self.root == self.workspace,
            operation: self.operation()?,
            ..Status::default()
        };
        let mut records = run
            .stdout
            .split(|byte| *byte == 0)
            .filter(|record| !record.is_empty());
        while let Some(record) = records.next() {
            let text = std::str::from_utf8(record).map_err(|_| {
                Error::new(ErrorKind::InvalidInput, "Git filename is not valid UTF-8")
            })?;
            if let Some(head) = text.strip_prefix("# branch.oid ") {
                if head != "(initial)" {
                    status.head = Some(head.into());
                }
            } else if let Some(branch) = text.strip_prefix("# branch.head ") {
                status.branch = if branch == "(detached)" {
                    "Detached HEAD".into()
                } else {
                    branch.into()
                };
            } else if let Some(upstream) = text.strip_prefix("# branch.upstream ") {
                status.upstream = Some(upstream.into());
            } else if let Some(ab) = text.strip_prefix("# branch.ab ") {
                let mut values = ab.split_whitespace();
                status.ahead = values
                    .next()
                    .and_then(|s| s.strip_prefix('+'))
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                status.behind = values
                    .next()
                    .and_then(|s| s.strip_prefix('-'))
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
            } else if let Some(path) = text.strip_prefix("? ") {
                status.entries.push(StatusEntry {
                    path: path.into(),
                    old_path: None,
                    index_status: '?',
                    worktree_status: '?',
                    conflicted: false,
                });
            } else if matches!(record[0], b'1' | b'2' | b'u') {
                let fields = text
                    .splitn(
                        match record[0] {
                            b'1' => 9,
                            b'2' => 10,
                            _ => 11,
                        },
                        ' ',
                    )
                    .collect::<Vec<_>>();
                let path = *fields
                    .last()
                    .ok_or_else(|| Error::new(ErrorKind::Failed, "Malformed Git status"))?;
                let xy = fields
                    .get(1)
                    .filter(|s| s.len() == 2)
                    .ok_or_else(|| Error::new(ErrorKind::Failed, "Malformed Git status"))?
                    .as_bytes();
                let old_path = if record[0] == b'2' {
                    Some(
                        std::str::from_utf8(records.next().ok_or_else(|| {
                            Error::new(ErrorKind::Failed, "Missing renamed Git path")
                        })?)
                        .map_err(|_| {
                            Error::new(ErrorKind::InvalidInput, "Git filename is not UTF-8")
                        })?
                        .into(),
                    )
                } else {
                    None
                };
                status.entries.push(StatusEntry {
                    path: path.into(),
                    old_path,
                    index_status: normalize_status(xy[0]),
                    worktree_status: normalize_status(xy[1]),
                    conflicted: record[0] == b'u',
                });
            }
        }
        // A parent repository remains discoverable, but files outside the
        // workspace capability must never become readable through Git.
        status
            .entries
            .retain(|entry| self.root.join(&entry.path).starts_with(&self.workspace));
        let refs = self.checked(
            &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
            None,
        )?;
        status.branches = String::from_utf8_lossy(&refs.stdout)
            .lines()
            .map(str::to_owned)
            .collect();
        status.snapshot = fingerprint(&[&run.stdout]);
        Ok(status)
    }
    fn blob(&self, object: &str) -> Result<Vec<u8>> {
        let run = self.run(&["cat-file", "blob", object], None)?;
        if run.status.success() {
            Ok(run.stdout)
        } else {
            Ok(Vec::new())
        }
    }
    fn diff(&self, path: &str, side: DiffSide) -> Result<PreparedDiff> {
        let absolute = self.path(path)?;
        let status = self.status()?;
        let entry = status.entries.iter().find(|entry| entry.path == path);
        let old_path = entry.and_then(|entry| entry.old_path.clone());
        if let Some(old_path) = old_path.as_deref() {
            self.path(old_path)?;
        }
        let conflicted = entry.is_some_and(|entry| entry.conflicted);
        let old_object = match side {
            DiffSide::Staged => format!("HEAD:{}", old_path.as_deref().unwrap_or(path)),
            DiffSide::Unstaged if conflicted => format!(":2:{path}"),
            DiffSide::Unstaged => format!(":{path}"),
        };
        let index_entries = self
            .checked(&["ls-files", "--stage", "-z", "--", path], None)?
            .stdout;
        let index_mode = index_entries.get(..6);
        let head_entries = self
            .run(
                &[
                    "ls-tree",
                    "-z",
                    "HEAD",
                    "--",
                    old_path.as_deref().unwrap_or(path),
                ],
                None,
            )?
            .stdout;
        let head_mode = head_entries.get(..6);
        let metadata = fs::symlink_metadata(&absolute).ok();
        let symlink = metadata
            .as_ref()
            .is_some_and(|m| m.file_type().is_symlink())
            || index_mode == Some(b"120000")
            || head_mode == Some(b"120000");
        let gitlink = index_mode == Some(b"160000") || head_mode == Some(b"160000");
        let old_bytes = if gitlink {
            self.object_reference(&old_object)?
        } else {
            self.blob(&old_object)?
        };
        let new_bytes = if gitlink {
            match side {
                DiffSide::Staged => self.object_reference(&format!(":{path}"))?,
                DiffSide::Unstaged => {
                    if metadata.as_ref().is_some_and(|m| m.is_dir())
                        && absolute.join(".git").exists()
                        && fs::canonicalize(&absolute)?.starts_with(&self.workspace)
                    {
                        let run = process::git(
                            &absolute,
                            &["rev-parse", "--verify", "HEAD"],
                            None,
                            &self.canceled,
                        )?;
                        if run.status.success() {
                            format!("Subproject commit {}\n", text_line(&run.stdout)?).into_bytes()
                        } else {
                            Vec::new()
                        }
                    } else {
                        self.object_reference(&format!(":{path}"))?
                    }
                }
            }
        } else {
            match side {
                DiffSide::Staged => self.blob(&format!(":{path}"))?,
                DiffSide::Unstaged => read_entry(&absolute)?,
            }
        };
        let untracked = entry.is_some_and(|entry| entry.index_status == '?');
        let mut args = vec![
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--full-index",
            "--binary",
            "--unified=0",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "--output-indicator-new=+",
            "--output-indicator-old=-",
            "--output-indicator-context= ",
        ];
        if side == DiffSide::Staged {
            args.push("--cached");
        }
        if untracked && side == DiffSide::Unstaged {
            args.extend(["--no-index", "--", "/dev/null", path]);
        } else {
            args.extend(["--", path]);
            if let Some(old) = old_path.as_deref() {
                args.push(old);
            }
        }
        let run = self.run(&args, None)?;
        if !run.status.success() && !(untracked && run.status.code() == Some(1)) {
            return run.checked().map(|_| unreachable!());
        }
        let patch = run.stdout;
        let (hunks, patches, header) = parse_patch(&patch)?;
        // `binary` is the presentation's non-text summary flag, including
        // symlinks and gitlinks: a text editor must never follow their targets.
        let binary = symlink
            || gitlink
            || old_bytes.contains(&0)
            || new_bytes.contains(&0)
            || patch.windows(16).any(|w| w == b"GIT binary patch");
        let regular = fs::symlink_metadata(&absolute)
            .map(|m| m.is_file())
            .unwrap_or(false);
        let can_apply_hunks = regular
            && !binary
            && !conflicted
            && !untracked
            && old_path.is_none()
            && !patch.starts_with(b"diff --cc")
            && !patch.windows(14).any(|w| w == b"new file mode ")
            && !patch.windows(18).any(|w| w == b"deleted file mode ")
            && !patch.windows(9).any(|w| w == b"old mode ");
        let snapshot = fingerprint(&[path.as_bytes(), &patch, &old_bytes, &new_bytes]);
        Ok(PreparedDiff {
            diff: Diff {
                path: path.into(),
                old_path,
                side,
                snapshot,
                old_bytes,
                new_bytes,
                hunks,
                binary,
                can_apply_hunks,
            },
            patches,
            header,
        })
    }
    fn object_reference(&self, object: &str) -> Result<Vec<u8>> {
        let run = self.run(&["rev-parse", "--verify", object], None)?;
        if run.status.success() {
            Ok(format!("Subproject commit {}\n", text_line(&run.stdout)?).into_bytes())
        } else {
            Ok(Vec::new())
        }
    }
    fn paths<'a>(&self, paths: &'a [String]) -> Result<Vec<String>> {
        let status = self.status()?;
        let mut result = Vec::new();
        for path in paths {
            self.path(path)?;
            result.push(path.clone());
            if let Some(old) = status
                .entries
                .iter()
                .find(|entry| &entry.path == path)
                .and_then(|entry| entry.old_path.as_ref())
            {
                self.path(old)?;
                result.push(old.clone());
            }
        }
        if result.is_empty() {
            result.push(".".into());
        }
        Ok(result)
    }
    fn done(&self, args: &[&str], input: Option<&[u8]>) -> Result<Output> {
        Ok(Output::Done {
            output: self.checked(args, input)?.log(),
        })
    }
    fn apply_hunk(
        &self,
        path: &str,
        snapshot: &str,
        hunk: usize,
        side: DiffSide,
        discard: bool,
    ) -> Result<Output> {
        let prepared = self.diff(path, side)?;
        if prepared.diff.snapshot != snapshot {
            return Err(stale());
        }
        if !prepared.diff.can_apply_hunks {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "This change must be applied as a whole file",
            ));
        }
        let hunk = prepared
            .patches
            .get(hunk)
            .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "Unknown Git hunk"))?;
        let mut patch = prepared.header;
        patch.extend_from_slice(hunk);
        let mut args = vec!["apply", "--unidiff-zero", "--whitespace=nowarn"];
        if !discard {
            args.push("--cached");
        }
        if discard || side == DiffSide::Staged {
            args.push("--reverse");
        }
        let mut check = args.clone();
        check.push("--check");
        self.checked(&check, Some(&patch))?;
        self.done(&args, Some(&patch))
    }
    fn apply_range(
        &self,
        path: &str,
        snapshot: &str,
        old_range: &std::ops::Range<usize>,
        new_range: &std::ops::Range<usize>,
        side: DiffSide,
        discard: bool,
    ) -> Result<Output> {
        let prepared = self.diff(path, side)?;
        if prepared.diff.snapshot != snapshot {
            return Err(stale());
        }
        if !prepared.diff.can_apply_hunks
            || bare_cr(&prepared.diff.old_bytes)
            || bare_cr(&prepared.diff.new_bytes)
        {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "This change must be applied as a whole file",
            ));
        }
        let old_lines = lines(&prepared.diff.old_bytes);
        let raw_new_lines = lines(&prepared.diff.new_bytes);
        let canonical_new = if side == DiffSide::Unstaged && !discard {
            canonical_new_bytes(&prepared)?
        } else {
            prepared.diff.new_bytes.clone()
        };
        let new_lines = lines(&canonical_new);
        if new_lines.len() != raw_new_lines.len() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "Git filters change line counts; stage this file as a whole",
            ));
        }
        if old_range.start > old_range.end
            || old_range.end > old_lines.len()
            || new_range.start > new_range.end
            || new_range.end > new_lines.len()
            || (old_range.is_empty() && new_range.is_empty())
        {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "Displayed Git change ranges are outside the current diff",
            ));
        }
        let mut removed = old_lines[old_range.clone()]
            .iter()
            .map(|line| line.to_vec())
            .collect::<Vec<_>>();
        if discard {
            let crlf = prepared.diff.new_bytes.windows(2).any(|w| w == b"\r\n");
            if crlf {
                for line in &mut removed {
                    if line.ends_with(b"\n") && !line.ends_with(b"\r\n") {
                        line.insert(line.len() - 1, b'\r');
                    }
                }
            }
        }
        let target = if side == DiffSide::Staged || discard {
            new_range.start
        } else {
            old_range.start
        };
        let mut patch = prepared.header;
        append_range_header(&mut patch, target, removed.len(), new_range.len());
        for line in &removed {
            append_patch_line(&mut patch, b'-', line);
        }
        for line in &new_lines[new_range.clone()] {
            append_patch_line(&mut patch, b'+', line);
        }
        let mut args = vec!["apply", "--unidiff-zero", "--whitespace=nowarn"];
        if !discard {
            args.push("--cached");
        }
        if side == DiffSide::Staged || discard {
            args.push("--reverse");
        }
        let mut check = args.clone();
        check.push("--check");
        self.checked(&check, Some(&patch))?;
        self.done(&args, Some(&patch))
    }
}

fn lines(bytes: &[u8]) -> Vec<&[u8]> {
    bytes.split_inclusive(|b| *b == b'\n').collect()
}
fn bare_cr(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .enumerate()
        .any(|(i, b)| *b == b'\r' && bytes.get(i + 1) != Some(&b'\n'))
}
fn append_patch_line(patch: &mut Vec<u8>, prefix: u8, line: &[u8]) {
    patch.push(prefix);
    patch.extend_from_slice(line);
    if !line.ends_with(b"\n") {
        patch.extend_from_slice(b"\n\\ No newline at end of file\n");
    }
}
fn append_range_header(patch: &mut Vec<u8>, start: usize, old_count: usize, new_count: usize) {
    patch.extend_from_slice(
        format!(
            "@@ -{},{} +{},{} @@\n",
            if old_count == 0 { start } else { start + 1 },
            old_count,
            if new_count == 0 { start } else { start + 1 },
            new_count
        )
        .as_bytes(),
    );
}
fn canonical_new_bytes(prepared: &PreparedDiff) -> Result<Vec<u8>> {
    let old = lines(&prepared.diff.old_bytes);
    let mut result = Vec::new();
    let mut cursor = 0;
    for (hunk, patch) in prepared.diff.hunks.iter().zip(&prepared.patches) {
        if hunk.old_start < cursor || hunk.old_start + hunk.old_count > old.len() {
            return Err(stale());
        }
        for line in &old[cursor..hunk.old_start] {
            result.extend_from_slice(line);
        }
        let mut previous_added = false;
        for line in patch.split_inclusive(|b| *b == b'\n').skip(1) {
            match line.first() {
                Some(b'+') | Some(b' ') => {
                    result.extend_from_slice(&line[1..]);
                    previous_added = true;
                }
                Some(b'\\') if previous_added => {
                    if result.ends_with(b"\n") {
                        result.pop();
                    }
                    previous_added = false;
                }
                _ => {
                    previous_added = false;
                }
            }
        }
        cursor = hunk.old_start + hunk.old_count;
    }
    for line in &old[cursor..] {
        result.extend_from_slice(line);
    }
    Ok(result)
}

fn normalize_status(byte: u8) -> char {
    if byte == b'.' { ' ' } else { byte as char }
}
fn text_line(bytes: &[u8]) -> Result<&str> {
    std::str::from_utf8(bytes.strip_suffix(b"\n").unwrap_or(bytes))
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "Git path is not valid UTF-8"))
}
fn path_text(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "Git path is not valid UTF-8"))
}
fn stale() -> Error {
    Error::new(
        ErrorKind::Stale,
        "The Git diff changed; refresh and review it before applying this action",
    )
}
fn read_entry(path: &Path) -> Result<Vec<u8>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    if metadata.file_type().is_symlink() {
        return Ok(path_text(&fs::read_link(path)?)?.into_bytes());
    }
    if !metadata.is_file() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "Git diff supports files and symbolic links",
        ));
    }
    if metadata.len() > process::MAX_BYTES as u64 {
        return Err(Error::new(
            ErrorKind::TooLarge,
            "File exceeds the 16 MiB diff limit",
        ));
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "Git diff requires a regular file",
        ));
    }
    let mut bytes = Vec::new();
    use std::io::Read;
    file.take(process::MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > process::MAX_BYTES {
        return Err(Error::new(
            ErrorKind::TooLarge,
            "File exceeds the 16 MiB diff limit",
        ));
    }
    Ok(bytes)
}
fn fingerprint(parts: &[&[u8]]) -> String {
    // Stable change identity, not an authentication digest. Applying a patch
    // still asks Git to validate the exact old content while locking the index.
    let mut hash = 0xcbf29ce484222325_u64;
    for part in parts {
        for byte in (part.len() as u64).to_le_bytes().iter().chain(part.iter()) {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    format!("{hash:016x}")
}
struct PreparedDiff {
    diff: Diff,
    header: Vec<u8>,
    patches: Vec<Vec<u8>>,
}
fn range(text: &str) -> Result<(usize, usize)> {
    let (start, count) = text.split_once(',').unwrap_or((text, "1"));
    let start: usize = start
        .parse()
        .map_err(|_| Error::new(ErrorKind::Failed, "Malformed Git hunk range"))?;
    let count: usize = count
        .parse()
        .map_err(|_| Error::new(ErrorKind::Failed, "Malformed Git hunk range"))?;
    Ok((
        if count == 0 {
            start
        } else {
            start.saturating_sub(1)
        },
        count,
    ))
}
fn parse_patch(patch: &[u8]) -> Result<(Vec<Hunk>, Vec<Vec<u8>>, Vec<u8>)> {
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut patches: Vec<Vec<u8>> = Vec::new();
    let mut header = Vec::new();
    for line in patch.split_inclusive(|byte| *byte == b'\n') {
        if line.starts_with(b"@@ ") {
            let text = String::from_utf8_lossy(line);
            let mut fields = text.split_whitespace();
            fields.next();
            let old = fields
                .next()
                .and_then(|s| s.strip_prefix('-'))
                .ok_or_else(|| Error::new(ErrorKind::Failed, "Malformed Git hunk"))?;
            let new = fields
                .next()
                .and_then(|s| s.strip_prefix('+'))
                .ok_or_else(|| Error::new(ErrorKind::Failed, "Malformed Git hunk"))?;
            let (old_start, old_count) = range(old)?;
            let (new_start, new_count) = range(new)?;
            hunks.push(Hunk {
                old_start,
                old_count,
                new_start,
                new_count,
                header: text.trim_end_matches('\n').into(),
                lines: Vec::new(),
            });
            patches.push(line.to_vec());
        } else if let Some(hunk) = hunks.last_mut() {
            patches.last_mut().unwrap().extend_from_slice(line);
            let kind = match line.first() {
                Some(b' ') => Some(LineKind::Context),
                Some(b'+') => Some(LineKind::Added),
                Some(b'-') => Some(LineKind::Removed),
                _ => None,
            };
            if let Some(kind) = kind {
                hunk.lines.push(DiffLine {
                    kind,
                    text: String::from_utf8_lossy(
                        line[1..].strip_suffix(b"\n").unwrap_or(&line[1..]),
                    )
                    .into(),
                });
            }
        } else {
            header.extend_from_slice(line);
        }
    }
    Ok((hunks, patches, header))
}

pub fn repository_key(root: &Path) -> Result<PathBuf> {
    let canceled = Arc::new(AtomicBool::new(false));
    let repo = Repository::open(root, canceled)?
        .ok_or_else(|| Error::new(ErrorKind::NotRepository, "Folder is not a Git repository"))?;
    repo.key()
}
pub fn execute(root: &Path, operation: &Operation) -> Result<Output> {
    execute_with_cancel(root, operation, Arc::new(AtomicBool::new(false)))
}
pub(crate) fn execute_with_cancel(
    root: &Path,
    operation: &Operation,
    canceled: Arc<AtomicBool>,
) -> Result<Output> {
    let Some(repo) = Repository::open(root, canceled.clone())? else {
        return if matches!(operation, Operation::Status) {
            Ok(Output::Status(Status::default()))
        } else {
            Err(Error::new(
                ErrorKind::NotRepository,
                "Folder is not a Git repository",
            ))
        };
    };
    if canceled.load(Ordering::Acquire) {
        return Err(Error::new(ErrorKind::Canceled, "Git operation canceled"));
    }
    let _lock = if operation.is_mutating() {
        repo.writable()?;
        Some(RepositoryLock::acquire(repo.key()?)?)
    } else {
        None
    };
    match operation {
        Operation::Status => Ok(Output::Status(repo.status()?)),
        Operation::Diff { path, side } => Ok(Output::Diff(repo.diff(path, *side)?.diff)),
        Operation::Stage { paths } | Operation::Unstage { paths } => {
            if matches!(operation, Operation::Stage { .. }) {
                let status = repo.status()?;
                if paths.is_empty() && status.entries.iter().any(|entry| entry.conflicted) {
                    return Err(Error::new(
                        ErrorKind::InvalidInput,
                        "Stage each conflict resolution separately before staging all changes",
                    ));
                }
                for path in paths {
                    if status
                        .entries
                        .iter()
                        .any(|entry| entry.path == *path && entry.conflicted)
                    {
                        let bytes = read_entry(&repo.path(path)?)?;
                        if !bytes.contains(&0)
                            && bytes.split(|b| *b == b'\n').any(|line| {
                                [b"<<<<<<<".as_slice(), b"=======", b">>>>>>>", b"|||||||"]
                                    .iter()
                                    .any(|prefix| line.starts_with(prefix))
                            })
                        {
                            return Err(Error::new(
                                ErrorKind::InvalidInput,
                                "Remove all conflict markers before staging this resolution",
                            ));
                        }
                    }
                }
            }
            let paths = repo.paths(paths)?;
            let mut args = if matches!(operation, Operation::Stage { .. }) {
                vec!["add", "-A", "--"]
            } else if repo.head()?.is_some() {
                vec!["reset", "-q", "HEAD", "--"]
            } else {
                vec!["rm", "--cached", "--force", "--ignore-unmatch", "-r", "--"]
            };
            args.extend(paths.iter().map(String::as_str));
            repo.done(&args, None)
        }
        Operation::StageHunk {
            path,
            snapshot,
            hunk,
        } => repo.apply_hunk(path, snapshot, *hunk, DiffSide::Unstaged, false),
        Operation::UnstageHunk {
            path,
            snapshot,
            hunk,
        } => repo.apply_hunk(path, snapshot, *hunk, DiffSide::Staged, false),
        Operation::DiscardHunk {
            path,
            snapshot,
            hunk,
        } => repo.apply_hunk(path, snapshot, *hunk, DiffSide::Unstaged, true),
        Operation::StageRange {
            path,
            snapshot,
            old_range,
            new_range,
        } => repo.apply_range(
            path,
            snapshot,
            old_range,
            new_range,
            DiffSide::Unstaged,
            false,
        ),
        Operation::UnstageRange {
            path,
            snapshot,
            old_range,
            new_range,
        } => repo.apply_range(
            path,
            snapshot,
            old_range,
            new_range,
            DiffSide::Staged,
            false,
        ),
        Operation::DiscardRange {
            path,
            snapshot,
            old_range,
            new_range,
        } => repo.apply_range(
            path,
            snapshot,
            old_range,
            new_range,
            DiffSide::Unstaged,
            true,
        ),
        Operation::Discard { path, snapshot } => {
            let prepared = repo.diff(path, DiffSide::Unstaged)?;
            if &prepared.diff.snapshot != snapshot {
                return Err(stale());
            }
            let status = repo.status()?;
            if status
                .entries
                .iter()
                .any(|entry| entry.path == *path && entry.conflicted)
            {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    "Choose a conflict resolution before discarding this file",
                ));
            }
            if status
                .entries
                .iter()
                .any(|entry| entry.path == *path && entry.index_status == '?')
            {
                let absolute = repo.path(path)?;
                fs::remove_file(absolute)?;
                Ok(Output::Done {
                    output: String::new(),
                })
            } else {
                repo.done(&["restore", "--worktree", "--", path], None)
            }
        }
        Operation::Commit { message } => {
            if message.trim().is_empty() {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    "Enter a commit message",
                ));
            }
            if repo.status()?.entries.iter().any(|entry| entry.conflicted) {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    "Resolve and stage all conflicts before committing",
                ));
            }
            repo.done(&["commit", "--file=-"], Some(message.as_bytes()))
        }
        Operation::CreateBranch { name } | Operation::SwitchBranch { name } => {
            if name.starts_with('-') || name.contains('\0') {
                return Err(Error::new(ErrorKind::InvalidInput, "Invalid branch name"));
            }
            repo.checked(&["check-ref-format", "--branch", name], None)?;
            if matches!(operation, Operation::CreateBranch { .. }) {
                repo.done(&["switch", "-c", name], None)
            } else {
                repo.done(&["switch", name], None)
            }
        }
        Operation::Fetch => repo.done(&["fetch"], None),
        Operation::Pull => repo.done(&["pull"], None),
        Operation::Push => repo.done(&["-c", "push.autoSetupRemote=true", "push"], None),
        Operation::Resolve { path, choice } => {
            repo.path(path)?;
            if !repo
                .status()?
                .entries
                .iter()
                .any(|entry| entry.path == *path && entry.conflicted)
            {
                return Err(Error::new(
                    ErrorKind::Stale,
                    "This file is no longer conflicted",
                ));
            }
            match choice {
                ConflictChoice::Delete => repo.done(&["rm", "--", path], None),
                ConflictChoice::Current | ConflictChoice::Incoming => {
                    let side = if *choice == ConflictChoice::Current {
                        "--ours"
                    } else {
                        "--theirs"
                    };
                    repo.done(&["checkout", side, "--", path], None)
                }
            }
        }
        Operation::Continue => match repo.operation()? {
            RepositoryOperation::Rebase => repo.done(&["rebase", "--continue"], None),
            RepositoryOperation::CherryPick => repo.done(&["cherry-pick", "--continue"], None),
            RepositoryOperation::Revert => repo.done(&["revert", "--continue"], None),
            RepositoryOperation::Merge => repo.done(&["commit", "--no-edit"], None),
            RepositoryOperation::None => Err(Error::new(
                ErrorKind::InvalidInput,
                "No Git operation to continue",
            )),
        },
        Operation::Abort => match repo.operation()? {
            RepositoryOperation::Rebase => repo.done(&["rebase", "--abort"], None),
            RepositoryOperation::CherryPick => repo.done(&["cherry-pick", "--abort"], None),
            RepositoryOperation::Revert => repo.done(&["revert", "--abort"], None),
            RepositoryOperation::Merge => repo.done(&["merge", "--abort"], None),
            RepositoryOperation::None => Err(Error::new(
                ErrorKind::InvalidInput,
                "No Git operation to abort",
            )),
        },
    }
}
