//! bEd: a workbench of bedtime's plugins, arranged.
//!
//! The app is policy and coordination; what's reusable lives in bedtime.
//! `main` picks the pieces and arranges what they share: the window, the
//! status bar's global items, keys and the dock menu.
//! - `runtime`: how Bevy runs (logging, frame drawing, the executor)
//! - `workbench`: workbench under bEd's name, with `themes`, `settings` and
//!   `menus`; `titlebar` draws the menu bar and tool buttons
//! - `workspace`: switching folders, numbered layouts, the Workspaces panel
//! - `plugins`: bedtime's panels and viewers, with bEd's answers to what
//!   they leave to the app
//! - `visuals`: backdrop, screen effects and the cat
//! - `perf`: the frame-rate status item
use bedtime_workbench::{
    AreaBackground, DockMenuAppExt, LayoutCamera, LayoutSurface, SurfaceBody, TabBarVisibility,
    Titlebar, status::StatusAppExt,
};
use bevy::{prelude::*, window::PrimaryWindow};

mod menus;
mod perf;
mod plugins;
mod runtime;
mod settings;
mod themes;
mod titlebar;
mod visuals;
mod workbench;
mod workspace;

/// bEd's own keys for actions that have none (VS Code's, where it has them).
const KEYS: &bedtime_workbench::Keymap = {
    use bedtime_workbench::{KeyChord, Modifiers};
    &[(
        "workbench.command_palette",
        &[
            KeyChord::new(Modifiers::PRIMARY.union(Modifiers::SHIFT), KeyCode::KeyP),
            KeyChord::new(Modifiers::NONE, KeyCode::F1),
        ],
    )]
};

fn main() -> AppExit {
    if let Some(exit) = bedtime_terminal::handle_shell_command() {
        return exit;
    }

    let mut app = App::new();
    app.add_plugins((
        runtime::default_plugins(DefaultPlugins.build()).set(WindowPlugin {
            primary_window: Some(window()),
            close_when_requested: false,
            ..default()
        }),
        // Menu items appear in the order their plugins add them.
        workbench::BedWorkbenchPlugin,
        workspace::BedWorkspacesPlugin,
        visuals::VisualsPlugin,
        perf::PerfPlugin,
        plugins::BedtimePlugins,
        bedtime_automation::AutomationPlugin::from_env(),
        // Last, so it sees every schedule the plugins made.
        runtime::RuntimePlugin,
    ))
    // The status bar's global items, left to right.
    .add_global_status(bedtime_workbench::workspaces::status_item)
    .add_global_status(bedtime_git::workbench::status::global_item)
    .add_global_status(bedtime_code::lsp::status::global_item)
    .add_global_status(perf::status_item)
    .add_dock_menu_item::<menus::NewWindow>()
    .add_systems(Startup, setup);
    bedtime_workbench::ActionAppExt::add_keymap(&mut app, KEYS);

    app.run()
}

/// The main window: transparent, its content under the titlebar.
fn window() -> Window {
    Window {
        resolution: (960, 640).into(),
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

fn setup(
    mut commands: Commands,
    window_ent: Single<Entity, With<PrimaryWindow>>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
) {
    window.title = "bEd".to_string();

    commands.entity(*window_ent).insert((
        // A layout of one panel is just that panel, drawn onto the window:
        // no tab bar, no background (a terminal looks native this way).
        LayoutSurface {
            tab_bar: TabBarVisibility::MultipleTabsOrAreas,
            area_background: AreaBackground::MultipleAreas,
            ..default()
        },
        // The panels, with the status bar under them.
        SurfaceBody(bedtime_workbench::status::body),
        Titlebar {
            height: if cfg!(target_os = "macos") {
                18.0
            } else {
                40.0
            },
            draw: titlebar::draw,
        },
    ));

    commands.spawn((Camera2d, LayoutCamera));
}
