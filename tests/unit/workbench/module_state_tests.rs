//! State capture and restoration through the common module host.
use super::*;
use crate::test_support::TempDir;
use bed_document_session::ByteEdit;
use bed_workbench_api::{Module, ModulePanel, Registrar};
use std::any::Any;

fn host(directory: &TempDir) -> Workbench {
    let mut settings = Settings::with_paths(
        directory.path("config"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
    )
    .unwrap();
    for setting in [
        "terminal_visible",
        "treesitter",
        "git_changed_lines",
        "autosave",
    ] {
        settings.settings[setting] = json!(false);
    }
    settings.terminal_visible = false;
    Workbench::with_settings(settings, crate::builtins::modules)
}

struct ScopedStateModule;
struct ScopedStatePanel {
    document: DocumentId,
}
impl Module for ScopedStateModule {
    fn id(&self) -> &'static str {
        "test.scoped-state"
    }
    fn register(&self, registrar: &mut Registrar<'_>) {
        registrar.panel("test.scoped-state.panel", "Live state");
    }
    fn command(
        &mut self,
        _: &str,
        _: &CommandContext,
        _: &HostContext<'_>,
        _: &mut Vec<HostRequest>,
    ) {
    }
    fn create_panel(
        &mut self,
        _: &str,
        document: Option<DocumentId>,
        _: &Value,
    ) -> Result<Box<dyn ModulePanel>, String> {
        Ok(Box::new(ScopedStatePanel {
            document: document.ok_or("Document required")?,
        }))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
impl ModulePanel for ScopedStatePanel {
    fn title(&self, _: &HostContext<'_>) -> String {
        "Live state".into()
    }
    fn draw(&mut self, _: &Ui, _: &HostContext<'_>, _: &mut Vec<HostRequest>) {
        panic!("State capture must not require drawing the hidden panel");
    }
    fn attached_document(&self) -> Option<DocumentId> {
        Some(self.document)
    }
    fn save_state_with_services(&mut self, services: &mut ModuleServices<'_>) -> io::Result<Value> {
        let snapshot = services.documents.snapshot(self.document)?;
        Ok(json!({
            "text": String::from_utf8(snapshot.bytes).unwrap(),
            "revision": services.documents.document_revision(self.document)?,
        }))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[test]
fn persistence_serializes_live_scoped_state_from_a_hidden_module_panel() {
    let directory = TempDir::new();
    let path = directory.write("project/source.txt", b"before");
    let mut workbench = host(&directory);
    workbench.open_or_focus(&path).unwrap();
    let text_panel = workbench.active_panel_id().unwrap();
    let document = workbench.active_document().unwrap();
    workbench
        .modules
        .registry
        .register(&ScopedStateModule)
        .unwrap();
    workbench
        .modules
        .instances
        .push(Box::new(ScopedStateModule));
    let state_panel = workbench
        .open_plugin_panel(
            "test.scoped-state.panel",
            Some(document),
            &Value::Null,
            None,
        )
        .unwrap();
    let text_index = workbench
        .tabs
        .iter()
        .position(|tab| tab.id == text_panel)
        .unwrap();
    workbench.switch_to_tab(text_index);
    workbench
        .session
        .apply_edits(
            document,
            workbench.session.document_revision(document).unwrap(),
            &[ByteEdit {
                range: 0..6,
                bytes: b"after".to_vec(),
            }],
        )
        .unwrap();
    let revision = workbench.session.document_revision(document).unwrap();
    workbench.persist_workspace().unwrap();
    let saved = workbench.last_state.as_ref().unwrap()["panels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|panel| panel["id"] == json!(state_panel))
        .unwrap();
    assert_eq!(
        saved["state"],
        json!({"text":"after", "revision": revision})
    );
    assert_eq!(workbench.active_panel_id(), Some(text_panel));
}

#[test]
fn nested_editor_state_restores_through_the_legacy_alias_and_persists_without_drawing() {
    let directory = TempDir::new();
    let path = directory.write("project/source.txt", b"one\nsecond\nthird\n");
    let path = path.canonicalize().unwrap();
    let mut workbench = host(&directory);
    workbench
        .restore_workspace(&json!({
            "version": 1, "focused": 11, "active_document_panel": 11,
            "panels": [{
                "id": 11, "kind": "plugin", "path": path, "document_kind": "text",
                "viewer": "bed.text", "panel_type": bed_module_editor::TEXT_PANEL_TYPE,
                "state": {"selections": [[1,3,1,1]], "primary": 0, "scroll": [12.0,45.0]}
            }]
        }))
        .unwrap();
    let panel = workbench.tabs.iter().find(|tab| tab.id == 11).unwrap();
    let panel = &panel.panel;
    assert_eq!(panel.kind, bed_module_editor::TEXT_PANEL_TYPE);
    assert_eq!(
        panel.viewer.as_deref(),
        Some(bed_module_editor::TEXT_VIEWER)
    );
    let view = panel.instance.view_id().unwrap();
    let state = workbench.session.view_snapshot(view).unwrap();
    assert_eq!(state.primary().ordered(), (1, 1, 1, 3));
    assert_eq!(state.requested_scroll, Some([12.0, 45.0]));
    assert_eq!(workbench.active_view(), Some(view));
    workbench.persist_workspace().unwrap();
    let saved = workbench.last_state.as_ref().unwrap()["panels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|panel| panel["id"] == json!(11))
        .unwrap();
    assert_eq!(saved["state"]["selections"], json!([[1, 3, 1, 1]]));
    assert_eq!(saved["state"]["scroll"], json!([12.0, 45.0]));
}
