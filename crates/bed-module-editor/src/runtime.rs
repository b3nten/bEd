//! Text editor interaction and language-server presentation owned by the editor feature.
use crate::{
    DIAGNOSTICS_PANEL_TYPE, EditorConfig, LSP_DASHBOARD_PANEL_TYPE, REFERENCES_PANEL_TYPE,
    lsp::lsp_ui::{ContextLspAction, LspAction, LspUi, LspView},
};
use bed_document_session::{DocumentId, EditorSession, ViewId};
use bed_editing::{editor_commands::CursorReveal, editor_events::Overlay};
use bed_editor_ui::{EditorView, HostAction, TextHit};
use bed_lsp::{lsp_locations::LspLocation, workspace_lsp::LspRequestOrigin};
use bed_ui::util::popup_style::context_menu_style;
use bed_workbench_api::{CommandContext, HostRequest, PanelTarget, SelectionContext};
use dear_imgui_rs::{MouseButton, Ui, sys};
use serde_json::Value;
use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::{CStr, CString},
    io,
    rc::{Rc, Weak},
};

/// Application operations produced by the editor. These retain their originating
/// view and request generation even if focus changes before the host handles them.
#[derive(Clone, Debug)]
pub enum EditorAction {
    Host {
        view: ViewId,
        action: HostAction,
    },
    Navigate {
        location: LspLocation,
        origin: Option<LspRequestOrigin>,
    },
    Command {
        id: String,
        context: CommandContext,
    },
}

#[derive(Clone, Debug)]
pub struct EditorMenuCommand {
    pub id: String,
    pub label: String,
    pub enabled: bool,
}

/// A concrete text editor extension: menu items receive the captured selection,
/// and can submit ordinary document/service requests without knowing panel internals.
pub trait EditorMenuExtension {
    fn commands(&self, session: &EditorSession, context: &CommandContext)
    -> Vec<EditorMenuCommand>;
    fn invoke(
        &mut self,
        id: &str,
        context: &CommandContext,
        session: &mut EditorSession,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()>;
}
type MenuProvider = Rc<RefCell<dyn EditorMenuExtension>>;

#[derive(Clone, Copy)]
pub(crate) enum ToolKind {
    Diagnostics,
    References,
    Dashboard,
}
impl ToolKind {
    pub fn from_panel(panel: &str) -> Option<Self> {
        match panel {
            DIAGNOSTICS_PANEL_TYPE => Some(Self::Diagnostics),
            REFERENCES_PANEL_TYPE => Some(Self::References),
            LSP_DASHBOARD_PANEL_TYPE => Some(Self::Dashboard),
            _ => None,
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Self::Diagnostics => "Diagnostics",
            Self::References => "References",
            Self::Dashboard => "Language Servers",
        }
    }
}

pub struct EditorRuntime {
    lsp_ui: LspUi,
    navigation_kind: ContextLspAction,
    pending_navigation: Option<ContextLspAction>,
    contexts: HashMap<ViewId, CommandContext>,
    menu_commands: HashMap<ViewId, Vec<EditorMenuCommand>>,
    viewer_menus: HashMap<ViewId, bed_workbench_api::ViewerMenu>,
    menu_extensions: Vec<Weak<RefCell<dyn EditorMenuExtension>>>,
    actions: Vec<EditorAction>,
    overlays: HashMap<ViewId, Overlay>,
    historical_contexts: HashMap<ViewId, Vec<u8>>,
    pub(crate) project_check: Option<bed_document_session::ProjectCheckConfig>,
    pub(crate) restore_project_check: bool,
    check_json: String,
    check_error: Option<String>,
    diagnostic_search: String,
    diagnostic_errors: bool,
    diagnostic_warnings: bool,
    diagnostic_other: bool,
}
impl Default for EditorRuntime {
    fn default() -> Self {
        Self {
            lsp_ui: LspUi::default(),
            navigation_kind: ContextLspAction::References,
            pending_navigation: None,
            contexts: HashMap::new(),
            menu_commands: HashMap::new(),
            viewer_menus: HashMap::new(),
            menu_extensions: Vec::new(),
            actions: Vec::new(),
            overlays: HashMap::new(),
            historical_contexts: HashMap::new(),
            project_check: None,
            restore_project_check: false,
            check_json: String::new(),
            check_error: None,
            diagnostic_search: String::new(),
            diagnostic_errors: true,
            diagnostic_warnings: true,
            diagnostic_other: true,
        }
    }
}
impl EditorRuntime {
    pub fn take_actions(&mut self) -> Vec<EditorAction> {
        std::mem::take(&mut self.actions)
    }
    pub fn register_menu(&mut self, provider: &MenuProvider) {
        self.menu_extensions
            .retain(|entry| entry.strong_count() > 0);
        let provider = Rc::downgrade(provider);
        if !self
            .menu_extensions
            .iter()
            .any(|entry| entry.ptr_eq(&provider))
        {
            self.menu_extensions.push(provider);
        }
    }
    /// Bridge existing workbench command registrations onto the concrete editor
    /// menu surface. New native features can register an EditorMenuExtension.
    pub fn set_menu_commands(&mut self, view: ViewId, commands: Vec<EditorMenuCommand>) {
        self.menu_commands.insert(view, commands);
    }
    pub fn set_viewer_menu(&mut self, view: ViewId, menu: Option<bed_workbench_api::ViewerMenu>) {
        if let Some(menu) = menu {
            self.viewer_menus.insert(view, menu);
        } else {
            self.viewer_menus.remove(&view);
        }
    }
    pub fn context(&self, view: ViewId) -> Option<&CommandContext> {
        self.contexts.get(&view)
    }
    pub fn overlay(&self, view: ViewId) -> Overlay {
        self.overlays.get(&view).copied().unwrap_or(Overlay::None)
    }
    pub(crate) fn record_overlay(&mut self, view: &EditorView) {
        self.overlays.insert(
            view.id(),
            if view.find_visible() {
                Overlay::Find
            } else if view.line_jump_visible() {
                Overlay::LineJump
            } else {
                Overlay::None
            },
        );
    }
    pub fn set_context(&mut self, view: ViewId, context: CommandContext) {
        self.contexts.insert(view, context);
    }
    pub fn forget_view(&mut self, view: ViewId) {
        self.contexts.remove(&view);
        self.menu_commands.remove(&view);
        self.viewer_menus.remove(&view);
        self.overlays.remove(&view);
        self.historical_contexts.remove(&view);
    }
    pub fn cancel_requests(&mut self) {
        self.lsp_ui.cancel_requests();
        self.pending_navigation = None;
        self.actions.clear();
    }
    pub fn retain_requests(&mut self, session: &EditorSession) {
        self.lsp_ui
            .retain_requests(|origin| session.accepts_lsp_origin(origin));
        self.contexts
            .retain(|view, _| session.document_for_view(*view).is_some());
        self.menu_commands
            .retain(|view, _| session.document_for_view(*view).is_some());
        self.overlays
            .retain(|view, _| session.document_for_view(*view).is_some());
        self.historical_contexts
            .retain(|view, _| session.document_for_view(*view).is_some());
    }
    pub fn capture_context(
        &mut self,
        session: &EditorSession,
        view: ViewId,
    ) -> io::Result<CommandContext> {
        let context = Self::selection_context(session, view)?;
        self.contexts.insert(view, context.clone());
        Ok(context)
    }
    pub fn selection_context(session: &EditorSession, view: ViewId) -> io::Result<CommandContext> {
        let document = session
            .document_for_view(view)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Editor view is detached"))?;
        let revision = session.document_revision(document)?;
        let selections = session.view_snapshot(view)?.selections;
        let (path, ranges, text) = session.with_document(document, |state| {
            let mut ranges = Vec::new();
            let mut selected = Vec::new();
            for selection in selections.iter().filter(|selection| !selection.empty()) {
                let (ar, ac, br, bc) = selection.ordered();
                let range = state.offset_from_row_col(ar, ac)..state.offset_from_row_col(br, bc);
                let mut bytes = vec![0; range.len()];
                state.copy_bytes(range.start, range.len(), &mut bytes);
                selected.push(String::from_utf8_lossy(&bytes).into_owned());
                ranges.push(range);
            }
            (state.path.clone(), ranges, selected.join("\n"))
        })?;
        Ok(CommandContext {
            path: Some(path),
            document: Some(document),
            revision: Some(revision),
            selection: Some(SelectionContext {
                document,
                revision,
                ranges,
                text,
            }),
        })
    }
    pub(crate) fn after_draw(
        &mut self,
        ui: &Ui,
        view: &mut EditorView,
        response: bed_editor_ui::ViewResponse,
        session: &mut EditorSession,
        config: &EditorConfig,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        if let Some(request) = response.definition_request {
            self.request_lsp(
                session,
                view.id(),
                ContextLspAction::Definition,
                Some((request.row, request.column)),
            )?;
        }
        self.actions.extend(
            response
                .actions
                .into_iter()
                .map(|action| EditorAction::Host {
                    view: view.id(),
                    action,
                }),
        );
        self.text_context_menu(ui, view, session, config, requests)?;
        self.lsp_ui
            .retain_requests(|origin| session.accepts_lsp_origin(origin));
        if let Some(client) = session
            .lsp()
            .and_then(|pool| pool.client_for_document(view.document_id()))
        {
            let snapshot = session.snapshot(view.document_id())?;
            let view_id = view.id();
            let origin = session.lsp().and_then(|pool| {
                pool.request_origin(view.document_id(), view_id, snapshot.generation, 0)
            });
            let frame = view.presentation();
            let layout = frame.layout;
            let hover = frame.hover_info;
            let dismissed = frame.hover_dismissed;
            if response.focused
                && !config.options.block_input
                && !session.view_snapshot(view_id)?.block_input
                && !ui.is_any_item_active()
                && let Some(origin) = origin
            {
                let requested = session.with_view(view_id, |editor| {
                    self.lsp_ui.keybinds_with_origin(
                        ui,
                        &mut client.borrow_mut(),
                        editor,
                        &config.lsp,
                        origin,
                    )
                })?;
                if requested {
                    for kind in [ContextLspAction::Definition, ContextLspAction::References] {
                        if self.lsp_ui.navigation_results(kind).is_some_and(|results| {
                            results.origin.is_some_and(|request| {
                                request.view_id == origin.view_id
                                    && request.document_id == origin.document_id
                                    && request.version == origin.version
                            }) && results.pending
                        }) {
                            self.pending_navigation = Some(kind);
                        }
                    }
                }
            }
            let mouse = ui.io().mouse_pos();
            let hover_here = mouse[0] >= layout.pane_pos[0]
                && mouse[0] < layout.pane_pos[0] + layout.pane_size[0]
                && mouse[1] >= layout.pane_pos[1]
                && mouse[1] < layout.pane_pos[1] + layout.pane_size[1];
            let caret_here = self.lsp_ui.symbol_at_caret()
                && self
                    .lsp_ui
                    .symbol_origin()
                    .is_some_and(|request| request.view_id == view_id);
            let over_symbol_popup = self.lsp_ui.symbol_popup_contains(mouse);
            let popup_here = over_symbol_popup
                && self
                    .lsp_ui
                    .symbol_origin()
                    .is_some_and(|request| request.view_id == view_id);
            if caret_here
                || (!self.lsp_ui.symbol_at_caret()
                    && (popup_here || (hover_here && !over_symbol_popup)))
            {
                self.lsp_ui.set_mouse_origin(origin);
                let caret_visual_row = Some(view.visual_row(session.view_snapshot(view_id)?.row));
                session.with_view(view_id, |editor| {
                    self.lsp_ui.render_hover(
                        ui,
                        &mut client.borrow_mut(),
                        editor,
                        &config.lsp,
                        LspView {
                            layout: &layout,
                            hover_info: hover,
                            hover_dismissed: dismissed,
                            tooltip_arbiter: frame.tooltip_arbiter,
                            caret_visual_row,
                        },
                    )
                })?;
            }
        }
        Ok(())
    }
    pub fn document_lsp_activity(session: &EditorSession, document: DocumentId) -> Option<String> {
        let client = session.lsp()?.client_for_document(document)?;
        let client = client.borrow();
        if !client.is_process_started() {
            return None;
        }
        let language = client.current_language();
        if let Some(progress) = client.progress().into_iter().find(|job| !job.finished) {
            let mut text = format!("{language}: {}", progress.title);
            if let Some(message) = progress
                .message
                .filter(|message| !message.is_empty() && *message != progress.title)
            {
                text.push_str(" — ");
                text.push_str(&message);
            }
            if let Some(percentage) = progress.percentage {
                text.push_str(&format!(" ({percentage}%)"));
            }
            return Some(text);
        }
        (!client.is_initialized()).then(|| format!("{language}: starting language server…"))
    }
    fn text_context_menu(
        &mut self,
        ui: &Ui,
        view: &mut EditorView,
        session: &mut EditorSession,
        config: &EditorConfig,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        let layout = view.presentation().layout;
        let mouse = ui.io().mouse_pos();
        let inside = mouse[0] >= layout.text_pos[0]
            && mouse[0] < layout.pane_pos[0] + layout.pane_size[0]
            && mouse[1] >= layout.pane_pos[1]
            && mouse[1] < layout.pane_pos[1] + layout.pane_size[1];
        let name = format!("##text_actions_{}", view.id().0);
        let historical_name = format!("##historical_actions_{}", view.id().0);
        if let Some(_popup) = ui.begin_popup(&historical_name) {
            if ui.menu_item("Copy removed line") {
                if let Some(bytes) = self.historical_contexts.get(&view.id()) {
                    set_clipboard(ui, bytes);
                }
            }
            ui.separator();
            if ui.menu_item_enabled_selected_no_shortcut("Reset Zoom", false, view.zoom() != 1.0) {
                view.reset_zoom();
            }
        }
        if inside && ui.is_mouse_clicked(MouseButton::Right) {
            let state = session.view_snapshot(view.id())?;
            let (row, column) = match view.hit_test(ui, session, mouse)? {
                TextHit::Document { row, column } => (row, column),
                TextHit::Historical { bytes, .. } => {
                    self.historical_contexts.insert(view.id(), bytes);
                    ui.open_popup(&historical_name);
                    return Ok(());
                }
                _ => return Ok(()),
            };
            let selected = state.selections.iter().any(|selection| {
                let (ar, ac, br, bc) = selection.ordered();
                !selection.empty() && (row, column) >= (ar, ac) && (row, column) <= (br, bc)
            });
            if !selected {
                session.with_commands(view.id(), |commands| {
                    commands.set_cursor(row, column, false, CursorReveal::Ensure)
                })?;
            }
            self.capture_context(session, view.id())?;
            ui.open_popup(&name);
        }
        let _menu_style = context_menu_style(ui);
        if let Some(_popup) = ui.begin_popup(&name) {
            if let Some(menu) = self.viewer_menus.get(&view.id()) {
                menu.draw(ui, requests);
                ui.separator();
            }
            if let Some(activity) = Self::document_lsp_activity(session, view.document_id()) {
                ui.text_disabled(activity);
            }
            let ready = session
                .lsp()
                .and_then(|pool| pool.client_for_document(view.document_id()))
                .is_some_and(|client| client.borrow().is_initialized());
            if let Some(action) = text_lsp_actions(ui, ready) {
                self.request_lsp(session, view.id(), action, None)?;
            }
            if !ready && ui.menu_item("Language Servers…") {
                requests.push(HostRequest::ShowPanelIn {
                    panel_type: LSP_DASHBOARD_PANEL_TYPE.into(),
                    document: None,
                    state: Value::Null,
                    action: None,
                    target: PanelTarget::LastFocused,
                });
            }
            ui.separator();
            self.draw_extension_menu(ui, view.id(), session, requests)?;
            let minimap = view.minimap_enabled(config.options.minimap_enabled);
            if ui.menu_item_enabled_selected_no_shortcut("Show Minimap", minimap, true) {
                view.set_minimap_enabled(!minimap);
            }
            if ui.menu_item_enabled_selected_no_shortcut("Reset Zoom", false, view.zoom() != 1.0) {
                view.reset_zoom();
            }
            ui.separator();
            let mutation_scope = ui.begin_disabled_with_cond(view.read_only());
            if ui.menu_item("Undo") && !view.read_only() {
                session.with_commands(view.id(), |commands| commands.undo())?;
            }
            if ui.menu_item("Redo") && !view.read_only() {
                session.with_commands(view.id(), |commands| commands.redo())?;
            }
            ui.separator();
            if ui.menu_item("Cut") && !view.read_only() {
                let bytes = session.with_commands(view.id(), |commands| commands.cut())?;
                set_clipboard(ui, &bytes);
            }
            drop(mutation_scope);
            if ui.menu_item("Copy") {
                let bytes = session.with_commands(view.id(), |commands| commands.copy())?;
                set_clipboard(ui, &bytes);
            }
            let mutation_scope = ui.begin_disabled_with_cond(view.read_only());
            if ui.menu_item("Paste")
                && !view.read_only()
                && let Some(text) = get_clipboard(ui)
            {
                session.with_commands(view.id(), |commands| commands.paste(text.as_bytes()))?;
            }
            drop(mutation_scope);
            if ui.menu_item("Select All") {
                session.with_commands(view.id(), |commands| commands.select_all())?;
            }
        }
        Ok(())
    }
    pub fn request_lsp(
        &mut self,
        session: &mut EditorSession,
        view: ViewId,
        kind: ContextLspAction,
        position: Option<(i32, i32)>,
    ) -> io::Result<()> {
        let document = session
            .document_for_view(view)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Editor view is detached"))?;
        if let Some(client) = session
            .lsp()
            .and_then(|pool| pool.client_for_document(document))
        {
            let snapshot = session.snapshot(document)?;
            if let Some(origin) = session
                .lsp()
                .and_then(|pool| pool.request_origin(document, view, snapshot.generation, 0))
            {
                let requested = session.with_view(view, |editor| {
                    if let Some((row, column)) = position {
                        self.lsp_ui.request_at_position(
                            kind,
                            &mut client.borrow_mut(),
                            editor,
                            origin,
                            row,
                            column,
                        )
                    } else {
                        self.lsp_ui
                            .request_at(kind, &mut client.borrow_mut(), editor, origin)
                    }
                })?;
                if requested && kind != ContextLspAction::Symbol {
                    self.pending_navigation = Some(kind);
                }
            }
        }
        Ok(())
    }
    pub fn tick(
        &mut self,
        session: &mut EditorSession,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        if self.restore_project_check {
            self.restore_project_check = false;
            if let Some(config) = &self.project_check {
                session.configure_project_check(config.clone());
            } else {
                session.reset_project_check();
            }
            self.check_json.clear();
            self.check_error = None;
        }
        self.retain_requests(session);
        let Some(kind) = self.pending_navigation else {
            return Ok(());
        };
        let Some(results) = self.lsp_ui.navigation_results(kind) else {
            self.pending_navigation = None;
            return Ok(());
        };
        if results.origin.is_none() {
            self.pending_navigation = None;
            return Ok(());
        }
        if kind == ContextLspAction::Definition && results.pending {
            return Ok(());
        }
        self.pending_navigation = None;
        if kind == ContextLspAction::Definition
            && let Some(action) = self.lsp_ui.take_single_definition()
        {
            self.open_lsp_action(session, action, requests)?;
        } else {
            self.navigation_kind = kind;
            requests.push(HostRequest::ShowPanel {
                panel_type: REFERENCES_PANEL_TYPE.into(),
                document: None,
                state: Value::Null,
                action: None,
            });
        }
        Ok(())
    }
    fn draw_diagnostics(&mut self, session: &mut EditorSession, ui: &Ui) -> Option<LspAction> {
        let snapshot = session.project_diagnostics();
        if ui.button("Run Check") {
            session.request_project_check();
        }
        ui.same_line();
        if ui.button("Cancel") && snapshot.checking {
            session.cancel_project_check();
        }
        ui.same_line();
        let mut config = session.project_check_config().clone();
        if ui.checkbox("Automatic checks", &mut config.automatic) {
            session.configure_project_check(config.clone());
            self.project_check = Some(config);
        }
        ui.text_wrapped(&snapshot.status);
        ui.text_disabled(format!(
            "{} errors · {} warnings · {}",
            snapshot.errors,
            snapshot.warnings,
            if snapshot.checking {
                "checking"
            } else if snapshot.stale {
                "stale / unsaved changes"
            } else if snapshot.complete {
                "project check complete"
            } else {
                "partial coverage"
            }
        ));
        if ui.collapsing_header(
            "Project Check Configuration",
            dear_imgui_rs::TreeNodeFlags::empty(),
        ) {
            if self.check_json.is_empty() {
                self.check_json =
                    serde_json::to_string_pretty(&session.project_check_config().to_json())
                        .unwrap();
            }
            ui.text_wrapped("Executable and arguments run in the project. Formats: cargo-json or lsp-jsonl (publishDiagnostics objects, one per line).");
            ui.input_text_multiline(
                "##check-config",
                &mut self.check_json,
                [
                    ui.content_region_avail()[0].max(1.0),
                    ui.text_line_height() * 8.0,
                ],
            )
            .build();
            if ui.button("Apply Check Configuration") {
                match serde_json::from_str::<Value>(&self.check_json).ok().and_then(|value| bed_document_session::ProjectCheckConfig::from_json(&value)) {
                    Some(config) if !config.program.trim().is_empty() => { session.configure_project_check(config.clone()); self.project_check = Some(config); self.check_error = None; },
                    _ => self.check_error = Some("Enter a program, arguments array, directory, supported format, and automatic flag.".into()),
                }
            }
            if let Some(error) = &self.check_error {
                ui.text_wrapped(error);
            }
        }
        ui.separator();
        ui.input_text("Search", &mut self.diagnostic_search).build();
        ui.checkbox("Errors", &mut self.diagnostic_errors);
        ui.same_line();
        ui.checkbox("Warnings", &mut self.diagnostic_warnings);
        ui.same_line();
        ui.checkbox("Other", &mut self.diagnostic_other);
        let query = self.diagnostic_search.to_lowercase();
        let mut action = None;
        let mut displayed = 0;
        for (path, items) in snapshot.by_path {
            let visible: Vec<_> = items
                .into_iter()
                .filter(|item| {
                    let severity = match item.severity {
                        1 => self.diagnostic_errors,
                        2 => self.diagnostic_warnings,
                        _ => self.diagnostic_other,
                    };
                    severity
                        && (query.is_empty()
                            || path.to_lowercase().contains(&query)
                            || item.message.to_lowercase().contains(&query))
                })
                .collect();
            if visible.is_empty() {
                continue;
            }
            let _id = ui.push_id(&path);
            ui.text(&path);
            for item in visible {
                displayed += 1;
                let label = match item.severity {
                    1 => "Error",
                    2 => "Warning",
                    _ => "Info",
                };
                if ui.selectable(format!(
                    "{}:{}  {}: {}",
                    item.start_line + 1,
                    item.start_character + 1,
                    label,
                    item.message
                )) {
                    action = Some(LspAction::OpenLocation(LspLocation {
                        file: path.clone(),
                        line: item.start_line,
                        character: item.start_character,
                    }));
                }
            }
        }
        if displayed == 0 {
            ui.text_disabled(if snapshot.errors + snapshot.warnings > 0 {
                "No diagnostics match the filters"
            } else {
                "No reported diagnostics"
            });
        }
        action
    }
    pub fn open_lsp_action(
        &mut self,
        session: &mut EditorSession,
        action: LspAction,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        match action {
            LspAction::OpenConfig(path) => {
                if session.is_remote() {
                    return Err(io::Error::other(format!(
                        "Language server configuration is local to this computer: {}. Open it in a local workspace, then reconnect.",
                        path.display()
                    )));
                }
                requests.push(HostRequest::OpenFileIn {
                    path: path.to_string_lossy().into_owned(),
                    viewer: None,
                    target: PanelTarget::LastFocused,
                });
            }
            LspAction::RestartServer(language) => {
                if let Some(pool) = session.lsp_mut() {
                    pool.retry_language(&language)?;
                }
            }
            LspAction::OpenLocation(location) => {
                let origin = self.lsp_ui.take_navigation_origin();
                if let Some(origin) = origin
                    && !session.accepts_lsp_origin(&origin)
                {
                    return Ok(());
                }
                self.actions
                    .push(EditorAction::Navigate { location, origin });
            }
        }
        Ok(())
    }
    fn draw_extension_menu(
        &mut self,
        ui: &Ui,
        view: ViewId,
        session: &mut EditorSession,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        let context = match self.contexts.get(&view) {
            Some(context) => context.clone(),
            None => self.capture_context(session, view)?,
        };
        self.menu_extensions
            .retain(|provider| provider.strong_count() > 0);
        let providers: Vec<_> = self
            .menu_extensions
            .iter()
            .filter_map(Weak::upgrade)
            .collect();
        for provider in providers {
            let commands = provider.borrow().commands(session, &context);
            for command in commands {
                let _id = ui.push_id(&command.id);
                if ui.menu_item_enabled_selected_no_shortcut(&command.label, false, command.enabled)
                {
                    provider
                        .borrow_mut()
                        .invoke(&command.id, &context, session, requests)?;
                }
            }
        }
        for command in self.menu_commands.get(&view).cloned().unwrap_or_default() {
            let _id = ui.push_id(&command.id);
            if ui.menu_item_enabled_selected_no_shortcut(command.label, false, command.enabled) {
                self.actions.push(EditorAction::Command {
                    id: command.id,
                    context: context.clone(),
                });
            }
        }
        Ok(())
    }
    pub(crate) fn draw_tool(
        &mut self,
        kind: ToolKind,
        ui: &Ui,
        session: &mut EditorSession,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        let action = match kind {
            ToolKind::Diagnostics => self.draw_diagnostics(session, ui),
            ToolKind::References => self.lsp_ui.render_navigation_body(ui, self.navigation_kind),
            ToolKind::Dashboard => {
                if let Some(pool) = session.lsp_mut() {
                    self.lsp_ui.render_workspace_dashboard_body(ui, pool)
                } else {
                    ui.text_disabled("Language servers start when a project is open");
                    None
                }
            }
        };
        if let Some(action) = action {
            self.open_lsp_action(session, action, requests)?;
        }
        Ok(())
    }
}
fn set_clipboard(ui: &Ui, bytes: &[u8]) {
    if let Ok(text) = CString::new(String::from_utf8_lossy(bytes).as_bytes()) {
        ui.with_bound_context(|| unsafe {
            sys::igSetClipboardText(text.as_ptr());
        });
    }
}
fn text_lsp_actions(ui: &Ui, ready: bool) -> Option<ContextLspAction> {
    let mut action = None;
    for (title, command) in [
        ("Go to Definition", ContextLspAction::Definition),
        ("Find References", ContextLspAction::References),
        ("Symbol Info", ContextLspAction::Symbol),
    ] {
        if ui.menu_item_enabled_selected_no_shortcut(title, false, ready) {
            action = Some(command);
        }
    }
    action
}
fn get_clipboard(ui: &Ui) -> Option<String> {
    ui.with_bound_context(|| unsafe {
        let text = sys::igGetClipboardText();
        (!text.is_null()).then(|| CStr::from_ptr(text).to_string_lossy().into_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_document_session::ByteEdit;
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions};

    struct ObservingMenu(Rc<RefCell<Vec<CommandContext>>>);
    impl EditorMenuExtension for ObservingMenu {
        fn commands(&self, _: &EditorSession, context: &CommandContext) -> Vec<EditorMenuCommand> {
            self.0.borrow_mut().push(context.clone());
            Vec::new()
        }
        fn invoke(
            &mut self,
            _: &str,
            _: &CommandContext,
            _: &mut EditorSession,
            _: &mut Vec<HostRequest>,
        ) -> io::Result<()> {
            unreachable!()
        }
    }

    #[test]
    fn menu_extensions_observe_captured_selection_once_and_unload_without_retention() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut session = EditorSession::new();
        let document = session.create_document(b"first value").unwrap();
        let view = EditorView::new(&mut session, document).unwrap();
        session
            .with_commands(view.id(), |commands| {
                commands.set_cursor(0, 5, true, CursorReveal::Ensure);
            })
            .unwrap();
        let mut runtime = EditorRuntime::default();
        let captured = runtime.capture_context(&session, view.id()).unwrap();
        let observations = Rc::new(RefCell::new(Vec::new()));
        let provider: MenuProvider = Rc::new(RefCell::new(ObservingMenu(observations.clone())));
        runtime.register_menu(&provider);
        runtime.register_menu(&provider);
        let weak = Rc::downgrade(&provider);
        session
            .apply_edits(
                document,
                session.document_revision(document).unwrap(),
                &[ByteEdit {
                    range: 0..5,
                    bytes: b"later".to_vec(),
                }],
            )
            .unwrap();
        assert_ne!(
            captured.revision,
            Some(session.document_revision(document).unwrap())
        );

        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut frame = |context: &mut Context, runtime: &mut EditorRuntime| {
            context.prepare_frame(FramePrepareOptions::new([400.0, 300.0], 1.0 / 60.0));
            let ui = context.frame();
            ui.window("Editor menu fixture")
                .position([0.0; 2], Condition::Always)
                .size([380.0, 280.0], Condition::Always)
                .build(|| {
                    runtime
                        .draw_extension_menu(ui, view.id(), &mut session, &mut Vec::new())
                        .unwrap();
                    ui.text("Concrete editor menu extension");
                });
            assert!(context.render_legacy().draw_data().total_vtx_count() > 0);
        };
        frame(&mut context, &mut runtime);
        frame(&mut context, &mut runtime);
        assert_eq!(
            observations.borrow().len(),
            2,
            "Duplicate registration must not duplicate contributions"
        );
        assert!(
            observations
                .borrow()
                .iter()
                .all(|context| context == &captured)
        );
        assert_eq!(captured.selection.as_ref().unwrap().text, "first");
        drop(provider);
        assert!(
            weak.upgrade().is_none(),
            "The editor must not keep an unloaded extension alive"
        );
        frame(&mut context, &mut runtime);
        assert_eq!(observations.borrow().len(), 2);
    }
}
