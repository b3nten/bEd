//! SQLite tabs retain files directly throughout workbench lifecycle operations.
use super::tests::{frame, workspace};
use super::*;
use crate::test_support::TempDir;
use bed_plugin_sqlite::{PANEL_ID, SqlitePanel, VIEWER_ID};
use bed_workbench_api::PanelInput;
use std::{fs, thread};

fn database(dir: &TempDir) -> PathBuf {
    let path = dir.path("project/data.sqlite");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(
            "CREATE TABLE items(id INTEGER PRIMARY KEY, label TEXT); \
             INSERT INTO items VALUES(1, 'first'); \
             CREATE TABLE notes(note TEXT); INSERT INTO notes VALUES('saved');",
        )
        .unwrap();
    fs::canonicalize(path).unwrap()
}

fn sqlite_panel(workbench: &Workbench, id: u64) -> &SqlitePanel {
    workbench
        .tabs
        .iter()
        .find(|tab| tab.id == id)
        .unwrap()
        .panel
        .instance
        .as_any()
        .downcast_ref()
        .unwrap()
}

fn wait_ready(workbench: &mut Workbench, id: u64) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let panel = workbench
            .tabs
            .iter_mut()
            .find(|tab| tab.id == id)
            .unwrap()
            .panel
            .instance
            .as_any_mut()
            .downcast_mut::<SqlitePanel>()
            .unwrap();
        panel.poll();
        assert!(panel.error().is_none(), "{:?}", panel.error());
        if panel.is_ready() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "SQLite panel did not become ready"
        );
        thread::sleep(Duration::from_millis(2));
    }
}

fn initialize(workbench: &mut Workbench) -> Context {
    workbench.settings.settings["ui_animations"] = json!(false);
    let mut context = Context::create();
    context.set_ini_filename(None::<PathBuf>).unwrap();
    workbench
        .initialize(&mut context, WorkbenchHostMode::Fullscreen)
        .unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    for _ in 0..3 {
        frame(&mut context, workbench);
    }
    context
}

#[test]
fn large_database_auto_open_deduplicates_without_creating_editor_documents() {
    let dir = TempDir::new();
    let path = database(&dir);
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(bed_files::files::MAX_FILE_SIZE as u64 + 4096)
        .unwrap();
    let mut workbench = workspace(&dir);
    workbench.set_project(&dir.path("project")).unwrap();
    assert!(workbench.open_or_focus(&path).unwrap());
    let id = workbench.active_panel_id().unwrap();
    assert_eq!(
        workbench
            .tabs
            .iter()
            .find(|tab| tab.id == id)
            .unwrap()
            .panel
            .input,
        PanelInput::LocalFile(path.clone())
    );
    wait_ready(&mut workbench, id);
    assert!(!workbench.open_or_focus(&path).unwrap());
    assert_eq!(workbench.active_panel_id(), Some(id));
    assert_eq!(
        workbench
            .tabs
            .iter()
            .filter(|tab| tab.panel.kind == PANEL_ID)
            .count(),
        1
    );
    assert!(workbench.session.document_ids().is_empty());
    assert!(workbench.active_document().is_none());
    assert!(workbench.active_view().is_none());
    assert!(workbench.active_snapshot().is_none());
    workbench.refresh_plugins().unwrap();
    assert!(workbench.modules.frame.documents.is_empty());
    workbench.cleanup().unwrap();
}

#[test]
fn sqlite_focus_disables_editor_save_and_duplicate_split_retain_file_backing() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = database(&dir);
    let text = dir.write("project/notes.txt", b"original text");
    let mut workbench = workspace(&dir);
    workbench.settings.settings["autosave"] = json!(false);
    workbench.set_project(&dir.path("project")).unwrap();
    workbench.open_or_focus(&text).unwrap();
    let document = workbench.active_document().unwrap();
    let view = workbench.active_view().unwrap();
    workbench
        .session
        .with_commands(view, |commands| commands.paste(b"unsaved "))
        .unwrap();
    workbench.open_or_focus(&path).unwrap();
    let original = workbench.active_panel_id().unwrap();
    wait_ready(&mut workbench, original);
    let mut context = initialize(&mut workbench);
    assert!(workbench.active_document().is_none());
    assert!(workbench.active_view().is_none());
    assert!(!workbench.dispatch(WindowCommand::Save).unwrap());
    assert!(workbench.session.snapshot(document).unwrap().dirty);
    assert_eq!(fs::read(&text).unwrap(), b"original text");

    workbench.dispatch(WindowCommand::DuplicateView).unwrap();
    let duplicate = workbench.active_panel_id().unwrap();
    assert_ne!(duplicate, original);
    wait_ready(&mut workbench, duplicate);
    for _ in 0..3 {
        frame(&mut context, &mut workbench);
    }
    workbench.dispatch(WindowCommand::SplitRight).unwrap();
    let split = workbench.active_panel_id().unwrap();
    wait_ready(&mut workbench, split);
    for _ in 0..4 {
        frame(&mut context, &mut workbench);
    }
    let local_tabs = workbench
        .tabs
        .iter()
        .filter(|tab| tab.panel.kind == PANEL_ID)
        .collect::<Vec<_>>();
    assert_eq!(local_tabs.len(), 3);
    for tab in &local_tabs {
        assert_eq!(tab.panel.input, PanelInput::LocalFile(path.clone()));
        assert_eq!(tab.panel.viewer.as_deref(), Some(VIEWER_ID));
        assert!(tab.panel.document().is_none());
        assert_ne!(tab.dock_id, 0);
    }
    assert_ne!(
        local_tabs
            .iter()
            .find(|tab| tab.id == duplicate)
            .unwrap()
            .dock_id,
        local_tabs
            .iter()
            .find(|tab| tab.id == split)
            .unwrap()
            .dock_id,
        "split SQLite panels occupy separate dock groups"
    );
    assert_eq!(workbench.session.document_ids(), vec![document]);
    assert!(workbench.active_document().is_none());
    assert!(workbench.active_view().is_none());
    assert!(!workbench.dispatch(WindowCommand::Save).unwrap());
    assert_eq!(fs::read(&text).unwrap(), b"original text");
    workbench
        .session
        .with_commands(view, |commands| commands.undo())
        .unwrap();
    workbench.cleanup().unwrap();
}

#[test]
fn saved_workspace_restores_local_backing_and_sql_draft_without_running_it() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = database(&dir);
    let draft = "SELECT * FROM run_only_when_requested";
    let mut workbench = workspace(&dir);
    workbench.set_project(&dir.path("project")).unwrap();
    let spec = workbench.workspace_spec.clone().unwrap();
    let viewer = workbench
        .modules
        .registry
        .viewer(VIEWER_ID)
        .unwrap()
        .clone();
    let id = workbench
        .add_local_file_panel(
            &path,
            &viewer,
            &json!({"selected_table":"notes", "tab":"Query", "sql":draft}),
        )
        .unwrap();
    wait_ready(&mut workbench, id);
    workbench.persist_workspace().unwrap();
    let layout = workbench.store.as_ref().unwrap().layout(&spec).unwrap();
    let saved = layout["panels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|panel| panel["panel_type"] == PANEL_ID)
        .unwrap();
    assert_eq!(saved["backing"], "local_file");
    assert_eq!(saved["path"], path.to_str().unwrap());
    assert!(saved["document_kind"].is_null());
    assert_eq!(saved["state"]["sql"], draft);
    workbench.cleanup().unwrap();

    let mut restored = workspace(&dir);
    restored.set_workspace(spec).unwrap();
    let id = restored
        .tabs
        .iter()
        .find(|tab| tab.panel.kind == PANEL_ID)
        .unwrap()
        .id;
    wait_ready(&mut restored, id);
    let mut context = initialize(&mut restored);
    for _ in 0..3 {
        frame(&mut context, &mut restored);
    }
    let panel = sqlite_panel(&restored, id);
    assert!(panel.is_ready());
    assert!(
        panel.error().is_none(),
        "the invalid draft must not have run"
    );
    let state = restored
        .tabs
        .iter()
        .find(|tab| tab.id == id)
        .unwrap()
        .panel
        .instance
        .save_state();
    assert_eq!(state["selected_table"], "notes");
    assert_eq!(state["tab"], "Query");
    assert_eq!(state["sql"], draft);
    assert!(restored.session.document_ids().is_empty());
    assert_eq!(
        restored
            .tabs
            .iter()
            .find(|tab| tab.id == id)
            .unwrap()
            .panel
            .input,
        PanelInput::LocalFile(path)
    );
    restored.cleanup().unwrap();
}

#[test]
fn mixed_saved_layout_cannot_restore_hex_over_an_open_database() {
    let dir = TempDir::new();
    let path = database(&dir);
    let spec = WorkspaceSpec::local(path.parent().unwrap().to_str().unwrap());
    let mut workbench = workspace(&dir);
    workbench
        .store
        .as_mut()
        .unwrap()
        .set_layout(
            &spec,
            json!({"version":1, "focused":20, "active_document_panel":20, "panels":[
                {"id":20, "kind":"plugin", "panel_type":PANEL_ID, "viewer":VIEWER_ID,
                 "backing":"local_file", "path":path, "state":{"tab":"Query"}},
                {"id":21, "kind":"hex", "panel_type":bed_module_editor::HEX_PANEL_TYPE,
                 "viewer":bed_module_editor::HEX_VIEWER, "document_kind":"bytes", "path":path}
            ]}),
        )
        .unwrap();
    workbench.set_workspace(spec).unwrap();
    wait_ready(&mut workbench, 20);
    assert!(
        workbench.error.is_some(),
        "the rejected editable tab reports its error"
    );
    assert!(workbench.session.document_ids().is_empty());
    assert!(!workbench.tabs.iter().any(|tab| tab.panel.hex().is_some()));
    assert!(workbench.active_document().is_none());
    assert!(
        workbench
            .session
            .open_file_with_kind(&path, DocumentKind::Bytes)
            .is_err(),
        "scoped module services cannot bypass the viewer's file protection"
    );
    let index = workbench.tabs.iter().position(|tab| tab.id == 20).unwrap();
    assert!(workbench.close_tab(index).unwrap());
    let document = workbench
        .session
        .open_file_with_kind(&path, DocumentKind::Bytes)
        .unwrap();
    assert_eq!(workbench.session.document_ids(), vec![document]);
    workbench.cleanup().unwrap();
}
