#![cfg_attr(docsrs, feature(doc_cfg))]
// bEd: deny rather than forbid, so `state::remove_metal_layers` can opt in.
#![deny(unsafe_code)]
#![doc(
    html_logo_url = "https://bevy.org/assets/icon.png",
    html_favicon_url = "https://bevy.org/assets/icon.png"
)]

//! `bevy_winit` provides utilities to handle window creation and the eventloop through [`winit`]
//!
//! Most commonly, the [`WinitPlugin`] is used as part of
//! [`DefaultPlugins`](https://docs.rs/bevy/latest/bevy/struct.DefaultPlugins.html).
//! The app's [runner](bevy_app::App::runner) is set by `WinitPlugin` and handles the `winit` [`EventLoop`].
//! See `winit_runner` for details.

extern crate alloc;

use bevy_derive::Deref;
use bevy_reflect::Reflect;
use bevy_window::{ExitSystems, RawHandleWrapperHolder, WindowEvent};
use core::cell::RefCell;
use winit::{
    event_loop::EventLoop,
    window::WindowId,
};

use bevy_a11y::AccessibilityRequested;
use bevy_app::{App, Last, OnAppExitSystems, Plugin};
use bevy_ecs::prelude::*;
use bevy_window::{CursorOptions, Window, WindowCreated};
use system::{changed_cursor_options, changed_windows, check_keyboard_focus_lost, despawn_windows};
pub use system::{create_monitors, create_windows};
#[cfg(all(target_family = "wasm", target_os = "unknown"))]
pub use winit::platform::web::CustomCursorExtWebSys;
pub use winit::{
    event_loop::EventLoopProxy,
    window::{CustomCursor as WinitCustomCursor, CustomCursorSource},
};
pub use winit_config::*;
pub use winit_monitors::*;
pub use winit_windows::*;

use crate::{
    accessibility::{AccessKitPlugin, WinitActionRequestHandlers},
    state::winit_runner,
};

pub mod accessibility;
pub mod converters;
mod cursor;
mod state;
mod system;
mod winit_config;
mod winit_monitors;
mod winit_windows;

thread_local! {
    /// Temporary storage of WinitWindows data to replace usage of `!Send` resources. This will be replaced with proper
    /// storage of `!Send` data after issue #17667 is complete.
    pub static WINIT_WINDOWS: RefCell<WinitWindows> = const { RefCell::new(WinitWindows::new()) };

    /// bEd: the event loop's proxy and display handle once a `WinitPlugin`
    /// has built it. winit allows one event loop per process, so Apps built
    /// later (see [`relaunch`]) reuse it.
    static EVENT_LOOP: RefCell<Option<(EventLoopProxy<WinitUserEvent>, winit::event_loop::OwnedDisplayHandle)>> =
        const { RefCell::new(None) };

    /// bEd: the App to run next, set by [`relaunch`].
    pub(crate) static RELAUNCH: RefCell<Option<Box<dyn FnOnce() -> App>>> = const { RefCell::new(None) };
}

/// bEd: whether this process already runs an event loop, so an App built now
/// is a relaunch. Process-wide setup (logging, signal handlers) is already done.
pub fn relaunching() -> bool {
    EVENT_LOOP.with_borrow(Option::is_some)
}

/// bEd: replace the running App with the one `make` builds, in the same
/// windows. This App exits normally (its exit systems run this update), then
/// is dropped, renderer and all, before `make` runs. The next App's windows
/// adopt the open native windows in order instead of creating new ones, keeping
/// their size and position.
pub fn relaunch(world: &mut World, make: impl FnOnce() -> App + 'static) {
    RELAUNCH.with_borrow_mut(|next| *next = Some(Box::new(make)));
    world.write_message(bevy_app::AppExit::Success);
}

/// A [`Plugin`] that uses `winit` to create and manage windows, and receive window and input
/// events.
///
/// This plugin will add systems and resources that sync with the `winit` backend and also
/// replace the existing [`App`] runner with one that constructs an [event loop](EventLoop) to
/// receive window and input events from the OS.
#[derive(Default)]
pub struct WinitPlugin {
    /// Allows the window (and the event loop) to be created on any thread
    /// instead of only the main thread.
    ///
    /// See [`EventLoopBuilder::build`](winit::event_loop::EventLoopBuilder::build) for more information on this.
    ///
    /// # Supported platforms
    ///
    /// Only works on Linux (X11/Wayland) and Windows.
    /// This field is ignored on other platforms.
    pub run_on_any_thread: bool,
}

impl Plugin for WinitPlugin {
    fn name(&self) -> &str {
        "bevy_winit::WinitPlugin"
    }

    fn build(&self, app: &mut App) {
        // bEd: a relaunched App reuses the running event loop.
        if let Some((proxy, display)) = EVENT_LOOP.with_borrow(Clone::clone) {
            self.add_systems(app, proxy, display);
            return;
        }
        let mut event_loop_builder = EventLoop::<WinitUserEvent>::with_user_event();

        // linux check is needed because x11 might be enabled on other platforms.
        #[cfg(all(target_os = "linux", feature = "x11"))]
        {
            use winit::platform::x11::EventLoopBuilderExtX11;

            // This allows a Bevy app to be started and ran outside the main thread.
            // A use case for this is to allow external applications to spawn a thread
            // which runs a Bevy app without requiring the Bevy app to need to reside on
            // the main thread, which can be problematic.
            event_loop_builder.with_any_thread(self.run_on_any_thread);
        }

        // linux check is needed because wayland might be enabled on other platforms.
        #[cfg(all(target_os = "linux", feature = "wayland"))]
        {
            use winit::platform::wayland::EventLoopBuilderExtWayland;
            event_loop_builder.with_any_thread(self.run_on_any_thread);
        }

        #[cfg(target_os = "macos")]
        {
            use bevy_ecs::system::SystemState;
            use winit::platform::macos::EventLoopBuilderExtMacOS;

            // Don't request app activation on startup if all its windows should
            // start unfocused. Otherwise, app activation would focus one of the
            // windows.
            let mut initial_windows_state =
                SystemState::<Query<(Entity, &Window)>>::new(app.world_mut());
            let initial_windows = initial_windows_state.get(app.world()).unwrap();
            let initially_focused = initial_windows.iter().any(|(_, window)| window.focused);
            event_loop_builder.with_activate_ignoring_other_apps(initially_focused);
        }

        #[cfg(target_os = "windows")]
        {
            use winit::platform::windows::EventLoopBuilderExtWindows;
            event_loop_builder.with_any_thread(self.run_on_any_thread);
        }

        #[cfg(target_os = "android")]
        {
            use winit::platform::android::EventLoopBuilderExtAndroid;
            let msg = "Bevy must be setup with the #[bevy_main] macro on Android";
            event_loop_builder
                .with_android_app(bevy_android::ANDROID_APP.get().expect(msg).clone());
        }

        let event_loop = event_loop_builder
            .build()
            .expect("Failed to build event loop");

        let event_loop_proxy = event_loop.create_proxy();

        // Wake up the event loop when `Ctrl+C` is received so that the app can
        // exit even while idle in a reactive update mode
        #[cfg(any(all(unix, not(target_os = "horizon")), windows))]
        {
            let event_loop_proxy = event_loop_proxy.clone();
            bevy_app::TerminalCtrlCHandlerPlugin::register_exit_handler(move || {
                let _ = event_loop_proxy.send_event(WinitUserEvent::WakeUp);
            });
        }

        let display = event_loop.owned_display_handle();
        EVENT_LOOP.with_borrow_mut(|stored| {
            *stored = Some((event_loop_proxy.clone(), display.clone()));
        });
        app.set_runner(|app| winit_runner(app, event_loop));
        self.add_systems(app, event_loop_proxy, display);
    }
}

impl WinitPlugin {
    fn add_systems(
        &self,
        app: &mut App,
        event_loop_proxy: EventLoopProxy<WinitUserEvent>,
        display: winit::event_loop::OwnedDisplayHandle,
    ) {
        app.init_resource::<WinitMonitors>()
            .init_resource::<WinitSettings>()
            .insert_resource(DisplayHandleWrapper(display))
            .insert_resource(EventLoopProxyWrapper(event_loop_proxy))
            .add_message::<RawWinitWindowEvent>()
            .add_systems(
                Last,
                (
                    changed_windows,
                    changed_cursor_options,
                    despawn_windows.after(ExitSystems).after(OnAppExitSystems),
                    check_keyboard_focus_lost,
                )
                    .chain(),
            );

        app.add_plugins(AccessKitPlugin);
        app.add_plugins(cursor::WinitCursorPlugin);

        app.add_observer(
            |_window: On<Add<Window>>, event_loop_proxy: Res<EventLoopProxyWrapper>| -> Result {
                event_loop_proxy.send_event(WinitUserEvent::WindowAdded)?;

                Ok(())
            },
        );
    }
}

/// Events that can be sent to perform actions inside the winit event loop.
///
/// Sent via the [`EventLoopProxyWrapper`] resource.
///
/// # Example
///
/// ```
/// # use bevy_ecs::prelude::*;
/// # use bevy_winit::{EventLoopProxyWrapper, WinitUserEvent};
/// fn wakeup_system(event_loop_proxy: Res<EventLoopProxyWrapper>) -> Result {
///     event_loop_proxy.send_event(WinitUserEvent::WakeUp)?;
///
///     Ok(())
/// }
/// ```
#[derive(Debug, Clone, Copy, Reflect)]
#[reflect(Debug, Clone)]
pub enum WinitUserEvent {
    /// Dummy event that just wakes up the winit event loop
    WakeUp,
    /// Tell winit that a window needs to be created
    WindowAdded,
}

/// The original window event as produced by Winit. This is meant as an escape
/// hatch for power users that wish to add custom Winit integrations.
/// If you want to process events for your app or game, you should instead use
/// `bevy::window::WindowEvent`, or one of its sub-events.
///
/// When you receive this event it has already been handled by Bevy's main loop.
/// Sending these events will NOT cause them to be processed by Bevy.
#[derive(Debug, Clone, Message)]
pub struct RawWinitWindowEvent {
    /// The window for which the event was fired.
    pub window_id: WindowId,
    /// The raw winit window event.
    pub event: winit::event::WindowEvent,
}

/// A wrapper type around [`winit::event_loop::EventLoopProxy`] with the specific
/// [`winit::event::Event::UserEvent`] used in the [`WinitPlugin`].
///
/// The `EventLoopProxy` can be used to request a redraw from outside bevy.
///
/// Use `Res<EventLoopProxyWrapper>` to retrieve this resource.
#[derive(Resource, Deref)]
pub struct EventLoopProxyWrapper(EventLoopProxy<WinitUserEvent>);

/// A wrapper around [`winit::event_loop::OwnedDisplayHandle`]
///
/// The `DisplayHandleWrapper` can be used to build integrations that rely on direct
/// access to the display handle
///
/// Use `Res<DisplayHandleWrapper>` to receive this resource.
#[derive(Resource, Deref)]
pub struct DisplayHandleWrapper(pub winit::event_loop::OwnedDisplayHandle);

trait AppSendEvent {
    fn send(&mut self, event: impl Into<WindowEvent>);
}

impl AppSendEvent for Vec<WindowEvent> {
    fn send(&mut self, event: impl Into<WindowEvent>) {
        self.push(Into::<WindowEvent>::into(event));
    }
}

/// The parameters of the [`create_windows`] system.
pub type CreateWindowParams<'w, 's> = (
    Commands<'w, 's>,
    Query<
        'w,
        's,
        (
            Entity,
            &'static mut Window,
            &'static CursorOptions,
            Option<&'static RawHandleWrapperHolder>,
        ),
        Added<Window>,
    >,
    MessageWriter<'w, WindowCreated>,
    ResMut<'w, WinitActionRequestHandlers>,
    Res<'w, AccessibilityRequested>,
    Res<'w, WinitMonitors>,
);

/// The parameters of the [`create_monitors`] system.
pub type CreateMonitorParams<'w, 's> = (Commands<'w, 's>, ResMut<'w, WinitMonitors>);
