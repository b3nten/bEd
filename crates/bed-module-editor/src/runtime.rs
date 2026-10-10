//! Text editor interaction and language-server presentation owned by the editor feature.
use crate::{
    DIAGNOSTICS_PANEL_TYPE, EditorConfig, LSP_DASHBOARD_PANEL_TYPE, REFERENCES_PANEL_TYPE,
    lsp::lsp_ui::{ContextLspAction, LspAction, LspUi, LspView},
};
use bed_document_session::{DocumentId, EditorSession, ViewId};
use bed_editing::{editor_commands::CursorReveal, editor_events::Overlay};
use bed_editor_ui::{EditorView, HostAction, TextHit};
use bed_lsp::{lsp_locations::LspLocation, workspace_lsp::LspRequestOrigin};
use bed_ui::{
    presentation::{fit_text, readable_color, same_line_if_fits},
    util::popup_style::{context_menu_style, controls_style, tooltip_text},
};
use bed_workbench_api::{CommandContext, HostRequest, PanelTarget, SelectionContext};
use dear_imgui_rs::{MouseButton, StyleColor, StyleVar, TreeNodeFlags, Ui, sys};
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
                            caret_position: Some(frame.caret_position),
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
            let soft_wrap = view.soft_wrap(config.options.soft_wrap);
            if ui.menu_item_enabled_selected_no_shortcut("Word Wrap", soft_wrap, true) {
                view.set_soft_wrap(!soft_wrap);
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
        let _controls = controls_style(ui);
        let fs = ui.current_font_size();
        let _spacing = ui.push_style_var(StyleVar::ItemSpacing([fs * 0.5, fs * 0.3]));
        let _padding = ui.push_style_var(StyleVar::FramePadding([fs * 0.45, fs * 0.2]));
        let button_width = |label: &str| ui.calc_text_size(label)[0] + fs * 0.9;
        {
            let _disabled = ui.begin_disabled_with_cond(snapshot.checking);
            let _primary =
                ui.push_style_color(StyleColor::Button, ui.style_color(StyleColor::Header));
            if ui.button("Run Check") {
                session.request_project_check();
            }
        }
        same_line_if_fits(ui, button_width("Cancel"));
        {
            let _disabled = ui.begin_disabled_with_cond(!snapshot.checking);
            if ui.button("Cancel") {
                session.cancel_project_check();
            }
        }
        same_line_if_fits(
            ui,
            ui.frame_height() + ui.calc_text_size("Auto")[0] + fs * 0.5,
        );
        let mut config = session.project_check_config().clone();
        if ui.checkbox("Auto", &mut config.automatic) {
            session.configure_project_check(config.clone());
            self.project_check = Some(config);
        }
        if ui.is_item_hovered() {
            tooltip_text(ui, "Automatically check the project after changes");
        }
        same_line_if_fits(ui, button_width("Setup"));
        if ui.button("Setup") {
            ui.open_popup("diagnostics_check_setup");
        }
        {
            let _popup_style = context_menu_style(ui);
            ui.with_bound_context(|| unsafe {
                sys::igSetNextWindowSize(
                    [(fs * 34.0).min(ui.io().display_size()[0] - fs * 2.0), 0.0].into(),
                    sys::ImGuiCond_Appearing,
                );
            });
            if let Some(_popup) = ui.begin_popup("diagnostics_check_setup") {
                ui.text("Project check");
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
                    match serde_json::from_str::<Value>(&self.check_json)
                        .ok()
                        .and_then(|value| bed_document_session::ProjectCheckConfig::from_json(&value))
                    {
                        Some(config) if !config.program.trim().is_empty() => {
                            session.configure_project_check(config.clone());
                            self.project_check = Some(config);
                            self.check_error = None;
                        }
                        _ => self.check_error = Some(
                            "Enter a program, arguments array, directory, supported format, and automatic flag.".into(),
                        ),
                    }
                }
                if let Some(error) = &self.check_error {
                    let _color = ui.push_style_color(
                        StyleColor::Text,
                        readable_color(ui, [0.95, 0.35, 0.3, 1.0]),
                    );
                    ui.text_wrapped(error);
                }
            }
        }
        let status = if snapshot.checking {
            "Checking…"
        } else if snapshot.stale {
            "Unsaved changes"
        } else if snapshot.complete {
            "Check complete"
        } else {
            "Partial coverage"
        };
        ui.text_disabled(fit_text(
            ui,
            if snapshot.checking || snapshot.stale || snapshot.status.is_empty() {
                status
            } else {
                &snapshot.status
            },
            ui.content_region_avail()[0],
        ));
        if ui.is_item_hovered() {
            tooltip_text(ui, format!("{status}\n{}", snapshot.status));
        }
        ui.separator();
        let other = snapshot
            .by_path
            .values()
            .flatten()
            .filter(|item| item.severity != 1 && item.severity != 2)
            .count();
        let filters = [
            (
                format!("Errors {}", snapshot.errors),
                &mut self.diagnostic_errors,
                1,
            ),
            (
                format!("Warnings {}", snapshot.warnings),
                &mut self.diagnostic_warnings,
                2,
            ),
            (format!("Other {other}"), &mut self.diagnostic_other, 3),
        ];
        for (index, (label, enabled, severity)) in filters.into_iter().enumerate() {
            if index > 0 {
                same_line_if_fits(ui, button_width(&label));
            }
            let color = readable_color(
                ui,
                bed_editor_ui::views::diagnostic_style::severity_color(severity),
            );
            let _text = ui.push_style_color(StyleColor::Text, color);
            let _selected = ui.push_style_color(
                StyleColor::Button,
                if *enabled {
                    ui.style_color(StyleColor::Header)
                } else {
                    [0.0; 4]
                },
            );
            if ui.button(&label) {
                *enabled = !*enabled;
            }
            if ui.is_item_hovered() {
                tooltip_text(
                    ui,
                    if *enabled {
                        "Click to hide this severity"
                    } else {
                        "Click to show this severity"
                    },
                );
            }
        }
        ui.set_next_item_width(ui.content_region_avail()[0].max(1.0));
        ui.input_text("##diagnostic_search", &mut self.diagnostic_search)
            .hint("Filter by message or file…")
            .build();
        let query = self.diagnostic_search.to_lowercase();
        let mut action = None;
        let mut displayed = 0;
        let has_diagnostics = snapshot.by_path.values().any(|items| !items.is_empty());
        let root = session.options().project_root.as_deref();
        ui.child_window("diagnostic_results")
            .size([0.0, 0.0])
            .build(ui, || {
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
                    displayed += visible.len();
                    let file = std::path::Path::new(&path);
                    let relative = root
                        .and_then(|root| file.strip_prefix(root).ok())
                        .unwrap_or(file);
                    let heading = format!("{} · {}", relative.display(), visible.len());
                    let heading = fit_text(
                        ui,
                        &heading,
                        (ui.content_region_avail()[0] - fs * 2.0).max(1.0),
                    );
                    let open = ui.collapsing_header(
                        format!("{heading}###file"),
                        TreeNodeFlags::DEFAULT_OPEN,
                    );
                    if ui.is_item_hovered() {
                        tooltip_text(ui, &path);
                    }
                    if !open {
                        continue;
                    }
                    for (index, item) in visible.into_iter().enumerate() {
                        let _id = ui.push_id(index as i32);
                        let severity =
                            bed_editor_ui::views::diagnostic_style::severity_label(item.severity);
                        let color = readable_color(
                            ui,
                            bed_editor_ui::views::diagnostic_style::severity_color(item.severity),
                        );
                        let width = ui.content_region_avail()[0].max(1.0);
                        let inset = fs * 0.7;
                        let wrap_width = (width - inset * 2.0).max(1.0);
                        let message_height =
                            ui.calc_text_size_with_opts(&item.message, false, wrap_width)[1];
                        let height = fs + message_height + fs * 0.6;
                        if ui
                            .selectable_config("##diagnostic")
                            .size([width, height])
                            .build()
                        {
                            action = Some(LspAction::OpenLocation(LspLocation {
                                file: path.clone(),
                                line: item.start_line,
                                character: item.start_character,
                            }));
                        }
                        let min = ui.item_rect_min();
                        let max = ui.item_rect_max();
                        let _clip = ui.push_clip_rect(min, max, true);
                        let draw = ui.get_window_draw_list();
                        draw.add_line(
                            [min[0] + fs * 0.15, min[1] + fs * 0.2],
                            [min[0] + fs * 0.15, max[1] - fs * 0.2],
                            color,
                        )
                        .thickness(2.0)
                        .build();
                        draw.add_text([min[0] + inset, min[1]], color, severity);
                        let location =
                            format!("{}:{}", item.start_line + 1, item.start_character + 1);
                        draw.add_text(
                            [max[0] - inset - ui.calc_text_size(&location)[0], min[1]],
                            ui.style_color(StyleColor::TextDisabled),
                            location,
                        );
                        draw.add_text_with_font(
                            ui.current_font(),
                            fs,
                            [min[0] + inset, min[1] + fs * 1.25],
                            ui.style_color(StyleColor::Text),
                            &item.message,
                            wrap_width,
                            None,
                        );
                    }
                }
                if displayed == 0 {
                    ui.dummy([0.0, fs * 0.6]);
                    ui.text_wrapped(if has_diagnostics {
                        "No diagnostics match the filters"
                    } else {
                        "No reported diagnostics"
                    });
                    let _muted = ui.push_style_color(
                        StyleColor::Text,
                        ui.style_color(StyleColor::TextDisabled),
                    );
                    ui.text_wrapped(if has_diagnostics {
                        "Try a different search or enable a severity above."
                    } else {
                        "Run a project check to refresh the results."
                    });
                }
            });
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

    #[cfg(unix)]
    #[test]
    fn diagnostics_wrap_without_horizontal_overflow_and_keep_source_navigation() {
        use bed_document_session::{CheckFormat, ProjectCheckConfig, SessionOptions};
        use std::{
            fs,
            time::{Duration, Instant},
        };

        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let root = std::env::temp_dir().join(format!("bed-diagnostic-ui-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let path = root.join("a_long_source_filename_é.rs");
        let uri = bed_lsp::lsp_uri::LspUri::file_uri_from_path(path.to_str().unwrap()).unwrap();
        let report = serde_json::json!({"uri":uri.to_string(), "diagnostics":[{
            "range":{"start":{"line":12,"character":7},"end":{"line":12,"character":9}},
            "severity":1,
            "message":"A long diagnostic with ##literal text and Unicode é that wraps across several lines in a narrow panel. The complete message should remain readable and clickable."
        }]});
        fs::write(root.join("report.jsonl"), format!("{report}\n")).unwrap();
        let mut session = EditorSession::with_options(SessionOptions {
            project_root: Some(root.clone()),
            ..Default::default()
        })
        .unwrap();
        session.tick();
        session.configure_project_check(ProjectCheckConfig {
            program: "/bin/cat".into(),
            arguments: vec!["report.jsonl".into()],
            directory: ".".into(),
            format: CheckFormat::LspJsonLines,
            automatic: false,
        });
        session.request_project_check();
        let start = Instant::now();
        while session.project_diagnostics().errors == 0 {
            session.tick();
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "Fixture check did not finish: {}",
                session.project_diagnostics().status
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut runtime = EditorRuntime::default();
        let mut previous_size = [0.0; 2];
        let mut frame = |context: &mut Context, runtime: &mut EditorRuntime, size: [f32; 2]| {
            let settled = previous_size == size;
            previous_size = size;
            context.prepare_frame(FramePrepareOptions::new([1000.0, 700.0], 1.0 / 60.0));
            let ui = context.frame();
            let mut action = None;
            let mut point = [0.0; 2];
            let mut content_height = 0.0;
            ui.window("diagnostic_layout")
                .position([0.0; 2], Condition::Always)
                .size(size, Condition::Always)
                .flags(dear_imgui_rs::WindowFlags::NO_TITLE_BAR)
                .build(|| {
                    action = runtime.draw_diagnostics(&mut session, ui);
                    ui.with_bound_context(|| unsafe {
                        let parent = sys::igGetCurrentWindow();
                        if settled {
                            assert!((*parent).ScrollMax.x <= 4.0, "Toolbar overflow at {size:?}");
                        }
                        let child = *(*parent).DC.ChildWindows.Data;
                        if settled {
                            assert!((*child).ScrollMax.x <= 4.0, "Result overflow at {size:?}");
                        }
                        assert!(
                            (*child).Pos.y + (*child).Size.y <= (*parent).InnerRect.Max.y + 1.0
                        );
                        assert!(
                            (*child).Size.y >= 40.0,
                            "Results need usable space at {size:?}"
                        );
                        content_height = (*child).ContentSize.y;
                        point = [
                            (*child).DC.CursorStartPos.x + 30.0,
                            (*child).DC.CursorStartPos.y
                                + ui.frame_height_with_spacing()
                                + ui.current_font_size() * 1.5,
                        ];
                    });
                });
            assert!(context.render_legacy().draw_data().total_vtx_count() > 0);
            (action, point, content_height)
        };
        for size in [[240.0, 360.0], [320.0, 260.0], [800.0, 220.0]] {
            for _ in 0..3 {
                frame(&mut context, &mut runtime, size);
            }
        }
        let size = [300.0, 340.0];
        let (_, point, _) = frame(&mut context, &mut runtime, size);
        context.io_mut().add_mouse_pos_event(point);
        frame(&mut context, &mut runtime, size);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        frame(&mut context, &mut runtime, size);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        let (action, _, full_height) = frame(&mut context, &mut runtime, size);
        assert!(
            matches!(action, Some(LspAction::OpenLocation(location)) if location.file == path.to_string_lossy() && location.line == 12 && location.character == 7)
        );
        runtime.diagnostic_errors = false;
        frame(&mut context, &mut runtime, size);
        assert!(frame(&mut context, &mut runtime, size).2 < full_height);
        runtime.diagnostic_errors = true;
        runtime.diagnostic_search = "no matching message".into();
        frame(&mut context, &mut runtime, size);
        assert!(frame(&mut context, &mut runtime, size).2 < full_height);
        fs::remove_dir_all(root).unwrap();
    }

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
