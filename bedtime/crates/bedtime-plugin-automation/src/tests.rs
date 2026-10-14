use super::*;
use bedtime_api::workbench::{Panel, PanelCreation, PanelResult};

struct TestPanel;

impl Panel for TestPanel {
    fn title(&self, _: &World) -> String {
        "Fixture".into()
    }
    fn draw(
        &mut self,
        _: &dear_imgui_rs::Ui,
        _: &mut World,
        _: &mut Vec<WorkbenchAction>,
        _shortcuts: &bedtime_api::workbench::ShortcutInput,
    ) {
    }
}

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(bedtime_workbench::WorkbenchPlugin)
        .add_plugins(AutomationPlugin);
    bedtime_api::workbench::register(app.world_mut(), "fixture", |registry| {
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
        .insert(bedtime_api::workbench::PanelSuspended);
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

#[test]
fn disabled_commands_share_menu_availability_and_do_not_execute() {
    #[derive(Resource, Default)]
    struct Calls(u32);
    let mut app = app();
    app.init_resource::<Calls>();
    bedtime_api::workbench::register(app.world_mut(), "commands", |registry| {
        registry.command(
            "commands.run",
            "Run",
            |In(_): In<CommandInvocation>,
             mut calls: ResMut<Calls>|
             -> bedtime_api::workbench::CommandResult {
                calls.0 += 1;
                Ok(Vec::new())
            },
        );
    })
    .unwrap();
    let mut presentations = bedtime_api::workbench::CommandPresentations::default();
    presentations.0.insert(
        "commands.run".into(),
        bedtime_api::workbench::CommandPresentation {
            label: Some("Busy".into()),
            enabled: false,
            visible: true,
            ..Default::default()
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

#[test]
fn contextual_availability_uses_the_resolved_command_source() {
    #[derive(Resource, Default)]
    struct Calls(u32);
    let mut app = app();
    let editor = open(&mut app);
    let tool = open(&mut app);
    app.init_resource::<Calls>();
    bedtime_api::workbench::register(app.world_mut(), "commands", |registry| {
        registry.command(
            "commands.edit",
            "Edit document",
            move |In(input): In<CommandInvocation>,
                  mut calls: ResMut<Calls>|
                  -> bedtime_api::workbench::CommandResult {
                if input.source != Some(editor.entity()) {
                    return Err("No editable document at this source".into());
                }
                calls.0 += 1;
                Ok(Vec::new())
            },
        );
    })
    .unwrap();
    app.world_mut()
        .resource_mut::<bedtime_api::workbench::CommandPresentations>()
        .0
        .insert(
            "commands.edit".into(),
            bedtime_api::workbench::CommandPresentation {
                enabled: false,
                scope: bedtime_api::workbench::CommandPresentationScope::Invocation {
                    source: Some(tool.entity()),
                    arguments: Value::Null,
                },
                ..Default::default()
            },
        );
    let captured_editor = submit(
        &mut app,
        Operation::InvokeCommand {
            command: "commands.edit".into(),
            source: Some(editor),
            arguments: Value::Null,
        },
    );
    let current_tool = submit(
        &mut app,
        Operation::InvokeCommand {
            command: "commands.edit".into(),
            source: Some(tool),
            arguments: Value::Null,
        },
    );
    app.update();
    assert!(result(&mut app, captured_editor).is_ok());
    assert_eq!(
        result(&mut app, current_tool).unwrap_err().kind,
        ErrorKind::Unavailable
    );
    assert_eq!(app.world().resource::<Calls>().0, 1);
    let close = submit(&mut app, Operation::ClosePanel(editor));
    app.update();
    result(&mut app, close).unwrap();
    let stale = submit(
        &mut app,
        Operation::InvokeCommand {
            command: "commands.edit".into(),
            source: Some(editor),
            arguments: Value::Null,
        },
    );
    app.update();
    assert_eq!(
        result(&mut app, stale).unwrap_err().kind,
        ErrorKind::StaleHandle
    );
    assert_eq!(app.world().resource::<Calls>().0, 1);
}

#[cfg(feature = "documents")]
#[test]
fn document_observations_work_without_application_policy_and_reject_retired_handles() {
    let mut first = app();
    let mut session = bedtime_documents::DocumentSession::new();
    session.create_document(b"observed").unwrap();
    first.insert_non_send(session);
    let inspect = submit(&mut first, Operation::Inspect);
    first.update();
    let OperationValue::Snapshot(snapshot) = result(&mut first, inspect).unwrap() else {
        panic!("Document snapshot")
    };
    let document = snapshot.documents[0].handle;
    let read = submit(&mut first, Operation::ReadDocument(document));
    first.update();
    let OperationValue::Document(read) = result(&mut first, read).unwrap() else {
        panic!("Document read receipt")
    };
    assert_eq!(read.bytes, b"observed");
    first.insert_non_send(bedtime_documents::DocumentSession::new());
    let stale = submit(&mut first, Operation::ReadDocument(document));
    first.update();
    assert_eq!(
        result(&mut first, stale).unwrap_err().kind,
        ErrorKind::StaleHandle
    );
    let mut other = app();
    other.insert_non_send(bedtime_documents::DocumentSession::new());
    let cross_app = submit(&mut other, Operation::ReadDocument(document));
    other.update();
    assert_eq!(
        result(&mut other, cross_app).unwrap_err().kind,
        ErrorKind::StaleHandle
    );
}

#[test]
fn readiness_uses_the_registered_frontend_without_application_policy() {
    let mut app = app();
    let panel = open(&mut app);
    let frontend = app
        .world()
        .get::<bedtime_api::workbench::PanelFrontend>(panel.entity())
        .map_or(panel.entity(), |frontend| frontend.0);
    app.world_mut()
        .entity_mut(frontend)
        .insert(bedtime_api::workbench::PanelReadiness::Pending);
    let inspect = submit(&mut app, Operation::Inspect);
    app.update();
    let OperationValue::Snapshot(snapshot) = result(&mut app, inspect).unwrap() else {
        panic!("Snapshot")
    };
    assert_eq!(snapshot.panels[0].readiness, Readiness::Pending);
}
