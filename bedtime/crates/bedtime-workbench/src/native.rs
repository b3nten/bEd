//! The native plugins: window close requests become `CloseRequested` (the quit
//! flow), plus the native menu bar and (macOS) the Dock menu. Window chrome is
//! bedtime-ui's.
use crate::CloseRequested;
use crate::actions::{Action, ActionId};
use bevy::{app::PluginGroupBuilder, prelude::*};

#[cfg(target_os = "macos")]
mod dock;

/// The actions in the app's Dock menu (macOS), in order: `set_dock_menu`.
#[derive(Resource, Clone, Debug, Default)]
pub struct DockMenu(pub Vec<ActionId>);

pub trait DockMenuAppExt {
    /// Show these actions in the app's Dock menu (right-click its icon, on
    /// macOS), each labelled as its action is. Elsewhere this does nothing.
    fn set_dock_menu(&mut self, actions: impl IntoIterator<Item = ActionId>) -> &mut Self;
    /// Add action `A` to the Dock menu.
    fn add_dock_menu_item<A: Action>(&mut self) -> &mut Self;
}

impl DockMenuAppExt for App {
    fn set_dock_menu(&mut self, actions: impl IntoIterator<Item = ActionId>) -> &mut Self {
        self.insert_resource(DockMenu(actions.into_iter().collect()))
    }
    fn add_dock_menu_item<A: Action>(&mut self) -> &mut Self {
        self.world_mut()
            .get_resource_or_init::<DockMenu>()
            .0
            .push(ActionId::of::<A>());
        self
    }
}

/// The Dock menu (macOS): installed at startup, its clicks run as actions.
pub struct DockMenuPlugin;
impl Plugin for DockMenuPlugin {
    fn build(&self, app: &mut App) {
        // Startup runs inside winit's event loop, after it set the delegate.
        #[cfg(target_os = "macos")]
        app.add_systems(Startup, dock::install)
            .add_systems(PreUpdate, dock::forward);
        let _ = app;
    }
}

#[derive(Message, Clone, Debug)]
pub struct NativeError(pub String);

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NativeSystems {
    /// Forwarding window close requests, in `PreUpdate`.
    Poll,
}

pub struct WorkbenchNativePlugin;
impl PluginGroup for WorkbenchNativePlugin {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            .add(WorkbenchNativeWindowPlugin)
            .add(bedtime_ui::layouts::ChromePlugin)
            .add(crate::menus::NativeMenuPlugin)
            .add(DockMenuPlugin)
    }
}

pub struct WorkbenchNativeWindowPlugin;
impl Plugin for WorkbenchNativeWindowPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<CloseRequested>()
            .add_message::<NativeError>()
            .add_message::<bevy::input::mouse::MouseButtonInput>()
            .add_systems(
                PreUpdate,
                forward_close_requests.in_set(NativeSystems::Poll),
            );
    }
    fn finish(&self, app: &mut App) {
        let plugins = app.get_added_plugins::<bevy::window::WindowPlugin>();
        let windows = plugins
            .first()
            .expect("Install Bevy WindowPlugin for native window integration");
        assert!(
            !windows.close_when_requested,
            "Set WindowPlugin::close_when_requested to false: application/workbench close policy owns close decisions"
        );
    }
}

fn forward_close_requests(
    mut requests: MessageReader<bevy::window::WindowCloseRequested>,
    mut output: MessageWriter<CloseRequested>,
) {
    for request in requests.read() {
        output.write(CloseRequested {
            window: Some(request.window),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::window::PrimaryWindow;
    #[test]
    fn native_window_adapter_reports_close_without_closing_or_installing_menus() {
        let mut app = App::new();
        app.add_plugins(bevy::window::WindowPlugin {
            close_when_requested: false,
            exit_condition: bevy::window::ExitCondition::DontExit,
            ..default()
        })
        .add_plugins(WorkbenchNativeWindowPlugin);
        app.finish();
        let window = app
            .world_mut()
            .query_filtered::<Entity, With<PrimaryWindow>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .write_message(bevy::window::WindowCloseRequested { window });
        app.update();
        let requests: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<CloseRequested>>()
            .drain()
            .collect();
        assert_eq!(
            requests,
            [CloseRequested {
                window: Some(window)
            }]
        );
        assert!(app.world().get::<Window>(window).is_some());
        assert!(!app.is_plugin_added::<crate::menus::NativeMenuPlugin>());
    }
}
