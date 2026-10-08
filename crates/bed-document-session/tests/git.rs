//! Faithful translations of all six ned tests/editor/git_libgit2_test.cpp cases,
//! plus real-repository regression coverage. Attribution: LICENSE and NOTICE.
use bed_document_session::{
    editor::Editor,
    git::{git_repo::GitRepo, git_service::EditorGit, line_diff::diff_lines},
};
use bed_editing::{editor_commands::CursorReveal, editor_state::EditorState};
use git2::{Repository, Signature};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
struct TempRepo {
    root: PathBuf,
    repo: Option<Repository>,
}
impl TempRepo {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "bed-git-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let repo = Repository::init(&root).unwrap();
        Self {
            root,
            repo: Some(repo),
        }
    }
    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
    fn root(&self) -> &str {
        self.root.to_str().unwrap()
    }
    fn write(&self, name: &str, bytes: &[u8]) {
        let path = self.path(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    fn commit(&self, names: &[&str]) {
        let repo = self.repo.as_ref().unwrap();
        let mut index = repo.index().unwrap();
        for name in names {
            index.add_path(Path::new(name)).unwrap();
        }
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let parent = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
        let signature = Signature::now("bed-test", "bed-test@example.com").unwrap();
        let parents = parent.iter().collect::<Vec<_>>();
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            "fixture",
            &tree,
            &parents,
        )
        .unwrap();
    }
    fn dirty() -> Self {
        let fixture = Self::new();
        fixture.write("README", b"line1\n");
        fixture.commit(&["README"]);
        fixture.write("README", b"line1\nline2-modified\n");
        fixture
    }
}
impl Drop for TempRepo {
    fn drop(&mut self) {
        self.repo.take();
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn lines(values: &[&str]) -> Vec<Vec<u8>> {
    values.iter().map(|line| line.as_bytes().to_vec()).collect()
}
fn state(raw: &[u8], path: &Path) -> EditorState {
    let mut state = EditorState::new();
    state.set_from_bytes(raw);
    state.path = path.to_str().unwrap().to_owned();
    state
}

#[test]
fn git_repo_open_fails_on_non_repo() {
    let fixture = TempRepo::new();
    let plain = fixture.path("plain");
    fs::create_dir_all(&plain).unwrap();
    let mut repo = GitRepo::new();
    assert!(!repo.open(plain.to_str().unwrap()));
}
#[test]
fn modified_paths_detect_dirty_worktree_then_clear_on_restore() {
    let fixture = TempRepo::dirty();
    let mut repo = GitRepo::new();
    assert!(repo.open(fixture.root()));
    assert!(repo.modified_paths().contains(b"README".as_slice()));
    fixture.write("README", b"line1\n");
    assert!(!repo.modified_paths().contains(b"README".as_slice()));
}
#[test]
fn head_lines_then_line_diff_sees_unsaved_buffer_edits() {
    let fixture = TempRepo::dirty();
    fixture.write("README", b"line1\n");
    let mut repo = GitRepo::new();
    assert!(repo.open(fixture.root()));
    let mut head = Vec::new();
    assert!(repo.head_lines("README", &mut head));
    assert!(!head.is_empty());
    let mut buffer = head.clone();
    if buffer.len() == 1 {
        buffer.push(b"unsaved".to_vec());
    } else {
        buffer[1] = b"unsaved".to_vec();
    }
    let diff = diff_lines(&head, &buffer);
    assert!(diff.additions > 0);
    assert!(!diff.added_lines.is_empty());
}
#[test]
fn diff_lines_pure_unit() {
    let diff = diff_lines(&lines(&["a", "b", "c"]), &lines(&["a", "x", "c"]));
    assert_eq!((diff.additions, diff.deletions), (1, 1));
    assert!(diff.added_lines.contains(&2));
}
#[test]
fn single_inserted_blank_marks_only_that_line() {
    let old: Vec<Vec<u8>> = (0..20)
        .map(|i| format!("content-{i}").into_bytes())
        .collect();
    let mut new = old.clone();
    new.insert(5, Vec::new());
    let diff = diff_lines(&old, &new);
    assert_eq!((diff.additions, diff.deletions), (1, 0));
    assert_eq!(diff.added_lines, HashSet::from([6]));
}
#[test]
fn single_insert_in_large_file_does_not_mark_every_later_line() {
    let old: Vec<Vec<u8>> = (0..3000)
        .map(|i| format!("unique-line-body-{i}").into_bytes())
        .collect();
    let mut new = old.clone();
    new.insert(100, b"ONLY-NEW-LINE".to_vec());
    let diff = diff_lines(&old, &new);
    assert_eq!((diff.additions, diff.deletions), (1, 0));
    assert_eq!(diff.added_lines, HashSet::from([101]));
}

#[test]
fn head_baseline_is_cached_until_reopen_including_unborn_head() {
    let fixture = TempRepo::new();
    let mut repo = GitRepo::new();
    assert!(repo.open(fixture.root()));
    let mut head = lines(&["stale output"]);
    assert!(repo.head_lines("README", &mut head));
    assert!(head.is_empty());
    fixture.write("README", b"first\n");
    fixture.commit(&["README"]);
    assert!(repo.head_lines("README", &mut head));
    assert!(head.is_empty());
    assert!(repo.open(fixture.root()));
    assert!(repo.head_lines("README", &mut head));
    assert_eq!(head, lines(&["first", ""]));
    fixture.write("README", b"second\n");
    fixture.commit(&["README"]);
    assert!(repo.head_lines("README", &mut head));
    assert_eq!(head, lines(&["first", ""]));
    assert!(repo.open(fixture.root()));
    assert!(repo.head_lines("README", &mut head));
    assert_eq!(head, lines(&["second", ""]));
}
#[test]
fn head_split_keeps_bom_raw_bytes_mixed_endings_and_trailing_empty_row() {
    let fixture = TempRepo::new();
    fixture.write("mixed", b"\xef\xbb\xbfA\r\nB\rC\n\xff\0\n");
    fixture.commit(&["mixed"]);
    let mut repo = GitRepo::new();
    assert!(repo.open(fixture.root()));
    let mut head = Vec::new();
    assert!(repo.head_lines("mixed", &mut head));
    assert_eq!(
        head,
        vec![
            b"\xef\xbb\xbfA".to_vec(),
            b"B".to_vec(),
            b"C".to_vec(),
            b"\xff\0".to_vec(),
            Vec::new()
        ]
    );
    assert!(repo.head_lines("missing", &mut head));
    assert!(head.is_empty());
    assert!(repo.head_lines("", &mut head));
    assert!(head.is_empty());
}
#[test]
fn status_includes_index_deletions_and_nested_untracked_but_excludes_ignored() {
    let fixture = TempRepo::new();
    fixture.write("tracked", b"original\n");
    fixture.write("deleted", b"original\n");
    fixture.write(".gitignore", b"ignored/\n");
    fixture.commit(&["tracked", "deleted", ".gitignore"]);
    fixture.write("tracked", b"staged\n");
    let mut index = fixture.repo.as_ref().unwrap().index().unwrap();
    index.add_path(Path::new("tracked")).unwrap();
    index.write().unwrap();
    // Worktree restored after staging still has an index-to-worktree delta.
    fixture.write("tracked", b"original\n");
    fs::remove_file(fixture.path("deleted")).unwrap();
    fixture.write("new/nested.txt", b"untracked");
    fixture.write("ignored/data.txt", b"ignored");
    let mut repo = GitRepo::new();
    assert!(repo.open(fixture.root()));
    let paths = repo.modified_paths();
    assert!(paths.contains(b"tracked".as_slice()));
    assert!(paths.contains(b"deleted".as_slice()));
    assert!(paths.contains(b"new/nested.txt".as_slice()));
    assert!(!paths.contains(b"ignored/data.txt".as_slice()));
}
#[test]
fn real_editor_events_update_git_cache_for_multiple_actions_and_undo_immediately() {
    let fixture = TempRepo::new();
    fixture.write("README", b"line1\n");
    fixture.commit(&["README"]);
    let path = fixture.path("README");
    let path = path.to_str().unwrap();
    let mut editor = Editor::new();
    editor.api().on_project_opened(fixture.root());
    editor.api().open_document(path, b"line1\n");
    assert!(editor.git.borrow().current_git_changes.is_empty());
    editor
        .commands()
        .set_cursor(0, 5, false, CursorReveal::Ensure);
    editor.commands().insert_newline();
    assert_eq!(editor.git.borrow().current_git_changes, "+1-0");
    // Greedy equal-prefix stripping matches HEAD's trailing empty row before
    // adding the duplicate empty row at the end, as in the original LCS.
    assert!(!editor.git.borrow().is_line_edited(path, 2));
    assert!(editor.git.borrow().is_line_edited(path, 3));
    assert!(!editor.git.borrow().is_file_modified(path));
    editor.commands().undo();
    assert!(editor.git.borrow().current_git_changes.is_empty());
    editor
        .commands()
        .set_cursor(0, 0, false, CursorReveal::Ensure);
    editor.commands().type_text(b"X\n");
    editor
        .commands()
        .set_cursor(1, 0, false, CursorReveal::Ensure);
    editor.commands().type_text(b"Y");
    assert_eq!(editor.git.borrow().current_git_changes, "+2-1");
    assert!(editor.git.borrow().is_line_edited(path, 1));
    assert!(editor.git.borrow().is_line_edited(path, 2));
    assert!(!editor.git.borrow().is_line_edited("different-path", 1));
    editor.commands().undo();
    assert_eq!(editor.git.borrow().current_git_changes, "+1-0");
    editor.commands().undo();
    assert!(editor.git.borrow().current_git_changes.is_empty());
}
#[test]
fn disabled_edit_keeps_old_markers_until_document_open_and_non_repo_has_all_added() {
    let fixture = TempRepo::dirty();
    let path = fixture.path("README");
    let mut document = state(b"line1\nchanged\n", &path);
    let mut git = EditorGit::new();
    git.init(&document, fixture.root(), true);
    assert!(git.is_line_edited(&document.path, 2));
    document.set_from_bytes(b"line1\n");
    git.on_did_edit(&document, 0, 1, false);
    assert!(git.is_line_edited(&document.path, 2));
    git.on_document_opened(&document, false);
    assert!(git.current_git_changes.is_empty());
    assert!(!git.is_line_edited(&document.path, 2));
    git.on_document_opened(&document, true);
    assert!(git.current_git_changes.is_empty());
    git.init(&document, "", true);
    git.on_document_opened(&document, true);
    assert_eq!(git.current_git_changes, "+2-0");
    assert!(git.is_line_edited(&document.path, 1));
    assert!(git.is_line_edited(&document.path, 2));
}
#[test]
fn status_path_queries_preserve_deleted_files_and_new_missing_parents() {
    let fixture = TempRepo::new();
    fixture.write("deleted", b"original\n");
    fixture.commit(&["deleted"]);
    fs::remove_file(fixture.path("deleted")).unwrap();
    fixture.write("new/nested.txt", b"untracked");
    let mut git = EditorGit::new();
    git.init(&EditorState::new(), fixture.root(), true);
    assert!(git.is_file_modified(fixture.path("deleted").to_str().unwrap()));
    assert!(git.is_file_modified(fixture.path("new/nested.txt").to_str().unwrap()));
    assert!(!git.is_file_modified(fixture.path("absent/../other").to_str().unwrap()));
    assert!(!git.is_file_modified(fixture.path("../outside").to_str().unwrap()));
}

#[cfg(unix)]
#[test]
fn invalid_utf8_status_bytes_do_not_alias_a_valid_replacement_character_name() {
    let fixture = TempRepo::new();
    let valid = "broken-�";
    fixture.write(valid, b"clean");
    fixture.commit(&[valid]);
    let raw_name = b"broken-\xff";
    let mut index = fixture.repo.as_ref().unwrap().index().unwrap();
    let mut entry = index.get_path(Path::new(valid), 0).unwrap();
    entry.path = raw_name.to_vec();
    // Real Git index paths are opaque bytes. Skip-worktree avoids requiring a
    // filesystem/sandbox that permits non-UTF-8 names; status still reports the
    // staged addition from the original HEAD-to-index path source.
    entry.flags |= 0x4000;
    entry.flags_extended |= 1 << 14;
    index.add(&entry).unwrap();
    index.write().unwrap();
    let mut repo = GitRepo::new();
    assert!(repo.open(fixture.root()));
    let paths = repo.modified_paths();
    assert!(paths.contains(raw_name.as_slice()));
    assert!(!paths.contains(valid.as_bytes()));
    let mut service = EditorGit::new();
    service.init(&EditorState::new(), fixture.root(), true);
    assert!(!service.is_file_modified(fixture.path(valid).to_str().unwrap()));
}
