//! Git workspace workflow. Repository workers outlive the panels that display them.
mod conflicts;
mod controller;
mod jobs;
mod panels;
mod status;

use bedtime_api::{
    features::{HostAction, MenuSlot},
    workbench::PanelPlacement,
};
use bedtime_documents::{DocumentId, DocumentSession, SessionEvent};
use controller::GitController;
use serde_json::{Value, json};

pub const MODULE_ID: &str = "bedtime.git";
pub const PANEL_ID: &str = "bedtime.git.panel";
pub const DIFF_PANEL_ID: &str = "bedtime.git.diff";
pub const SHOW_COMMAND: &str = "bedtime.git.show";
pub const REFRESH_COMMAND: &str = "bedtime.git.refresh";
pub const DIFF_COMMAND: &str = "bedtime.git.diff_file";

mod diff_feature;
mod feature;
pub use feature::{GitPanels, GitPlugin, GitState, GitTarget};
