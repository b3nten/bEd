//! Composable workbench services; the application supplies Bevy's window and renderer.
mod closing;
mod document_actions;
mod documents;
mod lifecycle;
mod notifications;
mod opening;
#[cfg(feature = "persistence")]
pub mod persistence;
mod untitled;
#[cfg(feature = "workspaces")]
pub mod workspaces;
/// Settings: [`bedtime-settings`](bedtime_settings), re-exported.
#[cfg(feature = "persistence")]
pub mod settings {
    pub use bedtime_settings::*;
}
pub use bedtime_actions::*;
pub use bedtime_menus::*;
pub use bedtime_ui::layouts::*;
pub use bedtime_work::*;
/// The `ui` parts of the crates above: drawing menus, collecting shortcuts,
/// the Keyboard Shortcuts section.
#[cfg(feature = "ui")]
pub mod ui {
    // Its public items need the `settings` feature too.
    #[allow(unused_imports)]
    pub use bedtime_actions::ui::*;
    pub use bedtime_menus::ui::*;
}
pub use closing::{CloseChoice, ClosePrompt, UnsavedDocument};
pub use document_actions::*;
pub use documents::*;
pub use lifecycle::*;
pub use notifications::{Notification, NotificationLevel, Notifications};
pub use opening::*;
pub use untitled::{SaveAs, Untitled};
#[cfg(feature = "native")]
mod native;
#[cfg(feature = "clipboard")]
pub use bedtime_clipboard::{Clipboard, ClipboardPlugin};
#[cfg(feature = "native")]
pub use native::{NativeError, NativeSystems, WorkbenchNativePlugin, WorkbenchNativeWindowPlugin};
#[cfg(feature = "ui")]
mod dialogs;
#[cfg(feature = "ui")]
mod ui_plugin;
#[cfg(feature = "ui")]
pub use bedtime_menus::ui::{MenuPresentation, draw_menu_bar, draw_popup_menu};
use bevy::{app::PluginGroupBuilder, prelude::*};
#[cfg(feature = "ui")]
pub use ui_plugin::WorkbenchUiPlugin;

/// The folder the app works in: where a file tree starts, a terminal opens,
/// relative paths resolve. `WorkspacesPlugin` sets it to the workspace's
/// folder; without workspaces the app sets its own, or leaves it unset.
#[derive(Resource, Clone, Debug, PartialEq, Eq)]
pub struct Root(pub std::path::PathBuf);

/// A request to quit, from the native window or an application command.
/// Workbench handles it: unsaved documents are saved or asked about, background
/// work finishes, then `AppExit` is sent.
#[derive(Message, Clone, Debug, PartialEq, Eq)]
pub struct CloseRequested {
    pub window: Option<Entity>,
}

/// Session resources, layout selection, requests and orderly shutdown in the main
/// application world. Empty workbenches are valid; applications compose or restore
/// their layout/panel graphs. Installs no window or renderer.
pub struct DocumentsPlugin;
impl Plugin for DocumentsPlugin {
    fn build(&self, app: &mut App) {
        lifecycle::install(app);
        #[cfg(feature = "persistence")]
        {
            persistence::register(app);
            app.add_systems(Update, report_settings_errors);
        }
    }
    fn finish(&self, app: &mut App) {
        fn needs<P: Plugin>(app: &App, name: &str) {
            assert!(
                app.is_plugin_added::<P>(),
                "DocumentsPlugin needs {name}, part of WorkbenchCorePlugins"
            );
        }
        needs::<WorkPlugin>(app, "WorkPlugin");
        needs::<ActionsPlugin>(app, "ActionsPlugin");
        needs::<LayoutsPlugin>(app, "LayoutsPlugin");
        #[cfg(feature = "persistence")]
        {
            needs::<persistence::PersistencePlugin>(app, "PersistencePlugin");
            needs::<bedtime_settings::SettingsPlugin>(app, "SettingsPlugin");
        }
    }
}

/// Workbench without windows: persistence and settings (`persistence`
/// feature), work, actions, layouts and documents. Replace a part with
/// `.set()` or drop one with `.disable()`, as with Bevy's `DefaultPlugins`.
pub struct WorkbenchCorePlugins;
impl PluginGroup for WorkbenchCorePlugins {
    fn build(self) -> PluginGroupBuilder {
        let group = PluginGroupBuilder::start::<Self>();
        #[cfg(feature = "persistence")]
        let group = group
            .add(persistence::PersistencePlugin)
            .add(bedtime_settings::SettingsPlugin);
        group
            .add(WorkPlugin)
            .add(ActionsPlugin)
            .add(LayoutsPlugin)
            .add(DocumentsPlugin)
    }
}

/// Show settings problems (values that can't be used) as notifications.
#[cfg(feature = "persistence")]
fn report_settings_errors(
    world: &mut World,
    mut cursor: Local<bevy::ecs::message::MessageCursor<bedtime_settings::SettingsError>>,
) {
    let errors: Vec<_> = cursor
        .read(world.resource::<Messages<bedtime_settings::SettingsError>>())
        .map(|error| error.0.clone())
        .collect();
    for error in errors {
        world.workbench_mut().notify(Notification::warning(error));
    }
}

/// The usual workbench: `WorkbenchCorePlugins`, plus the clipboard, native
/// window and menus, and the UI (by feature). `UiPlugin` remains the app's and
/// may be added in either order.
pub struct WorkbenchPlugins;
impl PluginGroup for WorkbenchPlugins {
    fn build(self) -> PluginGroupBuilder {
        let group = PluginGroupBuilder::start::<Self>().add_group(WorkbenchCorePlugins);
        #[cfg(feature = "clipboard")]
        let group = group.add(ClipboardPlugin);
        #[cfg(feature = "native")]
        let group = group.add_group(WorkbenchNativePlugin);
        #[cfg(feature = "ui")]
        let group = group.add(WorkbenchUiPlugin);
        group
    }
}

/// The workbench's read operations: a view of the `World`, from
/// `world.workbench()`. Reads that need no workbench rules are methods on the
/// resources and components themselves (`Settings::get`, `Layout`).
#[repr(transparent)]
pub struct WorkbenchRef {
    world: World,
}

/// The workbench's operations: a mutable view of the `World`, from
/// `world.workbench_mut()`. Every change that has to keep the workbench's
/// rules (membership, persistence, unsaved work, shutdown) goes through it.
/// It reads like a `WorkbenchRef` too. From a system, queue a closure:
/// `commands.queue(move |world: &mut World| world.workbench_mut().save_document(id))`.
#[repr(transparent)]
pub struct Workbench {
    world: World,
}

impl WorkbenchRef {
    pub fn world(&self) -> &World {
        &self.world
    }
}

impl Workbench {
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }
}

impl std::ops::Deref for Workbench {
    type Target = WorkbenchRef;
    fn deref(&self) -> &WorkbenchRef {
        view(&self.world)
    }
}

fn view(world: &World) -> &WorkbenchRef {
    // SAFETY: WorkbenchRef is a repr(transparent) wrapper of World.
    unsafe { &*(world as *const World).cast::<WorkbenchRef>() }
}

/// `world.workbench()` and `world.workbench_mut()`.
pub trait WorkbenchWorldExt {
    fn workbench(&self) -> &WorkbenchRef;
    fn workbench_mut(&mut self) -> &mut Workbench;
}

impl WorkbenchWorldExt for World {
    fn workbench(&self) -> &WorkbenchRef {
        view(self)
    }
    fn workbench_mut(&mut self) -> &mut Workbench {
        // SAFETY: Workbench is a repr(transparent) wrapper of World.
        unsafe { &mut *(self as *mut World).cast::<Workbench>() }
    }
}

#[cfg(test)]
mod tests;
