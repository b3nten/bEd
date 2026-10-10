//! Saved content-viewer slots accept resources without changing their arrangement.
use super::tests::{frame, workspace};
use super::*;
use crate::{test_support::TempDir, workspace::tiling::Layout};
use bed_workbench_api::PanelInput;

fn restored_viewer(dir: &TempDir, kind: &str, viewer: &str) -> (Workbench, u64) {
    let mut original = workspace(dir);
    original.set_directory(dir.root()).unwrap();
    original
        .close_panels_of_type(bed_module_projects::PANEL_ID)
        .unwrap();
    original.dispatch(WindowCommand::NewSettings).unwrap();
    let settings = original.active_panel_id().unwrap();
    let slot = original
        .open_plugin_panel(kind, PanelInput::None, &Value::Null, Some(viewer.into()))
        .unwrap();
    let mut layout = Layout::default();
    let right = layout.split(1, 0, 6250, 100).unwrap();
    layout.split(right, 1, 3300, 100).unwrap();
    original.tiling = TilingState::new(layout);
    original.tiling.assign(settings, 1);
    original.tiling.assign(slot, right);
    original.dock_built = true;
    original.save_default_layout().unwrap();
    original.cleanup().unwrap();

    let mut restored = workspace(dir);
    restored.set_directory(dir.root()).unwrap();
    restored.apply_default_layout().unwrap();
    assert!(restored.error.is_none(), "{:?}", restored.error);
    let id = restored
        .tabs
        .iter()
        .find(|tab| tab.panel.kind == kind)
        .unwrap()
        .id;
    assert_eq!(restored.area_for_panel(id), Some(right));
    assert!(
        restored.panel_input_empty(&restored.tabs.iter().find(|tab| tab.id == id).unwrap().panel)
    );
    (restored, id)
}

fn empty_slot(workbench: &mut Workbench, kind: &str, viewer: &str, area: u32) -> u64 {
    workbench
        .open_plugin_panel_at(
            kind,
            PanelInput::None,
            &Value::Null,
            Some(viewer.into()),
            Some(area),
        )
        .unwrap()
}

fn assert_empty(workbench: &Workbench, id: u64) {
    let panel = &workbench
        .tabs
        .iter()
        .find(|tab| tab.id == id)
        .unwrap()
        .panel;
    assert_eq!(panel.input, PanelInput::None);
    assert!(workbench.panel_input_empty(panel));
}

fn database(dir: &TempDir, name: &str) -> PathBuf {
    let path = dir.path(name);
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE items(label TEXT); INSERT INTO items VALUES('saved');")
        .unwrap();
    path.canonicalize().unwrap()
}

fn wait_for_database(workbench: &mut Workbench, id: u64) {
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
            .downcast_mut::<bed_plugin_sqlite::SqlitePanel>()
            .unwrap();
        panel.poll();
        assert!(panel.error().is_none(), "{:?}", panel.error());
        if panel.is_ready() {
            return;
        }
        assert!(Instant::now() < deadline, "Database did not load");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn image_files_fill_default_slots_and_preserve_loaded_views_and_explicit_duplicates() {
    let dir = TempDir::new();
    let first = dir.write(
        "first.svg",
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="3"><rect width="2" height="3" fill="red"/></svg>"#,
    );
    let second = dir.write(
        "second.svg",
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="5"><rect width="4" height="5" fill="blue"/></svg>"#,
    );
    let (mut workbench, first_slot) = restored_viewer(
        &dir,
        bed_plugin_image::PANEL_ID,
        bed_plugin_image::VIEWER_ID,
    );
    let area = workbench.area_for_panel(first_slot).unwrap();
    let before = workbench.current_tiling().clone();
    let count = workbench.tabs.len();

    assert!(workbench.open_or_focus(&first).unwrap());
    assert_eq!(workbench.active_panel_id(), Some(first_slot));
    assert_eq!(workbench.current_tiling(), &before);
    assert_eq!(workbench.tabs.len(), count);
    let first_document = workbench.active_document().unwrap();
    let first_bytes = workbench.session.snapshot(first_document).unwrap().bytes;

    let second_slot = empty_slot(
        &mut workbench,
        bed_plugin_image::PANEL_ID,
        bed_plugin_image::VIEWER_ID,
        area,
    );
    let before = workbench.current_tiling().clone();
    assert!(workbench.open_or_focus(&second).unwrap());
    assert_eq!(workbench.active_panel_id(), Some(second_slot));
    assert_eq!(workbench.current_tiling(), &before);
    assert_eq!(workbench.tabs.len(), count + 1);
    let second_document = workbench.active_document().unwrap();
    assert_ne!(first_document, second_document);
    assert_eq!(
        workbench.session.snapshot(first_document).unwrap().bytes,
        first_bytes
    );

    let unused = empty_slot(
        &mut workbench,
        bed_plugin_image::PANEL_ID,
        bed_plugin_image::VIEWER_ID,
        area,
    );
    let geometry = workbench.current_tiling().layout.clone();
    assert!(!workbench.open_or_focus(&first).unwrap());
    assert_eq!(workbench.active_panel_id(), Some(first_slot));
    assert_empty(&workbench, unused);
    assert!(
        workbench
            .open_file_with_viewer(&second, Some(bed_plugin_image::VIEWER_ID), true)
            .unwrap()
    );
    assert_ne!(workbench.active_panel_id(), Some(unused));
    assert_ne!(workbench.active_panel_id(), Some(second_slot));
    assert_eq!(workbench.active_document(), Some(second_document));
    assert_eq!(workbench.tabs.len(), count + 3);
    assert_eq!(workbench.current_tiling().layout, geometry);
    assert_empty(&workbench, unused);
    workbench.cleanup().unwrap();
}

#[test]
fn sqlite_files_fill_default_slots_without_document_sessions_or_replacing_loaded_databases() {
    let dir = TempDir::new();
    let first = database(&dir, "first.sqlite");
    let second = database(&dir, "second.sqlite");
    let (mut workbench, first_slot) = restored_viewer(
        &dir,
        bed_plugin_sqlite::PANEL_ID,
        bed_plugin_sqlite::VIEWER_ID,
    );
    let area = workbench.area_for_panel(first_slot).unwrap();
    let before = workbench.current_tiling().clone();
    let count = workbench.tabs.len();

    assert!(workbench.open_or_focus(&first).unwrap());
    assert_eq!(workbench.active_panel_id(), Some(first_slot));
    assert_eq!(workbench.current_tiling(), &before);
    assert_eq!(workbench.tabs.len(), count);
    wait_for_database(&mut workbench, first_slot);

    let second_slot = empty_slot(
        &mut workbench,
        bed_plugin_sqlite::PANEL_ID,
        bed_plugin_sqlite::VIEWER_ID,
        area,
    );
    let before = workbench.current_tiling().clone();
    assert!(workbench.open_or_focus(&second).unwrap());
    assert_eq!(workbench.active_panel_id(), Some(second_slot));
    assert_eq!(workbench.current_tiling(), &before);
    assert_eq!(workbench.tabs.len(), count + 1);
    wait_for_database(&mut workbench, second_slot);
    let first_panel = &workbench
        .tabs
        .iter()
        .find(|tab| tab.id == first_slot)
        .unwrap()
        .panel;
    assert_eq!(first_panel.input, PanelInput::LocalFile(first.clone()));
    assert!(!workbench.panel_input_empty(first_panel));
    assert!(workbench.session.document_ids().is_empty());

    let unused = empty_slot(
        &mut workbench,
        bed_plugin_sqlite::PANEL_ID,
        bed_plugin_sqlite::VIEWER_ID,
        area,
    );
    let geometry = workbench.current_tiling().layout.clone();
    assert!(!workbench.open_or_focus(&first).unwrap());
    assert_eq!(workbench.active_panel_id(), Some(first_slot));
    assert_empty(&workbench, unused);
    assert!(
        workbench
            .open_file_with_viewer(&second, Some(bed_plugin_sqlite::VIEWER_ID), true)
            .unwrap()
    );
    assert_ne!(workbench.active_panel_id(), Some(unused));
    assert_ne!(workbench.active_panel_id(), Some(second_slot));
    assert_eq!(workbench.tabs.len(), count + 3);
    assert_eq!(workbench.current_tiling().layout, geometry);
    assert_empty(&workbench, unused);
    assert!(workbench.session.document_ids().is_empty());
    workbench.cleanup().unwrap();
}

#[test]
fn missing_files_and_non_file_inputs_leave_default_viewers_unloaded() {
    let dir = TempDir::new();
    for (kind, viewer, missing) in [
        (
            bed_plugin_image::PANEL_ID,
            bed_plugin_image::VIEWER_ID,
            "missing.svg",
        ),
        (
            bed_plugin_sqlite::PANEL_ID,
            bed_plugin_sqlite::VIEWER_ID,
            "missing.sqlite",
        ),
    ] {
        let (mut workbench, slot) = restored_viewer(&dir, kind, viewer);
        let before = workbench.current_tiling().clone();
        let count = workbench.tabs.len();
        for path in [dir.path(missing), dir.root().to_owned()] {
            assert!(
                workbench
                    .open_file_with_viewer(&path, Some(viewer), false)
                    .is_err()
            );
            assert_eq!(workbench.active_panel_id(), Some(slot));
            assert_eq!(workbench.current_tiling(), &before);
            assert_eq!(workbench.tabs.len(), count);
            assert_empty(&workbench, slot);
            assert!(workbench.session.document_ids().is_empty());
        }
        workbench.cleanup().unwrap();
    }
}

#[test]
fn every_unloaded_content_viewer_renders_without_resource_sessions() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let mut workbench = workspace(&dir);
    workbench.set_directory(dir.root()).unwrap();
    workbench
        .close_panels_of_type(bed_module_projects::PANEL_ID)
        .unwrap();
    workbench.settings.settings["ui_animations"] = json!(false);
    let mut slots = Vec::new();
    for (kind, viewer) in [
        (bed_plugin_image::PANEL_ID, bed_plugin_image::VIEWER_ID),
        (bed_plugin_gltf::PANEL_ID, bed_plugin_gltf::VIEWER_ID),
        (bed_plugin_audio::PANEL_ID, bed_plugin_audio::VIEWER_ID),
        (bed_plugin_font::PANEL_ID, bed_plugin_font::VIEWER_ID),
        (bed_plugin_csv::PANEL_ID, bed_plugin_csv::VIEWER_ID),
        (bed_plugin_json::PANEL_ID, bed_plugin_json::VIEWER_ID),
        (
            bed_plugin_markdown::PANEL_ID,
            bed_plugin_markdown::VIEWER_ID,
        ),
        (bed_plugin_sqlite::PANEL_ID, bed_plugin_sqlite::VIEWER_ID),
    ] {
        slots.push(
            workbench
                .open_plugin_panel(kind, PanelInput::None, &Value::Null, Some(viewer.into()))
                .unwrap(),
        );
    }
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
    for slot in slots {
        let index = workbench
            .tabs
            .iter()
            .position(|tab| tab.id == slot)
            .unwrap();
        workbench.switch_to_tab(index);
        for _ in 0..3 {
            frame(&mut context, &mut workbench);
        }
        assert!(workbench.error.is_none(), "{:?}", workbench.error);
        assert_empty(&workbench, slot);
        let panel = &workbench.tabs[index].panel;
        assert!(panel.instance.render_output().is_none());
        assert_eq!(panel.instance.save_state(), Value::Null);
        assert!(workbench.session.document_ids().is_empty());
        assert_eq!(workbench.terminal.session_count(), 0);
    }
    workbench.cleanup().unwrap();
}
