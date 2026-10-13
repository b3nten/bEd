use super::*;
use bed_api::features::HostRequest;

fn capture(world: &World, source: Option<Entity>, request: HostAction) -> HostRequest {
    let layout = world.resource::<WorkbenchLayout>();
    let area = source
        .and_then(|source| bed_api::workbench::panel_owner(world, source))
        .and_then(|owner| world.get::<PanelPresentation>(owner))
        .and_then(|panel| layout.tiling.area_for(panel.id))
        .unwrap_or_else(|| layout.tiling.layout.largest_area());
    HostRequest::capture(
        world.non_send::<DocumentSession>().session_id(),
        source,
        request,
        area,
        world.resource::<bed_workbench::WorkbenchLayoutEpoch>().0,
    )
}

#[test]
fn emitted_document_requests_interleave_with_panel_commands_before_admission() {
    #[derive(Resource, Default)]
    struct Observed(Vec<Vec<u8>>);
    let dir = TempDir::new();
    let mut app = app(&dir);
    let panel = open_file(
        app.world_mut(),
        &dir.write("origin.txt", b"before"),
        None,
        false,
        None,
    )
    .unwrap();
    let source = panel_presentation(app.world(), panel).unwrap();
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
    app.init_resource::<Observed>();
    bed_api::workbench::register(app.world_mut(), "ordering", |registrar| {
        registrar.command(
            "ordering.observe",
            "Observe",
            move |_: In<CommandInvocation>,
                  session: NonSend<DocumentSession>,
                  mut observed: ResMut<Observed>| {
                observed.0.push(
                    session
                        .snapshot(document)
                        .map_err(|error| error.to_string())?
                        .bytes,
                );
                Ok(Vec::<WorkbenchAction>::new())
            },
        )
    })
    .unwrap();
    let first = capture(
        app.world(),
        Some(source),
        HostAction::ApplyEdits {
            document,
            revision,
            edits: vec![ByteEdit {
                range: 0..6,
                bytes: b"after".to_vec(),
            }],
        },
    );
    app.world_mut().write_message(first);
    let invocation = bed_workbench::capture_command(app.world(), "ordering.observe", Value::Null);
    app.world_mut()
        .resource_mut::<WorkbenchRequests>()
        .push(WorkbenchAction::Invoke(invocation));
    let second = capture(app.world(), Some(source), HostAction::Undo { document });
    app.world_mut().write_message(second);
    let other = open_file(
        app.world_mut(),
        &dir.write("other.txt", b"untouched"),
        None,
        false,
        None,
    )
    .unwrap();
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().focused,
        Some(other)
    );
    process_app_requests(app.world_mut()).unwrap();
    assert_eq!(
        app.world().resource::<Observed>().0,
        vec![b"after".to_vec()]
    );
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"before"
    );
    let other_document = app
        .world()
        .get::<PanelBinding>(other)
        .unwrap()
        .document
        .unwrap();
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .snapshot(other_document)
            .unwrap()
            .bytes,
        b"untouched"
    );
}

#[test]
fn delayed_document_messages_keep_the_original_session_and_retired_source() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    let stale_session = capture(
        app.world(),
        None,
        HostAction::OpenPanel {
            panel_type: "bed.color.panel".into(),
            document: None,
            state: Value::Null,
        },
    );
    let original_session = stale_session.session_id();
    let original_sequence = stale_session.sequence();
    app.world_mut().insert_non_send(DocumentSession::new());
    assert_ne!(
        original_session,
        app.world().non_send::<DocumentSession>().session_id()
    );
    app.world_mut().write_message(stale_session);
    drain_feature_intents(app.world_mut());
    let queued = app
        .world()
        .resource::<AppRequests>()
        .pending
        .front()
        .unwrap();
    assert_eq!(queued.request.session_id, original_session.0);
    assert_eq!(queued.sequence, original_sequence);
    process_app_requests(app.world_mut()).unwrap();
    assert!(!panel_entities(app.world_mut()).into_iter().any(|panel| {
        app.world().get::<PanelRecord>(panel).unwrap().panel_type == "bed.color.panel"
    }));

    let panel = open_file(
        app.world_mut(),
        &dir.write("retired.txt", b"clean"),
        None,
        false,
        None,
    )
    .unwrap();
    let source = panel_presentation(app.world(), panel).unwrap();
    let stale_source = capture(
        app.world(),
        Some(source),
        HostAction::OpenPanel {
            panel_type: "bed.color.panel".into(),
            document: None,
            state: Value::Null,
        },
    );
    retire_panels(app.world_mut(), &[panel]).unwrap();
    let replacement = open_file(
        app.world_mut(),
        &dir.write("replacement.txt", b"replacement"),
        None,
        false,
        None,
    )
    .unwrap();
    app.world_mut().write_message(stale_source);
    process_app_requests(app.world_mut()).unwrap();
    assert!(app.world().entities().contains(replacement));
    assert!(!panel_entities(app.world_mut()).into_iter().any(|panel| {
        app.world().get::<PanelRecord>(panel).unwrap().panel_type == "bed.color.panel"
    }));
}

#[test]
fn returned_layout_rejects_old_close_messages_and_scoped_panel_commands() {
    #[derive(Resource, Default)]
    struct AppCommands(usize);
    let dir = TempDir::new();
    let mut app = app(&dir);
    let panel = open_panel(
        app.world_mut(),
        "bed.color.panel",
        PanelInput::None,
        Value::Null,
        None,
        None,
        false,
    )
    .unwrap();
    let stale = capture(
        app.world(),
        None,
        HostAction::ClosePanels {
            panel_type: "bed.color.panel".into(),
        },
    );
    let tracked = app
        .world_mut()
        .resource_mut::<WorkbenchRequests>()
        .submit(WorkbenchAction::ClosePanel(panel));
    app.init_resource::<AppCommands>();
    bed_api::workbench::register(app.world_mut(), "app-command", |registrar| {
        registrar.command(
            "app-command.count",
            "Count",
            |_: In<CommandInvocation>, mut commands: ResMut<AppCommands>| {
                commands.0 += 1;
                Ok(Vec::<WorkbenchAction>::new())
            },
        )
    })
    .unwrap();
    let invocation = bed_workbench::capture_command(app.world(), "app-command.count", Value::Null);
    let unscoped = app
        .world_mut()
        .resource_mut::<WorkbenchRequests>()
        .submit_unscoped(WorkbenchAction::Invoke(invocation));
    let slot = app.world().resource::<LayoutSlots>().active();
    layouts::request_switch(app.world_mut(), LayoutSlot::Layout4, None).unwrap();
    layouts::request_switch(app.world_mut(), slot, None).unwrap();
    app.world_mut().write_message(stale);
    process_app_requests(app.world_mut()).unwrap();
    assert!(app.world().entities().contains(panel));
    assert_eq!(
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .take_result(tracked),
        Some(Ok(bed_workbench::WorkbenchOutcome::Ignored))
    );
    assert_eq!(
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .take_result(unscoped),
        Some(Ok(bed_workbench::WorkbenchOutcome::Applied))
    );
    assert_eq!(app.world().resource::<AppCommands>().0, 1);
}

#[test]
fn document_history_requests_remain_valid_after_an_earlier_layout_switch() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    let panel = open_file(
        app.world_mut(),
        &dir.write("history.txt", b"before"),
        None,
        false,
        None,
    )
    .unwrap();
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
    let source = panel_presentation(app.world(), panel).unwrap();
    let switch = submit(
        app.world_mut(),
        AppAction::Command(WindowCommand::UseLayout(LayoutSlot::Layout4)),
        None,
    );
    let undo = capture(app.world(), Some(source), HostAction::Undo { document });
    app.world_mut().write_message(undo);
    process_app_requests(app.world_mut()).unwrap();
    assert!(matches!(
        poll_app_request_result(app.world_mut(), switch),
        Some(Ok(AppRequestOutcome::Applied))
    ));
    assert!(!bed_api::workbench::panel_active(app.world(), panel));
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"before"
    );
}
