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
        r.menu(MenuSlot::File, OPEN_FILE_COMMAND);
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
        Ok(Box::new(CsvPanel::new(
            document.ok_or("CSV panels require a text document")?,
            state,
        )))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}
