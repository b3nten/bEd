#[cfg(target_os = "macos")]
use crate::DesktopAction;
use crate::{
    DesktopClipboard, DesktopError,
    menus::{self, MenuSnapshot},
};
use bevy::{
    prelude::*,
    window::{PrimaryWindow, RawHandleWrapper},
};
use std::{marker::PhantomData, rc::Rc};

#[derive(Default)]
pub(crate) struct NativeState {
    attempted: Option<Result<MenuSnapshot, String>>,
    #[cfg(target_os = "macos")]
    menu: Option<macos::InstalledMenu>,
    _main_thread: PhantomData<Rc<()>>,
}

impl Drop for NativeState {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some(menu) = &self.menu {
            menu.remove();
        }
    }
}

pub(crate) fn poll(world: &mut World) {
    if world
        .query_filtered::<Entity, (With<PrimaryWindow>, With<RawHandleWrapper>)>()
        .iter(world)
        .next()
        .is_none()
    {
        return;
    }
    if !world.contains_non_send::<DesktopClipboard>() {
        world.insert_non_send(DesktopClipboard::open());
    }
    let requested = menus::snapshot(world);
    #[cfg(target_os = "macos")]
    {
        let actions = world
            .non_send::<NativeState>()
            .menu
            .as_ref()
            .map_or_else(Vec::new, |menu| menu.take_activations());
        for (source, target) in actions {
            if requested
                .as_ref()
                .is_ok_and(|snapshot| snapshot.allows(source, target))
            {
                world.write_message(DesktopAction::Activated { source, target });
            }
        }
    }
    if world.non_send::<NativeState>().attempted.as_ref() == Some(&requested) {
        return;
    }
    world.non_send_mut::<NativeState>().attempted = Some(requested.clone());
    let result = requested;
    #[cfg(target_os = "macos")]
    let result = result.and_then(|snapshot| {
        let replacement =
            macos::InstalledMenu::build(&snapshot).map_err(|error| error.to_string())?;
        replacement.install();
        world.non_send_mut::<NativeState>().menu = Some(replacement);
        Ok(snapshot)
    });
    if let Err(error) = result {
        world.write_message(DesktopError(error));
    }
}

pub(crate) fn shutdown(world: &mut World) {
    if !world.resource::<Messages<AppExit>>().is_empty() {
        world.remove_non_send::<NativeState>();
        world.remove_non_send::<DesktopClipboard>();
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn sync_window_background(
    windows: Query<
        (&crate::WindowBackground, &RawHandleWrapper),
        Or<(Changed<crate::WindowBackground>, Changed<RawHandleWrapper>)>,
    >,
    _main_thread: NonSend<NativeState>,
) {
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
        NSColor, NSView,
    };
    use objc2_foundation::MainThreadMarker;
    use raw_window_handle::RawWindowHandle;
    for (background, handle) in &windows {
        MainThreadMarker::new().expect("AppKit window updates run on the main thread");
        let RawWindowHandle::AppKit(handle) = handle.get_window_handle() else {
            continue;
        };
        // SAFETY: Bevy retains the live window behind this NSView handle. This
        // system is main-thread-only and never stores a borrowed native pointer.
        let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        let window = view.window().expect("Live AppKit view must have a window");
        let [r, g, b, a] = background.0.map(f64::from);
        let color = NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a);
        window.setBackgroundColor(Some(&color));
        // Native title text and controls must contrast with the backing color.
        let appearance = NSAppearance::appearanceNamed(unsafe {
            if r * 0.2126 + g * 0.7152 + b * 0.0722 > 0.5 {
                NSAppearanceNameAqua
            } else {
                NSAppearanceNameDarkAqua
            }
        });
        window.setAppearance(appearance.as_deref());
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use crate::menus::{MenuNode, MenuSnapshot};
    use bevy::prelude::Entity;
    use muda::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
    use std::{collections::HashMap, io};

    pub(super) struct InstalledMenu {
        menu: Menu,
        actions: HashMap<MenuId, (Entity, Entity)>,
    }

    fn build_submenu(
        label: &str,
        entries: &[MenuNode],
        actions: &mut HashMap<MenuId, (Entity, Entity)>,
    ) -> io::Result<Submenu> {
        let submenu = Submenu::new(label, true);
        for entry in entries {
            match entry {
                MenuNode::Menu { label, entries, .. } => {
                    submenu
                        .append(&build_submenu(label, entries, actions)?)
                        .map_err(io::Error::other)?;
                }
                MenuNode::Item {
                    entity,
                    label,
                    enabled,
                    shortcut,
                    target,
                } => {
                    let item = MenuItem::new(label, *enabled, *shortcut);
                    if let Some(target) = target {
                        actions.insert(item.id().clone(), (*entity, *target));
                    }
                    submenu.append(&item).map_err(io::Error::other)?;
                }
                MenuNode::Separator(_) => {
                    submenu
                        .append(&PredefinedMenuItem::separator())
                        .map_err(io::Error::other)?;
                }
            }
        }
        Ok(submenu)
    }

    impl InstalledMenu {
        pub(super) fn build(snapshot: &MenuSnapshot) -> io::Result<Self> {
            let menu = Menu::new();
            let mut actions = HashMap::new();
            for group in &snapshot.entries {
                let MenuNode::Menu { label, entries, .. } = group else {
                    unreachable!("menu snapshots validate top-level groups")
                };
                menu.append(&build_submenu(label, entries, &mut actions)?)
                    .map_err(io::Error::other)?;
            }
            Ok(Self { menu, actions })
        }

        pub(super) fn install(&self) {
            self.menu.init_for_nsapp();
        }
        pub(super) fn remove(&self) {
            self.menu.remove_for_nsapp();
        }
        pub(super) fn take_activations(&self) -> Vec<(Entity, Entity)> {
            MenuEvent::receiver()
                .try_iter()
                .filter_map(|event| self.actions.get(&event.id).cloned())
                .collect()
        }
    }
}
