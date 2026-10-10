use crate::{
    assets::{self, AssetManager},
    layout::{self, Layout},
    model::Document,
    worker::{Job, Worker},
};
use bed_editing::identity::DocumentId;
use bed_plugin::{
    HostContext, HostRequest, ModuleServices, PanelAction, PluginPanel, Revision,
    gpu::{GpuContext, RenderOutput, RenderTarget},
};
use bed_remote::RemoteClient;
use dear_imgui_rs::{Ui, WindowFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{any::Any, ffi::CString, sync::Arc};

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct ViewState {
    scroll: [f32; 2],
}

pub struct MarkdownPanel {
    document: DocumentId,
    state: ViewState,
    worker: Worker,
    assets: AssetManager,
    revision: Option<Revision>,
    serial: u64,
    submitted: bool,
    parsed: Option<Document>,
    error: Option<String>,
    message: Option<String>,
    layout: Option<Layout>,
    layout_key: Option<(f32, f32, u64, dear_imgui_rs::FontId)>,
    restore_scroll: bool,
    pending_anchor: Option<String>,
    project_root: String,
    remote: Option<RemoteClient>,
}

impl MarkdownPanel {
    pub fn new(document: DocumentId, state: &Value) -> Result<Self, String> {
        let mut state: ViewState = serde_json::from_value(state.clone()).unwrap_or_default();
        for value in &mut state.scroll {
            if !value.is_finite() || *value < 0.0 {
                *value = 0.0;
            }
        }
        let worker = Worker::new()?;
        let serial = worker.invalidate();
        Ok(Self {
            document,
            state,
            worker,
            assets: AssetManager::new()?,
            revision: None,
            serial,
            submitted: false,
            parsed: None,
            error: None,
            message: None,
            layout: None,
            layout_key: None,
            restore_scroll: true,
            pending_anchor: None,
            project_root: String::new(),
            remote: None,
        })
    }
    pub fn is_ready(&self) -> bool {
        self.parsed.is_some() && self.error.is_none()
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    fn sync(&mut self, host: &HostContext<'_>) {
        let Some(document) = host.document(self.document) else {
            self.error = Some("This document is no longer open.".into());
            self.parsed = None;
            self.layout = None;
            return;
        };
        if self.revision != Some(document.revision) {
            self.revision = Some(document.revision);
            self.serial = self.worker.invalidate();
            self.submitted = false;
            self.parsed = None;
            self.layout = None;
            self.layout_key = None;
            self.error = None;
            self.message = None;
            self.restore_scroll = true;
        }
        if let Some(output) = self.worker.poll()
            && output.serial == self.serial
            && Some(output.revision) == self.revision
        {
            match output.result {
                Ok(parsed) => self.parsed = Some(parsed),
                Err(error) => self.error = Some(error),
            }
        }
        if !self.submitted {
            self.submitted = self.worker.submit(Job {
                serial: self.serial,
                revision: document.revision,
                bytes: Arc::clone(&document.bytes),
            });
        }
        // Clear stale assets immediately, including while the new model is loading.
        let images = self
            .parsed
            .as_ref()
            .map_or(&[][..], |parsed| parsed.images.as_slice());
        self.assets.sync(
            document.revision,
            Some(&document.path),
            &self.project_root,
            self.remote.clone(),
            images,
        );
        self.assets.poll();
    }
    fn edit_as_text(&self, host: &HostContext<'_>, requests: &mut Vec<HostRequest>) {
        if let Some(menu) = host.viewer_menu
            && let Some(choice) = menu.choices.iter().find(|choice| choice.text_editor)
        {
            requests.push(HostRequest::SwitchViewer {
                tab: menu.tab,
                document: self.document,
                viewer: choice.id.clone(),
            });
        } else if let Some(document) = host.document(self.document) {
            requests.push(HostRequest::OpenFile {
                path: document.path.clone(),
                viewer: Some("bed.text".into()),
            });
        }
    }
    fn activate_link(
        &mut self,
        target: &str,
        host: &HostContext<'_>,
        requests: &mut Vec<HostRequest>,
    ) {
        self.message = None;
        if let Some(fragment) = target.strip_prefix('#') {
            match assets::percent_decode(fragment) {
                Ok(anchor) => self.pending_anchor = Some(anchor),
                Err(error) => self.message = Some(error),
            }
        } else if target.to_ascii_lowercase().starts_with("https://")
            || target.to_ascii_lowercase().starts_with("http://")
        {
            let Ok(target) = CString::new(target) else {
                self.message = Some("The link contains an invalid NUL character.".into());
                return;
            };
            // Use Dear ImGui's platform integration, the same callback as TextLinkOpenURL.
            let opened = unsafe {
                let platform = dear_imgui_rs::sys::igGetPlatformIO_Nil();
                (*platform).Platform_OpenInShellFn.is_some_and(|open| {
                    open(dear_imgui_rs::sys::igGetCurrentContext(), target.as_ptr())
                })
            };
            if !opened {
                self.message = Some("The system browser could not open this link.".into());
            }
        } else if let Some(document) = host.document(self.document) {
            let path = if document.path.is_empty() {
                format!("{}/untitled.md", self.project_root)
            } else {
                document.path.clone()
            };
            match assets::resolve_link(&path, target) {
                Ok(path) => requests.push(HostRequest::OpenFile { path, viewer: None }),
                Err(error) => self.message = Some(error),
            }
        }
    }
    fn content(&mut self, ui: &Ui, host: &HostContext<'_>, requests: &mut Vec<HostRequest>) {
        let size = ui.content_region_avail();
        ui.child_window("markdown-document")
            .size(size)
            .flags(WindowFlags::HORIZONTAL_SCROLLBAR)
            .build(ui, || {
                if let Some(error) = &self.error {
                    ui.text_wrapped(error);
                    if ui.button("Edit as Text") {
                        self.edit_as_text(host, requests);
                    }
                    return;
                }
                let Some(parsed) = &self.parsed else {
                    ui.text_disabled("Rendering Markdown…");
                    return;
                };
                let width = ui.content_region_avail()[0].max(1.0);
                let key = (
                    width,
                    ui.current_font_size(),
                    self.assets.version(),
                    ui.current_font(),
                );
                if self.layout_key != Some(key) {
                    match layout::build(ui, parsed, width, &self.assets) {
                        Ok(layout) => {
                            self.layout = Some(layout);
                            self.layout_key = Some(key);
                        }
                        Err(error) => {
                            self.error = Some(error);
                            self.layout = None;
                        }
                    }
                }
                let Some(layout) = &self.layout else {
                    return;
                };
                let origin = ui.cursor_screen_pos();
                // The sole layout item supplies scrolling extent; draw-list primitives
                // never advance ImGui's cursor or pollute tab navigation.
                ui.dummy(layout.size);
                if self.restore_scroll {
                    ui.set_scroll_x(self.state.scroll[0]);
                    ui.set_scroll_y(self.state.scroll[1]);
                    self.restore_scroll = false;
                }
                if let Some(anchor) = self.pending_anchor.take() {
                    if anchor.is_empty() {
                        ui.set_scroll_y(0.0);
                    } else if let Some(y) = layout.anchors.get(&anchor) {
                        ui.set_scroll_y(*y);
                    } else {
                        self.message = Some(format!("Heading #{anchor} was not found."));
                    }
                }
                let target = layout.draw(ui, origin, &self.assets, host);
                self.state.scroll = [ui.scroll_x(), ui.scroll_y()];
                if let Some(target) = target {
                    self.activate_link(&target, host, requests);
                }
                if let Some(_popup) = ui.begin_popup_context_window() {
                    host.draw_viewer_menu(ui, requests);
                }
            });
    }
}

impl PluginPanel for MarkdownPanel {
    fn title(&self, host: &HostContext<'_>) -> String {
        host.document(self.document)
            .and_then(|document| document.path.rsplit(['/', '\\']).next())
            .filter(|name| !name.is_empty())
            .unwrap_or("Markdown Preview")
            .to_owned()
    }
    fn attached_document(&self) -> Option<DocumentId> {
        Some(self.document)
    }
    fn save_state(&self) -> Value {
        serde_json::to_value(&self.state).unwrap_or(Value::Null)
    }
    fn close(&mut self, _: &mut Vec<HostRequest>) {
        self.worker.close();
        self.assets.close();
        self.parsed = None;
        self.layout = None;
    }
    fn render_output(&self) -> Option<RenderOutput> {
        self.assets.render_output()
    }
    fn render(&mut self, gpu: &mut GpuContext<'_>, target: &RenderTarget) -> Result<(), String> {
        self.assets.render(gpu, target)
    }
    fn draw_with_services(
        &mut self,
        ui: &Ui,
        host: &HostContext<'_>,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> std::io::Result<()> {
        self.project_root = services.project_root.to_owned();
        self.remote = services.documents.remote_client();
        self.draw(ui, host, requests);
        Ok(())
    }
    fn action(
        &mut self,
        action: PanelAction,
        _: &HostContext<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> Result<bool, String> {
        match action {
            PanelAction::Undo => requests.push(HostRequest::Undo {
                document: self.document,
            }),
            PanelAction::Redo => requests.push(HostRequest::Redo {
                document: self.document,
            }),
            _ => return Ok(false),
        }
        Ok(true)
    }
    fn draw(&mut self, ui: &Ui, host: &HostContext<'_>, requests: &mut Vec<HostRequest>) {
        self.sync(host);
        if self
            .parsed
            .as_ref()
            .is_some_and(|parsed| !parsed.images.is_empty())
        {
            if ui.small_button("Reload images") {
                self.assets.refresh();
            }
            ui.separator();
        }
        if let Some(message) = &self.message {
            ui.text_wrapped(message);
        }
        self.content(ui, host, requests);
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
    use bed_plugin::{DocumentKind, PluginDocument, ViewerChoice, ViewerMenu};
    use std::collections::HashMap;
    #[test]
    fn relative_links_anchors_and_edit_as_text_use_host_contracts() {
        let id = DocumentId::next();
        let mut panel = MarkdownPanel::new(id, &Value::Null).unwrap();
        let documents = [PluginDocument {
            id,
            path: "/workspace/docs/readme.md".into(),
            kind: DocumentKind::Text,
            language_id: "markdown".into(),
            revision: (3, 2),
            dirty: true,
            bytes: Arc::from(&b"# Heading"[..]),
            text: None,
        }];
        let menu = ViewerMenu {
            tab: 12,
            document: id,
            current: crate::VIEWER_ID.into(),
            choices: vec![ViewerChoice {
                id: "bed.text".into(),
                label: "Text Editor".into(),
                text_editor: true,
            }],
        };
        let host = HostContext {
            remote: false,
            documents: &documents,
            active_document: Some(id),
            settings: &Value::Null,
            textures: &HashMap::new(),
            animations: false,
            workspace: 1,
            diagnostics: &Value::Null,
            viewer_menu: Some(&menu),
            default_viewers: &Value::Null,
        };
        let mut requests = Vec::new();
        panel.activate_link("../a%20b.md#heading", &host, &mut requests);
        assert!(
            matches!(&requests[0],HostRequest::OpenFile{path,viewer:None} if path=="/workspace/a b.md")
        );
        panel.activate_link("#hello%20world", &host, &mut requests);
        assert_eq!(panel.pending_anchor.as_deref(), Some("hello world"));
        panel.activate_link("javascript:bad()", &host, &mut requests);
        assert!(panel.message.is_some());
        assert_eq!(requests.len(), 1);
        panel.edit_as_text(&host, &mut requests);
        assert!(
            matches!(&requests[1],HostRequest::SwitchViewer{tab:12,document,viewer} if *document==id && viewer=="bed.text")
        );
    }
}
