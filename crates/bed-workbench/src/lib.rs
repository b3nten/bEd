//! Workbench shell, panel hosting, command routing and workspace services.
extern crate self as bed_workbench;
pub mod commands;
mod presentation;
pub mod shell;
pub mod workspace;
pub use shell::{WindowCommand, Workbench, WorkbenchHostMode, WorkbenchModules};
#[cfg(test)]
pub(crate) static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(test)]
#[path = "../../../src/builtins.rs"]
pub(crate) mod builtins;
#[cfg(test)]
#[allow(dead_code)]
#[path = "../../../tests/support/temp_dir.rs"]
pub(crate) mod test_support;
