//! Git workspace workflow. Repository workers outlive the panels that display them.
mod conflicts;
mod controller;
mod jobs;
mod panels;
mod status;

use bed_api::{
    features::{HostAction, MenuSlot},
    workbench::PanelPlacement,
};
use bed_documents::{DocumentId, DocumentSession, SessionEvent};
use controller::GitController;
use serde_json::{Value, json};

pub const MODULE_ID: &str = "bed.git";
pub const PANEL_ID: &str = "bed.git.panel";
pub const DIFF_PANEL_ID: &str = "bed.git.diff";
pub const SHOW_COMMAND: &str = "bed.git.show";
pub const REFRESH_COMMAND: &str = "bed.git.refresh";
pub const DIFF_COMMAND: &str = "bed.git.diff_file";

mod diff_feature;
mod feature;
pub use feature::{GitPanels, GitPlugin, GitState, GitTarget};
