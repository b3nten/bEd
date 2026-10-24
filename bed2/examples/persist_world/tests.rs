use super::*;

fn app() -> App {
    let mut app = App::new();
    register(&mut app);
    app
}

#[test]
fn marked_state_structure_and_scoped_bindings_round_trip() {
    let mut source = app();
    seed(source.world_mut());
    source
        .world_mut()
        .resource_mut::<GlobalSettings>()
        .large_text = true;
    for mut runtime in source
        .world_mut()
        .query::<&mut RuntimeOnly>()
        .iter_mut(source.world_mut())
    {
        runtime.clicks = 99;
    }
    let original_notes: Vec<_> = source
        .world_mut()
        .query::<&Notes>()
        .iter(source.world())
        .map(|n| n.text.clone())
        .collect();
    let archive = capture_archive(source.world_mut()).unwrap();
    assert!(!archive.global.contains("RuntimeOnly"));
    assert!(!archive.workspaces[0].contains("RuntimeOnly"));
    assert!(!archive.workspaces[0].contains("PanelPresentation"));
    let text = ron::ser::to_string(&archive).unwrap();
    let mut target = app();
    // Force different destination entity IDs, and keep unrelated live data intact.
    let untouched = target
        .world_mut()
        .spawn(Preferences {
            label: "existing".into(),
            value: 123,
        })
        .id();
    for _ in 0..40 {
        target.world_mut().spawn_empty();
    }
    let parts = prepare_archive(
        ron::from_str(&text).unwrap(),
        target.world().resource::<AppTypeRegistry>(),
    )
    .unwrap();
    restore_archive(target.world_mut(), parts).unwrap();
    let world = target.world_mut();
    assert!(world.resource::<GlobalSettings>().large_text);
    assert_eq!(world.get::<Preferences>(untouched).unwrap().value, 123);
    let mut restored: Vec<_> = world
        .query::<&Notes>()
        .iter(world)
        .map(|n| n.text.clone())
        .collect();
    let mut expected = original_notes;
    expected.sort();
    restored.sort();
    assert_eq!(restored, expected);
    assert_eq!(world.query::<&RuntimeOnly>().iter(world).count(), 8);
    assert!(
        world
            .query::<&RuntimeOnly>()
            .iter(world)
            .all(|r| r.clicks == 0)
    );
    assert!(
        world
            .query::<&PanelInstance>()
            .iter(world)
            .all(|p| p.draw.is_some())
    );
    for (_, area, p) in world.query::<(Entity, &Area, &Preferences)>().iter(world) {
        assert_eq!(p.label, "area value");
        let panel = area.selected.unwrap();
        assert!(world.get::<Notes>(panel).unwrap().text.ends_with("tab 0"));
    }
    for layout in world.query::<&Layout>().iter(world) {
        let area = layout.focused_area.unwrap();
        assert_eq!(world.get::<Area>(area).unwrap().rect.min, [0, 0]);
    }
    let mut scopes = [0; 3];
    for (e, binding) in world.query::<(Entity, &BindingPreferences)>().iter(world) {
        if let Some(p) = world.get::<InPanel>(e) {
            scopes[2] += 1;
            assert_eq!(binding.target, Some(p.0));
            assert_eq!(
                world.get::<InWorkspace>(e).unwrap().0,
                world.get::<InWorkspace>(p.0).unwrap().0
            );
        } else if world.get::<InWorkspace>(e).is_some() {
            scopes[1] += 1;
        } else {
            scopes[0] += 1;
        }
    }
    assert_eq!(scopes, [1, 2, 8]);
    // A second save/load still works after temporary bridge components are gone.
    capture_archive(world).unwrap();
}

#[test]
fn a_workspace_partition_restores_without_global_or_other_workspace_data() {
    let mut source = app();
    seed(source.world_mut());
    let workspace = source
        .world_mut()
        .query_filtered::<Entity, With<Workspace>>()
        .iter(source.world())
        .next()
        .unwrap();
    let text = persistence::capture(source.world_mut(), Some(workspace)).unwrap();
    let mut target = app();
    let prepared =
        persistence::prepare(&text, target.world().resource::<AppTypeRegistry>(), true).unwrap();
    persistence::restore(target.world_mut(), prepared).unwrap();
    let world = target.world_mut();
    assert_eq!(world.query::<&Workspace>().iter(world).count(), 1);
    assert_eq!(world.query::<&Notes>().iter(world).count(), 4);
    assert_eq!(
        world
            .query::<&Preferences>()
            .iter(world)
            .filter(|p| p.label == "global entity")
            .count(),
        0
    );
}

#[test]
fn cross_partition_refs_and_malformed_input_are_rejected_without_changing_live_data() {
    let mut source = app();
    seed(source.world_mut());
    let workspace = source
        .world_mut()
        .query_filtered::<Entity, With<Workspace>>()
        .iter(source.world())
        .next()
        .unwrap();
    source.world_mut().spawn(BindingPreferences {
        command: "bad".into(),
        shortcut: "K".into(),
        target: Some(workspace),
    });
    assert!(
        persistence::capture(source.world_mut(), None)
            .unwrap_err()
            .contains("leaves")
    );
    let mut target = app();
    seed(target.world_mut());
    let before = target
        .world_mut()
        .query::<&Notes>()
        .iter(target.world())
        .count();
    assert!(
        persistence::prepare("broken", target.world().resource::<AppTypeRegistry>(), true).is_err()
    );
    assert_eq!(
        target
            .world_mut()
            .query::<&Notes>()
            .iter(target.world())
            .count(),
        before
    );
}

#[test]
fn executable_roundtrip() {
    roundtrip().unwrap();
}

#[test]
fn reflected_but_unmarked_data_is_not_captured_or_admitted() {
    #[derive(Component, Reflect)]
    #[reflect(Component)]
    struct Unmarked {
        secret_runtime_value: u32,
    }
    let mut source = app();
    source.register_type::<Unmarked>();
    seed(source.world_mut());
    source.world_mut().spawn(Unmarked {
        secret_runtime_value: 42,
    });
    let text = persistence::capture(source.world_mut(), None).unwrap();
    assert!(!text.contains("Unmarked"));
    let registry = source.world().resource::<AppTypeRegistry>().clone();
    // A hand-built Bevy payload must pass the persistence allowlist on load too.
    let entity = source
        .world_mut()
        .query_filtered::<Entity, With<Unmarked>>()
        .single(source.world())
        .unwrap();
    let payload =
        bevy_world_serialization::DynamicWorldBuilder::from_world(source.world(), &registry.read())
            .deny_all()
            .allow_component::<Unmarked>()
            .extract_entity(entity)
            .build()
            .serialize(&registry.read())
            .unwrap();
    assert!(
        persistence::prepare(&payload, &registry, false)
            .err()
            .unwrap()
            .contains("not persistence-enabled")
    );
}
