//! Two independently owned scenes, without Workbench.
use bedtime_mascots::{Mascot, MascotKind, MascotPlugin, MascotScene};
use bedtime_ui::*;
use bevy::{prelude::*, window::PrimaryWindow};

fn main() -> AppExit {
    App::new()
        .add_plugins((
            DefaultPlugins.set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Bed and Ducky".into(),
                    resolution: (1000, 650).into(),
                    ..default()
                }),
                ..default()
            }),
            UiPlugin,
            MascotPlugin,
        ))
        .add_systems(Startup, setup)
        .run()
}

#[derive(Component)]
struct Scenes([Entity; 2]);
fn setup(world: &mut World) {
    let window = world
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(world)
        .unwrap();
    let theme = world.spawn(themes::dark()).id();
    world
        .entity_mut(window)
        .insert((UiSurface, UsesTheme(theme)));
    world.spawn((Camera2d, UiCamera(window)));
    let view = register_view(world, window, draw);
    let bed = world
        .spawn((MascotScene::new(MascotKind::Bed), ChildOf(view)))
        .id();
    let duck = world
        .spawn((MascotScene::new(MascotKind::Duck), ChildOf(view)))
        .id();
    world.entity_mut(view).insert((
        Transform::IDENTITY,
        Visibility::Inherited,
        Scenes([bed, duck]),
    ));
}
fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>) {
    let scenes = world.get::<Scenes>(context.view).unwrap().0;
    ui.columns(2, |columns| {
        for (ui, scene) in columns.iter_mut().zip(scenes) {
            Mascot::draw(ui, world, &context.for_view(scene), scene);
        }
    });
}
