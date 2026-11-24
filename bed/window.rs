//! bEd's main window: transparent, its content under the titlebar; the
//! panels with the status bar under them; and what bEd arranges of what the
//! plugins share: its own keys and the dock menu.
use crate::titlebar;
use bedtime::workbench::{
    AreaBackground, LayoutSurface, TabBarVisibility, Titlebar, settings::SettingsAppExt as _,
};
use bevy::{prelude::*, window::PrimaryWindow};

/// bEd's own keys for actions that have none (VS Code's, where it has them).
const KEYS: &bedtime::workbench::Keymap = {
    use bedtime::workbench::{KeyChord, Modifiers};
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
        app.init_resource::<titlebar::MenuIcon>()
            .register_setting::<titlebar::WindowMenu>()
            .register_setting::<titlebar::CompactMenu>()
            .add_systems(Startup, setup)
            // The native menu bar is installed (or not) after startup.
            .add_systems(Update, titlebar::fit);
        // The code actions' keys, as VS Code has them, and bEd's own.
        bedtime::workbench::ActionAppExt::add_keymap(app, bedtime::code::keymap::VSCODE);
        bedtime::workbench::ActionAppExt::add_keymap(app, KEYS);
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
/// script drives the app it opens unfocused, behind what's being worked in.
pub fn window() -> Window {
    Window {
        focused: !bedtime::workbench::driven_by_script(),
        resolution: (960, 640).into(),
        titlebar_show_title: false,
        titlebar_transparent: true,
        fullsize_content_view: true,
        transparent: true,
        // wgpu resolves `Auto` to opaque, so a transparent window names its
        // mode: Linux keeps the premultiplied alpha the UI writes (where the
        // surface offers it, as Wayland's do; otherwise the window is opaque).
        composite_alpha_mode: if cfg!(target_os = "macos") {
            bevy::window::CompositeAlphaMode::PostMultiplied
        } else if cfg!(target_os = "linux") {
            bevy::window::CompositeAlphaMode::PreMultiplied
        } else {
            bevy::window::CompositeAlphaMode::Auto
        },
        ..default()
    }
}

/// bEd's surface and titlebar, on the window of panels the app plugins made
/// it (`ShellPlugin`: the status bar under the panels, the camera).
fn setup(mut commands: Commands, window: Single<Entity, With<PrimaryWindow>>) {
    commands.entity(*window).insert((
        // A layout of one panel is just that panel, drawn onto the window:
        // no tab bar, no background (a terminal looks native this way).
        LayoutSurface {
            tab_bar: TabBarVisibility::MultipleTabsOrAreas,
            area_background: AreaBackground::MultipleAreas,
            ..default()
        },
        Titlebar {
            // `titlebar::fit` keeps it as tall as the menu needs.
            height: titlebar::height(None),
            draw: titlebar::draw,
        },
    ));
}
