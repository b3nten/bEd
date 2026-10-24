use super::*;
use std::time::Duration;

fn app() -> App {
    let mut app = App::new();
    app.init_resource::<Assets<Image>>()
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<StandardMaterial>>()
        .init_resource::<Time>()
        .add_plugins((TransformPlugin, MascotPlugin));
    app
}

#[test]
fn animation_is_local_bounded_and_pauses_while_hidden() {
    let mut bed = MascotScene::new(MascotKind::Bed);
    let mut duck = MascotScene::new(MascotKind::Duck);
    bed.pressed = true;
    assert!(bed.advance(true, 0.02));
    assert_eq!(bed.impulse, 1.0);
    duck.advance(true, 0.04);
    assert_eq!(duck.impulse, 0.0);
    let time = bed.time;
    bed.advance(false, 600.0);
    assert_eq!(bed.time, time);
    bed.advance(true, 600.0);
    assert!(bed.time - time <= 0.1);
    assert!(bed.impulse < 1.0);
    bed.animations = false;
    bed.pointer = [1.0, -1.0];
    bed.pressed = true;
    assert!(bed.advance(true, 0.04), "Reduced motion keeps click audio");
    assert_eq!((bed.pointer, bed.impulse, bed.time), ([0.0; 2], 0.0, 0.0));
}

#[test]
fn scenes_are_isolated_fit_portrait_targets_and_retire_with_their_owner() {
    let mut app = app();
    let unrelated = app.world_mut().spawn(RenderLayers::layer(1)).id();
    let bed = app
        .world_mut()
        .spawn(MascotScene::new(MascotKind::Bed))
        .id();
    let duck = app
        .world_mut()
        .spawn(MascotScene::new(MascotKind::Duck))
        .id();
    app.update();
    let bed_scene = app.world().get::<Scene>(bed).unwrap().clone();
    let duck_scene = app.world().get::<Scene>(duck).unwrap().clone();
    assert!(!bed_scene.layers.intersects(&duck_scene.layers));
    assert!(!bed_scene.layers.intersects(&RenderLayers::layer(0)));
    assert!(!bed_scene.layers.intersects(&RenderLayers::layer(1)));
    assert!(
        !app.world()
            .get::<Camera>(bed_scene.camera)
            .unwrap()
            .is_active
    );
    for entity in [bed, duck] {
        let mut viewport = app.world_mut().get_mut::<UiViewport>(entity).unwrap();
        viewport.visible = true;
        viewport.physical_size = UVec2::new(120, 900);
    }
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(Duration::from_secs_f32(0.04));
    app.update();
    for scene in [&bed_scene, &duck_scene] {
        assert!(app.world().get::<Camera>(scene.camera).unwrap().is_active);
        let local = app.world().get::<Transform>(scene.camera).unwrap();
        assert!(local.translation.is_finite());
        assert_eq!(
            app.world()
                .get::<GlobalTransform>(scene.camera)
                .unwrap()
                .translation(),
            local.translation
        );
    }
    app.world_mut().get_mut::<UiViewport>(duck).unwrap().visible = false;
    app.update();
    assert!(
        !app.world()
            .get::<Camera>(duck_scene.camera)
            .unwrap()
            .is_active
    );
    app.world_mut().despawn(bed);
    app.update();
    assert!(!app.world().entities().contains(bed_scene.root));
    assert!(!app.world().entities().contains(bed_scene.camera));
    assert!(app.world().entities().contains(duck_scene.camera));
    assert!(app.world().entities().contains(unrelated));
    let another = app
        .world_mut()
        .spawn(MascotScene::new(MascotKind::Bed))
        .id();
    app.update();
    assert_eq!(
        app.world().get::<Scene>(another).unwrap().layers,
        bed_scene.layers
    );
    app.world_mut().entity_mut(duck).remove::<MascotScene>();
    app.update();
    assert!(app.world().get::<UiViewport>(duck).is_none());
    assert!(!app.world().entities().contains(duck_scene.camera));
}

#[test]
fn beds_share_geometry_but_theme_changes_are_local() {
    let mut app = app();
    let a = app
        .world_mut()
        .spawn(MascotScene::new(MascotKind::Bed))
        .id();
    let b = app
        .world_mut()
        .spawn(MascotScene::new(MascotKind::Bed))
        .id();
    app.update();
    let meshes = app.world().resource::<Assets<Mesh>>().len();
    let c = app
        .world_mut()
        .spawn(MascotScene::new(MascotKind::Bed))
        .id();
    app.update();
    assert_eq!(meshes, app.world().resource::<Assets<Mesh>>().len());
    for (entity, accent) in [(a, [1.0, 0.0, 0.0, 1.0]), (b, [0.0, 0.0, 1.0, 1.0])] {
        app.world_mut()
            .get_mut::<MascotScene>(entity)
            .unwrap()
            .accent = accent;
        app.world_mut()
            .get_mut::<UiViewport>(entity)
            .unwrap()
            .visible = true;
    }
    app.update();
    let materials: Vec<_> = app
        .world_mut()
        .query::<(&MeshMaterial3d<StandardMaterial>, &RenderLayers)>()
        .iter(app.world())
        .map(|(m, l)| (m.0.clone(), l.clone()))
        .collect();
    for (entity, expected) in [
        (a, Color::srgb(1.0, 0.0, 0.0)),
        (b, Color::srgb(0.0, 0.0, 1.0)),
    ] {
        let layer = &app.world().get::<Scene>(entity).unwrap().layers;
        assert!(
            materials
                .iter()
                .filter(|(_, l)| l == layer)
                .any(|(handle, _)| app
                    .world()
                    .resource::<Assets<StandardMaterial>>()
                    .get(handle)
                    .unwrap()
                    .base_color
                    == expected)
        );
    }
    assert!(app.world().get::<MascotScene>(c).unwrap().error().is_none());
}

#[cfg(feature = "workbench")]
#[test]
fn panel_metadata_and_scene_kind_exist_at_spawn_and_preserve_supplied_state() {
    use bedtime_workbench2::{Panel, PanelInstance};
    let mut world = World::new();
    let bed = world.spawn(BedPanel).id();
    assert_eq!(world.get::<PanelInstance>(bed).unwrap().key, BedPanel::KEY);
    assert_eq!(
        world.get::<MascotScene>(bed).unwrap().kind(),
        MascotKind::Bed
    );
    let mut supplied = MascotScene::new(MascotKind::Duck);
    supplied.time = 2.0;
    let duck = world.spawn((DuckPanel, supplied)).id();
    assert_eq!(world.get::<PanelInstance>(duck).unwrap().title, "Ducky");
    assert_eq!(world.get::<MascotScene>(duck).unwrap().time, 2.0);
}
