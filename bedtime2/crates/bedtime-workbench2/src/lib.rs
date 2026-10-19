//! Composable workbench services; the application supplies Bevy's window and renderer.
mod commands;
mod composition;
mod document_commands;
mod documents;
mod membership;
mod storage;
pub use composition::{PanelTabStyle, reconcile_workbench};
pub use document_commands::*;
pub use documents::*;
pub use storage::*;
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
    WindowTitlebarGeometry, WorkbenchNativeMenuPlugin, WorkbenchNativePlugin,
    WorkbenchNativeWindowPlugin,
};
#[cfg(feature = "ui")]
mod dialogs;
#[cfg(feature = "ui")]
pub use dialogs::{ErrorDialog, ErrorDialogs, request_error_dialog};
#[cfg(feature = "ui")]
mod frontend;
#[cfg(feature = "ui")]
mod surface;
#[cfg(feature = "ui")]
pub use surface::{TabBarVisibility, WorkbenchCamera, WorkbenchSurface};
#[cfg(feature = "ui")]
mod shortcuts;
#[cfg(feature = "ui")]
mod ui;
use bevy::{app::PluginGroupBuilder, prelude::*};
#[cfg(feature = "ui")]
pub use frontend::{
    PanelTypePayload, Titlebar, TitlebarOwnership, WorkbenchPresentation, WorkbenchShell,
    WorkspaceUi, panel_drag_source, register_panel, register_panel_in_area,
};
#[cfg(feature = "ui")]
pub use ui::{MenuPresentation, WorkbenchUiPlugin, draw_menu_bar, draw_popup_menu};

/// Application close intention, shared by native window requests and commands.
#[derive(Message, Clone, Debug, PartialEq, Eq)]
pub struct CloseRequested {
    pub window: Option<Entity>,
}

/// Workspace entities, marker synchronization, requests and shutdown in the main
/// application world. Empty workbenches are valid; applications compose or restore
/// their workspace graphs. Installs no window or renderer.
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

/// Controlled operations for a selected workspace in the application world.
pub struct Workbench<'w> {
    pub(crate) world: &'w mut World,
    pub(crate) workspace: WorkspaceId,
}
impl<'w> Workbench<'w> {
    /// Access the active workspace. This legacy convenience panics when none is selected.
    pub fn new(world: &'w mut World) -> Self {
        let workspace = workspaces::active(world);
        Self::with_workspace(world, workspace).expect("Active workspace")
    }
    pub fn with_workspace(world: &'w mut World, workspace: WorkspaceId) -> Result<Self, String> {
        workspaces::validate(world, workspace)?;
        Ok(Self { world, workspace })
    }
    /// Select the owner of a panel, layout or other workspace member.
    pub fn for_entity(world: &'w mut World, entity: Entity) -> Result<Self, String> {
        let workspace = if world.get::<Workspace>(entity).is_some() {
            WorkspaceId(entity)
        } else {
            workspaces::owner(world, entity).ok_or("Entity has no workspace")?
        };
        Self::with_workspace(world, workspace)
    }
    pub fn world(&self) -> &World {
        self.world
    }
    pub fn world_mut(&mut self) -> &mut World {
        self.world
    }
    pub fn workspace_id(&self) -> WorkspaceId {
        self.workspace
    }
    pub fn workspace(&self) -> &Workspace {
        self.world.get::<Workspace>(self.workspace.0).unwrap()
    }
    pub fn work(&mut self) -> Mut<'_, WorkspaceWork> {
        self.world
            .get_mut::<WorkspaceWork>(self.workspace.0)
            .unwrap()
    }
    pub fn requests(&mut self) -> Mut<'_, WorkbenchRequests> {
        self.world
            .get_mut::<WorkbenchRequests>(self.workspace.0)
            .unwrap()
    }
    pub fn commands(&mut self) -> Mut<'_, ContextCommandRequests> {
        self.world
            .get_mut::<ContextCommandRequests>(self.workspace.0)
            .unwrap()
    }
    pub(crate) fn require_owned(&self, entity: Entity) -> Result<(), String> {
        if workspaces::owner(self.world, entity) != Some(self.workspace) {
            return Err("Entity belongs to another workspace or is unavailable".into());
        }
        Ok(())
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
mod document_integration_tests;
#[cfg(test)]
mod tests;
