use super::*;
use crate::tests::TestPanels;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

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

struct OtherStore;
impl Store for OtherStore {}

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
    panels_in_area(world, area)
        .iter()
        .map(|&panel| world.get::<Note>(panel).unwrap().text.clone())
        .collect()
}

#[test]
fn a_round_trip_restores_layouts_tabs_selection_focus_and_panel_data() {
    let mut source = app();
    let world = source.world_mut();
    let layout = crate::tests::layout(world);
    let mut workbench = Workbench::new(world);
    let left = workbench.destination_area(layout).unwrap();
    let right = workbench
        .split_area(left, SplitAxis::Horizontal, 6000, [100, 100])
        .unwrap();
    workbench.create_layout("Second").unwrap();
    let world = workbench.world_mut();
    note(world, left, "a");
    let b = note(world, left, "b");
    note(world, left, "c");
    note(world, right, "d");
    // A panel with nothing persisted can't be rebuilt, so it's left out.
    Workbench::new(world)
        .create_panel_in_area(left, "Bare")
        .unwrap();
    world.get_mut::<Area>(left).unwrap().selected = Some(b);
    world.get_mut::<Layout>(layout).unwrap().focused_area = Some(right);
    let saved = capture::<WorkbenchStore>(world).unwrap();

    let mut target = app();
    let world = target.world_mut();
    restore::<WorkbenchStore>(world, &saved).unwrap();
    let names: Vec<_> = layouts(world)
        .iter()
        .map(|&l| world.get::<Layout>(l).unwrap().name.clone())
        .collect();
    assert_eq!(names, ["Test", "Second"]);
    let layout = active_layout(world).unwrap();
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
    let restored = panels_in_area(world, left)[0];
    assert!(world.get::<Runtime>(restored).is_some());
    assert_eq!(world.get::<PanelInstance>(restored).unwrap().title, "Note");
}

#[test]
fn layouts_are_only_restored_into_an_empty_workbench() {
    let mut source = app();
    crate::tests::layout(source.world_mut());
    let saved = capture::<WorkbenchStore>(source.world_mut()).unwrap();
    let mut target = app();
    crate::tests::layout(target.world_mut());
    assert!(restore::<WorkbenchStore>(target.world_mut(), &saved).is_err());
}

#[test]
fn resources_are_saved_in_their_store() {
    let mut source = app();
    source.world_mut().insert_resource(Counter(7));
    assert!(
        capture::<WorkbenchStore>(source.world_mut())
            .unwrap()
            .contains("Counter")
    );

    source.persist_in::<Counter, OtherStore>();
    let world = source.world_mut();
    assert!(
        !capture::<WorkbenchStore>(world)
            .unwrap()
            .contains("Counter")
    );
    let saved = capture::<OtherStore>(world).unwrap();

    let mut target = app();
    target.persist_in::<Counter, OtherStore>();
    assert!(restore::<WorkbenchStore>(target.world_mut(), &saved).is_err());
    restore::<OtherStore>(target.world_mut(), &saved).unwrap();
    assert_eq!(target.world().resource::<Counter>(), &Counter(7));
}

#[test]
fn references_to_entities_that_arent_saved_are_refused() {
    let mut app = app();
    let world = app.world_mut();
    let runtime_only = world.spawn_empty().id();
    world.spawn(Pointer(runtime_only));
    let error = capture::<WorkbenchStore>(world).unwrap_err();
    assert!(error.contains("Pointer"));
}

#[test]
fn a_layout_template_builds_independent_copies_of_one_layout() {
    let mut source = app();
    let world = source.world_mut();
    let layout = crate::tests::layout(world);
    let mut workbench = Workbench::new(world);
    let left = workbench.destination_area(layout).unwrap();
    let right = workbench
        .split_area(left, SplitAxis::Horizontal, 6000, [100, 100])
        .unwrap();
    // Other layouts aren't part of the template.
    workbench.create_layout("Other").unwrap();
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
    let template = capture_layout(world, layout).unwrap();
    assert!(!template.as_str().contains("ActiveLayout"));
    assert!(capture_layout(world, left).is_err());

    // Into another workbench, after its layouts, and not made active.
    let mut target = app();
    let world = target.world_mut();
    let existing = crate::tests::layout(world);
    let copies = [
        instantiate_layout(world, &template).unwrap(),
        instantiate_layout(world, &template).unwrap(),
    ];
    assert_eq!(layouts(world), [existing, copies[0], copies[1]]);
    assert_eq!(active_layout(world), Some(existing));
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
        let d = panels_in_area(world, right)[0];
        assert_eq!(world.get::<Pointer>(d).unwrap().0, selected);
    }
    // The copies don't share panels.
    let panel =
        |copy| panels_in_area(world, world.get::<LayoutAreas>(copy).unwrap().entities()[0])[0];
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
    let area = Workbench::new(world).destination_area(layout).unwrap();
    note(world, area, "kept");
    let template = capture_layout(world, layout).unwrap();
    world.insert_resource(Defaults(Some(template.clone())));
    let saved = capture::<WorkbenchStore>(world).unwrap();
    // Nested, not an escaped string.
    assert!(saved.contains("\"kept\""), "{saved}");
    assert!(!saved.contains("\\\""), "{saved}");

    let mut target = app();
    target
        .register_type::<Defaults>()
        .init_resource::<Defaults>();
    restore::<WorkbenchStore>(target.world_mut(), &saved).unwrap();
    let world = target.world_mut();
    let restored = world.resource::<Defaults>().0.clone().unwrap();
    assert_eq!(restored, template);
    let copy = instantiate_layout(world, &restored).unwrap();
    let area = world.get::<LayoutAreas>(copy).unwrap().entities()[0];
    assert_eq!(texts(world, area), ["kept"]);
}

#[test]
fn a_bad_layout_template_adds_nothing() {
    let mut source = app();
    let world = source.world_mut();
    crate::tests::layout(world);
    Workbench::new(world).create_layout("Second").unwrap();
    let whole = capture::<WorkbenchStore>(world).unwrap();

    let mut target = app();
    let world = target.world_mut();
    // A whole workbench save holds two layouts and a resource.
    let whole = LayoutTemplate::from_ron(whole).unwrap();
    let error = instantiate_layout(world, &whole).unwrap_err();
    assert!(error.contains("resources"), "{error}");
    assert!(LayoutTemplate::from_ron("not ron (".into()).is_err());
    let not_a_world = LayoutTemplate::from_ron("(1, 2)".into()).unwrap();
    assert!(instantiate_layout(world, &not_a_world).is_err());
    assert!(layouts(world).is_empty());
    assert_eq!(world.query::<&Area>().iter(world).count(), 0);
}

/// An in-memory backend that counts writes and versions each entry.
#[derive(Clone, Default)]
pub(crate) struct MemoryBackend(Arc<Mutex<MemoryState>>);

#[derive(Default)]
struct MemoryState {
    entries: HashMap<String, (Vec<u8>, u64)>,
    writes: usize,
}

impl MemoryBackend {
    pub(crate) fn text(&self, entry: &str) -> Option<String> {
        let state = self.0.lock().unwrap();
        state
            .entries
            .get(entry)
            .map(|(bytes, _)| String::from_utf8(bytes.clone()).unwrap())
    }
    fn world(&self) -> Option<String> {
        self.text("world")
    }
    pub(crate) fn writes(&self) -> usize {
        self.0.lock().unwrap().writes
    }
}

impl StoreBackend for MemoryBackend {
    fn read(&self, entry: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .entries
            .get(entry)
            .map(|(b, _)| b.clone()))
    }
    fn write(&self, entry: &str, bytes: &[u8]) -> Result<(), String> {
        let mut state = self.0.lock().unwrap();
        let version = state.entries.get(entry).map_or(0, |(_, v)| v + 1);
        state
            .entries
            .insert(entry.into(), (bytes.to_vec(), version));
        state.writes += 1;
        Ok(())
    }
    fn version(&self, entry: &str) -> Result<Option<String>, String> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .entries
            .get(entry)
            .map(|(_, v)| v.to_string()))
    }
}

/// Saving runs about once a second, and always when the app exits.
pub(crate) fn save_now(app: &mut App) {
    app.world_mut().write_message(AppExit::Success);
    app.update();
    app.world_mut().resource_mut::<Messages<AppExit>>().clear();
}

#[test]
fn stores_save_changes_and_restore_them_at_startup() {
    let backend = MemoryBackend::default();
    let mut first = app();
    first.add_store::<WorkbenchStore>(backend.clone());
    first.update();
    let layout = crate::tests::layout(first.world_mut());
    let area = Workbench::new(first.world_mut())
        .destination_area(layout)
        .unwrap();
    note(first.world_mut(), area, "kept");
    save_now(&mut first);
    let writes = backend.writes();
    assert!(backend.world().unwrap().contains("kept"));
    save_now(&mut first);
    assert_eq!(backend.writes(), writes, "unchanged data isn't rewritten");

    let mut second = app();
    second.add_store::<WorkbenchStore>(backend.clone());
    second.update();
    let world = second.world_mut();
    let layout = active_layout(world).unwrap();
    let area = world.get::<LayoutAreas>(layout).unwrap().entities()[0];
    assert_eq!(texts(world, area), ["kept"]);
}

#[test]
fn saved_data_that_cant_be_restored_is_never_overwritten() {
    let backend = MemoryBackend::default();
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
    crate::tests::layout(app.world_mut());
    save_now(&mut app);
    assert_eq!(backend.world().unwrap(), "not a world");
}

#[test]
fn directory_stores_keep_one_file_per_entry() {
    let directory = std::env::temp_dir().join(format!("bedtime-store-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    let store = DirectoryStore::new(directory.join("nested"));
    assert_eq!(store.read("world").unwrap(), None);
    assert_eq!(store.version("world").unwrap(), None);
    store.write("world", b"first").unwrap();
    assert_eq!(store.read("world").unwrap().as_deref(), Some(&b"first"[..]));
    assert!(directory.join("nested/world.ron").exists());
    assert!(store.version("world").unwrap().is_some());
    store.write("world", b"second").unwrap();
    assert_eq!(
        store.read("world").unwrap().as_deref(),
        Some(&b"second"[..])
    );
    // Nothing is left behind from the atomic write.
    let files = std::fs::read_dir(directory.join("nested")).unwrap().count();
    assert_eq!(files, 1);
    std::fs::remove_dir_all(&directory).unwrap();
}
