//! Deterministic integration coverage for this crate, independent of bEd.
use bedtime_editing::identity::DocumentId;
use bedtime_editing::identity::ViewId;
use bedtime_lsp::jsonrpc::CONNECTION_CLOSED;
use bedtime_lsp::lsp_client::ProgressToken;
use bedtime_lsp::message_handler::RpcEvent;
use bedtime_lsp::workspace_lsp::LspRequestOrigin;
include!("../../../tests/support/lsp_fixture.rs");
#[path = "../../../tests/support/runner.rs"]
mod runner;
use bedtime_documents::{
    document_runtime::DocumentRuntime,
    session::{ClosePolicy, DocumentSession, SessionOptions},
};
use bedtime_editor_widgets::editor_view::{EditorView, EditorViewOptions};
use bedtime_plugin_editor::lsp::lsp_ui::{ContextLspAction, LspUi};
#[path = "../../../tests/support/headless_host.rs"]
#[allow(dead_code)]
mod headless_host;
fn main() {
    if dispatch_fixture() {
        return;
    }
    runner::run(&[
        (
            "work_done_progress_is_main_thread_owned_and_resets",
            work_done_progress_is_main_thread_owned_and_resets,
        ),
        (
            "shared_session_save_diagnostics_render_in_each_custom_view",
            shared_session_save_diagnostics_render_in_each_custom_view,
        ),
        (
            "typed_navigation_results_follow_the_requesting_view",
            typed_navigation_results_follow_the_requesting_view,
        ),
        (
            "workbench_single_definition_returns_to_requesting_view_without_references",
            workbench_single_definition_returns_to_requesting_view_without_references,
        ),
        (
            "workbench_cross_file_definition_uses_a_preview_and_preserves_same_file_tabs",
            workbench_cross_file_definition_uses_a_preview_and_preserves_same_file_tabs,
        ),
        (
            "workbench_multiple_and_stale_definitions_use_guarded_routes",
            workbench_multiple_and_stale_definitions_use_guarded_routes,
        ),
        (
            "workbench_primary_click_sends_clicked_utf16_position",
            workbench_primary_click_sends_clicked_utf16_position,
        ),
        (
            "workbench_cold_navigation_retains_one_action_and_position",
            workbench_cold_navigation_retains_one_action_and_position,
        ),
        (
            "workbench_navigation_retries_only_transient_errors",
            workbench_navigation_retries_only_transient_errors,
        ),
        (
            "workbench_navigation_empty_errors_and_timeout_are_distinct",
            workbench_navigation_empty_errors_and_timeout_are_distinct,
        ),
        (
            "workbench_latest_navigation_cancels_the_previous_rpc",
            workbench_latest_navigation_cancels_the_previous_rpc,
        ),
    ]);
}
fn shared_session_save_diagnostics_render_in_each_custom_view() {
    use bedtime_editing::text_commands::CursorReveal;
    use bedtime_editor_widgets::views::diagnostic_style::severity_color;
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions, sys};

    let fixture = PoolFixture::new(9900, "basic");
    let path = fixture.root.join("shared😀.rs");
    fs::write(&path, "a🙂b\n".as_bytes()).unwrap();
    let mut session = DocumentSession::with_options(SessionOptions {
        project_root: Some(fixture.root.clone()),
        lsp_config: Some(fixture.root.join("lsp.json")),
        ..SessionOptions::default()
    })
    .unwrap();
    session.lsp_mut().unwrap().set_server_arguments(
        "rust",
        vec!["--mock-server".into(), "session-diagnostics".into()],
    );
    let document = session.open_file(&path).unwrap();
    let canonical = session.snapshot(document).unwrap().path;
    let client = session
        .lsp()
        .unwrap()
        .client_for_document(document)
        .unwrap();
    let diagnostics = session
        .lsp()
        .unwrap()
        .diagnostics_for_document(document)
        .unwrap();
    wait_until(|| {
        let report = session.tick();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        client.borrow().is_initialized() && client.borrow().is_document_open(&canonical)
    });
    let mut left = EditorView::new(&mut session, document).unwrap();
    let mut right = EditorView::new(&mut session, document).unwrap();
    let mut context = Context::create();
    context.set_ini_filename(None::<PathBuf>).unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    let draw = |context: &mut Context,
                session: &mut DocumentSession,
                left: &mut EditorView,
                right: &mut EditorView| {
        context.prepare_frame(FramePrepareOptions::new([1000.0, 600.0], 1.0 / 60.0));
        let ui = context.frame();
        let diagnostic_mark = ui.with_bound_context(|| unsafe {
            // Diagnostic ink adapts to the host palette; retain the geometry
            // assertions without depending on the unadjusted source color.
            sys::igColorConvertFloat4ToU32(
                bedtime_ui::presentation::readable_color(ui, severity_color(1)).into(),
            )
        });
        ui.window("Session diagnostic canvas")
            .position([0.0, 0.0], Condition::Always)
            .size([980.0, 550.0], Condition::Always)
            .build(|| {
                for view in [left, right] {
                    view.draw(
                        ui,
                        session,
                        &EditorViewOptions {
                            size: [450.0, 480.0],
                            rainbow_mode: false,
                            ..EditorViewOptions::default()
                        },
                    )
                    .unwrap();
                    ui.same_line();
                }
            });
        let geometry = ui.with_bound_context(|| unsafe {
            let native = &*sys::igGetCurrentContext();
            let mut text_counts = Vec::new();
            let mut gutter_counts = Vec::new();
            for index in 0..native.Windows.Size {
                let window = &**native.Windows.Data.add(index as usize);
                let name = std::ffi::CStr::from_ptr(window.Name).to_string_lossy();
                if !name.contains("Session diagnostic canvas") {
                    continue;
                }
                let draw = &*window.DrawList;
                let marks = (0..draw.VtxBuffer.Size)
                    .filter(|index| {
                        (*draw.VtxBuffer.Data.add(*index as usize)).col == diagnostic_mark
                    })
                    .count();
                if name.contains("LineNumbers") {
                    gutter_counts.push(marks);
                } else if name.contains("##editor") {
                    text_counts.push(marks);
                }
            }
            (text_counts, gutter_counts)
        });
        drop(context.render_legacy());
        geometry
    };
    assert_eq!(
        draw(&mut context, &mut session, &mut left, &mut right),
        (vec![0, 0], vec![0, 0])
    );
    session
        .with_commands(left.id(), |commands| {
            commands.set_cursor(0, 5, false, CursorReveal::Ensure);
            commands.type_text(b"INVALID");
        })
        .unwrap();
    session.save(document).unwrap();
    let malformed = session.snapshot(document).unwrap();
    assert_eq!(malformed.bytes, "a🙂INVALIDb\n".as_bytes());
    // The mock publishes on didSave. Drawing the view never polls services or
    // publishes background results into the main-thread diagnostic store.
    thread::sleep(Duration::from_millis(20));
    assert!(diagnostics.for_document(&canonical).is_empty());
    assert_eq!(
        draw(&mut context, &mut session, &mut left, &mut right),
        (vec![0, 0], vec![0, 0])
    );
    wait_until(|| {
        let report = session.tick();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        !diagnostics.for_document(&canonical).is_empty()
    });
    let errors = diagnostics.for_document(&canonical);
    assert_eq!(errors.len(), 1);
    assert_eq!(
        (errors[0].start_character, errors[0].end_character),
        (3, 10)
    );
    assert_eq!(errors[0].message, "Session save diagnostic");
    let (text, gutter) = draw(&mut context, &mut session, &mut left, &mut right);
    assert_eq!(text.len(), 2);
    assert!(
        text.iter().all(|count| *count > 4),
        "each view paints the UTF-16 squiggle: {text:?}"
    );
    assert_eq!(gutter, vec![4, 4], "each view paints one error marker");
    let transcript = shared_request(&client, "test/transcript");
    let transcript = transcript.as_array().unwrap();
    assert_eq!(
        transcript
            .iter()
            .filter(|item| item["method"] == "textDocument/didOpen")
            .count(),
        1
    );
    let changes: Vec<_> = transcript
        .iter()
        .filter(|item| item["method"] == "textDocument/didChange")
        .collect();
    assert_eq!(changes.len(), 1);
    assert_eq!(
        changes[0]["params"]["contentChanges"][0]["range"]["start"]["character"],
        3
    );
    assert_eq!(
        changes[0]["params"]["textDocument"]["version"],
        malformed.version
    );
    let saves: Vec<_> = transcript
        .iter()
        .filter(|item| item["method"] == "textDocument/didSave")
        .collect();
    assert_eq!(saves.len(), 1);
    assert_eq!(saves[0]["params"]["text"], "a🙂INVALIDb\n");
    session
        .with_commands(right.id(), |commands| commands.undo())
        .unwrap();
    session.save(document).unwrap();
    wait_until(|| {
        session.tick();
        diagnostics.for_document(&canonical).is_empty()
    });
    assert_eq!(
        session.snapshot(document).unwrap().bytes,
        "a🙂b\n".as_bytes()
    );
    assert_eq!(
        draw(&mut context, &mut session, &mut left, &mut right),
        (vec![0, 0], vec![0, 0])
    );
    session.shutdown(ClosePolicy::Discard).unwrap();
}

struct NavigationWorkbench {
    fixture: PoolFixture,
    context: dear_imgui_rs::Context,
    app: bevy::app::App,
    client: Rc<RefCell<LspClient>>,
    document: DocumentId,
    requesting: ViewId,
    sibling: ViewId,
    request_tab: bevy::prelude::Entity,
    sibling_tab: bevy::prelude::Entity,
    path: String,
}

impl NavigationWorkbench {
    fn new() -> Self {
        Self::with_startup("basic", |_, _, _, _| {})
    }

    fn with_startup(
        scenario: &str,
        before_ready: impl FnOnce(
            &mut bevy::app::App,
            &mut dear_imgui_rs::Context,
            ViewId,
            bevy::prelude::Entity,
        ),
    ) -> Self {
        use bedtime_editing::text_commands::CursorReveal;
        let fixture = PoolFixture::new(9910, "basic");
        let path = fixture.root.join("navigation.rs");
        fs::write(&path, "é🙂symbol\nline target\nlast\n").unwrap();
        let config_dir = fixture.root.join("config");
        let mut configuration: Value =
            serde_json::from_slice(&fs::read(fixture.root.join("lsp.json")).unwrap()).unwrap();
        configuration["language_servers"]["rust"]["args"] = json!(["--mock-server", scenario]);
        if scenario == "navigation-timeout" {
            configuration["language_servers"]["rust"]["timeout_secs"] = json!(1);
        }
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("lsp.json"),
            serde_json::to_vec(&configuration).unwrap(),
        )
        .unwrap();
        let mut context = dear_imgui_rs::Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        let mut app = bevy::app::App::new();
        headless_host::install(&mut app, &config_dir);
        app.add_plugins(bedtime_plugin_editor::EditorPlugin);
        headless_host::finish(&mut app, &fixture.root);
        bedtime_core::settings::apply_settings_request(
            app.world_mut(),
            &bedtime_api::settings::SettingsRequest::batch(
                [
                    ("bedtime.editor", "treesitter"),
                    ("bedtime.editor", "git_changed_lines"),
                    ("bedtime.editor", "minimap"),
                ]
                .into_iter()
                .map(|(owner, key)| bedtime_api::settings::SettingsChange::set(owner, key, false))
                .collect(),
            ),
        )
        .unwrap();

        headless_host::prepare_ui(app.world_mut(), &mut context).unwrap();
        let request_tab = bedtime_core::open_file(app.world_mut(), &path, None, None).unwrap();
        let document = app
            .world()
            .resource::<bedtime_core::DocumentNavigation>()
            .active_document
            .unwrap();
        let requesting = app
            .world()
            .resource::<bedtime_core::DocumentNavigation>()
            .active_view
            .unwrap();
        headless_host::invoke(
            app.world_mut(),
            "bedtime.workbench.duplicate_view",
            Some(request_tab),
        )
        .unwrap();
        let sibling = app
            .world()
            .resource::<bedtime_core::DocumentNavigation>()
            .active_view
            .unwrap();
        let sibling_tab = app
            .world()
            .resource::<bedtime_workbench::WorkbenchLayout>()
            .focused
            .unwrap();
        app.world_mut()
            .non_send_mut::<DocumentSession>()
            .with_commands(requesting, |commands| {
                commands.set_cursor(0, 2, false, CursorReveal::Ensure)
            })
            .unwrap();
        app.world_mut()
            .non_send_mut::<DocumentSession>()
            .with_commands(sibling, |commands| {
                commands.set_cursor(2, 1, false, CursorReveal::Ensure)
            })
            .unwrap();
        let path = app
            .world()
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .path;
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        before_ready(&mut app, &mut context, requesting, request_tab);
        wait_until(|| {
            app.update();
            app.world()
                .non_send::<DocumentSession>()
                .lsp()
                .unwrap()
                .client_for_document(document)
                .is_some()
        });
        let client = app
            .world()
            .non_send::<DocumentSession>()
            .lsp()
            .unwrap()
            .client_for_document(document)
            .unwrap();
        wait_until(|| {
            app.update();
            client.borrow().is_initialized() && client.borrow().is_document_open(&path)
        });
        let mut result = Self {
            fixture,
            context,
            app,
            client,
            document,
            requesting,
            sibling,
            request_tab,
            sibling_tab,
            path,
        };
        result.focus(request_tab);
        result
    }

    fn frame(&mut self) {
        self.app.update();
        self.context
            .prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
                [1600.0, 1000.0],
                1.0 / 60.0,
            ));
        headless_host::draw_ui(self.app.world_mut(), self.context.frame()).unwrap();
        assert!(self.context.render_legacy().draw_data().total_vtx_count() > 0);
        bedtime_core::resolve_documents(self.app.world_mut());
    }

    fn focus(&mut self, tab: bevy::prelude::Entity) {
        bedtime_core::focus_panel(self.app.world_mut(), tab, true).unwrap();
        self.frame();
        self.frame();
    }

    fn session(&self) -> &DocumentSession {
        self.app.world().non_send::<DocumentSession>()
    }

    fn session_mut(&mut self) -> bevy::ecs::change_detection::Mut<'_, DocumentSession> {
        self.app.world_mut().non_send_mut::<DocumentSession>()
    }

    fn panel_count(&mut self, kind: &str) -> usize {
        self.app
            .world_mut()
            .query::<(
                &bedtime_workbench::PanelRecord,
                &bedtime_api::workbench::PanelPresentation,
            )>()
            .iter(self.app.world())
            .filter(|(record, _)| record.panel_type == kind)
            .count()
    }

    fn active_view(&self) -> Option<ViewId> {
        self.app
            .world()
            .resource::<bedtime_core::DocumentNavigation>()
            .active_view
    }

    fn close(&mut self, panel: bevy::prelude::Entity) {
        headless_host::workbench(
            self.app.world_mut(),
            bedtime_api::workbench::WorkbenchAction::ClosePanel(panel),
        )
        .unwrap();
        assert!(!self.app.world().entities().contains(panel));
    }

    fn configure(&self, result: Value) {
        shared_request_params(
            &self.client,
            "test/definition/configure",
            json!({"result":result,"hold":true}),
        );
    }

    fn location(&self, row: i32, column: i32) -> Value {
        json!({"uri":LspUri::file_uri_from_path(&self.path).unwrap().to_string(),"range":{"start":{"line":row,"character":column},"end":{"line":row,"character":column+1}}})
    }

    fn shortcut(&mut self) {
        use dear_imgui_rs::Key;
        self.focus(self.request_tab);
        self.context.io_mut().add_key_event(Key::F12, true);
        self.frame();
        self.context.io_mut().add_key_event(Key::F12, false);
        self.frame();
        assert_eq!(
            self.panel_count(bedtime_plugin_editor::REFERENCES_PANEL_TYPE),
            0
        );
        assert!(
            shared_request(&self.client, "test/transcript")
                .as_array()
                .unwrap()
                .iter()
                .any(|message| message["method"] == "textDocument/definition"),
            "native F12 must send the request"
        );
    }

    fn release(&self) {
        assert_eq!(shared_request(&self.client, "test/definition/release"), 1);
    }
}

impl Drop for NavigationWorkbench {
    fn drop(&mut self) {
        self.app
            .world_mut()
            .non_send_mut::<DocumentSession>()
            .shutdown(ClosePolicy::Discard)
            .unwrap();
        let _ = &self.fixture;
    }
}

fn workbench_single_definition_returns_to_requesting_view_without_references() {
    let mut fixture = NavigationWorkbench::new();
    let bytes = fixture.session().snapshot(fixture.document).unwrap().bytes;
    fixture.configure(fixture.location(1, 2));
    fixture.shortcut();
    let other = fixture
        .session()
        .view_snapshot(fixture.sibling)
        .unwrap()
        .selections;
    fixture.focus(fixture.sibling_tab);
    fixture.release();
    assert_eq!(
        fixture.panel_count(bedtime_plugin_editor::REFERENCES_PANEL_TYPE),
        0
    );
    fixture.frame();
    assert_eq!(fixture.active_view(), Some(fixture.requesting));
    let view = fixture.session().view_snapshot(fixture.requesting).unwrap();
    assert_eq!((view.row, view.column), (1, 2));
    assert_eq!(
        fixture
            .session()
            .view_snapshot(fixture.sibling)
            .unwrap()
            .selections,
        other
    );
    assert_eq!(
        fixture.panel_count(bedtime_plugin_editor::REFERENCES_PANEL_TYPE),
        0
    );
    assert_eq!(
        fixture.session().snapshot(fixture.document).unwrap().bytes,
        bytes
    );
}

fn workbench_cross_file_definition_uses_a_preview_and_preserves_same_file_tabs() {
    let mut fixture = NavigationWorkbench::new();
    let source_id = fixture
        .app
        .world()
        .get::<bedtime_api::workbench::PanelPresentation>(fixture.request_tab)
        .unwrap()
        .id;
    let sibling_id = fixture
        .app
        .world()
        .get::<bedtime_api::workbench::PanelPresentation>(fixture.sibling_tab)
        .unwrap()
        .id;
    let source_area = {
        let mut layout = fixture
            .app
            .world_mut()
            .resource_mut::<bedtime_workbench::WorkbenchLayout>();
        let tiling = &mut layout.tiling;
        let source_area = tiling.area_for(source_id).unwrap();
        let rect = tiling.layout.area(source_area).rect;
        let split = rect.min[0] + (rect.max[0] - rect.min[0]) * 3 / 10;
        let other_area = tiling.layout.split(source_area, 0, split, 100).unwrap();
        tiling.add_area(other_area);
        tiling.assign(sibling_id, other_area);
        assert_ne!(source_area, tiling.layout.largest_area());
        source_area
    };
    let path = fixture.fixture.root.join("definition.rs");
    fs::write(&path, "// target\nfn definition() {}\n").unwrap();
    fixture.configure(json!({
        "uri": LspUri::file_uri_from_path(path.to_str().unwrap()).unwrap().to_string(),
        "range": {"start": {"line": 1, "character": 3}, "end": {"line": 1, "character": 13}}
    }));
    fixture.shortcut();
    fixture.focus(fixture.sibling_tab);
    fixture.release();
    wait_until(|| {
        fixture.frame();
        fixture.active_view().is_some_and(|view| {
            let document = fixture.session().document_for_view(view).unwrap();
            fixture.session().snapshot(document).unwrap().path
                == fs::canonicalize(&path).unwrap().to_string_lossy()
        })
    });
    let panel = fixture
        .app
        .world()
        .resource::<bedtime_workbench::WorkbenchLayout>()
        .focused
        .unwrap();
    let id = fixture
        .app
        .world()
        .get::<bedtime_api::workbench::PanelPresentation>(panel)
        .unwrap()
        .id;
    assert_eq!(
        fixture
            .app
            .world()
            .resource::<bedtime_workbench::WorkbenchLayout>()
            .tiling
            .area_for(id),
        Some(source_area),
        "the preview belongs beside the requesting editor, even after focus moves to a larger panel"
    );
    assert!(
        fixture
            .app
            .world()
            .resource::<bedtime_workbench::WorkbenchLayout>()
            .tiling
            .areas
            .iter()
            .any(|group| group.preview == Some(id))
    );
    assert!(fixture.app.world().entities().contains(fixture.request_tab));
    assert!(fixture.app.world().entities().contains(fixture.sibling_tab));
    let view = fixture
        .app
        .world()
        .get::<bedtime_api::documents::PanelBinding>(panel)
        .unwrap()
        .view
        .unwrap();
    let snapshot = fixture.session().view_snapshot(view).unwrap();
    assert_eq!((snapshot.row, snapshot.column), (1, 3));
    assert_eq!(
        fixture.panel_count(bedtime_plugin_editor::REFERENCES_PANEL_TYPE),
        0
    );

    fixture.configure(fixture.location(1, 2));
    fixture.shortcut();
    fixture.release();
    wait_until(|| {
        fixture.frame();
        let snapshot = fixture.session().view_snapshot(fixture.requesting).unwrap();
        (snapshot.row, snapshot.column) == (1, 2)
    });
    assert_eq!(fixture.active_view(), Some(fixture.requesting));
    assert!(
        fixture
            .app
            .world()
            .resource::<bedtime_workbench::WorkbenchLayout>()
            .tiling
            .areas
            .iter()
            .any(|group| group.preview == Some(id)),
        "same-file definition jumps must leave the source kept and preserve the other preview"
    );
}

fn workbench_multiple_and_stale_definitions_use_guarded_routes() {
    let mut fixture = NavigationWorkbench::new();
    fixture.configure(json!([fixture.location(1, 2), fixture.location(2, 0)]));
    fixture.shortcut();
    fixture.release();
    fixture.frame();
    assert_eq!(
        fixture.panel_count(bedtime_plugin_editor::REFERENCES_PANEL_TYPE),
        1
    );
    assert_eq!(
        fixture
            .session()
            .view_snapshot(fixture.requesting)
            .unwrap()
            .row,
        0
    );
    let active = fixture
        .app
        .world()
        .resource::<bedtime_workbench::WorkbenchLayout>()
        .focused
        .unwrap();
    fixture.close(active);
    assert_eq!(
        fixture.panel_count(bedtime_plugin_editor::REFERENCES_PANEL_TYPE),
        0
    );
    fixture.configure(fixture.location(1, 2));
    fixture.shortcut();
    let requesting = fixture.requesting;
    fixture
        .session_mut()
        .with_commands(requesting, |commands| commands.type_text(b"x"))
        .unwrap();
    let expected = fixture
        .session()
        .view_snapshot(fixture.requesting)
        .unwrap()
        .selections;
    fixture.focus(fixture.sibling_tab);
    assert!(matches!(
        fixture
            .app
            .world()
            .non_send::<bedtime_plugin_editor::EditorRuntime>()
            .navigation_outcome()
            .map(|outcome| &outcome.kind),
        Some(bedtime_plugin_editor::NavigationOutcomeKind::Cancelled)
    ));
    assert!(
        shared_request(&fixture.client, "test/transcript")
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["method"] == "$/cancelRequest")
    );
    fixture.release();
    fixture.frame();
    assert_eq!(fixture.active_view(), Some(fixture.sibling));
    assert_eq!(
        fixture
            .session()
            .view_snapshot(fixture.requesting)
            .unwrap()
            .selections,
        expected
    );
    assert_eq!(
        fixture.panel_count(bedtime_plugin_editor::REFERENCES_PANEL_TYPE),
        0
    );
    drop(fixture);
    let mut fixture = NavigationWorkbench::new();
    fixture.configure(fixture.location(1, 2));
    fixture.shortcut();
    fixture.focus(fixture.sibling_tab);
    fixture.close(fixture.request_tab);
    assert!(matches!(
        fixture
            .app
            .world()
            .non_send::<bedtime_plugin_editor::EditorRuntime>()
            .navigation_outcome()
            .map(|outcome| &outcome.kind),
        Some(bedtime_plugin_editor::NavigationOutcomeKind::Cancelled)
    ));
    assert!(
        shared_request(&fixture.client, "test/transcript")
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["method"] == "$/cancelRequest")
    );
    assert!(
        fixture
            .session()
            .document_for_view(fixture.requesting)
            .is_none()
    );
    let expected = fixture
        .session()
        .view_snapshot(fixture.sibling)
        .unwrap()
        .selections;
    fixture.release();
    fixture.frame();
    assert_eq!(fixture.active_view(), Some(fixture.sibling));
    assert_eq!(
        fixture
            .session()
            .view_snapshot(fixture.sibling)
            .unwrap()
            .selections,
        expected
    );
    assert_eq!(
        fixture.panel_count(bedtime_plugin_editor::REFERENCES_PANEL_TYPE),
        0
    );
}

fn workbench_primary_click_sends_clicked_utf16_position() {
    use bedtime_editor_widgets::views::view_layout::{glyph_advance, line_column_x};
    use dear_imgui_rs::{Key, MouseButton, sys};
    let mut fixture = NavigationWorkbench::new();
    fixture.configure(fixture.location(1, 2));
    fixture
        .context
        .prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
            [1600.0, 1000.0],
            1.0 / 60.0,
        ));
    let ui = fixture.context.frame();
    headless_host::draw_ui(fixture.app.world_mut(), ui).unwrap();
    let name = std::ffi::CString::new(format!(
        "###bed_tab_{}",
        fixture
            .app
            .world()
            .get::<bedtime_api::workbench::PanelPresentation>(fixture.request_tab)
            .unwrap()
            .id
    ))
    .unwrap();
    let cursor_start = ui.with_bound_context(|| unsafe {
        let parent = sys::igFindWindowByName(name.as_ptr());
        assert!(!parent.is_null());
        let native = &*sys::igGetCurrentContext();
        (0..native.Windows.Size)
            .find_map(|index| {
                let window = &**native.Windows.Data.add(index as usize);
                let mut ancestor = window.ParentWindow;
                while !ancestor.is_null() && ancestor != parent {
                    ancestor = (*ancestor).ParentWindow;
                }
                (ancestor == parent
                    && std::ffi::CStr::from_ptr(window.Name)
                        .to_string_lossy()
                        .contains("##editor"))
                .then_some([window.DC.CursorStartPos.x, window.DC.CursorStartPos.y])
            })
            .expect("requesting view has an actual native canvas")
    });
    let font_size = fixture
        .app
        .world()
        .resource::<bedtime_api::settings::SettingsSnapshot>()
        .get("bedtime.ui", "fontSize")
        .unwrap()
        .as_f64()
        .unwrap() as f32;
    let font = ui.push_font_with_size(
        fixture
            .app
            .world()
            .non_send::<bedtime_ui::font::UiFonts>()
            .main,
        font_size,
    );
    let point = [
        line_column_x(
            ui,
            "é🙂symbol".as_bytes(),
            6,
            cursor_start[0] + font_size * 0.35,
        ) + glyph_advance(ui, "s") * 0.2,
        cursor_start[1] + font_size * 0.1 + ui.text_line_height() * 0.35,
    ];
    drop(font);
    drop(fixture.context.render_legacy());
    let before = fixture
        .session()
        .view_snapshot(fixture.requesting)
        .unwrap()
        .selections;
    let other = fixture
        .session()
        .view_snapshot(fixture.sibling)
        .unwrap()
        .selections;
    let modifier = if cfg!(target_os = "macos") {
        Key::ModSuper
    } else {
        Key::ModCtrl
    };
    fixture.context.io_mut().add_key_event(modifier, true);
    fixture.context.io_mut().add_mouse_pos_event(point);
    fixture
        .context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, true);
    fixture.frame();
    fixture
        .context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, false);
    fixture.context.io_mut().add_key_event(modifier, false);
    fixture.frame();
    assert_eq!(
        fixture
            .session()
            .view_snapshot(fixture.requesting)
            .unwrap()
            .selections,
        before
    );
    assert_eq!(
        fixture
            .session()
            .view_snapshot(fixture.sibling)
            .unwrap()
            .selections,
        other
    );
    assert_eq!(
        fixture.panel_count(bedtime_plugin_editor::REFERENCES_PANEL_TYPE),
        0
    );
    let transcript = shared_request(&fixture.client, "test/transcript");
    let request = transcript
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["method"] == "textDocument/definition")
        .expect("native primary-click reaches LSP");
    assert_eq!(
        request["params"]["position"],
        json!({"line":0,"character":3})
    );
    fixture.release();
    fixture.frame();
    assert_eq!(
        fixture
            .session()
            .view_snapshot(fixture.requesting)
            .unwrap()
            .row,
        1
    );
    assert_eq!(
        fixture.panel_count(bedtime_plugin_editor::REFERENCES_PANEL_TYPE),
        0
    );
}

fn navigation_canvas_point(
    world: &bevy::prelude::World,
    ui: &dear_imgui_rs::Ui,
    tab: bevy::prelude::Entity,
) -> [f32; 2] {
    use bedtime_editor_widgets::views::view_layout::{glyph_advance, line_column_x};
    use dear_imgui_rs::sys;
    let name = std::ffi::CString::new(format!(
        "###bed_tab_{}",
        world
            .get::<bedtime_api::workbench::PanelPresentation>(tab)
            .unwrap()
            .id
    ))
    .unwrap();
    let start = ui.with_bound_context(|| unsafe {
        let parent = sys::igFindWindowByName(name.as_ptr());
        assert!(!parent.is_null());
        let native = &*sys::igGetCurrentContext();
        (0..native.Windows.Size)
            .find_map(|index| {
                let window = &**native.Windows.Data.add(index as usize);
                let mut ancestor = window.ParentWindow;
                while !ancestor.is_null() && ancestor != parent {
                    ancestor = (*ancestor).ParentWindow;
                }
                (ancestor == parent
                    && std::ffi::CStr::from_ptr(window.Name)
                        .to_string_lossy()
                        .contains("##editor"))
                .then_some([window.DC.CursorStartPos.x, window.DC.CursorStartPos.y])
            })
            .expect("requesting view has a native editor canvas")
    });
    let font_size = world
        .resource::<bedtime_api::settings::SettingsSnapshot>()
        .get("bedtime.ui", "fontSize")
        .unwrap()
        .as_f64()
        .unwrap() as f32;
    let _font = ui.push_font_with_size(
        world.non_send::<bedtime_ui::font::UiFonts>().main,
        font_size,
    );
    [
        line_column_x(ui, "é🙂symbol".as_bytes(), 6, start[0] + font_size * 0.35)
            + glyph_advance(ui, "s") * 0.2,
        start[1] + font_size * 0.1 + ui.text_line_height() * 0.35,
    ]
}

fn workbench_cold_navigation_retains_one_action_and_position() {
    use bedtime_editing::text_commands::CursorReveal;
    use dear_imgui_rs::{FramePrepareOptions, Key, MouseButton};
    for action in ["shortcut", "primary click", "context menu"] {
        let mut fixture =
            NavigationWorkbench::with_startup("navigation-cold", |app, context, view, tab| {
                bedtime_core::focus_panel(app.world_mut(), tab, true).unwrap();
                let mut point = [0.0; 2];
                // Draw native focus and hit geometry without polling discovery or the
                // initialize response. The user acts while the server is not ready.
                for _ in 0..2 {
                    context.prepare_frame(FramePrepareOptions::new([1600.0, 1000.0], 1.0 / 60.0));
                    let ui = context.frame();
                    headless_host::draw_ui(app.world_mut(), ui).unwrap();
                    point = navigation_canvas_point(app.world(), ui, tab);
                    drop(context.render_legacy());
                }
                let document = app
                    .world()
                    .non_send::<DocumentSession>()
                    .document_for_view(view)
                    .unwrap();
                assert!(
                    !app.world()
                        .non_send::<DocumentSession>()
                        .lsp()
                        .unwrap()
                        .client_for_document(document)
                        .is_some_and(|client| client.borrow().is_initialized())
                );
                let modifier = if cfg!(target_os = "macos") {
                    Key::ModSuper
                } else {
                    Key::ModCtrl
                };
                context
                    .io_mut()
                    .add_key_event(modifier, action == "primary click");
                if action != "shortcut" {
                    context.io_mut().add_mouse_pos_event(point);
                    context.io_mut().add_mouse_button_event(
                        if action == "context menu" {
                            MouseButton::Right
                        } else {
                            MouseButton::Left
                        },
                        true,
                    );
                } else {
                    context.io_mut().add_key_event(Key::F12, true);
                }
                context.prepare_frame(FramePrepareOptions::new([1600.0, 1000.0], 1.0 / 60.0));
                headless_host::draw_ui(app.world_mut(), context.frame()).unwrap();
                drop(context.render_legacy());
                context
                    .io_mut()
                    .add_mouse_button_event(MouseButton::Left, false);
                context
                    .io_mut()
                    .add_mouse_button_event(MouseButton::Right, false);
                context.io_mut().add_key_event(Key::F12, false);
                context.io_mut().add_key_event(modifier, false);
                context.prepare_frame(FramePrepareOptions::new([1600.0, 1000.0], 1.0 / 60.0));
                let ui = context.frame();
                headless_host::draw_ui(app.world_mut(), ui).unwrap();
                let menu_point = (action == "context menu").then(|| {
                    let viewer_menu = bedtime_api::documents::panel_viewer_menu(
                        app.world(),
                        tab,
                        bedtime_plugin_editor::TEXT_VIEWER,
                    );
                    assert!(
                        viewer_menu.as_ref().is_none_or(|menu| menu
                            .choices
                            .iter()
                            .all(|choice| choice.id == menu.current)),
                        "fixture has no alternate-viewer rows"
                    );
                    let prefix = if viewer_menu.is_some() { 4.0 } else { 0.0 };
                    let font_size = app
                        .world()
                        .resource::<bedtime_api::settings::SettingsSnapshot>()
                        .get("bedtime.ui", "fontSize")
                        .unwrap()
                        .as_f64()
                        .unwrap() as f32;
                    let activity = bedtime_plugin_editor::EditorRuntime::document_lsp_activity(
                        app.world().non_send::<DocumentSession>(),
                        document,
                    )
                    .is_some();
                    ui.with_bound_context(|| unsafe {
                        let native = &*dear_imgui_rs::sys::igGetCurrentContext();
                        assert!(native.OpenPopupStack.Size > 0);
                        let popup = &*(*native
                            .OpenPopupStack
                            .Data
                            .add(native.OpenPopupStack.Size as usize - 1))
                        .Window;
                        [
                            popup.Pos.x + popup.Size.x * 0.5,
                            popup.DC.CursorStartPos.y
                                + prefix
                                + if activity { font_size + 4.0 } else { 0.0 }
                                + font_size * 0.5,
                        ]
                    })
                });
                drop(context.render_legacy());
                if let Some(point) = menu_point {
                    context.io_mut().add_mouse_pos_event(point);
                    context
                        .io_mut()
                        .add_mouse_button_event(MouseButton::Left, true);
                    context.prepare_frame(FramePrepareOptions::new([1600.0, 1000.0], 1.0 / 60.0));
                    headless_host::draw_ui(app.world_mut(), context.frame()).unwrap();
                    drop(context.render_legacy());
                    context
                        .io_mut()
                        .add_mouse_button_event(MouseButton::Left, false);
                    context.prepare_frame(FramePrepareOptions::new([1600.0, 1000.0], 1.0 / 60.0));
                    headless_host::draw_ui(app.world_mut(), context.frame()).unwrap();
                    drop(context.render_legacy());
                }
                let activity = app
                    .world()
                    .non_send::<bedtime_plugin_editor::EditorRuntime>()
                    .navigation_activity()
                    .expect("one cold UI action is retained");
                assert_eq!(
                    (activity.view, activity.stage),
                    (
                        view,
                        bedtime_plugin_editor::NavigationStage::WaitingForServer
                    )
                );
                app.world_mut()
                    .non_send_mut::<DocumentSession>()
                    .with_commands(view, |commands| {
                        commands.set_cursor(2, 1, false, CursorReveal::Ensure)
                    })
                    .unwrap();
            });
        wait_until(|| {
            fixture.frame();
            fixture
                .app
                .world()
                .non_send::<bedtime_plugin_editor::EditorRuntime>()
                .navigation_activity()
                .is_none()
        });
        assert!(
            fixture
                .app
                .world()
                .non_send::<bedtime_plugin_editor::EditorRuntime>()
                .navigation_outcome()
                .is_none()
        );
        let view = fixture.session().view_snapshot(fixture.requesting).unwrap();
        assert_eq!((view.row, view.column), (1, 2));
        assert!(
            fixture
                .client
                .borrow()
                .progress()
                .iter()
                .any(|job| !job.finished)
        );
        let transcript = shared_request(&fixture.client, "test/transcript");
        let messages = transcript.as_array().unwrap();
        let definitions: Vec<_> = messages
            .iter()
            .filter(|message| message["method"] == "textDocument/definition")
            .collect();
        assert_eq!(definitions.len(), 1);
        assert_eq!(
            definitions[0]["params"]["position"],
            json!({"line":0,"character":if action == "shortcut" {1} else {3}})
        );
        let index = |method| {
            messages
                .iter()
                .position(|message| message["method"] == method)
                .unwrap()
        };
        assert!(index("initialized") < index("textDocument/didOpen"));
        assert!(index("textDocument/didOpen") < index("textDocument/definition"));
        assert_eq!(
            fixture.panel_count(bedtime_plugin_editor::REFERENCES_PANEL_TYPE),
            0
        );
    }
}

fn workbench_navigation_retries_only_transient_errors() {
    let mut fixture = NavigationWorkbench::new();
    shared_request_params(
        &fixture.client,
        "test/definition/configure",
        json!({"result":fixture.location(1,2),"hold":false,"errors":[-32801,-32802]}),
    );
    fixture.shortcut();
    wait_until(|| {
        fixture.frame();
        fixture
            .app
            .world()
            .non_send::<bedtime_plugin_editor::EditorRuntime>()
            .navigation_activity()
            .is_none()
    });
    assert_eq!(
        fixture
            .session()
            .view_snapshot(fixture.requesting)
            .unwrap()
            .row,
        1
    );
    assert!(
        fixture
            .app
            .world()
            .non_send::<bedtime_plugin_editor::EditorRuntime>()
            .navigation_outcome()
            .is_none()
    );
    let runtime = fixture
        .app
        .world()
        .non_send::<bedtime_plugin_editor::EditorRuntime>();
    let timing = runtime.navigation_timing().unwrap();
    assert!(
        timing
            .response_received
            .unwrap()
            .duration_since(timing.first_sent.unwrap())
            >= Duration::from_millis(200)
    );
    let transcript = shared_request(&fixture.client, "test/transcript");
    let requests: Vec<_> = transcript
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["method"] == "textDocument/definition")
        .collect();
    assert_eq!(requests.len(), 3);
    assert!(
        requests
            .iter()
            .all(|request| request["params"] == requests[0]["params"])
    );
    assert_eq!(
        fixture.panel_count(bedtime_plugin_editor::REFERENCES_PANEL_TYPE),
        0
    );
}

fn workbench_navigation_empty_errors_and_timeout_are_distinct() {
    use bedtime_plugin_editor::NavigationOutcomeKind;
    for (scenario, result, errors, hold, expected_requests) in [
        ("basic", Value::Null, json!([]), false, 1),
        ("basic", json!({"invalid":"location"}), json!([]), false, 1),
        ("basic", Value::Null, json!([-32603]), false, 1),
        (
            "basic",
            Value::Null,
            json!([-32801, -32802, -32801]),
            false,
            3,
        ),
        ("navigation-timeout", Value::Null, json!([]), true, 1),
    ] {
        let mut fixture = NavigationWorkbench::with_startup(scenario, |_, _, _, _| {});
        shared_request_params(
            &fixture.client,
            "test/definition/configure",
            json!({"result":result,"errors":errors,"hold":hold}),
        );
        fixture.shortcut();
        wait_until(|| {
            fixture.frame();
            fixture
                .app
                .world()
                .non_send::<bedtime_plugin_editor::EditorRuntime>()
                .navigation_activity()
                .is_none()
        });
        let outcome = &fixture
            .app
            .world()
            .non_send::<bedtime_plugin_editor::EditorRuntime>()
            .navigation_outcome()
            .unwrap()
            .kind;
        if hold {
            assert_eq!(outcome, &NavigationOutcomeKind::TimedOut);
        } else if result.is_null() && errors.as_array().unwrap().is_empty() {
            assert_eq!(outcome, &NavigationOutcomeKind::Empty);
        } else {
            assert!(
                matches!(outcome, NavigationOutcomeKind::Failed(message) if !message.is_empty()),
                "{outcome:?}"
            );
        }
        assert_eq!(
            fixture.panel_count(bedtime_plugin_editor::REFERENCES_PANEL_TYPE),
            0
        );
        assert_eq!(
            fixture
                .session()
                .view_snapshot(fixture.requesting)
                .unwrap()
                .row,
            0
        );
        let transcript = shared_request(&fixture.client, "test/transcript");
        let requests = transcript
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["method"] == "textDocument/definition")
            .count();
        assert_eq!(requests, expected_requests);
        if hold {
            assert!(
                transcript
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|message| message["method"] == "$/cancelRequest")
            );
        }
    }
}

fn workbench_latest_navigation_cancels_the_previous_rpc() {
    let mut fixture = NavigationWorkbench::new();
    fixture.configure(fixture.location(1, 2));
    fixture.shortcut();
    fixture.configure(fixture.location(2, 0));
    fixture.shortcut();
    let transcript = shared_request(&fixture.client, "test/transcript");
    let messages = transcript.as_array().unwrap();
    let definitions: Vec<_> = messages
        .iter()
        .filter(|message| message["method"] == "textDocument/definition")
        .collect();
    assert_eq!(definitions.len(), 2);
    assert!(
        messages
            .iter()
            .any(|message| message["method"] == "$/cancelRequest"
                && message["params"]["id"] == definitions[0]["id"])
    );
    assert_eq!(
        shared_request(&fixture.client, "test/definition/release"),
        2
    );
    fixture.frame();
    assert_eq!(
        fixture
            .session()
            .view_snapshot(fixture.requesting)
            .unwrap()
            .row,
        2
    );
    assert_eq!(
        fixture.panel_count(bedtime_plugin_editor::REFERENCES_PANEL_TYPE),
        0
    );
    drop(fixture);

    let mut fixture = NavigationWorkbench::new();
    fixture.configure(fixture.location(1, 2));
    fixture.shortcut();
    let before = fixture
        .session()
        .view_snapshot(fixture.requesting)
        .unwrap()
        .selections;
    fixture.session_mut().retry_language("rust").unwrap();
    fixture.frame();
    assert!(matches!(
        fixture
            .app
            .world()
            .non_send::<bedtime_plugin_editor::EditorRuntime>()
            .navigation_outcome()
            .map(|outcome| &outcome.kind),
        Some(bedtime_plugin_editor::NavigationOutcomeKind::Cancelled)
    ));
    wait_until(|| {
        fixture.frame();
        fixture.client.borrow().is_initialized()
    });
    assert_eq!(
        fixture
            .session()
            .view_snapshot(fixture.requesting)
            .unwrap()
            .selections,
        before
    );
    assert_eq!(
        fixture.panel_count(bedtime_plugin_editor::REFERENCES_PANEL_TYPE),
        0
    );
    assert!(
        !shared_request(&fixture.client, "test/transcript")
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["method"] == "textDocument/definition")
    );
}

fn typed_navigation_results_follow_the_requesting_view() {
    let mut fixture = PoolFixture::new(70, "basic");
    let path = fixture.path("navigation.rs");
    fixture
        .pool
        .register_document(DocumentId(1), &path, b"symbol", 1, "rust")
        .unwrap();
    fixture.ready();
    let mut editor = DocumentRuntime::new();
    editor.state.set_from_bytes(b"symbol");
    editor.state.path = path;
    let client = fixture.pool.client_for_document(DocumentId(1)).unwrap();
    let mut ui = LspUi::default();
    let origin = fixture
        .pool
        .request_origin(DocumentId(1), ViewId(2), 3, 0)
        .unwrap();
    assert!(ui.request_at(
        ContextLspAction::Definition,
        &mut client.borrow_mut(),
        &editor,
        origin
    ));
    assert!(
        ui.navigation_results(ContextLspAction::Definition)
            .unwrap()
            .pending
    );
    assert!(!ui.is_overlay_visible());
    wait_until(|| {
        fixture.pool.poll();
        !ui.navigation_results(ContextLspAction::Definition)
            .unwrap()
            .pending
    });
    let definition = ui.navigation_results(ContextLspAction::Definition).unwrap();
    assert_eq!(definition.locations.len(), 1);
    assert_eq!(definition.origin.unwrap().view_id, ViewId(2));
    let other = LspRequestOrigin {
        view_id: ViewId(4),
        ..origin
    };
    assert!(ui.request_at(
        ContextLspAction::References,
        &mut client.borrow_mut(),
        &editor,
        other
    ));
    wait_until(|| {
        fixture.pool.poll();
        !ui.navigation_results(ContextLspAction::References)
            .unwrap()
            .pending
    });
    let references = ui.navigation_results(ContextLspAction::References).unwrap();
    assert_eq!(references.locations.len(), 2);
    assert_eq!(references.origin.unwrap().view_id, ViewId(4));
    ui.retain_requests(|origin| origin.view_id == ViewId(4));
    assert!(
        ui.navigation_results(ContextLspAction::Definition)
            .unwrap()
            .origin
            .is_none()
    );
    assert_eq!(
        ui.navigation_results(ContextLspAction::References)
            .unwrap()
            .locations
            .len(),
        2
    );
    ui.cancel_requests();
    assert!(
        ui.navigation_results(ContextLspAction::References)
            .unwrap()
            .origin
            .is_none()
    );
}

fn work_done_progress_is_main_thread_owned_and_resets() {
    let mut fixture = Fixture::new("basic");
    fixture.ready();
    let reply = Rc::new(RefCell::new(None));
    let received = Rc::clone(&reply);
    fixture
        .client
        .send_request("test/progress", json!({}), move |result| {
            *received.borrow_mut() = Some(result)
        })
        .unwrap();
    thread::sleep(Duration::from_millis(30));
    assert!(
        fixture.client.progress().is_empty(),
        "workers must not mutate progress before main-thread polling"
    );
    assert!(reply.borrow().is_none());
    let mut protocol_errors = Vec::new();
    wait_until(|| {
        protocol_errors.extend(
            fixture
                .client
                .poll()
                .into_iter()
                .filter(|event| matches!(event, RpcEvent::ProtocolError(_))),
        );
        reply.borrow().is_some()
    });
    assert!(reply.borrow_mut().take().unwrap().is_ok());
    assert!(
        protocol_errors.is_empty(),
        "partial/custom progress must be ignored: {protocol_errors:?}"
    );
    let jobs = fixture.client.progress();
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0].token, ProgressToken::String("index".into()));
    assert_eq!(jobs[0].title, "Indexing");
    assert_eq!(jobs[0].message.as_deref(), Some("Second crate"));
    assert_eq!(jobs[0].percentage, Some(50));
    assert!(!jobs[0].finished);
    assert_eq!(jobs[1].token, ProgressToken::Number(17));
    fixture.request("test/progress/end", json!({})).unwrap();
    assert!(fixture.client.progress().iter().all(|job| job.finished));
    assert_eq!(
        fixture.client.progress()[0].message.as_deref(),
        Some("Indexed")
    );
    fixture.client.shutdown();
    assert!(fixture.client.progress().is_empty());
    assert!(fixture.client.init(&fixture.document).unwrap());
    fixture.ready();
    assert!(fixture.client.progress().is_empty());
    fixture.request("test/progress", json!({})).unwrap();
    assert!(!fixture.client.progress().is_empty());
    assert_eq!(
        fixture.request("test/crash", json!({})).unwrap_err().code,
        CONNECTION_CLOSED
    );
    assert!(
        fixture.client.progress().is_empty(),
        "disconnect must clear stale loading activity"
    );

    let mut pool = PoolFixture::new(90, "basic");
    pool.pool
        .register_document(
            DocumentId(1),
            &pool.path("progress.rs"),
            b"fn main() {}",
            0,
            "rust",
        )
        .unwrap();
    pool.ready();
    let client = pool.pool.client_for_document(DocumentId(1)).unwrap();
    let reply = Rc::new(RefCell::new(None));
    let received = Rc::clone(&reply);
    client
        .borrow_mut()
        .send_request("test/progress", json!({}), move |result| {
            *received.borrow_mut() = Some(result)
        })
        .unwrap();
    wait_until(|| {
        pool.pool.poll();
        reply.borrow().is_some()
    });
    assert!(reply.borrow_mut().take().unwrap().is_ok());
    assert_eq!(
        pool.pool
            .server_statuses()
            .into_iter()
            .find(|status| status.language == "rust")
            .unwrap()
            .progress,
        client.borrow().progress()
    );
    // The actual docked dashboard body must draw active server work inside the
    // caller's native window, then stop presenting it as active after end.
    {
        use bedtime_plugin_editor::lsp::lsp_dashboard::LspDashboard;
        use dear_imgui_rs::{Condition, Context, FramePrepareOptions};
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut dashboard = LspDashboard::default();
        let draw =
            |context: &mut Context, dashboard: &mut LspDashboard, pool: &mut WorkspaceLsp| {
                context.prepare_frame(FramePrepareOptions::new([1000.0, 700.0], 1.0 / 60.0));
                let ui = context.frame();
                ui.window("Work-done dashboard")
                    .position([30.0, 30.0], Condition::Always)
                    .size([900.0, 600.0], Condition::Always)
                    .build(|| {
                        let geometry = (ui.window_pos(), ui.window_size());
                        assert!(dashboard.render_workspace_body(ui, pool).is_none());
                        assert_eq!(geometry, (ui.window_pos(), ui.window_size()));
                    });
                let frame = context.render_legacy();
                let draw = frame.draw_data();
                assert!(draw.total_vtx_count() > 0);
                draw.draw_lists()
                    .map(|list| {
                        list.vtx_buffer()
                            .iter()
                            .filter(|vertex| vertex.col == u32::from_le_bytes([230, 204, 102, 255]))
                            .count()
                    })
                    .sum::<usize>()
            };
        draw(&mut context, &mut dashboard, &mut pool.pool);
        assert!(
            draw(&mut context, &mut dashboard, &mut pool.pool) > 0,
            "active workspace progress must be visible in the docked dashboard"
        );
        let done = Rc::new(RefCell::new(false));
        let received = Rc::clone(&done);
        client
            .borrow_mut()
            .send_request("test/progress/end", json!({}), move |result| {
                result.unwrap();
                *received.borrow_mut() = true;
            })
            .unwrap();
        wait_until(|| {
            pool.pool.poll();
            *done.borrow()
        });
        assert_eq!(
            draw(&mut context, &mut dashboard, &mut pool.pool),
            0,
            "finished progress must not remain visually active"
        );
    }
    assert!(pool.pool.retry_language("rust").unwrap());
    assert!(client.borrow().progress().is_empty());
}
