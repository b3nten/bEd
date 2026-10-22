use super::*;

fn workspace(world: &mut World) -> Entity {
    world
        .spawn(Workspace {
            background_updates: false,
            ..default()
        })
        .id()
}
fn contents(world: &mut World, owner: Entity) -> (Entity, Entity, Entity) {
    let layout = world.spawn((Layout::default(), InWorkspace(owner))).id();
    let area = world.spawn((Area::default(), InLayout(layout))).id();
    let panel = world.spawn((PanelInstance::default(), InArea(area))).id();
    (layout, area, panel)
}

#[test]
fn selection_and_membership_are_current_without_running_a_schedule() {
    let mut world = World::new();
    let a = workspace(&mut world);
    let b = workspace(&mut world);
    world.entity_mut(a).insert(ActiveWorkspace);
    let (layout, area, panel) = contents(&mut world, a);
    for entity in [layout, area, panel] {
        assert_eq!(world.get::<InWorkspace>(entity).unwrap().0, a);
        assert!(world.get::<InActiveWorkspace>(entity).is_some());
        assert!(world.get::<InRunningWorkspace>(entity).is_some());
    }
    world.entity_mut(a).remove::<ActiveWorkspace>();
    world.entity_mut(b).insert(ActiveWorkspace);
    for entity in [layout, area, panel] {
        assert!(world.get::<InActiveWorkspace>(entity).is_none());
        assert!(world.get::<InRunningWorkspace>(entity).is_none());
    }
    let member = world.spawn(InWorkspace(b)).id();
    assert!(world.get::<InActiveWorkspace>(member).is_some());
    world.entity_mut(member).insert(InWorkspace(a));
    assert!(world.get::<InActiveWorkspace>(member).is_none());
    world.entity_mut(member).insert(InWorkspace(b));
    world.entity_mut(member).remove::<InWorkspace>();
    assert!(world.get::<InActiveWorkspace>(member).is_none());
    assert!(world.get::<InRunningWorkspace>(member).is_none());
}

#[test]
fn reparenting_subtrees_updates_ownership_before_the_old_workspace_is_despawned() {
    for moved in ["layout", "area", "panel"] {
        let mut world = World::new();
        let a = workspace(&mut world);
        let b = workspace(&mut world);
        world.entity_mut(b).insert(ActiveWorkspace);
        let (layout, area, panel) = contents(&mut world, a);
        match moved {
            "layout" => {
                world.entity_mut(layout).insert(InWorkspace(b));
            }
            "area" => {
                let destination = world.spawn((Layout::default(), InWorkspace(b))).id();
                world.entity_mut(area).insert(InLayout(destination));
            }
            "panel" => {
                let (_, destination, _) = contents(&mut world, b);
                world.entity_mut(panel).insert(InArea(destination));
            }
            _ => unreachable!(),
        }
        assert_eq!(world.get::<InWorkspace>(panel).unwrap().0, b, "{moved}");
        assert!(world.get::<InActiveWorkspace>(panel).is_some(), "{moved}");
        world.despawn(a);
        assert!(world.get::<PanelInstance>(panel).is_some(), "{moved}");
        assert_eq!(world.get::<InWorkspace>(panel).unwrap().0, b);
    }
}

#[test]
fn commands_reparent_descendants_before_a_following_despawn() {
    let mut world = World::new();
    let a = workspace(&mut world);
    let b = workspace(&mut world);
    world.entity_mut(b).insert(ActiveWorkspace);
    let (layout, area, panel) = contents(&mut world, a);
    world.commands().entity(layout).insert(InWorkspace(b));
    world.commands().entity(a).despawn();
    world.flush();
    for entity in [layout, area, panel] {
        assert!(world.get_entity(entity).is_ok());
        assert_eq!(world.get::<InWorkspace>(entity).unwrap().0, b);
        assert!(world.get::<InActiveWorkspace>(entity).is_some());
    }
}

#[test]
fn late_parent_membership_and_detaching_parent_links_refresh_descendants() {
    let mut world = World::new();
    let owner = workspace(&mut world);
    world.entity_mut(owner).insert(ActiveWorkspace);
    let layout = world.spawn(Layout::default()).id();
    let area = world.spawn((Area::default(), InLayout(layout))).id();
    let panel = world.spawn((PanelInstance::default(), InArea(area))).id();
    assert!(world.get::<InWorkspace>(panel).is_none());
    world.entity_mut(layout).insert(InWorkspace(owner));
    assert!(world.get::<InActiveWorkspace>(panel).is_some());
    world.entity_mut(area).remove::<InLayout>();
    for entity in [area, panel] {
        assert!(world.get::<InWorkspace>(entity).is_none());
        assert!(world.get::<InActiveWorkspace>(entity).is_none());
    }
    world.entity_mut(area).insert(InLayout(layout));
    assert!(world.get::<InActiveWorkspace>(panel).is_some());
    world.entity_mut(panel).remove::<InArea>();
    assert!(world.get::<InWorkspace>(panel).is_none());
    assert!(world.get::<InActiveWorkspace>(panel).is_none());
}

#[test]
fn update_consumers_see_selection_changes_after_commands_apply() {
    #[derive(Component)]
    struct Member;
    #[derive(Resource, Default)]
    struct Checked(bool);
    let mut app = App::new();
    app.add_plugins(WorkbenchCorePlugin)
        .init_resource::<Checked>();
    let a = workspace(app.world_mut());
    let b = workspace(app.world_mut());
    app.world_mut().entity_mut(a).insert(ActiveWorkspace);
    let existing = app.world_mut().spawn((Member, InWorkspace(a))).id();
    app.add_systems(
        Update,
        (
            move |mut commands: Commands| {
                commands.entity(a).remove::<ActiveWorkspace>();
                commands.entity(b).insert(ActiveWorkspace);
                commands.entity(existing).insert(InWorkspace(b));
                commands.spawn((Member, InWorkspace(b)));
            },
            move |members: Query<&InWorkspace, (With<Member>, With<InActiveWorkspace>)>,
                  mut checked: ResMut<Checked>| {
                assert_eq!(members.iter().count(), 2);
                assert!(members.iter().all(|owner| owner.0 == b));
                checked.0 = true;
            },
        )
            .chain(),
    );
    app.update();
    assert!(app.world().resource::<Checked>().0);
}

#[derive(Component, Default)]
#[require(PanelInstance = PanelInstance::of::<Notes>(), LocalState)]
struct Notes {
    text: String,
}
#[derive(Component, Default)]
struct LocalState {
    collapsed: bool,
}
impl Panel for Notes {
    const KEY: &'static str = "test.notes";
    const LABEL: &'static str = "Notes";
    #[cfg(feature = "ui")]
    fn draw(
        _: &mut bedtime_ui2::egui::Ui,
        _: &mut World,
        _: &bedtime_ui2::UiContext<'_>,
        _: Entity,
    ) {
    }
}

#[test]
fn concrete_spawn_is_usable_and_selected_without_initialization_or_focus_changes() {
    let mut world = World::new();
    let owner = workspace(&mut world);
    let layout = world.spawn((Layout::default(), InWorkspace(owner))).id();
    let first = world.spawn((Area::default(), InLayout(layout))).id();
    let other = world.spawn((Area::default(), InLayout(layout))).id();
    assert_eq!(
        world.get::<Layout>(layout).unwrap().focused_area(),
        Some(first)
    );
    let panel = world
        .spawn((
            Notes {
                text: "supplied".into(),
            },
            LocalState { collapsed: true },
            InArea(other),
        ))
        .id();
    assert_eq!(world.get::<Notes>(panel).unwrap().text, "supplied");
    assert!(world.get::<LocalState>(panel).unwrap().collapsed);
    assert_eq!(world.get::<PanelInstance>(panel).unwrap().title, "Notes");
    #[cfg(feature = "ui")]
    assert!(world.get::<PanelInstance>(panel).unwrap().draw.is_some());
    assert_eq!(world.get::<InWorkspace>(panel).unwrap().0, owner);
    assert_eq!(
        world.get::<Area>(other).unwrap().selected_panel(),
        Some(panel)
    );
    assert_eq!(
        world.get::<Layout>(layout).unwrap().focused_area(),
        Some(first)
    );
    let next = world.spawn((Notes::default(), InArea(other))).id();
    assert_eq!(panels_in_area(&world, other), &[panel, next]);
    assert_eq!(
        world.get::<Area>(other).unwrap().selected_panel(),
        Some(next)
    );
    // Reinsertion is not a new attachment and must not steal tab selection.
    world.entity_mut(panel).insert(InArea(other));
    assert_eq!(
        world.get::<Area>(other).unwrap().selected_panel(),
        Some(next)
    );
}

#[test]
fn selected_tab_and_focused_area_fallbacks_are_immediate() {
    let mut world = World::new();
    let owner = workspace(&mut world);
    let (layout, area, first) = contents(&mut world, owner);
    let middle = world.spawn((Notes::default(), InArea(area))).id();
    let last = world.spawn((Notes::default(), InArea(area))).id();
    Workbench::for_entity(&mut world, area)
        .unwrap()
        .select_panel(middle)
        .unwrap();
    world.despawn(middle);
    assert_eq!(
        world.get::<Area>(area).unwrap().selected_panel(),
        Some(last)
    );
    world.despawn(last);
    assert_eq!(
        world.get::<Area>(area).unwrap().selected_panel(),
        Some(first)
    );
    let destination = world.spawn((Area::default(), InLayout(layout))).id();
    world.entity_mut(first).insert(InArea(destination));
    assert_eq!(world.get::<Area>(area).unwrap().selected_panel(), None);
    assert_eq!(
        world.get::<Area>(destination).unwrap().selected_panel(),
        Some(first)
    );
    assert_eq!(
        world.get::<Layout>(layout).unwrap().focused_area(),
        Some(area)
    );
    world.despawn(area);
    assert_eq!(
        world.get::<Layout>(layout).unwrap().focused_area(),
        Some(destination)
    );
}

#[test]
fn deferred_panel_spawns_select_in_applied_order_and_preserve_local_overrides() {
    let mut world = World::new();
    let owner = workspace(&mut world);
    let (_, area, _) = contents(&mut world, owner);
    let first = world
        .commands()
        .spawn((Notes::default(), InArea(area)))
        .id();
    let last = world
        .commands()
        .spawn((
            Notes {
                text: "last".into(),
            },
            InArea(area),
        ))
        .id();
    world.flush();
    assert_eq!(
        world.get::<Area>(area).unwrap().selected_panel(),
        Some(last)
    );
    assert_eq!(world.get::<Notes>(last).unwrap().text, "last");
    assert!(world.get::<LocalState>(first).is_some());
    assert_eq!(world.get::<InWorkspace>(last).unwrap().0, owner);
}
