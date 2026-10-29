//! Optional platform adapters. Core menus and commands have no native dependency.
use crate::CloseRequested;
use bevy::{app::PluginGroupBuilder, prelude::*, window::RawHandleWrapper};

#[derive(Message, Clone, Debug)]
pub struct NativeError(pub String);

#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct WindowBackground([f32; 4]);
impl WindowBackground {
    pub fn new(rgba: [f32; 4]) -> Self {
        assert!(
            rgba.into_iter()
                .all(|v| v.is_finite() && (0.0..=1.0).contains(&v)),
            "Invalid window background"
        );
        Self(rgba)
    }
}

/// Observed native titlebar geometry in logical content-view coordinates, with
/// a top-left origin. Zero for windows whose titlebar is outside their content.
/// The host converts these measurements to egui points before allocating UI.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct WindowTitlebarGeometry {
    pub height: f32,
    pub controls: Option<Rect>,
}

#[cfg(target_os = "macos")]
#[derive(Component)]
pub(crate) struct WindowDrag;

/// Opt into a passive macOS material behind the transparent GPU surface.
/// Attach alongside WindowBackground (supplied by the workbench root).
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct WindowBlur(pub bool);

#[cfg(target_os = "macos")]
mod macos_background;

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NativeSystems {
    WindowGeometry,
    Poll,
}

pub struct WorkbenchNativePlugin;
impl PluginGroup for WorkbenchNativePlugin {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            .add(WorkbenchNativeWindowPlugin)
            .add(bedtime_menus::NativeMenuPlugin)
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
        #[cfg(target_os = "macos")]
        app.init_non_send::<macos_background::Backgrounds>()
            .add_systems(First, macos_background::finish_window_drags)
            .add_systems(
                PreUpdate,
                macos_background::measure_titlebar.in_set(NativeSystems::WindowGeometry),
            )
            .add_systems(
                Last,
                macos_background::sync.before(bevy::app::OnAppExitSystems),
            );
        #[cfg(all(target_os = "macos", feature = "ui"))]
        app.add_systems(
            PreUpdate,
            macos_background::sample_file_drag_cursor.before(bevy_egui::EguiInputSet::FocusContext),
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

#[cfg(all(target_os = "macos", feature = "ui"))]
pub(crate) fn titlebar_double_click(handle: &RawHandleWrapper) {
    macos_background::double_click(handle);
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
        assert!(!app.is_plugin_added::<bedtime_menus::NativeMenuPlugin>());
    }
}
