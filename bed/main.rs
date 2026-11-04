use bedtime_ui::UiPlugin;
use bedtime_workbench::{
    AreaBackground, LayoutCamera, LayoutSurface, SurfaceBody, TabBarVisibility, Titlebar,
    WorkbenchPlugins,
};
use bevy::{prelude::*, window::PrimaryWindow};
use std::path::PathBuf;
use workspace::{Start, Workspace};

#[cfg(target_os = "macos")]
mod dock;
mod effects;
mod files;
mod git;
mod language_servers;
mod layouts;
mod mascots;
mod menus;
mod new;
mod preview;
mod search;
mod settings;
mod status;
mod terminal;
mod theme_editor;
mod themes;
mod workspace;

fn main() -> AppExit {
    if let Some(exit) = bedtime_terminal::handle_shell_command() {
        return exit;
    }
    let argument = std::env::args_os().nth(1).map(PathBuf::from);
    app(Start::CommandLine(argument)).run()
}

pub fn app(start: Start) -> App {
    let config = config_directory();
    let workspaces = workspace::WorkspacesPlugin::new(start, config.clone(), app);
    let automation = bedtime_automation::AutomationPlugin::from_env();
    // A script is driving: don't take focus from what you're doing.
    let window = window(workspaces.workspace(), !automation.active());

    // Logging is set up once per process; a relaunched App leaves it alone.
    let mut defaults = DefaultPlugins.build();
    if bevy::winit::relaunching() {
        defaults = defaults.disable::<bevy::log::LogPlugin>();
    }

    let mut app = App::new();
    // Models are handed to Bevy's glTF loader from memory.
    bedtime_model::register_asset_source(&mut app);
    app.add_plugins((
        defaults.set(WindowPlugin {
            primary_window: Some(window),
            close_when_requested: false,
            ..default()
        }),
        UiPlugin,
        WorkbenchPlugins,
        workspaces,
        settings::BedSettingsPlugin,
        menus::MenusPlugin,
        layouts::LayoutSlotsPlugin,
        effects::EffectsPlugin,
        themes::ThemesPlugin,
        workspace::WorkspacesPanelPlugin,
        files::FilesPlugin,
        terminal::TerminalsPlugin,
        mascots::MascotsPlugin,
        automation,
    ))
    // Look for installed fonts now, so Settings can list them when opened.
    .add_plugins((
        new::NewPlugin,
        language_servers::LanguageServersPanelPlugin,
        git::BedGitPlugin,
        bedtime_http_client::HttpClientPlugin,
        search::BedSearchPlugin,
        preview::BedPreviewPlugin,
        theme_editor::ThemeEditorPlugin,
    ))
    .init_resource::<bedtime_ui::fonts::InstalledFonts>()
    .insert_resource(settings::GlobalLocation(
        config.map(|config| config.join("global")),
    ))
    .add_systems(Startup, setup);
    #[cfg(target_os = "macos")]
    app.add_plugins(dock::DockMenuPlugin);
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
        LayoutSurface {
            tab_bar: TabBarVisibility::MultipleTabsOrAreas,
            area_background: AreaBackground::MultipleAreas,
            ..default()
        },
        // The panels, with the status bar under them.
        SurfaceBody(status::body),
        Titlebar {
            height: if cfg!(target_os = "macos") {
                18.0
            } else {
                40.0
            },
            draw: menus::draw_titlebar,
        },
    ));
    commands.spawn((Camera2d, LayoutCamera));
}
