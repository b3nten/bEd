//! Regression coverage for file-list clipboard policy, operation acknowledgements,
//! and native drops routed by panel geometry rather than keyboard focus.
use super::*;
use crate::test_support::TempDir;
use bed_document_session::editor_session::ByteEdit;
use bed_workbench_api::{HostContext, HostRequest, ModulePanel, ModuleServices};
use std::{any::Any, cell::RefCell, rc::Rc, thread};

#[derive(Default)]
struct ClipboardState {
    paths: Vec<PathBuf>,
    reads: usize,
    writes: usize,
}
struct Clipboard(Rc<RefCell<ClipboardState>>);
impl FileClipboardService for Clipboard {
    fn read_files(&mut self) -> io::Result<Vec<PathBuf>> {
        let mut state = self.0.borrow_mut();
        state.reads += 1;
        Ok(state.paths.clone())
    }
    fn write_files(&mut self, paths: &[PathBuf]) -> io::Result<()> {
        let mut state = self.0.borrow_mut();
        state.writes += 1;
        state.paths = paths.to_vec();
        Ok(())
    }
}
fn workspace(dir: &TempDir) -> Workbench {
    let mut settings = Settings::with_paths(
        dir.path("config"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
    )
    .unwrap();
    settings.settings["terminal_visible"] = json!(false);
    settings.settings["treesitter"] = json!(false);
    settings.settings["git_changed_lines"] = json!(false);
    settings.terminal_visible = false;
    Workbench::with_settings(settings, crate::builtins::modules)
}
fn finish(workbench: &mut Workbench) -> OperationSummary {
    let deadline = Instant::now() + Duration::from_secs(10);
    while workbench.file_operations.active.is_some() {
        workbench.poll_file_operations().unwrap();
        assert!(
            Instant::now() < deadline,
            "file operation must finish without a UI frame"
        );
        thread::sleep(Duration::from_millis(2));
    }
    let summary = workbench
        .file_operations
        .summary
        .clone()
        .expect("operation result");
    assert!(summary.errors.is_empty(), "{:?}", summary.errors);
    assert!(!summary.canceled);
    summary
}
fn path(path: PathBuf) -> String {
    path.to_string_lossy().into_owned()
}

#[test]
fn system_file_clipboard_imports_copy_and_internal_cut_moves_with_dirty_views() {
    let dir = TempDir::new();
    let source = dir.write("project/source.txt", b"disk");
    let external = dir.write("external/import.txt", b"external");
    fs::create_dir_all(dir.path("project/destination")).unwrap();
    let mut workbench = workspace(&dir);
    workbench.set_project(&dir.path("project")).unwrap();
    let clipboard = Rc::new(RefCell::new(ClipboardState::default()));
    workbench.set_file_clipboard(Box::new(Clipboard(Rc::clone(&clipboard))));
    workbench
        .handle_tree_action(FileTreeAction::Copy(vec![path(source.clone())]))
        .unwrap();
    assert_eq!(
        clipboard.borrow().paths.as_slice(),
        std::slice::from_ref(&source)
    );
    assert_eq!(clipboard.borrow().writes, 1);
    clipboard.borrow_mut().paths = vec![external.clone()];
    let destination = path(dir.path("project/destination"));
    workbench
        .handle_tree_action(FileTreeAction::Paste(destination.clone()))
        .unwrap();
    finish(&mut workbench);
    assert_eq!(
        fs::read(dir.path("project/destination/import.txt")).unwrap(),
        b"external"
    );
    assert!(
        external.exists(),
        "OS clipboard imports preserve their source"
    );

    workbench
        .handle_tree_action(FileTreeAction::OpenMany(vec![path(source.clone())]))
        .unwrap();
    let document = workbench.session.document_for_path(&source).unwrap();
    let revision = workbench.session.document_revision(document).unwrap();
    workbench
        .session
        .apply_edits(
            document,
            revision,
            &[ByteEdit {
                range: 0..0,
                bytes: b"dirty ".to_vec(),
            }],
        )
        .unwrap();
    let view = workbench.active.unwrap();
    let clipboard_reads = clipboard.borrow().reads;
    workbench
        .handle_tree_action(FileTreeAction::Cut(vec![path(source.clone())]))
        .unwrap();
    workbench
        .handle_tree_action(FileTreeAction::Paste(destination))
        .unwrap();
    finish(&mut workbench);
    let target = dir.path("project/destination/source.txt");
    assert!(!source.exists());
    assert_eq!(
        fs::read(&target).unwrap(),
        b"disk",
        "move does not implicitly save edited buffers"
    );
    assert_eq!(workbench.session.document_for_path(&target), Some(document));
    assert_eq!(workbench.session.document_for_view(view), Some(document));
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        b"dirty disk"
    );
    assert!(workbench.session.snapshot(document).unwrap().dirty);
    assert_eq!(
        clipboard.borrow().reads,
        clipboard_reads,
        "internal cut bypasses the OS clipboard"
    );
    workbench
        .session
        .with_commands(view, |commands| commands.undo())
        .unwrap();
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        b"disk",
        "text undo history survives relocation"
    );
    workbench.cleanup().unwrap();
}

#[test]
fn duplicate_keeps_each_selected_items_original_parent_and_deduplicates_descendants() {
    let dir = TempDir::new();
    let a = dir.write("project/first/a.txt", b"a");
    let b = dir.write("project/second/b.txt", b"b");
    dir.write("project/folder/child.txt", b"child");
    let mut workbench = workspace(&dir);
    workbench.set_project(&dir.path("project")).unwrap();
    workbench
        .handle_tree_action(FileTreeAction::Duplicate(vec![
            path(a),
            path(b),
            path(dir.path("project/folder")),
            path(dir.path("project/folder/child.txt")),
        ]))
        .unwrap();
    let summary = finish(&mut workbench);
    assert_eq!(
        fs::read(dir.path("project/first/a copy.txt")).unwrap(),
        b"a"
    );
    assert_eq!(
        fs::read(dir.path("project/second/b copy.txt")).unwrap(),
        b"b"
    );
    assert_eq!(
        fs::read(dir.path("project/folder copy/child.txt")).unwrap(),
        b"child"
    );
    assert_eq!(summary.destinations.len(), 3);
    assert!(!dir.path("project/folder/child copy.txt").exists());
    workbench.cleanup().unwrap();
}

#[test]
fn clipboard_conflict_keep_both_preserves_source_destination_and_previous_clipboard() {
    let dir = TempDir::new();
    let existing = dir.write("project/data.txt", b"old");
    let imported = dir.write("external/data.txt", b"new");
    let mut workbench = workspace(&dir);
    workbench.set_project(&dir.path("project")).unwrap();
    let clipboard = Rc::new(RefCell::new(ClipboardState {
        paths: vec![imported.clone()],
        ..Default::default()
    }));
    workbench.set_file_clipboard(Box::new(Clipboard(Rc::clone(&clipboard))));
    workbench
        .handle_tree_action(FileTreeAction::Paste(path(dir.path("project"))))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while workbench.file_operations.conflict.is_none() {
        workbench.poll_file_operations().unwrap();
        assert!(Instant::now() < deadline, "conflict should reach the host");
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        clipboard.borrow().paths.as_slice(),
        std::slice::from_ref(&imported)
    );
    let conflict = workbench.file_operations.conflict.take().unwrap();
    conflict
        .reply
        .send(ConflictDecision {
            choice: ConflictChoice::KeepBoth,
            apply_to_all: false,
        })
        .unwrap();
    let summary = finish(&mut workbench);
    assert_eq!(fs::read(existing).unwrap(), b"old");
    assert_eq!(fs::read(&imported).unwrap(), b"new");
    assert_eq!(fs::read(dir.path("project/data copy.txt")).unwrap(), b"new");
    assert_eq!(
        summary.destinations,
        [path(
            dir.path("project/data copy.txt").canonicalize().unwrap()
        )]
    );
    assert_eq!(clipboard.borrow().writes, 0);
    workbench.cleanup().unwrap();
}

fn resolve_conflict(workbench: &mut Workbench, choice: ConflictChoice) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while workbench.file_operations.conflict.is_none() {
        workbench.poll_file_operations().unwrap();
        assert!(Instant::now() < deadline, "conflict should reach the host");
        thread::sleep(Duration::from_millis(2));
    }
    workbench
        .file_operations
        .conflict
        .take()
        .unwrap()
        .reply
        .send(ConflictDecision {
            choice,
            apply_to_all: false,
        })
        .unwrap();
}

#[test]
fn copy_replacement_keeps_clean_destination_document_and_panels_then_reloads_committed_bytes() {
    let dir = TempDir::new();
    let destination = dir.write("project/data.txt", b"old contents");
    let source = dir.write("external/data.txt", b"new replacement payload");
    let mut workbench = workspace(&dir);
    workbench.set_project(&dir.path("project")).unwrap();
    workbench.open_or_focus(&destination).unwrap();
    let document = workbench.session.document_for_path(&destination).unwrap();
    workbench.dispatch(WindowCommand::DuplicateView).unwrap();
    let panels: Vec<_> = workbench
        .tabs
        .iter()
        .filter(|tab| tab.panel.document() == Some(document))
        .map(|tab| tab.id)
        .collect();
    assert_eq!(panels.len(), 2);
    let clipboard = Rc::new(RefCell::new(ClipboardState {
        paths: vec![source.clone()],
        ..Default::default()
    }));
    workbench.set_file_clipboard(Box::new(Clipboard(clipboard)));
    workbench
        .handle_tree_action(FileTreeAction::Paste(path(dir.path("project"))))
        .unwrap();
    resolve_conflict(&mut workbench, ConflictChoice::Replace);
    finish(&mut workbench);
    assert_eq!(fs::read(&destination).unwrap(), b"new replacement payload");
    let deadline = Instant::now() + Duration::from_secs(10);
    while workbench.session.snapshot(document).unwrap().bytes != b"new replacement payload" {
        workbench.tick().unwrap();
        assert!(
            Instant::now() < deadline,
            "a clean replacement should reload after the disk commit"
        );
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        workbench.session.document_for_path(&destination),
        Some(document)
    );
    assert_eq!(
        workbench
            .tabs
            .iter()
            .filter(|tab| tab.panel.document() == Some(document))
            .map(|tab| tab.id)
            .collect::<Vec<_>>(),
        panels
    );
    assert!(!workbench.session.snapshot(document).unwrap().dirty);
    assert!(source.exists());
    workbench.cleanup().unwrap();
}

#[test]
fn copy_replacement_detaches_dirty_destination_preserving_panels_contents_and_undo() {
    let dir = TempDir::new();
    let destination = dir.write("project/data.txt", b"original contents");
    let source = dir.write("external/data.txt", b"replacement on disk");
    let mut workbench = workspace(&dir);
    workbench.set_project(&dir.path("project")).unwrap();
    workbench.open_or_focus(&destination).unwrap();
    let document = workbench.session.document_for_path(&destination).unwrap();
    let revision = workbench.session.document_revision(document).unwrap();
    workbench
        .session
        .apply_edits(
            document,
            revision,
            &[ByteEdit {
                range: 0..0,
                bytes: b"unsaved ".to_vec(),
            }],
        )
        .unwrap();
    let view = workbench.active.unwrap();
    workbench.dispatch(WindowCommand::DuplicateView).unwrap();
    let panels: Vec<_> = workbench
        .tabs
        .iter()
        .filter(|tab| tab.panel.document() == Some(document))
        .map(|tab| tab.id)
        .collect();
    let clipboard = Rc::new(RefCell::new(ClipboardState {
        paths: vec![source],
        ..Default::default()
    }));
    workbench.set_file_clipboard(Box::new(Clipboard(clipboard)));
    workbench
        .handle_tree_action(FileTreeAction::Paste(path(dir.path("project"))))
        .unwrap();
    resolve_conflict(&mut workbench, ConflictChoice::Replace);
    finish(&mut workbench);
    let snapshot = workbench.session.snapshot(document).unwrap();
    assert!(
        snapshot.path.is_empty(),
        "dirty destination becomes an unbound buffer"
    );
    assert_eq!(
        snapshot.original_path,
        Some(path(destination.canonicalize().unwrap()))
    );
    assert!(snapshot.dirty);
    assert_eq!(snapshot.bytes, b"unsaved original contents");
    assert_eq!(workbench.session.document_for_path(&destination), None);
    assert_eq!(
        workbench
            .tabs
            .iter()
            .filter(|tab| tab.panel.document() == Some(document))
            .map(|tab| tab.id)
            .collect::<Vec<_>>(),
        panels
    );
    assert!(
        !workbench.session.save(document).unwrap(),
        "detached documents require Save As"
    );
    workbench
        .session
        .with_commands(view, |commands| commands.undo())
        .unwrap();
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        b"original contents"
    );
    assert_eq!(fs::read(&destination).unwrap(), b"replacement on disk");
    let recovered = dir.path("project/recovered.txt");
    workbench.session.save_as(document, &recovered).unwrap();
    assert_eq!(fs::read(recovered).unwrap(), b"original contents");
    assert_eq!(fs::read(destination).unwrap(), b"replacement on disk");
    workbench.cleanup().unwrap();
}

struct DropPanel {
    events: Rc<RefCell<Vec<ExternalFileDrag>>>,
    response: ExternalFileDropResponse,
}
impl ModulePanel for DropPanel {
    fn title(&self, _: &HostContext<'_>) -> String {
        "Drop probe".into()
    }
    fn draw(&mut self, _: &Ui, _: &HostContext<'_>, _: &mut Vec<HostRequest>) {}
    fn external_files_with_services(
        &mut self,
        event: &ExternalFileDrag,
        _: &HostContext<'_>,
        _: &mut ModuleServices<'_>,
        _: &mut Vec<HostRequest>,
    ) -> io::Result<ExternalFileDropResponse> {
        self.events.borrow_mut().push(event.clone());
        Ok(self.response)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
fn drop_panel(
    workbench: &mut Workbench,
    response: ExternalFileDropResponse,
) -> (u64, Rc<RefCell<Vec<ExternalFileDrag>>>) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let id = workbench.push_panel(Panel {
        kind: "test.drop".into(),
        viewer: None,
        instance: Box::new(DropPanel {
            events: Rc::clone(&events),
            response,
        }),
    });
    (id, events)
}
fn composition(id: u64, viewport: u32, origin: [f32; 2]) -> PanelComposition {
    (
        id,
        viewport,
        0,
        true,
        [
            origin[0].to_bits(),
            origin[1].to_bits(),
            100.0_f32.to_bits(),
            100.0_f32.to_bits(),
        ],
    )
}
#[test]
fn native_drops_use_hovered_panel_and_viewport_and_ignored_drops_keep_opening_fallback() {
    let dir = TempDir::new();
    dir.write("project/keep.txt", b"keep");
    let external = dir.write("external/open.txt", b"open");
    let another_project = dir.path("another");
    fs::create_dir(&another_project).unwrap();
    let mut workbench = workspace(&dir);
    workbench.set_project(&dir.path("project")).unwrap();
    let (ignored, ignored_events) = drop_panel(&mut workbench, ExternalFileDropResponse::Ignored);
    let (accepted, accepted_events) =
        drop_panel(&mut workbench, ExternalFileDropResponse::Accepted);
    workbench.focused = Some(ignored);
    workbench.composition = vec![
        composition(ignored, 1, [0.0, 0.0]),
        composition(accepted, 7, [200.0, 0.0]),
    ];
    let event = ExternalFileDrag {
        paths: vec![external.clone()],
        position: [250.0, 50.0],
        viewport: 7,
        phase: ExternalFileDragPhase::Hover,
        modifiers: Default::default(),
    };
    assert!(workbench.external_file_drag(event.clone()).unwrap());
    assert!(ignored_events.borrow().is_empty());
    assert_eq!(
        accepted_events.borrow().last().unwrap().phase,
        ExternalFileDragPhase::Hover
    );
    let mut drop = event.clone();
    drop.phase = ExternalFileDragPhase::Drop;
    assert!(workbench.external_file_drag(drop).unwrap());
    assert!(
        workbench.session.document_for_path(&external).is_none(),
        "accepted drop suppresses default opening"
    );
    assert_eq!(
        workbench.focused,
        Some(ignored),
        "drag handling does not steal keyboard focus"
    );

    assert!(workbench.external_file_drag(event.clone()).unwrap());
    let mut elsewhere = event.clone();
    elsewhere.position = [50.0, 50.0];
    elsewhere.viewport = 1;
    assert!(!workbench.external_file_drag(elsewhere.clone()).unwrap());
    assert_eq!(
        accepted_events.borrow().last().unwrap().phase,
        ExternalFileDragPhase::Cancel
    );
    assert_eq!(
        ignored_events.borrow().last().unwrap().phase,
        ExternalFileDragPhase::Hover
    );
    let mut wrong_viewport = event.clone();
    wrong_viewport.viewport = 999;
    assert!(!workbench.external_file_drag(wrong_viewport).unwrap());
    assert_eq!(
        ignored_events.borrow().last().unwrap().phase,
        ExternalFileDragPhase::Cancel
    );
    elsewhere.phase = ExternalFileDragPhase::Drop;
    assert!(!workbench.external_file_drag(elsewhere).unwrap());
    assert!(workbench.session.document_for_path(&external).is_some());

    let project_drop = ExternalFileDrag {
        paths: vec![another_project.clone()],
        position: [500.0, 500.0],
        viewport: 1,
        phase: ExternalFileDragPhase::Drop,
        modifiers: Default::default(),
    };
    assert!(!workbench.external_file_drag(project_drop).unwrap());
    assert_eq!(
        Path::new(&workbench.project_root),
        another_project.canonicalize().unwrap()
    );
    workbench.cleanup().unwrap();
}

#[cfg(unix)]
fn remote_clipboard_workspace(dir: &TempDir, root: &Path) -> Workbench {
    let mut workbench = workspace(dir);
    workbench.settings.settings["autosave"] = json!(false);
    let executable = std::env::current_exe().unwrap();
    let helper = executable
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("bed-headless");
    let client = if helper.is_file() {
        bed_remote::RemoteClient::launch_local(helper).unwrap()
    } else {
        let mut command = std::process::Command::new(env!("CARGO"));
        command
            .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .args([
                "run",
                "--quiet",
                "--offline",
                "-p",
                "bed-headless",
                "--",
                "--stdio",
            ]);
        bed_remote::RemoteClient::launch_command(command).unwrap()
    };
    workbench
        .activate_remote_workspace(
            WorkspaceSpec {
                name: "Remote clipboard fixture".into(),
                root: root.to_str().unwrap().into(),
                target: WorkspaceTarget::Ssh {
                    host: "clipboard-fixture".into(),
                },
            },
            bed_remote::SshTarget::new("clipboard-fixture"),
            client,
        )
        .unwrap();
    let mut options = workbench.session.options().clone();
    options.autosave = None;
    options.lsp_config = None;
    workbench.session.configure(options).unwrap();
    workbench
}

#[cfg(unix)]
#[test]
fn remote_copy_publishes_complete_binary_staging_only_after_completion() {
    let dir = TempDir::new();
    let bytes = [0, 0xff, 0xef, 0xbb, 0xbf, b'\r', b'\n'].repeat(170_000);
    let source = dir
        .write("remote/source.bin", &bytes)
        .canonicalize()
        .unwrap();
    let remote_root = dir.path("remote").canonicalize().unwrap();
    let previous = dir.write("local/previous.txt", b"previous clipboard");
    let mut workbench = remote_clipboard_workspace(&dir, &remote_root);
    let clipboard = Rc::new(RefCell::new(ClipboardState {
        paths: vec![previous.clone()],
        ..Default::default()
    }));
    workbench.set_file_clipboard(Box::new(Clipboard(Rc::clone(&clipboard))));
    workbench
        .handle_tree_action(FileTreeAction::Copy(vec![path(source.clone())]))
        .unwrap();
    assert!(workbench.file_operations.active.is_some());
    assert_eq!(
        clipboard.borrow().paths.as_slice(),
        std::slice::from_ref(&previous)
    );
    assert_eq!(clipboard.borrow().writes, 0);
    let deadline = Instant::now() + Duration::from_secs(10);
    while workbench.file_operations.active.is_some() {
        assert_eq!(
            clipboard.borrow().paths.as_slice(),
            std::slice::from_ref(&previous)
        );
        assert_eq!(clipboard.borrow().writes, 0);
        workbench.poll_file_operations().unwrap();
        if workbench.file_operations.active.is_some() {
            assert_eq!(
                clipboard.borrow().paths.as_slice(),
                std::slice::from_ref(&previous)
            );
        }
        assert!(Instant::now() < deadline, "remote file export stalled");
        thread::sleep(Duration::from_millis(2));
    }
    let summary = workbench.file_operations.summary.as_ref().unwrap();
    assert!(summary.errors.is_empty(), "{:?}", summary.errors);
    assert!(!summary.canceled);
    assert_eq!(summary.bytes, bytes.len() as u64);
    assert_eq!(clipboard.borrow().writes, 1);
    let staged = clipboard.borrow().paths[0].clone();
    assert_eq!(staged.file_name(), source.file_name());
    assert_ne!(staged, source);
    assert!(
        staged
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .ends_with("bed-file-exports")
    );
    assert_eq!(fs::read(&staged).unwrap(), bytes);
    assert!(source.exists(), "Copy retains the remote source");
    workbench.cleanup().unwrap();
    assert_eq!(
        fs::read(&staged).unwrap(),
        bytes,
        "staged OS clipboard exports survive workbench exit"
    );
    fs::remove_dir_all(staged.parent().unwrap()).unwrap();
}

#[cfg(unix)]
#[test]
fn remote_os_paste_preserves_binary_bytes_and_internal_cut_retains_dirty_identity() {
    let dir = TempDir::new();
    let source = dir
        .write("remote/source.txt", b"disk")
        .canonicalize()
        .unwrap();
    let bytes = [0, 0xff, 0xef, 0xbb, 0xbf, b'\r', b'\n'].repeat(170_000);
    let upload = dir
        .write("local/upload.bin", &bytes)
        .canonicalize()
        .unwrap();
    fs::create_dir(dir.path("remote/destination")).unwrap();
    let remote_root = dir.path("remote").canonicalize().unwrap();
    let destination = remote_root.join("destination");
    let mut workbench = remote_clipboard_workspace(&dir, &remote_root);
    let clipboard = Rc::new(RefCell::new(ClipboardState {
        paths: vec![upload.clone()],
        ..Default::default()
    }));
    workbench.set_file_clipboard(Box::new(Clipboard(Rc::clone(&clipboard))));
    workbench
        .handle_tree_action(FileTreeAction::Paste(path(destination.clone())))
        .unwrap();
    finish(&mut workbench);
    assert_eq!(fs::read(destination.join("upload.bin")).unwrap(), bytes);
    assert_eq!(
        fs::read(&upload).unwrap(),
        bytes,
        "OS Paste copies its local source"
    );
    assert_eq!(clipboard.borrow().writes, 0);

    workbench.open_or_focus(&source).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while workbench.session.document_for_path(&source).is_none() {
        workbench.tick().unwrap();
        assert!(Instant::now() < deadline, "remote document open stalled");
        thread::sleep(Duration::from_millis(2));
    }
    let document = workbench.session.document_for_path(&source).unwrap();
    let view = workbench.active_view().unwrap();
    workbench
        .session
        .with_commands(view, |commands| commands.type_text(b"dirty "))
        .unwrap();
    let reads = clipboard.borrow().reads;
    workbench
        .handle_tree_action(FileTreeAction::Cut(vec![path(source.clone())]))
        .unwrap();
    workbench
        .handle_tree_action(FileTreeAction::Paste(path(destination.clone())))
        .unwrap();
    finish(&mut workbench);
    let moved = destination.join("source.txt");
    assert!(!source.exists());
    assert_eq!(
        fs::read(&moved).unwrap(),
        b"disk",
        "relocation does not implicitly save dirty buffers"
    );
    assert_eq!(workbench.session.document_for_path(&moved), Some(document));
    assert_eq!(workbench.session.document_for_view(view), Some(document));
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        b"dirty disk"
    );
    assert!(workbench.session.snapshot(document).unwrap().dirty);
    assert_eq!(
        clipboard.borrow().reads,
        reads,
        "internal Cut bypasses the native clipboard"
    );
    assert_eq!(clipboard.borrow().writes, 0);
    workbench
        .session
        .with_commands(view, |commands| commands.undo())
        .unwrap();
    assert_eq!(workbench.session.snapshot(document).unwrap().bytes, b"disk");
    workbench.session.save(document).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while workbench.session.save_pending(document) {
        workbench.tick().unwrap();
        assert!(Instant::now() < deadline, "relocated document save stalled");
        thread::sleep(Duration::from_millis(2));
    }
    workbench.cleanup().unwrap();
}

#[test]
fn canceling_a_queued_replacement_retains_clean_destination_document_and_panels() {
    let dir = TempDir::new();
    let source = dir.write("project/source/data.txt", b"new contents");
    let destination = dir.write("project/destination/data.txt", b"old contents");
    let mut workbench = workspace(&dir);
    workbench.set_project(&dir.path("project")).unwrap();
    workbench.open_or_focus(&destination).unwrap();
    let document = workbench.active_document().unwrap();
    workbench.dispatch(WindowCommand::DuplicateView).unwrap();
    let panels = workbench
        .tabs
        .iter()
        .filter(|tab| tab.panel.document() == Some(document))
        .map(|tab| tab.id)
        .collect::<Vec<_>>();
    assert_eq!(panels.len(), 2);
    workbench
        .handle_tree_action(FileTreeAction::Move {
            paths: vec![path(source.clone())],
            destination: path(dir.path("project/destination")),
        })
        .unwrap();
    resolve_conflict(&mut workbench, ConflictChoice::Replace);

    // Hold the worker exactly at its lifecycle handshake, then put the event
    // back in the host queue before cancellation. This avoids a timing race.
    let active = workbench.file_operations.active.as_mut().unwrap();
    let pending = loop {
        let event = active
            .job
            .events
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        if matches!(event, OperationEvent::BeforeReplace { .. }) {
            break event;
        }
        assert!(matches!(event, OperationEvent::Progress { .. }));
    };
    let (queued, receiver) = std::sync::mpsc::channel();
    let original = std::mem::replace(&mut active.job.events, receiver);
    queued.send(pending).unwrap();
    let relay = thread::spawn(move || {
        for event in original {
            if queued.send(event).is_err() {
                break;
            }
        }
    });
    active.job.cancel();
    let deadline = Instant::now() + Duration::from_secs(10);
    while workbench.file_operations.active.is_some() {
        workbench.poll_file_operations().unwrap();
        assert!(
            Instant::now() < deadline,
            "canceled replacement did not finish"
        );
        thread::sleep(Duration::from_millis(2));
    }
    relay.join().unwrap();
    assert!(workbench.file_operations.summary.as_ref().unwrap().canceled);
    assert_eq!(
        workbench.session.document_for_path(&destination),
        Some(document)
    );
    assert_eq!(
        workbench
            .tabs
            .iter()
            .filter(|tab| tab.panel.document() == Some(document))
            .map(|tab| tab.id)
            .collect::<Vec<_>>(),
        panels
    );
    let snapshot = workbench.session.snapshot(document).unwrap();
    assert_eq!(snapshot.bytes, b"old contents");
    assert!(snapshot.original_path.is_none());
    assert!(!snapshot.dirty);
    assert_eq!(fs::read(source).unwrap(), b"new contents");
    assert_eq!(fs::read(destination).unwrap(), b"old contents");
    assert!(
        fs::read_dir(dir.path("project/destination"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".bed-transfer-"))
    );
    workbench.cleanup().unwrap();
}
