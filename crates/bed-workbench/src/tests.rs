use super::*;
use api::{CommandResult, PanelCreation};
use std::{cell::Cell, rc::Rc, sync::Mutex};
use workspace::tiling::Layout;

pub(super) static IMGUI_TEST_LOCK: Mutex<()> = Mutex::new(());

fn imgui_context() -> dear_imgui_rs::Context {
    let mut context = dear_imgui_rs::Context::create();
    context
        .io_mut()
        .set_config_flags(dear_imgui_rs::ConfigFlags::DOCKING_ENABLE);
    context
        .set_ini_filename::<std::path::PathBuf>(None)
        .unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    context
}

struct CounterPanel(Entity);
impl Panel for CounterPanel {
    fn title(&self, _world: &World) -> String {
        "Counter".into()
    }
    fn draw(
        &mut self,
        ui: &Ui,
        _: &mut World,
        requests: &mut Vec<Action>,
        _shortcuts: &bed_workbench_api::ShortcutInput,
    ) {
        if ui.button("Close") || _shortcuts.take(ui, "test.close") {
            requests.push(Action::ClosePanel(self.0));
        }
    }
    fn save(&self, _world: &World) -> Value {
        json!({"count":7})
    }
}
fn factory(In(input): In<PanelCreation>) -> PanelResult {
    Ok(Box::new(CounterPanel(input.entity)))
}
fn open(In(input): In<CommandInvocation>) -> CommandResult {
    Ok(vec![Action::OpenPanel {
        panel_type: "test.counter".into(),
        state: Value::Null,
        area: Some(input.area),
    }])
}
fn app() -> App {
    let mut app = App::new();
    app.add_plugins(WorkbenchPlugin);
    register(app.world_mut(), "test", |registrar| {
        registrar.panel("test.counter", "Counter", factory);
        registrar.command("test.open", "Open Counter", open);
        registrar.panel_shortcut("test.close", "Close Counter", "test.counter", &[]);
        registrar.menu("test.open");
    })
    .unwrap();
    app
}
fn invoke(app: &mut App, command: &str) {
    let request = capture_command(app.world(), command, Value::Null);
    app.world_mut()
        .resource_mut::<WorkbenchRequests>()
        .push(Action::Invoke(request));
}

#[test]
fn transient_panels_are_omitted_from_persisted_content_focus_and_layout() {
    let mut app = app();
    invoke(&mut app, "test.open");
    invoke(&mut app, "test.open");
    process_requests(app.world_mut());
    let transient = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    app.world_mut()
        .entity_mut(transient)
        .insert(api::PanelPersistence(false));
    let value = save(app.world_mut());
    assert_eq!(value["panels"].as_array().unwrap().len(), 1);
    assert!(value["focused"].is_null());
    let persisted = value["panels"][0]["id"].as_u64().unwrap();
    let tiling = TilingState::from_value(&value["tiling"], [persisted]).unwrap();
    assert!(tiling.area_for(persisted).is_some());
    let transient_id = app.world().get::<PanelPresentation>(transient).unwrap().id;
    assert!(tiling.area_for(transient_id).is_none());
    assert!(
        app.world().get::<PanelRecord>(transient).is_some(),
        "saving leaves the live launcher open"
    );
}

#[derive(Component)]
struct PresentationDraft(u32);

#[test]
fn retained_presentation_commands_keep_their_source_and_close_the_owner_after_preflight() {
    let mut app = app();
    invoke(&mut app, "test.open");
    process_requests(app.world_mut());
    let owner = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    let child = app
        .world_mut()
        .spawn((
            api::PanelOwner(owner),
            PresentationDraft(7),
            PanelRecord {
                panel_type: "test.counter".into(),
                serialized: json!({"state":{"count":7}}),
                unavailable: None,
            },
        ))
        .id();
    install_panel_frontend(app.world_mut(), child, Box::new(CounterPanel(child))).unwrap();
    register(app.world_mut(), "presentation", |registrar| {
        registrar.command(
            "presentation.edit",
            "Edit presentation",
            |In(invocation): In<CommandInvocation>, world: &mut World| -> CommandResult {
                let source = invocation.source.unwrap();
                world.get_mut::<PresentationDraft>(source).unwrap().0 += 1;
                Ok(vec![Action::FocusPanel(source)])
            },
        );
    })
    .unwrap();
    app.world_mut()
        .resource_mut::<WorkbenchRequests>()
        .push(Action::Invoke(CommandInvocation {
            command: "presentation.edit".into(),
            source: Some(child),
            area: 1,
            arguments: Value::Null,
        }));
    process_requests(app.world_mut());
    assert_eq!(app.world().get::<PresentationDraft>(child).unwrap().0, 8);
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().focused,
        Some(owner)
    );
    assert_eq!(
        save(app.world_mut())["panels"].as_array().unwrap().len(),
        1,
        "retained content is not an additional layout panel"
    );
    app.insert_resource(WorkbenchClosePolicy(|world, entity| {
        assert!(
            world.get::<PanelPresentation>(entity).is_some(),
            "close preflight receives the owner"
        );
        Ok(false)
    }));
    app.world_mut()
        .resource_mut::<WorkbenchRequests>()
        .push(Action::ClosePanel(child));
    process_requests(app.world_mut());
    assert!(app.world().get::<PresentationDraft>(child).is_some());
    assert!(commit_close(app.world_mut(), owner));
    assert!(app.world().get_entity(child).is_err());
    assert!(app.world().get_entity(owner).is_err());
    assert!(take_panel_frontend(app.world_mut(), child).is_none());
}

#[test]
fn registration_failure_publishes_neither_definitions_nor_system_entities() {
    let mut app = app();
    let before = app.world_mut().query::<Entity>().iter(app.world()).count();
    let error = register(app.world_mut(), "other", |registrar| {
        registrar.command("other.valid", "Valid", open);
        registrar.panel("test.invalid_namespace", "Invalid", factory);
    })
    .unwrap_err();
    assert!(error.contains("Invalid or duplicate contribution"));
    assert!(
        app.world()
            .resource::<Registry>()
            .command("other.valid")
            .is_none()
    );
    assert_eq!(
        app.world_mut().query::<Entity>().iter(app.world()).count(),
        before
    );
    register(app.world_mut(), "other", |registrar| {
        registrar.command("other.valid", "Valid", open)
    })
    .unwrap();
    assert_eq!(
        app.world()
            .resource::<Registry>()
            .commands()
            .iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>(),
        ["test.open", "other.valid"]
    );
}

#[derive(Resource, Default)]
struct Observed(bool);
fn observe(_: In<CommandInvocation>, world: &mut World) -> CommandResult {
    let focused = world.resource::<WorkbenchLayout>().focused.unwrap();
    world.resource_mut::<Observed>().0 = world.get::<PanelRecord>(focused).is_some();
    Ok(Vec::new())
}
#[test]
fn command_effects_commit_before_the_next_fifo_command() {
    let mut app = app();
    app.init_resource::<Observed>();
    register(app.world_mut(), "observer", |registrar| {
        registrar.command("observer.check", "Check", observe)
    })
    .unwrap();
    invoke(&mut app, "test.open");
    invoke(&mut app, "observer.check");
    process_requests(app.world_mut());
    assert!(app.world().resource::<Observed>().0);
    assert!(
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .take_errors()
            .is_empty()
    );
}

#[test]
fn unavailable_feature_roundtrip_preserves_opaque_state_and_restores_when_installed() {
    let mut app = app();
    let mut layout = TilingState::new(Layout::default());
    layout.assign(41, 1);
    let saved = json!({"version":1,"panels":[{
        "id":41,"panel_type":"omitted.counter","state":{"count":12},
        "viewer":"omitted.format","input":{"path":"sample.bin"},"future_field":[1,2,3]
    }],"focused":41,"focused_area":1,"tiling":layout.to_value([41])});
    restore(app.world_mut(), &saved).unwrap();
    assert_eq!(save(app.world_mut()), saved);
    let entity = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    assert!(
        app.world()
            .get::<PanelRecord>(entity)
            .unwrap()
            .unavailable
            .is_some()
    );
    register(app.world_mut(), "omitted", |registrar| {
        registrar.panel("omitted.counter", "Counter", factory)
    })
    .unwrap();
    restore(app.world_mut(), &saved).unwrap();
    let restored = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    assert_ne!(restored, entity);
    assert!(app.world().get_entity(entity).is_err());
    assert!(
        app.world()
            .get::<PanelRecord>(restored)
            .unwrap()
            .unavailable
            .is_none()
    );
    assert_eq!(
        save(app.world_mut())["panels"][0]["future_field"],
        json!([1, 2, 3])
    );
}

#[test]
fn captured_command_does_not_target_an_entity_replacing_its_saved_id() {
    let mut app = app();
    invoke(&mut app, "test.open");
    process_requests(app.world_mut());
    let old = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    let stale = capture_command(app.world(), "test.open", Value::Null);
    let saved = save(app.world_mut());
    restore(app.world_mut(), &saved).unwrap();
    let replacement = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    assert_ne!(old, replacement);
    app.world_mut()
        .resource_mut::<WorkbenchRequests>()
        .push(Action::Invoke(stale));
    process_requests(app.world_mut());
    assert_eq!(
        app.world_mut()
            .query::<&PanelRecord>()
            .iter(app.world())
            .count(),
        1
    );
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().focused,
        Some(replacement)
    );
}

#[test]
fn invalid_restore_preserves_live_entities_state_and_layout() {
    let mut app = app();
    invoke(&mut app, "test.open");
    process_requests(app.world_mut());
    let entity = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    let saved = save(app.world_mut());
    for field in ["focused", "tiling"] {
        let mut invalid = saved.clone();
        invalid[field] = json!(999);
        assert!(restore(app.world_mut(), &invalid).is_err());
        assert!(app.world().get::<PanelRecord>(entity).is_some());
        assert_eq!(save(app.world_mut()), saved);
    }
}

#[test]
fn restored_selection_requests_only_the_saved_tab_and_exhausted_ids_fail_cleanly() {
    let mut app = app();
    let mut tiling = TilingState::default();
    tiling.assign(1, 1);
    tiling.assign(u32::MAX as u64, 1);
    let saved = json!({"version":1,"panels":[
        {"id":1,"panel_type":"test.counter","state":{}},
        {"id":u32::MAX,"panel_type":"test.counter","state":{}}
    ],"focused":1,"tiling":tiling.to_value([1,u32::MAX as u64])});
    restore(app.world_mut(), &saved).unwrap();
    let focus_flags = app
        .world_mut()
        .query::<&PanelPresentation>()
        .iter(app.world())
        .map(|panel| (panel.id, panel.focus))
        .collect::<HashMap<_, _>>();
    assert!(focus_flags[&1]);
    assert!(!focus_flags[&(u32::MAX as u64)]);
    invoke(&mut app, "test.open");
    process_requests(app.world_mut());
    assert_eq!(
        app.world_mut()
            .query::<&PanelRecord>()
            .iter(app.world())
            .count(),
        2
    );
    assert_eq!(
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .take_errors(),
        ["Panel IDs exhausted"]
    );
}

#[test]
fn requested_focus_survives_frontend_observations_across_tiled_areas() {
    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
    let mut app = app();
    let mut tiling = TilingState::default();
    tiling.assign(1, 1);
    tiling.assign(2, 1);
    tiling.split_tab(2, 1, 0, true, [100, 100]);
    restore(
        app.world_mut(),
        &json!({"version":1,"panels":[
        {"id":1,"panel_type":"test.counter","state":{}},
        {"id":2,"panel_type":"test.counter","state":{}}
    ],"focused":1,"tiling":tiling.to_value([1,2])}),
    )
    .unwrap();
    let entities = app
        .world_mut()
        .query::<(Entity, &PanelPresentation)>()
        .iter(app.world())
        .map(|(entity, panel)| (panel.id, entity))
        .collect::<HashMap<_, _>>();
    let mut context = imgui_context();
    for id in [1, 2, 1] {
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .push(Action::FocusPanel(entities[&id]));
        process_requests(app.world_mut());
        for _ in 0..3 {
            context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
                [800.0, 600.0],
                1.0 / 60.0,
            ));
            render_ui(app.world_mut(), context.frame());
            drop(context.render_legacy());
            process_requests(app.world_mut());
            assert_eq!(
                app.world().resource::<WorkbenchLayout>().focused,
                Some(entities[&id])
            );
        }
    }
}

#[test]
fn foreign_presentation_targets_cannot_mutate_generic_panels_with_the_same_saved_id() {
    let mut app = app();
    invoke(&mut app, "test.open");
    process_requests(app.world_mut());
    let owned = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    let id = app.world().get::<PanelPresentation>(owned).unwrap().id;
    // The editor uses this shared presentation component, but owns its panel
    // lifecycle separately. Its saved IDs can legitimately overlap ours.
    let mut foreign_presentation = PanelPresentation::new(id);
    foreign_presentation.focus = false;
    let foreign = app.world_mut().spawn(foreign_presentation.clone()).id();
    let before = app.world().resource::<WorkbenchLayout>().clone();
    let mut requests = app.world_mut().resource_mut::<WorkbenchRequests>();
    requests.push(Action::FocusPanel(foreign));
    requests.push(Action::SplitPanel {
        panel: foreign,
        axis: 0,
        high: true,
    });
    requests.push(Action::ClosePanel(foreign));
    process_requests(app.world_mut());
    assert_eq!(app.world().resource::<WorkbenchLayout>(), &before);
    assert_eq!(
        app.world().get::<PanelPresentation>(foreign),
        Some(&foreign_presentation)
    );
    assert!(app.world().get::<PanelRecord>(owned).is_some());

    // Registered editor commands deliberately accept sources in the shared
    // world even though generic panel lifecycle actions do not own them.
    app.world_mut()
        .resource_mut::<WorkbenchRequests>()
        .push(Action::Invoke(CommandInvocation {
            command: "test.open".into(),
            source: Some(foreign),
            area: 1,
            arguments: Value::Null,
        }));
    process_requests(app.world_mut());
    assert_eq!(
        app.world_mut()
            .query::<&PanelRecord>()
            .iter(app.world())
            .count(),
        2
    );
    assert_eq!(
        app.world().get::<PanelPresentation>(foreign),
        Some(&foreign_presentation)
    );
    assert!(
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .take_errors()
            .is_empty()
    );
}

#[derive(Component)]
struct WidgetState(i64);
struct MutableWidget(Entity);
impl Panel for MutableWidget {
    fn title(&self, world: &World) -> String {
        format!("Value {}", world.get::<WidgetState>(self.0).unwrap().0)
    }
    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        requests: &mut Vec<Action>,
        _shortcuts: &bed_workbench_api::ShortcutInput,
    ) {
        world.get_mut::<WidgetState>(self.0).unwrap().0 += 1;
        ui.text(self.title(world));
        requests.push(Action::ClosePanel(self.0));
    }
    fn save(&self, world: &World) -> Value {
        json!({"value":world.get::<WidgetState>(self.0).unwrap().0})
    }
}

#[test]
fn application_compositor_updates_world_widget_state_and_returns_deferred_intentions() {
    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
    let mut app = app();
    invoke(&mut app, "test.open");
    process_requests(app.world_mut());
    let entity = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    app.world_mut().entity_mut(entity).insert(WidgetState(41));
    let previous = install_panel_frontend(app.world_mut(), entity, Box::new(MutableWidget(entity)))
        .unwrap()
        .unwrap();
    assert_eq!(previous.title(app.world()), "Counter");

    let mut context = imgui_context();
    context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
        [800.0, 600.0],
        1.0 / 60.0,
    ));
    let ui = context.frame();
    let mut actions = Vec::new();
    ui.window("application compositor").build(|| {
        actions = render_panel(app.world_mut(), entity, ui).unwrap();
    });
    drop(context.render_legacy());
    assert_eq!(app.world().get::<WidgetState>(entity).unwrap().0, 42);
    assert_eq!(panel_title(app.world_mut(), entity).unwrap(), "Value 42");
    assert_eq!(
        panel_state(app.world_mut(), entity).unwrap(),
        json!({"value":42})
    );
    assert!(matches!(actions.as_slice(), [Action::ClosePanel(target)] if *target == entity));
    assert!(app.world().get::<PanelRecord>(entity).is_some());
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().focused,
        Some(entity)
    );
    for action in actions {
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .push(action);
    }
    process_requests(app.world_mut());
    assert!(app.world().get_entity(entity).is_err());
}

#[derive(Resource, Default)]
struct CloseGate {
    ready: bool,
    calls: Vec<Entity>,
}
fn wait_for_save(world: &mut World, entity: Entity) -> Result<bool, String> {
    assert!(!world.contains_resource::<UiLoan>());
    let mut gate = world.resource_mut::<CloseGate>();
    gate.calls.push(entity);
    Ok(gate.ready)
}

#[test]
fn deferred_close_keeps_the_frontend_and_layout_until_application_commits() {
    let mut app = app();
    app.init_resource::<CloseGate>()
        .insert_resource(WorkbenchClosePolicy(wait_for_save));
    invoke(&mut app, "test.open");
    process_requests(app.world_mut());
    let entity = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    let saved = save(app.world_mut());
    app.world_mut()
        .resource_mut::<WorkbenchRequests>()
        .push(Action::ClosePanel(entity));
    process_requests(app.world_mut());
    assert_eq!(app.world().resource::<CloseGate>().calls, [entity]);
    assert_eq!(save(app.world_mut()), saved);
    assert_eq!(panel_title(app.world_mut(), entity).unwrap(), "Counter");
    assert!(
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .take_errors()
            .is_empty()
    );

    app.world_mut().resource_mut::<CloseGate>().ready = true;
    assert!(commit_close(app.world_mut(), entity));
    assert!(!commit_close(app.world_mut(), entity));
    assert_eq!(app.world().resource::<CloseGate>().calls, [entity]);
    assert!(app.world().get_entity(entity).is_err());
    assert!(
        app.world()
            .resource::<WorkbenchLayout>()
            .tiling
            .areas
            .iter()
            .all(|area| area.tabs.is_empty())
    );

    invoke(&mut app, "test.open");
    process_requests(app.world_mut());
    let next = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    app.world_mut()
        .resource_mut::<WorkbenchRequests>()
        .push(Action::ClosePanel(next));
    process_requests(app.world_mut());
    assert!(app.world().get_entity(next).is_err());
    assert_eq!(app.world().resource::<CloseGate>().calls, [entity, next]);
}

#[derive(Resource)]
struct RestoreCloseGate {
    blocked: Entity,
    calls: Vec<Entity>,
}
fn restore_close_policy(world: &mut World, entity: Entity) -> Result<bool, String> {
    let mut gate = world.resource_mut::<RestoreCloseGate>();
    gate.calls.push(entity);
    Ok(entity != gate.blocked)
}

#[test]
fn restore_preflights_all_panels_before_removing_an_approved_sibling() {
    let mut app = app();
    invoke(&mut app, "test.open");
    invoke(&mut app, "test.open");
    process_requests(app.world_mut());
    let blocked = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    let old = app
        .world_mut()
        .query_filtered::<Entity, With<PanelRecord>>()
        .iter(app.world())
        .collect::<Vec<_>>();
    let saved = save(app.world_mut());
    app.insert_resource(RestoreCloseGate {
        blocked,
        calls: Vec::new(),
    })
    .insert_resource(WorkbenchClosePolicy(restore_close_policy));
    let mut replacement_layout = TilingState::default();
    replacement_layout.assign(41, 1);
    let replacement = json!({"version":1,"panels":[
        {"id":41,"panel_type":"test.counter","state":{}}
    ],"focused":41,"tiling":replacement_layout.to_value([41])});
    assert!(
        restore(app.world_mut(), &replacement)
            .unwrap_err()
            .contains("waiting")
    );
    assert_eq!(app.world().resource::<RestoreCloseGate>().calls, old);
    assert_eq!(save(app.world_mut()), saved);
    for &entity in &old {
        assert_eq!(panel_title(app.world_mut(), entity).unwrap(), "Counter");
    }
    app.world_mut().remove_resource::<WorkbenchClosePolicy>();
    restore(app.world_mut(), &replacement).unwrap();
    for entity in old {
        assert!(app.world().get_entity(entity).is_err());
    }
    assert_eq!(save(app.world_mut())["panels"][0]["id"], 41);
}

struct PanickingWidget {
    entity: Entity,
    draft: i64,
    draws: Rc<Cell<usize>>,
}
impl Panel for PanickingWidget {
    fn title(&self, _world: &World) -> String {
        "Draft".into()
    }
    fn draw(
        &mut self,
        _ui: &Ui,
        _world: &mut World,
        requests: &mut Vec<Action>,
        _shortcuts: &bed_workbench_api::ShortcutInput,
    ) {
        self.draws.set(self.draws.get() + 1);
        self.draft = 999;
        requests.push(Action::ClosePanel(self.entity));
        unsafe {
            dear_imgui_rs::sys::igBegin(
                c"unfinished extension window".as_ptr(),
                std::ptr::null_mut(),
                0,
            );
        }
        panic!("widget draw failed");
    }
    fn save(&self, _world: &World) -> Value {
        json!({"draft":self.draft})
    }
}

#[test]
fn panicking_surface_discards_intentions_and_retains_its_entry_state_after_transfer() {
    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
    let mut app = app();
    invoke(&mut app, "test.open");
    process_requests(app.world_mut());
    let entity = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    let draws = Rc::new(Cell::new(0));
    install_panel_frontend(
        app.world_mut(),
        entity,
        Box::new(PanickingWidget {
            entity,
            draft: 7,
            draws: Rc::clone(&draws),
        }),
    )
    .unwrap();
    let mut context = imgui_context();
    context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
        [800.0, 600.0],
        1.0 / 60.0,
    ));
    let ui = context.frame();
    let mut failure = None;
    ui.window("application compositor").build(|| {
        failure = Some(render_panel(app.world_mut(), entity, ui).unwrap_err());
        ui.text("The application still draws after recovery");
    });
    drop(context.render_legacy());
    assert!(failure.unwrap().contains("widget draw failed"));
    assert_eq!(draws.get(), 1);
    assert_eq!(
        panel_state(app.world_mut(), entity).unwrap(),
        json!({"draft":7})
    );
    assert!(app.world().get::<PanelRecord>(entity).is_some());
    process_requests(app.world_mut());
    assert!(app.world().get::<PanelRecord>(entity).is_some());
    let quarantined = take_panel_frontend(app.world_mut(), entity).unwrap();
    assert!(
        install_panel_frontend(app.world_mut(), entity, quarantined)
            .unwrap()
            .is_none()
    );
    context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
        [800.0, 600.0],
        1.0 / 60.0,
    ));
    let ui = context.frame();
    ui.window("application compositor").build(|| {
        assert!(
            render_panel(app.world_mut(), entity, ui)
                .unwrap()
                .is_empty()
        );
    });
    drop(context.render_legacy());
    assert_eq!(draws.get(), 1, "a damaged frontend must never run again");
    assert_eq!(
        panel_state(app.world_mut(), entity).unwrap(),
        json!({"draft":7})
    );
    assert!(commit_close(app.world_mut(), entity));
}

#[test]
fn tracked_close_completes_only_after_the_deferred_entity_is_retired() {
    let mut app = app();
    invoke(&mut app, "test.open");
    process_requests(app.world_mut());
    let panel = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    app.insert_resource(WorkbenchClosePolicy(|_, _| Ok(false)));
    let request = app
        .world_mut()
        .resource_mut::<WorkbenchRequests>()
        .submit(Action::ClosePanel(panel));
    process_requests(app.world_mut());
    assert!(
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .take_result(request)
            .is_none()
    );
    assert!(app.world().get::<PanelRecord>(panel).is_some());
    assert!(commit_close(app.world_mut(), panel));
    process_requests(app.world_mut());
    assert_eq!(
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .take_result(request),
        Some(Ok(WorkbenchOutcome::Closed(panel)))
    );
}

#[test]
fn external_request_mode_leaves_the_same_queue_to_the_application_reducer() {
    let mut app = app();
    app.insert_resource(WorkbenchRequestMode::External);
    let id = app
        .world_mut()
        .resource_mut::<WorkbenchRequests>()
        .submit(Action::Invoke(CommandInvocation {
            command: "test.open".into(),
            source: None,
            area: 1,
            arguments: Value::Null,
        }));
    process_requests(app.world_mut());
    assert!(app.world().resource::<WorkbenchLayout>().focused.is_none());
    let queued = app
        .world_mut()
        .resource_mut::<WorkbenchRequests>()
        .take_next()
        .unwrap();
    assert_eq!(queued.id, Some(id));
    let (outcome, waiting) = execute_action(app.world_mut(), queued.action).unwrap();
    assert!(waiting.is_empty());
    app.world_mut()
        .resource_mut::<WorkbenchRequests>()
        .complete(id, Ok(outcome));
    assert_eq!(
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .take_result(id),
        Some(Ok(WorkbenchOutcome::Applied))
    );
    assert!(app.world().resource::<WorkbenchLayout>().focused.is_some());
}

#[test]
fn splitting_the_focused_panel_updates_its_area_and_roundtrips_exactly() {
    for axis in [0, 1] {
        let mut app = app();
        invoke(&mut app, "test.open");
        process_requests(app.world_mut());
        let source = app.world().resource::<WorkbenchLayout>().focused.unwrap();
        let panel_id = app.world().get::<PanelPresentation>(source).unwrap().id;
        let original_area = app.world().resource::<WorkbenchLayout>().focused_area;
        let id = app
            .world_mut()
            .resource_mut::<WorkbenchRequests>()
            .submit(Action::SplitPanel {
                panel: source,
                axis,
                high: true,
            });
        process_requests(app.world_mut());
        assert_eq!(
            app.world_mut()
                .resource_mut::<WorkbenchRequests>()
                .take_result(id),
            Some(Ok(WorkbenchOutcome::Applied))
        );
        let layout = app.world().resource::<WorkbenchLayout>();
        assert_eq!(layout.focused, Some(source));
        assert_ne!(layout.focused_area, original_area);
        assert_eq!(layout.focused_area, layout.tiling.area_for(panel_id));
        let saved = save(app.world_mut());
        restore(app.world_mut(), &saved).unwrap();
        assert_eq!(save(app.world_mut()), saved);
    }
}

#[test]
fn applying_a_tab_move_updates_focused_area_while_preserving_explicit_area_focus() {
    let mut app = app();
    invoke(&mut app, "test.open");
    invoke(&mut app, "test.open");
    process_requests(app.world_mut());
    let source = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    let panel_id = app.world().get::<PanelPresentation>(source).unwrap().id;
    let mut tiling = app.world().resource::<WorkbenchLayout>().tiling.clone();
    let destination = tiling.layout.split(1, 0, 5000, 100).unwrap();
    tiling.add_area(destination);
    tiling.assign(panel_id, destination);
    let ids = tiling
        .areas
        .iter()
        .flat_map(|area| area.tabs.iter().copied())
        .collect::<Vec<_>>();
    app.world_mut()
        .resource_mut::<WorkbenchRequests>()
        .push(Action::SetLayout {
            tiling: tiling.to_value(ids.clone()),
            close: Vec::new(),
        });
    process_requests(app.world_mut());
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().focused,
        Some(source)
    );
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().focused_area,
        Some(destination)
    );
    let saved = save(app.world_mut());
    restore(app.world_mut(), &saved).unwrap();
    assert_eq!(save(app.world_mut()), saved);
    focus_area(app.world_mut(), 1);
    apply_layout(app.world_mut(), &tiling.to_value(ids)).unwrap();
    assert_eq!(app.world().resource::<WorkbenchLayout>().focused, None);
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().focused_area,
        Some(1)
    );
}

#[test]
fn showing_a_panel_uses_its_recent_focus_and_creating_is_distinct() {
    let mut app = app();
    for _ in 0..2 {
        invoke(&mut app, "test.open");
        process_requests(app.world_mut());
    }
    let newest = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    let older = app
        .world_mut()
        .query::<Entity>()
        .iter(app.world())
        .find(|e| *e != newest && app.world().get::<PanelRecord>(*e).is_some())
        .unwrap();
    focus_panel(app.world_mut(), older);
    app.world_mut()
        .resource_mut::<WorkbenchRequests>()
        .push(Action::ShowPanel {
            panel_type: "test.counter".into(),
            state: Value::Null,
            area: None,
        });
    process_requests(app.world_mut());
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().focused,
        Some(older)
    );
    assert_eq!(
        app.world_mut()
            .query::<&PanelRecord>()
            .iter(app.world())
            .count(),
        2
    );
    commit_close(app.world_mut(), older);
    assert_eq!(recent_panel(app.world_mut(), "test.counter"), Some(newest));
    commit_close(app.world_mut(), newest);
    app.world_mut()
        .resource_mut::<WorkbenchRequests>()
        .push(Action::ShowPanel {
            panel_type: "test.counter".into(),
            state: Value::Null,
            area: None,
        });
    process_requests(app.world_mut());
    assert!(app.world().resource::<WorkbenchLayout>().focused.is_some());
    assert_eq!(
        app.world_mut()
            .query::<&PanelRecord>()
            .iter(app.world())
            .count(),
        1
    );
}

#[test]
fn individual_panel_rendering_refreshes_bindings_each_frame() {
    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
    let mut app = app();
    invoke(&mut app, "test.open");
    process_requests(app.world_mut());
    let entity = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    let action = app
        .world()
        .resource::<Registry>()
        .shortcuts()
        .iter()
        .find(|a| a.id == "test.close")
        .unwrap()
        .clone();
    app.world_mut().init_non_send::<WorkbenchShortcuts>();
    let mut context = imgui_context();
    for (shortcut, key) in [
        ("F12", dear_imgui_rs::Key::F12),
        ("F11", dear_imgui_rs::Key::F11),
    ] {
        app.world_mut()
            .non_send_mut::<WorkbenchShortcuts>()
            .overrides
            .replace(
                &action,
                Some(0).filter(|_| shortcut == "F11"),
                shortcut.parse().unwrap(),
            )
            .unwrap();
        context.io_mut().add_key_event(key, true);
        context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
            [600.0, 400.0],
            1.0 / 60.0,
        ));
        let ui = context.frame();
        let mut requests = Vec::new();
        ui.window("Direct rendering").focused(true).build(|| {
            requests = render_panel(app.world_mut(), entity, ui).unwrap();
        });
        assert!(matches!(requests.as_slice(),[Action::ClosePanel(closed)] if *closed==entity));
        drop(context.render_legacy());
        context.io_mut().add_key_event(key, false);
        context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
            [600.0, 400.0],
            1.0 / 60.0,
        ));
        let _ = context.frame();
        drop(context.render_legacy());
    }
}
