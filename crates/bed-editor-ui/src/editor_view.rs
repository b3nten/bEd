//! Bed's custom document view inside the host's existing Dear ImGui frame.
//! Host-owned: containers, windows, context, backends, fonts, menus and effects.
//! Source attribution: LICENSE and NOTICE.
pub use crate::source_debug::{
    BreakpointStatus, SourceBreakpoint, SourceDebugAction, SourceDebugPresentation,
};
use crate::{
    editor_frame::EditorFrame,
    editor_input::{DefinitionRequest, EditorInput, HostAction},
};
use bed_document_session::editor_session::{
    ClosePolicy, DocumentId, EditorSession, ViewId, WorkspaceId,
};
use dear_imgui_rs::{ContextId, FocusedFlags, FontId, Key, StyleColor, StyleVar, Ui, WindowFlags};
use std::{io, rc::Rc};

#[derive(Clone, Debug)]
pub struct EditorViewOptions {
    pub extensions: crate::extensions::EditorExtensions,
    pub size: [f32; 2],
    pub font: Option<FontId>,
    pub background_color: Option<[f32; 4]>,
    pub rainbow_mode: bool,
    pub minimap_enabled: bool,
    pub line_jump_key: Option<Key>,
    pub block_input: bool,
    pub source_debug: Option<SourceDebugPresentation>,
    pub diff: Option<crate::DiffPresentation>,
    /// Allows navigation and copying while preventing document mutations.
    pub read_only: bool,
}

impl Default for EditorViewOptions {
    fn default() -> Self {
        Self {
            extensions: Default::default(),
            size: [0.0; 2],
            font: None,
            background_color: None,
            rainbow_mode: true,
            minimap_enabled: false,
            line_jump_key: Some(Key::Semicolon),
            block_input: false,
            source_debug: None,
            diff: None,
            read_only: false,
        }
    }
}
#[derive(Clone, Debug)]
pub struct ViewResponse {
    pub view: ViewId,
    pub document: DocumentId,
    pub version: i32,
    pub generation: u64,
    pub focused: bool,
    pub actions: Vec<HostAction>,
    pub source_actions: Vec<SourceDebugAction>,
    pub diff_actions: Vec<crate::DiffAction>,
    pub definition_request: Option<DefinitionRequest>,
}
/// Geometry and hover data for host menus and LSP presentation. Frame internals stay private.
pub struct ViewPresentation<'a> {
    pub layout: crate::views::view_layout::ViewLayout,
    pub hover_info: crate::views::hover_trigger::Info,
    pub hover_dismissed: bool,
    pub tooltip_arbiter: &'a crate::views::hover_tooltip::TooltipArbiter,
}
pub struct EditorView {
    id: ViewId,
    document: DocumentId,
    workspace: WorkspaceId,
    context: Option<ContextId>,
    // Stable allocation preserves upstream find/line-jump IDs while hosts move
    // widget handles between panels or reallocate their own tab registries.
    frame: Box<EditorFrame>,
    input: EditorInput,
    minimap_override: Option<bool>,
    zoom: f32,
    zoom_reset: Option<ZoomReset>,
    rendered_font: Option<(FontId, f32)>,
    _lifetime: Rc<()>,
}
struct ZoomReset {
    from: f32,
    elapsed: f32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextHit {
    Document { row: i32, column: i32 },
    Historical { old_row: usize, bytes: Vec<u8> },
    Control,
    None,
}
impl EditorView {
    pub fn new(session: &mut EditorSession, document: DocumentId) -> io::Result<Self> {
        let (id, token) = session.create_managed_view(document)?;
        let frame = session.with_view(id, |editor| Box::new(EditorFrame::new(editor)))?;
        Ok(Self {
            id,
            document,
            workspace: session.workspace_id(),
            context: None,
            frame,
            input: EditorInput::default(),
            minimap_override: None,
            zoom: 1.0,
            zoom_reset: None,
            rendered_font: None,
            _lifetime: token,
        })
    }
    pub fn id(&self) -> ViewId {
        self.id
    }
    pub fn document_id(&self) -> DocumentId {
        self.document
    }
    pub fn context_id(&self) -> Option<ContextId> {
        self.context
    }
    /// Animate explicit location jumps and zoom resets.
    /// Wheel, trackpad and ordinary caret scrolling always use native behavior.
    /// Enabled by default; disabling finishes active animations on the next draw.
    pub fn set_navigation_animations(&mut self, enabled: bool) {
        self.frame.navigation_animations = enabled;
    }
    /// Override this view's minimap for its lifetime without changing host settings.
    pub fn set_minimap_enabled(&mut self, enabled: bool) {
        self.minimap_override = Some(enabled);
    }
    pub fn minimap_enabled(&self, default: bool) -> bool {
        self.minimap_override.unwrap_or(default)
    }
    /// View-local magnification, limited to 60%–140%.
    /// Neither document state nor host settings change.
    pub fn zoom(&self) -> f32 {
        self.zoom
    }
    pub fn zoom_in(&mut self) {
        self.set_zoom(self.zoom + 0.1);
    }
    pub fn zoom_out(&mut self) {
        self.set_zoom(self.zoom - 0.1);
    }
    /// Ease back to the host's font size over 100 ms, unless animations are disabled.
    /// Keyboard and gesture zoom interrupt an active reset at its current scale.
    pub fn reset_zoom(&mut self) {
        if !self.frame.navigation_animations {
            self.set_zoom(1.0);
        } else if self.zoom != 1.0 && self.zoom_reset.is_none() {
            self.zoom_reset = Some(ZoomReset {
                from: self.zoom,
                elapsed: 0.0,
            });
        }
    }
    /// Apply a positive, finite gesture multiplier.
    pub fn zoom_by(&mut self, factor: f32) {
        assert!(factor.is_finite() && factor > 0.0);
        self.set_zoom(self.zoom * factor);
    }
    fn set_zoom(&mut self, zoom: f32) {
        self.zoom_reset = None;
        let zoom = zoom.clamp(0.6, 1.4);
        self.zoom = if (zoom - 1.0).abs() < 0.00001 {
            1.0
        } else {
            zoom
        };
    }
    pub fn draw(
        &mut self,
        ui: &Ui,
        session: &mut EditorSession,
        options: &EditorViewOptions,
    ) -> io::Result<ViewResponse> {
        self.check_session(session)?;
        if self.context.is_some_and(|id| id != ui.context_id()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "EditorView belongs to a different ImGui context generation",
            ));
        }
        ui.with_bound_context(|| {
            // Ui binds its live context. No pointer or native/GPU borrow escapes.
            if !unsafe { (*dear_imgui_rs::sys::igGetCurrentContext()).WithinFrameScope } {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Draw EditorView inside the host's open frame",
                ));
            }
            let source_revision = session.document_revision(self.document)?;
            let source_extension = options.extensions.source_debug(session, self.document)?;
            let git_extension = options.extensions.source_git(session, self.document)?;
            if let Some(reset) = &mut self.zoom_reset {
                reset.elapsed += ui.io().delta_time();
                let t = (reset.elapsed / 0.1).min(1.0);
                let eased = t * t * (3.0 - 2.0 * t);
                self.zoom = reset.from + (1.0 - reset.from) * eased;
                if t >= 1.0 || !self.frame.navigation_animations {
                    self.set_zoom(1.0);
                }
            }
            let _font = options.font.map(|font| ui.push_font(font));
            // PushFont takes an unscaled base size; current_font_size already
            // includes host and DPI scaling and would apply those twice.
            let base_size = unsafe { (*dear_imgui_rs::sys::igGetCurrentContext()).FontSizeBase };
            let zoom_size = base_size * self.zoom;
            let _zoom_font = ui.push_font_with_size(None, zoom_size);
            let _padding = ui.push_style_var(StyleVar::WindowPadding([0.0; 2]));
            let _border = ui.push_style_var(StyleVar::ChildBorderSize(0.0));
            self.frame.background_color = options
                .background_color
                .unwrap_or_else(|| ui.style_color(StyleColor::WindowBg));
            self.frame.rainbow_mode = options.rainbow_mode;
            self.frame.minimap_enabled = self.minimap_enabled(options.minimap_enabled);
            self.frame.line_jump_key = options.line_jump_key;
            self.frame.external_overlay = options.block_input;
            self.frame.read_only =
                options.read_only || session.with_document(self.document, |s| s.read_only)?;
            self.frame.diff = options.diff.clone();
            self.frame.source_git = git_extension
                .as_ref()
                .map(|(_, presentation)| presentation.clone());
            self.frame.source_debug = source_extension
                .as_ref()
                .map(|(_, presentation)| presentation.clone())
                .or_else(|| options.source_debug.clone());
            let mut response = None;
            ui.child_window(format!("##bed_view_{}", self.id.0))
                .size(options.size)
                .flags(
                    WindowFlags::NO_SCROLLBAR
                        | WindowFlags::NO_SCROLL_WITH_MOUSE
                        | WindowFlags::NO_NAV_INPUTS,
                )
                .build(ui, || {
                    self.rendered_font = Some((ui.current_font(), zoom_size));
                    response = Some((|| {
                        let mut response = session.with_view(self.id, |editor| {
                            let actions = self.frame.run(ui, editor, &mut self.input);
                            ViewResponse {
                                view: self.id,
                                document: self.document,
                                version: editor.state.version,
                                generation: editor.document_generation(),
                                focused: ui
                                    .is_window_focused_with_flags(FocusedFlags::CHILD_WINDOWS),
                                actions,
                                source_actions: self.frame.take_source_actions(),
                                diff_actions: self.frame.take_diff_actions(),
                                definition_request: self.input.take_definition_request(),
                            }
                        })?;
                        for action in &mut response.diff_actions {
                            action.revision = source_revision;
                        }
                        if session.document_revision(self.document)? != source_revision {
                            response.diff_actions.clear();
                        }
                        if let Some((provider, _)) = &git_extension {
                            if session.document_revision(self.document)? == source_revision {
                                for action in self.frame.take_git_actions() {
                                    provider
                                        .borrow_mut()
                                        .action(session, self.document, action)?;
                                }
                            } else {
                                self.frame.take_git_actions();
                            }
                        }
                        if let Some((provider, _)) = &source_extension {
                            // Input may edit the document while drawing. A row or
                            // hover collected for the old text must not escape.
                            if session.document_revision(self.document)? == source_revision {
                                let mut provider = provider.borrow_mut();
                                for action in response.source_actions.drain(..) {
                                    provider.action(session, self.document, action)?;
                                }
                                provider.draw_hover(ui, session, self)?;
                            } else {
                                response.source_actions.clear();
                            }
                        }
                        Ok(response)
                    })());
                });
            let result = match response {
                Some(result) => result,
                None => {
                    let snapshot = session.snapshot(self.document)?;
                    Ok(ViewResponse {
                        view: self.id,
                        document: self.document,
                        version: snapshot.version,
                        generation: snapshot.generation,
                        focused: false,
                        actions: Vec::new(),
                        source_actions: Vec::new(),
                        diff_actions: Vec::new(),
                        definition_request: None,
                    })
                }
            };
            if result.is_ok() {
                self.context = Some(ui.context_id());
            }
            result
        })
    }
    pub fn close(&mut self, session: &mut EditorSession, policy: ClosePolicy) -> io::Result<bool> {
        self.check_session(session)?;
        session.close_view(self.id, policy)
    }
    pub fn open_find(&mut self, session: &mut EditorSession) -> io::Result<()> {
        self.check_session(session)?;
        self.frame.line_jump.dismiss();
        session.with_view(self.id, |editor| self.frame.finder.open(editor))
    }
    pub fn open_line_jump(&mut self) {
        self.frame.finder.dismiss();
        self.frame.line_jump.open();
    }
    pub fn find_visible(&self) -> bool {
        self.frame.finder.active
    }
    pub fn line_jump_visible(&self) -> bool {
        self.frame.line_jump.active
    }
    pub fn presentation(&self) -> ViewPresentation<'_> {
        ViewPresentation {
            layout: self.frame.layout,
            hover_info: self.frame.hover_info(),
            hover_dismissed: self.frame.hover_dismissed(),
            tooltip_arbiter: &self.frame.tooltip_arbiter,
        }
    }
    pub fn read_only(&self) -> bool {
        self.frame.read_only
    }
    pub fn projected(&self) -> bool {
        self.frame.projection.is_projected()
    }
    pub fn visual_row(&self, document_row: i32) -> usize {
        self.frame.projection.visual_row(document_row)
    }
    pub fn hit_test(
        &self,
        ui: &Ui,
        session: &EditorSession,
        position: [f32; 2],
    ) -> io::Result<TextHit> {
        self.check_session(session)?;
        if self.context.is_some_and(|id| id != ui.context_id()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Hit-test EditorView in its drawing context",
            ));
        }
        // Host menus call this after draw() has restored the host's font.
        let _font = self
            .rendered_font
            .map(|(font, size)| ui.push_font_with_size(Some(font), size));
        let layout = &self.frame.layout;
        let revision = session.document_revision(self.document)?;
        if self.frame.projection_revision() != Some(revision) {
            return Ok(TextHit::None);
        }
        if layout.line_height <= 0.0
            || position[0] < layout.text_pos[0]
            || position[0] >= layout.pane_pos[0] + layout.pane_size[0]
            || position[1] < layout.pane_pos[1]
            || position[1] >= layout.pane_pos[1] + layout.pane_size[1]
        {
            return Ok(TextHit::None);
        }
        let visual = ((position[1] - layout.text_pos[1]) / layout.line_height).floor();
        if visual < 0.0 {
            return Ok(TextHit::None);
        }
        if let Some(row) = self.frame.projection.document_row(visual as usize) {
            let column = session.with_document(self.document, |document| {
                let line = document.line(row);
                bed_editing::util::utf8::snap_to_utf8_char_boundary(
                    &line,
                    crate::views::view_layout::column_at_x(
                        ui,
                        &line,
                        position[0] - layout.text_pos[0],
                    ),
                )
            })?;
            return Ok(TextHit::Document { row, column });
        }
        Ok(match self.frame.projection.rows.get(visual as usize) {
            Some(crate::diff::ProjectedRow::Historical { old_row, bytes }) => TextHit::Historical {
                old_row: *old_row,
                bytes: bytes.clone(),
            },
            Some(_) => TextHit::Control,
            None => TextHit::None,
        })
    }
    fn check_session(&self, session: &EditorSession) -> io::Result<()> {
        if session.workspace_id() != self.workspace {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "EditorView belongs to a different EditorSession",
            ));
        }
        if session.document_for_view(self.id) != Some(self.document) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "EditorView has been detached",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::{EditorExtensions, SourceDebugExtension};
    use crate::views::view_layout::{glyph_advance, line_column_x};
    use bed_editing::{editor_commands::CursorReveal, util::utf8::utf8_byte_offset_to_utf16};
    use dear_imgui_rs::{Condition, Context, FontSource, FramePrepareOptions, MouseButton};
    use std::cell::RefCell;

    #[derive(Default)]
    struct ExtensionCalls {
        queries: Vec<DocumentId>,
        actions: Vec<(DocumentId, SourceDebugAction)>,
        hovers: Vec<(DocumentId, ViewId, [f32; 2])>,
    }

    struct SourceExtensionProbe {
        document: DocumentId,
        calls: Rc<RefCell<ExtensionCalls>>,
    }

    impl SourceDebugExtension for SourceExtensionProbe {
        fn presentation(
            &self,
            _session: &EditorSession,
            document: DocumentId,
        ) -> io::Result<Option<SourceDebugPresentation>> {
            self.calls.borrow_mut().queries.push(document);
            Ok((document == self.document).then(SourceDebugPresentation::default))
        }

        fn action(
            &mut self,
            session: &EditorSession,
            document: DocumentId,
            action: SourceDebugAction,
        ) -> io::Result<()> {
            assert_eq!(document, self.document);
            // Extensions can inspect documents after the drawing borrow ends.
            session.snapshot(document)?;
            self.calls.borrow_mut().actions.push((document, action));
            Ok(())
        }

        fn draw_hover(
            &mut self,
            ui: &Ui,
            session: &EditorSession,
            view: &EditorView,
        ) -> io::Result<()> {
            assert_eq!(session.document_for_view(view.id()), Some(self.document));
            self.calls
                .borrow_mut()
                .hovers
                .push((view.document_id(), view.id(), ui.window_size()));
            Ok(())
        }
    }

    fn source_extension_probe(
        document: DocumentId,
    ) -> (
        Rc<RefCell<dyn SourceDebugExtension>>,
        Rc<RefCell<ExtensionCalls>>,
    ) {
        let calls = Rc::new(RefCell::new(ExtensionCalls::default()));
        let provider: Rc<RefCell<dyn SourceDebugExtension>> =
            Rc::new(RefCell::new(SourceExtensionProbe {
                document,
                calls: calls.clone(),
            }));
        (provider, calls)
    }

    fn extension_context() -> Context {
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        context
    }

    fn extension_frame(
        context: &mut Context,
        session: &mut EditorSession,
        views: &mut [&mut EditorView],
        extensions: &EditorExtensions,
        hidden: bool,
    ) -> (Vec<ViewResponse>, Vec<[f32; 2]>) {
        context.prepare_frame(FramePrepareOptions::new([1120.0, 500.0], 1.0));
        let ui = context.frame();
        let mut responses = Vec::new();
        let mut targets = Vec::new();
        ui.window("Extension host")
            .position([0.0; 2], Condition::Always)
            .size([1100.0, 450.0], Condition::Always)
            .flags(WindowFlags::NO_TITLE_BAR | WindowFlags::NO_MOVE | WindowFlags::NO_RESIZE)
            .build(|| {
                if hidden {
                    ui.set_cursor_pos([0.0, 10_000.0]);
                }
                for view in views.iter_mut() {
                    responses.push(
                        view.draw(
                            ui,
                            session,
                            &EditorViewOptions {
                                extensions: extensions.clone(),
                                size: [300.0, 360.0],
                                ..Default::default()
                            },
                        )
                        .unwrap(),
                    );
                    let layout = view.frame.layout;
                    targets.push([
                        layout.pane_pos[0]
                            + crate::views::gutter_view::GutterView::debug_column_width(ui, true)
                                * 0.32,
                        layout.pane_pos[1] + layout.editor_top_margin + layout.line_height * 2.4,
                    ]);
                    ui.same_line();
                }
            });
        drop(context.render_legacy());
        (responses, targets)
    }

    #[test]
    fn source_extensions_route_actions_and_hovers_to_the_provider_and_originating_views() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = extension_context();
        let mut session = EditorSession::new();
        let source = b"first\nsecond\nthird\nfourth";
        let document = session.create_document(source).unwrap();
        let other = session.create_document(source).unwrap();
        let mut left = EditorView::new(&mut session, document).unwrap();
        let mut right = EditorView::new(&mut session, document).unwrap();
        let mut third = EditorView::new(&mut session, other).unwrap();
        let (provider, calls) = source_extension_probe(document);
        let (other_provider, other_calls) = source_extension_probe(other);
        let extensions = EditorExtensions::default();
        // The first provider declines the two views of the other document.
        extensions.register_source_debug(&other_provider);
        extensions.register_source_debug(&provider);
        extension_frame(
            &mut context,
            &mut session,
            &mut [&mut left, &mut right, &mut third],
            &extensions,
            false,
        );
        calls.borrow_mut().hovers.clear();
        other_calls.borrow_mut().hovers.clear();
        let (_, targets) = extension_frame(
            &mut context,
            &mut session,
            &mut [&mut left, &mut right, &mut third],
            &extensions,
            false,
        );
        assert_eq!(
            calls.borrow().hovers,
            vec![
                (document, left.id(), [300.0, 360.0]),
                (document, right.id(), [300.0, 360.0]),
            ],
            "hover callbacks must retain each editor child's scope"
        );
        assert_eq!(
            other_calls.borrow().hovers,
            vec![(other, third.id(), [300.0, 360.0])]
        );
        context.io_mut().add_mouse_pos_event(targets[1]);
        extension_frame(
            &mut context,
            &mut session,
            &mut [&mut left, &mut right, &mut third],
            &extensions,
            false,
        );
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        let (responses, _) = extension_frame(
            &mut context,
            &mut session,
            &mut [&mut left, &mut right, &mut third],
            &extensions,
            false,
        );
        assert_eq!(
            calls.borrow().actions,
            vec![(document, SourceDebugAction::ToggleBreakpoint { row: 2 })]
        );
        assert!(other_calls.borrow().actions.is_empty());
        assert!(
            responses
                .iter()
                .all(|response| response.source_actions.is_empty())
        );
        drop(provider);
        let (_, targets) = extension_frame(
            &mut context,
            &mut session,
            &mut [&mut left, &mut right, &mut third],
            &extensions,
            false,
        );
        assert!(left.frame.source_debug.is_none());
        assert!(right.frame.source_debug.is_none());
        assert!(third.frame.source_debug.is_some());
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        context.io_mut().add_mouse_pos_event(targets[2]);
    }

    #[test]
    fn source_extensions_skip_hidden_or_changed_view_presentations() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = extension_context();
        let mut session = EditorSession::new();
        let document = session.create_document(b"source").unwrap();
        let mut view = EditorView::new(&mut session, document).unwrap();
        let (provider, calls) = source_extension_probe(document);
        let extensions = EditorExtensions::default();
        extensions.register_source_debug(&provider);
        session.request_focus(view.id()).unwrap();
        for _ in 0..2 {
            extension_frame(
                &mut context,
                &mut session,
                &mut [&mut view],
                &extensions,
                false,
            );
        }
        let before = session.document_revision(document).unwrap();
        calls.borrow_mut().hovers.clear();
        context.io_mut().add_input_characters_utf8("x");
        extension_frame(
            &mut context,
            &mut session,
            &mut [&mut view],
            &extensions,
            false,
        );
        assert_ne!(session.document_revision(document).unwrap(), before);
        assert!(
            calls.borrow().hovers.is_empty(),
            "hover geometry belongs to the old revision"
        );
        extension_frame(
            &mut context,
            &mut session,
            &mut [&mut view],
            &extensions,
            false,
        );
        assert_eq!(calls.borrow().hovers.len(), 1);
        calls.borrow_mut().hovers.clear();
        extension_frame(
            &mut context,
            &mut session,
            &mut [&mut view],
            &extensions,
            true,
        );
        assert!(
            calls.borrow().hovers.is_empty(),
            "a clipped child has no live hover scope"
        );
    }

    #[test]
    fn source_extensions_are_not_called_for_foreign_or_detached_views() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = extension_context();
        let mut session = EditorSession::new();
        let document = session.create_document(b"source").unwrap();
        let mut view = EditorView::new(&mut session, document).unwrap();
        let (provider, calls) = source_extension_probe(document);
        let extensions = EditorExtensions::default();
        extensions.register_source_debug(&provider);
        let options = EditorViewOptions {
            extensions,
            ..Default::default()
        };
        context.prepare_frame(FramePrepareOptions::new([800.0, 500.0], 1.0));
        let ui = context.frame();
        let mut foreign = EditorSession::new();
        assert_eq!(
            view.draw(ui, &mut foreign, &options).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        session.detach_view(view.id());
        assert_eq!(
            view.draw(ui, &mut session, &options).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert!(calls.borrow().queries.is_empty());
        assert!(calls.borrow().actions.is_empty());
        assert!(calls.borrow().hovers.is_empty());
        drop(context.render_legacy());
    }

    fn frame(
        context: &mut Context,
        session: &mut EditorSession,
        left: &mut EditorView,
        right: &mut EditorView,
        blocked: bool,
    ) -> (Vec<ViewResponse>, [f32; 2]) {
        // Keep independent clicks outside native double-click time.
        context.prepare_frame(FramePrepareOptions::new([1000.0, 700.0], 1.0));
        let ui = context.frame();
        let mut responses = Vec::new();
        let mut target = [0.0; 2];
        let right_id = right.id();
        ui.window("Host click layout")
            .position([0.0; 2], Condition::Always)
            .size([980.0, 650.0], Condition::Always)
            .flags(WindowFlags::NO_TITLE_BAR | WindowFlags::NO_MOVE | WindowFlags::NO_RESIZE)
            .build(|| {
                for view in [left, right] {
                    responses.push(
                        view.draw(
                            ui,
                            session,
                            &EditorViewOptions {
                                size: [450.0, 560.0],
                                block_input: blocked,
                                ..Default::default()
                            },
                        )
                        .unwrap(),
                    );
                    if view.id() == right_id {
                        let layout = view.frame.layout;
                        target = [
                            line_column_x(ui, "é🙂target".as_bytes(), 6, layout.text_pos[0])
                                + glyph_advance(ui, "t") * 0.2,
                            layout.text_pos[1] + layout.line_height * 1.25,
                        ];
                    }
                    ui.same_line();
                }
            });
        assert!(context.render_legacy().draw_data().total_vtx_count() > 0);
        (responses, target)
    }

    #[test]
    fn primary_click_requests_definition_in_clicked_view_without_selection_drag() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mappings: &[bool] = if cfg!(target_os = "macos") {
            &[false, true]
        } else {
            &[false]
        };
        for &mac_mapping in mappings {
            verify_primary_click(mac_mapping);
        }
    }

    fn verify_primary_click(mac_mapping: bool) {
        let mut context = Context::create();
        context.io_mut().set_config_macosx_behaviors(mac_mapping);
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .add_font(&[FontSource::default_font_with_size(15.0)]);
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut session = EditorSession::new();
        let document = session
            .create_document("prefix\né🙂target tail".as_bytes())
            .unwrap();
        let mut left = EditorView::new(&mut session, document).unwrap();
        let mut right = EditorView::new(&mut session, document).unwrap();
        for view in [left.id(), right.id()] {
            session
                .with_commands(view, |commands| {
                    commands.set_selection(0, 0, 0, 3, CursorReveal::Ensure);
                    commands.add_cursor_below();
                })
                .unwrap();
        }
        session.request_focus(left.id()).unwrap();
        frame(&mut context, &mut session, &mut left, &mut right, false);
        let (_, target) = frame(&mut context, &mut session, &mut left, &mut right, false);
        let before_left = session.view_snapshot(left.id()).unwrap().selections;
        let before_right = session.view_snapshot(right.id()).unwrap().selections;
        assert_eq!(before_right.len(), 2);
        let before_document = session.snapshot(document).unwrap();
        context.io_mut().add_mouse_pos_event(target);
        frame(&mut context, &mut session, &mut left, &mut right, false);
        let modifier = if cfg!(target_os = "macos") {
            Key::ModSuper
        } else {
            Key::ModCtrl
        };
        context.io_mut().add_key_event(modifier, true);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        let (responses, _) = frame(&mut context, &mut session, &mut left, &mut right, false);
        assert!(
            session.lsp().is_none(),
            "a view emits navigation without requiring a server"
        );
        assert_eq!(responses[0].definition_request, None);
        assert_eq!(responses[1].view, right.id());
        assert_eq!(responses[1].document, document);
        assert_eq!(responses[1].version, before_document.version);
        assert_eq!(responses[1].generation, before_document.generation);
        let request = responses[1].definition_request.unwrap();
        assert_eq!(request, DefinitionRequest { row: 1, column: 6 });
        assert_eq!(
            utf8_byte_offset_to_utf16("é🙂target".as_bytes(), request.column),
            3
        );
        assert_eq!(
            session.view_snapshot(left.id()).unwrap().selections,
            before_left
        );
        assert_eq!(
            session.view_snapshot(right.id()).unwrap().selections,
            before_right
        );
        context
            .io_mut()
            .add_mouse_pos_event([target[0] + 80.0, target[1] + 20.0]);
        let (responses, _) = frame(&mut context, &mut session, &mut left, &mut right, false);
        assert!(
            responses
                .iter()
                .all(|response| response.definition_request.is_none())
        );
        assert_eq!(
            session.view_snapshot(right.id()).unwrap().selections,
            before_right
        );
        assert_eq!(
            session.snapshot(document).unwrap().bytes,
            before_document.bytes
        );
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        context.io_mut().add_key_event(modifier, false);
        let (_, target) = frame(&mut context, &mut session, &mut left, &mut right, false);
        context.io_mut().add_mouse_pos_event(target);
        frame(&mut context, &mut session, &mut left, &mut right, false);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        let (responses, _) = frame(&mut context, &mut session, &mut left, &mut right, false);
        assert!(
            responses
                .iter()
                .all(|response| response.definition_request.is_none())
        );
        let cursor = session.view_snapshot(right.id()).unwrap();
        assert_eq!((cursor.row, cursor.column), (1, 6));
        assert_eq!(cursor.selections.len(), 1);
        assert_eq!(
            session.view_snapshot(left.id()).unwrap().selections,
            before_left
        );
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        frame(&mut context, &mut session, &mut left, &mut right, false);
        context.io_mut().add_key_event(modifier, true);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        let (responses, _) = frame(&mut context, &mut session, &mut left, &mut right, true);
        assert!(
            responses
                .iter()
                .all(|response| response.definition_request.is_none())
        );
        assert_eq!(
            session.view_snapshot(right.id()).unwrap().selections,
            cursor.selections
        );
    }

    fn debug_frame(
        context: &mut Context,
        session: &mut EditorSession,
        view: &mut EditorView,
        presentation: Option<SourceDebugPresentation>,
        blocked: bool,
    ) -> (ViewResponse, [f32; 2]) {
        context.prepare_frame(FramePrepareOptions::new([800.0, 500.0], 1.0));
        let ui = context.frame();
        let mut response = None;
        let mut target = [0.0; 2];
        ui.window("Debug source host")
            .position([0.0; 2], Condition::Always)
            .size([780.0, 450.0], Condition::Always)
            .flags(WindowFlags::NO_TITLE_BAR | WindowFlags::NO_MOVE | WindowFlags::NO_RESIZE)
            .build(|| {
                response = Some(
                    view.draw(
                        ui,
                        session,
                        &EditorViewOptions {
                            size: [700.0, 360.0],
                            rainbow_mode: false,
                            source_debug: presentation,
                            block_input: blocked,
                            ..Default::default()
                        },
                    )
                    .unwrap(),
                );
                let layout = view.frame.layout;
                target = [
                    layout.pane_pos[0]
                        + crate::views::gutter_view::GutterView::debug_column_width(ui, true)
                            * 0.32,
                    layout.pane_pos[1] + layout.editor_top_margin + layout.line_height * 2.4,
                ];
            });
        drop(context.render_legacy());
        (response.unwrap(), target)
    }

    #[test]
    fn source_breakpoint_click_uses_scrolled_row_without_changing_caret_and_respects_overlays() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut session = EditorSession::new();
        let bytes = (0..100)
            .map(|row| format!("source row {row}\n"))
            .collect::<String>();
        let document = session.create_document(bytes.as_bytes()).unwrap();
        let mut view = EditorView::new(&mut session, document).unwrap();
        view.set_navigation_animations(false);
        session
            .with_commands(view.id(), |commands| {
                commands.set_selection(0, 0, 0, 6, CursorReveal::Ensure);
            })
            .unwrap();
        let debug = Some(SourceDebugPresentation::default());
        debug_frame(&mut context, &mut session, &mut view, debug.clone(), false);
        let line_height = view.frame.layout.line_height;
        session
            .set_scroll(view.id(), 0.0, line_height * 40.0)
            .unwrap();
        let (_, target) = debug_frame(&mut context, &mut session, &mut view, debug.clone(), false);
        debug_frame(&mut context, &mut session, &mut view, debug.clone(), false);
        assert!(
            (session.view_snapshot(view.id()).unwrap().scroll_position[1] - line_height * 40.0)
                .abs()
                < 1.0
        );
        let before = session.view_snapshot(view.id()).unwrap().selections;
        context.io_mut().add_mouse_pos_event(target);
        debug_frame(&mut context, &mut session, &mut view, debug.clone(), false);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        let (response, _) =
            debug_frame(&mut context, &mut session, &mut view, debug.clone(), false);
        assert_eq!(
            response.source_actions,
            vec![SourceDebugAction::ToggleBreakpoint { row: 42 }]
        );
        assert_eq!(session.view_snapshot(view.id()).unwrap().selections, before);
        assert_eq!(session.snapshot(document).unwrap().bytes, bytes.as_bytes());
        let (response, _) =
            debug_frame(&mut context, &mut session, &mut view, debug.clone(), false);
        assert!(
            response.source_actions.is_empty(),
            "holding the button must not repeat"
        );
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        debug_frame(&mut context, &mut session, &mut view, debug.clone(), true);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        let (response, _) = debug_frame(&mut context, &mut session, &mut view, debug, true);
        assert!(response.source_actions.is_empty());
        assert_eq!(session.view_snapshot(view.id()).unwrap().selections, before);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        debug_frame(&mut context, &mut session, &mut view, None, false);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        let (response, _) = debug_frame(&mut context, &mut session, &mut view, None, false);
        assert!(
            response.source_actions.is_empty(),
            "ordinary embeds do not emit debug actions"
        );
    }
    fn diff_frame(
        context: &mut Context,
        session: &mut EditorSession,
        view: &mut EditorView,
        options: &EditorViewOptions,
    ) -> ViewResponse {
        context.prepare_frame(FramePrepareOptions::new([900.0, 600.0], 1.0));
        let ui = context.frame();
        let mut result = None;
        ui.window("Diff host")
            .position([0.0; 2], Condition::Always)
            .size([880.0, 550.0], Condition::Always)
            .flags(WindowFlags::NO_TITLE_BAR | WindowFlags::NO_MOVE | WindowFlags::NO_RESIZE)
            .build(|| {
                result = Some(view.draw(ui, session, options).unwrap());
            });
        drop(context.render_legacy());
        result.unwrap()
    }

    #[test]
    fn unified_diff_edits_shared_document_and_reprojects_before_painting() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = extension_context();
        let mut session = EditorSession::new();
        let document = session.create_document(b"head\nnew\ntail\n").unwrap();
        let mut diff = EditorView::new(&mut session, document).unwrap();
        let normal = EditorView::new(&mut session, document).unwrap();
        let options = EditorViewOptions {
            diff: Some(crate::DiffPresentation {
                baseline: (&b"head\nold\ntail\n"[..]).into(),
                comparison: 9,
                actions: vec![crate::DiffActionKind::Stage],
                actions_enabled: true,
                saved: None,
            }),
            ..Default::default()
        };
        session.request_focus(diff.id()).unwrap();
        diff_frame(&mut context, &mut session, &mut diff, &options);
        diff_frame(&mut context, &mut session, &mut diff, &options);
        assert_eq!(diff.visual_row(1), 3);
        // Clicking the removed row does not move the real document caret.
        let layout = diff.frame.layout;
        let ghost = [
            layout.text_pos[0] + 1.0,
            layout.text_pos[1] + 2.25 * layout.line_height,
        ];
        let before = session.view_snapshot(diff.id()).unwrap().selections;
        context.io_mut().add_mouse_pos_event(ghost);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        diff_frame(&mut context, &mut session, &mut diff, &options);
        assert_eq!(session.view_snapshot(diff.id()).unwrap().selections, before);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        diff_frame(&mut context, &mut session, &mut diff, &options);
        // Two real carets span a ghost row; typing modifies only the working text.
        session
            .with_commands(diff.id(), |commands| {
                commands.set_selections(
                    vec![
                        bed_editing::editor_view_state::Selection {
                            head_row: 1,
                            head_column: 0,
                            anchor_row: 1,
                            anchor_column: 0,
                            preferred_column: 0,
                        },
                        bed_editing::editor_view_state::Selection {
                            head_row: 2,
                            head_column: 0,
                            anchor_row: 2,
                            anchor_column: 0,
                            preferred_column: 0,
                        },
                    ],
                    0,
                    CursorReveal::Ensure,
                )
            })
            .unwrap();
        context.io_mut().add_input_characters_utf8("X");
        diff_frame(&mut context, &mut session, &mut diff, &options);
        assert_eq!(
            session.snapshot(document).unwrap().bytes,
            b"head\nXnew\nXtail\n"
        );
        assert_eq!(
            session
                .with_view(normal.id(), |editor| editor.state.join())
                .unwrap(),
            b"head\nXnew\nXtail\n"
        );
        assert_eq!(
            diff.frame.projection_revision(),
            Some(session.document_revision(document).unwrap())
        );
        session
            .with_commands(normal.id(), |commands| commands.undo())
            .unwrap();
        diff_frame(&mut context, &mut session, &mut diff, &options);
        assert_eq!(
            session.snapshot(document).unwrap().bytes,
            b"head\nnew\ntail\n"
        );
    }

    #[test]
    fn staged_snapshot_keeps_navigation_and_blocks_character_delete_paste_and_undo() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = extension_context();
        let mut session = EditorSession::new();
        let document = session
            .create_snapshot_document(b"staged\nsecond", "rust")
            .unwrap();
        let mut view = EditorView::new(&mut session, document).unwrap();
        let options = EditorViewOptions::default();
        session.request_focus(view.id()).unwrap();
        diff_frame(&mut context, &mut session, &mut view, &options);
        diff_frame(&mut context, &mut session, &mut view, &options);
        assert!(view.read_only());
        context.io_mut().add_input_characters_utf8("bad");
        context.io_mut().add_key_event(Key::Delete, true);
        context.io_mut().add_key_event(Key::DownArrow, true);
        diff_frame(&mut context, &mut session, &mut view, &options);
        assert_eq!(session.snapshot(document).unwrap().bytes, b"staged\nsecond");
        assert_eq!(session.view_snapshot(view.id()).unwrap().row, 1);
        context.io_mut().add_key_event(Key::Delete, false);
        context.io_mut().add_key_event(Key::DownArrow, false);
        // Direct menu actions are guarded at the session as well.
        session
            .with_commands(view.id(), |commands| {
                commands.paste(b"bad");
                commands.undo();
                commands.redo();
            })
            .unwrap();
        assert_eq!(session.snapshot(document).unwrap().bytes, b"staged\nsecond");
    }

    #[test]
    fn conflict_controls_resolve_in_normal_editor_and_undo_as_one_edit() {
        use crate::extensions::SourceGitExtension;
        use crate::{ConflictChoice, SourceConflict, SourceGitAction, SourceGitPresentation};
        struct ConflictProvider {
            document: DocumentId,
        }
        impl SourceGitExtension for ConflictProvider {
            fn presentation(
                &self,
                session: &EditorSession,
                document: DocumentId,
            ) -> io::Result<Option<SourceGitPresentation>> {
                let snapshot = session.snapshot(document)?;
                Ok(
                    (document == self.document && snapshot.bytes.starts_with(b"<<<<<<<")).then(
                        || SourceGitPresentation {
                            conflicts: vec![SourceConflict {
                                id: 7,
                                row: 0,
                                label: "Conflict 1".into(),
                                current_label: "ours".into(),
                                incoming_label: "theirs".into(),
                            }],
                        },
                    ),
                )
            }
            fn action(
                &mut self,
                session: &mut EditorSession,
                document: DocumentId,
                action: SourceGitAction,
            ) -> io::Result<()> {
                assert_eq!(
                    action,
                    SourceGitAction::ResolveConflict {
                        id: 7,
                        choice: ConflictChoice::Current
                    }
                );
                let snapshot = session.snapshot(document)?;
                session.apply_edits(
                    document,
                    session.document_revision(document)?,
                    &[bed_document_session::editor_session::ByteEdit {
                        range: 0..snapshot.bytes.len(),
                        bytes: b"ours\n".to_vec(),
                    }],
                )
            }
        }
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = extension_context();
        let mut session = EditorSession::new();
        let bytes = b"<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> branch\n";
        let document = session.create_document(bytes).unwrap();
        let mut view = EditorView::new(&mut session, document).unwrap();
        let provider: Rc<RefCell<dyn SourceGitExtension>> =
            Rc::new(RefCell::new(ConflictProvider { document }));
        let options = EditorViewOptions::default();
        options.extensions.register_source_git(&provider);
        session.request_focus(view.id()).unwrap();
        diff_frame(&mut context, &mut session, &mut view, &options);
        diff_frame(&mut context, &mut session, &mut view, &options);
        assert_eq!(view.visual_row(0), 1);
        let layout = view.frame.layout;
        context
            .io_mut()
            .add_mouse_pos_event([layout.text_pos[0] + 5.0, layout.text_pos[1] + 4.0]);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        diff_frame(&mut context, &mut session, &mut view, &options);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        diff_frame(&mut context, &mut session, &mut view, &options);
        assert_eq!(session.snapshot(document).unwrap().bytes, b"ours\n");
        session
            .with_commands(view.id(), |commands| commands.undo())
            .unwrap();
        assert_eq!(session.snapshot(document).unwrap().bytes, bytes);
        drop(provider);
        diff_frame(&mut context, &mut session, &mut view, &options);
        assert!(!view.projected());
    }
    #[test]
    fn unified_diff_hit_testing_distinguishes_history_controls_and_utf8_live_text() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = extension_context();
        let mut session = EditorSession::new();
        let document = session
            .create_document("head\né🙂new\ntail".as_bytes())
            .unwrap();
        let mut view = EditorView::new(&mut session, document).unwrap();
        let options = EditorViewOptions {
            diff: Some(crate::DiffPresentation {
                baseline: (&b"head\nold\ntail"[..]).into(),
                comparison: 2,
                actions: vec![],
                actions_enabled: true,
                saved: None,
            }),
            ..Default::default()
        };
        diff_frame(&mut context, &mut session, &mut view, &options);
        diff_frame(&mut context, &mut session, &mut view, &options);
        context.prepare_frame(FramePrepareOptions::new([900.0, 600.0], 1.0));
        let ui = context.frame();
        ui.window("Diff host")
            .position([0.0; 2], Condition::Always)
            .size([880.0, 550.0], Condition::Always)
            .build(|| {
                view.draw(ui, &mut session, &options).unwrap();
                let layout = view.frame.layout;
                let x = line_column_x(ui, "é🙂new".as_bytes(), 6, layout.text_pos[0]);
                let hit = |row: f32| {
                    view.hit_test(
                        ui,
                        &session,
                        [x, layout.text_pos[1] + row * layout.line_height],
                    )
                    .unwrap()
                };
                assert_eq!(hit(1.25), TextHit::Control);
                assert_eq!(
                    hit(2.25),
                    TextHit::Historical {
                        old_row: 1,
                        bytes: b"old".to_vec()
                    }
                );
                assert_eq!(hit(3.25), TextHit::Document { row: 1, column: 6 });
            });
        drop(context.render_legacy());
    }
}
