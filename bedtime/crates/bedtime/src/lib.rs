//! Desktop apps with Bevy, in layers, re-exported here by feature:
//!
//! - [`ui`]: the UI toolkit (theme, fonts, icons, widgets), layouts of
//!   panels, and window chrome. Always included.
//! - [`workbench`] (default): desktop-app services (documents, actions and
//!   keys, menus, settings, background work) with `WorkbenchPlugins`. Its
//!   parts are also re-exported here: [`actions`], [`menus`], [`settings`],
//!   [`work`]. `workspaces` adds workspaces.
//! - Building blocks below the workbench: [`persistence`], [`document`],
//!   [`filesystem`].
//! - Plugins: [`filetree`], [`code`], [`terminal`].
pub use bedtime_ui as ui;

#[cfg(feature = "code")]
pub use bedtime_code as code;
#[cfg(feature = "documents")]
pub use bedtime_document as document;
#[cfg(feature = "filesystem")]
pub use bedtime_filesystem as filesystem;
#[cfg(feature = "filetree")]
pub use bedtime_filetree as filetree;
#[cfg(feature = "persistence")]
pub use bedtime_persistence as persistence;
#[cfg(feature = "terminal")]
pub use bedtime_terminal as terminal;
#[cfg(feature = "workbench")]
pub use bedtime_workbench as workbench;
#[cfg(feature = "workbench")]
pub use bedtime_workbench::{actions, menus, settings, work};
