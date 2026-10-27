use super::*;

#[derive(Component, Reflect, Clone)]
#[reflect(Component, WorkbenchPersist)]
#[require(PanelInstance=PanelInstance::of::<Self>(), Runtime)]
struct Note {
    text: String,
}
#[derive(Component, Default)]
struct Runtime;
impl Panel for Note {
    const LABEL: &'static str = "Note";
    #[cfg(feature = "ui")]
    fn draw(_: &mut bedtime_ui::egui::Ui, _: &mut World, _: &bedtime_ui::UiContext<'_>, _: Entity) {
    }
}
#[derive(Resource, Reflect, Clone, PartialEq, Debug)]
#[reflect(Resource, WorkbenchPersist)]
struct Settings(u32);
#[derive(Component, Reflect, Clone)]
#[reflect(Component, WorkbenchPersist)]
struct Link(#[entities] Entity);
#[derive(Component, Reflect, Clone)]
#[reflect(Component, WorkbenchPersist)]
struct Unmapped(Entity);
#[derive(Component, Reflect, Clone)]
#[reflect(Component)]
struct Unmarked(u32);

#[derive(Clone)]
struct ReflectGlobalPersist;
impl<T> CreateTypeData<T> for ReflectGlobalPersist {
    fn create_type_data(_: ()) -> Self {
        Self
    }
}
#[derive(Resource, Reflect, Clone, Debug, PartialEq)]
#[reflect(Resource, GlobalPersist)]
struct GlobalSettings(u32);
#[derive(Component, Reflect, Clone)]
#[reflect(Component, GlobalPersist)]
struct GlobalNote(String);

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(WorkbenchCorePlugin)
        .register_type::<Note>()
        .register_type::<Settings>()
        .register_type::<Link>()
        .register_type::<Unmapped>()
        .register_type::<Unmarked>()
        .register_type::<GlobalSettings>()
        .register_type::<GlobalNote>();
    app
}
fn graph(world: &mut World) -> (Entity, Entity) {
    let layout = Workbench::new(world).create_layout("Notes").unwrap();
    let area = Workbench::new(world).destination_area(layout).unwrap();
    let panel = world
        .spawn((
            Note {
                text: "saved".into(),
            },
            InArea(area),
            Unmarked(99),
        ))
        .id();
    Workbench::new(world).activate_layout(layout).unwrap();
    (layout, panel)
}
fn snapshot(world: &mut World) -> Snapshot {
    Workbench::capture_world::<ReflectWorkbenchPersist>(world)
        .unwrap()
        .serialize(world.resource::<AppTypeRegistry>())
        .unwrap()
}
fn decode(snapshot: &Snapshot, world: &World) -> Result<CapturedWorld, String> {
    CapturedWorld::deserialize(snapshot, world.resource::<AppTypeRegistry>())
}

#[test]
fn marked_data_remaps_references_and_rebuilds_runtime_without_a_catalog() {
    let mut source = app();
    let (_, panel) = graph(source.world_mut());
    let part = source.world_mut().spawn((Link(panel), InPanel(panel))).id();
    source.insert_resource(Settings(42));
    let saved = snapshot(source.world_mut());
    assert!(!saved.data.contains("Unmarked"));
    assert!(!saved.data.contains("Runtime"));
    assert!(!saved.data.contains("LayoutState"));
    let mut target = app();
    for _ in 0..30 {
        target.world_mut().spawn_empty();
    }
    let captured = decode(&saved, target.world()).unwrap();
    let ids = Workbench::restore_world(target.world_mut(), captured).unwrap();
    assert!(ids.iter().all(|e| *e != panel && *e != part));
    let restored = target
        .world_mut()
        .query_filtered::<Entity, With<Note>>()
        .single(target.world())
        .unwrap();
    assert!(target.world().get::<Runtime>(restored).is_some());
    assert_eq!(target.world().resource::<Settings>(), &Settings(42));
    assert_eq!(
        target.world().get::<PanelInstance>(restored).unwrap().key,
        Note::KEY
    );
    #[cfg(feature = "ui")]
    assert!(
        target
            .world()
            .get::<PanelInstance>(restored)
            .unwrap()
            .draw
            .is_some()
    );
    assert_eq!(
        target
            .world_mut()
            .query::<&Link>()
            .single(target.world())
            .unwrap()
            .0,
        restored
    );
    assert_eq!(
        target
            .world()
            .get::<InPanel>(
                ids.iter()
                    .copied()
                    .find(|e| target.world().get::<Link>(*e).is_some())
                    .unwrap()
            )
            .unwrap()
            .0,
        restored
    );
    assert_eq!(snapshot(target.world_mut()), saved);
}

#[test]
fn application_marker_selects_independent_components_and_resources() {
    let mut source = app();
    graph(source.world_mut());
    source.insert_resource(Settings(8));
    source.insert_resource(GlobalSettings(12));
    source.world_mut().spawn(GlobalNote("global".into()));
    let registry = source.world().resource::<AppTypeRegistry>().clone();
    let mut globals = Workbench::capture_world::<ReflectGlobalPersist>(source.world_mut()).unwrap();
    assert!(globals.layouts().is_empty());
    assert!(globals.resource::<Settings>().is_none());
    assert_eq!(globals.components::<GlobalNote>().len(), 1);
    // The application's filter affects detached data only.
    globals.dynamic.entities.clear();
    globals.replace_resource(GlobalSettings(99)).unwrap();
    let saved = globals.serialize(&registry).unwrap();
    assert_eq!(
        source.world().resource::<GlobalSettings>(),
        &GlobalSettings(12)
    );
    assert_eq!(
        source
            .world_mut()
            .query::<&GlobalNote>()
            .iter(source.world())
            .count(),
        1
    );
    assert!(CapturedWorld::<ReflectWorkbenchPersist>::deserialize(&saved, &registry).is_err());
    let globals = CapturedWorld::<ReflectGlobalPersist>::deserialize(&saved, &registry).unwrap();
    let mut target = app();
    Workbench::restore_world(target.world_mut(), globals).unwrap();
    assert_eq!(
        target.world().resource::<GlobalSettings>(),
        &GlobalSettings(99)
    );
    assert!(layouts(target.world()).is_empty());
}

#[test]
fn captured_component_values_can_be_edited_without_mutating_the_live_world() {
    let mut source = app();
    let (_, panel) = graph(source.world_mut());
    let mut captured =
        Workbench::capture_world::<ReflectWorkbenchPersist>(source.world_mut()).unwrap();
    let entity = captured
        .dynamic
        .entities
        .iter_mut()
        .find(|e| e.entity == panel)
        .unwrap();
    let note = entity
        .components
        .iter_mut()
        .find(|v| type_id(v.as_ref()) == Some(TypeId::of::<Note>()))
        .unwrap();
    *note = Box::new(Note {
        text: "edited snapshot".into(),
    });
    assert_eq!(source.world().get::<Note>(panel).unwrap().text, "saved");
    let saved = captured
        .serialize(source.world().resource::<AppTypeRegistry>())
        .unwrap();
    let mut target = app();
    let captured = decode(&saved, target.world()).unwrap();
    Workbench::restore_world(target.world_mut(), captured).unwrap();
    assert_eq!(
        target
            .world_mut()
            .query::<&Note>()
            .single(target.world())
            .unwrap()
            .text,
        "edited snapshot"
    );
}

#[test]
fn filtering_a_complete_layout_preserves_the_remaining_graph() {
    let mut source = app();
    let (layout, _) = graph(source.world_mut());
    let empty = Workbench::new(source.world_mut())
        .create_layout("Empty")
        .unwrap();
    let mut captured =
        Workbench::capture_world::<ReflectWorkbenchPersist>(source.world_mut()).unwrap();
    let areas = source.world().get::<LayoutAreas>(empty).unwrap().entities();
    captured
        .dynamic
        .entities
        .retain(|e| e.entity != empty && !areas.contains(&e.entity));
    captured
        .replace_resource(LayoutOrder(vec![layout]))
        .unwrap();
    let saved = captured
        .serialize(source.world().resource::<AppTypeRegistry>())
        .unwrap();
    let mut target = app();
    let captured = decode(&saved, target.world()).unwrap();
    Workbench::restore_world(target.world_mut(), captured).unwrap();
    assert_eq!(layouts(target.world()).len(), 1);
    assert_eq!(layouts(source.world()).len(), 2);
}

#[test]
fn dangling_references_after_filtering_fail_before_restore_mutates_destination() {
    let mut source = app();
    let (_, panel) = graph(source.world_mut());
    let mut captured =
        Workbench::capture_world::<ReflectWorkbenchPersist>(source.world_mut()).unwrap();
    captured.dynamic.entities.retain(|e| e.entity != panel);
    assert!(
        captured
            .serialize(source.world().resource::<AppTypeRegistry>())
            .is_err()
    );
    let mut target = app();
    let before = target.world().entities().len();
    assert!(Workbench::restore_world(target.world_mut(), captured).is_err());
    assert_eq!(target.world().entities().len(), before);
}

#[test]
fn reference_closure_and_mapping_are_required() {
    let mut source = app();
    let outside = source.world_mut().spawn_empty().id();
    let inside = source.world_mut().spawn(Link(outside)).id();
    assert!(
        Workbench::capture_world::<ReflectWorkbenchPersist>(source.world_mut())
            .unwrap()
            .serialize(source.world().resource::<AppTypeRegistry>())
            .is_err()
    );
    source
        .world_mut()
        .entity_mut(inside)
        .remove::<Link>()
        .insert(Unmapped(inside));
    assert!(
        Workbench::capture_world::<ReflectWorkbenchPersist>(source.world_mut())
            .unwrap()
            .serialize(source.world().resource::<AppTypeRegistry>())
            .is_err()
    );
}

#[test]
fn newer_unknown_unmarked_and_duplicate_data_are_rejected() {
    let mut source = app();
    graph(source.world_mut());
    let saved = snapshot(source.world_mut());
    let registry = source.world().resource::<AppTypeRegistry>().clone();
    let mut invalid = saved.clone();
    invalid.version += 1;
    assert!(CapturedWorld::<ReflectWorkbenchPersist>::deserialize(&invalid, &registry).is_err());
    invalid.version = VERSION - 1;
    assert!(CapturedWorld::<ReflectWorkbenchPersist>::deserialize(&invalid, &registry).is_err());
    invalid = saved.clone();
    invalid.data = invalid.data.replace("Note", "UnknownNote");
    assert!(CapturedWorld::<ReflectWorkbenchPersist>::deserialize(&invalid, &registry).is_err());
    let mut captured = decode(&saved, source.world()).unwrap();
    captured.dynamic.entities[0]
        .components
        .push(Box::new(Unmarked(7)));
    invalid.data = captured.dynamic.serialize(&registry.read()).unwrap();
    assert!(CapturedWorld::<ReflectWorkbenchPersist>::deserialize(&invalid, &registry).is_err());
    let mut captured = decode(&saved, source.world()).unwrap();
    captured
        .dynamic
        .resources
        .push(Box::new(LayoutOrder(vec![])));
    assert!(captured.serialize(&registry).is_err());
}

#[test]
fn invalid_geometry_and_nonempty_destination_preserve_existing_graph() {
    let mut source = app();
    graph(source.world_mut());
    let saved = snapshot(source.world_mut());
    let registry = source.world().resource::<AppTypeRegistry>().clone();
    let mut captured = decode(&saved, source.world()).unwrap();
    for e in &mut captured.dynamic.entities {
        if let Some(mut area) = value::<Area>(e) {
            area.rect.max = [-1, 0];
            e.components
                .retain(|v| type_id(v.as_ref()) != Some(TypeId::of::<Area>()));
            e.components.push(Box::new(area));
        }
    }
    assert!(captured.serialize(&registry).is_err());
    let mut target = app();
    let (layout, panel) = graph(target.world_mut());
    let captured = decode(&saved, target.world()).unwrap();
    assert!(Workbench::restore_world(target.world_mut(), captured).is_err());
    assert_eq!(active_layout(target.world()), Some(layout));
    assert!(target.world().get::<Note>(panel).is_some());
}

#[test]
fn validation_does_not_construct_required_runtime() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CONSTRUCTIONS: AtomicUsize = AtomicUsize::new(0);
    #[derive(Component)]
    struct CountedRuntime;
    impl Default for CountedRuntime {
        fn default() -> Self {
            CONSTRUCTIONS.fetch_add(1, Ordering::SeqCst);
            Self
        }
    }
    #[derive(Component, Reflect, Clone)]
    #[reflect(Component, WorkbenchPersist)]
    #[require(CountedRuntime)]
    struct Durable(u32);
    let mut source = app();
    source.register_type::<Durable>();
    source.world_mut().spawn(Durable(1));
    let before = CONSTRUCTIONS.load(Ordering::SeqCst);
    let saved = snapshot(source.world_mut());
    let mut target = app();
    target.register_type::<Durable>();
    let captured = decode(&saved, target.world()).unwrap();
    assert_eq!(CONSTRUCTIONS.load(Ordering::SeqCst), before);
    Workbench::restore_world(target.world_mut(), captured).unwrap();
    assert_eq!(CONSTRUCTIONS.load(Ordering::SeqCst), before + 1);
}

#[test]
fn concrete_panels_cannot_be_smuggled_onto_layout_entities() {
    let mut source = app();
    graph(source.world_mut());
    let mut captured =
        Workbench::capture_world::<ReflectWorkbenchPersist>(source.world_mut()).unwrap();
    let layout = captured
        .dynamic
        .entities
        .iter_mut()
        .find(|e| value::<Layout>(e).is_some())
        .unwrap();
    layout.components.push(Box::new(Note {
        text: "invalid role".into(),
    }));
    assert!(
        captured
            .serialize(source.world().resource::<AppTypeRegistry>())
            .is_err()
    );
}

#[test]
fn unsupported_panels_fail_encoding_instead_of_disappearing() {
    let mut source = app();
    let (_, panel) = graph(source.world_mut());
    source.world_mut().entity_mut(panel).remove::<Note>();
    assert!(
        Workbench::capture_world::<ReflectWorkbenchPersist>(source.world_mut())
            .unwrap()
            .serialize(source.world().resource::<AppTypeRegistry>())
            .is_err()
    );
}

#[test]
fn roundtrip_preserves_layout_order_geometry_focus_and_background_tab_selection() {
    let mut source = app();
    let (_, panel) = graph(source.world_mut());
    let left = source.world().get::<InArea>(panel).unwrap().0;
    let right = Workbench::new(source.world_mut())
        .split_area(left, SplitAxis::Horizontal, 3500, [1, 1])
        .unwrap();
    let moved = source
        .world_mut()
        .spawn((
            Note {
                text: "ordered first".into(),
            },
            InArea(left),
        ))
        .id();
    source.world_mut().spawn((
        Note {
            text: "right".into(),
        },
        InArea(right),
    ));
    Workbench::new(source.world_mut())
        .create_layout("Other")
        .unwrap();
    Workbench::new(source.world_mut())
        .move_tab(moved, left, 0)
        .unwrap();
    Workbench::new(source.world_mut())
        .focus_area(right)
        .unwrap();
    Workbench::new(source.world_mut())
        .select_panel(panel)
        .unwrap();
    let saved = snapshot(source.world_mut());
    let mut target = app();
    let captured = decode(&saved, target.world()).unwrap();
    Workbench::restore_world(target.world_mut(), captured).unwrap();
    assert_eq!(snapshot(target.world_mut()), saved);
    let layout = active_layout(target.world()).unwrap();
    assert_eq!(target.world().get::<Layout>(layout).unwrap().name, "Notes");
}

#[test]
fn application_relationship_hooks_rebuild_reverse_membership() {
    #[derive(Component, Reflect, Clone)]
    #[reflect(Component, GlobalPersist)]
    #[relationship(relationship_target = Children)]
    struct Parent(#[entities] Entity);
    #[derive(Component, Default)]
    #[relationship_target(relationship = Parent)]
    struct Children(Vec<Entity>);
    let mut source = app();
    source.register_type::<Parent>();
    let parent = source.world_mut().spawn(GlobalNote("parent".into())).id();
    source
        .world_mut()
        .spawn((GlobalNote("child".into()), Parent(parent)));
    let saved = Workbench::capture_world::<ReflectGlobalPersist>(source.world_mut())
        .unwrap()
        .serialize(source.world().resource::<AppTypeRegistry>())
        .unwrap();
    let mut target = app();
    target.register_type::<Parent>();
    let captured = CapturedWorld::<ReflectGlobalPersist>::deserialize(
        &saved,
        target.world().resource::<AppTypeRegistry>(),
    )
    .unwrap();
    Workbench::restore_world(target.world_mut(), captured).unwrap();
    let (parent, children) = target
        .world_mut()
        .query::<(Entity, &Children)>()
        .single(target.world())
        .unwrap();
    assert_eq!(children.0.len(), 1);
    assert_eq!(
        target.world().get::<Parent>(children.0[0]).unwrap().0,
        parent
    );
}

#[test]
fn an_empty_layout_without_a_reverse_collection_roundtrips() {
    let mut source = app();
    let layout = source.world_mut().spawn(Layout::default()).id();
    source
        .world_mut()
        .entity_mut(layout)
        .remove::<LayoutAreas>();
    let saved = snapshot(source.world_mut());
    let mut target = app();
    let captured = decode(&saved, target.world()).unwrap();
    Workbench::restore_world(target.world_mut(), captured).unwrap();
    assert_eq!(layouts(target.world()).len(), 1);
    assert!(
        target
            .world()
            .get::<LayoutAreas>(layouts(target.world())[0])
            .unwrap()
            .entities()
            .is_empty()
    );
    assert_eq!(snapshot(target.world_mut()), saved);
}

#[test]
fn runtime_only_panel_parts_are_excluded_and_resource_references_are_remapped() {
    use bevy::ecs::reflect::ReflectMapEntities;
    #[derive(Resource, Reflect, Clone, bevy::ecs::entity::MapEntities)]
    #[reflect(Resource, MapEntities, WorkbenchPersist)]
    struct Selected(#[entities] Entity);
    let mut source = app();
    source.register_type::<Selected>();
    let (_, panel) = graph(source.world_mut());
    let runtime = source.world_mut().spawn((InPanel(panel), Runtime)).id();
    source.insert_resource(Selected(panel));
    let captured = Workbench::capture_world::<ReflectWorkbenchPersist>(source.world_mut()).unwrap();
    assert!(
        captured
            .dynamic
            .entities
            .iter()
            .all(|e| e.entity != runtime)
    );
    let saved = captured
        .serialize(source.world().resource::<AppTypeRegistry>())
        .unwrap();
    let mut target = app();
    target.register_type::<Selected>();
    let captured = decode(&saved, target.world()).unwrap();
    Workbench::restore_world(target.world_mut(), captured).unwrap();
    let selected = target.world().resource::<Selected>().0;
    assert!(target.world().get::<Note>(selected).is_some());
    assert!(
        target
            .world_mut()
            .query::<&InPanel>()
            .iter(target.world())
            .next()
            .is_none()
    );
    assert_eq!(snapshot(target.world_mut()), saved);
}

#[test]
fn world_restore_window_closes_without_disabling_capture_or_layout_replacement() {
    let mut source = app();
    let (layout, _) = graph(source.world_mut());
    source.insert_resource(Settings(42));
    let saved = snapshot(source.world_mut());
    let layout_saved =
        Workbench::capture_layout::<ReflectWorkbenchPersist>(source.world_mut(), layout)
            .unwrap()
            .serialize(source.world().resource::<AppTypeRegistry>())
            .unwrap();
    let mut target = app();
    target.insert_resource(Settings(7));
    Workbench::finish_world_restore(target.world_mut());
    let capture = decode(&saved, target.world()).unwrap();
    assert!(Workbench::restore_world(target.world_mut(), capture).is_err());
    assert_eq!(target.world().resource::<Settings>(), &Settings(7));
    assert!(layouts(target.world()).is_empty());
    let empty = Workbench::new(target.world_mut())
        .create_layout("Target")
        .unwrap();
    let capture = CapturedLayout::<ReflectWorkbenchPersist>::deserialize(
        &layout_saved,
        target.world().resource::<AppTypeRegistry>(),
    )
    .unwrap();
    Workbench::replace_layout(target.world_mut(), empty, capture).unwrap();
    assert_eq!(target.world().resource::<Settings>(), &Settings(7));
    assert_eq!(
        Workbench::capture_world::<ReflectWorkbenchPersist>(target.world_mut())
            .unwrap()
            .components::<Note>()
            .len(),
        1
    );
}

#[test]
fn layout_replacement_rebuilds_owned_data_and_preserves_other_layouts_resources_and_documents() {
    let mut app = app();
    let (layout, panel) = graph(app.world_mut());
    let part = app.world_mut().spawn((Link(panel), InPanel(panel))).id();
    let (other, other_panel) = graph(app.world_mut());
    Workbench::new(app.world_mut())
        .activate_layout(layout)
        .unwrap();
    app.insert_resource(Settings(42));
    let document = open_memory_document(
        app.world_mut(),
        "unsaved".to_owned(),
        DocumentOptions::unsaved("Draft"),
    )
    .unwrap();
    bind_document_view(app.world_mut(), panel, document).unwrap();
    let capture =
        Workbench::capture_layout::<ReflectWorkbenchPersist>(app.world_mut(), layout).unwrap();
    assert!(capture.dynamic.resources.is_empty());
    assert_eq!(capture.components::<Note>().len(), 1);
    assert_eq!(capture.components::<InPanel>().len(), 1);
    let saved = capture
        .serialize(app.world().resource::<AppTypeRegistry>())
        .unwrap();
    app.world_mut().get_mut::<Note>(panel).unwrap().text = "changed".into();
    let capture = CapturedLayout::<ReflectWorkbenchPersist>::deserialize(
        &saved,
        app.world().resource::<AppTypeRegistry>(),
    )
    .unwrap();
    let replaced = Workbench::replace_layout(app.world_mut(), layout, capture).unwrap();
    assert_eq!(layouts(app.world()), &[replaced, other]);
    assert_eq!(active_layout(app.world()), Some(replaced));
    assert!(app.world().get_entity(layout).is_err());
    assert!(app.world().get_entity(panel).is_err());
    assert!(app.world().get_entity(part).is_err());
    assert_eq!(app.world().get::<Note>(other_panel).unwrap().text, "saved");
    assert_eq!(app.world().resource::<Settings>(), &Settings(42));
    assert_eq!(read_document(app.world(), document).unwrap(), "unsaved");
    let new_panel = Workbench::new(app.world_mut()).panels(replaced)[0];
    assert_eq!(app.world().get::<Note>(new_panel).unwrap().text, "saved");
    assert!(app.world().get::<Runtime>(new_panel).is_some());
    assert_eq!(
        app.world_mut()
            .query::<&Link>()
            .single(app.world())
            .unwrap()
            .0,
        new_panel
    );
    assert_eq!(
        Workbench::capture_layout::<ReflectWorkbenchPersist>(app.world_mut(), replaced)
            .unwrap()
            .serialize(app.world().resource::<AppTypeRegistry>())
            .unwrap(),
        saved
    );
}

#[test]
fn invalid_or_cross_scope_layout_data_and_drafts_preserve_the_live_layout() {
    let mut app = app();
    let (layout, panel) = graph(app.world_mut());
    let (_, foreign) = graph(app.world_mut());
    let registry = app.world().resource::<AppTypeRegistry>().clone();
    let mut capture =
        Workbench::capture_layout::<ReflectWorkbenchPersist>(app.world_mut(), layout).unwrap();
    capture.insert_component(panel, Link(foreign));
    assert!(capture.serialize(&registry).is_err());
    assert!(Workbench::replace_layout(app.world_mut(), layout, capture).is_err());
    assert!(app.world().get::<Note>(panel).is_some());
    let mut capture =
        Workbench::capture_layout::<ReflectWorkbenchPersist>(app.world_mut(), layout).unwrap();
    capture.dynamic.resources.push(Box::new(Settings(99)));
    assert!(Workbench::replace_layout(app.world_mut(), layout, capture).is_err());
    let mut capture =
        Workbench::capture_layout::<ReflectWorkbenchPersist>(app.world_mut(), layout).unwrap();
    let area = app.world().get::<InArea>(panel).unwrap().0;
    capture.insert_component(
        area,
        Area {
            rect: AreaRect {
                min: [0, 0],
                max: [-1, 1],
            },
            selected: Some(panel),
        },
    );
    assert!(Workbench::replace_layout(app.world_mut(), layout, capture).is_err());
    assert!(app.world().get::<Note>(panel).is_some());
    let document = open_memory_document(
        app.world_mut(),
        String::new(),
        DocumentOptions::loaded("Draft"),
    )
    .unwrap();
    bind_document_view(app.world_mut(), panel, document).unwrap();
    app.world_mut().entity_mut(panel).insert(DocumentDraft);
    let capture =
        Workbench::capture_layout::<ReflectWorkbenchPersist>(app.world_mut(), layout).unwrap();
    assert!(Workbench::replace_layout(app.world_mut(), layout, capture).is_err());
    assert!(app.world().get::<DocumentDraft>(panel).is_some());
}

#[test]
fn removing_a_panel_from_detached_layout_keeps_order_and_clears_owned_parts() {
    let mut app = app();
    let (layout, first) = graph(app.world_mut());
    let area = app.world().get::<InArea>(first).unwrap().0;
    let second = app
        .world_mut()
        .spawn((
            Note {
                text: "second".into(),
            },
            InArea(area),
        ))
        .id();
    app.world_mut().spawn((Link(second), InPanel(second)));
    let registry = app.world().resource::<AppTypeRegistry>().clone();
    let mut capture =
        Workbench::capture_layout::<ReflectWorkbenchPersist>(app.world_mut(), layout).unwrap();
    capture.remove_panel(second, &registry).unwrap();
    let saved = capture.serialize(&registry).unwrap();
    let capture =
        CapturedLayout::<ReflectWorkbenchPersist>::deserialize(&saved, &registry).unwrap();
    let root = Workbench::replace_layout(app.world_mut(), layout, capture).unwrap();
    let area = app.world().get::<LayoutAreas>(root).unwrap().entities()[0];
    let panels = panels_in_area(app.world(), area);
    assert_eq!(panels.len(), 1);
    assert_eq!(app.world().get::<Note>(panels[0]).unwrap().text, "saved");
    assert_eq!(
        app.world().get::<Area>(area).unwrap().selected,
        Some(panels[0])
    );
    assert_eq!(
        app.world_mut().query::<&Link>().iter(app.world()).count(),
        0
    );
}

#[test]
fn layout_replacement_cancels_retired_panel_work_without_shutting_down_the_session() {
    use std::time::{Duration, Instant};
    let mut app = app();
    let (layout, panel) = graph(app.world_mut());
    let task = Workbench::new(app.world_mut())
        .work()
        .spawn_task(Some(panel), "Old panel", |cancel| {
            while !cancel.wait(Duration::from_millis(5)) {}
        })
        .unwrap();
    let capture =
        Workbench::capture_layout::<ReflectWorkbenchPersist>(app.world_mut(), layout).unwrap();
    Workbench::replace_layout(app.world_mut(), layout, capture).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while matches!(task.status(), WorkStatus::Running | WorkStatus::Cancelling) {
        app.update();
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(task.status(), WorkStatus::Cancelled);
    assert!(!app.world().resource::<ManagedWork>().is_closing());
    Workbench::new(app.world_mut())
        .work()
        .spawn_task(None, "New session work", |_| 42)
        .unwrap();
}
