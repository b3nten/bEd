use super::*;
use crate::tests::TestPanels;

#[derive(Component, Reflect, Default)]
#[reflect(Component, Persist)]
#[require(PanelInstance = PanelInstance::of::<Self>(), Runtime)]
struct Note {
    text: String,
}

impl Panel for Note {
    const LABEL: &'static str = "Note";
    #[cfg(feature = "ui")]
    fn draw(_: &mut bedtime_ui::egui::Ui, _: &mut World, _: &bedtime_ui::UiContext<'_>, _: Entity) {
    }
}

/// Runtime state a panel rebuilds through `#[require]`; never saved.
#[derive(Component, Default)]
struct Runtime;

#[derive(Resource, Reflect, Default, PartialEq, Debug)]
#[reflect(Resource, Persist)]
struct Counter(u32);

#[derive(Component, Reflect)]
#[reflect(Component, Persist)]
struct Pointer(#[entities] Entity);

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(WorkbenchCorePlugin)
        .register_type::<Note>()
        .register_type::<Counter>()
        .register_type::<Pointer>()
        .init_resource::<Counter>();
    app
}

fn note(world: &mut World, area: Entity, text: &str) -> Entity {
    world.spawn((Note { text: text.into() }, InArea(area))).id()
}

fn texts(world: &World, area: Entity) -> Vec<String> {
    world
        .layouts()
        .panels_in_area(area)
        .iter()
        .map(|&panel| world.get::<Note>(panel).unwrap().text.clone())
        .collect()
}

#[test]
fn a_round_trip_restores_layouts_tabs_selection_focus_and_panel_data() {
    let mut source = app();
    let world = source.world_mut();
    let layout = crate::tests::layout(world);
    let workbench = world.layouts_mut();
    let left = workbench.destination_area(layout).unwrap();
    let right = workbench
        .split_area(left, SplitAxis::Horizontal, 6000, [100, 100])
        .unwrap();
    workbench.create("Second").unwrap();
    let world = workbench.world_mut();
    note(world, left, "a");
    let b = note(world, left, "b");
    note(world, left, "c");
    note(world, right, "d");
    // A panel with nothing persisted can't be rebuilt, so it's left out.
    world
        .layouts_mut()
        .create_panel_in_area(left, "Bare")
        .unwrap();
    world.get_mut::<Area>(left).unwrap().selected = Some(b);
    world.get_mut::<Layout>(layout).unwrap().focused_area = Some(right);
    let saved = world.persistence().capture().unwrap();

    let mut target = app();
    let world = target.world_mut();
    world.persistence_mut().restore(&saved).unwrap();
    let names: Vec<_> = world
        .layouts()
        .all()
        .iter()
        .map(|&l| world.get::<Layout>(l).unwrap().name.clone())
        .collect();
    assert_eq!(names, ["Test", "Second"]);
    let layout = world.layouts().active().unwrap();
    assert_eq!(world.get::<Layout>(layout).unwrap().name, "Test");
    let areas = world
        .get::<LayoutAreas>(layout)
        .unwrap()
        .entities()
        .to_vec();
    let (left, right) = (areas[0], areas[1]);
    assert_eq!(texts(world, left), ["a", "b", "c"]);
    assert_eq!(texts(world, right), ["d"]);
    let selected = world.get::<Area>(left).unwrap().selected.unwrap();
    assert_eq!(world.get::<Note>(selected).unwrap().text, "b");
    assert_eq!(
        world.get::<Layout>(layout).unwrap().focused_area,
        Some(right)
    );
    assert_eq!(world.get::<Area>(right).unwrap().rect().min[0], 6000);
    let restored = world.layouts().panels_in_area(left)[0];
    assert!(world.get::<Runtime>(restored).is_some());
    assert_eq!(world.get::<PanelInstance>(restored).unwrap().title, "Note");
}

#[test]
fn layouts_are_only_restored_into_an_empty_workbench() {
    let mut source = app();
    crate::tests::layout(source.world_mut());
    let saved = source.world_mut().persistence().capture().unwrap();
    let mut target = app();
    crate::tests::layout(target.world_mut());
    assert!(
        target
            .world_mut()
            .persistence_mut()
            .restore(&saved)
            .is_err()
    );
}

#[test]
fn references_to_entities_that_arent_saved_are_refused() {
    let mut app = app();
    let world = app.world_mut();
    let runtime_only = world.spawn_empty().id();
    world.spawn(Pointer(runtime_only));
    let error = world.persistence().capture().unwrap_err();
    assert!(error.contains("Pointer"));
}

#[test]
fn a_layout_template_builds_independent_copies_of_one_layout() {
    let mut source = app();
    let world = source.world_mut();
    let layout = crate::tests::layout(world);
    let workbench = world.layouts_mut();
    let left = workbench.destination_area(layout).unwrap();
    let right = workbench
        .split_area(left, SplitAxis::Horizontal, 6000, [100, 100])
        .unwrap();
    // Other layouts aren't part of the template.
    workbench.create("Other").unwrap();
    let world = workbench.world_mut();
    note(world, left, "a");
    let b = note(world, left, "b");
    let d = note(world, right, "d");
    // Refers to a panel in the layout: kept, and pointed at the copy.
    world.entity_mut(d).insert(Pointer(b));
    // Refers to something outside the layout (a document, say): left out.
    let outside = world.spawn_empty().id();
    let tied = note(world, right, "tied to a document");
    world.entity_mut(tied).insert(Pointer(outside));
    world.get_mut::<Area>(left).unwrap().selected = Some(b);
    world.get_mut::<Layout>(layout).unwrap().focused_area = Some(right);
    let template = world.layouts_mut().capture_layout(layout).unwrap();
    assert!(!template.as_str().contains("ActiveLayout"));
    assert!(world.layouts_mut().capture_layout(left).is_err());

    // Into another workbench, after its layouts, and not made active.
    let mut target = app();
    let world = target.world_mut();
    let existing = crate::tests::layout(world);
    let copies = [
        world.layouts_mut().instantiate_layout(&template).unwrap(),
        world.layouts_mut().instantiate_layout(&template).unwrap(),
    ];
    assert_eq!(world.layouts().all(), [existing, copies[0], copies[1]]);
    assert_eq!(world.layouts().active(), Some(existing));
    for copy in copies {
        assert_eq!(world.get::<Layout>(copy).unwrap().name, "Test");
        let areas = world.get::<LayoutAreas>(copy).unwrap().entities().to_vec();
        let (left, right) = (areas[0], areas[1]);
        assert_eq!(texts(world, left), ["a", "b"]);
        assert_eq!(texts(world, right), ["d"]);
        assert_eq!(world.get::<Area>(right).unwrap().rect().min[0], 6000);
        let selected = world.get::<Area>(left).unwrap().selected.unwrap();
        assert_eq!(world.get::<Note>(selected).unwrap().text, "b");
        assert_eq!(world.get::<Layout>(copy).unwrap().focused_area, Some(right));
        let d = world.layouts().panels_in_area(right)[0];
        assert_eq!(world.get::<Pointer>(d).unwrap().0, selected);
    }
    // The copies don't share panels.
    let panel = |copy| {
        world
            .layouts()
            .panels_in_area(world.get::<LayoutAreas>(copy).unwrap().entities()[0])[0]
    };
    assert_ne!(panel(copies[0]), panel(copies[1]));
}

/// An app keeps templates in a persisted resource.
#[derive(Resource, Reflect, Default)]
#[reflect(Resource, Persist)]
struct Defaults(Option<LayoutTemplate>);

#[test]
fn a_saved_template_is_nested_ron_and_round_trips() {
    let mut source = app();
    source.register_type::<Defaults>();
    let world = source.world_mut();
    let layout = crate::tests::layout(world);
    let area = world.layouts_mut().destination_area(layout).unwrap();
    note(world, area, "kept");
    let template = world.layouts_mut().capture_layout(layout).unwrap();
    world.insert_resource(Defaults(Some(template.clone())));
    let saved = world.persistence().capture().unwrap();
    // Nested, not an escaped string.
    assert!(saved.contains("\"kept\""), "{saved}");
    assert!(!saved.contains("\\\""), "{saved}");

    let mut target = app();
    target
        .register_type::<Defaults>()
        .init_resource::<Defaults>();
    target
        .world_mut()
        .persistence_mut()
        .restore(&saved)
        .unwrap();
    let world = target.world_mut();
    let restored = world.resource::<Defaults>().0.clone().unwrap();
    assert_eq!(restored, template);
    let copy = world.layouts_mut().instantiate_layout(&restored).unwrap();
    let area = world.get::<LayoutAreas>(copy).unwrap().entities()[0];
    assert_eq!(texts(world, area), ["kept"]);
}

#[test]
fn a_bad_layout_template_adds_nothing() {
    let mut source = app();
    let world = source.world_mut();
    crate::tests::layout(world);
    world.layouts_mut().create("Second").unwrap();
    let whole = world.persistence().capture().unwrap();

    let mut target = app();
    let world = target.world_mut();
    // A whole workbench save holds two layouts and a resource.
    let whole = LayoutTemplate::from_ron(whole).unwrap();
    let error = world.layouts_mut().instantiate_layout(&whole).unwrap_err();
    assert!(error.contains("resources"), "{error}");
    assert!(LayoutTemplate::from_ron("not ron (".into()).is_err());
    let not_a_world = LayoutTemplate::from_ron("(1, 2)".into()).unwrap();
    assert!(
        world
            .layouts_mut()
            .instantiate_layout(&not_a_world)
            .is_err()
    );
    assert!(world.layouts().all().is_empty());
    assert_eq!(world.query::<&Area>().iter(world).count(), 0);
}

/// Saving runs about once a second, and always when the app exits.
pub(crate) fn save_now(app: &mut App) {
    app.world_mut().write_message(AppExit::Success);
    app.update();
    app.world_mut().resource_mut::<Messages<AppExit>>().clear();
}

#[test]
fn stores_save_changes_and_restore_them_at_startup() {
    let backend = MemoryStore::default();
    let mut first = app();
    first.add_store::<WorkbenchStore>(backend.clone());
    first.update();
    let layout = crate::tests::layout(first.world_mut());
    let area = first
        .world_mut()
        .layouts_mut()
        .destination_area(layout)
        .unwrap();
    note(first.world_mut(), area, "kept");
    save_now(&mut first);
    let writes = backend.writes();
    assert!(backend.text("world").unwrap().contains("kept"));
    save_now(&mut first);
    assert_eq!(backend.writes(), writes, "unchanged data isn't rewritten");

    let mut second = app();
    second.add_store::<WorkbenchStore>(backend.clone());
    second.update();
    let world = second.world_mut();
    let layout = world.layouts().active().unwrap();
    let area = world.get::<LayoutAreas>(layout).unwrap().entities()[0];
    assert_eq!(texts(world, area), ["kept"]);
}

#[test]
fn saved_data_that_cant_be_restored_is_never_overwritten() {
    let backend = MemoryStore::default();
    backend.write("world", b"not a world").unwrap();
    let mut app = app();
    app.add_store::<WorkbenchStore>(backend.clone());
    app.update();
    assert!(
        app.world()
            .resource::<Stores>()
            .error::<WorkbenchStore>()
            .is_some()
    );
    let reported = app
        .world()
        .resource::<crate::Notifications>()
        .latest()
        .unwrap();
    assert!(reported.text.starts_with("Couldn't restore saved world"));
    crate::tests::layout(app.world_mut());
    save_now(&mut app);
    assert_eq!(backend.text("world").unwrap(), "not a world");
}
