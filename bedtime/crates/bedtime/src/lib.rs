//! Desktop apps with Bevy, in layers. Each crate is re-exported here behind a
//! feature of the same name:
//!
//! - [`ui`]: the UI toolkit (theme, fonts, icons, widgets), layouts of
//!   panels, and window chrome. Always included.
//! - Building blocks: [`persistence`], [`settings`], [`actions`], [`menus`],
//!   [`work`], [`document`], [`filesystem`].
//! - [`workbench`] (default): the framework for document-based apps, with
//!   `WorkbenchPlugins`. `workspaces` adds workspaces, which need the
//!   bevy_winit fork.
//! - Plugins: [`filetree`].
//!
//! See `docs/architecture.md` for how the layers fit together.
pub use bedtime_ui as ui;

#[cfg(feature = "actions")]
pub use bedtime_actions as actions;
#[cfg(feature = "documents")]
pub use bedtime_document as document;
#[cfg(feature = "filesystem")]
pub use bedtime_filesystem as filesystem;
#[cfg(feature = "filetree")]
pub use bedtime_filetree as filetree;
#[cfg(feature = "menus")]
pub use bedtime_menus as menus;
#[cfg(feature = "persistence")]
pub use bedtime_persistence as persistence;
#[cfg(feature = "settings")]
pub use bedtime_settings as settings;
#[cfg(feature = "work")]
pub use bedtime_work as work;
#[cfg(feature = "workbench")]
pub use bedtime_workbench as workbench;
