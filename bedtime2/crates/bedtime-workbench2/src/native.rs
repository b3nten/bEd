//! Optional platform adapters. Core menus and commands have no native dependency.
use crate::{
    CloseRequested, CommandContext, CommandRequests, KeyChord, MenuNode, MenuSnapshot,
    commands::NativeShortcuts, menu_snapshot,
};
use bevy::{
    app::PluginGroupBuilder,
    prelude::*,
    window::{PrimaryWindow, RawHandleWrapper},
};
use std::{marker::PhantomData, rc::Rc};

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

/// Select one abstract MenuBar for native presentation. Other roots remain
/// available to in-application menu bars and contextual menus.
#[derive(Component)]
pub struct NativeMenuBar;

#[derive(Resource, Clone, Copy, Debug)]
pub struct NativeCapabilities {
    pub native_menus: bool,
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NativeSystems {
    Poll,
}

pub struct WorkbenchNativePlugin;
impl PluginGroup for WorkbenchNativePlugin {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            .add(WorkbenchNativeWindowPlugin)
            .add(WorkbenchNativeMenuPlugin)
    }
}
pub struct WorkbenchNativeWindowPlugin;
#[derive(Default)]
struct WindowThread(PhantomData<Rc<()>>);
impl Plugin for WorkbenchNativeWindowPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<CloseRequested>()
            .add_message::<NativeError>()
            .init_non_send::<WindowThread>()
            .add_systems(
                PreUpdate,
                forward_close_requests.in_set(NativeSystems::Poll),
            );
        #[cfg(target_os = "macos")]
        app.add_systems(
            Last,
            sync_window_background.before(bevy::app::OnAppExitSystems),
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
pub struct WorkbenchNativeMenuPlugin;
impl Plugin for WorkbenchNativeMenuPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(NativeCapabilities {
            native_menus: cfg!(target_os = "macos"),
        })
        .add_message::<NativeError>()
        .init_non_send::<NativeState>()
        .init_resource::<NativeShortcuts>()
        .add_systems(PreUpdate, poll.in_set(NativeSystems::Poll))
        .add_systems(Last, shutdown.after(bevy::app::OnAppExitSystems));
    }
    fn finish(&self, app: &mut App) {
        assert!(
            app.is_plugin_added::<crate::WorkbenchCorePlugin>(),
            "Native menus require WorkbenchCorePlugin"
        );
    }
}

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
    let roots: Vec<_> = world
        .query_filtered::<Entity, With<NativeMenuBar>>()
        .iter(world)
        .collect();
    let requested = match roots.as_slice() {
        [] => Ok(MenuSnapshot::default()),
        [root] if world.get::<crate::MenuBar>(*root).is_some() => {
            menu_snapshot(world, *root, CommandContext::Focused)
        }
        [_] => Err("NativeMenuBar must select an abstract MenuBar".into()),
        _ => Err("Only one native menu bar can be selected".into()),
    };
    #[cfg(target_os = "macos")]
    {
        let actions = world
            .non_send::<NativeState>()
            .menu
            .as_ref()
            .map_or_else(Vec::new, |menu| menu.take_activations());
        for (source, target) in actions {
            if let Some(request) = requested
                .as_ref()
                .ok()
                .and_then(|snapshot| snapshot.request(source, target))
            {
                world.resource_mut::<CommandRequests>().submit(request);
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
        if snapshot.root.is_some() {
            let replacement =
                macos::InstalledMenu::build(&snapshot).map_err(|error| error.to_string())?;
            replacement.install();
            world.non_send_mut::<NativeState>().menu = Some(replacement);
        } else if let Some(old) = world.non_send_mut::<NativeState>().menu.take() {
            old.remove();
        }
        fn collect(nodes: &[MenuNode], chords: &mut std::collections::HashSet<KeyChord>) {
            for node in nodes {
                match node {
                    MenuNode::Menu { entries, .. } => collect(entries, chords),
                    MenuNode::Item {
                        shortcut: Some(chord),
                        ..
                    } => {
                        chords.insert(KeyChord::new(chord.modifiers.resolved(), chord.key));
                    }
                    _ => {}
                }
            }
        }
        let mut chords = std::collections::HashSet::new();
        collect(&snapshot.entries, &mut chords);
        world.resource_mut::<NativeShortcuts>().0 = chords;
        Ok(snapshot)
    });
    if let Err(error) = result {
        world.write_message(NativeError(error));
    }
}

pub(crate) fn shutdown(world: &mut World) {
    if !world.resource::<Messages<AppExit>>().is_empty() {
        world.remove_non_send::<NativeState>();
        world.resource_mut::<NativeShortcuts>().0.clear();
    }
}

#[cfg(target_os = "macos")]
fn sync_window_background(
    windows: Query<
        (&crate::WindowBackground, &RawHandleWrapper),
        Or<(Changed<crate::WindowBackground>, Changed<RawHandleWrapper>)>,
    >,
    _main_thread: NonSend<WindowThread>,
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
    use crate::{CommandType, KeyChord, MenuNode, MenuSnapshot, Modifiers};
    use bevy::prelude::Entity;
    use muda::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
    use std::{collections::HashMap, io};

    pub(super) struct InstalledMenu {
        menu: Menu,
        actions: HashMap<MenuId, (Entity, CommandType)>,
    }

    fn accelerator(chord: KeyChord) -> io::Result<muda::accelerator::Accelerator> {
        use muda::accelerator::{Accelerator, Code, Modifiers as NativeModifiers};
        let mut modifiers = NativeModifiers::empty();
        let m = chord.modifiers.resolved();
        for (flag, native) in [
            (Modifiers::CONTROL, NativeModifiers::CONTROL),
            (Modifiers::SHIFT, NativeModifiers::SHIFT),
            (Modifiers::ALT, NativeModifiers::ALT),
            (Modifiers::META, NativeModifiers::META),
        ] {
            if m.contains(flag) {
                modifiers |= native;
            }
        }
        let key = format!("{:?}", chord.key).parse::<Code>().map_err(|_| {
            io::Error::other("Key cannot be represented by the native menu backend")
        })?;
        Ok(Accelerator::new(modifiers, key))
    }

    fn build_submenu(
        label: &str,
        entries: &[MenuNode],
        actions: &mut HashMap<MenuId, (Entity, CommandType)>,
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
                    ..
                } => {
                    let accelerator = shortcut.map(accelerator).transpose()?;
                    let item = MenuItem::new(label, *enabled, accelerator);
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
        pub(super) fn take_activations(&self) -> Vec<(Entity, CommandType)> {
            MenuEvent::receiver()
                .try_iter()
                .filter_map(|event| self.actions.get(&event.id).cloned())
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(!app.is_plugin_added::<WorkbenchNativeMenuPlugin>());
    }
}
