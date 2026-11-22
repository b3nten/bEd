//! bEd's main window: transparent, its content under the titlebar; the
//! panels with the status bar under them; and what `main` arranged of what
//! the plugins share: the status bar's global items, bEd's own keys and the
//! dock menu.
use crate::{menus, perf, titlebar};
use bedtime_workbench::{
    AreaBackground, DockMenuAppExt, LayoutCamera, LayoutSurface, SurfaceBody, TabBarVisibility,
    Titlebar, status::StatusAppExt,
};
use bevy::{prelude::*, window::PrimaryWindow};

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

/// Add it after the plugins whose status items and actions it arranges.
pub struct BedWindowPlugin;

impl Plugin for BedWindowPlugin {
    fn build(&self, app: &mut App) {
        // The status bar's global items, left to right.
        app.add_global_status(bedtime_workbench::workspaces::status_item)
            .add_global_status(bedtime_git::workbench::status::global_item)
            .add_global_status(bedtime_code::lsp::status::global_item)
            .add_global_status(perf::status_item)
            .add_dock_menu_item::<menus::NewWindow>()
            .add_systems(Startup, setup);
        bedtime_workbench::ActionAppExt::add_keymap(app, KEYS);
    }
}

/// Bevy's window plugin with bEd's main window. The workbench decides when
/// the window closes (unsaved work is asked about first).
pub fn window_plugin() -> WindowPlugin {
    WindowPlugin {
        primary_window: Some(window()),
        close_when_requested: false,
        ..default()
    }
}

/// The main window: transparent, its content under the titlebar. While a
/// script drives the app it opens unfocused, behind what's being worked in,
/// so the script takes neither the keyboard nor the front.
pub fn window() -> Window {
    Window {
        focused: !bedtime_automation::AutomationPlugin::from_env().active(),
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
