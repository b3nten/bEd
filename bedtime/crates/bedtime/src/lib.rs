//! Desktop apps with Bevy, in layers, re-exported here by feature:
//!
//! - [`ui`]: the UI toolkit (theme, fonts, icons, widgets), layouts of
//!   panels, and window chrome. Always included.
//! - [`workbench`] (default): an app's services (actions and keys, menus,
//!   settings, the status bar, background work, quitting), saved between
//!   runs. [`AppPlugins`] is all of it from the app's name. Its parts are
//!   also re-exported here: [`actions`], [`menus`], [`settings`], [`work`].
//! - `documents` adds documents to it (`workbench::DocumentsPlugins`), and
//!   `workspaces` folders with their own store and settings
//!   (`workbench::workspaces`). An app without them leaves them out.
//! - `native` (default): the platform's window chrome and menu bar.
//! - Building blocks below the workbench: [`persistence`], [`document`],
//!   [`filesystem`].
//! - Plugins: [`file_explorer`], [`code`], [`terminal`].
//! - [`automation`]: scripts driving the app, with the `automation` feature.
pub use bedtime_automation as automation;
pub use bedtime_ui as ui;

#[cfg(feature = "code")]
pub use bedtime_code as code;
#[cfg(feature = "documents")]
pub use bedtime_document as document;
#[cfg(feature = "file_explorer")]
pub use bedtime_file_explorer as file_explorer;
#[cfg(feature = "filesystem")]
pub use bedtime_filesystem as filesystem;
#[cfg(feature = "persistence")]
pub use bedtime_persistence as persistence;
#[cfg(feature = "terminal")]
pub use bedtime_terminal as terminal;
#[cfg(feature = "workbench")]
pub use bedtime_workbench as workbench;
#[cfg(feature = "workbench")]
pub use bedtime_workbench::{AppInfo, AppPlugins, BedtimeWorldExt};
#[cfg(feature = "workbench")]
pub use bedtime_workbench::{actions, menus, settings, work};
