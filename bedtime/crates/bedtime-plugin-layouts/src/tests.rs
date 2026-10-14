use super::*;
use bedtime_api::workbench::{
    Panel, PanelCreation, PanelPersistence, PanelResult, ShortcutInput, WorkbenchAction,
};
use dear_imgui_rs::Ui;

#[derive(Component)]
struct Contents(Value);

#[derive(Resource, Default)]
struct ApplicationActions(usize);

struct Fixture(Entity);
impl Panel for Fixture {
    fn title(&self, _: &World) -> String {
        "Fixture".into()
    }
    fn draw(&mut self, _: &Ui, _: &mut World, _: &mut Vec<WorkbenchAction>, _: &ShortcutInput) {}
    fn save(&self, world: &World) -> Value {
        world.get::<Contents>(self.0).unwrap().0.clone()
    }
}

fn app() -> App {
    app_with::<WorkbenchLayouts>()
}
fn app_with<Integration: LayoutIntegration + 'static>() -> App {
    let mut app = App::new();
    app.add_plugins(LayoutControllerPlugin::<LayoutId, Integration>::new(
        LayoutId::default(),
    ));
    app.init_resource::<ApplicationActions>();
    bedtime_api::workbench::register(app.world_mut(), "fixture", |registrar| {
        registrar.panel(
            "fixture.panel",
            "Fixture",
            |In(creation): In<PanelCreation>, world: &mut World| -> PanelResult {
                world
                    .entity_mut(creation.entity)
                    .insert(Contents(creation.state));
                Ok(Box::new(Fixture(creation.entity)))
            },
        );
        registrar.command(
            "fixture.open",
            "Open Fixture",
            |In(command): In<bedtime_api::workbench::CommandInvocation>| {
                Ok(vec![WorkbenchAction::OpenPanel {
                    panel_type: "fixture.panel".into(),
                    state: Value::Null,
                    area: Some(command.area),
                }])
            },
        );
        registrar.command(
            "fixture.application",
            "Application WorkbenchAction",
            |_: In<bedtime_api::workbench::CommandInvocation>,
             mut actions: ResMut<ApplicationActions>| {
                actions.0 += 1;
                Ok(Vec::new())
            },
        );
    })
    .unwrap();
    app
}

#[test]
fn delayed_area_commands_expire_after_switching_away_and_back() {
    let mut app = app();
    let original = panel(app.world_mut(), "original");
    let directed = bedtime_workbench::capture_command(app.world(), "fixture.open", Value::Null);
    let application =
        bedtime_workbench::capture_command(app.world(), "fixture.application", Value::Null);
    let directed_id = app
        .world_mut()
        .resource_mut::<bedtime_workbench::WorkbenchRequests>()
        .submit(WorkbenchAction::Invoke(directed));
    let application_id = app
        .world_mut()
        .resource_mut::<bedtime_workbench::WorkbenchRequests>()
        .submit_unscoped(WorkbenchAction::Invoke(application));
    transition::<LayoutId, WorkbenchLayouts>(app.world_mut(), LayoutId::new(2).unwrap(), &empty())
        .unwrap();
    transition::<LayoutId, WorkbenchLayouts>(app.world_mut(), LayoutId::default(), &empty())
        .unwrap();
    bedtime_workbench::process_requests(app.world_mut());
    assert_eq!(active_panels(app.world_mut()), vec![original]);
    assert_eq!(
        app.world_mut()
            .resource_mut::<bedtime_workbench::WorkbenchRequests>()
            .take_result(directed_id)
            .unwrap()
            .unwrap(),
        bedtime_workbench::WorkbenchOutcome::Ignored
    );
    assert!(
        app.world_mut()
            .resource_mut::<bedtime_workbench::WorkbenchRequests>()
            .take_result(application_id)
            .unwrap()
            .is_ok()
    );
    assert_eq!(app.world().resource::<ApplicationActions>().0, 1);
}

fn panel(world: &mut World, draft: &str) -> Entity {
    bedtime_workbench::open_panel(
        world,
        json!({"panel_type":"fixture.panel", "state":{"draft":draft}}),
        None,
        None,
    )
    .unwrap()
}

fn empty() -> Value {
    json!({"version":1,"panels":[],"tiling":bedtime_workbench::workspace::tiling_state::TilingState::default().to_value([]),"focused":null,"focused_area":1})
}

#[test]
fn arbitrary_numbered_layouts_keep_live_panels_drafts_and_focus() {
    let mut app = app();
    let original = panel(app.world_mut(), "unsaved draft");
    let epoch = *app
        .world()
        .resource::<bedtime_workbench::WorkbenchLayoutEpoch>();
    let twelve = LayoutId::new(12).unwrap();
    transition::<LayoutId, WorkbenchLayouts>(app.world_mut(), twelve, &empty()).unwrap();
    assert!(!bedtime_api::workbench::panel_active(app.world(), original));
    assert!(app.world().entities().contains(original));
    assert!(
        app.world()
            .resource::<bedtime_workbench::WorkbenchLayoutEpoch>()
            .0
            > epoch.0
    );
    let second = panel(app.world_mut(), "second draft");
    transition::<LayoutId, WorkbenchLayouts>(app.world_mut(), LayoutId::default(), &empty())
        .unwrap();
    assert_eq!(active_panels(app.world_mut()), vec![original]);
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().focused,
        Some(original)
    );
    assert!(!bedtime_api::workbench::panel_active(app.world(), second));
    let snapshot = save(app.world_mut());
    assert_eq!(snapshot["active_layout_slot"], 1);
    assert_eq!(
        snapshot["layouts"]["1"]["panels"][0]["state"]["draft"],
        "unsaved draft"
    );
    assert_eq!(
        snapshot["layouts"]["12"]["panels"][0]["state"]["draft"],
        "second draft"
    );
}

#[test]
fn stored_layout_ids_remap_without_closing_retained_panels() {
    let mut app = app();
    let original = panel(app.world_mut(), "original");
    let saved = bedtime_workbench::save(app.world_mut());
    transition::<LayoutId, WorkbenchLayouts>(app.world_mut(), LayoutId::new(2).unwrap(), &saved)
        .unwrap();
    let copied = active_panels(app.world_mut())[0];
    assert_ne!(copied, original);
    assert_ne!(
        app.world().get::<PanelPresentation>(copied).unwrap().id,
        app.world().get::<PanelPresentation>(original).unwrap().id
    );
    assert!(app.world().entities().contains(original));
    assert_eq!(
        app.world().get::<Contents>(copied).unwrap().0["draft"],
        "original"
    );
    transition::<LayoutId, WorkbenchLayouts>(app.world_mut(), LayoutId::default(), &empty())
        .unwrap();
    assert_eq!(active_panels(app.world_mut()), vec![original]);
}

struct FailedPersistence;
impl LayoutIntegration for FailedPersistence {
    type Context = ();
    fn capture_context(_: &World) {}
    fn activate_context(world: &mut World, context: Option<&()>) -> io::Result<()> {
        WorkbenchLayouts::activate_context(world, context)
    }
    fn restore(world: &mut World, value: &Value) -> io::Result<()> {
        restore(world, value)
    }
    fn retire(world: &mut World, panels: &[Entity]) -> io::Result<()> {
        WorkbenchLayouts::retire(world, panels)
    }
    fn finish(_: &mut World) -> io::Result<()> {
        Err(io::Error::other("Persistence failed"))
    }
}

#[test]
fn persistence_failure_restores_membership_focus_and_activation_identity() {
    let mut app = app();
    let original = panel(app.world_mut(), "retained");
    let layout = app.world().resource::<WorkbenchLayout>().clone();
    let epoch = *app
        .world()
        .resource::<bedtime_workbench::WorkbenchLayoutEpoch>();
    let template = bedtime_workbench::save(app.world_mut());
    let result = transition::<LayoutId, FailedPersistence>(
        app.world_mut(),
        LayoutId::new(2).unwrap(),
        &template,
    );
    assert_eq!(result.unwrap_err().to_string(), "Persistence failed");
    assert_eq!(
        app.world().resource::<LayoutSlots>().active(),
        LayoutId::default()
    );
    assert_eq!(app.world().resource::<WorkbenchLayout>(), &layout);
    assert_eq!(
        *app.world()
            .resource::<bedtime_workbench::WorkbenchLayoutEpoch>(),
        epoch
    );
    assert_eq!(active_panels(app.world_mut()), vec![original]);
    assert_eq!(
        app.world_mut()
            .query::<&PanelPresentation>()
            .iter(app.world())
            .count(),
        1
    );
}

#[test]
fn templates_omit_state_and_snapshots_retain_unknown_saved_layouts() {
    let mut app = app();
    let record = json!({"version":99,"unknown":"retained verbatim"});
    app.world_mut()
        .insert_resource(LayoutSlots::<LayoutId>::from_saved(
            LayoutId::default(),
            Map::from_iter([("other".into(), record.clone())]),
        ));
    let original = panel(app.world_mut(), "sensitive draft");
    app.world_mut()
        .get_mut::<PanelRecord>(original)
        .unwrap()
        .serialized["path"] = json!("private-input");
    let transient = panel(app.world_mut(), "transient");
    app.world_mut()
        .entity_mut(transient)
        .insert(PanelPersistence(false));
    let template = template(app.world_mut());
    assert_eq!(template["panels"].as_array().unwrap().len(), 1);
    assert!(template["panels"][0].get("state").is_none());
    assert!(template["panels"][0].get("path").is_none());
    assert!(app.world().entities().contains(original));
    assert_eq!(save(app.world_mut())["layouts"]["other"], record);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum CustomKey {
    Editing,
    Monitoring,
}
impl LayoutKey for CustomKey {
    fn saved_key(&self) -> String {
        match self {
            Self::Editing => "editing",
            Self::Monitoring => "monitoring",
        }
        .into()
    }
    fn saved_value(&self) -> Value {
        Value::String(self.saved_key())
    }
}

#[test]
fn applications_choose_custom_layout_identities() {
    let mut app = app();
    app.world_mut()
        .insert_resource(LayoutSlots::<CustomKey>::new(CustomKey::Editing));
    let original = panel(app.world_mut(), "editing");
    transition::<CustomKey, WorkbenchLayouts>(app.world_mut(), CustomKey::Monitoring, &empty())
        .unwrap();
    assert!(!bedtime_api::workbench::panel_active(app.world(), original));
    let saved = snapshot::<CustomKey, ()>(
        app.world_mut(),
        |_| (),
        |world, panels, layout, _| Ok(bedtime_workbench::save_layout(world, panels, layout)),
    )
    .unwrap();
    assert_eq!(saved["active_layout_slot"], "monitoring");
    assert_eq!(
        saved["layouts"]["editing"]["panels"][0]["state"]["draft"],
        "editing"
    );
}

#[test]
fn definitions_validate_geometry_and_keep_initial_state_separate_from_saved_state() {
    use bedtime_workbench::workspace::tiling::Layout;
    let mut geometry = Layout::default();
    let right = geometry.split(1, 0, 6000, 1).unwrap();
    let mut definition = LayoutDefinition::new(geometry);
    assert!(
        definition
            .add_panel(PanelDefinition::new("fixture.panel"), 999)
            .is_err()
    );
    assert!(
        definition
            .add_panel(PanelDefinition::new("invalid type"), 1)
            .is_err()
    );
    let first = definition
        .add_panel(
            PanelDefinition::new("fixture.panel").initial_state(json!({"count":10})),
            right,
        )
        .unwrap();
    definition.focus(first).unwrap();
    assert!(definition.focus(999).is_err());
    let saved = definition.to_value();
    assert!(saved["panels"][0].get("state").is_none());
    assert_eq!(
        LayoutDefinition::from_value(&saved).unwrap().snapshot()["panels"][0]["state"]["count"],
        10
    );
    let mut old_snapshot = definition.snapshot();
    old_snapshot["panels"][0]["path"] = json!("/private/document");
    old_snapshot["panels"][0]["state"] = json!({"draft":"private"});
    let initial = LayoutDefinition::from_value(&old_snapshot)
        .unwrap()
        .snapshot();
    assert!(initial["panels"][0].get("path").is_none());
    assert!(initial["panels"][0]["state"].is_null());
    let mut invalid = saved;
    invalid["focused"] = json!(999);
    assert!(LayoutDefinition::from_value(&invalid).is_err());
}

#[test]
fn rearranging_preserves_instance_state_and_moves_extra_tabs_without_retiring_them() {
    use bedtime_workbench::workspace::tiling::Layout;
    let mut app = app();
    let first = panel(app.world_mut(), "first draft");
    let second = panel(app.world_mut(), "second draft");
    let extra = panel(app.world_mut(), "extra draft");
    let mut geometry = Layout::default();
    let right = geometry.split(1, 0, 6500, 1).unwrap();
    let mut definition = LayoutDefinition::new(geometry);
    let first_id = definition
        .add_panel(
            PanelDefinition::new("fixture.panel")
                .initial_state(json!({"draft":"must not replace existing"})),
            right,
        )
        .unwrap();
    definition
        .add_panel(PanelDefinition::new("fixture.panel"), 1)
        .unwrap();
    definition.focus(first_id).unwrap();
    rearrange::<WorkbenchLayouts>(app.world_mut(), &definition).unwrap();
    assert_eq!(active_panels(app.world_mut()), vec![first, second, extra]);
    assert_eq!(
        app.world().get::<Contents>(first).unwrap().0["draft"],
        "first draft"
    );
    assert_eq!(
        app.world().get::<Contents>(second).unwrap().0["draft"],
        "second draft"
    );
    assert_eq!(
        app.world().get::<Contents>(extra).unwrap().0["draft"],
        "extra draft"
    );
    let layout = app.world().resource::<WorkbenchLayout>();
    assert_eq!(layout.focused, Some(first));
    assert_eq!(
        layout
            .tiling
            .area_for(app.world().get::<PanelPresentation>(first).unwrap().id),
        Some(right)
    );
    assert_eq!(
        layout
            .tiling
            .area_for(app.world().get::<PanelPresentation>(extra).unwrap().id),
        Some(layout.tiling.layout.largest_area())
    );
}

#[test]
fn failed_rearrangement_removes_new_instances_and_restores_existing_geometry_and_focus() {
    let mut app = app();
    let original = panel(app.world_mut(), "keep");
    let previous = app.world().resource::<WorkbenchLayout>().clone();
    let mut definition = LayoutDefinition::new(Default::default());
    definition
        .add_panel(PanelDefinition::new("fixture.panel"), 1)
        .unwrap();
    definition
        .add_panel(
            PanelDefinition::new("fixture.panel").initial_state(json!({"draft":"new"})),
            1,
        )
        .unwrap();
    let error = rearrange::<FailedPersistence>(app.world_mut(), &definition).unwrap_err();
    assert_eq!(error.to_string(), "Persistence failed");
    assert_eq!(app.world().resource::<WorkbenchLayout>(), &previous);
    assert_eq!(active_panels(app.world_mut()), vec![original]);
    assert_eq!(
        app.world().get::<Contents>(original).unwrap().0["draft"],
        "keep"
    );
}

#[derive(Resource, Default)]
struct Waiting(bool);
struct Deferred;
impl LayoutIntegration for Deferred {
    type Context = ();
    fn capture_context(_: &World) {}
    fn activate_context(world: &mut World, context: Option<&()>) -> io::Result<()> {
        WorkbenchLayouts::activate_context(world, context)
    }
    fn restore(world: &mut World, value: &Value) -> io::Result<()> {
        WorkbenchLayouts::restore(world, value)
    }
    fn retire(world: &mut World, panels: &[Entity]) -> io::Result<()> {
        WorkbenchLayouts::retire(world, panels)
    }
    fn ready(world: &World) -> bool {
        !world.resource::<Waiting>().0
    }
}

#[test]
fn shared_switch_queue_waits_for_barriers_and_completes_in_order() {
    let mut app = app();
    app.insert_resource(Waiting(true));
    let original = panel(app.world_mut(), "keep live");
    let second = submit_transition::<LayoutId, Deferred>(
        app.world_mut(),
        LayoutId::new(2).unwrap(),
        empty(),
    );
    let back =
        submit_transition::<LayoutId, Deferred>(app.world_mut(), LayoutId::default(), empty());
    assert!(take_result::<LayoutId>(app.world_mut(), second).is_none());
    assert_eq!(active_panels(app.world_mut()), vec![original]);
    app.world_mut().resource_mut::<Waiting>().0 = false;
    poll::<LayoutId, Deferred>(app.world_mut());
    assert_eq!(
        take_result::<LayoutId>(app.world_mut(), second),
        Some(Ok(()))
    );
    assert_eq!(take_result::<LayoutId>(app.world_mut(), back), Some(Ok(())));
    assert_eq!(active_panels(app.world_mut()), vec![original]);
    app.world_mut().resource_mut::<Waiting>().0 = true;
    let cancelled = submit_transition::<LayoutId, Deferred>(
        app.world_mut(),
        LayoutId::new(3).unwrap(),
        empty(),
    );
    cancel_requests::<LayoutId>(app.world_mut(), "Workspace changed");
    app.world_mut().resource_mut::<Waiting>().0 = false;
    poll::<LayoutId, Deferred>(app.world_mut());
    assert_eq!(
        take_result::<LayoutId>(app.world_mut(), cancelled),
        Some(Err("Workspace changed".into()))
    );
    assert_eq!(
        app.world().resource::<LayoutSlots>().active(),
        LayoutId::default()
    );
}

#[test]
fn persistence_adapter_runs_at_the_controlled_transition_boundaries() {
    let mut app = app();
    // Use an ordinary application resource and Bevy system for the adapter.
    #[derive(Resource, Default)]
    struct Writes(Vec<u32>);
    app.init_resource::<Writes>();
    let writer = app
        .world_mut()
        .register_system(|world: &mut World| -> Result<(), String> {
            let slot = world.resource::<LayoutSlots>().active().number();
            world.resource_mut::<Writes>().0.push(slot);
            Ok(())
        });
    app.insert_resource(LayoutPersistence(writer));
    transition::<LayoutId, WorkbenchLayouts>(app.world_mut(), LayoutId::new(2).unwrap(), &empty())
        .unwrap();
    assert_eq!(app.world().resource::<Writes>().0, vec![1, 2]);
}

struct PartialCreation;
impl LayoutIntegration for PartialCreation {
    type Context = ();
    fn capture_context(_: &World) {}
    fn activate_context(world: &mut World, context: Option<&()>) -> io::Result<()> {
        WorkbenchLayouts::activate_context(world, context)
    }
    fn restore(world: &mut World, value: &Value) -> io::Result<()> {
        WorkbenchLayouts::restore(world, value)
    }
    fn retire(world: &mut World, panels: &[Entity]) -> io::Result<()> {
        WorkbenchLayouts::retire(world, panels)
    }
    fn create(world: &mut World, spec: &PanelDefinition) -> io::Result<Entity> {
        WorkbenchLayouts::create(world, spec)?;
        Err(io::Error::other("Factory failed after allocation"))
    }
}
#[test]
fn failed_factory_cleans_up_its_partially_created_instance() {
    let mut app = app();
    let original = panel(app.world_mut(), "keep");
    let previous = app.world().resource::<WorkbenchLayout>().clone();
    let mut definition = LayoutDefinition::new(Default::default());
    definition
        .add_panel(PanelDefinition::new("fixture.panel"), 1)
        .unwrap();
    definition
        .add_panel(PanelDefinition::new("fixture.panel"), 1)
        .unwrap();
    assert_eq!(
        rearrange::<PartialCreation>(app.world_mut(), &definition)
            .unwrap_err()
            .to_string(),
        "Factory failed after allocation"
    );
    assert_eq!(active_panels(app.world_mut()), vec![original]);
    assert_eq!(app.world().resource::<WorkbenchLayout>(), &previous);
}

#[test]
fn reset_uses_the_same_pending_operation_barrier_as_switching() {
    let mut app = app();
    app.insert_resource(Waiting(true));
    let original = panel(app.world_mut(), "unsaved draft");
    let previous = app.world().resource::<WorkbenchLayout>().clone();
    let mut geometry = bedtime_workbench::workspace::tiling::Layout::default();
    let right = geometry.split(1, 0, 5000, 1).unwrap();
    let mut definition = LayoutDefinition::new(geometry);
    definition
        .add_panel(PanelDefinition::new("fixture.panel"), right)
        .unwrap();
    definition.focus_area(1).unwrap();
    let request = submit_rearrange::<LayoutId, Deferred>(app.world_mut(), definition);
    assert!(take_result::<LayoutId>(app.world_mut(), request).is_none());
    assert_eq!(app.world().resource::<WorkbenchLayout>(), &previous);
    app.world_mut().resource_mut::<Waiting>().0 = false;
    poll::<LayoutId, Deferred>(app.world_mut());
    assert_eq!(
        take_result::<LayoutId>(app.world_mut(), request),
        Some(Ok(()))
    );
    assert_eq!(active_panels(app.world_mut()), vec![original]);
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().focused_area,
        Some(1)
    );
    assert_eq!(
        app.world().get::<Contents>(original).unwrap().0["draft"],
        "unsaved draft"
    );
}

#[test]
fn installed_controller_resumes_queued_requests_without_application_polling() {
    let mut app = app_with::<Deferred>();
    app.insert_resource(Waiting(true));
    let original = panel(app.world_mut(), "retained");
    let request = submit_transition::<LayoutId, Deferred>(
        app.world_mut(),
        LayoutId::new(2).unwrap(),
        empty(),
    );
    app.update();
    assert!(take_result::<LayoutId>(app.world_mut(), request).is_none());
    app.world_mut().resource_mut::<Waiting>().0 = false;
    app.update();
    assert_eq!(
        take_result::<LayoutId>(app.world_mut(), request),
        Some(Ok(()))
    );
    assert!(!bedtime_api::workbench::panel_active(app.world(), original));
    assert!(app.world().entities().contains(original));
}
#[test]
#[should_panic(expected = "A workbench has one layout controller")]
fn installing_a_second_controller_is_rejected_even_with_different_key_types() {
    let mut app = app();
    app.add_plugins(LayoutControllerPlugin::<CustomKey, WorkbenchLayouts>::new(
        CustomKey::Editing,
    ));
}
