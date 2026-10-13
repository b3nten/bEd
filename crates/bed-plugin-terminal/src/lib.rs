//! TerminalPlugin attaches panel entities to the world's shared BedTerminal.

mod feature;
#[cfg(unix)]
mod shell;
#[cfg(unix)]
pub use shell::socket_path as shell_socket_path;
/// Dispatch terminal shell helpers before creating the GUI. Applications installing
/// TerminalPlugin call this from their executable when exposing registered shell commands.
mod helper;
pub use feature::{
    TerminalAppIntegrationPlugin, TerminalAppearance, TerminalFrontend, TerminalLaunchContext,
    TerminalPanelState, TerminalPlugin, TerminalRetentions, close_panel, focus_panel,
};
pub use helper::run_shell_command as run_helper;
use std::path::{Path, PathBuf};

pub const MODULE_ID: &str = "bed.terminal";
pub const PANEL_ID: &str = "bed.terminal.panel";
pub const NEW_COMMAND: &str = "bed.terminal.new";

/// Install shell panels and register their executable helper before GUI startup.
pub fn launch_registration() -> bed_api::launch::LaunchRegistration {
    use bed_api::launch::{LaunchRegistration, ProcessEntrypoint, ProcessSelector};
    use bevy::prelude::App;
    LaunchRegistration {
        owner: MODULE_ID,
        entrypoints: vec![ProcessEntrypoint {
            selector: ProcessSelector::ExecutablePrefix("+".into()),
            run: |args| run_helper(args).transpose().map(|result| result.map(|_| 0)),
        }],
        install: Box::new(|app: &mut App| {
            app.add_plugins((TerminalPlugin, TerminalAppIntegrationPlugin));
        }),
    }
}

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
