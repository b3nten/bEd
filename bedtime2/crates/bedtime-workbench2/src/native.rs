//! Optional platform adapters. Core menus and commands have no native dependency.
use crate::{CloseRequested, InvocationContext, MenuSnapshot, menu_snapshot};
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

/// Select one abstract MenuBar for native presentation. Other roots remain
/// available to in-application menu bars and contextual menus.
#[derive(Component)]
pub struct NativeMenuBar;

/// Opt into a passive macOS material behind the transparent GPU surface.
/// Attach alongside WindowBackground (supplied by the workbench root).
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct WindowBlur(pub bool);

#[cfg(target_os = "macos")]
mod macos_background;

#[derive(Resource, Clone, Copy, Debug)]
pub struct NativeCapabilities {
    pub native_menus: bool,
}

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
            .add(WorkbenchNativeMenuPlugin)
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

pub struct WorkbenchNativeMenuPlugin;
impl Plugin for WorkbenchNativeMenuPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(NativeCapabilities {
            native_menus: cfg!(target_os = "macos"),
        })
        .add_message::<NativeError>()
        .configure_sets(
            PreUpdate,
            NativeSystems::Poll
                .after(crate::WorkspaceSystems::Prepare)
                .before(crate::WorkspaceSystems::Dispatch),
        )
        .init_non_send::<NativeState>()
        .add_systems(PreUpdate, poll.in_set(NativeSystems::Poll))
        .add_systems(Last, shutdown.after(bevy::app::OnAppExitSystems));
        #[cfg(feature = "ui")]
        app.configure_sets(
            PreUpdate,
            NativeSystems::Poll.before(bedtime_ui2::UiSystems::Draw),
        );
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
    workspace: Option<crate::WorkspaceId>,
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

/// Whether this abstract root currently has an installed operating-system menu.
/// Capability alone is not installation success (for example before a window exists).
pub fn native_menu_installed(world: &World, root: Entity) -> bool {
    world
        .get_non_send::<NativeState>()
        .is_some_and(|state| state.installed_root == Some(root))
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
    let id = crate::workspaces::selected(world);
    if world.non_send::<NativeState>().workspace != id {
        #[cfg(target_os = "macos")]
        if let Some(menu) = world.non_send_mut::<NativeState>().menu.take() {
            let _ = menu.take_activations();
            menu.remove();
        }
        let mut native = world.non_send_mut::<NativeState>();
        native.attempted = None;
        native.installed_root = None;
        native.workspace = id;
    }
    poll_workspace(world);
}

// Global application menus survive an empty workbench. Workspace-owned roots
// participate only while their workspace is active; competing roots are errors.
fn requested_menu(world: &mut World) -> Result<MenuSnapshot, String> {
    let roots: Vec<_> = world
        .query_filtered::<(
            Entity,
            Option<&crate::InWorkspace>,
            Has<crate::InActiveWorkspace>,
        ), With<NativeMenuBar>>()
        .iter(world)
        .filter_map(|(entity, owner, active)| (owner.is_none() || active).then_some(entity))
        .collect();
    match roots.as_slice() {
        [] => Ok(MenuSnapshot::default()),
        [root] if world.get::<crate::MenuBar>(*root).is_some() => {
            menu_snapshot(world, *root, InvocationContext::Focused)
        }
        [_] => Err("NativeMenuBar must select an abstract MenuBar".into()),
        _ => Err("Only one native menu bar can be selected".into()),
    }
}

fn poll_workspace(world: &mut World) {
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
                .and_then(|root| menu_snapshot(world, root, context).ok())
                .and_then(|snapshot| snapshot.request(source, target))
                .filter(|request| request.context == context)
            {
                let workspace = requested
                    .as_ref()
                    .ok()
                    .and_then(|menu| menu.root)
                    .and_then(|root| crate::menus::optional_menu_workspace(world, root));
                if let Some(workspace) = workspace {
                    crate::Workbench::with_workspace(world, workspace)
                        .expect("Native menu workspace")
                        .commands()
                        .submit(request);
                } else {
                    world
                        .resource_mut::<crate::GlobalCommandRequests>()
                        .submit(request);
                }
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
        world.write_message(NativeError(error));
    }
}

pub(crate) fn shutdown(world: &mut World) {
    if !world.resource::<Messages<AppExit>>().is_empty() {
        world.remove_non_send::<NativeState>();
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use crate::{ContextCommandId, InvocationContext, MenuNode, MenuSnapshot};
    use bevy::prelude::Entity;
    use muda::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
    use std::{collections::HashMap, io};

    pub(super) struct InstalledMenu {
        menu: Menu,
        actions: HashMap<MenuId, (Entity, ContextCommandId, InvocationContext)>,
    }

    fn accelerator(chord: crate::KeyChord) -> Option<muda::accelerator::Accelerator> {
        use muda::accelerator::{Accelerator, Code, Modifiers};
        let key = format!("{:?}", chord.key).parse::<Code>().ok()?;
        let resolved = chord.modifiers.resolved();
        let mut modifiers = Modifiers::empty();
        for (source, target) in [
            (crate::Modifiers::CONTROL, Modifiers::CONTROL),
            (crate::Modifiers::SHIFT, Modifiers::SHIFT),
            (crate::Modifiers::ALT, Modifiers::ALT),
            (crate::Modifiers::META, Modifiers::META),
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
        actions: &mut HashMap<MenuId, (Entity, ContextCommandId, InvocationContext)>,
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
        pub(super) fn take_activations(
            &self,
        ) -> Vec<(Entity, ContextCommandId, InvocationContext)> {
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
            accelerator(crate::KeyChord::new(
                crate::Modifiers::PRIMARY,
                bevy::input::keyboard::KeyCode::KeyS
            )),
            Some(Accelerator::new(Modifiers::META, Code::KeyS))
        );
        assert_eq!(
            accelerator(crate::KeyChord::new(
                crate::Modifiers::PRIMARY | crate::Modifiers::SHIFT,
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
    #[test]
    fn global_native_root_survives_empty_workbench_and_workspace_switches() {
        let mut app = App::new();
        app.add_plugins(crate::WorkbenchCorePlugin);
        let root = app.world_mut().spawn((crate::MenuBar, NativeMenuBar)).id();
        assert_eq!(requested_menu(app.world_mut()).unwrap().root, Some(root));
        let workspace = crate::Workspaces::new(app.world_mut())
            .open("Project")
            .entity();
        assert_eq!(requested_menu(app.world_mut()).unwrap().root, Some(root));
        app.world_mut().despawn(workspace);
        assert_eq!(requested_menu(app.world_mut()).unwrap().root, Some(root));
        app.world_mut().spawn((crate::MenuBar, NativeMenuBar));
        assert!(
            requested_menu(app.world_mut()).is_err(),
            "Ambiguous roots cannot silently replace the application menu"
        );
    }

    #[test]
    fn inactive_workspace_native_roots_do_not_compete_with_global_root() {
        let mut app = App::new();
        app.add_plugins(crate::WorkbenchCorePlugin);
        let workspace = crate::Workspaces::new(app.world_mut())
            .open("Project")
            .entity();
        app.world_mut()
            .entity_mut(workspace)
            .remove::<crate::ActiveWorkspace>();
        app.world_mut()
            .spawn((crate::MenuBar, NativeMenuBar, crate::InWorkspace(workspace)));
        let root = app.world_mut().spawn((crate::MenuBar, NativeMenuBar)).id();
        assert_eq!(requested_menu(app.world_mut()).unwrap().root, Some(root));
    }

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
