use bedtime_ui::UiPlugin;
use bedtime_workbench::{
    Titlebar, WorkbenchCamera, WorkbenchPlugin, WorkbenchSurface,
    persistence::{DirectoryStore, StoreAppExt, WorkbenchStore},
    settings::{Setting, SettingsAppExt, read_setting},
};
use bevy::{prelude::*, window::PrimaryWindow};
use std::path::{Path, PathBuf};
use workspace::{Opened, RecentWorkspaces};

mod menus;
mod panels;
mod settings;
mod workspace;

fn main() -> AppExit {
    let argument = std::env::args_os().nth(1).map(PathBuf::from);
    app(argument).run()
}

/// The App for one workspace. Switching workspace drops it and builds another
/// in the same window (`bevy_winit::relaunch`).
pub fn app(argument: Option<PathBuf>) -> App {
    let config = config_directory();
    let global = config.as_ref().map(|config| config.join("global"));
    // Which workspace to open is decided before the app exists, from what the
    // global store holds.
    let mut recent = RecentWorkspaces::default();
    let mut reopen_last = <settings::ReopenLastWorkspace as Setting>::default();
    if let Some(global) = &global {
        let store = DirectoryStore::new(global);
        recent = RecentWorkspaces::read(&store);
        reopen_last = read_setting::<settings::ReopenLastWorkspace>(&store).unwrap_or(reopen_last);
    }
    let opened = workspace::open(argument, config.as_deref(), &recent, reopen_last);
    if let (Some(workspace), Some(global)) = (&opened.workspace, &global)
        && let Err(error) = recent.remember(&DirectoryStore::new(global), &workspace.root)
    {
        warn!("Couldn't update recent folders: {error}");
    }
    let title = opened.workspace.as_ref().map_or("bEd".into(), |workspace| {
        format!("{} — bEd", workspace.name())
    });
    let automation = bedtime_automation::AutomationPlugin::from_env();
    let mut app = App::new();
    // Logging is set up once per process; a relaunched App leaves it alone.
    let mut defaults = DefaultPlugins.build();
    if bevy::winit::relaunching() {
        defaults = defaults.disable::<bevy::log::LogPlugin>();
    }
    app.add_plugins((
        defaults.set(WindowPlugin {
            primary_window: Some(Window {
                title,
                resolution: (960, 640).into(),
                // A script is driving: don't take focus from what you're doing.
                focused: !automation.active(),
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
            }),
            close_when_requested: false,
            ..default()
        }),
        UiPlugin,
        WorkbenchPlugin,
        settings::SettingsPlugin,
        menus::MenusPlugin,
        automation,
    ))
    .insert_resource(recent)
    // Look for installed fonts now, so Settings can list them when opened.
    .init_resource::<bedtime_ui::fonts::InstalledFonts>()
    .add_systems(Startup, setup);
    stores(&mut app, config.as_deref(), global, opened);
    app
}

/// Where things are saved. Global settings and data go in `<config>/global`.
/// The workbench (layout, panels, workspace settings) goes in the workspace's
/// store, or in `<config>/workbench` when no folder is open.
fn stores(app: &mut App, config: Option<&Path>, global: Option<PathBuf>, opened: Opened) {
    if let Some(global) = &global {
        app.add_store::<settings::GlobalStore>(DirectoryStore::new(global));
    }
    app.insert_resource(settings::GlobalLocation(global));
    let Some(workspace) = opened.workspace else {
        if let Some(config) = config {
            app.add_store::<WorkbenchStore>(DirectoryStore::new(config.join("workbench")));
        }
        return;
    };
    // Workspace settings override global ones. Without a claimed store they
    // still apply, but only in memory.
    app.add_settings_layer::<WorkbenchStore>();
    if let (Some(lock), Some(config)) = (opened.lock, config) {
        app.add_store::<WorkbenchStore>(DirectoryStore::new(workspace.store_directory(config)))
            .insert_resource(lock);
    }
    if let Some(unclaimed) = opened.unclaimed {
        app.insert_resource(workspace::Unclaimed(unclaimed));
    }
    app.insert_resource(workspace)
        .add_systems(Update, workspace::report_unclaimed);
}

/// `$BED_CONFIG_DIR`, else `$XDG_CONFIG_HOME/bed`, else `~/.config/bed`.
fn config_directory() -> Option<PathBuf> {
    if let Some(directory) = std::env::var_os("BED_CONFIG_DIR") {
        return Some(directory.into());
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("bed"))
}

fn setup(mut commands: Commands, window: Single<Entity, With<PrimaryWindow>>) {
    commands.entity(*window).insert((
        WorkbenchSurface::default(),
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
