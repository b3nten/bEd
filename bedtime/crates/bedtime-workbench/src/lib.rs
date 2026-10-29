//! Composable workbench services; the application supplies Bevy's window and renderer.
mod actions;
mod closing;
mod composition;
mod document_actions;
mod documents;
mod membership;
pub use composition::PanelTabStyle;
#[cfg(any(debug_assertions, feature = "persistence"))]
pub(crate) use composition::validate_workbench;
pub use document_actions::*;
pub use documents::*;
mod geometry;
mod layout;
mod lifecycle;
mod menus;
mod notifications;
#[cfg(feature = "persistence")]
pub mod persistence;
#[cfg(feature = "persistence")]
pub mod settings;
mod work;
pub use actions::*;
pub use closing::{CloseChoice, ClosePrompt, UnsavedDocument};
pub use layout::*;
pub use lifecycle::*;
pub use menus::*;
pub use notifications::{Notification, NotificationLevel, Notifications};
pub use work::*;
#[cfg(feature = "native")]
mod native;
#[cfg(feature = "clipboard")]
pub use bedtime_clipboard::{Clipboard, ClipboardPlugin};
#[cfg(feature = "native")]
pub use native::{
    NativeCapabilities, NativeError, NativeMenuBar, NativeSystems, WindowBackground, WindowBlur,
    WindowTitlebarGeometry, WorkbenchNativeMenuPlugin, WorkbenchNativePlugin,
    WorkbenchNativeWindowPlugin,
};
#[cfg(feature = "ui")]
mod dialogs;
#[cfg(feature = "ui")]
pub use dialogs::{ErrorDialog, ErrorDialogs};
#[cfg(feature = "ui")]
mod frontend;
#[cfg(feature = "ui")]
mod surface;
#[cfg(feature = "ui")]
pub use surface::{AreaBackground, TabBarVisibility, WorkbenchCamera, WorkbenchSurface};
#[cfg(feature = "ui")]
mod shortcuts;
#[cfg(feature = "ui")]
mod ui;
use bevy::{app::PluginGroupBuilder, prelude::*};
#[cfg(feature = "ui")]
pub use frontend::{
    PanelFactoryPayload, Titlebar, TitlebarOwnership, WorkbenchPresentation, WorkbenchShell,
    WorkbenchUi, panel_drag_source,
};
#[cfg(feature = "ui")]
pub use ui::{MenuPresentation, WorkbenchUiPlugin, draw_menu_bar, draw_popup_menu};

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
pub struct WorkbenchCorePlugin;
impl Plugin for WorkbenchCorePlugin {
    fn build(&self, app: &mut App) {
        lifecycle::install(app);
        #[cfg(feature = "persistence")]
        {
            persistence::register(app);
            settings::install(app);
        }
    }
}

/// The usual workbench setup. Features select compiled capabilities; this group
/// can also be customized with Bevy's PluginGroupBuilder::disable.
/// UiPlugin remains application-owned and may be registered in either order.
pub struct WorkbenchPlugin;
impl PluginGroup for WorkbenchPlugin {
    fn build(self) -> PluginGroupBuilder {
        let group = PluginGroupBuilder::start::<Self>().add(WorkbenchCorePlugin);
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

fn valid_key(key: &str) -> bool {
    let mut count = 0;
    let valid = key.split('.').all(|part| {
        count += 1;
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    });
    valid && count >= 2
}

#[cfg(test)]
mod tests;
