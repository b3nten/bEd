//! Text editor interaction and language-server presentation owned by the editor feature.
use crate::{
    EditorConfig, LSP_DASHBOARD_PANEL_TYPE, REFERENCES_PANEL_TYPE,
    lsp::lsp_ui::{ContextLspAction, LspAction, LspUi, LspView},
};
use bedtime_api::{documents::CommandContext, features::HostAction, workbench::PanelTarget};
use bedtime_documents::{DocumentId, DocumentSession, SessionId, ViewId};
use bedtime_editing::{editing_events::Overlay, text_commands::CursorReveal};
use bedtime_editor_widgets::{EditorHostAction, EditorView, TextHit};
use bedtime_lsp::{
    jsonrpc::RpcId, lsp_client::LspClient, lsp_locations::LspLocation,
    workspace_lsp::LspRequestOrigin,
};
use bedtime_ui::{
    components::{context_menu_style, controls_style, tooltip_text},
    presentation::{fit_text, readable_color, same_line_if_fits},
};
use dear_imgui_rs::{MouseButton, StyleColor, StyleVar, TreeNodeFlags, Ui, sys};
use serde_json::Value;
use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::{CStr, CString},
    io,
    rc::Rc,
    time::{Duration, Instant},
};

/// Application operations produced by the editor. These retain their originating
/// view and request generation even if focus changes before the host handles them.
#[derive(Clone, Debug)]
pub enum EditorAction {
    Host {
        view: ViewId,
        action: EditorHostAction,
    },
    Navigate {
        location: LspLocation,
        origin: Option<LspRequestOrigin>,
    },
    Command {
        view: ViewId,
        id: bedtime_api::CommandId,
        context: CommandContext,
    },
}

#[derive(Clone, Debug)]
pub struct EditorMenuCommand {
    pub id: bedtime_api::CommandId,
    pub label: String,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationStage {
    WaitingForServer,
    Requesting,
    Retrying,
}

#[derive(Clone, Debug)]
pub struct NavigationActivity {
    pub view: ViewId,
    pub action: ContextLspAction,
    pub stage: NavigationStage,
    pub elapsed: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NavigationOutcomeKind {
    Empty,
    Failed(String),
    TimedOut,
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct NavigationOutcome {
    pub view: ViewId,
    pub action: ContextLspAction,
    pub kind: NavigationOutcomeKind,
}

#[derive(Clone, Debug)]
pub struct NavigationTiming {
    pub view: ViewId,
    pub action: ContextLspAction,
    pub started: Instant,
    pub first_sent: Option<Instant>,
    pub response_received: Option<Instant>,
}

struct NavigationIntent {
    session_id: SessionId,
    document: DocumentId,
    view: ViewId,
    revision: (u64, u64),
    action: ContextLspAction,
    position: (i32, i32),
    started: Instant,
    deadline: Instant,
    stage: NavigationStage,
    origin: Option<LspRequestOrigin>,
    rpc: Option<(Rc<RefCell<LspClient>>, RpcId)>,
    retries: usize,
    retry_at: Option<Instant>,
}

impl NavigationIntent {
    fn is_current(&self, session: &DocumentSession) -> bool {
        self.session_id == session.session_id()
            && session.document_for_view(self.view) == Some(self.document)
            && session.document_revision(self.document).ok() == Some(self.revision)
            && self
                .origin
                .is_none_or(|origin| session.accepts_lsp_origin(&origin))
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ToolKind {
    Diagnostics,
    References,
    Dashboard,
}

impl ToolKind {
    pub fn title(self) -> &'static str {
        match self {
            Self::Diagnostics => "Diagnostics",
            Self::References => "References",
            Self::Dashboard => "Language Servers",
        }
    }
}

pub struct EditorRuntime {
    pub(crate) completion: crate::lsp::completion::Completion,
    language_actions: crate::lsp::editor_actions::EditorActions,
    language_errors: Vec<String>,
    lsp_ui: LspUi,
    navigation_kind: ContextLspAction,
    pending_navigation: Option<NavigationIntent>,
    navigation_outcome: Option<NavigationOutcome>,
    navigation_timing: Option<NavigationTiming>,
    contexts: HashMap<ViewId, CommandContext>,
    menu_commands: HashMap<ViewId, Vec<EditorMenuCommand>>,
    viewer_menus: HashMap<ViewId, bedtime_api::documents::ViewerMenu>,
    actions: Vec<EditorAction>,
    overlays: HashMap<ViewId, Overlay>,
    historical_contexts: HashMap<ViewId, Vec<u8>>,
    pub(crate) project_check: Option<bedtime_documents::ProjectCheckConfig>,
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
            completion: Default::default(),
            language_actions: Default::default(),
            language_errors: Vec::new(),
            lsp_ui: LspUi::default(),
            navigation_kind: ContextLspAction::References,
            pending_navigation: None,
            navigation_outcome: None,
            navigation_timing: None,
            contexts: HashMap::new(),
            menu_commands: HashMap::new(),
            viewer_menus: HashMap::new(),
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
    pub fn request_lsp_command(
        &mut self,
        session: &mut DocumentSession,
        view: ViewId,
        command: &str,
    ) -> io::Result<()> {
        if command == "bedtime.editor.completion" {
            self.request_completion(session, view)
        } else {
            self.completion.cancel();
            self.language_actions.request(session, view, command)
        }
    }

    pub(crate) fn language_action_blocks_view(&self, view: ViewId) -> bool {
        self.language_actions.blocks_view(view)
    }

    pub(crate) fn completion_visible_for(&self, view: ViewId) -> bool {
        self.completion.visible_for(view)
    }

    pub fn request_completion(
        &mut self,
        session: &mut DocumentSession,
        view: ViewId,
    ) -> io::Result<()> {
        self.completion.request(session, view, None, false)
    }

    pub(crate) fn before_draw(
        &mut self,
        ui: &Ui,
        view: &mut EditorView,
        session: &mut DocumentSession,
        config: &EditorConfig,
    ) -> io::Result<()> {
        let mut settings = config.lsp.clone();
        settings.shortcuts = config.options.shortcuts.clone();
        let mut blocked =
            config.options.block_input || self.language_actions.blocks_view(view.id());
        if ui.is_window_focused_with_flags(dear_imgui_rs::FocusedFlags::CHILD_WINDOWS)
            && !blocked
            && !ui.io().want_text_input()
            && !view.find_visible()
            && !view.line_jump_visible()
        {
            for (command, _) in crate::lsp::editor_actions::COMMANDS.into_iter().skip(1) {
                if settings.shortcuts.take(ui, command) {
                    self.completion.cancel();
                    if let Err(error) = self.language_actions.request(session, view.id(), command) {
                        self.language_errors.push(error.to_string());
                    }
                }
            }
            blocked |= self.language_actions.blocks_view(view.id());
        }
        self.completion
            .before_draw(ui, view, session, &settings, blocked)
    }

    pub fn navigation_activity(&self) -> Option<NavigationActivity> {
        self.pending_navigation
            .as_ref()
            .map(|intent| NavigationActivity {
                view: intent.view,
                action: intent.action,
                stage: intent.stage,
                elapsed: intent.started.elapsed(),
            })
    }

    pub fn navigation_outcome(&self) -> Option<&NavigationOutcome> {
        self.navigation_outcome.as_ref()
    }

    pub fn navigation_timing(&self) -> Option<&NavigationTiming> {
        self.navigation_timing.as_ref()
    }

    fn finish_navigation(&mut self, mut intent: NavigationIntent, outcome: NavigationOutcomeKind) {
        // Invalidate the callback before canceling the RPC: cancellation delivers
        // synchronously, and an obsolete reply must never become a new result.
        self.lsp_ui.cancel_navigation(intent.action);
        if let Some((client, id)) = intent.rpc.take() {
            let _ = client.borrow_mut().cancel_request(&id);
        }
        self.navigation_outcome = Some(NavigationOutcome {
            view: intent.view,
            action: intent.action,
            kind: outcome,
        });
    }

    pub fn take_actions(&mut self) -> Vec<EditorAction> {
        std::mem::take(&mut self.actions)
    }

    pub(crate) fn drain_requests(
        &mut self,
        session: &DocumentSession,
    ) -> Vec<(Option<ViewId>, HostAction)> {
        self.take_actions()
            .into_iter()
            .filter_map(|action| {
                Some(match action {
                    EditorAction::Host { view, action } => {
                        let request = match action {
                            EditorHostAction::Open => HostAction::OpenFileDialog { viewer: None },
                            EditorHostAction::Save => HostAction::Save {
                                document: session.document_for_view(view)?,
                            },
                            EditorHostAction::SaveAs => HostAction::SaveAsDialog {
                                document: session.document_for_view(view)?,
                            },
                            EditorHostAction::Undo => HostAction::Undo {
                                document: session.document_for_view(view)?,
                            },
                            EditorHostAction::Redo => HostAction::Redo {
                                document: session.document_for_view(view)?,
                            },
                        };
                        (Some(view), request)
                    }
                    EditorAction::Command { view, id, context } => (
                        Some(view),
                        HostAction::Command {
                            command: id.into(),
                            context,
                        },
                    ),
                    EditorAction::Navigate { location, origin } => (
                        origin.map(|origin| origin.view_id),
                        HostAction::NavigateLocation {
                            path: location.file,
                            row: location.line,
                            utf16_column: location.character,
                            origin,
                        },
                    ),
                })
            })
            .collect()
    }

    /// Captured registry commands drawn on this view’s context menu.
    pub fn set_menu_commands(&mut self, view: ViewId, commands: Vec<EditorMenuCommand>) {
        self.menu_commands.insert(view, commands);
    }

    pub fn set_viewer_menu(
        &mut self,
        view: ViewId,
        menu: Option<bedtime_api::documents::ViewerMenu>,
    ) {
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
        self.language_actions.forget_view(view);
        self.completion.forget_view(view);
        if self
            .pending_navigation
            .as_ref()
            .is_some_and(|intent| intent.view == view)
            && let Some(intent) = self.pending_navigation.take()
        {
            self.finish_navigation(intent, NavigationOutcomeKind::Cancelled);
        }
        self.contexts.remove(&view);
        self.menu_commands.remove(&view);
        self.viewer_menus.remove(&view);
        self.overlays.remove(&view);
        self.historical_contexts.remove(&view);
    }

    pub fn cancel_requests(&mut self) {
        self.language_actions.cancel();
        self.completion.cancel();
        if let Some(intent) = self.pending_navigation.take() {
            self.finish_navigation(intent, NavigationOutcomeKind::Cancelled);
        }
        self.lsp_ui.cancel_requests();
        self.actions.clear();
    }

    pub fn retain_requests(&mut self, session: &DocumentSession) {
        self.completion.retain(session);
        if self
            .pending_navigation
            .as_ref()
            .is_some_and(|intent| !intent.is_current(session))
            && let Some(intent) = self.pending_navigation.take()
        {
            self.finish_navigation(intent, NavigationOutcomeKind::Cancelled);
        }
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
        session: &DocumentSession,
        view: ViewId,
    ) -> io::Result<CommandContext> {
        let context = Self::selection_context(session, view)?;
        self.contexts.insert(view, context.clone());
        Ok(context)
    }

    pub fn selection_context(
        session: &DocumentSession,
        view: ViewId,
    ) -> io::Result<CommandContext> {
        bedtime_api::documents::selection_context(session, view)
    }

    pub(crate) fn after_draw(
        &mut self,
        ui: &Ui,
        view: &mut EditorView,
        response: bedtime_editor_widgets::ViewResponse,
        session: &mut DocumentSession,
        config: &EditorConfig,
        requests: &mut Vec<HostAction>,
    ) -> io::Result<()> {
        for message in std::mem::take(&mut self.language_errors) {
            requests.push(HostAction::Notify { message });
        }
        self.language_actions.render(ui, view, session, requests)?;
        self.completion.observe(&response, session, &config.lsp)?;
        self.completion
            .render(ui, view, session, config.navigation_animations)?;
        for (view, command) in self.completion.take_commands() {
            if let Err(error) = self
                .language_actions
                .execute_completion_command(session, view, command)
            {
                requests.push(HostAction::Notify {
                    message: error.to_string(),
                });
            }
        }
        if let Some(message) = self.completion.take_error() {
            requests.push(HostAction::Notify { message });
        }
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
        if response.focused
            && !config.options.block_input
            && !session.view_snapshot(view.id())?.block_input
            && !ui.is_any_item_active()
        {
            for (key, action) in [
                ("bedtime.editor.definition", ContextLspAction::Definition),
                ("bedtime.editor.references", ContextLspAction::References),
                ("bedtime.editor.symbol_info", ContextLspAction::Symbol),
            ] {
                if config.lsp.shortcuts.take(ui, key) {
                    self.request_lsp(session, view.id(), action, None)?;
                }
            }
        }
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
            if !self.completion.visible_for(view_id)
                && !self.language_actions.blocks_view(view_id)
                && (caret_here
                    || (!self.lsp_ui.symbol_at_caret()
                        && (popup_here || (hover_here && !over_symbol_popup))))
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

    pub fn document_lsp_activity(
        session: &DocumentSession,
        document: DocumentId,
    ) -> Option<String> {
        let client = session.lsp()?.client_for_document(document)?;
        let client = client.borrow();
        if !client.is_process_started() {
            return None;
        }
        let server = client.current_server();
        if let Some(progress) = client.progress().into_iter().find(|job| !job.finished) {
            let mut text = format!("{server}: {}", progress.title);
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
        (!client.is_initialized()).then(|| format!("{server}: starting language server…"))
    }

    fn text_context_menu(
        &mut self,
        ui: &Ui,
        view: &mut EditorView,
        session: &mut DocumentSession,
        config: &EditorConfig,
        requests: &mut Vec<HostAction>,
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
            if ui.menu_item("Copy removed line")
                && let Some(bytes) = self.historical_contexts.get(&view.id())
            {
                set_clipboard(ui, bytes);
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
        let _menu_style = context_menu_style(ui, &bedtime_ui::UiFrameContext::for_ui(ui));
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
            if let Some(action) = text_lsp_actions(ui, session.lsp().is_some(), ready) {
                self.request_lsp(session, view.id(), action, None)?;
            }
            if ready {
                for (command, label) in crate::lsp::editor_actions::COMMANDS {
                    if ui.menu_item_enabled_selected_no_shortcut(label, false, !view.read_only())
                        && let Err(error) = self.request_lsp_command(session, view.id(), command)
                    {
                        requests.push(HostAction::Notify {
                            message: error.to_string(),
                        });
                    }
                }
            }
            if !ready && ui.menu_item("Language Servers…") {
                requests.push(HostAction::ShowPanelIn {
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
            if view.folding_enabled() {
                let folds = session.view_snapshot(view.id())?;
                let available = !folds.folds.ranges().is_empty();
                if ui.menu_item_enabled_selected_no_shortcut("Toggle Fold", false, available) {
                    session.with_commands(view.id(), |commands| {
                        commands.toggle_fold(folds.row);
                    })?;
                }
                if ui.menu_item_enabled_selected_no_shortcut("Fold All", false, available) {
                    session.with_commands(view.id(), |commands| commands.fold_all())?;
                }
                if ui.menu_item_enabled_selected_no_shortcut(
                    "Unfold All",
                    false,
                    !folds.folds.collapsed_ranges().is_empty(),
                ) {
                    session.with_commands(view.id(), |commands| commands.unfold_all())?;
                }
            }
            if ui.menu_item_enabled_selected_no_shortcut("Reset Zoom", false, view.zoom() != 1.0) {
                view.reset_zoom();
            }
            ui.separator();
            let mutation_scope = ui.begin_disabled_with_cond(view.read_only());
            if ui.menu_item("Undo") && !view.read_only() {
                session.undo_document(view.document_id())?;
            }
            if ui.menu_item("Redo") && !view.read_only() {
                session.redo_document(view.document_id())?;
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
        session: &mut DocumentSession,
        view: ViewId,
        kind: ContextLspAction,
        position: Option<(i32, i32)>,
    ) -> io::Result<()> {
        let document = session
            .document_for_view(view)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Editor view is detached"))?;
        let snapshot = session.snapshot(document)?;
        let origin = session
            .lsp()
            .and_then(|pool| pool.request_origin(document, view, snapshot.generation, 0));
        if kind == ContextLspAction::Symbol {
            if let (Some(origin), Some(client)) = (
                origin,
                session
                    .lsp()
                    .and_then(|pool| pool.client_for_document(document)),
            ) {
                session.with_view(view, |editor| {
                    self.lsp_ui
                        .request_at(kind, &mut client.borrow_mut(), editor, origin)
                })?;
            }
            return Ok(());
        }
        if let Some(intent) = self.pending_navigation.take() {
            self.finish_navigation(intent, NavigationOutcomeKind::Cancelled);
        }
        self.lsp_ui.cancel_navigation(kind);
        self.navigation_outcome = None;
        let position = position
            .unwrap_or(session.with_view(view, |editor| (editor.view.row, editor.view.column))?);
        let timeout = session
            .lsp()
            .and_then(|pool| {
                let language = pool
                    .config()
                    .detect_language_with_content(&snapshot.path, &snapshot.bytes)?;
                let server = pool.config().server(language.language_server.as_deref()?)?;
                Some(Duration::from_secs(server.timeout_secs))
            })
            .unwrap_or(Duration::from_secs(20));
        let now = Instant::now();
        self.navigation_timing = Some(NavigationTiming {
            view,
            action: kind,
            started: now,
            first_sent: None,
            response_received: None,
        });
        self.pending_navigation = Some(NavigationIntent {
            session_id: session.session_id(),
            document,
            view,
            revision: session.document_revision(document)?,
            action: kind,
            position,
            started: now,
            deadline: now + timeout,
            stage: NavigationStage::WaitingForServer,
            origin,
            rpc: None,
            retries: 0,
            retry_at: None,
        });
        self.advance_navigation(session, &mut Vec::new(), now)?;
        Ok(())
    }

    pub fn tick(
        &mut self,
        session: &mut DocumentSession,
        requests: &mut Vec<HostAction>,
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
        self.language_actions.tick(session, requests)?;
        self.advance_navigation(session, requests, Instant::now())
    }

    fn advance_navigation(
        &mut self,
        session: &mut DocumentSession,
        requests: &mut Vec<HostAction>,
        now: Instant,
    ) -> io::Result<()> {
        let Some(mut intent) = self.pending_navigation.take() else {
            return Ok(());
        };
        if !intent.is_current(session) {
            self.finish_navigation(intent, NavigationOutcomeKind::Cancelled);
            return Ok(());
        }
        if now >= intent.deadline {
            self.finish_navigation(intent, NavigationOutcomeKind::TimedOut);
            return Ok(());
        }
        if intent.rpc.is_some() {
            let results = self
                .lsp_ui
                .navigation_results(intent.action)
                .expect("navigation has a goto request");
            if results.pending {
                self.pending_navigation = Some(intent);
                return Ok(());
            }
            intent.rpc = None;
            if let Some(timing) = &mut self.navigation_timing {
                timing.response_received = results.received_at;
            }
            if let Some(error) = results.error {
                if error.is_retryable() && intent.retries < 2 {
                    let delay = Duration::from_millis(if intent.retries == 0 { 50 } else { 150 });
                    intent.retries += 1;
                    intent.retry_at = Some(now + delay);
                    intent.stage = NavigationStage::Retrying;
                    self.pending_navigation = Some(intent);
                } else {
                    self.finish_navigation(intent, NavigationOutcomeKind::Failed(error.message()));
                }
                return Ok(());
            }
            if results.origin.is_none() {
                self.finish_navigation(intent, NavigationOutcomeKind::Cancelled);
                return Ok(());
            }
            if results.locations.is_empty() {
                self.finish_navigation(intent, NavigationOutcomeKind::Empty);
                return Ok(());
            }
            self.navigation_outcome = None;
            if intent.action == ContextLspAction::Definition
                && let Some(action) = self.lsp_ui.take_single_definition()
            {
                self.open_lsp_action(session, action, requests)?;
            } else {
                self.navigation_kind = intent.action;
                requests.push(HostAction::ShowPanel {
                    panel_type: REFERENCES_PANEL_TYPE.into(),
                    document: None,
                    state: Value::Null,
                    action: None,
                });
            }
            return Ok(());
        }
        if intent.retry_at.is_some_and(|retry_at| now < retry_at) {
            self.pending_navigation = Some(intent);
            return Ok(());
        }
        let Some(pool) = session.lsp() else {
            self.finish_navigation(
                intent,
                NavigationOutcomeKind::Failed(
                    "Language servers are unavailable for this workspace".into(),
                ),
            );
            return Ok(());
        };
        let Some(client) = pool.client_for_document(intent.document) else {
            if pool.is_discovering() {
                self.pending_navigation = Some(intent);
            } else {
                let message = pool
                    .config_error()
                    .unwrap_or("No language server is available for this document")
                    .to_owned();
                self.finish_navigation(intent, NavigationOutcomeKind::Failed(message));
            }
            return Ok(());
        };
        if !client.borrow().is_process_started() {
            let message = client
                .borrow()
                .last_error()
                .unwrap_or("Language server is disconnected")
                .to_owned();
            self.finish_navigation(intent, NavigationOutcomeKind::Failed(message));
            return Ok(());
        }
        let origin = pool
            .request_origin(intent.document, intent.view, intent.revision.0, 0)
            .expect("a bound document has a request origin");
        intent.origin = Some(origin);
        if intent.stage == NavigationStage::WaitingForServer {
            intent.deadline = intent.started + client.borrow().request_timeout();
        }
        let path = session.with_view(intent.view, |editor| editor.state.path.clone())?;
        if !client.borrow().is_initialized() || !client.borrow().is_document_open(&path) {
            self.pending_navigation = Some(intent);
            return Ok(());
        }
        let (row, column) = intent.position;
        session.with_view(intent.view, |editor| {
            self.lsp_ui.request_at_position(
                intent.action,
                &mut client.borrow_mut(),
                editor,
                origin,
                row,
                column,
            )
        })?;
        let results = self
            .lsp_ui
            .navigation_results(intent.action)
            .expect("navigation has a goto request");
        intent.origin = results.origin;
        intent.retry_at = None;
        intent.stage = NavigationStage::Requesting;
        if let Some(id) = results.rpc_id {
            if let Some(timing) = &mut self.navigation_timing {
                timing.first_sent.get_or_insert(now);
            }
            intent.rpc = Some((client, id));
            self.pending_navigation = Some(intent);
        } else {
            let message = results.error.map_or_else(
                || "Language server rejected the navigation request".into(),
                |error| error.message(),
            );
            self.finish_navigation(intent, NavigationOutcomeKind::Failed(message));
        }
        Ok(())
    }

    fn draw_diagnostics(&mut self, session: &mut DocumentSession, ui: &Ui) -> Option<LspAction> {
        let snapshot = session.project_diagnostics();
        let _controls = controls_style(ui, &bedtime_ui::UiFrameContext::for_ui(ui));
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
            tooltip_text(
                ui,
                &bedtime_ui::UiFrameContext::for_ui(ui),
                "Automatically check the project after changes",
            );
        }
        same_line_if_fits(ui, button_width("Setup"));
        if ui.button("Setup") {
            ui.open_popup("diagnostics_check_setup");
        }
        {
            let _popup_style = context_menu_style(ui, &bedtime_ui::UiFrameContext::for_ui(ui));
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
                        .and_then(|value| bedtime_documents::ProjectCheckConfig::from_json(&value))
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
            "Results may be stale"
        } else if snapshot.complete {
            "Check complete"
        } else {
            "Partial coverage"
        };
        ui.text_disabled(fit_text(
            ui,
            if snapshot.checking || snapshot.status.is_empty() {
                status
            } else {
                &snapshot.status
            },
            ui.content_region_avail()[0],
        ));
        if ui.is_item_hovered() {
            tooltip_text(
                ui,
                &bedtime_ui::UiFrameContext::for_ui(ui),
                format!("{status}\n{}", snapshot.status),
            );
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
                bedtime_editor_widgets::views::diagnostic_style::severity_color(severity),
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
                    &bedtime_ui::UiFrameContext::for_ui(ui),
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
                        tooltip_text(ui, &bedtime_ui::UiFrameContext::for_ui(ui), &path);
                    }
                    if !open {
                        continue;
                    }
                    for (index, item) in visible.into_iter().enumerate() {
                        let _id = ui.push_id(index as i32);
                        let severity =
                            bedtime_editor_widgets::views::diagnostic_style::severity_label(
                                item.severity,
                            );
                        let color = readable_color(
                            ui,
                            bedtime_editor_widgets::views::diagnostic_style::severity_color(
                                item.severity,
                            ),
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
        session: &mut DocumentSession,
        action: LspAction,
        requests: &mut Vec<HostAction>,
    ) -> io::Result<()> {
        match action {
            LspAction::OpenConfig(path) => {
                if session.is_remote() {
                    return Err(io::Error::other(format!(
                        "Language server configuration is local to this computer: {}. Open it in a local workspace, then reconnect.",
                        path.display()
                    )));
                }
                requests.push(HostAction::OpenFileIn {
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
            LspAction::RestartInstance(instance) => {
                if session.lsp().is_some() {
                    session.retry_server(&instance)?;
                }
            }
            LspAction::InstallRustTooling(instance) => {
                if let Some(pool) = session.lsp_mut() {
                    pool.install_rust_tooling(&instance)?;
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
        session: &mut DocumentSession,
        _requests: &mut Vec<HostAction>,
    ) -> io::Result<()> {
        let context = match self.contexts.get(&view) {
            Some(context) => context.clone(),
            None => self.capture_context(session, view)?,
        };
        for command in self.menu_commands.get(&view).cloned().unwrap_or_default() {
            let _id = ui.push_id(command.id.as_str());
            if ui.menu_item_enabled_selected_no_shortcut(command.label, false, command.enabled) {
                self.actions.push(EditorAction::Command {
                    view,
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
        session: &mut DocumentSession,
        requests: &mut Vec<HostAction>,
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

fn text_lsp_actions(ui: &Ui, available: bool, ready: bool) -> Option<ContextLspAction> {
    let mut action = None;
    for (title, command) in [
        ("Go to Definition", ContextLspAction::Definition),
        ("Find References", ContextLspAction::References),
        ("Symbol Info", ContextLspAction::Symbol),
    ] {
        let enabled = if command == ContextLspAction::Symbol {
            ready
        } else {
            available
        };
        if ui.menu_item_enabled_selected_no_shortcut(title, false, enabled) {
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
    use bedtime_api::documents::SelectionContext;
    use bedtime_documents::ByteEdit;
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions};

    #[test]
    fn owned_actions_emit_once_in_order_with_captured_view_and_lsp_coordinates() {
        let mut runtime = EditorRuntime::default();
        let mut session = DocumentSession::new();
        let document = session.create_document(b"source").unwrap();
        let widget = EditorView::new(&mut session, document).unwrap();
        let view = widget.id();
        let context = CommandContext {
            path: Some("source.rs".into()),
            document: Some(document),
            revision: Some((7, 0)),
            selection: Some(SelectionContext {
                document,
                revision: (7, 0),
                ranges: vec![2..5, 9..12],
                text: "abc\nxyz".into(),
            }),
        };
        runtime.actions = vec![
            EditorAction::Host {
                view,
                action: EditorHostAction::Save,
            },
            EditorAction::Command {
                view,
                id: "debug.toggle_breakpoint".into(),
                context: context.clone(),
            },
            EditorAction::Navigate {
                location: LspLocation {
                    file: "definition.rs".into(),
                    line: 3,
                    character: 4,
                },
                origin: None,
            },
        ];
        let requests = runtime.drain_requests(&session);
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].0, Some(view));
        assert!(matches!(requests[0].1, HostAction::Save { document: saved } if saved == document));
        assert_eq!(requests[1].0, Some(view));
        let HostAction::Command {
            command,
            context: captured,
        } = &requests[1].1
        else {
            panic!("a menu command becomes a captured host request")
        };
        assert_eq!(command, "debug.toggle_breakpoint");
        assert_eq!(captured, &context);
        assert!(
            matches!(&requests[2], (None, HostAction::NavigateLocation {path,row:3,utf16_column:4,origin:None}) if path == "definition.rs")
        );
        assert!(
            runtime.drain_requests(&session).is_empty(),
            "every action is emitted once"
        );
    }

    #[test]
    fn dashboard_restart_only_replaces_the_selected_server_root() {
        use bedtime_documents::SessionOptions;
        use bedtime_lsp::workspace_lsp::ServerInstanceId;
        let directory = crate::test_support::TempDir::new();
        let config = directory.write(
            "lsp.json",
            br#"{"languages":[],"language_servers":{"shared":{"command":"/bed-no-server"}}}"#,
        );
        let mut session = DocumentSession::with_options(SessionOptions {
            project_root: Some(directory.root().to_owned()),
            lsp_config: Some(config),
            ..Default::default()
        })
        .unwrap();
        let first = ServerInstanceId {
            server: "shared".into(),
            root: directory.root().join("first"),
        };
        let second = ServerInstanceId {
            server: "shared".into(),
            root: directory.root().join("second"),
        };
        assert!(!session.retry_server(&first).unwrap());
        assert!(!session.retry_server(&second).unwrap());
        let generation = |session: &DocumentSession, instance: &ServerInstanceId| {
            session
                .lsp()
                .unwrap()
                .server_statuses()
                .into_iter()
                .find(|status| &status.instance == instance)
                .unwrap()
                .generation
        };
        let first_before = generation(&session, &first);
        let second_before = generation(&session, &second);
        EditorRuntime::default()
            .open_lsp_action(
                &mut session,
                LspAction::RestartInstance(second.clone()),
                &mut Vec::new(),
            )
            .unwrap();
        assert_eq!(generation(&session, &first), first_before);
        assert_ne!(generation(&session, &second), second_before);
    }

    #[cfg(unix)]
    #[test]
    fn dashboard_install_action_repairs_the_selected_rust_server() {
        use crate::rustup_fixture::RustupFixture;
        use bedtime_documents::SessionOptions;
        use bedtime_lsp::workspace_lsp::ServerInstanceId;
        use std::{
            thread,
            time::{Duration, Instant},
        };
        let fixture = RustupFixture::new();
        let root = fixture.root("first");
        let mut session = DocumentSession::with_options(SessionOptions {
            project_root: Some(root.clone()),
            lsp_config: Some(fixture.config.clone()),
            ..Default::default()
        })
        .unwrap();
        let instance = ServerInstanceId {
            server: "rust-analyzer".into(),
            root,
        };
        session.retry_server(&instance).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while session.lsp().unwrap().server_statuses()[0]
            .rust_issue
            .is_none()
        {
            session.tick();
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        let mut requests = Vec::new();
        EditorRuntime::default()
            .open_lsp_action(
                &mut session,
                LspAction::InstallRustTooling(instance.clone()),
                &mut requests,
            )
            .unwrap();
        assert!(requests.is_empty());
        let deadline = Instant::now() + Duration::from_secs(5);
        while !session.lsp().unwrap().server_statuses()[0].initialized {
            session.tick();
            assert!(
                Instant::now() < deadline,
                "{:?}",
                session.lsp().unwrap().server_statuses()
            );
            thread::sleep(Duration::from_millis(5));
        }
        assert!(fixture.dir.path("installed").exists());
    }

    #[cfg(unix)]
    #[test]
    fn diagnostics_wrap_without_horizontal_overflow_and_keep_source_navigation() {
        use bedtime_documents::{CheckFormat, ProjectCheckConfig, SessionOptions};
        use std::{
            fs,
            time::{Duration, Instant},
        };

        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let root = std::env::temp_dir().join(format!("bed-diagnostic-ui-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let path = root.join("a_long_source_filename_é.rs");
        let uri = bedtime_lsp::lsp_uri::LspUri::file_uri_from_path(path.to_str().unwrap()).unwrap();
        let report = serde_json::json!({"uri":uri.to_string(), "diagnostics":[{
            "range":{"start":{"line":12,"character":7},"end":{"line":12,"character":9}},
            "severity":1,
            "message":"A long diagnostic with ##literal text and Unicode é that wraps across several lines in a narrow panel. The complete message should remain readable and clickable."
        }]});
        fs::write(root.join("report.jsonl"), format!("{report}\n")).unwrap();
        let mut session = DocumentSession::with_options(SessionOptions {
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

    #[test]
    fn registry_menu_keeps_the_originating_selection_after_document_changes() {
        let mut session = DocumentSession::new();
        let document = session.create_document(b"first value").unwrap();
        let view = EditorView::new(&mut session, document).unwrap();
        session
            .with_commands(view.id(), |commands| {
                commands.set_cursor(0, 5, true, CursorReveal::Ensure)
            })
            .unwrap();
        let mut runtime = EditorRuntime::default();
        let captured = runtime.capture_context(&session, view.id()).unwrap();
        runtime.set_context(view.id(), captured.clone());
        runtime.set_menu_commands(
            view.id(),
            vec![EditorMenuCommand {
                id: "example.inspect".into(),
                label: "Inspect".into(),
                enabled: true,
            }],
        );
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
        assert_eq!(runtime.context(view.id()), Some(&captured));
        assert_ne!(
            captured.revision,
            Some(session.document_revision(document).unwrap())
        );
        assert_eq!(captured.selection.unwrap().text, "first");
        runtime.forget_view(view.id());
        assert!(runtime.context(view.id()).is_none());
        assert!(!runtime.menu_commands.contains_key(&view.id()));
    }
}
