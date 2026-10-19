use bed2_demo_game::CubeGamePlugin;
use bevy::prelude::*;
mod support;

fn main() {
    let mut app = App::new();
    app.add_plugins((
        DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Cube game — standalone Bevy".into(),
                resolution: (960, 700).into(),
                ..default()
            }),
            ..default()
        }),
        CubeGamePlugin,
    ));
    app.add_systems(Startup, bed2_demo_game::setup);
    support::install(&mut app);
    app.run();
}
