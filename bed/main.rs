use bedtime_ui::UiPlugin;
use bedtime_workbench::{
    AreaBackground, TabBarVisibility, Titlebar, WorkbenchCamera, WorkbenchPlugin, WorkbenchSurface,
};
use bevy::{prelude::*, window::PrimaryWindow};
use std::path::PathBuf;
use workspace::{Start, Workspace};

mod effects;
mod layouts;
mod menus;
mod panels;
mod settings;
mod status;
mod themes;
mod workspace;

fn main() -> AppExit {
    let argument = std::env::args_os().nth(1).map(PathBuf::from);
    app(Start::CommandLine(argument)).run()
}

/// The App for one workspace. Switching workspace drops it and builds another
/// in the same window (`bevy::winit::relaunch`).
pub fn app(start: Start) -> App {
    let launch = workspace::launch(start);
    let automation = bedtime_automation::AutomationPlugin::from_env();
    // A script is driving: don't take focus from what you're doing.
    let window = window(&launch.opened.workspace, !automation.active());

    // Logging is set up once per process; a relaunched App leaves it alone.
    let mut defaults = DefaultPlugins.build();
    if bevy::winit::relaunching() {
        defaults = defaults.disable::<bevy::log::LogPlugin>();
    }

    let mut app = App::new();
    app.add_plugins((
        defaults.set(WindowPlugin {
            primary_window: Some(window),
            close_when_requested: false,
            ..default()
        }),
        UiPlugin,
        WorkbenchPlugin,
        settings::SettingsPlugin,
        menus::MenusPlugin,
        layouts::LayoutsPlugin,
        effects::EffectsPlugin,
        themes::ThemesPlugin,
        workspace::WorkspacesPanelPlugin,
        status::StatusBarPlugin,
        automation,
    ))
    // Look for installed fonts now, so Settings can list them when opened.
    .init_resource::<bedtime_ui::fonts::InstalledFonts>()
    .add_systems(Startup, setup);
    launch.install(&mut app);
    app
}

/// `$BED_CONFIG_DIR`, else `$XDG_CONFIG_HOME/bed`, else `~/.config/bed`.
pub fn config_directory() -> Option<PathBuf> {
    bedtime_persistence::config_directory("bed", Some("BED_CONFIG_DIR"))
}

/// A transparent window whose content runs under the titlebar.
fn window(workspace: &Workspace, focused: bool) -> Window {
    Window {
        title: if workspace.default {
            "bEd".into()
        } else {
            format!("{} — bEd", workspace.name())
        },
        resolution: (960, 640).into(),
        focused,
        titlebar_show_title: false,
        titlebar_transparent: true,
        fullsize_content_view: true,
        transparent: true,
        composite_alpha_mode: if cfg!(target_os = "macos") {
            bevy::window::CompositeAlphaMode::PostMultiplied
        } else {
            bevy::window::CompositeAlphaMode::Auto
        },
        ..default()
    }
}

fn setup(mut commands: Commands, window: Single<Entity, With<PrimaryWindow>>) {
    commands.entity(*window).insert((
        // A layout of one panel is just that panel, drawn onto the window:
        // no tab bar, no background (a terminal looks native this way).
        WorkbenchSurface {
            tab_bar: TabBarVisibility::MultipleTabsOrAreas,
            area_background: AreaBackground::MultipleAreas,
            ..default()
        },
        Titlebar {
            height: if cfg!(target_os = "macos") {
                18.0
            } else {
                40.0
            },
            draw: menus::draw_titlebar,
        },
    ));
    commands.spawn((Camera2d, WorkbenchCamera));
}
