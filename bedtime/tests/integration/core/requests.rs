//! Captured requests and admission ordering are shared host behavior.
use crate::headless_host;
use crate::temp_dir;
use bedtime_api::{
    documents::{PanelBinding, PanelInput},
    features::HostAction,
    workbench::{PanelTarget, WorkbenchAction},
};
use bedtime_core::*;
use bedtime_documents::{ByteEdit, DocumentSession};
use bedtime_workbench::{PanelPresentation, WorkbenchLayout, WorkbenchRequests};
use bevy::prelude::*;
use serde_json::Value;
use temp_dir::TempDir;
struct Tool;
impl bedtime_api::workbench::Panel for Tool {
    fn title(&self, _: &World) -> String {
        "Tool".into()
    }
    fn draw(
        &mut self,
        _: &dear_imgui_rs::Ui,
        _: &mut World,
        _: &mut Vec<WorkbenchAction>,
        _: &bedtime_api::workbench::ShortcutInput,
    ) {
    }
}
fn app(dir: &TempDir) -> App {
    let mut app = App::new();
    headless_host::install(&mut app, &dir.path("config"));
    app.add_plugins(bedtime_plugin_editor::EditorPlugin);
    bedtime_workbench::register(app.world_mut(), "fixture", |registrar| {
        registrar.panel(
            "fixture.tool",
            "Tool",
            |_: In<bedtime_api::workbench::PanelCreation>| -> bedtime_api::workbench::PanelResult {
                Ok(Box::new(Tool))
            },
        );
    })
    .unwrap();
    headless_host::finish(&mut app, dir.root());
    app
}
fn target_fixture(app: &mut App) -> (Entity, Entity, Entity, u32, u32) {
    let first = open_panel(
        app.world_mut(),
        &bedtime_api::PanelTypeId::from(bedtime_plugin_editor::TEXT_PANEL_TYPE),
        PanelInput::None,
        Value::Null,
        None,
        None,
        false,
    )
    .unwrap();
    let last = open_panel(
        app.world_mut(),
        &bedtime_api::PanelTypeId::from(bedtime_plugin_editor::TEXT_PANEL_TYPE),
        PanelInput::None,
        Value::Null,
        None,
        None,
        false,
    )
    .unwrap();
    let singleton = open_panel(
        app.world_mut(),
        &bedtime_api::PanelTypeId::new("fixture.tool"),
        PanelInput::None,
        Value::Null,
        None,
        None,
        false,
    )
    .unwrap();
    let mut layout = bedtime_workbench::workspace::tiling::Layout::default();
    let largest = layout.split(1, 0, 3000, 100).unwrap();
    let last_area = layout.split(1, 1, 5000, 100).unwrap();
    let mut tiling = bedtime_workbench::workspace::tiling_state::TilingState::new(layout);
    for (panel, area) in [(first, 1), (last, last_area), (singleton, 1)] {
        tiling.assign(
            app.world().get::<PanelPresentation>(panel).unwrap().id,
            area,
        );
    }
    app.world_mut().resource_mut::<WorkbenchLayout>().tiling = tiling;
    focus_panel(app.world_mut(), first, true).unwrap();
    focus_panel(app.world_mut(), last, true).unwrap();
    (first, last, singleton, largest, last_area)
}

#[test]
fn last_focused_and_explicit_file_areas_are_captured_before_later_focus_changes() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for target in [PanelTarget::LastFocused, PanelTarget::LargestArea] {
        let dir = TempDir::new();
        let mut app = app(&dir);
        let (first, last, _, largest, last_area) = target_fixture(&mut app);
        let path = dir.write("target.txt", b"target");
        let request = capture_host_request_area(
            app.world(),
            Some(last),
            HostAction::OpenFileIn {
                path: path.to_string_lossy().into_owned(),
                viewer: None,
                target,
            },
            (target == PanelTarget::LargestArea).then_some(largest),
        );
        let expected = match target {
            PanelTarget::LastFocused => last_area,
            PanelTarget::LargestArea => largest,
        };
        assert_eq!(request.area(), expected);
        focus_panel(app.world_mut(), first, true).unwrap();
        let id = submit_captured_host_request(app.world_mut(), request);
        resolve_documents(app.world_mut());
        let panel = match take_host_result(app.world_mut(), id).unwrap().unwrap() {
            HostOutcome::Opened(panel) => panel,
            _ => panic!(),
        };
        assert_eq!(
            app.world()
                .resource::<WorkbenchLayout>()
                .tiling
                .area_for(app.world().get::<PanelPresentation>(panel).unwrap().id),
            Some(expected)
        );
    }
}

#[test]
fn plugin_only_apps_drive_requests_and_keep_revision_errors_typed() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let dir = TempDir::new();
    let mut app = app(&dir);
    let path = dir.write("revision.txt", b"original");
    let request = capture_host_request(
        app.world(),
        None,
        HostAction::OpenFile {
            path: path.to_string_lossy().into_owned(),
            viewer: None,
        },
    );
    let id = submit_captured_host_request(app.world_mut(), request);
    app.update();
    let panel = match take_host_result(app.world_mut(), id).unwrap().unwrap() {
        HostOutcome::Opened(panel) => panel,
        _ => panic!(),
    };
    let document = app
        .world()
        .get::<PanelBinding>(panel)
        .unwrap()
        .document
        .unwrap();
    let revision = app
        .world()
        .non_send::<DocumentSession>()
        .document_revision(document)
        .unwrap();
    let stale = capture_host_request(
        app.world(),
        Some(panel),
        HostAction::ApplyEdits {
            document,
            revision,
            edits: vec![ByteEdit {
                range: 0..1,
                bytes: b"x".to_vec(),
            }],
        },
    );
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .apply_edits(
            document,
            revision,
            &[ByteEdit {
                range: 0..1,
                bytes: b"y".to_vec(),
            }],
        )
        .unwrap();
    let id = submit_captured_host_request(app.world_mut(), stale);
    app.update();
    assert_eq!(
        take_host_result(app.world_mut(), id).unwrap().unwrap_err(),
        "Document changed; refresh before applying edits"
    );
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"yriginal"
    );
}

#[test]
fn a_failed_batch_retains_panels_and_never_applies_later_intentions() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let dir = TempDir::new();
    let mut app = app(&dir);
    let path = dir.write("batch.txt", b"before");
    let panel = open_file_instance(app.world_mut(), &path, None, false, None).unwrap();
    let document = app
        .world()
        .get::<PanelBinding>(panel)
        .unwrap()
        .document
        .unwrap();
    let revision = app
        .world()
        .non_send::<DocumentSession>()
        .document_revision(document)
        .unwrap();
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .apply_edits(
            document,
            revision,
            &[ByteEdit {
                range: 0..6,
                bytes: b"after".to_vec(),
            }],
        )
        .unwrap();
    let effects = [
        HostAction::ApplyEdits {
            document,
            revision,
            edits: vec![],
        },
        HostAction::Save { document },
    ]
    .into_iter()
    .map(|action| capture_host_request(app.world(), Some(panel), action))
    .collect();
    let id = submit_host_batch(app.world_mut(), effects);
    app.update();
    assert_eq!(
        take_host_result(app.world_mut(), id).unwrap().unwrap_err(),
        "Document changed; refresh before applying edits"
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"before");
    assert!(app.world().entities().contains(panel));
}

#[test]
fn generic_and_editor_ingress_use_one_submission_order_without_redirecting_sources() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let dir = TempDir::new();
    let mut app = app(&dir);
    let a = open_file_instance(
        app.world_mut(),
        &dir.write("a.txt", b"a"),
        None,
        false,
        None,
    )
    .unwrap();
    let b = open_file_instance(
        app.world_mut(),
        &dir.write("b.txt", b"b"),
        None,
        false,
        None,
    )
    .unwrap();
    let first = app
        .world_mut()
        .resource_mut::<WorkbenchRequests>()
        .submit(WorkbenchAction::FocusPanel(a));
    let second = app
        .world_mut()
        .resource_mut::<WorkbenchRequests>()
        .submit(WorkbenchAction::FocusPanel(b));
    let third = app
        .world_mut()
        .resource_mut::<WorkbenchRequests>()
        .submit(WorkbenchAction::ObserveFocus(a));
    app.update();
    assert!(
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .take_result(first)
            .unwrap()
            .is_ok()
    );
    assert!(
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .take_result(second)
            .unwrap()
            .is_ok()
    );
    assert!(
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .take_result(third)
            .unwrap()
            .is_ok()
    );
    // A delayed native observation cannot override the newer explicit focus.
    assert_eq!(app.world().resource::<WorkbenchLayout>().focused, Some(b));
}

#[test]
fn saving_a_captured_panel_never_redirects_to_newer_focus() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for from_widget in [false, true] {
        let dir = TempDir::new();
        let mut app = app(&dir);
        let first = dir.root().join("first.txt");
        let second = dir.root().join("second.txt");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();
        let a = open_file_instance(app.world_mut(), &first, None, false, None).unwrap();
        let document = app
            .world()
            .get::<PanelBinding>(a)
            .unwrap()
            .document
            .unwrap();
        let revision = app
            .world()
            .non_send::<DocumentSession>()
            .document_revision(document)
            .unwrap();
        headless_host::host(
            app.world_mut(),
            Some(a),
            HostAction::ApplyEdits {
                document,
                revision,
                edits: vec![ByteEdit {
                    range: 0..5,
                    bytes: b"changed".to_vec(),
                }],
            },
        )
        .unwrap();
        let request = capture_host_request(
            app.world(),
            Some(a),
            if from_widget {
                HostAction::Save { document }
            } else {
                HostAction::Command {
                    command: "bedtime.documents.save".into(),
                    context: bedtime_api::documents::CommandContext {
                        document: Some(document),
                        ..Default::default()
                    },
                }
            },
        );
        let b = open_file_instance(app.world_mut(), &second, None, false, None).unwrap();
        let id = submit_captured_host_request(app.world_mut(), request);
        let result = headless_host::wait_host(app.world_mut(), id);
        assert!(
            matches!(result, Ok(HostOutcome::Saved { .. })),
            "save receipt: {result:?}"
        );
        assert_eq!(std::fs::read(&first).unwrap(), b"changed");
        assert_eq!(std::fs::read(&second).unwrap(), b"second");
        assert_eq!(app.world().resource::<WorkbenchLayout>().focused, Some(b));
    }
}
