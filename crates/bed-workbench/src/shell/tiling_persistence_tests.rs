//! Real panel restoration uses workspace geometry without a native frame.
use super::*;
use crate::{
    shell::tests::workspace,
    test_support::TempDir,
    workspace::{tiling::Layout, tiling_state::TilingState},
};

#[test]
fn restored_empty_areas_stay_empty_instead_of_opening_a_projects_panel() {
    let dir = TempDir::new();
    let mut workbench = workspace(&dir);
    workbench
        .close_panels_of_type(bed_module_projects::PANEL_ID)
        .unwrap();
    let mut layout = Layout::default();
    layout.split(1, 0, 5000, 100).unwrap();
    let tiling = TilingState::new(layout);
    workbench
        .restore_workspace(&json!({
            "version":2,
            "panels":[],
            "tiling":tiling.to_value([]),
        }))
        .unwrap();
    assert!(workbench.tabs.is_empty());
    assert_eq!(workbench.tiling, tiling);
}

#[test]
fn saving_before_first_frame_restores_area_geometry_tab_order_and_shared_documents() {
    let dir = TempDir::new();
    let path = dir.write("document.txt", b"shared document");
    let mut original = workspace(&dir);
    original.set_project(dir.root()).unwrap();
    original.open_or_focus(&path).unwrap();
    original.dispatch(WindowCommand::DuplicateView).unwrap();
    original.dispatch(WindowCommand::NewSettings).unwrap();
    let mut layout = Layout::default();
    let right = layout.split(1, 0, 5000, 100).unwrap();
    layout.split(right, 1, 5000, 100).unwrap();
    let mut tiling = TilingState::new(layout);
    for (index, tab) in original.tabs.iter().rev().enumerate() {
        tiling.assign(tab.id, index as u32 % 3 + 1);
    }
    if let Some(focused) = original.focused {
        let area = tiling.area_for(focused).unwrap();
        tiling
            .areas
            .iter_mut()
            .find(|group| group.area == area)
            .unwrap()
            .selected = Some(focused);
    }
    original.tiling = tiling;
    original.dock_built = true;
    original.persist_workspace().unwrap();
    let mut saved = original.last_state.clone().unwrap();
    assert_eq!(saved["version"], json!(2));
    assert!(original.context_binding.is_none());
    let saved_ids = saved["panels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|panel| panel["id"].as_u64().unwrap())
        .collect::<Vec<_>>();
    let expected = TilingState::from_value(&saved["tiling"], saved_ids.iter().copied()).unwrap();
    for group in saved["tiling"]["areas"].as_array_mut().unwrap() {
        group["tab_bar_visible"] = json!(false);
    }
    original
        .store
        .as_mut()
        .unwrap()
        .set_layout(original.workspace_spec.as_ref().unwrap(), saved.clone())
        .unwrap();
    let mut restored = workspace(&dir);
    restored.open_startup_workspace(dir.root()).unwrap();
    assert!(restored.error.is_none(), "{:?}", restored.error);
    assert_eq!(restored.tiling, expected);
    assert_eq!(restored.session.document_ids().len(), 1);
    let document = restored.session.document_ids()[0];
    assert_eq!(restored.session.view_count(document), 2);
    restored.persist_workspace().unwrap();
    assert_eq!(
        restored.last_state.as_ref().unwrap()["tiling"],
        expected.to_value(saved_ids),
        "legacy hidden-bar overrides are ignored on restore and omitted on the next save"
    );
}

#[test]
fn invalid_saved_membership_restores_real_documents_into_a_valid_default_layout() {
    let dir = TempDir::new();
    let path = dir.write("document.txt", b"never lose this document");
    let mut original = workspace(&dir);
    original.set_project(dir.root()).unwrap();
    original.open_or_focus(&path).unwrap();
    original.dispatch(WindowCommand::DuplicateView).unwrap();
    original.persist_workspace().unwrap();
    let mut saved = original.last_state.clone().unwrap();
    let id = saved["panels"][0]["id"].as_u64().unwrap();
    saved["tiling"]["areas"][0]["tabs"]
        .as_array_mut()
        .unwrap()
        .push(json!(id));
    original
        .store
        .as_mut()
        .unwrap()
        .set_layout(original.workspace_spec.as_ref().unwrap(), saved)
        .unwrap();
    let mut restored = workspace(&dir);
    restored.open_startup_workspace(dir.root()).unwrap();
    assert!(
        restored
            .error
            .as_deref()
            .unwrap()
            .contains("default layout")
    );
    assert_eq!(restored.session.document_ids().len(), 1);
    let document = restored.session.document_ids()[0];
    assert_eq!(restored.session.view_count(document), 2);
    for tab in &restored.tabs {
        assert!(restored.tiling.area_for(tab.id).is_some());
    }
    restored.persist_workspace().unwrap();
    let saved = restored.last_state.as_ref().unwrap();
    assert!(
        TilingState::from_value(
            &saved["tiling"],
            saved["panels"]
                .as_array()
                .unwrap()
                .iter()
                .map(|panel| panel["id"].as_u64().unwrap()),
        )
        .is_ok()
    );
}

#[test]
fn attaching_a_saved_tiling_remaps_collisions_and_keeps_unsaved_carried_work() {
    let dir = TempDir::new();
    let carried_path = dir.write("carried.txt", b"live work");
    let saved_path = dir.write("project/saved.txt", b"saved work");
    let mut workbench = workspace(&dir);
    workbench.open_or_focus(&carried_path).unwrap();
    let carried = workbench.active_panel_id().unwrap();
    let document = workbench.active_document().unwrap();
    let view = workbench.active_view().unwrap();
    workbench
        .session
        .with_commands(view, |commands| commands.paste(b"dirty "))
        .unwrap();
    let mut layout = Layout::default();
    let right = layout.split(1, 0, 2000, 100).unwrap();
    let mut tiling = TilingState::new(layout);
    tiling.assign(carried, right);
    let spec = WorkspaceSpec::local(
        dir.path("project")
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap(),
    );
    workbench
        .store
        .as_mut()
        .unwrap()
        .set_layout(
            &spec,
            json!({
                "version":2,
                "tiling":tiling.to_value([carried]),
                "panels":[{"id":carried,"kind":"document","path":saved_path}],
                "focused":carried,
                "active_document_panel":carried,
            }),
        )
        .unwrap();
    assert!(workbench.attach_workspace(&dir.path("project")).unwrap());
    assert!(workbench.session.snapshot(document).unwrap().dirty);
    let saved_document = workbench.session.document_for_path(&saved_path).unwrap();
    let saved_panel = workbench
        .tabs
        .iter()
        .find(|tab| tab.panel.document() == Some(saved_document))
        .unwrap()
        .id;
    assert_ne!(saved_panel, carried);
    assert_eq!(workbench.tiling.layout, tiling.layout);
    assert_eq!(workbench.tiling.area_for(saved_panel), Some(right));
    assert_eq!(workbench.tiling.area_for(carried), Some(right));
    assert!(
        workbench
            .tabs
            .iter()
            .any(|tab| tab.panel.view_id() == Some(view))
    );
    assert!(workbench.pending_graft.is_none());
    assert!(workbench.last_state.is_some());
}
