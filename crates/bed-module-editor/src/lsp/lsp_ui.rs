//! Main-thread adapter for ned LSPClient::keybinds/render and dashboard.
use crate::{
    lsp::{
        lsp_dashboard::LspDashboard,
        lsp_goto::{Kind, LspGoto},
        lsp_symbol_info::LspSymbolInfo,
        lsp_uri_options::LspUriOptions,
    },
    presentation::LspPresentationOptions,
};
use bed_document_session::editor::Editor;
use bed_editor_ui::views::{
    hover_tooltip::TooltipArbiter, hover_trigger::Info, view_layout::ViewLayout,
};
use bed_lsp::{
    lsp_client::LspClient,
    lsp_locations::LspLocation,
    workspace_lsp::{LspRequestOrigin, WorkspaceLsp},
};
use dear_imgui_rs::Ui;
use std::path::PathBuf;

pub struct LspView<'a> {
    pub layout: &'a ViewLayout,
    pub hover_info: Info,
    pub hover_dismissed: bool,
    pub tooltip_arbiter: &'a TooltipArbiter,
    pub caret_visual_row: Option<usize>,
}
pub struct LspPane<'a> {
    pub editor: &'a Editor,
    pub view: LspView<'a>,
}
#[derive(Clone, Debug)]
pub enum LspAction {
    OpenLocation(LspLocation),
    OpenConfig(PathBuf),
    RestartServer(String),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextLspAction {
    Definition,
    References,
    Symbol,
}
#[derive(Clone, Debug)]
pub struct NavigationResults {
    pub origin: Option<LspRequestOrigin>,
    pub locations: Vec<LspLocation>,
    pub pending: bool,
}
pub struct LspUi {
    pub dashboard: LspDashboard,
    symbol_info: LspSymbolInfo,
    goto_definition: LspGoto,
    goto_references: LspGoto,
    uri_options: LspUriOptions,
    navigation_origin: Option<LspRequestOrigin>,
}
impl Default for LspUi {
    fn default() -> Self {
        Self {
            dashboard: LspDashboard::default(),
            symbol_info: LspSymbolInfo::default(),
            goto_definition: LspGoto::new(Kind::Definition),
            goto_references: LspGoto::new(Kind::References),
            uri_options: LspUriOptions::default(),
            navigation_origin: None,
        }
    }
}
impl LspUi {
    pub fn request_at(
        &mut self,
        action: ContextLspAction,
        client: &mut LspClient,
        editor: &Editor,
        origin: LspRequestOrigin,
    ) -> bool {
        if !client.is_initialized() || !client.is_process_started() {
            return false;
        }
        match action {
            ContextLspAction::Definition => {
                self.goto_definition.get_with_origin(client, editor, origin);
                self.goto_definition.show = false;
            }
            ContextLspAction::References => {
                self.goto_references.get_with_origin(client, editor, origin);
                self.goto_references.show = false;
            }
            ContextLspAction::Symbol => self.symbol_info.get_with_origin(client, editor, origin),
        }
        true
    }
    /// Request navigation at a clicked byte position without moving the view's
    /// cursor or changing its selections.
    pub fn request_at_position(
        &mut self,
        action: ContextLspAction,
        client: &mut LspClient,
        editor: &Editor,
        origin: LspRequestOrigin,
        row: i32,
        column: i32,
    ) -> bool {
        if !client.is_initialized() || !client.is_process_started() {
            return false;
        }
        let goto = match action {
            ContextLspAction::Definition => &mut self.goto_definition,
            ContextLspAction::References => &mut self.goto_references,
            ContextLspAction::Symbol => return false,
        };
        goto.get_with_origin_at_position(client, editor, origin, row, column);
        goto.show = false;
        true
    }
    pub fn navigation_results(&self, action: ContextLspAction) -> Option<NavigationResults> {
        let goto = match action {
            ContextLspAction::Definition => &self.goto_definition,
            ContextLspAction::References => &self.goto_references,
            ContextLspAction::Symbol => return None,
        };
        Some(NavigationResults {
            origin: goto.origin(),
            locations: goto.snapshot(),
            pending: goto.is_pending(),
        })
    }
    pub fn render_navigation_body(
        &mut self,
        ui: &Ui,
        action: ContextLspAction,
    ) -> Option<LspAction> {
        let goto = match action {
            ContextLspAction::Definition => &mut self.goto_definition,
            ContextLspAction::References => &mut self.goto_references,
            ContextLspAction::Symbol => return None,
        };
        // A docked result tab owns visibility; this request does not block the
        // editor or also create the legacy floating picker.
        goto.show = false;
        let selected =
            self.uri_options
                .render_body(ui, goto.title(), &goto.snapshot(), goto.is_pending());
        if selected.is_some() {
            self.navigation_origin = goto.origin();
        }
        selected.map(LspAction::OpenLocation)
    }
    pub fn render_workspace_dashboard_body(
        &mut self,
        ui: &Ui,
        workspace: &mut WorkspaceLsp,
    ) -> Option<LspAction> {
        let path = self.dashboard.render_workspace_body(ui, workspace);
        if let Some(language) = self.dashboard.take_restart_language() {
            return Some(LspAction::RestartServer(language));
        }
        path.map(LspAction::OpenConfig)
    }
    /// Accept the same primary modifiers as the editor's keyboard dispatcher.
    pub fn keybinds(
        &mut self,
        ui: &Ui,
        client: &mut LspClient,
        editor: &Editor,
        settings: &LspPresentationOptions,
    ) -> bool {
        self.keybinds_optional_origin(ui, client, editor, settings, None)
    }
    pub fn keybinds_with_origin(
        &mut self,
        ui: &Ui,
        client: &mut LspClient,
        editor: &Editor,
        settings: &LspPresentationOptions,
        origin: LspRequestOrigin,
    ) -> bool {
        self.keybinds_optional_origin(ui, client, editor, settings, Some(origin))
    }
    fn keybinds_optional_origin(
        &mut self,
        ui: &Ui,
        client: &mut LspClient,
        editor: &Editor,
        settings: &LspPresentationOptions,
        origin: Option<LspRequestOrigin>,
    ) -> bool {
        if !client.is_process_started() || !(ui.io().key_ctrl() || ui.io().key_super()) {
            return false;
        }
        let pressed = |action| {
            settings
                .action_key(action)
                .is_some_and(|key| ui.is_key_pressed_with_repeat(key, false))
        };
        let mut shortcut_pressed = false;
        if pressed("lsp_symbol_info") {
            if let Some(origin) = origin {
                self.symbol_info.get_with_origin(client, editor, origin);
            } else {
                self.symbol_info.get(client, editor);
            }
            shortcut_pressed = true;
        }
        if pressed("lsp_find_def") {
            if let Some(origin) = origin {
                self.goto_definition.get_with_origin(client, editor, origin);
            } else {
                self.goto_definition.get(client, editor);
            }
            shortcut_pressed = true;
        }
        if pressed("lsp_find_ref") {
            if let Some(origin) = origin {
                self.goto_references.get_with_origin(client, editor, origin);
            } else {
                self.goto_references.get(client, editor);
            }
            shortcut_pressed = true;
        }
        shortcut_pressed
    }
    pub fn symbol_at_caret(&self) -> bool {
        self.symbol_info.at_caret()
    }
    pub fn symbol_origin(&self) -> Option<LspRequestOrigin> {
        self.symbol_info.origin()
    }
    pub fn symbol_popup_contains(&self, mouse: [f32; 2]) -> bool {
        self.symbol_info.popup_contains(mouse)
    }
    pub fn set_mouse_origin(&mut self, origin: Option<LspRequestOrigin>) {
        self.symbol_info.set_mouse_origin(origin);
    }
    pub fn retain_requests(&mut self, mut valid: impl FnMut(&LspRequestOrigin) -> bool) {
        self.symbol_info.retain_request(&mut valid);
        self.goto_definition.retain_request(&mut valid);
        self.goto_references.retain_request(&mut valid);
    }
    pub fn take_navigation_origin(&mut self) -> Option<LspRequestOrigin> {
        self.navigation_origin.take()
    }
    /// Consume a completed, unambiguous definition while retaining the view
    /// that requested it for the host's generation checks and focus routing.
    pub fn take_single_definition(&mut self) -> Option<LspAction> {
        if self.goto_definition.is_pending() {
            return None;
        }
        let locations = self.goto_definition.snapshot();
        if locations.len() != 1 {
            return None;
        }
        self.navigation_origin = self.goto_definition.origin();
        self.goto_definition.cancel();
        Some(LspAction::OpenLocation(
            locations.into_iter().next().unwrap(),
        ))
    }
    pub fn is_overlay_visible(&self) -> bool {
        self.goto_definition.is_visible()
            || self.goto_references.is_visible()
            || self.dashboard.is_visible()
    }
    pub fn cancel_requests(&mut self) {
        self.symbol_info.cancel();
        self.goto_definition.cancel();
        self.goto_references.cancel();
    }
    pub fn render(
        &mut self,
        ui: &Ui,
        client: &mut LspClient,
        editor: &Editor,
        settings: &LspPresentationOptions,
        view: LspView<'_>,
    ) -> Option<LspAction> {
        self.symbol_info.render(ui, client, editor, &view);
        self.render_options(ui, client, settings, view.layout)
    }
    /// Render only symbol information for the supplied document/view. Docked
    /// workspaces render dashboard and navigation content in their own tabs.
    pub fn render_hover(
        &mut self,
        ui: &Ui,
        client: &mut LspClient,
        editor: &Editor,
        _settings: &LspPresentationOptions,
        view: LspView<'_>,
    ) {
        self.symbol_info.render(ui, client, editor, &view);
    }
    /// Split panes retain keyboard navigation on the focused document while
    /// mouse hover uses the document underneath the pointer, as in Workbench.
    pub fn render_panes(
        &mut self,
        ui: &Ui,
        client: &mut LspClient,
        settings: &LspPresentationOptions,
        active: LspPane<'_>,
        hovered: LspPane<'_>,
    ) -> Option<LspAction> {
        self.render_panes_inner(ui, client, settings, active, hovered, true)
    }
    pub fn render_panes_workspace(
        &mut self,
        ui: &Ui,
        client: &mut LspClient,
        settings: &LspPresentationOptions,
        active: LspPane<'_>,
        hovered: LspPane<'_>,
    ) -> Option<LspAction> {
        self.render_panes_inner(ui, client, settings, active, hovered, false)
    }
    fn render_panes_inner(
        &mut self,
        ui: &Ui,
        client: &mut LspClient,
        settings: &LspPresentationOptions,
        active: LspPane<'_>,
        hovered: LspPane<'_>,
        dashboard: bool,
    ) -> Option<LspAction> {
        let pane = if self.symbol_info.at_caret() {
            &active
        } else {
            &hovered
        };
        self.symbol_info.render(ui, client, pane.editor, &pane.view);
        self.render_options_inner(ui, client, settings, active.view.layout, dashboard)
    }
    fn render_options(
        &mut self,
        ui: &Ui,
        client: &mut LspClient,
        settings: &LspPresentationOptions,
        layout: &ViewLayout,
    ) -> Option<LspAction> {
        self.render_options_inner(ui, client, settings, layout, true)
    }
    pub fn render_workspace_dashboard(
        &mut self,
        ui: &Ui,
        workspace: &mut WorkspaceLsp,
        settings: &LspPresentationOptions,
    ) -> Option<LspAction> {
        let path = self.dashboard.render_workspace(ui, workspace, settings);
        if let Some(language) = self.dashboard.take_restart_language() {
            return Some(LspAction::RestartServer(language));
        }
        path.map(LspAction::OpenConfig)
    }
    fn render_options_inner(
        &mut self,
        ui: &Ui,
        client: &mut LspClient,
        settings: &LspPresentationOptions,
        layout: &ViewLayout,
        dashboard: bool,
    ) -> Option<LspAction> {
        let mut action = None;
        for goto in [&mut self.goto_definition, &mut self.goto_references] {
            if goto.show {
                let title = goto.title();
                let options = goto.snapshot();
                if let Some(location) =
                    self.uri_options
                        .render(ui, title, &options, &mut goto.show, settings, layout)
                {
                    self.navigation_origin = goto.origin();
                    action = Some(LspAction::OpenLocation(location));
                }
            }
        }
        if dashboard && let Some(path) = self.dashboard.render(ui, client, settings) {
            action = Some(LspAction::OpenConfig(path));
        }
        if dashboard && let Some(language) = self.dashboard.take_restart_language() {
            action = Some(LspAction::RestartServer(language));
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_document_session::EditorSession;
    use bed_editor_ui::{EditorView, EditorViewOptions};
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions, Key};
    #[test]
    fn native_dashboard_blocks_document_input_and_releases_after_escape_frame() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut session = EditorSession::new();
        let doc = session.create_document(b"buffer").unwrap();
        let mut view = EditorView::new(&mut session, doc).unwrap();
        let mut client = LspClient::new(PathBuf::from("/bed-no-config/lsp.json"));
        let mut lsp = LspUi::default();
        lsp.dashboard.set_show(true, &client);
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut draw = |context: &mut Context, lsp: &mut LspUi| {
            context.prepare_frame(FramePrepareOptions::new([1200.0, 800.0], 1.0 / 60.0));
            let ui = context.frame();
            let blocked = lsp.is_overlay_visible();
            ui.window("Document host")
                .size([600.0, 500.0], Condition::Always)
                .build(|| {
                    if !blocked {
                        session.request_focus(view.id()).unwrap();
                    }
                    view.draw(
                        ui,
                        &mut session,
                        &EditorViewOptions {
                            block_input: blocked,
                            ..Default::default()
                        },
                    )
                    .unwrap();
                });
            lsp.dashboard
                .render(ui, &mut client, &LspPresentationOptions::default());
            drop(context.render_legacy());
            (
                session.snapshot(doc).unwrap(),
                session.view_snapshot(view.id()).unwrap(),
            )
        };
        draw(&mut context, &mut lsp);
        context.io_mut().add_input_characters_utf8("X");
        context.io_mut().add_key_event(Key::Enter, true);
        let (doc, state) = draw(&mut context, &mut lsp);
        assert!(lsp.is_overlay_visible());
        assert!(state.block_input);
        assert_eq!(doc.bytes, b"buffer");
        context.io_mut().clear_input_keys();
        context.io_mut().add_key_event(Key::Escape, true);
        let (doc, state) = draw(&mut context, &mut lsp);
        assert!(!lsp.is_overlay_visible());
        assert!(state.block_input);
        assert_eq!(doc.bytes, b"buffer");
        let (_, state) = draw(&mut context, &mut lsp);
        assert!(!state.block_input);
    }
}
