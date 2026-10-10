//! Resource opening reuses only pristine compatible panels without moving work.
use super::*;
use crate::{shell::tests::workspace, test_support::TempDir, workspace::tiling::Layout};
use bed_workbench_api::{HostRequest, PanelInput};

fn empty_workbench(dir: &TempDir) -> Workbench {
    let mut workbench = workspace(dir);
    workbench
        .close_panels_of_type(bed_module_projects::PANEL_ID)
        .unwrap();
    workbench.tiling = TilingState::default();
    workbench.dock_built = true;
    workbench
}

fn new_editor(workbench: &mut Workbench, area: u32) -> (u64, DocumentId) {
    workbench
        .dispatch_at(WindowCommand::NewDocument, Some(area))
        .unwrap();
    (
        workbench.active_panel_id().unwrap(),
        workbench.active_document().unwrap(),
    )
}

#[test]
fn focused_empty_editor_reuses_its_tab_and_releases_its_scratch_document() {
    let dir = TempDir::new();
    let path = dir.write("file.txt", b"opened contents");
    let mut workbench = empty_workbench(&dir);
    let (first, scratch) = new_editor(&mut workbench, 1);
    let (second, other_scratch) = new_editor(&mut workbench, 1);
    workbench.tiling.areas[0].tabs = vec![second, first];
    let index = workbench
        .tabs
        .iter()
        .position(|tab| tab.id == first)
        .unwrap();
    workbench.switch_to_tab(index);
    let before = workbench.current_tiling().clone();
    let count = workbench.tabs.len();

    workbench.open_or_focus(&path).unwrap();

    assert_eq!(workbench.active_panel_id(), Some(first));
    assert_eq!(workbench.tabs.len(), count);
    assert_eq!(workbench.current_tiling(), &before);
    assert_eq!(
        workbench.active_snapshot().unwrap().bytes,
        b"opened contents"
    );
    assert!(!workbench.session.document_ids().contains(&scratch));
    assert!(workbench.session.document_ids().contains(&other_scratch));
    workbench.cleanup().unwrap();
}

#[test]
fn input_reuses_target_area_tab_order_before_other_areas() {
    let dir = TempDir::new();
    let path = dir.write("file.txt", b"contents");
    let mut workbench = empty_workbench(&dir);
    let mut layout = Layout::default();
    let right = layout.split(1, 0, 5000, 100).unwrap();
    workbench.tiling = TilingState::new(layout);
    let (left_editor, left_scratch) = new_editor(&mut workbench, 1);
    let (first_right, first_scratch) = new_editor(&mut workbench, right);
    let (second_right, replaced_scratch) = new_editor(&mut workbench, right);
    workbench
        .dispatch_at(WindowCommand::NewSettings, Some(right))
        .unwrap();
    let settings = workbench.active_panel_id().unwrap();
    let group = workbench
        .tiling
        .areas
        .iter_mut()
        .find(|group| group.area == right)
        .unwrap();
    group.tabs = vec![second_right, first_right, settings];
    let before = workbench.current_tiling().clone();

    workbench
        .open_file_with_viewer_at(&path, None, false, Some(right))
        .unwrap();

    assert_eq!(workbench.active_panel_id(), Some(second_right));
    assert_eq!(workbench.area_for_panel(second_right), Some(right));
    assert_eq!(workbench.area_for_panel(left_editor), Some(1));
    assert_eq!(workbench.current_tiling().layout, before.layout);
    assert_eq!(
        workbench.current_tiling().areas[1].tabs,
        before.areas[1].tabs
    );
    assert!(workbench.session.document_ids().contains(&left_scratch));
    assert!(workbench.session.document_ids().contains(&first_scratch));
    assert!(!workbench.session.document_ids().contains(&replaced_scratch));
    workbench.cleanup().unwrap();
}

#[test]
fn an_explicit_target_without_a_compatible_slot_keeps_other_area_editors_empty() {
    let dir = TempDir::new();
    let path = dir.write("file.txt", b"contents");
    let mut workbench = empty_workbench(&dir);
    let mut layout = Layout::default();
    let right = layout.split(1, 0, 5000, 100).unwrap();
    workbench.tiling = TilingState::new(layout.clone());
    let (empty_id, scratch) = new_editor(&mut workbench, 1);
    workbench
        .dispatch_at(WindowCommand::NewSettings, Some(right))
        .unwrap();
    let count = workbench.tabs.len();

    workbench
        .open_file_with_viewer_at(&path, None, false, Some(right))
        .unwrap();

    let opened = workbench.active_panel_id().unwrap();
    assert_ne!(opened, empty_id);
    assert_eq!(workbench.tabs.len(), count + 1);
    assert_eq!(workbench.area_for_panel(opened), Some(right));
    assert_eq!(workbench.area_for_panel(empty_id), Some(1));
    assert_eq!(workbench.current_tiling().layout, layout);
    assert!(workbench.session.document_ids().contains(&scratch));
    assert!(
        workbench
            .session
            .snapshot(scratch)
            .unwrap()
            .bytes
            .is_empty()
    );
    workbench.cleanup().unwrap();
}

#[test]
fn an_additional_remote_completion_creates_a_view_even_when_the_document_has_a_panel() {
    let dir = TempDir::new();
    let path = dir.write("file.txt", b"contents");
    let mut workbench = empty_workbench(&dir);
    workbench.open_or_focus(&path).unwrap();
    let original = workbench.active_panel_id().unwrap();
    let document = workbench.active_document().unwrap();
    let (empty_id, scratch) = new_editor(&mut workbench, 1);
    let request = workbench.session.snapshot(document).unwrap().path;
    workbench
        .modules
        .pending_open
        .insert(request.clone(), Some(bed_module_editor::TEXT_VIEWER.into()));
    workbench.modules.pending_additional.insert(request);
    let count = workbench.tabs.len();

    workbench.finish_remote_open(document).unwrap();

    let additional = workbench.active_panel_id().unwrap();
    assert_ne!(additional, original);
    assert_ne!(additional, empty_id);
    assert_eq!(workbench.tabs.len(), count + 1);
    assert_eq!(workbench.session.view_count(document), 2);
    assert!(workbench.session.document_ids().contains(&scratch));
    assert!(workbench.modules.pending_open.is_empty());
    assert!(workbench.modules.pending_additional.is_empty());
    workbench.cleanup().unwrap();
}

#[test]
fn shared_empty_documents_and_undone_edits_are_never_replaced() {
    let dir = TempDir::new();
    let first_path = dir.write("first.txt", b"first");
    let second_path = dir.write("second.txt", b"second");
    let mut workbench = empty_workbench(&dir);
    let (shared_id, shared_document) = new_editor(&mut workbench, 1);
    let sibling = workbench.add_view(shared_document).unwrap();
    assert_eq!(workbench.session.view_count(shared_document), 2);

    workbench.open_or_focus(&first_path).unwrap();

    assert_ne!(workbench.active_panel_id(), Some(shared_id));
    assert_ne!(workbench.active_panel_id(), Some(sibling));
    assert_eq!(workbench.session.view_count(shared_document), 2);
    assert!(workbench.tabs.iter().any(|tab| tab.id == shared_id));
    assert!(workbench.tabs.iter().any(|tab| tab.id == sibling));

    let (used_id, used_document) = new_editor(&mut workbench, 1);
    let view = workbench.active_view().unwrap();
    workbench
        .session
        .with_commands(view, |commands| commands.paste(b"used"))
        .unwrap();
    workbench.session.undo_document(used_document).unwrap();
    assert!(
        workbench
            .session
            .snapshot(used_document)
            .unwrap()
            .bytes
            .is_empty()
    );
    let count = workbench.tabs.len();

    workbench.open_or_focus(&second_path).unwrap();

    assert_eq!(workbench.tabs.len(), count + 1);
    assert_ne!(workbench.active_panel_id(), Some(used_id));
    assert!(workbench.session.document_ids().contains(&used_document));
    assert_eq!(workbench.session.view_count(shared_document), 2);
    // Undo retains edit history and dirty state. Discard this test buffer
    // explicitly so normal window cleanup does not request Save As.
    let used = workbench
        .tabs
        .iter()
        .position(|tab| tab.id == used_id)
        .unwrap();
    workbench.remove_tab(used).unwrap();
    workbench.cleanup().unwrap();
}

#[test]
fn incompatible_viewers_and_explicit_additional_views_keep_empty_editors() {
    let dir = TempDir::new();
    let path = dir.write("file.txt", b"contents");
    let mut workbench = empty_workbench(&dir);
    let (empty_id, scratch) = new_editor(&mut workbench, 1);
    workbench
        .open_file_with_viewer(&path, Some(bed_module_editor::HEX_VIEWER), false)
        .unwrap();
    assert_ne!(workbench.active_panel_id(), Some(empty_id));
    assert!(workbench.session.document_ids().contains(&scratch));
    assert_eq!(workbench.tabs.len(), 2);
    workbench.cleanup().unwrap();

    let mut additional = empty_workbench(&dir);
    let (empty_id, scratch) = new_editor(&mut additional, 1);
    additional
        .open_file_with_viewer(&path, Some(bed_module_editor::TEXT_VIEWER), true)
        .unwrap();
    assert_ne!(additional.active_panel_id(), Some(empty_id));
    assert!(additional.session.document_ids().contains(&scratch));
    assert_eq!(additional.tabs.len(), 2);
    additional.cleanup().unwrap();
}

#[test]
fn existing_file_focus_has_priority_over_an_empty_editor() {
    let dir = TempDir::new();
    let path = dir.write("file.txt", b"contents");
    let mut workbench = empty_workbench(&dir);
    workbench.open_or_focus(&path).unwrap();
    let loaded = workbench.active_panel_id().unwrap();
    let (empty_id, scratch) = new_editor(&mut workbench, 1);
    let count = workbench.tabs.len();

    assert!(!workbench.open_or_focus(&path).unwrap());

    assert_eq!(workbench.active_panel_id(), Some(loaded));
    assert_eq!(workbench.tabs.len(), count);
    assert!(workbench.tabs.iter().any(|tab| tab.id == empty_id));
    assert!(workbench.session.document_ids().contains(&scratch));
    workbench.cleanup().unwrap();
}

#[test]
fn failed_file_open_leaves_the_empty_editor_untouched() {
    let dir = TempDir::new();
    let mut workbench = empty_workbench(&dir);
    let (empty_id, scratch) = new_editor(&mut workbench, 1);
    let before = workbench.current_tiling().clone();

    assert!(workbench.open_or_focus(&dir.path("missing.txt")).is_err());

    assert_eq!(workbench.active_panel_id(), Some(empty_id));
    assert_eq!(workbench.tabs.len(), 1);
    assert_eq!(workbench.current_tiling(), &before);
    assert_eq!(workbench.active_document(), Some(scratch));
    assert!(workbench.panel_input_empty(&workbench.tabs[0].panel));
    workbench.cleanup().unwrap();
}

#[test]
fn comparison_requests_fill_an_empty_diff_and_failed_factories_keep_it_empty() {
    let dir = TempDir::new();
    let mut workbench = empty_workbench(&dir);
    let empty = workbench
        .open_plugin_panel(
            bed_module_git::DIFF_PANEL_ID,
            PanelInput::None,
            &Value::Null,
            None,
        )
        .unwrap();
    let before = workbench.current_tiling().clone();
    workbench.modules.requests.push(HostRequest::ShowPanel {
        panel_type: bed_module_git::DIFF_PANEL_ID.into(),
        document: None,
        state: json!({"path":"../outside.rs","side":"unstaged"}),
        action: None,
    });
    workbench.process_plugin_requests().unwrap();
    assert_eq!(workbench.active_panel_id(), Some(empty));
    assert_eq!(workbench.current_tiling(), &before);
    assert!(workbench.panel_input_empty(&workbench.tabs[0].panel));

    workbench.modules.requests.push(HostRequest::ShowPanel {
        panel_type: bed_module_git::DIFF_PANEL_ID.into(),
        document: None,
        state: json!({"path":"a.rs","side":"unstaged"}),
        action: None,
    });
    workbench.process_plugin_requests().unwrap();

    assert_eq!(workbench.active_panel_id(), Some(empty));
    assert_eq!(workbench.tabs.len(), 1);
    assert_eq!(workbench.current_tiling(), &before);
    assert!(!workbench.panel_input_empty(&workbench.tabs[0].panel));
    assert_eq!(
        workbench.tabs[0].panel.instance.save_state()["path"],
        "a.rs"
    );
    workbench.cleanup().unwrap();
}

#[test]
fn repeated_singleton_default_entries_do_not_duplicate_runtime_membership() {
    let dir = TempDir::new();
    let mut workbench = empty_workbench(&dir);
    let mut layout = Layout::default();
    let right = layout.split(1, 0, 5000, 100).unwrap();
    let mut tiling = TilingState::new(layout.clone());
    tiling.assign(11, 1);
    tiling.assign(12, right);
    workbench
        .store
        .as_mut()
        .unwrap()
        .set_default_layout(json!({
            "version":1,
            "panels":[
                {"id":11,"panel_type":bed_module_git::PANEL_ID,"viewer":null},
                {"id":12,"panel_type":bed_module_git::PANEL_ID,"viewer":null},
            ],
            "tiling":tiling.to_value([11,12]),
            "focused":11,
        }))
        .unwrap();

    workbench.apply_default_layout().unwrap();

    assert_eq!(workbench.tabs.len(), 1);
    let id = workbench.tabs[0].id;
    assert_eq!(workbench.current_tiling().layout, layout);
    assert_eq!(workbench.current_tiling().areas[0].tabs, [id]);
    assert!(workbench.current_tiling().areas[1].tabs.is_empty());
    assert_eq!(workbench.active_panel_id(), Some(id));
    assert!(workbench.error.is_some());
    workbench.cleanup().unwrap();
}
