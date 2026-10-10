//! Built-in text and byte editors hosted through the common workbench module API.
//!
//! `bed_editor_ui::EditorView` remains directly embeddable. This crate adds the desktop
//! panel lifecycle without making the workbench API depend on text editor types.
pub mod lsp;
pub mod presentation;
mod runtime;
use bed_document_session::{DocumentId, EditorSession};
use bed_editing::{editor_commands::CursorReveal, editor_view_state::Selection};
use bed_editor_ui::{EditorView, EditorViewOptions, hex_editor::HexEditor};
use bed_ui::util::popup_style::tooltip_text;
use bed_workbench_api::{
    CommandContext, DocumentKind, HostContext, HostRequest, Module, ModulePanel, ModuleServices,
    PanelAction, PanelPlacement, Registrar,
};
use dear_imgui_rs::Ui;
pub use runtime::{EditorAction, EditorMenuCommand, EditorMenuExtension, EditorRuntime};
use serde_json::{Value, json};
use std::{any::Any, cell::RefCell, io, path::Path, rc::Rc};

#[cfg(test)]
pub(crate) static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
#[allow(dead_code)]
#[path = "../../../tests/support/temp_dir.rs"]
pub(crate) mod test_support;

pub const MODULE_ID: &str = "bed.editor";
pub const TEXT_PANEL_TYPE: &str = "bed.editor.text-panel";
pub const HEX_PANEL_TYPE: &str = "bed.editor.hex-panel";
pub const TEXT_VIEWER: &str = "bed.editor.text";
pub const HEX_VIEWER: &str = "bed.editor.hex";
pub const DIAGNOSTICS_PANEL_TYPE: &str = "bed.editor.diagnostics-panel";
pub const REFERENCES_PANEL_TYPE: &str = "bed.editor.references-panel";
pub const LSP_DASHBOARD_PANEL_TYPE: &str = "bed.editor.language-servers-panel";

/// Application presentation settings shared by existing and newly created views.
/// Editor-specific feature contributions are registered in `options.extensions`.
#[derive(Clone, Debug)]
pub struct EditorConfig {
    pub options: EditorViewOptions,
    /// Animate explicit navigation jumps and zoom resets.
    pub navigation_animations: bool,
    pub lsp: crate::presentation::LspPresentationOptions,
}
impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            options: EditorViewOptions::default(),
            navigation_animations: true,
            lsp: Default::default(),
        }
    }
}

pub struct EditorModule {
    config: Rc<RefCell<EditorConfig>>,
    runtime: Rc<RefCell<EditorRuntime>>,
}
impl EditorModule {
    pub fn new(config: Rc<RefCell<EditorConfig>>) -> Self {
        Self {
            config,
            runtime: Rc::new(RefCell::new(EditorRuntime::default())),
        }
    }
    pub fn config(&self) -> &Rc<RefCell<EditorConfig>> {
        &self.config
    }
    pub fn runtime(&self) -> &Rc<RefCell<EditorRuntime>> {
        &self.runtime
    }
}
impl Default for EditorModule {
    fn default() -> Self {
        Self::new(Rc::new(RefCell::new(EditorConfig::default())))
    }
}
impl Module for EditorModule {
    fn id(&self) -> &'static str {
        MODULE_ID
    }
    fn register(&self, registrar: &mut Registrar<'_>) {
        registrar.panel_options(
            TEXT_PANEL_TYPE,
            "Text editor",
            false,
            PanelPlacement::Center,
            Some("document"),
        );
        registrar.panel_options(
            HEX_PANEL_TYPE,
            "Hex editor",
            false,
            PanelPlacement::Center,
            Some("hex"),
        );
        registrar.fallback_viewer(
            TEXT_VIEWER,
            "Text editor",
            TEXT_PANEL_TYPE,
            DocumentKind::Text,
            &["bed.text"],
        );
        registrar.fallback_viewer(
            HEX_VIEWER,
            "Hex editor",
            HEX_PANEL_TYPE,
            DocumentKind::Bytes,
            &["bed.hex"],
        );
        for (kind, title, legacy) in [
            (DIAGNOSTICS_PANEL_TYPE, "Diagnostics", "diagnostics"),
            (REFERENCES_PANEL_TYPE, "References", "references"),
            (LSP_DASHBOARD_PANEL_TYPE, "Language Servers", "lsp"),
        ] {
            registrar.panel_options(kind, title, false, PanelPlacement::Center, Some(legacy));
        }
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
        _: Option<DocumentId>,
        _: &Value,
    ) -> Result<Box<dyn ModulePanel>, String> {
        Err("Editor panels require document services".into())
    }
    fn create_panel_with_services(
        &mut self,
        panel_type: &str,
        input: bed_workbench_api::PanelInput,
        state: &Value,
        services: &mut ModuleServices<'_>,
    ) -> Result<Box<dyn ModulePanel>, String> {
        if input.local_file().is_some() {
            return Err("This module cannot open a local-file panel".into());
        }
        let document = input.document();
        if let Some(kind) = runtime::ToolKind::from_panel(panel_type) {
            return Ok(Box::new(ToolPanel {
                kind,
                runtime: self.runtime.clone(),
            }));
        }
        let kind = match panel_type {
            TEXT_PANEL_TYPE => DocumentKind::Text,
            HEX_PANEL_TYPE => DocumentKind::Bytes,
            _ => return Err(format!("Unknown editor panel type: {panel_type}")),
        };
        let created = document.is_none();
        let document = match document {
            Some(document) => document,
            None => services
                .documents
                .create_document_with_kind(&[], kind)
                .map_err(|error| error.to_string())?,
        };
        let panel = match panel_type {
            TEXT_PANEL_TYPE => TextPanel::with_runtime(
                services.documents,
                document,
                state,
                self.config.clone(),
                self.runtime.clone(),
            )
            .map(|panel| Box::new(panel) as Box<dyn ModulePanel>)
            .map_err(|error| error.to_string()),
            HEX_PANEL_TYPE => {
                if services
                    .documents
                    .document_kind(document)
                    .map_err(|error| error.to_string())?
                    != DocumentKind::Bytes
                {
                    Err("Hex editor requires an exact-byte document".into())
                } else {
                    let mut editor = HexEditor::new(document);
                    editor.restore_state(state);
                    editor
                        .synchronize(services.documents)
                        .map(|()| {
                            Box::new(HexPanel {
                                editor,
                                initial_revision: pristine_revision(services.documents, document),
                            }) as Box<dyn ModulePanel>
                        })
                        .map_err(|error| error.to_string())
                }
            }
            _ => unreachable!(),
        };
        if created && panel.is_err() {
            let _ = services.documents.close_document(
                document,
                bed_document_session::editor_session::ClosePolicy::Discard,
            );
        }
        panel
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn restore_workspace(&mut self, state: Option<&Value>, _: &str) {
        let mut runtime = self.runtime.borrow_mut();
        runtime.project_check = state.and_then(|value| {
            bed_document_session::ProjectCheckConfig::from_json(&value["project_check"])
        });
        runtime.restore_project_check = true;
    }
    fn save_workspace(&self) -> Value {
        json!({"project_check":self.runtime.borrow().project_check.as_ref().map(|config| config.to_json())})
    }
    fn tick_with_services(
        &mut self,
        _: &HostContext<'_>,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        self.runtime.borrow_mut().tick(services.documents, requests)
    }
    fn document_events(
        &mut self,
        session: &EditorSession,
        _: &[bed_document_session::SessionEvent],
    ) -> io::Result<()> {
        self.runtime.borrow_mut().retain_requests(session);
        Ok(())
    }
    fn shutdown(&mut self, _: &mut ModuleServices<'_>) {
        self.runtime.borrow_mut().cancel_requests();
    }
}

/// The concrete editor capability used by integrations needing text presentation.
/// The common module contract never needs to mention `EditorView`.
pub struct TextPanel {
    view: EditorView,
    config: Rc<RefCell<EditorConfig>>,
    state: Value,
    runtime: Rc<RefCell<EditorRuntime>>,
    initial_revision: Option<bed_workbench_api::Revision>,
}
impl TextPanel {
    pub fn new(
        session: &mut EditorSession,
        document: DocumentId,
        state: &Value,
        config: Rc<RefCell<EditorConfig>>,
    ) -> io::Result<Self> {
        Self::with_runtime(
            session,
            document,
            state,
            config,
            Rc::new(RefCell::new(EditorRuntime::default())),
        )
    }
    pub fn with_runtime(
        session: &mut EditorSession,
        document: DocumentId,
        state: &Value,
        config: Rc<RefCell<EditorConfig>>,
        runtime: Rc<RefCell<EditorRuntime>>,
    ) -> io::Result<Self> {
        let view = EditorView::new(session, document)?;
        if let Err(error) = restore_text_state(session, view.id(), state) {
            session.detach_view(view.id());
            return Err(error);
        }
        session.request_focus(view.id())?;
        let mut panel = Self {
            view,
            config,
            state: Value::Null,
            runtime,
            initial_revision: pristine_revision(session, document),
        };
        panel.refresh_state(session)?;
        panel.runtime.borrow_mut().record_overlay(&panel.view);
        Ok(panel)
    }
    pub fn view(&self) -> &EditorView {
        &self.view
    }
    pub fn view_mut(&mut self) -> &mut EditorView {
        &mut self.view
    }
    pub fn refresh_state(&mut self, session: &EditorSession) -> io::Result<()> {
        let state = session.view_snapshot(self.view.id())?;
        self.state = json!({
            "selections": state.selections.iter().map(|selection| [selection.head_row, selection.head_column, selection.anchor_row, selection.anchor_column]).collect::<Vec<_>>(),
            "primary": state.primary_index,
            "scroll": state.requested_scroll.unwrap_or(state.scroll_position),
        });
        Ok(())
    }
    /// Embed this editor with surface-specific presentation while preserving the
    /// shared document, menus, LSP integration and workbench lifecycle.
    pub fn draw_with_options(
        &mut self,
        ui: &Ui,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
        options: &EditorViewOptions,
    ) -> io::Result<bed_editor_ui::ViewResponse> {
        let mut config = self.config.borrow().clone();
        config.options = options.clone();
        self.view
            .set_navigation_animations(config.navigation_animations);
        if let Some(activity) =
            EditorRuntime::document_lsp_activity(services.documents, self.view.document_id())
        {
            ui.text_disabled(&activity);
            if ui.is_item_hovered() {
                tooltip_text(ui, &activity);
            }
        }
        let response = self.view.draw(ui, services.documents, &config.options)?;
        self.runtime.borrow_mut().after_draw(
            ui,
            &mut self.view,
            response.clone(),
            services.documents,
            &config,
            requests,
        )?;
        self.runtime.borrow_mut().record_overlay(&self.view);
        self.refresh_state(services.documents)?;
        Ok(response)
    }
}
impl ModulePanel for TextPanel {
    fn is_input_empty(&self, host: &HostContext<'_>) -> bool {
        input_is_pristine(host, self.view.document_id(), self.initial_revision)
    }
    fn title(&self, host: &HostContext<'_>) -> String {
        document_title(host, self.view.document_id(), false)
    }
    fn draw(&mut self, ui: &Ui, _: &HostContext<'_>, _: &mut Vec<HostRequest>) {
        ui.text_disabled("Text panel requires workbench document services");
    }
    fn draw_with_services(
        &mut self,
        ui: &Ui,
        host: &HostContext<'_>,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        self.runtime
            .borrow_mut()
            .set_viewer_menu(self.view.id(), host.viewer_menu.cloned());
        let options = self.config.borrow().options.clone();
        self.draw_with_options(ui, services, requests, &options)
            .map(|_| ())
    }
    fn action_with_services(
        &mut self,
        action: PanelAction,
        _: &HostContext<'_>,
        services: &mut ModuleServices<'_>,
        _requests: &mut Vec<HostRequest>,
    ) -> Result<bool, String> {
        match action {
            PanelAction::Find => self
                .view
                .open_find(services.documents)
                .map_err(|error| error.to_string())?,
            PanelAction::GoToLine => self.view.open_line_jump(),
            PanelAction::SelectAll => services
                .documents
                .with_commands(self.view.id(), |commands| commands.select_all())
                .map_err(|error| error.to_string())?,
            PanelAction::Undo if self.view.read_only() => {}
            PanelAction::Redo if self.view.read_only() => {}
            PanelAction::Undo => services
                .documents
                .with_commands(self.view.id(), |commands| commands.undo())
                .map_err(|error| error.to_string())?,
            PanelAction::Redo => services
                .documents
                .with_commands(self.view.id(), |commands| commands.redo())
                .map_err(|error| error.to_string())?,
            PanelAction::CommitEdit => {}
        }
        self.refresh_state(services.documents)
            .map_err(|error| error.to_string())?;
        self.runtime.borrow_mut().record_overlay(&self.view);
        Ok(true)
    }
    fn close_with_services(
        &mut self,
        services: &mut ModuleServices<'_>,
        _: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        // Closing a surface releases only its view. The host closes the document
        // after checking every attached panel and applying its save policy.
        services.documents.detach_view(self.view.id());
        self.runtime.borrow_mut().forget_view(self.view.id());
        Ok(())
    }
    fn save_state(&self) -> Value {
        self.state.clone()
    }
    fn save_state_with_services(&mut self, services: &mut ModuleServices<'_>) -> io::Result<Value> {
        self.refresh_state(services.documents)?;
        Ok(self.save_state())
    }
    fn view_id(&self) -> Option<bed_document_session::ViewId> {
        Some(self.view.id())
    }
    fn window_padding(&self) -> Option<[f32; 2]> {
        Some([0.0; 2])
    }
    fn attached_document(&self) -> Option<DocumentId> {
        Some(self.view.document_id())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

pub struct HexPanel {
    editor: HexEditor,
    initial_revision: Option<bed_workbench_api::Revision>,
}

struct ToolPanel {
    kind: runtime::ToolKind,
    runtime: Rc<RefCell<EditorRuntime>>,
}
impl ModulePanel for ToolPanel {
    fn title(&self, _: &HostContext<'_>) -> String {
        self.kind.title().into()
    }
    fn draw(&mut self, ui: &Ui, _: &HostContext<'_>, _: &mut Vec<HostRequest>) {
        ui.text_disabled("Editor tools require document services");
    }
    fn draw_with_services(
        &mut self,
        ui: &Ui,
        _: &HostContext<'_>,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        self.runtime
            .borrow_mut()
            .draw_tool(self.kind, ui, services.documents, requests)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
impl HexPanel {
    pub fn editor(&self) -> &HexEditor {
        &self.editor
    }
    pub fn editor_mut(&mut self) -> &mut HexEditor {
        &mut self.editor
    }
}
impl ModulePanel for HexPanel {
    fn is_input_empty(&self, host: &HostContext<'_>) -> bool {
        input_is_pristine(host, self.editor.document(), self.initial_revision)
    }
    fn title(&self, host: &HostContext<'_>) -> String {
        document_title(host, self.editor.document(), true)
    }
    fn draw(&mut self, ui: &Ui, _: &HostContext<'_>, _: &mut Vec<HostRequest>) {
        ui.text_disabled("Hex panel requires workbench document services");
    }
    fn draw_with_services(
        &mut self,
        ui: &Ui,
        _: &HostContext<'_>,
        services: &mut ModuleServices<'_>,
        _: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        self.editor.draw(ui, services.documents)
    }
    fn action_with_services(
        &mut self,
        action: PanelAction,
        _: &HostContext<'_>,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> Result<bool, String> {
        match action {
            PanelAction::SelectAll => self
                .editor
                .select_all(services.documents)
                .map_err(|error| error.to_string())?,
            PanelAction::Undo => requests.push(HostRequest::Undo {
                document: self.editor.document(),
            }),
            PanelAction::Redo => requests.push(HostRequest::Redo {
                document: self.editor.document(),
            }),
            PanelAction::CommitEdit => {}
            PanelAction::Find | PanelAction::GoToLine => return Ok(false),
        }
        Ok(true)
    }
    fn save_state(&self) -> Value {
        self.editor.state()
    }
    fn save_state_with_services(&mut self, services: &mut ModuleServices<'_>) -> io::Result<Value> {
        self.editor.synchronize(services.documents)?;
        Ok(self.save_state())
    }
    fn attached_document(&self) -> Option<DocumentId> {
        Some(self.editor.document())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

fn document_title(host: &HostContext<'_>, document: DocumentId, hex: bool) -> String {
    let Some(document) = host.document(document) else {
        return "Closed".into();
    };
    let name = if document.path.is_empty() {
        "Untitled".to_owned()
    } else {
        Path::new(&document.path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    };
    if hex { format!("{name} [Hex]") } else { name }
}

// Once a buffer has been used, undoing its contents must not make it available
// for replacement. The initial revision also protects clean empty byte editors.
fn pristine_revision(
    session: &EditorSession,
    document: DocumentId,
) -> Option<bed_workbench_api::Revision> {
    if session
        .with_document(document, |state| {
            state.path.is_empty()
                && state.byte_size() == 0
                && !state.dirty
                && state.version == 0
                && !state.read_only
        })
        .ok()?
        && session.original_path(document).ok()?.is_none()
    {
        session.document_revision(document).ok()
    } else {
        None
    }
}

fn input_is_pristine(
    host: &HostContext<'_>,
    document: DocumentId,
    initial_revision: Option<bed_workbench_api::Revision>,
) -> bool {
    host.document(document).is_some_and(|document| {
        initial_revision == Some(document.revision)
            && document.path.is_empty()
            && document.bytes.is_empty()
            && !document.dirty
    })
}

fn restore_text_state(
    session: &mut EditorSession,
    view: bed_document_session::ViewId,
    state: &Value,
) -> io::Result<()> {
    if let Some(values) = state.get("selections").and_then(Value::as_array) {
        let selections = values
            .iter()
            .filter_map(|value| {
                let value = value.as_array()?;
                Some(Selection {
                    head_row: value.first()?.as_i64()? as i32,
                    head_column: value.get(1)?.as_i64()? as i32,
                    anchor_row: value.get(2)?.as_i64()? as i32,
                    anchor_column: value.get(3)?.as_i64()? as i32,
                    preferred_column: 0,
                })
            })
            .collect();
        session.with_commands(view, |commands| {
            commands.set_selections(
                selections,
                state.get("primary").and_then(Value::as_u64).unwrap_or(0) as usize,
                CursorReveal::Ensure,
            )
        })?;
    }
    if let Some(scroll) = state
        .get("scroll")
        .and_then(Value::as_array)
        .filter(|scroll| scroll.len() == 2)
    {
        session.set_scroll(
            view,
            scroll[0].as_f64().unwrap_or(0.0) as f32,
            scroll[1].as_f64().unwrap_or(0.0) as f32,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_document_session::ByteEdit;
    use bed_workbench_api::{FileDialogService, Registry, TerminalLaunch, TerminalService};

    struct NoDialogs;
    impl FileDialogService for NoDialogs {
        fn pick_file(&mut self, _: &Path, _: &[&str]) -> Option<std::path::PathBuf> {
            unreachable!()
        }
    }

    struct NoTerminals;
    impl TerminalService for NoTerminals {
        fn spawn(&mut self, _: TerminalLaunch) -> io::Result<(u64, u32)> {
            unreachable!()
        }
        fn stop(&mut self, _: u64) {
            unreachable!()
        }
        fn release(&mut self, _: u64) {
            unreachable!()
        }
    }

    fn input_empty(panel: &dyn ModulePanel, session: &EditorSession) -> bool {
        let documents = session
            .document_ids()
            .into_iter()
            .map(|id| {
                let document = session.snapshot(id).unwrap();
                bed_workbench_api::PluginDocument {
                    id,
                    path: document.path,
                    kind: document.kind,
                    language_id: document.language_id,
                    revision: session.document_revision(id).unwrap(),
                    dirty: document.dirty,
                    bytes: document.bytes.into(),
                    text: None,
                }
            })
            .collect::<Vec<_>>();
        panel.is_input_empty(&HostContext {
            remote: false,
            documents: &documents,
            active_document: None,
            settings: &Value::Null,
            textures: &Default::default(),
            animations: false,
            workspace: session.workspace_id().0,
            diagnostics: &Value::Null,
            viewer_menu: None,
            default_viewers: &Value::Null,
        })
    }

    #[test]
    fn default_editors_create_independent_clean_buffers_and_never_replace_used_input() {
        let mut session = EditorSession::new();
        let mut terminals = NoTerminals;
        let mut dialogs = NoDialogs;
        let mut services = ModuleServices {
            documents: &mut session,
            terminals: &mut terminals,
            dialogs: &mut dialogs,
            project_root: "",
            working_directory: "",
            active_view: None,
            resources: Default::default(),
            settings_ui: None,
        };
        let mut module = EditorModule::default();
        for (panel_type, kind) in [
            (TEXT_PANEL_TYPE, DocumentKind::Text),
            (HEX_PANEL_TYPE, DocumentKind::Bytes),
        ] {
            let mut first = module
                .create_panel_with_services(
                    panel_type,
                    bed_workbench_api::PanelInput::None,
                    &Value::Null,
                    &mut services,
                )
                .unwrap();
            let mut second = module
                .create_panel_with_services(
                    panel_type,
                    bed_workbench_api::PanelInput::None,
                    &Value::Null,
                    &mut services,
                )
                .unwrap();
            let document = first.attached_document().unwrap();
            assert_ne!(document, second.attached_document().unwrap());
            let snapshot = services.documents.snapshot(document).unwrap();
            assert_eq!(snapshot.kind, kind);
            assert!(snapshot.bytes.is_empty() && snapshot.path.is_empty() && !snapshot.dirty);
            assert!(input_empty(first.as_ref(), services.documents));
            assert!(input_empty(second.as_ref(), services.documents));
            services
                .documents
                .apply_edits(
                    document,
                    services.documents.document_revision(document).unwrap(),
                    &[ByteEdit {
                        range: 0..0,
                        bytes: b"x".to_vec(),
                    }],
                )
                .unwrap();
            assert!(!input_empty(first.as_ref(), services.documents));
            services.documents.undo_document(document).unwrap();
            assert!(
                services
                    .documents
                    .snapshot(document)
                    .unwrap()
                    .bytes
                    .is_empty()
            );
            assert!(!input_empty(first.as_ref(), services.documents));
            assert!(input_empty(second.as_ref(), services.documents));
            let mut reopened = module
                .create_panel_with_services(
                    panel_type,
                    Some(document).into(),
                    &Value::Null,
                    &mut services,
                )
                .unwrap();
            assert!(!input_empty(reopened.as_ref(), services.documents));
            reopened
                .close_with_services(&mut services, &mut Vec::new())
                .unwrap();
            first
                .close_with_services(&mut services, &mut Vec::new())
                .unwrap();
            second
                .close_with_services(&mut services, &mut Vec::new())
                .unwrap();
            assert!(services.documents.view_ids(document).is_empty());
        }
    }

    #[test]
    fn built_in_editors_register_default_viewers_and_legacy_aliases() {
        let mut registry = Registry::default();
        registry.register(&EditorModule::default()).unwrap();
        assert_eq!(
            registry.default_viewer(DocumentKind::Text).unwrap().id,
            TEXT_VIEWER
        );
        assert_eq!(
            registry.default_viewer(DocumentKind::Bytes).unwrap().id,
            HEX_VIEWER
        );
        assert_eq!(
            registry.viewer("bed.text").unwrap().panel_type,
            TEXT_PANEL_TYPE
        );
        assert_eq!(
            registry.viewer("bed.hex").unwrap().panel_type,
            HEX_PANEL_TYPE
        );
        assert!(registry.viewer_for_path("main.rs").is_none());
    }

    #[test]
    fn hidden_text_panels_persist_transformed_selections_and_pending_scroll() {
        let mut session = EditorSession::new();
        let document = session.create_document(b"abc\ndef").unwrap();
        let config = Rc::new(RefCell::new(EditorConfig::default()));
        let mut panel = TextPanel::new(
            &mut session,
            document,
            &json!({
                "selections": [[0, 2, 0, 1]], "primary": 0, "scroll": [4.0, 8.0]
            }),
            config.clone(),
        )
        .unwrap();
        let mut sibling = TextPanel::new(&mut session, document, &Value::Null, config).unwrap();
        assert_ne!(panel.view().id(), sibling.view().id());
        session
            .apply_edits(
                document,
                session.document_revision(document).unwrap(),
                &[ByteEdit {
                    range: 0..0,
                    bytes: b"XY".to_vec(),
                }],
            )
            .unwrap();
        panel.refresh_state(&session).unwrap();
        let saved = panel.save_state();
        assert_eq!(saved["selections"], json!([[0, 4, 0, 3]]));
        assert_eq!(saved["scroll"], json!([4.0, 8.0]));
        sibling.refresh_state(&session).unwrap();
        assert_ne!(saved["selections"], sibling.save_state()["selections"]);
    }

    #[test]
    fn closing_last_text_panel_preserves_host_owned_document_and_history() {
        let mut session = EditorSession::new();
        let document = session.create_document(b"original").unwrap();
        let mut terminals = NoTerminals;
        let mut dialogs = NoDialogs;
        let mut services = ModuleServices {
            documents: &mut session,
            terminals: &mut terminals,
            dialogs: &mut dialogs,
            project_root: "",
            working_directory: "",
            active_view: None,
            resources: Default::default(),
            settings_ui: None,
        };
        let mut module = EditorModule::default();
        let mut panel = module
            .create_panel_with_services(
                TEXT_PANEL_TYPE,
                Some(document).into(),
                &Value::Null,
                &mut services,
            )
            .unwrap();
        let view = panel.view_id().unwrap();
        services
            .documents
            .apply_edits(
                document,
                services.documents.document_revision(document).unwrap(),
                &[ByteEdit {
                    range: 0..8,
                    bytes: b"changed".to_vec(),
                }],
            )
            .unwrap();
        panel
            .close_with_services(&mut services, &mut Vec::new())
            .unwrap();
        assert_eq!(services.documents.document_for_view(view), None);
        assert_eq!(
            services.documents.snapshot(document).unwrap().bytes,
            b"changed"
        );
        services.documents.undo_document(document).unwrap();
        assert_eq!(
            services.documents.snapshot(document).unwrap().bytes,
            b"original"
        );
    }

    #[test]
    fn panel_history_restores_the_originating_split_caret_without_moving_its_sibling() {
        let mut session = EditorSession::new();
        let document = session.create_document(b"abcdef").unwrap();
        let config = Rc::new(RefCell::new(EditorConfig::default()));
        let left = TextPanel::new(&mut session, document, &Value::Null, config.clone()).unwrap();
        let mut right = TextPanel::new(&mut session, document, &Value::Null, config).unwrap();
        session
            .with_commands(right.view().id(), |commands| {
                commands.set_cursor(0, 3, false, CursorReveal::Ensure);
                commands.type_text(b"X");
            })
            .unwrap();
        assert_eq!(session.view_snapshot(left.view().id()).unwrap().column, 0);
        assert_eq!(session.view_snapshot(right.view().id()).unwrap().column, 4);
        let textures = std::collections::HashMap::new();
        let host = HostContext {
            remote: false,
            default_viewers: &Value::Null,
            viewer_menu: None,
            documents: &[],
            active_document: Some(document),
            settings: &Value::Null,
            textures: &textures,
            animations: false,
            workspace: session.workspace_id().0,
            diagnostics: &Value::Null,
        };
        let mut terminals = NoTerminals;
        let mut dialogs = NoDialogs;
        let mut services = ModuleServices {
            documents: &mut session,
            terminals: &mut terminals,
            dialogs: &mut dialogs,
            project_root: "",
            working_directory: "",
            active_view: Some(right.view().id()),
            resources: Default::default(),
            settings_ui: None,
        };
        let mut requests = Vec::new();
        right
            .action_with_services(PanelAction::Undo, &host, &mut services, &mut requests)
            .unwrap();
        assert_eq!(
            services.documents.snapshot(document).unwrap().bytes,
            b"abcdef"
        );
        assert_eq!(
            services
                .documents
                .view_snapshot(right.view().id())
                .unwrap()
                .column,
            3
        );
        assert_eq!(
            services
                .documents
                .view_snapshot(left.view().id())
                .unwrap()
                .column,
            0
        );
        right
            .action_with_services(PanelAction::Redo, &host, &mut services, &mut requests)
            .unwrap();
        assert_eq!(
            services.documents.snapshot(document).unwrap().bytes,
            b"abcXdef"
        );
        assert_eq!(
            services
                .documents
                .view_snapshot(right.view().id())
                .unwrap()
                .column,
            4
        );
        assert_eq!(
            services
                .documents
                .view_snapshot(left.view().id())
                .unwrap()
                .column,
            0
        );
    }

    #[test]
    fn panels_reject_mismatched_document_kinds_without_leaking_views() {
        let mut session = EditorSession::new();
        let text = session.create_document(b"text").unwrap();
        let bytes = session
            .create_document_with_kind(&[0xff], DocumentKind::Bytes)
            .unwrap();
        let mut terminals = NoTerminals;
        let mut dialogs = NoDialogs;
        let mut services = ModuleServices {
            documents: &mut session,
            terminals: &mut terminals,
            dialogs: &mut dialogs,
            project_root: "",
            working_directory: "",
            active_view: None,
            resources: Default::default(),
            settings_ui: None,
        };
        let mut module = EditorModule::default();
        assert!(
            module
                .create_panel_with_services(
                    HEX_PANEL_TYPE,
                    Some(text).into(),
                    &Value::Null,
                    &mut services
                )
                .is_err()
        );
        assert!(
            module
                .create_panel_with_services(
                    TEXT_PANEL_TYPE,
                    Some(bytes).into(),
                    &Value::Null,
                    &mut services
                )
                .is_err()
        );
        assert!(services.documents.view_ids(bytes).is_empty());
    }
}
