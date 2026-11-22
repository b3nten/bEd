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
//! - Plugins, a feature each: [`file_explorer`], [`code`], [`terminal`],
//!   [`audio`], [`cat`], [`datatree`], [`diagram`], [`font`], [`frame_rate`],
//!   [`git`], [`hex`], [`http_client`], [`image`], [`markdown`], [`mascots`],
//!   [`model`], [`notes`], [`schedules`], [`search`], [`sqlite`], [`table`],
//!   [`video`], [`web`].
//! - [`automation`]: scripts driving the app, with the `automation` feature.
pub use bedtime_automation as automation;
pub use bedtime_ui as ui;

#[cfg(feature = "audio")]
pub use bedtime_audio as audio;
#[cfg(feature = "cat")]
pub use bedtime_cat as cat;
#[cfg(feature = "code")]
pub use bedtime_code as code;
#[cfg(feature = "datatree")]
pub use bedtime_datatree as datatree;
#[cfg(feature = "diagram")]
pub use bedtime_diagram as diagram;
#[cfg(feature = "documents")]
pub use bedtime_document as document;
#[cfg(feature = "file_explorer")]
pub use bedtime_file_explorer as file_explorer;
#[cfg(feature = "filesystem")]
pub use bedtime_filesystem as filesystem;
#[cfg(feature = "font")]
pub use bedtime_font as font;
#[cfg(feature = "frame_rate")]
pub use bedtime_frame_rate as frame_rate;
#[cfg(feature = "git")]
pub use bedtime_git as git;
#[cfg(feature = "hex")]
pub use bedtime_hex as hex;
#[cfg(feature = "http_client")]
pub use bedtime_http_client as http_client;
#[cfg(feature = "image")]
pub use bedtime_image as image;
#[cfg(feature = "markdown")]
pub use bedtime_markdown as markdown;
#[cfg(feature = "mascots")]
pub use bedtime_mascots as mascots;
#[cfg(feature = "model")]
pub use bedtime_model as model;
#[cfg(feature = "notes")]
pub use bedtime_notes as notes;
#[cfg(feature = "persistence")]
pub use bedtime_persistence as persistence;
#[cfg(feature = "schedules")]
pub use bedtime_schedules as schedules;
#[cfg(feature = "search")]
pub use bedtime_search as search;
#[cfg(feature = "sqlite")]
pub use bedtime_sqlite as sqlite;
#[cfg(feature = "table")]
pub use bedtime_table as table;
#[cfg(feature = "terminal")]
pub use bedtime_terminal as terminal;
#[cfg(feature = "video")]
pub use bedtime_video as video;
#[cfg(feature = "web")]
pub use bedtime_web as web;
#[cfg(feature = "workbench")]
pub use bedtime_workbench as workbench;
#[cfg(feature = "workbench")]
pub use bedtime_workbench::{AppInfo, AppPlugins, BedtimeWorldExt};
#[cfg(feature = "workbench")]
pub use bedtime_workbench::{actions, menus, settings, work};
