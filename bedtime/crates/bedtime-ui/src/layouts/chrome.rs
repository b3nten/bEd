//! Window chrome: the window's background colour and macOS's titlebar,
//! measured so a `Titlebar` can be drawn into it, and blur behind the window.
//! The macOS parts need the `native` feature; elsewhere these components are
//! plain data and windows keep their usual chrome.
use bevy::prelude::*;

/// The window's background colour, behind a transparent GPU surface. The
/// layout surface keeps it in step with the theme.
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

/// Opt into a passive macOS material behind the transparent GPU surface.
/// Attach alongside `WindowBackground` (the layout surface supplies it).
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct WindowBlur(pub bool);

/// A titlebar drag in progress, ended when the button is released.
#[cfg(all(target_os = "macos", feature = "native"))]
#[derive(Component)]
pub(crate) struct WindowDrag;

#[cfg(all(target_os = "macos", feature = "native"))]
mod macos_background;

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChromeSystems {
    /// Measuring the native titlebar, in `PreUpdate` before drawing.
    WindowGeometry,
}

/// The native window chrome (macOS, with the `native` feature): measures the
/// titlebar, keeps the window background and blur, and moves the window when
/// its titlebar is dragged. Needs Bevy's windowing.
pub struct ChromePlugin;

impl Plugin for ChromePlugin {
    fn build(&self, app: &mut App) {
        install(app);
    }
}

fn install(app: &mut App) {
    app.configure_sets(
        PreUpdate,
        ChromeSystems::WindowGeometry.before(crate::UiSystems::Draw),
    );
    #[cfg(all(target_os = "macos", feature = "native"))]
    app.add_message::<bevy::input::mouse::MouseButtonInput>()
        .init_non_send::<macos_background::Backgrounds>()
        .add_systems(First, macos_background::finish_window_drags)
        .add_systems(
            PreUpdate,
            (
                macos_background::measure_titlebar.in_set(ChromeSystems::WindowGeometry),
                macos_background::sample_file_drag_cursor
                    .before(bevy_egui::EguiInputSet::FocusContext),
            ),
        )
        .add_systems(
            Last,
            macos_background::sync.before(bevy::app::OnAppExitSystems),
        );
}

/// Zoom or minimise the window, as double-clicking its titlebar does.
#[cfg(all(target_os = "macos", feature = "native"))]
pub(crate) fn titlebar_double_click(handle: &bevy::window::RawHandleWrapper) {
    macos_background::double_click(handle);
}
