//! A Bevy app's schedules, read from the running app: which systems run, in
//! what order and in which sets, where the `ApplyDeferred` sync points are,
//! and the components and resources each system reads and writes.
//!
//! [`ScheduleExtractPlugin`] keeps an [`AppSnapshot`] resource up to date;
//! [`snapshot_now`] takes one on demand. With the `panel` feature,
//! [`SchedulesPlugin`] shows it in a workbench panel; with `profile`, the
//! panel's timeline shows how long each system took.
//!
//! Adapted from `bevygraph` (see `licenses/bevygraph`).

pub mod extract;
pub mod flow;
pub mod frame;
pub mod layout;
#[cfg(feature = "panel")]
mod plugin;
#[cfg(feature = "profile")]
pub mod profile;
pub mod snapshot;
#[cfg(feature = "panel")]
pub mod view;

pub use extract::{ExtractConfig, ScheduleExtractPlugin, snapshot_now};
#[cfg(feature = "panel")]
pub use plugin::{OpenSchedules, SchedulesPanel, SchedulesPlugin};
pub use snapshot::AppSnapshot;
