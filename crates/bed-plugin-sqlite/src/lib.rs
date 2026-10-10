//! Read-only SQLite browsing over local files, independent of editor documents.
mod backend;
mod panel;
mod worker;

use bed_editing::identity::DocumentId;
use bed_workbench_api::{
    CommandContext, HostContext, HostRequest, MenuSlot, Module, ModulePanel, ModuleServices,
    PanelInput, Registrar,
};
use serde_json::Value;
use std::any::Any;

pub use panel::SqlitePanel;
pub const PANEL_ID: &str = "bed.sqlite.panel";
pub const VIEWER_ID: &str = "bed.sqlite.viewer";
pub const OPEN_COMMAND: &str = "bed.sqlite.open";
pub const OPEN_FILE_COMMAND: &str = "bed.sqlite.open-file";

#[derive(Default)]
pub struct SqliteModule;

impl Module for SqliteModule {
    fn id(&self) -> &'static str {
        "bed.sqlite"
    }

    fn register(&self, registrar: &mut Registrar<'_>) {
        registrar.panel(PANEL_ID, "SQLite Viewer");
        registrar.local_file_viewer(
            VIEWER_ID,
            "SQLite Viewer",
            PANEL_ID,
            &["db", "sqlite", "sqlite3"],
        );
        registrar.command(OPEN_COMMAND, "Open SQLite Database…", None);
        registrar.command(OPEN_FILE_COMMAND, "Open in SQLite Viewer", None);
        registrar.menu(MenuSlot::Application, OPEN_COMMAND);
        registrar.menu(MenuSlot::File, OPEN_FILE_COMMAND);
    }

    fn command_enabled(
        &self,
        command: &str,
        context: &CommandContext,
        host: &HostContext<'_>,
    ) -> bool {
        !host.remote && (command != OPEN_FILE_COMMAND || context.path.is_some())
    }

    fn command_visible(&self, _: &str, _: &CommandContext, host: &HostContext<'_>) -> bool {
        !host.remote
    }

    fn command(
        &mut self,
        command: &str,
        context: &CommandContext,
        _: &HostContext<'_>,
        requests: &mut Vec<HostRequest>,
    ) {
        match command {
            OPEN_COMMAND => requests.push(HostRequest::OpenFileDialog {
                viewer: Some(VIEWER_ID.into()),
            }),
            OPEN_FILE_COMMAND => {
                if let Some(path) = &context.path {
                    requests.push(HostRequest::OpenFile {
                        path: path.clone(),
                        viewer: Some(VIEWER_ID.into()),
                    });
                }
            }
            _ => {}
        }
    }

    fn create_panel(
        &mut self,
        panel_type: &str,
        document: Option<DocumentId>,
        _: &Value,
    ) -> Result<Box<dyn ModulePanel>, String> {
        if panel_type != PANEL_ID {
            return Err(format!("Unknown SQLite panel: {panel_type}"));
        }
        if document.is_some() {
            return Err("SQLite Viewer requires a local file".into());
        }
        Ok(Box::new(EmptyPanel))
    }

    fn create_panel_with_services(
        &mut self,
        panel_type: &str,
        input: PanelInput,
        state: &Value,
        services: &mut ModuleServices<'_>,
    ) -> Result<Box<dyn ModulePanel>, String> {
        if panel_type != PANEL_ID {
            return Err(format!("Unknown SQLite panel: {panel_type}"));
        }
        if matches!(input, PanelInput::None) {
            return Ok(Box::new(EmptyPanel));
        }
        if services.documents.is_remote() {
            return Err("SQLite Viewer supports local databases only".into());
        }
        let PanelInput::LocalFile(path) = input else {
            return Err("SQLite Viewer requires a local file".into());
        };
        Ok(Box::new(SqlitePanel::new(path, state)))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_commands_are_unavailable_in_remote_workspaces() {
        let textures = Default::default();
        let mut host = HostContext {
            remote: true,
            documents: &[],
            active_document: None,
            settings: &Value::Null,
            textures: &textures,
            animations: false,
            workspace: 1,
            diagnostics: &Value::Null,
            viewer_menu: None,
            default_viewers: &Value::Null,
        };
        let context = CommandContext {
            path: Some("/remote/data.sqlite".into()),
            ..Default::default()
        };
        for command in [OPEN_COMMAND, OPEN_FILE_COMMAND] {
            assert!(!SqliteModule.command_visible(command, &context, &host));
            assert!(!SqliteModule.command_enabled(command, &context, &host));
        }
        host.remote = false;
        for command in [OPEN_COMMAND, OPEN_FILE_COMMAND] {
            assert!(SqliteModule.command_visible(command, &context, &host));
            assert!(SqliteModule.command_enabled(command, &context, &host));
        }
    }
}

/// An unloaded database viewer does not open a connection or start a worker.
struct EmptyPanel;

impl ModulePanel for EmptyPanel {
    fn title(&self, _: &HostContext<'_>) -> String {
        "SQLite Viewer".into()
    }

    fn draw(
        &mut self,
        ui: &dear_imgui_rs::Ui,
        host: &HostContext<'_>,
        requests: &mut Vec<HostRequest>,
    ) {
        ui.text_wrapped("Open a SQLite database to browse its tables.");
        if host.remote {
            ui.text_wrapped("SQLite Viewer supports local databases only.");
        } else if ui.button("Open Database…") {
            requests.push(HostRequest::OpenFileDialog {
                viewer: Some(VIEWER_ID.into()),
            });
        }
    }

    fn is_input_empty(&self, _: &HostContext<'_>) -> bool {
        true
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod default_panel_tests {
    use super::*;

    #[test]
    fn no_input_creates_an_unloaded_viewer() {
        let panel = SqliteModule
            .create_panel(PANEL_ID, None, &Value::Null)
            .unwrap();
        let textures = Default::default();
        let host = HostContext {
            remote: false,
            documents: &[],
            active_document: None,
            settings: &Value::Null,
            textures: &textures,
            animations: false,
            workspace: 1,
            diagnostics: &Value::Null,
            viewer_menu: None,
            default_viewers: &Value::Null,
        };
        assert_eq!(panel.title(&host), "SQLite Viewer");
        assert!(panel.is_input_empty(&host));
        assert!(panel.attached_document().is_none());
        assert_eq!(panel.save_state(), Value::Null);
    }
}
