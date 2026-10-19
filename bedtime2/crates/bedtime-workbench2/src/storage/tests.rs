use super::*;

#[derive(Component, Default, Debug, PartialEq)]
struct Draft(String);
struct Notes;
impl Panel for Notes {
    const KEY: &'static str = "test.notes";
    const LABEL: &'static str = "Notes";
    fn initialize(entity: &mut EntityWorldMut<'_>) {
        entity.entry::<Draft>().or_default();
    }
    #[cfg(feature = "ui")]
    fn draw(
        _: &mut bedtime_ui2::egui::Ui,
        _: &mut World,
        _: &bedtime_ui2::UiContext<'_>,
        _: Entity,
    ) {
    }
}
struct Inspector;
impl Panel for Inspector {
    const KEY: &'static str = "test.inspector";
    const LABEL: &'static str = "Inspector";
    #[cfg(feature = "ui")]
    fn draw(
        _: &mut bedtime_ui2::egui::Ui,
        _: &mut World,
        _: &bedtime_ui2::UiContext<'_>,
        _: Entity,
    ) {
    }
}
fn app() -> App {
    let mut app = App::new();
    app.register_panel_type::<Notes>()
        .register_panel_type::<Inspector>()
        .add_plugins(WorkbenchCorePlugin);
    app
}
fn compose(world: &mut World) -> (Entity, Entity, Entity, Entity) {
    let workspace = world
        .spawn((
            Workspace {
                name: "Project".into(),
                ..default()
            },
            ActiveWorkspace,
        ))
        .id();
    let layout = world
        .spawn((
            Layout {
                name: "Code".into(),
                ..default()
            },
            InWorkspace(workspace),
            ActiveLayout,
        ))
        .id();
    let area = world.spawn((Area::default(), InLayout(layout))).id();
    let panel = world
        .spawn((
            PanelInstance::new(Notes::KEY),
            InArea(area),
            Draft("Keep me".into()),
        ))
        .id();
    (workspace, layout, area, panel)
}
fn events(world: &mut World) -> Vec<WorkspaceChanged> {
    world
        .resource_mut::<Messages<WorkspaceChanged>>()
        .drain()
        .collect()
}

#[test]
fn commands_composition_initializes_existing_entities_and_preserves_supplied_state() {
    let mut app = app();
    app.add_systems(Startup, |mut commands: Commands| {
        let workspace = commands.spawn((Workspace::default(), ActiveWorkspace)).id();
        let layout = commands
            .spawn((Layout::default(), InWorkspace(workspace), ActiveLayout))
            .id();
        let area = commands.spawn((Area::default(), InLayout(layout))).id();
        commands.spawn((
            PanelInstance::new(Notes::KEY),
            InArea(area),
            Draft("Supplied".into()),
        ));
    });
    app.update();
    let world = app.world_mut();
    let (panel, draft, owner) = world
        .query::<(Entity, &Draft, &InWorkspace)>()
        .single(world)
        .unwrap();
    assert_eq!(draft.0, "Supplied");
    assert!(world.get::<InActiveWorkspace>(panel).is_some());
    assert!(world.get::<WorkspaceWork>(owner.0).is_some());
    #[cfg(feature = "ui")]
    assert!(world.get::<crate::frontend::PanelRenderer>(panel).is_some());
    app.update();
    assert_eq!(
        app.world_mut().query::<&Draft>().iter(app.world()).count(),
        1
    );
}

#[test]
fn empty_workbench_workspace_and_layout_are_valid_and_remain_empty() {
    let mut app = app();
    app.update();
    assert_eq!(Workspaces::new(app.world_mut()).selected(), None);
    let workspace = app
        .world_mut()
        .spawn((Workspace::default(), ActiveWorkspace))
        .id();
    app.update();
    assert_eq!(active_layout(app.world(), workspace), None);
    assert!(
        capture_workspace(app.world(), workspace)
            .unwrap()
            .layouts
            .is_empty()
    );
    let layout = app
        .world_mut()
        .spawn((Layout::default(), InWorkspace(workspace), ActiveLayout))
        .id();
    app.update();
    let snapshot = capture_layout(app.world(), layout).unwrap();
    assert!(snapshot.areas.is_empty());
    assert!(
        Workbench::new(app.world_mut())
            .create_panel(layout, "No destination")
            .is_err()
    );
    app.world_mut().despawn(workspace);
    app.update();
    assert_eq!(Workspaces::new(app.world_mut()).selected(), None);
    assert!(app.world().get_entity(layout).is_err());
}

#[test]
fn markers_are_authoritative_and_active_layout_is_per_workspace() {
    let mut app = app();
    let (a, layout, _, panel) = compose(app.world_mut());
    let b = app.world_mut().spawn(Workspace::default()).id();
    let other = app
        .world_mut()
        .spawn((Layout::default(), InWorkspace(b), ActiveLayout))
        .id();
    app.update();
    assert_eq!(active_layout(app.world(), a), Some(layout));
    assert_eq!(active_layout(app.world(), b), Some(other));
    app.world_mut().entity_mut(a).remove::<ActiveWorkspace>();
    app.world_mut().entity_mut(b).insert(ActiveWorkspace);
    app.update();
    assert!(app.world().get::<InActiveWorkspace>(panel).is_none());
    assert!(app.world().get::<InActiveWorkspace>(other).is_some());
    app.world_mut().entity_mut(other).remove::<ActiveLayout>();
    app.update();
    assert_eq!(active_layout(app.world(), b), None);
}

#[test]
fn duplicate_active_markers_and_dangling_parents_fail_clearly() {
    let mut app = app();
    let (workspace, _, _, _) = compose(app.world_mut());
    let other = app
        .world_mut()
        .spawn((Workspace::default(), ActiveWorkspace))
        .id();
    assert!(
        reconcile_workbench(app.world_mut())
            .unwrap_err()
            .contains("Multiple ActiveWorkspace")
    );
    app.world_mut()
        .entity_mut(other)
        .remove::<ActiveWorkspace>();
    let duplicate = app
        .world_mut()
        .spawn((Layout::default(), InWorkspace(workspace), ActiveLayout))
        .id();
    assert!(
        reconcile_workbench(app.world_mut())
            .unwrap_err()
            .contains("Multiple ActiveLayout")
    );
    app.world_mut().despawn(duplicate);
    let orphan = app.world_mut().spawn(Area::default()).id();
    assert!(
        reconcile_workbench(app.world_mut())
            .unwrap_err()
            .contains("Area requires InLayout")
    );
    app.world_mut().despawn(orphan);
    reconcile_workbench(app.world_mut()).unwrap();
}

#[test]
fn workspace_roundtrip_keeps_geometry_panel_types_tab_order_and_selection() {
    let mut app = app();
    let (workspace, layout, area, first) = compose(app.world_mut());
    let second = app
        .world_mut()
        .spawn((PanelInstance::new(Inspector::KEY), InArea(area)))
        .id();
    app.update();
    app.world_mut().get_mut::<Area>(area).unwrap().selected = Some(second);
    let mut w = Workbench::new(app.world_mut());
    let right = w
        .split_area(area, SplitAxis::Horizontal, 4000, [100, 100])
        .unwrap();
    let kind = w
        .world_mut()
        .query::<(Entity, &PanelType)>()
        .iter(w.world())
        .find(|(_, p)| p.key == Notes::KEY)
        .unwrap()
        .0;
    let third = w.open_panel(kind, right).unwrap();
    w.world_mut()
        .get_mut::<PanelInstance>(third)
        .unwrap()
        .preview = true;
    let empty = w.create_layout("Empty tabs").unwrap();
    let mut w = Workbench::with_workspace(w.world_mut(), WorkspaceId(workspace)).unwrap();
    w.activate_layout(layout).unwrap();
    app.update();
    app.world_mut()
        .get_mut::<Workspace>(workspace)
        .unwrap()
        .background_updates = false;
    let saved = capture_workspace(app.world(), workspace).unwrap();
    let json = serde_json::to_string(&saved).unwrap();
    let decoded: WorkspaceSnapshot = serde_json::from_str(&json).unwrap();
    app.world_mut().despawn(workspace);
    let loaded = restore_workspace(app.world_mut(), &decoded).unwrap();
    app.world_mut()
        .entity_mut(loaded.workspace)
        .insert(ActiveWorkspace);
    app.update();
    let loaded_events: Vec<_> = events(app.world_mut())
        .into_iter()
        .filter(|e| e.workspace == loaded.workspace)
        .collect();
    assert_eq!(loaded_events.len(), 1);
    assert_eq!(loaded_events[0].change, WorkspaceChange::Created);
    assert_eq!(
        capture_workspace(app.world(), loaded.workspace).unwrap(),
        saved
    );
    assert_ne!(loaded.layouts[0].layout, layout);
    assert_ne!(loaded.layouts[0].areas[0].panels[0], first);
    assert!(app.world().get_entity(empty).is_err());
    assert_eq!(
        app.world()
            .get::<Draft>(loaded.layouts[0].areas[0].panels[0]),
        Some(&Draft::default())
    );
    assert_eq!(loaded.layouts.len(), 2);
}

#[test]
fn layout_templates_create_fresh_instances_and_reset_preserves_root_metadata() {
    #[derive(Component)]
    struct Slot(u8);
    let mut app = app();
    let (workspace, layout, _, old_panel) = compose(app.world_mut());
    app.update();
    let template = capture_layout(app.world(), layout).unwrap();
    let fresh = restore_layout(app.world_mut(), workspace, &template).unwrap();
    assert_ne!(fresh.layout, layout);
    assert_ne!(fresh.areas[0].panels[0], old_panel);
    app.world_mut().entity_mut(layout).insert(Slot(3));
    let reset = replace_layout(app.world_mut(), layout, &template).unwrap();
    app.update();
    assert_eq!(reset.layout, layout);
    assert_eq!(app.world().get::<Slot>(layout).unwrap().0, 3);
    assert!(app.world().get::<ActiveLayout>(layout).is_some());
    assert!(app.world().get_entity(old_panel).is_err());
    assert_eq!(capture_layout(app.world(), layout).unwrap(), template);
}

#[test]
fn malformed_or_unknown_saved_data_cannot_partially_mutate_the_world() {
    let mut app = app();
    let (workspace, layout, _, _) = compose(app.world_mut());
    app.update();
    let saved = capture_workspace(app.world(), workspace).unwrap();
    let mut bad = Vec::new();
    let mut s = saved.clone();
    s.version = 999;
    bad.push(s);
    let mut s = saved.clone();
    s.active_layout = Some(88);
    bad.push(s);
    let mut s = saved.clone();
    s.layouts[0].focused_area = Some(88);
    bad.push(s);
    let mut s = saved.clone();
    s.layouts[0].areas[0].selected_panel = Some(88);
    bad.push(s);
    let mut s = saved.clone();
    s.layouts[0].areas[0].panels[0].key = "missing.panel".into();
    bad.push(s);
    let mut s = saved.clone();
    s.layouts[0].areas[0].rect.max[0] = 5000;
    bad.push(s);
    let mut s = saved.clone();
    let duplicate = s.layouts[0].areas[0].clone();
    s.layouts[0].areas.push(duplicate);
    bad.push(s);
    let mut s = saved.clone();
    s.layouts[0].areas[0].rect.min[0] = i32::MIN;
    bad.push(s);
    let count = app.world().iter_entities().count();
    for snapshot in bad {
        assert!(restore_workspace(app.world_mut(), &snapshot).is_err());
        assert_eq!(app.world().iter_entities().count(), count);
        assert_eq!(capture_workspace(app.world(), workspace).unwrap(), saved);
    }
    let mut invalid = saved.layouts[0].clone();
    invalid.version = 99;
    assert!(replace_layout(app.world_mut(), layout, &invalid).is_err());
    assert_eq!(capture_workspace(app.world(), workspace).unwrap(), saved);
}

#[test]
fn workspace_change_messages_coalesce_direct_edits_and_include_removals() {
    let mut app = app();
    let (workspace, layout, area, panel) = compose(app.world_mut());
    app.update();
    let initial = events(app.world_mut());
    assert_eq!(initial.len(), 1);
    assert_eq!(initial[0].change, WorkspaceChange::Created);
    app.update();
    assert!(events(app.world_mut()).is_empty());
    app.world_mut()
        .get_mut::<Workspace>(workspace)
        .unwrap()
        .name = "Renamed".into();
    app.world_mut().get_mut::<Layout>(layout).unwrap().name = "Renamed too".into();
    app.world_mut()
        .get_mut::<PanelInstance>(panel)
        .unwrap()
        .preview = true;
    app.update();
    let changed = events(app.world_mut());
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].workspace, workspace);
    assert_eq!(changed[0].change, WorkspaceChange::Modified);
    app.world_mut().despawn(panel);
    app.update();
    assert_eq!(app.world().get::<Area>(area).unwrap().selected, None);
    assert_eq!(events(app.world_mut()).len(), 1);
    app.world_mut().despawn(workspace);
    app.update();
    let removed = events(app.world_mut());
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0].change, WorkspaceChange::Removed);
}

#[cfg(feature = "ui")]
#[test]
fn workbench_surface_draws_without_workspace_or_layout() {
    let mut app = App::new();
    app.add_plugins(WorkbenchUiPlugin);
    app.world_mut()
        .spawn((Window::default(), WorkbenchSurface::default()));
    app.world_mut().spawn((Camera2d, WorkbenchCamera));
    app.update();
    let workspace = app
        .world_mut()
        .spawn((Workspace::default(), ActiveWorkspace))
        .id();
    app.update();
    let layout = app
        .world_mut()
        .spawn((Layout::default(), InWorkspace(workspace), ActiveLayout))
        .id();
    app.update();
    app.world_mut().despawn(layout);
    app.update();
    app.world_mut().despawn(workspace);
    app.update();
}

#[test]
fn direct_tab_reparenting_updates_selection_membership_and_gesture_revision() {
    let mut app = app();
    let (_, layout, area, panel) = compose(app.world_mut());
    app.update();
    let right = Workbench::new(app.world_mut())
        .split_area(area, SplitAxis::Horizontal, 5000, [100, 100])
        .unwrap();
    app.update();
    let stamp = Workbench::new(app.world_mut()).stamp();
    app.world_mut().entity_mut(panel).insert(InArea(right));
    app.update();
    assert_eq!(app.world().get::<Area>(area).unwrap().selected, None);
    assert_eq!(
        app.world().get::<Area>(right).unwrap().selected,
        Some(panel)
    );
    assert_ne!(Workbench::new(app.world_mut()).stamp(), stamp);
    assert_eq!(
        app.world().get::<Layout>(layout).unwrap().focused_area,
        Some(area)
    );
}

#[test]
fn direct_geometry_edits_invalidate_gestures_and_notify_in_the_same_frame() {
    #[derive(Resource, Default)]
    struct Observed(Vec<WorkspaceChanged>);
    let mut app = app();
    let (_, _, area, _) = compose(app.world_mut());
    app.init_resource::<Observed>().add_systems(
        PostUpdate,
        (|mut events: MessageReader<WorkspaceChanged>, mut seen: ResMut<Observed>| {
            seen.0.extend(events.read().copied());
        })
        .after(WorkspaceSystems::Notify),
    );
    app.update();
    let right = Workbench::new(app.world_mut())
        .split_area(area, SplitAxis::Horizontal, 5000, [100, 100])
        .unwrap();
    app.update();
    app.world_mut().resource_mut::<Observed>().0.clear();
    let stamp = Workbench::new(app.world_mut()).stamp();
    app.world_mut().get_mut::<Area>(area).unwrap().rect.max[0] = 4000;
    app.world_mut().get_mut::<Area>(right).unwrap().rect.min[0] = 4000;
    app.update();
    assert_ne!(Workbench::new(app.world_mut()).stamp(), stamp);
    assert_eq!(app.world().resource::<Observed>().0.len(), 1);
}

#[test]
fn raw_graph_can_be_built_before_registration_with_no_extra_initialization_steps() {
    let mut app = App::new();
    app.add_plugins(WorkbenchCorePlugin);
    let (_, _, _, panel) = compose(app.world_mut());
    app.register_panel_type::<Notes>();
    app.update();
    assert_eq!(app.world().get::<Draft>(panel).unwrap().0, "Keep me");
}
