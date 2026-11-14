//! The OS's light or dark setting, as `SystemAppearance`, kept current by
//! `UiPlugin` so an app can pick a theme to match.
//!
//! On macOS it's read from the application, not the window: bedtime pins the
//! window's appearance to its theme when it draws the titlebar, so the
//! window's (and so Winit's `WindowThemeChanged`) would report that instead.
//! Elsewhere it's the primary window's theme, as Winit reports it.
use super::Appearance;
use bevy::{
    prelude::*,
    window::{PrimaryWindow, WindowTheme, WindowThemeChanged},
};

/// Whether the OS is set to light or dark; `None` when it doesn't say (some
/// Linux desktops, or before the window exists).
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SystemAppearance(pub Option<Appearance>);

impl From<WindowTheme> for Appearance {
    fn from(theme: WindowTheme) -> Self {
        match theme {
            WindowTheme::Light => Self::Light,
            WindowTheme::Dark => Self::Dark,
        }
    }
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<SystemAppearance>();
    #[cfg(all(target_os = "macos", feature = "native"))]
    app.init_non_send::<macos::Observer>()
        .add_systems(Startup, macos::observe)
        .add_systems(PreUpdate, macos::follow.before(crate::UiSystems::Draw));
    #[cfg(not(all(target_os = "macos", feature = "native")))]
    app.add_systems(
        PreUpdate,
        follow_window_theme.before(crate::UiSystems::Draw),
    );
}

/// The primary window's theme: its latest change, or the one it was created
/// with.
#[cfg_attr(all(target_os = "macos", feature = "native"), allow(dead_code))]
fn follow_window_theme(
    windows: Query<(Entity, &Window), With<PrimaryWindow>>,
    mut changes: MessageReader<WindowThemeChanged>,
    mut system: ResMut<SystemAppearance>,
) {
    let Ok((primary, window)) = windows.single() else {
        changes.clear();
        return;
    };
    let changed = changes
        .read()
        .filter(|change| change.window == primary)
        .last()
        .map(|change| change.theme);
    let theme = changed.or(system.0.is_none().then_some(window.window_theme).flatten());
    if let Some(theme) = theme {
        system.set_if_neq(SystemAppearance(Some(theme.into())));
    }
}

#[cfg(all(target_os = "macos", feature = "native"))]
mod macos {
    use super::{Appearance, SystemAppearance};
    use crate::UiWaker;
    use bevy::prelude::*;
    use objc2::{
        DefinedClass, MainThreadOnly, define_class, msg_send, rc::Retained, runtime::AnyObject,
    };
    use objc2_app_kit::{
        NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication,
    };
    use objc2_foundation::{MainThreadMarker, NSArray, NSObject, NSString, ns_string};
    use std::{cell::Cell, ffi::c_void};

    pub(super) struct Watch {
        /// Set when the appearance may have changed, until it's next read.
        changed: Cell<bool>,
        waker: UiWaker,
    }

    define_class!(
        // SAFETY: NSObject has no subclassing requirements, and this only
        // overrides the key-value observing callback.
        #[unsafe(super(NSObject))]
        #[name = "BedtimeSystemAppearanceObserver"]
        #[thread_kind = MainThreadOnly]
        #[ivars = Watch]
        pub(super) struct AppearanceObserver;

        impl AppearanceObserver {
            #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
            fn observe_value(
                &self,
                _key_path: Option<&NSString>,
                _object: Option<&AnyObject>,
                _change: Option<&AnyObject>,
                _context: *mut c_void,
            ) {
                self.ivars().changed.set(true);
                self.ivars().waker.wake();
            }
        }
    );

    /// The observer on the application's `effectiveAppearance`, once
    /// installed; removed when dropped.
    #[derive(Default)]
    pub(super) struct Observer(Option<Retained<AppearanceObserver>>);

    fn key_path() -> &'static NSString {
        ns_string!("effectiveAppearance")
    }

    impl Drop for Observer {
        fn drop(&mut self) {
            if let Some(observer) = self.0.take()
                && let Some(mtm) = MainThreadMarker::new()
            {
                let app = NSApplication::sharedApplication(mtm);
                // SAFETY: `observer` was added for this key path in `observe`.
                let _: () =
                    unsafe { msg_send![&*app, removeObserver: &*observer, forKeyPath: key_path()] };
            }
        }
    }

    /// Read the appearance now, and watch for changes. A change wakes a
    /// reactive app, which may be in the background while the setting's
    /// changed.
    pub(super) fn observe(
        _main_thread: bevy::ecs::system::NonSendMarker,
        mut observer: NonSendMut<Observer>,
        waker: Option<Res<UiWaker>>,
        mut system: ResMut<SystemAppearance>,
    ) {
        let mtm = MainThreadMarker::new().expect("AppKit is observed on the main thread");
        let app = NSApplication::sharedApplication(mtm);
        system.set_if_neq(SystemAppearance(Some(appearance_of(
            &app.effectiveAppearance(),
        ))));
        let watch = Watch {
            changed: Cell::new(false),
            waker: waker.map(|waker| waker.clone()).unwrap_or_default(),
        };
        let this = AppearanceObserver::alloc(mtm).set_ivars(watch);
        // SAFETY: NSObject's designated initializer.
        let this: Retained<AppearanceObserver> = unsafe { msg_send![super(this), init] };
        // SAFETY: `effectiveAppearance` is key-value observable on
        // NSApplication; the observer is removed when `Observer` is dropped.
        let _: () = unsafe {
            msg_send![
                &*app,
                addObserver: &*this,
                forKeyPath: key_path(),
                options: 0usize,
                context: std::ptr::null_mut::<c_void>()
            ]
        };
        observer.0 = Some(this);
    }

    /// Read the appearance again when it may have changed.
    pub(super) fn follow(
        _main_thread: bevy::ecs::system::NonSendMarker,
        observer: NonSend<Observer>,
        mut system: ResMut<SystemAppearance>,
    ) {
        let Some(observer) = &observer.0 else {
            return;
        };
        if !observer.ivars().changed.replace(false) {
            return;
        }
        let mtm = MainThreadMarker::new().expect("AppKit is read on the main thread");
        let app = NSApplication::sharedApplication(mtm);
        system.set_if_neq(SystemAppearance(Some(appearance_of(
            &app.effectiveAppearance(),
        ))));
    }

    /// Light or dark, counting variants (high contrast, say) as their base.
    pub(super) fn appearance_of(appearance: &NSAppearance) -> Appearance {
        // SAFETY: AppKit's appearance name constants.
        let (light, dark) = unsafe { (NSAppearanceNameAqua, NSAppearanceNameDarkAqua) };
        let names = NSArray::from_slice(&[light, dark]);
        match appearance.bestMatchFromAppearancesWithNames(&names) {
            Some(best) if &*best == dark => Appearance::Dark,
            _ => Appearance::Light,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn aqua_appearances_are_light_or_dark() {
            let named = |name| NSAppearance::appearanceNamed(name).unwrap();
            // SAFETY: AppKit's appearance name constants.
            let (light, dark) = unsafe { (NSAppearanceNameAqua, NSAppearanceNameDarkAqua) };
            assert_eq!(appearance_of(&named(light)), Appearance::Light);
            assert_eq!(appearance_of(&named(dark)), Appearance::Dark);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    #[test]
    fn follows_the_primary_window_theme_and_its_changes() {
        let mut world = World::new();
        world.init_resource::<SystemAppearance>();
        world.init_resource::<Messages<WindowThemeChanged>>();
        let primary = world
            .spawn((
                Window {
                    window_theme: Some(WindowTheme::Dark),
                    ..default()
                },
                PrimaryWindow,
            ))
            .id();
        let other = world.spawn(Window::default()).id();
        let appearance = |world: &World| world.resource::<SystemAppearance>().0;

        // The theme the window was created with.
        world.run_system_once(follow_window_theme).unwrap();
        assert_eq!(appearance(&world), Some(Appearance::Dark));

        // Only the primary window's changes count.
        world.write_message(WindowThemeChanged {
            window: other,
            theme: WindowTheme::Light,
        });
        world.run_system_once(follow_window_theme).unwrap();
        assert_eq!(appearance(&world), Some(Appearance::Dark));

        world.write_message(WindowThemeChanged {
            window: primary,
            theme: WindowTheme::Light,
        });
        world.run_system_once(follow_window_theme).unwrap();
        assert_eq!(appearance(&world), Some(Appearance::Light));
    }
}
