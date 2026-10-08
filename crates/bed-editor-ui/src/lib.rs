//! Embeddable text and byte editor widgets, independent of workbench panels.
//! Hosts supply the UI frame, documents, presentation settings and extensions.
//! Attribution: workspace LICENSE and NOTICE.
pub mod editor_frame;
pub mod editor_input;
pub mod editor_view;
pub mod extensions;
pub mod hex_editor;
pub mod source_debug;
pub mod util;
pub mod views;

pub use editor_input::{DefinitionRequest, HostAction};
pub use editor_view::{EditorView, EditorViewOptions, ViewPresentation, ViewResponse};
pub use source_debug::{
    BreakpointStatus, SourceBreakpoint, SourceDebugAction, SourceDebugPresentation,
};

#[cfg(test)]
pub(crate) static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
#[allow(dead_code)]
#[path = "../../../tests/support/temp_dir.rs"]
pub(crate) mod test_support;

#[cfg(test)]
mod theme_tests;
