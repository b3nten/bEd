use bedtime_ui::UiPlugin;
use bedtime_workbench::{CloseRequested, WorkbenchCamera, WorkbenchPlugin, WorkbenchSurface};
use bevy::{prelude::*, window::PrimaryWindow};

fn main() -> AppExit {
    App::new()
        .add_plugins((
            DefaultPlugins.set(WindowPlugin {
                primary_window: Some(Window {
                    title: "bEd".into(),
                    resolution: (960, 640).into(),
                    titlebar_show_title: false,
                    titlebar_transparent: true,
                    transparent: true,
                    ..default()
                }),
                close_when_requested: false,
                ..default()
            }),
            UiPlugin,
            WorkbenchPlugin,
        ))
        .add_systems(Startup, setup)
        .add_systems(Update, close)
        .run()
}

fn setup(mut commands: Commands, window: Single<Entity, With<PrimaryWindow>>) {
    commands.entity(*window).insert(WorkbenchSurface::default());
    commands.spawn((Camera2d, WorkbenchCamera));
}

fn close(mut requests: MessageReader<CloseRequested>, mut exit: MessageWriter<AppExit>) {
    if requests.read().count() > 0 {
        exit.write(AppExit::Success);
    }
}
