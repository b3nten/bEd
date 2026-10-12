#[cfg(feature = "editor")]
#[path = "../../../tests/support/editor_host.rs"]
mod editor_host;
use super::*;
use bed_workbench_api::{Panel, PanelCreation, PanelResult};

struct TestPanel;
impl Panel for TestPanel {
    fn title(&self, _: &World) -> String {
        "Fixture".into()
    }
    fn draw(
        &mut self,
        _: &dear_imgui_rs::Ui,
        _: &mut World,
        _: &mut Vec<Action>,
        _shortcuts: &bed_workbench_api::ShortcutInput,
    ) {
    }
}
fn app() -> App {
    let mut app = App::new();
    app.add_plugins(bed_workbench::WorkbenchPlugin)
        .add_plugins(AutomationPlugin);
    bed_workbench_api::register(app.world_mut(), "fixture", |registry| {
        registry.panel(
            "fixture.panel",
            "Fixture",
            |In(_): In<PanelCreation>| -> PanelResult { Ok(Box::new(TestPanel)) },
        );
    })
    .unwrap();
    app
}
fn submit(app: &mut App, operation: Operation) -> OperationId {
    app.world_mut()
        .resource_mut::<AutomationRequests>()
        .submit(operation)
}
fn result(app: &mut App, id: OperationId) -> OperationResult {
    app.world_mut()
        .resource_mut::<AutomationResults>()
        .take(id)
        .expect("operation complete")
}
fn open(app: &mut App) -> PanelHandle {
    let id = submit(
        app,
        Operation::OpenPanel {
            panel_type: "fixture.panel".into(),
            state: Value::Null,
            area: None,
        },
    );
    assert_eq!(
        app.world().resource::<AutomationResults>().status(id),
        Some(OperationStatus::Pending)
    );
    app.update();
    match result(app, id).unwrap() {
        OperationValue::Opened(panel) => panel,
        _ => panic!("open receipt"),
    }
}
#[test]
fn requests_have_actual_receipts_and_snapshots_are_owned() {
    let mut app = app();
    let panel = open(&mut app);
    let id = submit(&mut app, Operation::Inspect);
    app.update();
    let OperationValue::Snapshot(snapshot) = result(&mut app, id).unwrap() else {
        panic!("snapshot")
    };
    assert_eq!(snapshot.panels.len(), 1);
    assert_eq!(snapshot.panels[0].handle, panel);
    let close = submit(&mut app, Operation::ClosePanel(panel));
    app.update();
    assert!(matches!(
        result(&mut app, close),
        Ok(OperationValue::Closed(_))
    ));
    assert_eq!(snapshot.panels[0].title, "Fixture");
    let stale = submit(&mut app, Operation::FocusPanel(panel));
    app.update();
    assert_eq!(
        result(&mut app, stale).unwrap_err().kind,
        ErrorKind::StaleHandle
    );
}
#[test]
fn handles_cannot_cross_app_instances() {
    let mut first = app();
    let panel = open(&mut first);
    let mut second = app();
    open(&mut second);
    let id = submit(&mut second, Operation::FocusPanel(panel));
    second.update();
    assert_eq!(
        result(&mut second, id).unwrap_err().kind,
        ErrorKind::StaleHandle
    );
}

#[test]
fn snapshots_describe_the_active_layout_while_suspended_panels_remain_alive() {
    let mut app = app();
    let suspended = open(&mut app);
    let active = open(&mut app);
    let id = app
        .world()
        .get::<PanelPresentation>(suspended.entity())
        .unwrap()
        .id;
    app.world_mut()
        .entity_mut(suspended.entity())
        .insert(bed_workbench_api::PanelSuspended);
    app.world_mut()
        .resource_mut::<WorkbenchLayout>()
        .tiling
        .remove(id);
    let inspect = submit(&mut app, Operation::Inspect);
    app.update();
    let OperationValue::Snapshot(snapshot) = result(&mut app, inspect).unwrap() else {
        panic!("snapshot")
    };
    assert_eq!(snapshot.panels.len(), 1);
    assert_eq!(snapshot.panels[0].handle, active);
    assert!(app.world().entities().contains(suspended.entity()));
}
#[test]
fn missing_desktop_and_failed_dependencies_complete_as_errors() {
    let mut app = app();
    let input = submit(&mut app, Operation::Input(Vec::new()));
    let capture = submit(
        &mut app,
        Operation::Capture {
            targets: vec![CaptureTarget::Window],
            after: Some(input),
            timeout: Duration::from_secs(1),
        },
    );
    app.update();
    assert_eq!(
        result(&mut app, input).unwrap_err().kind,
        ErrorKind::Unavailable
    );
    assert_eq!(
        result(&mut app, capture).unwrap_err().kind,
        ErrorKind::DependencyFailed
    );
    let expired = submit(
        &mut app,
        Operation::Capture {
            targets: vec![CaptureTarget::Window],
            after: None,
            timeout: Duration::ZERO,
        },
    );
    app.update();
    assert_eq!(
        result(&mut app, expired).unwrap_err().kind,
        ErrorKind::DeadlineExceeded
    );
}

#[test]
fn exit_completes_waiting_operations_instead_of_leaving_pending_status() {
    let mut app = app();
    app.add_message::<AppExit>();
    app.init_resource::<DesktopCaptureRequests>()
        .init_resource::<DesktopCaptureResults>()
        .init_resource::<DesktopFrameInfo>();
    let id = submit(
        &mut app,
        Operation::Capture {
            targets: vec![CaptureTarget::Window],
            after: None,
            timeout: Duration::from_secs(30),
        },
    );
    app.world_mut().write_message(AppExit::Success);
    app.update();
    assert_eq!(
        app.world().resource::<AutomationResults>().status(id),
        Some(OperationStatus::Complete)
    );
    assert_eq!(result(&mut app, id).unwrap_err().kind, ErrorKind::Failed);
}

#[cfg(feature = "editor")]
#[test]
fn disabled_commands_share_menu_availability_and_do_not_execute() {
    #[derive(Resource, Default)]
    struct Calls(u32);
    let mut app = app();
    app.init_resource::<Calls>();
    bed_workbench_api::register(app.world_mut(), "commands", |registry| {
        registry.command(
            "commands.run",
            "Run",
            |In(_): In<CommandInvocation>,
             mut calls: ResMut<Calls>|
             -> bed_workbench_api::CommandResult {
                calls.0 += 1;
                Ok(Vec::new())
            },
        );
    })
    .unwrap();
    let mut presentations = bed_editor_api::CommandPresentations::default();
    presentations.0.insert(
        "commands.run".into(),
        bed_editor_api::CommandPresentation {
            label: Some("Busy".into()),
            enabled: false,
            visible: true,
        },
    );
    app.insert_resource(presentations);
    let inspect = submit(&mut app, Operation::Inspect);
    let invoke = submit(
        &mut app,
        Operation::InvokeCommand {
            command: "commands.run".into(),
            source: None,
            arguments: Value::Null,
        },
    );
    app.update();
    assert_eq!(
        result(&mut app, invoke).unwrap_err().kind,
        ErrorKind::Unavailable
    );
    assert_eq!(app.world().resource::<Calls>().0, 0);
    let OperationValue::Snapshot(snapshot) = result(&mut app, inspect).unwrap() else {
        panic!("snapshot")
    };
    let command = snapshot
        .commands
        .iter()
        .find(|command| command.id == "commands.run")
        .unwrap();
    assert_eq!(command.label, "Busy");
    assert!(!command.enabled);
}

#[cfg(feature = "editor")]
mod documents {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };
    struct Fixture {
        app: App,
        root: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "bed-automation-doc-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            let mut app = App::new();
            editor_host::install(
                &mut app,
                bed_config::Settings::with_paths(
                    root.join("config"),
                    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
                )
                .unwrap(),
            )
            .unwrap();
            app.add_plugins(bed_editor_workbench::EditorWorkbenchPlugin {
                options: bed_editor_workbench::EditorWorkbenchOptions {
                    working_directory: Some(root.clone()),
                    ..default()
                },
            })
            .add_plugins(bed_editor::EditorPlugin)
            .add_plugins(AutomationPlugin);
            app.update();
            Self { app, root }
        }
        fn run(&mut self, operation: EditorOperation) -> OperationResult {
            let id = submit(&mut self.app, Operation::Editor(operation));
            self.app.update();
            result(&mut self.app, id)
        }
        fn inspect(&mut self) -> Snapshot {
            let id = submit(&mut self.app, Operation::Inspect);
            self.app.update();
            let OperationValue::Snapshot(snapshot) = result(&mut self.app, id).unwrap() else {
                panic!("snapshot")
            };
            snapshot
        }
        fn open(&mut self) -> DocumentInfo {
            let path = self.root.join("document.txt");
            fs::write(&path, b"original").unwrap();
            self.run(EditorOperation::OpenFile {
                path,
                viewer: None,
                additional: false,
            })
            .unwrap();
            self.inspect().documents.remove(0)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    #[test]
    fn edits_undo_and_save_complete_through_the_same_editor_queue() {
        let mut fixture = Fixture::new();
        let document = fixture.open();
        let snapshot = fixture.inspect();
        let panel = snapshot
            .panels
            .iter()
            .find(|panel| panel.panel_type == bed_editor::TEXT_PANEL_TYPE)
            .expect("text panel");
        assert!(panel.title.contains("document.txt"));
        assert_eq!(panel.readiness, Readiness::Ready);
        fixture
            .run(EditorOperation::ApplyEdits {
                document: document.handle,
                revision: document.revision,
                edits: vec![bed_document_session::editor_session::ByteEdit {
                    range: 0..8,
                    bytes: b"changed".to_vec(),
                }],
            })
            .unwrap();
        let OperationValue::Document(read) = fixture
            .run(EditorOperation::ReadDocument(document.handle))
            .unwrap()
        else {
            panic!("read")
        };
        assert_eq!(read.bytes, b"changed");
        assert!(read.info.dirty);
        let OperationValue::Saved(saved) = fixture
            .run(EditorOperation::Save(vec![document.handle]))
            .unwrap()
        else {
            panic!("actual save receipt")
        };
        assert_eq!(saved.len(), 1);
        assert_eq!(fs::read(&document.path).unwrap(), b"changed");
        fixture.run(EditorOperation::Undo(document.handle)).unwrap();
        let OperationValue::Document(read) = fixture
            .run(EditorOperation::ReadDocument(document.handle))
            .unwrap()
        else {
            panic!("read")
        };
        assert_eq!(read.bytes, b"original");
        assert!(read.info.dirty);
    }
    #[test]
    fn stale_edits_fail_without_mutating_the_current_document() {
        let mut fixture = Fixture::new();
        let document = fixture.open();
        fixture
            .run(EditorOperation::ApplyEdits {
                document: document.handle,
                revision: document.revision,
                edits: vec![bed_document_session::editor_session::ByteEdit {
                    range: 0..8,
                    bytes: b"new".to_vec(),
                }],
            })
            .unwrap();
        let error = fixture
            .run(EditorOperation::ApplyEdits {
                document: document.handle,
                revision: document.revision,
                edits: vec![bed_document_session::editor_session::ByteEdit {
                    range: 0..3,
                    bytes: b"stale".to_vec(),
                }],
            })
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::RevisionConflict);
        let OperationValue::Document(read) = fixture
            .run(EditorOperation::ReadDocument(document.handle))
            .unwrap()
        else {
            panic!("read")
        };
        assert_eq!(read.bytes, b"new");
    }
    #[test]
    fn attachment_preserves_handles_and_replacement_retires_them() {
        let mut fixture = Fixture::new();
        let document = fixture.open();
        fixture
            .run(EditorOperation::AttachWorkspace(fixture.root.clone()))
            .unwrap();
        assert!(
            fixture
                .run(EditorOperation::ReadDocument(document.handle))
                .is_ok()
        );
        let replacement = fixture.root.join("other");
        fs::create_dir(&replacement).unwrap();
        fixture
            .run(EditorOperation::SetWorkspace(
                bed_editor_api::workspace::WorkspaceSpec::local(replacement.to_str().unwrap()),
            ))
            .unwrap();
        assert_eq!(
            fixture
                .run(EditorOperation::ReadDocument(document.handle))
                .unwrap_err()
                .kind,
            ErrorKind::StaleHandle
        );
    }
}
