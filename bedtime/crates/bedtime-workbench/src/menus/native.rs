//! The native menu bar (`native` feature): macOS's, through muda. Elsewhere
//! nothing is installed and apps draw their egui menu bar instead.
use crate::actions::{ActionRequests, InvocationContext};
use crate::menus::{MenuBar, MenuSnapshot, MenusWorldExt};
use bevy::{
    prelude::*,
    window::{PrimaryWindow, RawHandleWrapper},
};
use std::{marker::PhantomData, rc::Rc};

/// Shows the `MenuBar` marked `NativeMenuBar` as the operating system's menu
/// bar (macOS), and runs its items' actions. Elsewhere it does nothing.
pub struct NativeMenuPlugin;
impl Plugin for NativeMenuPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<NativeMenuError>()
            .init_non_send::<NativeState>()
            .add_systems(PreUpdate, poll.in_set(NativeMenuSystems::Poll))
            .add_systems(Last, shutdown.after(bevy::app::OnAppExitSystems));
        #[cfg(feature = "ui")]
        app.configure_sets(
            PreUpdate,
            NativeMenuSystems::Poll.before(bedtime_ui::UiSystems::Draw),
        );
    }
    fn finish(&self, app: &mut App) {
        assert!(
            app.is_plugin_added::<crate::actions::ActionsPlugin>(),
            "Native menus require ActionsPlugin"
        );
    }
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NativeMenuSystems {
    /// Installing the menu bar when it changes and running chosen items, in
    /// `PreUpdate`.
    Poll,
}

/// The native menu bar couldn't be built or installed.
#[derive(Message, Clone, Debug)]
pub struct NativeMenuError(pub String);

/// Select one `MenuBar` for native presentation. Other roots remain available
/// to in-app menu bars and context menus.
#[derive(Component)]
pub struct NativeMenuBar;

#[derive(Default)]
struct NativeState {
    attempted: Option<Result<MenuSnapshot, String>>,
    installed_root: Option<Entity>,
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

/// Whether `root` is installed as the operating system's menu bar. Being on a
/// platform with native menus isn't enough (before a window exists, say).
#[cfg(feature = "ui")]
pub(crate) fn installed(world: &World, root: Entity) -> bool {
    world
        .get_non_send::<NativeState>()
        .is_some_and(|state| state.installed_root == Some(root))
}

fn poll(world: &mut World) {
    if world
        .query_filtered::<Entity, (With<PrimaryWindow>, With<RawHandleWrapper>)>()
        .iter(world)
        .next()
        .is_none()
    {
        return;
    }
    poll_menus(world);
}

// Global application menus survive an empty workbench. Panel-owned roots
// contribute only in their panel context; competing roots are errors.
fn requested_menu(world: &mut World) -> Result<MenuSnapshot, String> {
    let roots: Vec<_> = world
        .query_filtered::<Entity, With<NativeMenuBar>>()
        .iter(world)
        .collect();
    match roots.as_slice() {
        [] => Ok(MenuSnapshot::default()),
        [root] if world.get::<MenuBar>(*root).is_some() => {
            world.menus().snapshot(*root, InvocationContext::Focused)
        }
        [_] => Err("NativeMenuBar must select an abstract MenuBar".into()),
        _ => Err("Only one native menu bar can be selected".into()),
    }
}

fn poll_menus(world: &mut World) {
    let requested = requested_menu(world);
    #[cfg(target_os = "macos")]
    {
        let actions = world
            .non_send::<NativeState>()
            .menu
            .as_ref()
            .map_or_else(Vec::new, |menu| menu.take_activations());
        for (source, target, context) in actions {
            if let Some(request) = requested
                .as_ref()
                .ok()
                .and_then(|snapshot| snapshot.root)
                .and_then(|root| world.menus().snapshot(root, context).ok())
                .and_then(|snapshot| snapshot.request(source, target))
                .filter(|request| request.context == context)
            {
                world.resource_mut::<ActionRequests>().submit(request);
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
            let mut native = world.non_send_mut::<NativeState>();
            native.menu = Some(replacement);
            native.installed_root = snapshot.root;
        } else {
            let mut native = world.non_send_mut::<NativeState>();
            if let Some(old) = native.menu.take() {
                old.remove();
            }
            native.installed_root = None;
        }
        Ok(snapshot)
    });
    if let Err(error) = result {
        let mut native = world.non_send_mut::<NativeState>();
        #[cfg(target_os = "macos")]
        if let Some(old) = native.menu.take() {
            old.remove();
        }
        native.installed_root = None;
        world.write_message(NativeMenuError(error));
    }
}

fn shutdown(world: &mut World) {
    if !world.resource::<Messages<AppExit>>().is_empty() {
        world.remove_non_send::<NativeState>();
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use crate::actions::{ActionId, InvocationContext};
    use crate::menus::{MenuNode, MenuSnapshot};
    use bevy::prelude::Entity;
    use muda::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
    use std::{collections::HashMap, io};

    pub(super) struct InstalledMenu {
        menu: Menu,
        actions: HashMap<MenuId, (Entity, ActionId, InvocationContext)>,
    }

    fn accelerator(chord: crate::actions::KeyChord) -> Option<muda::accelerator::Accelerator> {
        use muda::accelerator::{Accelerator, Code, Modifiers};
        let key = format!("{:?}", chord.key).parse::<Code>().ok()?;
        let resolved = chord.modifiers.resolved();
        let mut modifiers = Modifiers::empty();
        for (source, target) in [
            (crate::actions::Modifiers::CONTROL, Modifiers::CONTROL),
            (crate::actions::Modifiers::SHIFT, Modifiers::SHIFT),
            (crate::actions::Modifiers::ALT, Modifiers::ALT),
            (crate::actions::Modifiers::META, Modifiers::META),
        ] {
            if resolved.contains(source) {
                modifiers |= target;
            }
        }
        Some(Accelerator::new(modifiers, key))
    }

    fn build_submenu(
        label: &str,
        entries: &[MenuNode],
        actions: &mut HashMap<MenuId, (Entity, ActionId, InvocationContext)>,
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
                    context,
                    ..
                } => {
                    let accelerator = shortcut.and_then(accelerator);
                    let label = match (shortcut, accelerator) {
                        (Some(chord), None) => format!("{}    {}", label, chord.label()),
                        _ => label.clone(),
                    };
                    let item = MenuItem::new(label, *enabled, accelerator);
                    if let Some(target) = target {
                        actions.insert(item.id().clone(), (*entity, target.clone(), *context));
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
        pub(super) fn take_activations(&self) -> Vec<(Entity, ActionId, InvocationContext)> {
            MenuEvent::receiver()
                .try_iter()
                .filter_map(|event| self.actions.get(&event.id).cloned())
                .collect()
        }
    }

    #[test]
    fn primary_chords_use_native_command_accelerators_not_label_suffixes() {
        use muda::accelerator::{Accelerator, Code, Modifiers};
        assert_eq!(
            accelerator(crate::actions::KeyChord::new(
                crate::actions::Modifiers::PRIMARY,
                bevy::input::keyboard::KeyCode::KeyS
            )),
            Some(Accelerator::new(Modifiers::META, Code::KeyS))
        );
        assert_eq!(
            accelerator(crate::actions::KeyChord::new(
                crate::actions::Modifiers::PRIMARY | crate::actions::Modifiers::SHIFT,
                bevy::input::keyboard::KeyCode::KeyZ
            )),
            Some(Accelerator::new(
                Modifiers::META | Modifiers::SHIFT,
                Code::KeyZ
            ))
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::ActionsPlugin;

    #[test]
    fn one_native_root_is_requested_and_two_are_an_error() {
        let mut app = App::new();
        app.add_plugins(ActionsPlugin);
        assert_eq!(requested_menu(app.world_mut()).unwrap().root, None);
        let root = app.world_mut().spawn((MenuBar, NativeMenuBar)).id();
        assert_eq!(requested_menu(app.world_mut()).unwrap().root, Some(root));
        app.world_mut().spawn((MenuBar, NativeMenuBar));
        assert!(
            requested_menu(app.world_mut()).is_err(),
            "Ambiguous roots cannot silently replace the application menu"
        );
    }
}
