//! Bed ui components. Attribution: workspace LICENSE, NOTICE and UPSTREAM_REVISION.
pub mod editor_frame;
pub mod editor_input;
pub mod editor_view;
pub mod lsp;
pub mod presentation;
pub mod util;
pub mod views;

#[cfg(test)]
pub(crate) static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
#[allow(dead_code)]
#[path = "../../../tests/support/temp_dir.rs"]
pub(crate) mod test_support;

pub use editor_input::{DefinitionRequest, HostAction};
pub use editor_view::{EditorView, EditorViewOptions, ViewPresentation, ViewResponse};

#[cfg(test)]
mod theme_tests;
