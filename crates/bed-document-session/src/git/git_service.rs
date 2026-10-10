//! Git gutter/status service translated from ned editor/services/git/git_service.{h,cpp}.
//! Source attribution and revision: LICENSE and NOTICE.
use crate::git::{git_repo::GitRepo, line_diff::diff_lines};
use bed_editing::editor_state::EditorState;
use std::{
    collections::{BTreeSet, HashSet},
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Default)]
pub struct EditorGit {
    pub current_git_changes: String,
    project_root: String,
    discover_from_document: bool,
    document_path: String,
    repo: GitRepo,
    baseline: Vec<Vec<u8>>,
    baseline_path: String,
    baseline_available: bool,
    current_lines: Vec<Vec<u8>>,
    dirty_lines: HashSet<i32>,
    modified_files: BTreeSet<Vec<u8>>,
    last_status: Option<Instant>,
}

impl EditorGit {
    /// Remote HEAD bytes feed the original line-diff algorithm without opening a local repository.
    pub fn install_remote_baseline(
        &mut self,
        state: &EditorState,
        bytes: Option<&[u8]>,
        enabled: bool,
    ) {
        self.document_path.clone_from(&state.path);
        self.discover_from_document = false;
        self.baseline_available = true;
        self.baseline = bytes
            .map(|raw| {
                let mut baseline = EditorState::new();
                baseline.set_from_bytes(raw);
                let mut lines = Vec::new();
                baseline.lines_into(&mut lines);
                lines
            })
            .unwrap_or_default();
        self.baseline_path.clone_from(&state.path);
        self.rebuild_line_cache(state);
        self.recompute_gutter_from_cache(state, enabled);
    }
    pub fn new() -> Self {
        Self::default()
    }
    pub fn init(&mut self, state: &EditorState, project_root: &str, enabled: bool) {
        self.clear_gutter();
        self.document_path.clone_from(&state.path);
        self.discover_from_document = project_root.is_empty();
        self.project_root = project_root.to_owned();
        self.modified_files.clear();
        self.repo.close();
        let directory = if project_root.is_empty() {
            Path::new(&state.path)
                .parent()
                .and_then(Path::to_str)
                .unwrap_or("")
        } else {
            project_root
        };
        if directory.is_empty() || !self.repo.open(directory) {
            return;
        }
        self.baseline_available = true;
        if let Some(workdir) = self.repo.workdir() {
            self.project_root = workdir.to_string_lossy().into_owned();
        }
        self.refresh_status();
        self.last_status = Some(Instant::now());
        if !state.path.is_empty() {
            self.on_document_opened(state, enabled);
        }
    }
    fn clear_gutter(&mut self) {
        self.baseline_available = false;
        self.dirty_lines.clear();
        self.current_git_changes.clear();
        self.baseline.clear();
        self.baseline_path.clear();
        self.current_lines.clear();
    }
    fn relative_path(&self, path: &str) -> String {
        if self.project_root.is_empty() || path.is_empty() {
            return String::new();
        }
        let (Ok(root), Ok(file)) = (
            weakly_canonical(Path::new(&self.project_root)),
            weakly_canonical(Path::new(path)),
        ) else {
            return String::new();
        };
        let Ok(relative) = file.strip_prefix(root) else {
            return String::new();
        };
        if relative.as_os_str().is_empty() {
            return ".".into();
        }
        relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    }
    fn load_baseline(&mut self, state: &EditorState) {
        self.baseline.clear();
        self.baseline_path.clear();
        if state.path.is_empty() || !self.repo.is_open() {
            return;
        }
        let relative = self.relative_path(&state.path);
        if relative.is_empty() {
            return;
        }
        self.repo.head_lines(&relative, &mut self.baseline);
        self.baseline_path.clone_from(&state.path);
    }
    fn rebuild_line_cache(&mut self, state: &EditorState) {
        state.lines_into(&mut self.current_lines);
    }
    fn sync_line_cache_from_edit(
        &mut self,
        state: &EditorState,
        first_row: i32,
        last_row: i32,
    ) -> bool {
        let new_n = state.line_count();
        if new_n <= 0 {
            self.current_lines.clear();
            return true;
        }
        let old_n = self.current_lines.len() as i32;
        if old_n == 0 || (new_n - old_n).abs() > 500 {
            self.rebuild_line_cache(state);
            return self.current_lines.len() == new_n as usize;
        }
        let mut lo = first_row.clamp(0, new_n - 1);
        let mut hi = last_row.clamp(0, new_n - 1);
        if lo > hi {
            std::mem::swap(&mut lo, &mut hi);
        }
        let new_dirty = hi - lo + 1;
        let old_dirty = new_dirty - (new_n - old_n);
        if old_dirty < 0 || lo + old_dirty > old_n {
            self.rebuild_line_cache(state);
            return self.current_lines.len() == new_n as usize;
        }
        self.current_lines.splice(
            lo as usize..(lo + old_dirty) as usize,
            std::iter::repeat_with(Vec::new).take(new_dirty as usize),
        );
        if self.current_lines.len() != new_n as usize {
            self.rebuild_line_cache(state);
            return self.current_lines.len() == new_n as usize;
        }
        for row in lo..=hi {
            state.line_into(row, &mut self.current_lines[row as usize], usize::MAX);
        }
        true
    }
    fn recompute_gutter_from_cache(&mut self, state: &EditorState, enabled: bool) {
        self.dirty_lines.clear();
        self.current_git_changes.clear();
        if !enabled || state.path.is_empty() || !self.baseline_available {
            return;
        }
        if self.baseline_path != state.path {
            self.load_baseline(state);
        }
        if self.current_lines.len() != state.line_count() as usize {
            self.rebuild_line_cache(state);
        }
        let diff = diff_lines(&self.baseline, &self.current_lines);
        self.dirty_lines = diff.added_lines;
        if diff.additions > 0 || diff.deletions > 0 {
            self.current_git_changes = format!("+{}-{}", diff.additions, diff.deletions);
        }
    }
    pub fn on_document_opened(&mut self, state: &EditorState, enabled: bool) {
        if self.discover_from_document && self.document_path != state.path {
            self.init(state, "", enabled);
            return;
        }
        self.document_path.clone_from(&state.path);
        self.load_baseline(state);
        self.rebuild_line_cache(state);
        self.recompute_gutter_from_cache(state, enabled);
    }
    pub fn on_did_edit(
        &mut self,
        state: &EditorState,
        first_row: i32,
        last_row: i32,
        enabled: bool,
    ) {
        self.document_path.clone_from(&state.path);
        // Preserve disabled marker/cache behavior: edits while disabled do not
        // clear existing markers or synchronize current_lines.
        if !enabled {
            return;
        }
        if self.baseline_path != state.path {
            self.load_baseline(state);
        }
        if !self.sync_line_cache_from_edit(state, first_row, last_row) {
            self.rebuild_line_cache(state);
        }
        self.recompute_gutter_from_cache(state, enabled);
    }
    fn refresh_status(&mut self) {
        self.modified_files = self.repo.modified_paths();
    }
    pub fn invalidate_baseline(&mut self, state: &EditorState, enabled: bool) {
        self.repo.invalidate_head();
        self.load_baseline(state);
        self.rebuild_line_cache(state);
        self.recompute_gutter_from_cache(state, enabled);
        self.refresh_status();
        self.last_status = Some(Instant::now());
    }
    pub fn poll(&mut self) {
        self.poll_at(Instant::now());
    }
    fn poll_at(&mut self, now: Instant) {
        if !self.repo.is_open()
            || self
                .last_status
                .is_some_and(|last| now.duration_since(last) < Duration::from_millis(1000))
        {
            return;
        }
        self.refresh_status();
        self.last_status = Some(now);
    }
    pub fn is_line_edited(&self, path: &str, line_number: i32) -> bool {
        path == self.document_path && self.dirty_lines.contains(&line_number)
    }
    pub fn is_file_modified(&self, path: &str) -> bool {
        self.repo.is_open()
            && self
                .modified_files
                .contains(self.relative_path(path).as_bytes())
    }
}

// std has canonicalize but not weakly_canonical. Canonicalize the longest
// existing prefix and normalize the remaining components, preserving deleted
// and new paths as the original filesystem::relative algorithm does.
fn weakly_canonical(path: &Path) -> std::io::Result<PathBuf> {
    let mut prefix = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut missing = Vec::new();
    let mut result = loop {
        match std::fs::canonicalize(&prefix) {
            Ok(path) => break path,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                let Some(last) = prefix.components().next_back() else {
                    return Err(error);
                };
                if matches!(last, Component::RootDir | Component::Prefix(_)) {
                    return Err(error);
                }
                missing.push(last.as_os_str().to_owned());
                if !prefix.pop() {
                    return Err(error);
                }
            }
            Err(error) => return Err(error),
        }
    };
    for part in missing.into_iter().rev() {
        if part == ".." {
            result.pop();
        } else if part != "." {
            result.push(part);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use bed_editing::editor_operations::{EditorOperations, OpKind, TextOp};

    #[test]
    fn incremental_cache_splices_insertions_and_multi_row_deletions() {
        let mut state = EditorState::new();
        state.set_from_bytes(b"a\nbb\nc\nd");
        let mut service = EditorGit::new();
        service.rebuild_line_cache(&state);
        let mut ops = EditorOperations::new();
        ops.apply(
            &mut state,
            &TextOp {
                row: 1,
                column: 1,
                text: b"\nX\n".to_vec(),
                ..Default::default()
            },
        );
        assert!(service.sync_line_cache_from_edit(&state, 1, 3));
        assert_eq!(service.current_lines, state.lines());
        ops.apply(
            &mut state,
            &TextOp {
                kind: OpKind::Delete,
                row: 0,
                column: 1,
                length: 7,
                ..Default::default()
            },
        );
        assert!(service.sync_line_cache_from_edit(&state, 0, 0));
        assert_eq!(service.current_lines, state.lines());
    }

    #[test]
    fn cold_huge_and_incoherent_dirty_spans_fall_back_to_full_cache() {
        let mut state = EditorState::new();
        state.set_from_bytes(b"a\nb\nc\nd");
        let mut service = EditorGit::new();
        assert!(service.sync_line_cache_from_edit(&state, 3, 1));
        assert_eq!(service.current_lines, state.lines());
        state.set_from_bytes(
            &(0..900)
                .map(|i| format!("line{i}\n"))
                .collect::<String>()
                .into_bytes(),
        );
        assert!(service.sync_line_cache_from_edit(&state, 0, 0));
        assert_eq!(service.current_lines, state.lines());
        state.set_from_bytes(b"a");
        assert!(service.sync_line_cache_from_edit(&state, 0, 0));
        state.set_from_bytes(b"a\nb\nc\nd");
        assert!(service.sync_line_cache_from_edit(&state, 999, 999));
        assert_eq!(service.current_lines, state.lines());
        state.set_from_bytes(b"a\nB\nC\nD");
        assert!(service.sync_line_cache_from_edit(&state, 3, -10));
        assert_eq!(service.current_lines, state.lines());
    }

    #[test]
    fn standalone_files_discover_git_and_files_outside_git_have_no_gutter() {
        let temp = TempDir::new();
        let repo = git2::Repository::init(temp.root()).unwrap();
        let file = temp.write("nested/note.txt", b"base\n");
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("nested/note.txt")).unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let signature = git2::Signature::now("Test", "test@example.invalid").unwrap();
        repo.commit(Some("HEAD"), &signature, &signature, "base", &tree, &[])
            .unwrap();
        let mut state = EditorState::new();
        state.path = file.to_string_lossy().into();
        state.set_from_bytes(b"edited\n");
        let mut service = EditorGit::new();
        service.init(&state, "", true);
        assert!(service.is_line_edited(&state.path, 1));
        let outside = TempDir::new();
        state.path = outside
            .write("note.txt", b"outside\n")
            .to_string_lossy()
            .into();
        service.on_document_opened(&state, true);
        assert!(!service.repo.is_open());
        assert!(service.dirty_lines.is_empty());
        state.path = file.to_string_lossy().into();
        service.on_document_opened(&state, true);
        assert!(service.repo.is_open());
        assert!(service.is_line_edited(&state.path, 1));
    }

    #[test]
    fn status_poll_refreshes_at_one_second_boundary_and_clears_on_project_switch() {
        let temp = TempDir::new();
        let _repo = git2::Repository::init(temp.root()).unwrap();
        let mut service = EditorGit::new();
        service.init(&EditorState::new(), temp.root().to_str().unwrap(), true);
        let last = service.last_status.unwrap();
        let file = temp.write("untracked", b"new");
        let path = file.to_str().unwrap();
        service.poll_at(last + Duration::from_millis(999));
        assert!(!service.is_file_modified(path));
        service.poll_at(last + Duration::from_millis(1000));
        assert!(service.is_file_modified(path));
        std::fs::remove_file(&file).unwrap();
        service.poll_at(last + Duration::from_millis(2000));
        assert!(!service.is_file_modified(path));
        service.init(&EditorState::new(), "", true);
        assert!(service.modified_files.is_empty());
        assert!(!service.repo.is_open());
    }

    #[test]
    fn weak_paths_normalize_missing_suffixes_and_reject_outside_roots() {
        let temp = TempDir::new();
        let mut service = EditorGit::new();
        service.init(&EditorState::new(), temp.root().to_str().unwrap(), true);
        let missing = temp.path("missing/../new.txt");
        assert_eq!(service.relative_path(missing.to_str().unwrap()), "new.txt");
        assert_eq!(service.relative_path(temp.root().to_str().unwrap()), ".");
        assert!(
            service
                .relative_path(temp.path("../outside.txt").to_str().unwrap())
                .is_empty()
        );
    }

    #[cfg(unix)]
    #[test]
    fn weak_paths_follow_existing_symlink_prefixes_before_normalizing() {
        let temp = TempDir::new();
        let inside = temp.path("inside");
        std::fs::create_dir_all(&inside).unwrap();
        std::os::unix::fs::symlink(&inside, temp.path("link")).unwrap();
        let mut service = EditorGit::new();
        service.init(&EditorState::new(), temp.root().to_str().unwrap(), true);
        assert_eq!(
            service.relative_path(temp.path("link/missing.txt").to_str().unwrap()),
            "inside/missing.txt"
        );
        let outside = TempDir::new();
        std::os::unix::fs::symlink(outside.root(), temp.path("outside-link")).unwrap();
        assert!(
            service
                .relative_path(temp.path("outside-link/missing.txt").to_str().unwrap())
                .is_empty()
        );
    }
}
