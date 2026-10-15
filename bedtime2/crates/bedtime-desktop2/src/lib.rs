//! Native integration for an application-owned Bevy window.
//!
//! Install after `WindowPlugin`, with `close_when_requested: false`. Native
//! commands and close requests become [`DesktopAction`] messages. The application
//! decides when to exit by writing `AppExit`; this plugin never closes a window.
//! Rendering, ImGui and workbench policy belong to higher layers.

mod clipboard;
mod menus;
mod native;

pub use clipboard::DesktopClipboard;
pub use menus::{
    Accelerator, ActionEnabled, Activates, ActivationSources, Code, InMenu, Menu, MenuBar,
    MenuEntries, MenuItem, MenuSeparator, Modifiers,
};

use bevy::{prelude::*, window::WindowCloseRequested};

/// Owned observations from the platform. Action targets have application-defined meaning.
#[derive(Message, Clone, Debug, PartialEq, Eq)]
pub enum DesktopAction {
    Activated {
        source: Entity,
        target: Entity,
    },
    /// `None` requests application quit; `Some` identifies a window close request.
    CloseRequested {
        window: Option<Entity>,
    },
}

#[derive(Message, Clone, Debug)]
pub struct DesktopError(pub String);

/// Native backing color in sRGB, including the area behind a transparent title
/// bar on macOS. Other platforms continue to use their normal window decoration.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct WindowBackground([f32; 4]);

impl WindowBackground {
    pub fn new(rgba: [f32; 4]) -> Self {
        assert!(
            rgba.into_iter()
                .all(|v| v.is_finite() && (0.0..=1.0).contains(&v)),
            "Window background channels must be finite and in 0..=1"
        );
        Self(rgba)
    }
}

#[derive(Resource, Clone, Copy, Debug)]
pub struct DesktopCapabilities {
    pub native_menus: bool,
}

/// UI integration can prepare after native actions have been collected.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DesktopSystems {
    Native,
}

/// Adds platform services to Bevy. The application installs its own Bevy plugins.
pub struct DesktopPlugin;

impl Plugin for DesktopPlugin {
    fn build(&self, app: &mut App) {
        let windows = app.get_added_plugins::<bevy::window::WindowPlugin>();

        let windows = windows
            .first()
            .expect("Install Bevy's WindowPlugin before DesktopPlugin");

        assert!(
            !windows.close_when_requested,
            "Set WindowPlugin::close_when_requested to false: the application owns close decisions"
        );

        app.insert_resource(DesktopCapabilities {
            native_menus: cfg!(target_os = "macos"),
        })
        .init_non_send::<native::NativeState>()
        .add_message::<DesktopAction>()
        .add_message::<DesktopError>()
        .add_systems(
            PreUpdate,
            (forward_close_requests, native::poll)
                .chain()
                .in_set(DesktopSystems::Native),
        )
        .add_systems(Last, native::shutdown.after(bevy::app::OnAppExitSystems));
        #[cfg(target_os = "macos")]
        app.add_systems(
            Last,
            native::sync_window_background.before(native::shutdown),
        );
    }
}

fn forward_close_requests(
    mut requests: MessageReader<WindowCloseRequested>,
    mut actions: MessageWriter<DesktopAction>,
) {
    for request in requests.read() {
        actions.write(DesktopAction::CloseRequested {
            window: Some(request.window),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::window::{ExitCondition, PrimaryWindow, WindowPlugin};

    #[test]
    fn closing_waits_for_the_application_without_a_renderer_or_native_window() {
        let mut app = App::new();
        app.add_plugins(WindowPlugin {
            close_when_requested: false,
            exit_condition: ExitCondition::DontExit,
            ..default()
        })
        .add_plugins(DesktopPlugin);
        let window = app
            .world_mut()
            .query_filtered::<Entity, With<PrimaryWindow>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .write_message(WindowCloseRequested { window });
        app.update();

        let actions: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<DesktopAction>>()
            .drain()
            .collect();
        assert_eq!(
            actions,
            [DesktopAction::CloseRequested {
                window: Some(window)
            }]
        );
        for _ in 0..4 {
            app.update();
        }
        assert!(app.world().get::<Window>(window).is_some());
        assert!(app.world().resource::<Messages<AppExit>>().is_empty());
        assert!(!app.world().contains_non_send::<DesktopClipboard>());
    }
}
