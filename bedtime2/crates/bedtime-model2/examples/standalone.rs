//! Two independent views of a worker-prepared model, without Workbench.
use bedtime_model2::{ModelPlugin, ModelScene, ModelView, PreparedModel};
use bedtime_ui2::*;
use bevy::{prelude::*, window::PrimaryWindow};
use std::sync::{Arc, Mutex, mpsc};
#[derive(Resource)]
struct Loading(Mutex<mpsc::Receiver<Result<Arc<PreparedModel>, String>>>);
#[derive(Component)]
struct Scenes([Entity; 2]);
fn main() -> AppExit {
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let result = PreparedModel::prepare(
            include_bytes!("../../../../bedtime/tests/fixtures/gltf/cube.glb"),
            std::path::Path::new("cube.glb"),
            |_| Err("No sidecars".into()),
        )
        .map(Arc::new);
        let _ = send.send(result);
    });
    let mut app = App::new();
    bedtime_model2::register_asset_source(&mut app);
    app.add_plugins((DefaultPlugins, UiPlugin, ModelPlugin))
        .insert_resource(Loading(Mutex::new(receive)))
        .add_systems(Startup, setup)
        .add_systems(Update, accept)
        .run()
}
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
    let first = world.spawn((ModelScene::default(), ChildOf(view))).id();
    let second = world.spawn((ModelScene::default(), ChildOf(view))).id();
    world.entity_mut(view).insert((
        Transform::IDENTITY,
        Visibility::Inherited,
        Scenes([first, second]),
    ));
}
fn accept(world: &mut World) {
    let result = world
        .get_resource::<Loading>()
        .and_then(|r| r.0.lock().unwrap().try_recv().ok());
    if let Some(result) = result {
        for mut scene in world.query::<&mut ModelScene>().iter_mut(world) {
            match &result {
                Ok(model) => scene.set_model(model.clone()),
                Err(error) => scene.set_error(error.clone()),
            }
        }
        world.remove_resource::<Loading>();
    }
}
fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>) {
    let scenes = world.get::<Scenes>(context.view).unwrap().0;
    ui.columns(2, |columns| {
        for (ui, scene) in columns.iter_mut().zip(scenes) {
            ModelView::show(ui, world, scene);
        }
    });
}
