//! Native smoke fixtures for the registered debugger module.
//! Debug state, panels and editor contributions live in bed-module-debug.
use super::Workbench;
use std::io;

impl Workbench {
    pub fn debug_smoke_setup(&mut self) -> io::Result<()> {
        self.with_module_services(|modules, services| {
            let module = modules
                .instances
                .iter_mut()
                .find_map(|module| {
                    module
                        .as_any_mut()
                        .downcast_mut::<bed_module_debug::DebugModule>()
                })
                .ok_or_else(|| io::Error::other("Debugger module is unavailable"))?;
            module.smoke_setup(services, &mut modules.requests)
        })?;
        self.process_plugin_requests()
    }
    pub fn debug_smoke_ready(&self) -> io::Result<bool> {
        self.modules
            .instances
            .iter()
            .find_map(|module| {
                module
                    .as_any()
                    .downcast_ref::<bed_module_debug::DebugModule>()
            })
            .ok_or_else(|| io::Error::other("Debugger module is unavailable"))?
            .smoke_ready()
    }
}

#[cfg(test)]
use bed_debug::{CargoLaunch, DebugProfile, DebugSession, LaunchConfig, SessionState};
#[cfg(test)]
use bed_editing::editor_events::Overlay;
#[cfg(test)]
use bed_module_debug::fixtures::*;
#[cfg(test)]
use bed_terminal::terminal_pty::{PtyOptions, TerminalShell};
#[cfg(test)]
use dear_imgui_rs::{Key, Ui, sys};
#[cfg(test)]
use serde_json::json;
#[cfg(test)]
use std::path::{Path, PathBuf};

#[cfg(test)]
impl Workbench {
    fn debugger(&self) -> std::cell::RefMut<'_, Debugger> {
        self.modules
            .instances
            .iter()
            .find_map(|module| {
                module
                    .as_any()
                    .downcast_ref::<bed_module_debug::DebugModule>()
            })
            .unwrap()
            .borrow_state()
    }
    fn debug_action(&mut self, action: Action) -> io::Result<()> {
        self.with_module_services(|modules, services| {
            let state = modules
                .instances
                .iter()
                .find_map(|module| {
                    module
                        .as_any()
                        .downcast_ref::<bed_module_debug::DebugModule>()
                })
                .unwrap()
                .state();
            state
                .borrow_mut()
                .debug_action(action, services, &mut modules.requests)
        })?;
        self.process_plugin_requests()
    }
    fn show_debugger(&mut self) {
        self.dispatch_command(bed_module_debug::SHOW_COMMAND)
            .unwrap();
    }
    fn tick_debugger(&mut self) -> io::Result<()> {
        self.tick_plugins()
    }
    fn stop_debugger(&mut self) {
        let _ = self.with_module_services(|modules, services| {
            let state = modules
                .instances
                .iter()
                .find_map(|module| {
                    module
                        .as_any()
                        .downcast_ref::<bed_module_debug::DebugModule>()
                })
                .unwrap()
                .state();
            state
                .borrow_mut()
                .stop_debugger(services, &mut modules.requests);
            Ok(())
        });
    }
    fn toggle_debug_breakpoint(
        &mut self,
        document: bed_document_session::DocumentId,
        row: i32,
    ) -> io::Result<()> {
        self.debugger()
            .toggle_debug_breakpoint(&self.session, document, row)
    }
    fn debug_source_presentation(
        &self,
        document: bed_document_session::DocumentId,
    ) -> io::Result<Option<bed_editor_ui::SourceDebugPresentation>> {
        self.debugger()
            .debug_source_presentation(&self.session, document)
    }
    fn persist_debugger_settings(&mut self) -> io::Result<()> {
        self.persist_module_settings()
    }
    fn debug_shortcuts(&mut self, ui: &Ui) -> io::Result<()> {
        self.module_shortcuts(ui)
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/workbench/debugger_tests.rs"]
mod workbench_tests;
