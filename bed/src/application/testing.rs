//! bEd regression fixtures and app-specific automation adapters.

pub use super::requests::*;
#[cfg(test)]
use super::runtime::layouts;
#[cfg(test)]
use super::runtime::{
    AppOptions, AppRuntimePlugin, ApplicationState, DocumentNavigation, LayoutSlot, LayoutSlots,
    WorkspacePersistence, persist_workspace, publish_app_state, save_current_workspace,
};
#[cfg(test)]
use super::runtime::{InitialAppOptions, install_test_infrastructure, test_application_paths};
#[cfg(test)]
use super::workspace::store::{WorkspaceSpec, WorkspaceStore};
#[cfg(test)]
use bedtime_api::{
    documents::{PanelBinding, PanelInput},
    features::{HostAction, HostEffects},
    workbench::{
        CommandInvocation, PanelClose, PanelCreation, PanelFocus, PanelTarget, WorkbenchAction,
    },
};
#[cfg(test)]
use bedtime_documents::{ByteEdit, DocumentId, DocumentSession};
#[cfg(test)]
use bedtime_plugin_workspaces::WorkspaceAction;
#[cfg(test)]
use bedtime_workbench::{PanelPresentation, PanelRecord, WorkbenchLayout, WorkbenchRequests};
#[cfg(test)]
use bevy::prelude::*;
#[cfg(test)]
use serde_json::{Value, json};
#[cfg(test)]
use std::{
    collections::HashSet,
    io,
    path::{Path, PathBuf},
    time::Instant,
};
#[cfg(test)]
#[path = "terminal_tests.rs"]
pub(in crate::application) mod terminal_tests;
#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "transaction_tests.rs"]
mod transaction_tests;
