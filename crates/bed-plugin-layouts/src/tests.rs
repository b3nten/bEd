use super::*;
use bed_api::workbench::{
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
    let mut app = App::new();
    app.add_plugins(LayoutsPlugin);
    app.init_resource::<ApplicationActions>();
    bed_api::workbench::register(app.world_mut(), "fixture", |registrar| {
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
            |In(command): In<bed_api::workbench::CommandInvocation>| {
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
            |_: In<bed_api::workbench::CommandInvocation>,
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
    let directed = bed_workbench::capture_command(app.world(), "fixture.open", Value::Null);
    let application =
        bed_workbench::capture_command(app.world(), "fixture.application", Value::Null);
    let directed_id = app
        .world_mut()
        .resource_mut::<bed_workbench::WorkbenchRequests>()
        .submit(WorkbenchAction::Invoke(directed));
    let application_id = app
        .world_mut()
        .resource_mut::<bed_workbench::WorkbenchRequests>()
        .submit_unscoped(WorkbenchAction::Invoke(application));
    transition::<LayoutId, WorkbenchLayouts>(app.world_mut(), LayoutId::new(2).unwrap(), &empty())
        .unwrap();
    transition::<LayoutId, WorkbenchLayouts>(app.world_mut(), LayoutId::default(), &empty())
        .unwrap();
    bed_workbench::process_requests(app.world_mut());
    assert_eq!(active_panels(app.world_mut()), vec![original]);
    assert_eq!(
        app.world_mut()
            .resource_mut::<bed_workbench::WorkbenchRequests>()
            .take_result(directed_id)
            .unwrap()
            .unwrap(),
        bed_workbench::WorkbenchOutcome::Ignored
    );
    assert!(
        app.world_mut()
            .resource_mut::<bed_workbench::WorkbenchRequests>()
            .take_result(application_id)
            .unwrap()
            .is_ok()
    );
    assert_eq!(app.world().resource::<ApplicationActions>().0, 1);
}

fn panel(world: &mut World, draft: &str) -> Entity {
    bed_workbench::open_panel(
        world,
        json!({"panel_type":"fixture.panel", "state":{"draft":draft}}),
        None,
        None,
    )
    .unwrap()
}

fn empty() -> Value {
    json!({"version":1,"panels":[],"tiling":bed_workbench::workspace::tiling_state::TilingState::default().to_value([]),"focused":null,"focused_area":1})
}

#[test]
fn arbitrary_numbered_layouts_keep_live_panels_drafts_and_focus() {
    let mut app = app();
    let original = panel(app.world_mut(), "unsaved draft");
    let epoch = *app
        .world()
        .resource::<bed_workbench::WorkbenchLayoutEpoch>();
    let twelve = LayoutId::new(12).unwrap();
    transition::<LayoutId, WorkbenchLayouts>(app.world_mut(), twelve, &empty()).unwrap();
    assert!(!bed_api::workbench::panel_active(app.world(), original));
    assert!(app.world().entities().contains(original));
    assert!(
        app.world()
            .resource::<bed_workbench::WorkbenchLayoutEpoch>()
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
    assert!(!bed_api::workbench::panel_active(app.world(), second));
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
    let saved = bed_workbench::save(app.world_mut());
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
        .resource::<bed_workbench::WorkbenchLayoutEpoch>();
    let template = bed_workbench::save(app.world_mut());
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
            .resource::<bed_workbench::WorkbenchLayoutEpoch>(),
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
    assert!(!bed_api::workbench::panel_active(app.world(), original));
    let saved = snapshot::<CustomKey, ()>(
        app.world_mut(),
        |_| (),
        |world, panels, layout, _| Ok(bed_workbench::save_layout(world, panels, layout)),
    )
    .unwrap();
    assert_eq!(saved["active_layout_slot"], "monitoring");
    assert_eq!(
        saved["layouts"]["editing"]["panels"][0]["state"]["draft"],
        "editing"
    );
}
