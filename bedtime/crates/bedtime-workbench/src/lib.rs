//! Composable workbench services; the application supplies Bevy's window and renderer.
pub mod actions;
#[cfg(all(feature = "ui", feature = "persistence"))]
mod app;
#[cfg(all(feature = "ui", feature = "persistence"))]
pub mod appearance;
#[cfg(feature = "documents")]
mod closing;
#[cfg(feature = "documents")]
mod document_actions;
#[cfg(feature = "documents")]
mod documents;
mod exit;
#[cfg(feature = "files")]
mod formats;
mod lifecycle;
pub mod menus;
mod notifications;
#[cfg(feature = "documents")]
mod opening;
#[cfg(feature = "ui")]
pub mod palette;
mod panel_actions;
#[cfg(feature = "persistence")]
pub mod persistence;
mod session;
#[cfg(feature = "persistence")]
pub mod settings;
#[cfg(feature = "ui")]
pub mod status;
#[cfg(feature = "documents")]
mod untitled;
pub mod work;
#[cfg(feature = "workspaces")]
pub mod workspaces;
pub use actions::*;
#[cfg(all(feature = "ui", feature = "persistence"))]
pub use app::{AppInfo, AppPlugins, Quit, ShellPlugin, driven_by_script};
pub use bedtime_ui::layouts::*;
pub use menus::*;
pub use work::*;
/// The `ui` parts of the modules above: drawing menus, collecting shortcuts,
/// the Keyboard Shortcuts section.
#[cfg(feature = "ui")]
pub mod ui {
    // Its public items need the `settings` feature too.
    #[allow(unused_imports)]
    pub use crate::actions::ui::*;
    pub use crate::menus::ui::*;
}
#[cfg(feature = "documents")]
pub use closing::{CloseChoice, ClosePrompt, UnsavedDocument};
#[cfg(feature = "documents")]
pub use document_actions::*;
#[cfg(feature = "documents")]
pub use documents::*;
pub use exit::{ExitAppExt, ExitGate, ExitSystems, Gate};
#[cfg(feature = "files")]
pub use formats::*;
pub use lifecycle::*;
pub use notifications::{Notification, NotificationLevel, Notifications};
#[cfg(feature = "documents")]
pub use opening::*;
pub use panel_actions::{
    ClosePanel, PanelsAppExt, ResetZoom, Show, ShowPanel, ToggleMaximizePanel, ZoomIn, ZoomOut,
    open_new,
};
pub use session::{InWorkspace, OnWorkspaceClose, OnWorkspaceClosing, OnWorkspaceOpen};
#[cfg(feature = "documents")]
pub use untitled::{SaveAs, Untitled};
#[cfg(feature = "native")]
mod native;
#[cfg(feature = "clipboard")]
pub use bedtime_clipboard::{Clipboard, ClipboardPlugin};
#[cfg(feature = "native")]
pub use native::{
    DockMenu, DockMenuAppExt, DockMenuPlugin, NativeError, NativeSystems, WorkbenchNativePlugin,
    WorkbenchNativeWindowPlugin,
};
#[cfg(all(feature = "ui", feature = "documents"))]
mod dialogs;
#[cfg(feature = "ui")]
mod ui_plugin;
use bevy::{app::PluginGroupBuilder, prelude::*};
#[cfg(feature = "ui")]
pub use menus::ui::{MenuPresentation, draw_menu_bar, draw_popup_menu};
#[cfg(feature = "ui")]
pub use palette::{CommandPalette, CommandPalettePlugin, ShowCommandPalette};
#[cfg(feature = "ui")]
pub use ui_plugin::WorkbenchUiPlugin;

/// The folder the app works in: where a file tree starts, a terminal opens,
/// relative paths resolve. `WorkspacesPlugin` sets it to the workspace's
/// folder; without workspaces the app sets its own, or leaves it unset.
#[derive(Resource, Clone, Debug, PartialEq, Eq)]
pub struct Root(pub std::path::PathBuf);

/// A request to quit, from the native window or an application command.
/// The session handles it (`exit`): what would be lost is settled (unsaved
/// documents are saved or asked about), background work finishes, then
/// `AppExit` is sent.
#[derive(Message, Clone, Debug, PartialEq, Eq)]
pub struct CloseRequested {
    pub window: Option<Entity>,
}

/// The session: notifications, quitting (`exit`), the workspace's lifetime
/// (`OnWorkspaceOpen` and the rest), which panel actions are for, and saving
/// the workbench (`persistence` feature). An empty workbench is valid: the
/// app composes or restores its layouts and panels. No window or renderer.
pub struct SessionPlugin;
impl Plugin for SessionPlugin {
    fn build(&self, app: &mut App) {
        lifecycle::install(app);
        session::install(app);
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
                "SessionPlugin needs {name}, part of AppCorePlugins"
            );
        }
        needs::<WorkPlugin>(app, "WorkPlugin");
        needs::<ActionsPlugin>(app, "ActionsPlugin");
        needs::<LayoutsPlugin>(app, "LayoutsPlugin");
        #[cfg(feature = "persistence")]
        {
            needs::<persistence::PersistencePlugin>(app, "PersistencePlugin");
            needs::<settings::SettingsPlugin>(app, "SettingsPlugin");
            persistence::finish(app);
        }
    }
}

/// Documents: their store, formats and viewers, opening by path, New, Save,
/// Undo and Redo (in the menus, for the focused panel's document), and
/// keeping unsaved ones through closing a panel or exiting (with the `ui`
/// feature, the prompt that asks). For an app whose panels show documents;
/// one without them leaves it out.
#[cfg(feature = "documents")]
pub struct DocumentsPlugin;
#[cfg(feature = "documents")]
impl Plugin for DocumentsPlugin {
    fn build(&self, app: &mut App) {
        lifecycle::install_documents(app);
        #[cfg(feature = "ui")]
        app.add_modal(crate::dialogs::draw_close_prompt);
    }
    fn finish(&self, app: &mut App) {
        assert!(
            app.is_plugin_added::<SessionPlugin>(),
            "DocumentsPlugin needs SessionPlugin, part of AppCorePlugins"
        );
    }
}

/// An app's services without windows: persistence and settings
/// (`persistence` feature), work, actions, layouts and the session. Replace
/// a part with `.set()` or drop one with `.disable()`, as with Bevy's
/// `DefaultPlugins`.
pub struct AppCorePlugins;
impl PluginGroup for AppCorePlugins {
    fn build(self) -> PluginGroupBuilder {
        let group = PluginGroupBuilder::start::<Self>();
        #[cfg(feature = "persistence")]
        let group = group
            .add(persistence::PersistencePlugin::default())
            .add(settings::SettingsPlugin);
        group
            .add(WorkPlugin)
            .add(ActionsPlugin)
            .add(LayoutsPlugin)
            .add(SessionPlugin)
    }
}

/// Workbench without windows: `AppCorePlugins` and documents.
#[cfg(feature = "documents")]
pub struct WorkbenchCorePlugins;
#[cfg(feature = "documents")]
impl PluginGroup for WorkbenchCorePlugins {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            .add_group(AppCorePlugins)
            .add(DocumentsPlugin)
    }
}

/// Show settings problems (values that can't be used) as notifications.
#[cfg(feature = "persistence")]
fn report_settings_errors(
    world: &mut World,
    mut cursor: Local<bevy::ecs::message::MessageCursor<settings::SettingsError>>,
) {
    let errors: Vec<_> = cursor
        .read(world.resource::<Messages<settings::SettingsError>>())
        .map(|error| error.0.clone())
        .collect();
    for error in errors {
        world.bedtime_mut().notify(Notification::warning(error));
    }
}

/// An app's services with its window: `AppCorePlugins`, plus the clipboard,
/// native window and menus, the UI and appearance (by feature). `UiPlugin`
/// remains the app's and may be added in either order. No documents: those
/// are `DocumentsPlugins`.
pub struct AppWindowPlugins;
impl PluginGroup for AppWindowPlugins {
    fn build(self) -> PluginGroupBuilder {
        let group = PluginGroupBuilder::start::<Self>().add_group(AppCorePlugins);
        #[cfg(feature = "clipboard")]
        let group = group.add(ClipboardPlugin);
        #[cfg(feature = "native")]
        let group = group.add_group(WorkbenchNativePlugin);
        #[cfg(feature = "ui")]
        let group = group.add(WorkbenchUiPlugin);
        let group = group.add(StandardMenusPlugin::default());
        #[cfg(all(feature = "ui", feature = "persistence"))]
        let group = group.add(appearance::AppearancePlugin);
        group
    }
}

/// Documents, for an app that has them: `DocumentsPlugin`, and its settings
/// (`persistence` feature). Add it after the app's services (`AppPlugins`,
/// `AppWindowPlugins` or `AppCorePlugins`).
#[cfg(feature = "documents")]
pub struct DocumentsPlugins;
#[cfg(feature = "documents")]
impl PluginGroup for DocumentsPlugins {
    fn build(self) -> PluginGroupBuilder {
        let group = PluginGroupBuilder::start::<Self>().add(DocumentsPlugin);
        #[cfg(feature = "persistence")]
        let group = group.add(DocumentSettingsPlugin);
        group
    }
}

/// The usual workbench: `AppWindowPlugins` and `DocumentsPlugins`.
#[cfg(feature = "documents")]
pub struct WorkbenchPlugins;
#[cfg(feature = "documents")]
impl PluginGroup for WorkbenchPlugins {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            .add_group(AppWindowPlugins)
            .add_group(DocumentsPlugins)
    }
}

/// bedtime's read operations: a view of the `World`, from `world.bedtime()`.
/// One name for all of them: its own (documents, notifications, the session)
/// and each part's, by name: `layouts()`, `actions()`, `menus()`,
/// `settings()`, `persistence()`. Reads that need no rules are methods on
/// the resources and components themselves (`Layout`, `Area`).
#[repr(transparent)]
pub struct BedtimeRef {
    world: World,
}

/// bedtime's operations: a mutable view of the `World`, from
/// `world.bedtime_mut()`. Every change that has to keep bedtime's rules
/// (membership, persistence, unsaved work, shutdown) goes through it, or
/// through one of its parts (`layouts_mut()`, `settings_mut()`,
/// `actions_mut()`, `persistence_mut()`). It reads like a `BedtimeRef` too.
/// From a system, queue a closure:
/// `commands.queue(move |world: &mut World| world.bedtime_mut().save_document(id))`.
#[repr(transparent)]
pub struct Bedtime {
    world: World,
}

impl BedtimeRef {
    pub fn world(&self) -> &World {
        &self.world
    }

    /// Layouts, their areas and panels.
    pub fn layouts(&self) -> &LayoutsRef {
        LayoutsWorldExt::layouts(&self.world)
    }

    /// Actions: what's available, and its keys.
    pub fn actions(&self) -> &ActionsRef {
        actions::ActionsWorldExt::actions(&self.world)
    }

    /// Menus, as they'd show.
    pub fn menus(&self) -> &Menus {
        menus::MenusWorldExt::menus(&self.world)
    }

    /// Settings, as they resolve through their layers.
    #[cfg(feature = "persistence")]
    pub fn settings(&self) -> &settings::Settings {
        settings::SettingsWorldExt::settings(&self.world)
    }

    /// What's saved, and where.
    #[cfg(feature = "persistence")]
    pub fn persistence(&self) -> &persistence::PersistenceRef {
        persistence::PersistenceWorldExt::persistence(&self.world)
    }
}

impl Bedtime {
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// Changing layouts: creating, activating, splitting, closing.
    pub fn layouts_mut(&mut self) -> &mut Layouts {
        LayoutsWorldExt::layouts_mut(&mut self.world)
    }

    /// Running actions, and changing what handles them.
    pub fn actions_mut(&mut self) -> &mut Actions {
        actions::ActionsWorldExt::actions_mut(&mut self.world)
    }

    /// Changing settings, in a layer.
    #[cfg(feature = "persistence")]
    pub fn settings_mut(&mut self) -> &mut settings::SettingsMut {
        settings::SettingsWorldExt::settings_mut(&mut self.world)
    }

    /// Capturing and restoring what's saved.
    #[cfg(feature = "persistence")]
    pub fn persistence_mut(&mut self) -> &mut persistence::Persistence {
        persistence::PersistenceWorldExt::persistence_mut(&mut self.world)
    }

    /// Spawn `bundle` (a panel and its state) in the focused area, placed
    /// and focused when this returns.
    pub fn open_panel(&mut self, bundle: impl Bundle) -> Entity {
        PanelsWorldExt::open_panel(&mut self.world, bundle)
    }

    /// Spawn `bundle` at `placement`, placed and focused when this returns;
    /// an error if it couldn't be placed.
    pub fn open_panel_at(
        &mut self,
        bundle: impl Bundle,
        placement: Placement,
    ) -> Result<Entity, String> {
        PanelsWorldExt::open_panel_at(&mut self.world, bundle, placement)
    }

    /// Show an error in a dialog, whatever panel it came from.
    pub fn request_error_dialog(&mut self, dialog: ErrorDialog) {
        ErrorDialogWorldExt::request_error_dialog(&mut self.world, dialog);
    }

    /// Open a link that leaves the app, as the app's `ExternalOpener` does.
    pub fn open_externally(&mut self, url: &str) -> Result<(), String> {
        bedtime_ui::LinksWorldExt::open_externally(&mut self.world, url)
    }

    /// Open a link clicked (or asked for) in `panel`, so an opener showing
    /// it in the app can put it somewhere else than on that panel.
    pub fn open_externally_from(&mut self, url: &str, panel: Entity) -> Result<(), String> {
        bedtime_ui::LinksWorldExt::open_externally_from(&mut self.world, url, panel)
    }
}

impl std::ops::Deref for Bedtime {
    type Target = BedtimeRef;
    fn deref(&self) -> &BedtimeRef {
        view(&self.world)
    }
}

fn view(world: &World) -> &BedtimeRef {
    // SAFETY: BedtimeRef is a repr(transparent) wrapper of World.
    unsafe { &*(world as *const World).cast::<BedtimeRef>() }
}

/// `world.bedtime()` and `world.bedtime_mut()`: the one name for bedtime's
/// operations on a `World`.
pub trait BedtimeWorldExt {
    fn bedtime(&self) -> &BedtimeRef;
    fn bedtime_mut(&mut self) -> &mut Bedtime;
}

impl BedtimeWorldExt for World {
    fn bedtime(&self) -> &BedtimeRef {
        view(self)
    }
    fn bedtime_mut(&mut self) -> &mut Bedtime {
        // SAFETY: Bedtime is a repr(transparent) wrapper of World.
        unsafe { &mut *(self as *mut World).cast::<Bedtime>() }
    }
}

#[cfg(test)]
mod tests;
