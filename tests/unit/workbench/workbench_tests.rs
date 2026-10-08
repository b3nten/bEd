use super::*;
use crate::test_support::TempDir;
use dear_imgui_rs::FramePrepareOptions;
pub(super) fn workspace(dir: &TempDir) -> Workbench {
    let mut settings = Settings::with_paths(
        dir.path("config"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
    )
    .unwrap();
    settings.settings["terminal_visible"] = json!(false);
    settings.terminal_visible = false;
    settings.settings["treesitter"] = json!(false);
    settings.settings["git_changed_lines"] = json!(false);
    Workbench::with_settings(settings, crate::builtins::modules)
}
#[test]
fn tree_actions_persist_without_a_frame_and_project_switch_resets_reveal() {
    let dir = TempDir::new();
    dir.write("first/.secret", b"keep");
    dir.write("first/visible", b"keep");
    dir.write("second/visible", b"keep");
    let mut workbench = workspace(&dir);
    workbench.set_project(&dir.path("first")).unwrap();
    let hidden = dir
        .path("first/.secret")
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    workbench.open_or_focus(Path::new(&hidden)).unwrap();
    workbench
        .handle_tree_action(FileTreeAction::SetPathHidden {
            path: hidden.clone(),
            hidden: true,
        })
        .unwrap();
    workbench
        .handle_tree_action(FileTreeAction::SetHideHidden(true))
        .unwrap();
    workbench
        .handle_tree_action(FileTreeAction::SetHideGitignored(true))
        .unwrap();
    workbench
        .handle_tree_action(FileTreeAction::SetShowHidden(true))
        .unwrap();
    assert!(workbench.file_dialog.is_none());
    assert_eq!(workbench.active_snapshot().unwrap().bytes, b"keep");
    let spec = workbench.workspace_spec.clone().unwrap();
    let reloaded = WorkspaceStore::load(&dir.path("config")).unwrap();
    let expected = workbench.file_explorer().file_tree.preferences.clone();
    assert_eq!(
        reloaded.module_settings(&spec, bed_module_explorer::MODULE_ID),
        Some(&expected.to_value())
    );
    workbench.set_project(&dir.path("second")).unwrap();
    assert_eq!(
        workbench.file_explorer().file_tree.preferences,
        Default::default()
    );
    assert!(!workbench.file_explorer().file_tree.show_hidden);
    workbench.set_project(&dir.path("first")).unwrap();
    assert_eq!(workbench.file_explorer().file_tree.preferences, expected);
    assert!(!workbench.file_explorer().file_tree.show_hidden);
    workbench
        .handle_tree_action(FileTreeAction::SetPathHidden {
            path: hidden,
            hidden: false,
        })
        .unwrap();
    assert!(
        workbench
            .file_explorer()
            .file_tree
            .preferences
            .hidden_paths
            .is_empty()
    );
    assert!(workbench.file_explorer().file_tree.preferences.hide_hidden);
    let source = dir.path("first/visible").canonicalize().unwrap();
    workbench
        .handle_tree_action(FileTreeAction::SetPathHidden {
            path: source.to_str().unwrap().into(),
            hidden: true,
        })
        .unwrap();
    workbench.rename_path(&source, "renamed").unwrap();
    assert!(
        workbench
            .file_explorer()
            .file_tree
            .preferences
            .hidden_paths
            .contains("visible")
    );
    assert!(
        !workbench
            .file_explorer()
            .file_tree
            .preferences
            .hidden_paths
            .contains("renamed")
    );
    workbench.cleanup().unwrap();
}
pub(super) fn frame(context: &mut Context, workbench: &mut Workbench) {
    context.prepare_frame(FramePrepareOptions::new([1200.0, 800.0], 1.0 / 60.0));
    workbench.render(context.frame()).unwrap();
    drop(context.render_legacy());
}
#[test]
fn project_picker_has_no_documents_or_terminal_processes() {
    let dir = TempDir::new();
    let workspace = workspace(&dir);
    assert_eq!(workspace.session.document_ids().len(), 0);
    assert_eq!(workspace.terminal.session_count(), 0);
    assert!(workspace.panel_visible("projects"));
    assert!(!workspace.panel_visible("explorer"));
}
#[test]
fn new_document_from_picker_has_undo_without_a_project() {
    let dir = TempDir::new();
    let mut workspace = workspace(&dir);
    workspace.dispatch(WindowCommand::NewDocument).unwrap();
    let doc = workspace.active_document().unwrap();
    let view = workspace.active_view().unwrap();
    workspace
        .session
        .with_commands(view, |commands| commands.paste(b"hello"))
        .unwrap();
    workspace
        .session
        .with_commands(view, |commands| commands.undo())
        .unwrap();
    assert_eq!(workspace.session.snapshot(doc).unwrap().bytes, b"");
    assert!(workspace.session.lsp().is_none());
    assert!(!workspace.session.options().persistent_history);
    assert!(workspace.project_root.is_empty());
}
#[test]
fn spawned_search_panels_keep_queries_and_worker_results_independent() {
    let dir = TempDir::new();
    let first_file = dir.write("first.txt", "first_🙂_needle".as_bytes());
    let second_file = dir.write("second.txt", "second_é_needle".as_bytes());
    let mut workspace = workspace(&dir);
    workspace.set_project(dir.root()).unwrap();
    workspace.dispatch(WindowCommand::NewContentSearch).unwrap();
    let first = workspace.active_panel_id().unwrap();
    workspace
        .search_panel(first)
        .unwrap()
        .start(&workspace.project_root, "first_🙂_needle", false);
    workspace.dispatch(WindowCommand::NewContentSearch).unwrap();
    let second = workspace.active_panel_id().unwrap();
    workspace
        .search_panel(second)
        .unwrap()
        .start(&workspace.project_root, "second_é_needle", true);
    let deadline = Instant::now() + Duration::from_secs(5);
    while workspace.modules.search.is_searching() {
        workspace.tick().unwrap();
        assert!(
            Instant::now() < deadline,
            "independent searches must complete"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    for (id, query, path) in [
        (first, "first_🙂_needle", first_file),
        (second, "second_é_needle", second_file),
    ] {
        let search = workspace.search_panel(id).unwrap();
        assert_eq!(search.query, query);
        assert_eq!(search.results.len(), 1);
        assert_eq!(
            Path::new(&search.results[0].file.full_path),
            std::fs::canonicalize(path).unwrap()
        );
    }
    workspace.dispatch(WindowCommand::FindProject).unwrap();
    assert_eq!(workspace.panel_count("search"), 2);
    let index = workspace
        .tabs
        .iter()
        .position(|tab| tab.id == first)
        .unwrap();
    workspace.close_tab(index).unwrap();
    assert_eq!(workspace.panel_count("search"), 1);
    assert!(!workspace.search_panel(first).is_some());
    assert_eq!(
        workspace.search_panel(second).unwrap().query,
        "second_é_needle"
    );
    assert_eq!(workspace.search_panel(second).unwrap().results.len(), 1);
    workspace.cleanup().unwrap();
    assert!(workspace.modules.search.panel_count() == 0);
}
#[test]
fn navigation_within_shared_document_keeps_the_requesting_view() {
    let dir = TempDir::new();
    let path = dir.write("file.txt", "éxy".as_bytes());
    let mut workspace = workspace(&dir);
    workspace.open_or_focus(&path).unwrap();
    let first = workspace.active_view().unwrap();
    workspace.dispatch(WindowCommand::DuplicateView).unwrap();
    let second = workspace.active_view().unwrap();
    let old = workspace.session.view_snapshot(first).unwrap();
    let file = workspace.active_snapshot().unwrap().path;
    workspace
        .open_lsp_action(LspAction::OpenLocation(LspLocation {
            file,
            line: 0,
            character: 2,
        }))
        .unwrap();
    assert_eq!(workspace.active_view(), Some(second));
    assert_eq!(workspace.session.view_snapshot(first).unwrap(), old);
    assert_eq!(workspace.session.view_snapshot(second).unwrap().column, 3);
}
#[test]
fn duplicate_views_share_document_and_undo_but_not_selection() {
    let dir = TempDir::new();
    let path = dir.write("file", b"abc");
    let mut workspace = workspace(&dir);
    workspace.open_or_focus(&path).unwrap();
    let first = workspace.active_view().unwrap();
    let doc = workspace.active_document().unwrap();
    workspace.dispatch(WindowCommand::DuplicateView).unwrap();
    let second = workspace.active_view().unwrap();
    assert_ne!(first, second);
    assert_eq!(workspace.session.document_ids(), vec![doc]);
    workspace
        .session
        .with_commands(first, |commands| {
            commands.set_cursor(0, 1, false, CursorReveal::Ensure)
        })
        .unwrap();
    workspace
        .session
        .with_commands(second, |commands| {
            commands.set_cursor(0, 3, false, CursorReveal::Ensure);
            commands.paste("🙂".as_bytes());
        })
        .unwrap();
    assert_eq!(
        workspace.session.snapshot(doc).unwrap().bytes,
        "abc🙂".as_bytes()
    );
    assert_eq!(workspace.session.view_snapshot(first).unwrap().column, 1);
    workspace
        .session
        .with_commands(first, |commands| commands.undo())
        .unwrap();
    assert_eq!(workspace.session.snapshot(doc).unwrap().bytes, b"abc");
    workspace.close_tab(workspace.active_index()).unwrap();
    assert_eq!(workspace.session.view_count(doc), 1);
}
#[test]
fn folder_rename_rebinds_every_view_and_preserves_undo() {
    let dir = TempDir::new();
    std::fs::create_dir(dir.path("old")).unwrap();
    let path = dir.write("old/file.rs", b"abc");
    let mut workspace = workspace(&dir);
    workspace.set_project(dir.root()).unwrap();
    workspace.open_or_focus(&path).unwrap();
    workspace.dispatch(WindowCommand::DuplicateView).unwrap();
    let view = workspace.active_view().unwrap();
    let doc = workspace.active_document().unwrap();
    workspace
        .session
        .with_commands(view, |commands| commands.paste(b"x"))
        .unwrap();
    let target = workspace.rename_path(&dir.path("old"), "new").unwrap();
    assert_eq!(
        Path::new(&workspace.session.snapshot(doc).unwrap().path),
        target.join("file.rs")
    );
    assert_eq!(workspace.session.view_count(doc), 2);
    assert!(!dir.path("old").exists());
    workspace
        .session
        .with_commands(view, |commands| commands.undo())
        .unwrap();
    assert_eq!(workspace.session.snapshot(doc).unwrap().bytes, b"abc");
    workspace.session.save(doc).unwrap();
    assert_eq!(std::fs::read(target.join("file.rs")).unwrap(), b"abc");
}
#[test]
fn failed_rename_and_trash_preserve_identity_and_writes() {
    let dir = TempDir::new();
    let path = dir.write("source", b"abc");
    dir.write("target", b"keep");
    let mut workspace = workspace(&dir);
    workspace.open_or_focus(&path).unwrap();
    let doc = workspace.active_document().unwrap();
    let before = workspace.session.snapshot(doc).unwrap();
    assert!(workspace.rename_path(&path, "target").is_err());
    assert!(
        workspace
            .trash_path_with(&path, |_| Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "denied"
            )))
            .is_err()
    );
    let after = workspace.session.snapshot(doc).unwrap();
    assert_eq!(after.path, before.path);
    assert_eq!(after.bytes, before.bytes);
    assert!(after.disk_conflict.is_none());
    assert!(path.is_file());
    workspace
        .session
        .with_commands(workspace.active_view().unwrap(), |commands| {
            commands.paste(b"dirty")
        })
        .unwrap();
    workspace
        .trash_path_with(&path, |path| std::fs::remove_file(path))
        .unwrap();
    assert!(workspace.session.snapshot(doc).unwrap().path.is_empty());
    assert!(!workspace.session.save(doc).unwrap());
    assert!(!path.exists());
    let moved = dir.path("rescued");
    workspace.session.save_as(doc, &moved).unwrap();
    assert!(moved.is_file());
    assert!(!path.exists());
}
#[test]
fn deleting_a_folder_closes_all_clean_views_and_preserves_dirty_documents() {
    let dir = TempDir::new();
    let clean = dir.write("project/folder/clean.txt", b"clean");
    let dirty = dir
        .write("project/folder/dirty.txt", b"dirty")
        .canonicalize()
        .unwrap();
    let mut workbench = workspace(&dir);
    workbench.set_project(&dir.path("project")).unwrap();
    workbench.open_or_focus(&clean).unwrap();
    let clean_document = workbench.active_document().unwrap();
    workbench.dispatch(WindowCommand::DuplicateView).unwrap();
    assert_eq!(workbench.session.view_count(clean_document), 2);
    workbench.open_or_focus(&dirty).unwrap();
    let document = workbench.active_document().unwrap();
    let view = workbench.active_view().unwrap();
    workbench
        .session
        .with_commands(view, |commands| commands.type_text(b"unsaved "))
        .unwrap();
    workbench
        .trash_path_with(&dir.path("project/folder"), |path| {
            std::fs::remove_dir_all(path)
        })
        .unwrap();
    assert!(workbench.session.snapshot(clean_document).is_err());
    assert!(
        workbench
            .tabs
            .iter()
            .all(|tab| tab.panel.document() != Some(clean_document))
    );
    let snapshot = workbench.session.snapshot(document).unwrap();
    assert_eq!(snapshot.bytes, b"unsaved dirty");
    assert!(snapshot.path.is_empty());
    assert_eq!(snapshot.original_path.as_deref(), dirty.to_str());
    assert_eq!(workbench.session.document_for_view(view), Some(document));
    assert!(
        workbench
            .title(
                workbench
                    .tabs
                    .iter()
                    .find(|tab| tab.panel.document() == Some(document))
                    .unwrap()
            )
            .contains("dirty.txt (deleted)")
    );
    workbench
        .session
        .with_commands(view, |commands| commands.undo())
        .unwrap();
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        b"dirty"
    );
}
#[test]
fn failed_group_save_keeps_all_panels_and_terminals() {
    let dir = TempDir::new();
    let first = dir.write("a", b"a");
    std::fs::create_dir(dir.path("folder")).unwrap();
    let second = dir.write("folder/b", b"b");
    let mut workspace = workspace(&dir);
    workspace.open_or_focus(&first).unwrap();
    workspace.open_or_focus(&second).unwrap();
    let view = workspace.active_view().unwrap();
    workspace
        .session
        .with_commands(view, |commands| commands.paste(b"change"))
        .unwrap();
    workspace.dispatch(WindowCommand::NewTerminal).unwrap();
    let count = workspace.tab_count();
    let terminals = workspace.terminal.session_ids();
    std::fs::remove_file(second).unwrap();
    std::fs::remove_dir(dir.path("folder")).unwrap();
    assert!(workspace.request_close_all().is_err());
    assert_eq!(workspace.tab_count(), count);
    assert_eq!(workspace.terminal.session_ids(), terminals);
    assert_eq!(workspace.session.document_ids().len(), 2);
}
#[test]
fn guarded_close_persists_layout_once_and_restores_shared_views() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("file.txt", b"abc");
    let mut context = Context::create();
    let mut workspace = workspace(&dir);
    workspace
        .initialize(&mut context, WorkbenchHostMode::Fullscreen)
        .unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    workspace.open_or_focus(&path).unwrap();
    workspace.dispatch(WindowCommand::DuplicateView).unwrap();
    workspace.dispatch(WindowCommand::Settings).unwrap();
    workspace.dispatch(WindowCommand::NewContentSearch).unwrap();
    workspace.close_tab(workspace.active_index()).unwrap();
    workspace.dispatch(WindowCommand::NewContentSearch).unwrap();
    let search_id = workspace.active_panel_id().unwrap();
    frame(&mut context, &mut workspace);
    frame(&mut context, &mut workspace);
    let ids = workspace.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>();
    workspace.request_close_all().unwrap();
    let stored = std::fs::read(dir.path("config/workspaces.json")).unwrap();
    workspace.cleanup().unwrap();
    assert_eq!(
        stored,
        std::fs::read(dir.path("config/workspaces.json")).unwrap()
    );
    let mut restored = super::tests::workspace(&dir);
    restored.set_project(dir.root()).unwrap();
    assert_eq!(
        restored.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
        ids
    );
    assert_eq!(restored.session.document_ids().len(), 1);
    let doc = restored.session.document_ids()[0];
    assert_eq!(restored.session.view_count(doc), 2);
    assert!(restored.panel_visible("settings"));
    assert_eq!(restored.panel_count("search"), 1);
    assert!(restored.search_panel(search_id).is_some());
    restored.dispatch(WindowCommand::FindProject).unwrap();
    assert_eq!(restored.active_panel_id(), Some(search_id));
}
