//! Composable workbench services; the application supplies Bevy's window and renderer.
mod commands;
mod geometry;
mod layout;
mod menus;
mod work;
mod workspaces;
pub use commands::*;
pub use layout::*;
pub use menus::*;
pub use work::*;
pub use workspaces::*;
#[cfg(feature = "native")]
mod native;
#[cfg(feature = "clipboard")]
pub use bedtime_clipboard2::{Clipboard, ClipboardPlugin};
#[cfg(feature = "native")]
pub use native::{
    NativeCapabilities, NativeError, NativeMenuBar, NativeSystems, WindowBackground, WindowBlur,
    WindowTitlebarInset, WorkbenchNativeMenuPlugin, WorkbenchNativePlugin,
    WorkbenchNativeWindowPlugin,
};
#[cfg(feature = "ui")]
mod frontend;
#[cfg(feature = "ui")]
mod input;
#[cfg(feature = "ui")]
mod ui;
#[cfg(feature = "ui")]
pub use bedtime_ui2::PanelSettings;
use bevy::{app::PluginGroupBuilder, prelude::*};
#[cfg(feature = "ui")]
pub use frontend::{
    PanelTypePayload, WorkbenchHost, WorkbenchPresentation, WorkspaceUi, WorkspaceViewport,
    attach_surface, panel_drag_source, register_panel, register_panel_in_area,
};
#[cfg(feature = "ui")]
pub use input::{PanelInput, register_panel_input};
#[cfg(feature = "ui")]
pub use ui::{MenuPresentation, WorkbenchUiPlugin, draw_menu_bar, draw_popup_menu};

/// Application close intention, shared by native window requests and commands.
#[derive(Message, Clone, Debug, PartialEq, Eq)]
pub struct CloseRequested {
    pub window: Option<Entity>,
}

/// Host registry, workspace scheduling and shutdown. Creates one active Untitled
/// runtime; installs no renderer, native services or clipboard.
pub struct WorkbenchCorePlugin;
impl Plugin for WorkbenchCorePlugin {
    fn build(&self, app: &mut App) {
        workspaces::install(app);
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

/// Controlled operations in one selected workspace world. A host selects its
/// active world once; a workspace system keeps its own local world.
pub struct Workbench<'w> {
    pub(crate) world: &'w mut World,
}

impl<'w> Workbench<'w> {
    pub fn new(world: &'w mut World) -> Self {
        let world = workspaces::selected_world(world, None).expect("Active workspace");
        assert!(
            world.contains_resource::<ContextCommandRequests>(),
            "Install WorkbenchCorePlugin before using workbench"
        );
        Self { world }
    }

    pub fn with_workspace(world: &'w mut World, workspace: WorkspaceId) -> Result<Self, String> {
        Ok(Self {
            world: workspaces::selected_world(world, Some(workspace))?,
        })
    }

    /// The selected world remains fixed for the lifetime of this borrow.
    pub fn world(&self) -> &World {
        self.world
    }

    pub fn world_mut(&mut self) -> &mut World {
        self.world
    }

    pub fn workspace_id(&self) -> WorkspaceId {
        self.world.resource::<WorkspaceIdentity>().id()
    }

    pub fn work(&mut self) -> Mut<'_, WorkspaceWork> {
        self.world.resource_mut::<WorkspaceWork>()
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
