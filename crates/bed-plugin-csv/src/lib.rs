//! CSV/TSV tables backed by the host's shared, revision-checked text document.
mod model;
mod panel;
mod worker;

use bed_editing::identity::DocumentId;
use bed_plugin::{
    CommandContext, DocumentKind, HostContext, HostRequest, MenuSlot, Plugin, PluginPanel,
    Registrar,
};
use serde_json::Value;
use std::any::Any;

pub use panel::CsvPanel;
pub const PANEL_ID: &str = "bed.csv.panel";
pub const VIEWER_ID: &str = "bed.csv.viewer";
pub const OPEN_COMMAND: &str = "bed.csv.open";
pub const OPEN_FILE_COMMAND: &str = "bed.csv.open-file";

#[derive(Default)]
pub struct CsvPlugin;
impl Plugin for CsvPlugin {
    fn id(&self) -> &'static str {
        "bed.csv"
    }
    fn register(&self, r: &mut Registrar<'_>) {
        r.panel(PANEL_ID, "CSV Table");
        r.viewer(
            VIEWER_ID,
            "CSV Table",
            PANEL_ID,
            &["csv", "tsv"],
            DocumentKind::Text,
        );
        r.command(OPEN_COMMAND, "Open CSV/TSV…", None);
        r.command(OPEN_FILE_COMMAND, "Open in CSV Table", None);
        r.menu(MenuSlot::Application, OPEN_COMMAND);
    }
    fn command_enabled(
        &self,
        command: &str,
        context: &CommandContext,
        _: &HostContext<'_>,
    ) -> bool {
        command != OPEN_FILE_COMMAND
            || context.path.as_deref().is_some_and(|path| {
                path.rsplit(['/', '\\'])
                    .next()
                    .and_then(|name| name.rsplit_once('.'))
                    .is_some_and(|(_, ext)| {
                        ["csv", "tsv"].iter().any(|v| v.eq_ignore_ascii_case(ext))
                    })
            })
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
        kind: &str,
        document: Option<DocumentId>,
        state: &Value,
    ) -> Result<Box<dyn PluginPanel>, String> {
        if kind != PANEL_ID {
            return Err(format!("Unknown CSV panel: {kind}"));
        }
        let Some(document) = document else {
            return Ok(Box::new(EmptyPanel));
        };
        Ok(Box::new(CsvPanel::new(document, state)))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// An unloaded viewer owns no document, worker, or resource session.
struct EmptyPanel;

impl PluginPanel for EmptyPanel {
    fn title(&self, _: &HostContext<'_>) -> String {
        "CSV Table".into()
    }

    fn draw(
        &mut self,
        ui: &dear_imgui_rs::Ui,
        _: &HostContext<'_>,
        requests: &mut Vec<HostRequest>,
    ) {
        ui.text_wrapped("Open a CSV or TSV file to view its table.");
        if ui.button("Open CSV/TSV…") {
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
        let mut plugin = CsvPlugin::default();
        let panel = plugin.create_panel(PANEL_ID, None, &Value::Null).unwrap();
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
        assert_eq!(panel.title(&host), "CSV Table");
        assert!(panel.is_input_empty(&host));
        assert!(panel.attached_document().is_none());
        assert_eq!(panel.save_state(), Value::Null);
    }
}
