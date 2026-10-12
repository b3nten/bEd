//! TerminalPlugin attaches panel entities to the world's shared BedTerminal.

mod feature;
#[cfg(unix)]
mod shell;
pub use feature::{
    TerminalAppearance, TerminalFrontend, TerminalPanelState, TerminalPlugin, TerminalRetentions,
    TerminalWorkspace, close_panel, focus_panel,
};
use std::path::{Path, PathBuf};

pub const MODULE_ID: &str = "bed.terminal";
pub const PANEL_ID: &str = "bed.terminal.panel";
pub const NEW_COMMAND: &str = "bed.terminal.new";

pub fn shell_directory(saved: Option<&str>, base_directory: &str, remote: bool) -> Option<PathBuf> {
    saved
        .filter(|path| remote || Path::new(path).is_dir())
        .map(PathBuf::from)
        .or_else(|| (!base_directory.is_empty()).then(|| PathBuf::from(base_directory)))
}

#[cfg(feature = "desktop")]
mod input;
#[cfg(feature = "desktop")]
pub use input::{TerminalInputPolicy, TerminalInputSystems, clipboard_file_paste};

#[cfg(test)]
#[path = "../../../tests/support/temp_dir.rs"]
mod test_support;

#[cfg(test)]
pub(crate) static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
