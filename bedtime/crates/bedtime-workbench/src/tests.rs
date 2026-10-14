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

// Raw ImGui fixtures explicitly own the same UI lifecycle as a desktop frontend.
#[derive(Resource, Default)]
pub(super) struct RequestedUiTheme(pub bedtime_ui::appearance::UiTheme);
pub(super) fn prepare_ui(world: &mut World, context: &mut dear_imgui_rs::Context) {
    world.init_resource::<RequestedUiTheme>();
    let mut theme = world.resource::<RequestedUiTheme>().0.clone();
    // Raw fixtures initially leave FontSizeBase at zero; their default font is 13px.
    theme.fonts.size = context.style().font_size_base().max(13.0);
    let mut runtime = world
        .remove_non_send::<bedtime_ui::UiRuntime>()
        .unwrap_or_else(|| bedtime_ui::UiRuntime::new(context));
    let mut fonts = world
        .remove_non_send::<bedtime_ui::font::UiFonts>()
        .unwrap_or_default();
    runtime.prepare(context, &theme, &mut fonts).unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    world.insert_non_send(runtime);
    world.insert_non_send(fonts);
}
pub(super) fn draw_with_ui(world: &mut World, ui: &Ui, draw: impl FnOnce(&mut World, &Ui)) {
    let context = world.non_send::<bedtime_ui::UiRuntime>().frame_context();
    world.non_send_mut::<bedtime_ui::UiRuntime>().begin(ui);
    context.scope(|| draw(world, ui));
    world.non_send_mut::<bedtime_ui::UiRuntime>().end(ui);
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
        requests: &mut Vec<WorkbenchAction>,
        _shortcuts: &bedtime_api::workbench::ShortcutInput,
    ) {
        if ui.button("Close") || _shortcuts.take(ui, "test.close") {
            requests.push(WorkbenchAction::ClosePanel(self.0));
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
    Ok(vec![WorkbenchAction::OpenPanel {
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

#[test]
fn panel_focus_hooks_follow_explicit_requests_and_changed_observations() {
    #[derive(Resource, Default)]
    struct Focused(Vec<Entity>);
    let mut app = app();
    app.init_resource::<Focused>();
    let focus = app.world_mut().register_system(
        |In(entity): In<Entity>, mut focused: ResMut<Focused>| -> Result<(), String> {
            focused.0.push(entity);
            Ok(())
        },
    );
    register(app.world_mut(), "focus", |registrar| {
        registrar.panel(
            "focus.panel",
            "Focus fixture",
            move |In(creation): In<PanelCreation>, world: &mut World| -> PanelResult {
                world
                    .entity_mut(creation.entity)
                    .insert(api::PanelFocus(focus));
                Ok(Box::new(CounterPanel(creation.entity)))
            },
        );
    })
    .unwrap();
    let first = open_panel(
        app.world_mut(),
        json!({"panel_type":"focus.panel"}),
        None,
        None,
    )
    .unwrap();
    let second = open_panel(
        app.world_mut(),
        json!({"panel_type":"focus.panel"}),
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        app.world().resource::<Focused>().0,
        [first, second],
        "newly installed frontends receive initial focus"
    );
    focus_panel(app.world_mut(), first);
    app.world_mut()
        .resource_mut::<WorkbenchRequests>()
        .push(WorkbenchAction::ObserveFocus(first));
    process_requests(app.world_mut());
    assert_eq!(
        app.world().resource::<Focused>().0,
        [first, second, first],
        "observing the selected widget does not focus it again"
    );

    // The native window has acknowledged the explicit focus. A later native
    // selection may now move to another panel and focus its owned widget.
    app.world_mut()
        .get_mut::<PanelPresentation>(first)
        .unwrap()
        .focus = false;
    app.world_mut()
        .resource_mut::<WorkbenchRequests>()
        .push(WorkbenchAction::ObserveFocus(second));
    process_requests(app.world_mut());
    app.world_mut()
        .resource_mut::<WorkbenchRequests>()
        .push(WorkbenchAction::FocusPanel(second));
    process_requests(app.world_mut());
    assert_eq!(
        app.world().resource::<Focused>().0,
        [first, second, first, second, second],
        "an explicit request refocuses even the selected widget"
    );
    focus_panel(app.world_mut(), first);
    let saved = save(app.world_mut());
    restore(app.world_mut(), &saved).unwrap();
    let restored = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    assert_eq!(
        app.world().resource::<Focused>().0.last(),
        Some(&restored),
        "restoration finishes with the saved widget, regardless of factory order"
    );
}

fn invoke(app: &mut App, command: &str) {
    let request = capture_command(app.world(), command, Value::Null);
    app.world_mut()
        .resource_mut::<WorkbenchRequests>()
        .push(WorkbenchAction::Invoke(request));
}

#[test]
fn command_availability_is_checked_when_delayed_requests_execute() {
    let mut app = app();
    for (enabled, visible) in [(false, true), (true, false)] {
        let command = capture_command(app.world(), "test.open", Value::Null);
        let id = app
            .world_mut()
            .resource_mut::<WorkbenchRequests>()
            .submit(WorkbenchAction::Invoke(command));
        app.world_mut()
            .resource_mut::<api::CommandPresentations>()
            .0
            .insert(
                "test.open".into(),
                api::CommandPresentation {
                    enabled,
                    visible,
                    label: None,
                    ..Default::default()
                },
            );
        process_requests(app.world_mut());
        assert_eq!(
            app.world_mut()
                .resource_mut::<WorkbenchRequests>()
                .take_result(id)
                .unwrap()
                .unwrap(),
            WorkbenchOutcome::Ignored
        );
        assert!(
            save(app.world_mut())["panels"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    app.world_mut()
        .resource_mut::<api::CommandPresentations>()
        .0
        .clear();
    invoke(&mut app, "test.open");
    process_requests(app.world_mut());
    assert_eq!(save(app.world_mut())["panels"].as_array().unwrap().len(), 1);
}

#[test]
fn invocation_facts_follow_the_canonical_source_and_exact_arguments() {
    let mut app = app();
    invoke(&mut app, "test.open");
    process_requests(app.world_mut());
    let first = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    invoke(&mut app, "test.open");
    process_requests(app.world_mut());
    let second = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    let frontend = app.world_mut().spawn(api::PanelOwner(first)).id();
    for (fact_arguments, source, arguments, opens_panel) in [
        (Value::Null, frontend, Value::Null, false),
        (Value::Null, second, Value::Null, true),
        (Value::Null, first, json!({"target":1}), true),
        (json!({"target":1}), first, json!({"target":1}), false),
        (json!({"target":1}), first, json!({"target":2}), true),
    ] {
        app.world_mut()
            .resource_mut::<api::CommandPresentations>()
            .0
            .insert(
                "test.open".into(),
                api::CommandPresentation {
                    enabled: false,
                    scope: api::CommandPresentationScope::Invocation {
                        source: Some(first),
                        arguments: fact_arguments,
                    },
                    ..Default::default()
                },
            );
        let receipt =
            app.world_mut()
                .resource_mut::<WorkbenchRequests>()
                .submit(WorkbenchAction::Invoke(CommandInvocation {
                    command: "test.open".into(),
                    source: Some(source),
                    area: 1,
                    arguments,
                }));
        process_requests(app.world_mut());
        let expected = if opens_panel {
            WorkbenchOutcome::Opened(app.world().resource::<WorkbenchLayout>().focused.unwrap())
        } else {
            WorkbenchOutcome::Ignored
        };
        assert_eq!(
            app.world_mut()
                .resource_mut::<WorkbenchRequests>()
                .take_result(receipt),
            Some(Ok(expected))
        );
    }
    app.world_mut()
        .resource_mut::<api::CommandPresentations>()
        .0
        .insert(
            "test.open".into(),
            api::CommandPresentation {
                enabled: false,
                ..Default::default()
            },
        );
    let receipt =
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .submit(WorkbenchAction::Invoke(CommandInvocation {
                command: "test.open".into(),
                source: Some(second),
                area: 1,
                arguments: json!({"target":1}),
            }));
    process_requests(app.world_mut());
    assert_eq!(
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .take_result(receipt),
        Some(Ok(WorkbenchOutcome::Ignored))
    );
}

#[test]
fn host_prepares_panels_before_factories_and_preserves_failed_inputs() {
    #[derive(Component)]
    struct Association;
    let mut app = app();
    register(app.world_mut(), "prepared", |registrar| {
        registrar.panel(
            "prepared.panel",
            "Prepared",
            |In(input): In<PanelCreation>, world: &mut World| -> PanelResult {
                if world.get::<Association>(input.entity).is_none() {
                    return Err("Document association is missing".into());
                }
                Ok(Box::new(CounterPanel(input.entity)))
            },
        );
    })
    .unwrap();
    app.world_mut()
        .insert_resource(PanelPreparation(|world, entity| {
            world.entity_mut(entity).insert(Association);
            Ok(())
        }));
    let prepared = open_panel(
        app.world_mut(),
        json!({"panel_type":"prepared.panel", "state":{"document":"canonical"}}),
        None,
        None,
    )
    .unwrap();
    assert!(app.world().get::<Association>(prepared).is_some());
    assert!(
        app.world()
            .get::<PanelRecord>(prepared)
            .unwrap()
            .unavailable
            .is_none()
    );
    app.world_mut().insert_resource(PanelPreparation(|_, _| {
        Err("Document cannot be restored".into())
    }));
    let failed = open_panel(
        app.world_mut(),
        json!({"panel_type":"prepared.panel", "state":{"document":"retain-on-failure"}}),
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        app.world()
            .get::<PanelRecord>(failed)
            .unwrap()
            .unavailable
            .as_deref(),
        Some("Document cannot be restored")
    );
    let saved = save(app.world_mut());
    assert!(
        saved["panels"]
            .as_array()
            .unwrap()
            .iter()
            .any(|record| record["state"]["document"] == "retain-on-failure")
    );
}

#[test]
fn suspended_panels_keep_state_without_editor_or_slot_integration() {
    let mut app = app();
    invoke(&mut app, "test.open");
    process_requests(app.world_mut());
    let hidden = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    app.world_mut()
        .entity_mut(hidden)
        .insert(api::PanelSuspended);
    app.world_mut().resource_mut::<WorkbenchLayout>().focused = None;
    let empty = TilingState::default().to_value([]);
    apply_layout(app.world_mut(), &empty).unwrap();
    assert!(recent_panel(app.world_mut(), "test.counter").is_none());
    focus_panel(app.world_mut(), hidden);
    observe_focus(app.world_mut(), hidden);
    assert!(app.world().resource::<WorkbenchLayout>().focused.is_none());
    assert_eq!(
        panel_state(app.world_mut(), hidden).unwrap(),
        json!({"count":7})
    );
    assert!(
        save(app.world_mut())["panels"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    invoke(&mut app, "test.open");
    process_requests(app.world_mut());
    let active = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    assert_ne!(active, hidden);
    assert_ne!(
        app.world().get::<PanelPresentation>(hidden).unwrap().id,
        app.world().get::<PanelPresentation>(active).unwrap().id
    );
    assert_eq!(save(app.world_mut())["panels"].as_array().unwrap().len(), 1);
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
                Ok(vec![WorkbenchAction::FocusPanel(source)])
            },
        );
    })
    .unwrap();
    app.world_mut()
        .resource_mut::<WorkbenchRequests>()
        .push(WorkbenchAction::Invoke(CommandInvocation {
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
        .push(WorkbenchAction::ClosePanel(child));
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
            .resource::<WorkbenchRegistry>()
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
            .resource::<WorkbenchRegistry>()
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
        .push(WorkbenchAction::Invoke(stale));
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
            .push(WorkbenchAction::FocusPanel(entities[&id]));
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
fn closing_and_reopening_a_tab_preserves_queued_focus_until_the_native_window_draws() {
    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
    let mut app = app();
    app.init_resource::<RequestedUiTheme>();
    app.world_mut()
        .resource_mut::<RequestedUiTheme>()
        .0
        .motion
        .enabled = false;
    let mut context = imgui_context();
    let draw = |app: &mut App, context: &mut dear_imgui_rs::Context| {
        context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
            [800.0, 600.0],
            1.0 / 60.0,
        ));
        prepare_ui(app.world_mut(), context);
        draw_with_ui(app.world_mut(), context.frame(), crate::render_ui);
        drop(context.render_legacy());
    };
    let original = open_panel(
        app.world_mut(),
        json!({"panel_type":"test.counter"}),
        Some(1),
        None,
    )
    .unwrap();
    let surviving = open_panel(
        app.world_mut(),
        json!({"panel_type":"test.counter"}),
        Some(1),
        None,
    )
    .unwrap();
    focus_panel(app.world_mut(), original);
    for _ in 0..6 {
        draw(&mut app, &mut context);
        process_requests(app.world_mut());
    }
    assert!(
        app.world()
            .get::<api::PanelGeometry>(original)
            .unwrap()
            .visible
    );
    assert!(commit_close(app.world_mut(), original));
    assert_eq!(app.world().resource::<WorkbenchLayout>().focused, None);

    // Closing the selected native window reveals the surviving tab. Capture
    // that native fact while an earlier application open is still queued.
    let surviving_id = app.world().get::<PanelPresentation>(surviving).unwrap().id;
    let name = std::ffi::CString::new(format!("###bed_tab_{surviving_id}")).unwrap();
    context.binding().with_bound_context(|| unsafe {
        dear_imgui_rs::sys::igSetWindowFocus_Str(name.as_ptr());
    });
    let open =
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .submit(WorkbenchAction::OpenPanel {
                panel_type: "test.counter".into(),
                state: Value::Null,
                area: Some(1),
            });
    draw(&mut app, &mut context);
    assert!(
        app.world()
            .resource::<WorkbenchRequests>()
            .pending
            .iter()
            .any(|request| matches!(request.action, WorkbenchAction::ObserveFocus(panel) if panel == surviving)),
        "the frontend must capture the surviving tab's native focus before the queued open"
    );
    process_requests(app.world_mut());
    let WorkbenchOutcome::Opened(reopened) = app
        .world_mut()
        .resource_mut::<WorkbenchRequests>()
        .take_result(open)
        .unwrap()
        .unwrap()
    else {
        panic!("queued open did not create a tab");
    };
    let reopened_id = app.world().get::<PanelPresentation>(reopened).unwrap().id;
    assert_ne!(reopened, original);
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().focused,
        Some(reopened)
    );
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().tiling.areas[0].selected,
        Some(reopened_id)
    );

    // Native docking may draw the new window immediately or need another
    // frame. A skipped first Begin must leave the focus request pending.
    draw(&mut app, &mut context);
    if !app
        .world()
        .get::<api::PanelGeometry>(reopened)
        .is_some_and(|geometry| geometry.visible)
    {
        assert!(
            app.world()
                .get::<PanelPresentation>(reopened)
                .unwrap()
                .focus,
            "a window that has not drawn must retain its requested native focus"
        );
    }
    process_requests(app.world_mut());
    for _ in 0..6 {
        draw(&mut app, &mut context);
        process_requests(app.world_mut());
        assert_eq!(
            app.world().resource::<WorkbenchLayout>().focused,
            Some(reopened)
        );
    }
    let presentation = app.world().get::<PanelPresentation>(reopened).unwrap();
    assert!(
        !presentation.focus,
        "native focus must acknowledge the request"
    );
    assert_ne!(presentation.dock_id, 0);
    assert!(
        app.world()
            .get::<api::PanelGeometry>(reopened)
            .unwrap()
            .visible
    );
    assert!(
        !app.world()
            .get::<api::PanelGeometry>(surviving)
            .unwrap()
            .visible
    );
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().tiling.areas[0].tabs,
        [surviving_id, reopened_id]
    );
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
    requests.push(WorkbenchAction::FocusPanel(foreign));
    requests.push(WorkbenchAction::SplitPanel {
        panel: foreign,
        axis: 0,
        high: true,
    });
    requests.push(WorkbenchAction::ClosePanel(foreign));
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
        .push(WorkbenchAction::Invoke(CommandInvocation {
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
        requests: &mut Vec<WorkbenchAction>,
        _shortcuts: &bedtime_api::workbench::ShortcutInput,
    ) {
        world.get_mut::<WidgetState>(self.0).unwrap().0 += 1;
        ui.text(self.title(world));
        requests.push(WorkbenchAction::ClosePanel(self.0));
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
    assert!(
        matches!(actions.as_slice(), [WorkbenchAction::ClosePanel(target)] if *target == entity)
    );
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
        .push(WorkbenchAction::ClosePanel(entity));
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
        .push(WorkbenchAction::ClosePanel(next));
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
        requests: &mut Vec<WorkbenchAction>,
        _shortcuts: &bedtime_api::workbench::ShortcutInput,
    ) {
        self.draws.set(self.draws.get() + 1);
        self.draft = 999;
        requests.push(WorkbenchAction::ClosePanel(self.entity));
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
        .submit(WorkbenchAction::ClosePanel(panel));
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

#[derive(Resource, Default)]
struct DeferredCloseDecision {
    cancel: bool,
    groups: Vec<Vec<Entity>>,
}

fn deferred_close_decision(world: &mut World, _: Entity) -> Result<bool, String> {
    let group = world.resource::<ClosingPanels>().0.clone();
    let mut decision = world.resource_mut::<DeferredCloseDecision>();
    decision.groups.push(group);
    if decision.cancel {
        Err("Operation cancelled".into())
    } else {
        Ok(false)
    }
}

#[test]
fn deferred_close_cancellation_completes_the_receipt_and_keeps_the_panel() {
    let mut app = app();
    invoke(&mut app, "test.open");
    process_requests(app.world_mut());
    let panel = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    let saved = save(app.world_mut());
    app.init_resource::<DeferredCloseDecision>()
        .insert_resource(WorkbenchClosePolicy(deferred_close_decision));
    let request = app
        .world_mut()
        .resource_mut::<WorkbenchRequests>()
        .submit(WorkbenchAction::ClosePanel(panel));
    process_requests(app.world_mut());
    poll_request_results(app.world_mut());
    assert!(
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .take_result(request)
            .is_none()
    );
    assert_eq!(
        app.world().resource::<DeferredCloseDecision>().groups,
        [vec![panel], vec![panel]]
    );
    app.world_mut()
        .resource_mut::<DeferredCloseDecision>()
        .cancel = true;
    poll_request_results(app.world_mut());
    assert_eq!(
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .take_result(request),
        Some(Err("Operation cancelled".into()))
    );
    assert_eq!(save(app.world_mut()), saved);
    assert!(app.world().get::<PanelRecord>(panel).is_some());
    assert!(
        app.world()
            .resource::<WorkbenchRequests>()
            .waiting
            .is_empty()
    );
}

#[test]
fn deferred_layout_cancellation_retries_the_complete_close_group_without_retiring_it() {
    let mut app = app();
    let mut panels = Vec::new();
    for _ in 0..2 {
        invoke(&mut app, "test.open");
        process_requests(app.world_mut());
        panels.push(app.world().resource::<WorkbenchLayout>().focused.unwrap());
    }
    let saved = save(app.world_mut());
    app.init_resource::<DeferredCloseDecision>()
        .insert_resource(WorkbenchClosePolicy(deferred_close_decision));
    let request =
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .submit(WorkbenchAction::SetLayout {
                tiling: TilingState::default().to_value(std::iter::empty()),
                close: panels.clone(),
            });
    process_requests(app.world_mut());
    assert!(waiting_for_layout(app.world()));
    app.world_mut()
        .resource_mut::<DeferredCloseDecision>()
        .cancel = true;
    poll_request_results(app.world_mut());
    assert_eq!(
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .take_result(request),
        Some(Err("Operation cancelled".into()))
    );
    assert_eq!(save(app.world_mut()), saved);
    assert!(
        panels
            .iter()
            .all(|panel| app.world().get::<PanelRecord>(*panel).is_some())
    );
    assert!(
        app.world()
            .resource::<DeferredCloseDecision>()
            .groups
            .iter()
            .all(|group| group == &panels)
    );
    assert!(!waiting_for_layout(app.world()));
}

#[test]
fn an_untracked_deferred_close_retries_the_policy_until_approved() {
    let mut app = app();
    invoke(&mut app, "test.open");
    process_requests(app.world_mut());
    let panel = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    app.init_resource::<CloseGate>()
        .insert_resource(WorkbenchClosePolicy(wait_for_save));
    app.world_mut()
        .resource_mut::<WorkbenchRequests>()
        .push(WorkbenchAction::ClosePanel(panel));
    process_requests(app.world_mut());
    assert!(app.world().get::<PanelRecord>(panel).is_some());
    app.world_mut().resource_mut::<CloseGate>().ready = true;
    poll_request_results(app.world_mut());
    assert!(app.world().get_entity(panel).is_err());
    assert_eq!(app.world().resource::<CloseGate>().calls, [panel, panel]);
    assert!(
        app.world()
            .resource::<WorkbenchRequests>()
            .waiting
            .is_empty()
    );
}

#[test]
fn external_admission_preserves_command_receipts_in_the_shared_resolver() {
    let mut app = app();
    app.insert_resource(WorkbenchRequestMode::External);
    let id = app
        .world_mut()
        .resource_mut::<WorkbenchRequests>()
        .submit(WorkbenchAction::Invoke(CommandInvocation {
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
    assert_eq!(resolve_tracked_request(app.world_mut(), queued), id);
    let opened = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    assert_eq!(
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .take_result(id),
        Some(Ok(WorkbenchOutcome::Opened(opened)))
    );
    assert!(app.world().get::<PanelRecord>(opened).is_some());
    assert!(
        app.world()
            .resource::<WorkbenchRequests>()
            .pending
            .is_empty()
    );
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
        let id = app.world_mut().resource_mut::<WorkbenchRequests>().submit(
            WorkbenchAction::SplitPanel {
                panel: source,
                axis,
                high: true,
            },
        );
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
        .push(WorkbenchAction::SetLayout {
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
        .push(WorkbenchAction::ShowPanel {
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
        .push(WorkbenchAction::ShowPanel {
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
        .resource::<WorkbenchRegistry>()
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
                (shortcut == "F11").then_some(0),
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
        assert!(
            matches!(requests.as_slice(),[WorkbenchAction::ClosePanel(closed)] if *closed==entity)
        );
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
