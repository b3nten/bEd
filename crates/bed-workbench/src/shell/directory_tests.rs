use super::tests::workspace;
use super::*;
use crate::test_support::TempDir;

#[test]
fn unsaved_window_keeps_its_base_when_terminal_focus_changes() {
    let dir = TempDir::new();
    let first_file = dir
        .write("first/note.txt", b"first needle")
        .canonicalize()
        .unwrap();
    let second_file = dir
        .write("second/note.txt", b"second needle")
        .canonicalize()
        .unwrap();
    let first = first_file.parent().unwrap().canonicalize().unwrap();
    let second = second_file.parent().unwrap().canonicalize().unwrap();
    let mut workbench = workspace(&dir);
    workbench.set_directory(&first).unwrap();
    assert_eq!(workbench.working_directory(), first);
    let first_terminal = workbench.terminal.new_session_at(&first);
    workbench.dispatch(WindowCommand::NewSettings).unwrap();
    assert_eq!(workbench.working_directory(), first);
    assert_eq!(
        workbench.window_title(),
        format!("bEd • {}", first.display())
    );
    assert!(workbench.dispatch(WindowCommand::NewExplorer).unwrap());
    assert!(workbench.dispatch(WindowCommand::NewContentSearch).unwrap());
    let first_search = workbench.active_panel_id().unwrap();
    workbench.search_panel(first_search).unwrap().query = "needle".into();
    let second_terminal = workbench.terminal.new_session_at(&second);
    workbench.dispatch(WindowCommand::NewSettings).unwrap();
    assert_eq!(workbench.working_directory(), first);
    assert_eq!(
        workbench.window_title(),
        format!("bEd • {}", first.display())
    );
    workbench.dispatch(WindowCommand::NewContentSearch).unwrap();
    let second_search = workbench.active_panel_id().unwrap();
    workbench.search_panel(second_search).unwrap().query = "needle".into();
    assert_eq!(workbench.files_root(), first.to_str().unwrap());
    assert_eq!(
        workbench.search_panel(first_search).unwrap().root,
        first.to_str().unwrap()
    );
    assert_eq!(
        workbench.search_panel(second_search).unwrap().root,
        first.to_str().unwrap()
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        workbench.tick().unwrap();
        if workbench.search_panel(first_search).unwrap().results.len() == 1
            && workbench.search_panel(second_search).unwrap().results.len() == 1
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Folder searches did not finish: {:?}",
            workbench.error
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        workbench.search_panel(first_search).unwrap().results[0]
            .file
            .full_path,
        first_file.to_string_lossy()
    );
    assert_eq!(
        workbench.search_panel(second_search).unwrap().results[0]
            .file
            .full_path,
        first_file.to_string_lossy()
    );
    workbench.open_or_focus(&first_file).unwrap();
    let document = workbench.active_document().unwrap();
    let renamed = workbench.rename_path(&first_file, "renamed.txt").unwrap();
    assert!(renamed.is_file());
    assert_eq!(
        workbench.session.snapshot(document).unwrap().path,
        renamed.to_string_lossy()
    );
    assert!(workbench.rename_path(&second_file, "outside.txt").is_err());
    workbench.terminal.focus_session(first_terminal);
    assert_eq!(workbench.working_directory(), first);
    workbench.terminal.close_session_id(first_terminal);
    workbench.terminal.close_session_id(second_terminal);
    assert_eq!(workbench.working_directory(), first);
    assert!(workbench.workspace_spec.is_none());
    workbench.cleanup().unwrap();
    assert!(!dir.path("config/workspaces.json").exists());
    assert!(!first.join(".undo-redo-bed.json").exists());
}

#[test]
fn unsaved_tools_are_available_and_project_services_stay_disabled() {
    let dir = TempDir::new();
    let mut workbench = workspace(&dir);
    workbench.set_directory(dir.root()).unwrap();
    for command in [
        WindowCommand::Debug,
        WindowCommand::NewDiagnostics,
        WindowCommand::NewReferences,
        WindowCommand::NewLspDashboard,
    ] {
        assert!(!workbench.dispatch(command).unwrap());
    }
    for command in [
        WindowCommand::NewExplorer,
        WindowCommand::FindFile,
        WindowCommand::NewContentSearch,
        WindowCommand::NewStructure,
        WindowCommand::NewDocument,
    ] {
        assert!(workbench.dispatch(command).unwrap());
    }
    let view = workbench.active_view().unwrap();
    let document = workbench.active_document().unwrap();
    assert!(
        workbench
            .dispatch_command(bed_module_git::SHOW_COMMAND)
            .unwrap()
    );
    assert!(
        !workbench
            .dispatch_command(bed_module_debug::SHOW_COMMAND)
            .unwrap()
    );
    assert!(workbench.session.lsp().is_none());
    assert!(workbench.session.options().project_root.is_none());
    assert!(!workbench.session.options().persistent_history);
    workbench
        .session
        .with_commands(view, |commands| commands.type_text(b"hello"))
        .unwrap();
    workbench
        .session
        .with_commands(view, |commands| commands.undo())
        .unwrap();
    assert!(
        workbench
            .session
            .snapshot(document)
            .unwrap()
            .bytes
            .is_empty()
    );
    // New unnamed documents still need Save As after undoing to empty. This
    // test checks rootless services and undo, so discard its temporary buffer.
    let index = workbench
        .tabs
        .iter()
        .position(|tab| tab.panel.view_id() == Some(view))
        .unwrap();
    workbench.remove_tab(index).unwrap();
    workbench.cleanup().unwrap();
}

#[test]
fn workspace_keeps_its_default_directory_and_removes_transient_scope_labels() {
    let dir = TempDir::new();
    dir.write("project/note.txt", b"note");
    dir.write("other/note.txt", b"other");
    let mut workbench = workspace(&dir);
    workbench.set_directory(&dir.path("other")).unwrap();
    workbench.dispatch(WindowCommand::NewExplorer).unwrap();
    workbench.set_project(&dir.path("project")).unwrap();
    workbench.terminal.new_session_at(dir.path("other"));
    assert_eq!(
        workbench.working_directory(),
        dir.path("project").canonicalize().unwrap()
    );
    assert_eq!(workbench.window_title(), "bEd • project");
    assert!(workbench.session.options().persistent_history);
    workbench.cleanup().unwrap();
}
