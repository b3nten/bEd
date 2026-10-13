//! Debugger module: owns sessions and panels and contributes to the text editor.
//!
//! The GUI-free DAP implementation remains in `bed-debugger`. This crate integrates
//! it through workbench services and the editor's concrete extension contract.
mod controller;

use bed_api::{
    features::{HostAction, MenuSlot},
    workbench::PanelPlacement,
};
use bed_debugger::{CargoLaunch, SessionState};
use bed_documents::session::{DocumentId, DocumentSession};

use controller::{Debugger, DebuggerAction};
#[cfg(test)]
use dear_imgui_rs::Key;
use dear_imgui_rs::Ui;
use serde_json::Value;
use std::{io, path::Path};

pub const MODULE_ID: &str = "bed.debug";
pub const PANEL_ID: &str = "bed.debug.panel";
pub const SHOW_COMMAND: &str = "bed.debug.show";
pub const START_COMMAND: &str = "bed.debug.start";
pub const STOP_COMMAND: &str = "bed.debug.stop";
pub const RESTART_COMMAND: &str = "bed.debug.restart";
pub const CONTINUE_COMMAND: &str = "bed.debug.continue";
pub const PAUSE_COMMAND: &str = "bed.debug.pause";
pub const OVER_COMMAND: &str = "bed.debug.step_over";
pub const INTO_COMMAND: &str = "bed.debug.step_into";
pub const OUT_COMMAND: &str = "bed.debug.step_out";
pub const BREAKPOINT_COMMAND: &str = "bed.debug.toggle_breakpoint";

/// Declare the native debugger trampoline before the application's GUI exists.
pub fn launch_registration() -> bed_api::launch::LaunchRegistration {
    use bed_api::launch::{LaunchRegistration, ProcessEntrypoint, ProcessSelector};
    LaunchRegistration {
        owner: MODULE_ID,
        entrypoints: vec![ProcessEntrypoint {
            selector: ProcessSelector::FirstArgument(bed_debugger::launcher::FLAG.into()),
            run: |args| bed_debugger::launcher::run(args.into_iter().skip(2)).map(|_| Some(0)),
        }],
        install: Box::new(|app| {
            app.add_plugins(DebuggerPlugin);
        }),
    }
}

pub(crate) fn show_panel() -> HostAction {
    HostAction::ShowPanel {
        panel_type: PANEL_ID.into(),
        document: None,
        state: Value::Null,
        action: None,
    }
}

/// Native state injection for host regression tests; production integrations use
/// commands, scoped services and the editor extension contract.
#[cfg(feature = "test-support")]
pub mod fixtures {
    pub use crate::controller::source::{Breakpoint, SourceDebugState};
    pub use crate::controller::{
        Debugger, DebuggerAction, program_terminal_title, same_debug_source,
    };
}

mod feature;
pub use feature::{DebugState, DebugTarget, DebuggerPlugin, shortcuts};

#[cfg(test)]
static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod test_support {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicUsize, Ordering},
    };

    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "bed-module-debug-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }

        pub fn root(&self) -> &Path {
            &self.0
        }

        pub fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, bytes).unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
