//! The same game in a full-window UI viewport, without Workbench.
use bed2_demo_game::{CubeGamePlugin, GameCamera};
use bedtime_ui2::{
    Component as _, EmbeddedRenderPlugin, UiCamera, UiContext, UiPlugin, UiSurface, UiViewport,
    UsesTheme, ViewportOutput, egui, register_view, themes,
};
use bevy::{app::PluginsState, prelude::*, window::PrimaryWindow};
mod support;

struct Game(App);

fn main() {
    let mut app = App::new();
    app.add_plugins((
        DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Cube game — embedded without Workbench".into(),
                resolution: (960, 700).into(),
                ..default()
            }),
            ..default()
        }),
        UiPlugin,
    ))
    .add_systems(Startup, setup)
    .add_systems(Update, update_game);
    support::install(&mut app);
    app.run();
}

fn setup(host: &mut World) {
    let window = host
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(host)
        .unwrap();
    let theme = host.spawn(themes::dark()).id();
    host.entity_mut(window)
        .insert((UiSurface, UsesTheme(theme)));
    host.spawn((
        Camera2d,
        Camera {
            order: 100,
            ..default()
        },
        UiCamera(window),
    ));
    let output = ViewportOutput::new(host, UVec2::new(960, 700));
    let view = register_view(host, window, draw);
    host.entity_mut(view)
        .insert(UiViewport::new(output.image.clone()));

    let mut game = App::new();
    game.add_plugins((
        EmbeddedRenderPlugin::<GameCamera>::new(output),
        CubeGamePlugin,
    ));
    game.add_systems(Startup, bed2_demo_game::setup);
    host.insert_non_send(Game(game));
}

fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>) {
    UiViewport::draw(ui, world, context, context.view);
}

fn update_game(mut game: NonSendMut<Game>, time: Res<Time<Real>>) {
    let app = &mut game.0;
    match app.plugins_state() {
        PluginsState::Adding => return,
        PluginsState::Ready => {
            app.finish();
            app.cleanup();
        }
        PluginsState::Finished => app.cleanup(),
        PluginsState::Cleaned => {}
    }
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(time.delta()));
    app.update();
}
