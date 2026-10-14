//! bEd startup, numbered layouts, application preferences and status policy.
mod command_registration;
pub mod runtime;
mod status_bar;
pub(crate) use status_bar::publish_identity as publish_status_identity;
#[cfg(any(test, feature = "automation"))]
pub mod testing;
pub mod workspace;
pub use bedtime_api::workbench::{PanelGeometry, PanelPresentation};
pub use bedtime_workbench::{PanelRecord, WorkbenchLayout};
pub use runtime::*;
pub use workspace::store::{WorkspaceSpec, WorkspaceStore, WorkspaceTarget};

#[cfg(test)]
pub(crate) use crate::test_support;
