//! Source outline plugin. It consumes immutable host snapshots and requests navigation.
pub mod presentation;
use bed_editing::identity::DocumentId;
use bed_highlight::outline::{OutlineKey, OutlineService};
use bed_plugin::{
    CommandContext, DocumentKind, HostContext, HostRequest, MenuSlot, Plugin, PluginPanel,
    Registrar,
};
use dear_imgui_rs::Ui;
use serde_json::Value;
use std::{
    any::Any,
    cell::{Ref, RefCell},
    rc::Rc,
};

pub const PLUGIN_ID: &str = "bed.structure";
pub const PANEL_ID: &str = "bed.structure.panel";
pub const OPEN_COMMAND: &str = "bed.structure.open";

#[derive(Default)]
struct Shared {
    outline: Option<OutlineService>,
    panels: usize,
}
#[derive(Default)]
pub struct StructurePlugin {
    shared: Rc<RefCell<Shared>>,
}
impl StructurePlugin {
    /// Read-only inspection is also useful to embedding hosts and regression tests.
    pub fn outline(&self) -> Option<Ref<'_, OutlineService>> {
        Ref::filter_map(self.shared.borrow(), |shared| shared.outline.as_ref()).ok()
    }
    /// Outline locations are valid only for the source currently being shown.
    pub fn navigation_request(
        jump: presentation::StructureJump,
        host: &HostContext<'_>,
    ) -> Option<HostRequest> {
        let active = host.active()?;
        if active.id != jump.key.document
            || active.revision != (jump.key.generation, jump.key.revision)
            || active.path != jump.key.path
            || active.language_id != jump.key.language_id
        {
            return None;
        }
        Some(HostRequest::Navigate {
            document: jump.key.document,
            byte_offset: jump.offset,
            revision: active.revision,
        })
    }
    fn sync(&self, host: &HostContext<'_>) {
        let mut shared = self.shared.borrow_mut();
        if shared.panels == 0 {
            shared.outline = None;
            return;
        }
        let outline = shared.outline.get_or_insert_with(OutlineService::default);
        let target = host
            .active()
            .filter(|document| document.kind == DocumentKind::Text);
        if let Some(document) = target {
            let key = OutlineKey {
                document: document.id,
                generation: document.revision.0,
                revision: document.revision.1,
                path: document.path.clone(),
                language_id: document.language_id.clone(),
            };
            if outline.requested() != Some(&key)
                && let Some(text) = &document.text
            {
                outline.request(key, text.clone());
            }
            outline.poll();
        } else {
            outline.clear();
        }
    }
}
impl Plugin for StructurePlugin {
    fn id(&self) -> &'static str {
        PLUGIN_ID
    }
    fn register(&self, registrar: &mut Registrar<'_>) {
        registrar.panel(PANEL_ID, "Structure");
        registrar.command(OPEN_COMMAND, "Structure", Some("structure"));
        registrar.toolbar(OPEN_COMMAND);
        registrar.menu(MenuSlot::Application, OPEN_COMMAND);
        registrar.menu(MenuSlot::TextSelection, OPEN_COMMAND);
    }
    fn tick(&mut self, host: &HostContext<'_>, _requests: &mut Vec<HostRequest>) {
        self.sync(host);
    }
    fn command(
        &mut self,
        command: &str,
        _context: &CommandContext,
        _host: &HostContext<'_>,
        requests: &mut Vec<HostRequest>,
    ) {
        if command == OPEN_COMMAND {
            requests.push(HostRequest::OpenPanel {
                panel_type: PANEL_ID.into(),
                document: None,
                state: Value::Null,
            });
        }
    }
    fn create_panel(
        &mut self,
        panel_type: &str,
        _document: Option<DocumentId>,
        _state: &Value,
    ) -> Result<Box<dyn PluginPanel>, String> {
        if panel_type != PANEL_ID {
            return Err(format!("Unknown structure panel: {panel_type}"));
        }
        self.shared.borrow_mut().panels += 1;
        Ok(Box::new(StructurePanel {
            shared: Rc::clone(&self.shared),
            presentation: presentation::StructurePanel::default(),
        }))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

pub struct StructurePanel {
    shared: Rc<RefCell<Shared>>,
    pub presentation: presentation::StructurePanel,
}
impl Drop for StructurePanel {
    fn drop(&mut self) {
        let mut shared = self.shared.borrow_mut();
        shared.panels = shared.panels.saturating_sub(1);
        if shared.panels == 0 {
            shared.outline = None;
        }
    }
}
impl PluginPanel for StructurePanel {
    fn title(&self, _host: &HostContext<'_>) -> String {
        "Structure".into()
    }
    fn draw(&mut self, ui: &Ui, host: &HostContext<'_>, requests: &mut Vec<HostRequest>) {
        // Focus can change earlier in the frame; sync to the host's fresh snapshot.
        StructurePlugin {
            shared: Rc::clone(&self.shared),
        }
        .sync(host);
        let shared = self.shared.borrow();
        if let Some(outline) = &shared.outline
            && let Some(jump) = self.presentation.draw(ui, outline, host.animations)
            && let Some(request) = StructurePlugin::navigation_request(jump, host)
        {
            requests.push(request);
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_editing::editor_state::EditorState;
    use bed_plugin::PluginDocument;
    use std::{
        collections::HashMap,
        sync::Arc,
        time::{Duration, Instant},
    };

    #[test]
    fn outline_uses_unsaved_snapshot_and_worker_stops_after_last_panel() {
        let mut plugin = StructurePlugin::default();
        let mut registry = bed_plugin::Registry::default();
        registry.register(&plugin).unwrap();
        assert!(plugin.outline().is_none());
        let panel = plugin.create_panel(PANEL_ID, None, &Value::Null).unwrap();
        let mut state = EditorState::default();
        state.set_from_bytes(b"fn unsaved() {}\n");
        let documents = [PluginDocument {
            id: DocumentId(7),
            path: "source.rs".into(),
            kind: DocumentKind::Text,
            language_id: "rust".into(),
            revision: (1, 2),
            dirty: true,
            bytes: Arc::from(&b"fn unsaved() {}\n"[..]),
            text: Some(state.snapshot()),
        }];
        let settings = Value::Null;
        let textures = HashMap::new();
        let host = HostContext {
            remote: false,
            default_viewers: &Value::Null,
            viewer_menu: None,
            documents: &documents,
            active_document: Some(DocumentId(7)),
            settings: &settings,
            textures: &textures,
            animations: false,
            workspace: 1,
            diagnostics: &Value::Null,
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            plugin.tick(&host, &mut Vec::new());
            if plugin.outline().is_some_and(|outline| !outline.updating()) {
                break;
            }
            assert!(Instant::now() < deadline);
            thread_sleep();
        }
        assert_eq!(
            plugin.outline().unwrap().result().unwrap().nodes[0].label,
            "unsaved"
        );
        drop(panel);
        assert!(plugin.outline().is_none());
    }
    fn thread_sleep() {
        std::thread::sleep(Duration::from_millis(5));
    }
}
