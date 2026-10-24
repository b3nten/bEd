use super::*;
use bedtime_editor2::{BufferEdit, TextBuffer};
use bedtime_git::Status;
use std::{fs, process::Command, sync::atomic::AtomicU64};

pub(super) struct Repo(pub(super) PathBuf);
impl Repo {
    pub(super) fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "bed2-git-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        let repo = Self(fs::canonicalize(path).unwrap());
        repo.git(&["init", "-b", "main"]);
        repo.git(&["config", "user.name", "Bed Test"]);
        repo.git(&["config", "user.email", "bed@example.invalid"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        repo.git(&["config", "core.hooksPath", ".git/hooks"]);
        repo
    }
    pub(super) fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.0)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
    pub(super) fn write(&self, name: &str, text: &str) {
        fs::write(self.0.join(name), text).unwrap();
    }
    pub(super) fn initial(&self) {
        self.write("a.txt", "original\n");
        self.git(&["add", "a.txt"]);
        self.git(&["commit", "-m", "Initial"]);
    }
    #[cfg(unix)]
    fn hook(&self) {
        use std::os::unix::fs::PermissionsExt;
        let path = self.0.join(".git/hooks/pre-commit");
        fs::write(&path, "#!/bin/sh\ntouch .git/hook-started\nwhile [ ! -f .git/release-hook ]; do sleep 0.05; done\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}
impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(super) fn host(root: &Path) -> (App, Entity, Entity) {
    let mut app = App::new();
    app.add_plugins((
        WorkbenchCorePlugin,
        crate::file_editor::FileEditorPlugin,
        GitPlugin,
    ));
    let workspace = Workspaces::new(app.world_mut()).open("Git test").entity();
    app.world_mut()
        .entity_mut(workspace)
        .insert(crate::files::WorkspaceRoot(root.to_owned()));
    let layout = active_layout(app.world(), workspace).unwrap();
    let area = Workbench::for_entity(app.world_mut(), layout)
        .unwrap()
        .destination_area(layout)
        .unwrap();
    let panel = app
        .world_mut()
        .spawn((GitPanel::default(), InArea(area)))
        .id();
    settle(&mut app, workspace);
    (app, workspace, panel)
}
pub(super) fn wait(app: &mut App, mut done: impl FnMut(&World) -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        app.update();
        if done(app.world()) {
            return;
        }
        assert!(Instant::now() < until, "Git worker did not settle");
        std::thread::sleep(Duration::from_millis(2));
    }
}
pub(super) fn settle(app: &mut App, workspace: Entity) {
    wait(app, |w| {
        w.get::<GitSession>(workspace).is_some_and(|s| {
            s.view.is_some() && s.scan.is_none() && s.pending.is_none() && !s.refresh
        })
    });
}
fn action(app: &mut App, workspace: Entity, operation: Operation) -> Result<(), String> {
    let mut session = app
        .world_mut()
        .entity_mut(workspace)
        .take::<GitSession>()
        .unwrap();
    let result = begin(app.world_mut(), workspace, &mut session, operation);
    app.world_mut().entity_mut(workspace).insert(session);
    result
}
fn refresh(app: &mut App, workspace: Entity) {
    app.world_mut()
        .get_mut::<GitSession>(workspace)
        .unwrap()
        .invalidate();
    settle(app, workspace);
}
pub(super) fn bind(app: &mut App, panel: Entity, repo: &Repo) -> DocumentHandle<TextBuffer> {
    let text = TextBuffer::from(fs::read_to_string(repo.0.join("a.txt")).unwrap());
    crate::file_editor::bind_loaded(app.world_mut(), panel, &repo.0.join("a.txt"), &text, None)
        .unwrap();
    let id = app.world().get::<DocumentView>(panel).unwrap().document();
    cast_document(app.world(), id).unwrap()
}
pub(super) fn edit(app: &mut App, document: DocumentHandle<TextBuffer>, text: &str) {
    let info = inspect_document(app.world(), document.id()).unwrap();
    let len = read_document(app.world(), document).unwrap().len();
    edit_document(
        app.world_mut(),
        document,
        info.revision,
        BufferEdit::new(0..len, text),
        EditOptions::new("Test edit"),
    )
    .unwrap();
}

#[test]
fn stage_partial_unstage_and_commit_preserve_newer_message() {
    let repo = Repo::new();
    repo.initial();
    repo.write("a.txt", "changed\n");
    let (mut app, workspace, _) = host(&repo.0);
    action(
        &mut app,
        workspace,
        Operation::Stage {
            paths: vec!["a.txt".into()],
        },
    )
    .unwrap();
    settle(&mut app, workspace);
    assert_eq!(repo.git(&["show", ":a.txt"]), "changed\n");
    repo.write("a.txt", "more changes\n");
    refresh(&mut app, workspace);
    assert_eq!(
        staging(
            &app.world()
                .get::<GitSession>(workspace)
                .unwrap()
                .view
                .as_ref()
                .unwrap()
                .status
                .entries[0]
        ),
        Staging::Partial
    );
    action(
        &mut app,
        workspace,
        Operation::Unstage {
            paths: vec!["a.txt".into()],
        },
    )
    .unwrap();
    settle(&mut app, workspace);
    assert_eq!(repo.git(&["show", ":a.txt"]), "original\n");
    action(
        &mut app,
        workspace,
        Operation::Stage {
            paths: vec!["a.txt".into()],
        },
    )
    .unwrap();
    settle(&mut app, workspace);
    app.world_mut()
        .get_mut::<GitSession>(workspace)
        .unwrap()
        .draft = "Submitted".into();
    action(
        &mut app,
        workspace,
        Operation::Commit {
            mode: Default::default(),
            message: "Submitted".into(),
        },
    )
    .unwrap();
    app.world_mut()
        .get_mut::<GitSession>(workspace)
        .unwrap()
        .draft = "Next commit".into();
    settle(&mut app, workspace);
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]).trim(), "Submitted");
    assert_eq!(
        app.world().get::<GitSession>(workspace).unwrap().draft,
        "Next commit"
    );
    assert!(
        app.world()
            .get::<GitSession>(workspace)
            .unwrap()
            .error
            .is_none()
    );
}

#[test]
fn staging_saves_the_shared_document_before_reading_working_bytes() {
    let repo = Repo::new();
    repo.initial();
    let (mut app, workspace, panel) = host(&repo.0);
    let document = bind(&mut app, panel, &repo);
    edit(&mut app, document, "unsaved document\n");
    action(
        &mut app,
        workspace,
        Operation::Stage {
            paths: vec!["a.txt".into()],
        },
    )
    .unwrap();
    settle(&mut app, workspace);
    assert_eq!(repo.git(&["show", ":a.txt"]), "unsaved document\n");
    assert!(!inspect_document(app.world(), document.id()).unwrap().dirty);
    assert!(
        app.world()
            .get::<GitSession>(workspace)
            .unwrap()
            .error
            .is_none()
    );
}

#[test]
fn edits_after_save_capture_stop_staging_without_losing_text() {
    let repo = Repo::new();
    repo.initial();
    let (mut app, workspace, panel) = host(&repo.0);
    let document = bind(&mut app, panel, &repo);
    edit(&mut app, document, "saved capture\n");
    action(
        &mut app,
        workspace,
        Operation::Stage {
            paths: vec!["a.txt".into()],
        },
    )
    .unwrap();
    edit(&mut app, document, "newer edit\n");
    settle(&mut app, workspace);
    assert_eq!(repo.git(&["show", ":a.txt"]), "original\n");
    assert_eq!(
        read_document(app.world(), document)
            .unwrap()
            .range_text(0..11),
        "newer edit\n"
    );
    assert!(
        app.world()
            .get::<GitSession>(workspace)
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .contains("changed")
    );
}

#[test]
fn failed_source_checked_save_never_stages_external_changes() {
    let repo = Repo::new();
    repo.initial();
    let (mut app, workspace, panel) = host(&repo.0);
    let document = bind(&mut app, panel, &repo);
    edit(&mut app, document, "my edits\n");
    repo.write("a.txt", "external\n");
    action(
        &mut app,
        workspace,
        Operation::Stage {
            paths: vec!["a.txt".into()],
        },
    )
    .unwrap();
    settle(&mut app, workspace);
    assert_eq!(repo.git(&["show", ":a.txt"]), "original\n");
    assert_eq!(
        fs::read_to_string(repo.0.join("a.txt")).unwrap(),
        "external\n"
    );
    assert!(inspect_document(app.world(), document.id()).unwrap().dirty);
    assert!(
        app.world()
            .get::<GitSession>(workspace)
            .unwrap()
            .error
            .is_some()
    );
}

#[test]
fn discard_rejects_dirty_buffers_and_reconciles_clean_documents_with_undo() {
    let repo = Repo::new();
    repo.initial();
    repo.write("a.txt", "working\n");
    let (mut app, workspace, panel) = host(&repo.0);
    let document = bind(&mut app, panel, &repo);
    let Output::Diff(diff) = bedtime_git::execute(
        &repo.0,
        &Operation::Diff {
            path: "a.txt".into(),
            side: DiffSide::Unstaged,
        },
    )
    .unwrap() else {
        panic!()
    };
    edit(&mut app, document, "dirty\n");
    assert!(
        action(
            &mut app,
            workspace,
            Operation::Discard {
                path: "a.txt".into(),
                snapshot: diff.snapshot.clone()
            }
        )
        .is_err()
    );
    undo_document(app.world_mut(), document.id()).unwrap();
    action(
        &mut app,
        workspace,
        Operation::Discard {
            path: "a.txt".into(),
            snapshot: diff.snapshot,
        },
    )
    .unwrap();
    settle(&mut app, workspace);
    assert_eq!(
        fs::read_to_string(repo.0.join("a.txt")).unwrap(),
        "original\n"
    );
    assert_eq!(
        read_document(app.world(), document)
            .unwrap()
            .range_text(0..9),
        "original\n"
    );
    assert!(!inspect_document(app.world(), document.id()).unwrap().dirty);
    undo_document(app.world_mut(), document.id()).unwrap();
    assert_eq!(
        read_document(app.world(), document)
            .unwrap()
            .range_text(0..8),
        "working\n"
    );
}

#[test]
fn stale_discard_and_external_index_change_before_commit_are_rejected() {
    let repo = Repo::new();
    repo.initial();
    repo.write("a.txt", "working\n");
    let (mut app, workspace, _) = host(&repo.0);
    let Output::Diff(diff) = bedtime_git::execute(
        &repo.0,
        &Operation::Diff {
            path: "a.txt".into(),
            side: DiffSide::Unstaged,
        },
    )
    .unwrap() else {
        panic!()
    };
    repo.write("a.txt", "new external\n");
    action(
        &mut app,
        workspace,
        Operation::Discard {
            path: "a.txt".into(),
            snapshot: diff.snapshot,
        },
    )
    .unwrap();
    settle(&mut app, workspace);
    assert_eq!(
        fs::read_to_string(repo.0.join("a.txt")).unwrap(),
        "new external\n"
    );
    assert!(
        app.world()
            .get::<GitSession>(workspace)
            .unwrap()
            .error
            .is_some()
    );
    action(
        &mut app,
        workspace,
        Operation::Stage {
            paths: vec!["a.txt".into()],
        },
    )
    .unwrap();
    settle(&mut app, workspace);
    action(
        &mut app,
        workspace,
        Operation::Commit {
            mode: Default::default(),
            message: "Do not commit extra content".into(),
        },
    )
    .unwrap();
    repo.write("a.txt", "unexpected staged content\n");
    repo.git(&["add", "a.txt"]);
    settle(&mut app, workspace);
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]).trim(), "Initial");
}

#[test]
fn nested_workspace_is_read_only_and_non_repository_is_a_clean_empty_state() {
    let repo = Repo::new();
    repo.initial();
    fs::create_dir(repo.0.join("nested")).unwrap();
    let (mut app, workspace, _) = host(&repo.0.join("nested"));
    assert!(
        !app.world()
            .get::<GitSession>(workspace)
            .unwrap()
            .view
            .as_ref()
            .unwrap()
            .status
            .writable
    );
    assert!(action(&mut app, workspace, Operation::Stage { paths: vec![] }).is_err());
    let temporary = Repo::new();
    fs::remove_dir_all(temporary.0.join(".git")).unwrap();
    let (app, workspace, _) = host(&temporary.0);
    let session = app.world().get::<GitSession>(workspace).unwrap();
    assert!(session.view.as_ref().unwrap().status.root.is_empty());
    assert!(session.error.is_none());
}

#[test]
fn refresh_requests_coalesce_and_old_reads_do_not_acknowledge_a_write() {
    let repo = Repo::new();
    repo.initial();
    repo.write("a.txt", "modified\n");
    let (mut app, workspace, _) = host(&repo.0);
    let started = app.world().get::<GitSession>(workspace).unwrap().scans;
    for _ in 0..10 {
        app.world_mut()
            .get_mut::<GitSession>(workspace)
            .unwrap()
            .invalidate();
    }
    settle(&mut app, workspace);
    assert_eq!(
        app.world().get::<GitSession>(workspace).unwrap().scans,
        started + 1
    );
    action(
        &mut app,
        workspace,
        Operation::Stage {
            paths: vec!["a.txt".into()],
        },
    )
    .unwrap();
    assert!(!app.world().get::<GitSession>(workspace).unwrap().writable());
    settle(&mut app, workspace);
    assert!(app.world().get::<GitSession>(workspace).unwrap().writable());
    assert_eq!(
        app.world()
            .get::<GitSession>(workspace)
            .unwrap()
            .view
            .as_ref()
            .unwrap()
            .status
            .entries[0]
            .index_status,
        'M'
    );
}

#[test]
fn file_write_leases_cover_descendant_workspaces_and_release_on_drop() {
    let writes = crate::file_editor::FileWrites::default();
    let file = writes.file(Path::new("/project/sub/a.txt")).unwrap();
    assert!(writes.repository(Path::new("/project")).is_err());
    drop(file);
    let repo = writes.repository(Path::new("/project")).unwrap();
    assert!(writes.file(Path::new("/project/sub/a.txt")).is_err());
    assert!(writes.repository(Path::new("/project/sub")).is_err());
    assert!(writes.file(Path::new("/elsewhere/a.txt")).is_ok());
    drop(repo);
    assert!(writes.file(Path::new("/project/sub/a.txt")).is_ok());
}

#[cfg(unix)]
#[test]
fn closing_a_panel_keeps_a_commit_alive_and_cancellation_keeps_the_draft() {
    let repo = Repo::new();
    repo.initial();
    repo.hook();
    repo.write("a.txt", "modified\n");
    repo.git(&["add", "a.txt"]);
    let (mut app, workspace, panel) = host(&repo.0);
    app.world_mut()
        .get_mut::<GitSession>(workspace)
        .unwrap()
        .draft = "Commit through closed panel".into();
    action(
        &mut app,
        workspace,
        Operation::Commit {
            mode: Default::default(),
            message: "Commit through closed panel".into(),
        },
    )
    .unwrap();
    wait(&mut app, |_| repo.0.join(".git/hook-started").exists());
    app.world_mut().despawn(panel);
    fs::write(repo.0.join(".git/release-hook"), "").unwrap();
    settle(&mut app, workspace);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%s"]).trim(),
        "Commit through closed panel"
    );
    assert!(
        app.world()
            .get::<GitSession>(workspace)
            .unwrap()
            .draft
            .is_empty()
    );
    fs::remove_file(repo.0.join(".git/hook-started")).unwrap();
    fs::remove_file(repo.0.join(".git/release-hook")).unwrap();
    repo.write("a.txt", "next\n");
    repo.git(&["add", "a.txt"]);
    refresh(&mut app, workspace);
    app.world_mut()
        .get_mut::<GitSession>(workspace)
        .unwrap()
        .draft = "Keep on cancellation".into();
    action(
        &mut app,
        workspace,
        Operation::Commit {
            mode: Default::default(),
            message: "Keep on cancellation".into(),
        },
    )
    .unwrap();
    wait(&mut app, |_| repo.0.join(".git/hook-started").exists());
    cancel(&mut app.world_mut().get_mut::<GitSession>(workspace).unwrap());
    settle(&mut app, workspace);
    assert_eq!(
        app.world().get::<GitSession>(workspace).unwrap().draft,
        "Keep on cancellation"
    );
    assert!(!repo.0.join(".git/index.lock").exists());
    assert!(
        app.world()
            .get::<GitSession>(workspace)
            .unwrap()
            .error
            .is_some()
    );
}

#[test]
fn grouped_rows_preserve_staged_new_files_and_partial_group_state() {
    let view = RepositoryView::new(Status {
        entries: vec![
            StatusEntry {
                path: "b.txt".into(),
                old_path: None,
                index_status: 'A',
                worktree_status: ' ',
                conflicted: false,
            },
            StatusEntry {
                path: "a.txt".into(),
                old_path: None,
                index_status: '?',
                worktree_status: '?',
                conflicted: false,
            },
            StatusEntry {
                path: "c.txt".into(),
                old_path: None,
                index_status: 'M',
                worktree_status: 'M',
                conflicted: false,
            },
        ],
        ..Default::default()
    });
    assert_eq!(view.staged_count, 2);
    assert!(matches!(
        &view.rows[0],
        Row::Header(Group::Tracked, _, Staging::Partial)
    ));
    assert!(
        matches!(&view.rows[2], Row::Header(Group::Untracked, indices, Staging::Partial) if indices.iter().collect::<Vec<_>>() == vec![1,0])
    );
}

#[test]
fn branch_switch_refreshes_retained_documents_without_a_filetree_panel() {
    let repo = Repo::new();
    repo.initial();
    repo.git(&["switch", "-c", "other"]);
    repo.write("a.txt", "other branch\n");
    repo.git(&["commit", "-am", "Other"]);
    repo.git(&["switch", "main"]);
    let (mut app, workspace, panel) = host(&repo.0);
    let document = bind(&mut app, panel, &repo);
    bedtime_editor2::workbench::unbind_text_editor(app.world_mut(), panel).unwrap();
    action(
        &mut app,
        workspace,
        Operation::SwitchBranch {
            name: "other".into(),
        },
    )
    .unwrap();
    settle(&mut app, workspace);
    assert_eq!(
        read_document(app.world(), document)
            .unwrap()
            .range_text(0..13),
        "other branch\n"
    );
    assert!(!inspect_document(app.world(), document.id()).unwrap().dirty);
    assert_eq!(
        app.world()
            .get::<GitSession>(workspace)
            .unwrap()
            .view
            .as_ref()
            .unwrap()
            .status
            .branch,
        "other"
    );
    assert!(
        app.world()
            .get::<GitSession>(workspace)
            .unwrap()
            .error
            .is_none()
    );
}

#[test]
fn conflict_resolution_requires_marker_free_text_before_staging() {
    let repo = Repo::new();
    repo.initial();
    repo.git(&["switch", "-c", "other"]);
    repo.write("a.txt", "other\n");
    repo.git(&["commit", "-am", "Other"]);
    repo.git(&["switch", "main"]);
    repo.write("a.txt", "main\n");
    repo.git(&["commit", "-am", "Main"]);
    let output = Command::new("git")
        .arg("-C")
        .arg(&repo.0)
        .args(["merge", "other"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let (mut app, workspace, _) = host(&repo.0);
    action(
        &mut app,
        workspace,
        Operation::Stage {
            paths: vec!["a.txt".into()],
        },
    )
    .unwrap();
    settle(&mut app, workspace);
    assert!(
        app.world()
            .get::<GitSession>(workspace)
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .contains("conflict markers")
    );
    action(
        &mut app,
        workspace,
        Operation::Resolve {
            path: "a.txt".into(),
            choice: bedtime_git::ConflictChoice::Incoming,
        },
    )
    .unwrap();
    settle(&mut app, workspace);
    assert_eq!(fs::read_to_string(repo.0.join("a.txt")).unwrap(), "other\n");
    action(
        &mut app,
        workspace,
        Operation::Stage {
            paths: vec!["a.txt".into()],
        },
    )
    .unwrap();
    settle(&mut app, workspace);
    assert!(
        !app.world()
            .get::<GitSession>(workspace)
            .unwrap()
            .view
            .as_ref()
            .unwrap()
            .status
            .entries
            .iter()
            .any(|e| e.conflicted)
    );
}

#[cfg(unix)]
#[test]
fn closing_workspace_cancels_and_drains_a_hook_before_retiring() {
    let repo = Repo::new();
    repo.initial();
    repo.hook();
    repo.write("a.txt", "changed\n");
    repo.git(&["add", "a.txt"]);
    let (mut app, workspace, _) = host(&repo.0);
    action(
        &mut app,
        workspace,
        Operation::Commit {
            mode: Default::default(),
            message: "Cancel on close".into(),
        },
    )
    .unwrap();
    wait(&mut app, |_| repo.0.join(".git/hook-started").exists());
    let id = Workbench::for_entity(app.world_mut(), workspace)
        .unwrap()
        .workspace_id();
    Workspaces::new(app.world_mut()).close(id, None).unwrap();
    wait(&mut app, |w| !w.entities().contains(workspace));
    assert!(!repo.0.join(".git/index.lock").exists());
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]).trim(), "Initial");
}

#[test]
fn a_restored_draft_is_shared_when_the_first_git_panel_attaches() {
    let repo = Repo::new();
    repo.initial();
    let mut app = App::new();
    app.add_plugins((WorkbenchCorePlugin, GitPlugin));
    let workspace = Workspaces::new(app.world_mut()).open("Restored").entity();
    app.world_mut()
        .entity_mut(workspace)
        .insert(crate::files::WorkspaceRoot(repo.0.clone()));
    restore_draft(
        app.world_mut(),
        workspace,
        "Retain this commit message".into(),
    );
    assert_eq!(
        commit_draft(app.world(), workspace),
        "Retain this commit message"
    );
    assert!(app.world().get::<GitSession>(workspace).is_none());
    let layout = active_layout(app.world(), workspace).unwrap();
    let area = Workbench::for_entity(app.world_mut(), layout)
        .unwrap()
        .destination_area(layout)
        .unwrap();
    app.world_mut().spawn((GitPanel::default(), InArea(area)));
    settle(&mut app, workspace);
    assert_eq!(
        commit_draft(app.world(), workspace),
        "Retain this commit message"
    );
    assert!(app.world().get::<RestoredDraft>(workspace).is_none());
}

#[test]
fn a_refresh_during_an_inflight_scan_rejects_the_old_observation() {
    let repo = Repo::new();
    repo.initial();
    let (mut app, workspace, _) = host(&repo.0);
    let captured = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let ready = captured.clone();
    let gate = release.clone();
    let root = repo.0.clone();
    let task = Workbench::for_entity(app.world_mut(), workspace)
        .unwrap()
        .work()
        .spawn_task(None, "Held status observation", move |cancel| {
            let Output::Status(status) =
                git_job(root, Operation::Status, &cancel, &AtomicBool::new(false))?
            else {
                unreachable!()
            };
            ready.store(true, Ordering::Release);
            while !gate.load(Ordering::Acquire) {
                if cancel.wait(Duration::from_millis(2)) {
                    return Err("Cancelled".into());
                }
            }
            Ok(RepositoryView::new(status))
        })
        .unwrap();
    let mut session = app.world_mut().get_mut::<GitSession>(workspace).unwrap();
    session.scan = Some((session.epoch, task));
    wait(&mut app, |_| captured.load(Ordering::Acquire));
    repo.write("a.txt", "external changed after read\n");
    app.world_mut()
        .get_mut::<GitSession>(workspace)
        .unwrap()
        .invalidate();
    app.update();
    assert!(!app.world().get::<GitSession>(workspace).unwrap().writable());
    release.store(true, Ordering::Release);
    settle(&mut app, workspace);
    let session = app.world().get::<GitSession>(workspace).unwrap();
    assert!(session.writable());
    assert_eq!(
        session.view.as_ref().unwrap().status.entries[0].worktree_status,
        'M'
    );
}

#[test]
fn changing_presentation_reuses_status_and_accepts_only_the_latest_projection() {
    let repo = Repo::new();
    repo.initial();
    fs::create_dir(repo.0.join("src")).unwrap();
    repo.write("src/new.txt", "new\n");
    let (mut app, workspace, _) = host(&repo.0);
    let source = app
        .world()
        .get::<GitSession>(workspace)
        .unwrap()
        .view
        .as_ref()
        .unwrap()
        .status
        .clone();
    let scans = app.world().get::<GitSession>(workspace).unwrap().scans;
    for options in [
        GitPanelOptions {
            view: ViewMode::Tree,
            group_by: GroupBy::StagedUnstaged,
        },
        GitPanelOptions {
            view: ViewMode::List,
            group_by: GroupBy::None,
        },
        GitPanelOptions {
            view: ViewMode::Tree,
            group_by: GroupBy::None,
        },
    ] {
        let mut session = app
            .world_mut()
            .entity_mut(workspace)
            .take::<GitSession>()
            .unwrap();
        session.last_scan = Instant::now();
        session.options = options;
        reproject(app.world_mut(), workspace, &mut session).unwrap();
        app.world_mut().entity_mut(workspace).insert(session);
    }
    wait(&mut app, |world| {
        world
            .get::<GitSession>(workspace)
            .unwrap()
            .projection
            .is_none()
    });
    let session = app.world().get::<GitSession>(workspace).unwrap();
    assert_eq!(session.scans, scans);
    assert!(Arc::ptr_eq(&source, &session.view.as_ref().unwrap().status));
    assert_eq!(
        session.view.as_ref().unwrap().options,
        GitPanelOptions {
            view: ViewMode::Tree,
            group_by: GroupBy::None
        }
    );
    assert!(
        session
            .view
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .any(|row| matches!(row, Row::Directory { .. }))
    );
    assert!(
        !session
            .view
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .any(|row| matches!(row, Row::Header(..)))
    );
}
