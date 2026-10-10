//! Native Markdown previews of the host's shared text documents.
mod assets;
mod gpu;
mod layout;
mod model;
mod panel;
mod worker;

use bed_editing::identity::DocumentId;
use bed_plugin::{
    CommandContext, DocumentKind, HostContext, HostRequest, MenuSlot, Plugin, PluginPanel,
    Registrar,
};
pub use panel::MarkdownPanel;
use serde_json::Value;
use std::any::Any;

pub const PLUGIN_ID: &str = "bed.markdown";
pub const PANEL_ID: &str = "bed.markdown.panel";
pub const VIEWER_ID: &str = "bed.markdown.viewer";
pub const OPEN_COMMAND: &str = "bed.markdown.open";
pub const OPEN_FILE_COMMAND: &str = "bed.markdown.open-file";
pub const SUPPORTED_EXTENSIONS: &[&str] = &["md", "markdown", "mdown", "mkd"];

#[derive(Default)]
pub struct MarkdownPlugin;
impl Plugin for MarkdownPlugin {
    fn id(&self) -> &'static str {
        PLUGIN_ID
    }
    fn register(&self, r: &mut Registrar<'_>) {
        r.panel(PANEL_ID, "Markdown Preview");
        r.viewer(
            VIEWER_ID,
            "Markdown Preview",
            PANEL_ID,
            SUPPORTED_EXTENSIONS,
            DocumentKind::Text,
        );
        r.command(OPEN_COMMAND, "Open Markdown…", None);
        r.command(OPEN_FILE_COMMAND, "Open in Markdown Preview", None);
        r.menu(MenuSlot::Application, OPEN_COMMAND);
    }
    fn command_enabled(
        &self,
        command: &str,
        context: &CommandContext,
        _: &HostContext<'_>,
    ) -> bool {
        command != OPEN_FILE_COMMAND || context.path.as_deref().is_some_and(supported_path)
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
            return Err(format!("Unknown Markdown panel: {kind}"));
        }
        let Some(document) = document else {
            return Ok(Box::new(EmptyPanel));
        };
        Ok(Box::new(MarkdownPanel::new(document, state)?))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

fn supported_path(path: &str) -> bool {
    path.rsplit(['/', '\\'])
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .is_some_and(|(_, ext)| {
            SUPPORTED_EXTENSIONS
                .iter()
                .any(|value| value.eq_ignore_ascii_case(ext))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn supported_extensions_are_case_insensitive() {
        for path in ["README.MD", "a.markdown", "a.mdown", "a.MkD"] {
            assert!(supported_path(path));
        }
        for path in ["README", "a.json", "file.md/image.png"] {
            assert!(!supported_path(path));
        }
    }
}

/// An unloaded viewer owns no document, worker, or resource session.
struct EmptyPanel;

impl PluginPanel for EmptyPanel {
    fn title(&self, _: &HostContext<'_>) -> String {
        "Markdown Preview".into()
    }

    fn draw(
        &mut self,
        ui: &dear_imgui_rs::Ui,
        _: &HostContext<'_>,
        requests: &mut Vec<HostRequest>,
    ) {
        ui.text_wrapped("Open a Markdown file to preview it.");
        if ui.button("Open Markdown…") {
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
        let mut plugin = MarkdownPlugin::default();
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
        assert_eq!(panel.title(&host), "Markdown Preview");
        assert!(panel.is_input_empty(&host));
        assert!(panel.attached_document().is_none());
        assert_eq!(panel.save_state(), Value::Null);
        assert!(panel.render_output().is_none());
    }
}
