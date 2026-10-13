#![cfg(feature = "language-services")]

use bevy::prelude::*;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[path = "../../../tests/support/temp_dir.rs"]
#[allow(dead_code)]
mod test_support;

use test_support::TempDir as Fixture;
static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
#[ignore = "requires installed rust-analyzer and rust-src; renders real dependency definition jumps"]
fn rust_dependency_navigation_reuses_one_server_and_keeps_definitions_visible() {
    use bed_api::documents::PanelBinding;
    use bed_documents::{ClosePolicy, DocumentSession};
    use bed_plugin_editor::lsp::lsp_ui::ContextLspAction;
    use bed_workbench::WorkbenchLayout;
    use bevy::ecs::system::SystemState;
    use dear_imgui_rs::Context;

    fn frame(app: &mut App, context: &mut Context) {
        app.update();
        let frame = app.world().non_send::<bed_ui::UiRuntime>().frame_context();
        let ui = context.frame();
        app.world_mut()
            .non_send_mut::<bed_ui::UiRuntime>()
            .begin(ui);
        frame.scope(|| bed_workbench::render_ui(app.world_mut(), ui));
        app.world_mut().non_send_mut::<bed_ui::UiRuntime>().end(ui);
        drop(context.render_legacy());
        bed_core::resolve_documents(app.world_mut());
        std::thread::sleep(Duration::from_millis(10));
    }

    fn caret_visible(app: &App, panel: Entity) -> bool {
        let Some(surface) = app
            .world()
            .get::<bed_api::workbench::PanelFrontend>(panel)
            .map(|frontend| frontend.0)
        else {
            return false;
        };
        let Some(geometry) = app.world().get::<bed_workbench::api::PanelGeometry>(panel) else {
            return false;
        };
        app.world()
            .non_send::<bed_plugin_editor::EditorViews>()
            .text(surface)
            .is_some_and(|view| {
                let caret = view.presentation().caret_position;
                caret[1] > geometry.content_min[1] && caret[1] < geometry.content_max[1]
            })
    }

    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
    let fixture = Fixture::new();
    let root = fixture.0.join("rust-project");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"),
        "[package]\nname = \"bed_definition_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n").unwrap();
    let source = root.join("src/main.rs");
    std::fs::write(
        &source,
        "fn main() { let _value: String = String::new(); }\n",
    )
    .unwrap();
    let mut app = App::new();
    app.add_plugins((
        bevy::MinimalPlugins,
        bevy::input::InputPlugin,
        bed_core::DocumentHostPlugin,
        bed_plugin_editor::EditorPlugin,
    ));
    bed_core::settings::initialize_settings(app.world_mut(), None).unwrap();
    bed_core::settings::finalize_settings(app.world_mut()).unwrap();
    let lsp_config = fixture.0.join("lsp.json");
    std::fs::write(
        &lsp_config,
        serde_json::to_vec(&serde_json::json!({
            "language_servers":{"rust-analyzer":{"command":"rust-analyzer"}}
        }))
        .unwrap(),
    )
    .unwrap();
    app.insert_resource(bed_files::FileContext::local(&root).unwrap());
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .configure(bed_documents::SessionOptions {
            project_root: Some(root),
            lsp_config: Some(lsp_config),
            lsp_config_mode: bed_documents::session::LspConfigMode::Layered,
            highlighting: true,
            ..Default::default()
        })
        .unwrap();
    app.update();
    let panel = bed_core::open_file(app.world_mut(), &source, None, None).unwrap();
    let source_binding = app.world().get::<PanelBinding>(panel).unwrap().clone();
    let document = source_binding.document.unwrap();
    let view = source_binding.view.unwrap();
    let mut context = Context::create();
    let mut runtime = bed_ui::UiRuntime::new(&context);
    let mut fonts = bed_ui::font::UiFonts::default();
    runtime
        .prepare(
            &mut context,
            &bed_ui::appearance::UiTheme::default(),
            &mut fonts,
        )
        .unwrap();
    app.insert_non_send(runtime).insert_non_send(fonts);
    context.set_ini_filename(None::<PathBuf>).unwrap();
    context
        .io_mut()
        .set_config_flags(dear_imgui_rs::ConfigFlags::DOCKING_ENABLE);
    context.io_mut().set_display_size([1200., 800.]);
    context.io_mut().set_delta_time(1. / 60.);
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    // Prepare the fixture's initial workspace analysis. Each navigation below is
    // a single editor action; a null/error response fails that action rather
    // than making the test silently issue another definition request.
    let startup = Instant::now();
    let deadline = startup + Duration::from_secs(90);
    let mut settled_since = None;
    let mut progress = std::collections::BTreeSet::new();
    let client = loop {
        frame(&mut app, &mut context);
        let client = app
            .world()
            .non_send::<DocumentSession>()
            .lsp()
            .unwrap()
            .client_for_document(document);
        if let Some(client) = &client {
            let client = client.borrow();
            let jobs = client.progress();
            for job in &jobs {
                progress.insert(job.title.clone());
            }
            let ready = client.is_initialized()
                && !progress.is_empty()
                && jobs.iter().all(|job| job.finished);
            if ready {
                let since = settled_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= Duration::from_millis(750) {
                    drop(client);
                    break app
                        .world()
                        .non_send::<DocumentSession>()
                        .lsp()
                        .unwrap()
                        .client_for_document(document)
                        .unwrap();
                }
            } else {
                settled_since = None;
            }
        }
        assert!(
            Instant::now() < deadline,
            "initial workspace analysis did not settle: {progress:?}"
        );
    };
    let process = client.borrow().process_id().unwrap();
    eprintln!(
        "Initial workspace analysis: {:?}; progress: {progress:?}",
        startup.elapsed()
    );
    let mut string_binding = None;
    let mut string_source = String::new();
    for step in 0..3 {
        let (view, position, suffix, declaration) = if step == 1 {
            let binding: &PanelBinding = string_binding.as_ref().unwrap();
            let (row, line) = string_source
                .lines()
                .enumerate()
                .find(|(_, line)| line.contains("vec: Vec<u8>"))
                .unwrap();
            (
                binding.view.unwrap(),
                (row as i32, (line.find("Vec").unwrap() + 1) as i32),
                "alloc/src/vec/mod.rs",
                "pub struct Vec<",
            )
        } else {
            (view, (0, 25), "alloc/src/string.rs", "pub struct String")
        };
        let requested = Instant::now();
        {
            let mut state = SystemState::<(
                NonSendMut<bed_plugin_editor::EditorRuntime>,
                NonSendMut<DocumentSession>,
            )>::new(app.world_mut());
            let (mut runtime, mut session) = state.get_mut(app.world_mut()).unwrap();
            runtime
                .request_lsp(
                    &mut session,
                    view,
                    ContextLspAction::Definition,
                    Some(position),
                )
                .unwrap();
        }
        let definition = loop {
            frame(&mut app, &mut context);
            let runtime = app.world().non_send::<bed_plugin_editor::EditorRuntime>();
            assert!(
                runtime.navigation_outcome().is_none(),
                "single navigation action failed: {:?}",
                runtime.navigation_outcome()
            );
            let focused = app.world().resource::<WorkbenchLayout>().focused.unwrap();
            let binding = app.world().get::<PanelBinding>(focused).unwrap();
            if binding.document.is_some_and(|document| {
                app.world()
                    .non_send::<DocumentSession>()
                    .snapshot(document)
                    .unwrap()
                    .path
                    .ends_with(suffix)
            }) {
                break focused;
            }
            assert!(
                requested.elapsed() < Duration::from_secs(20),
                "single navigation timed out: {:?}",
                runtime.navigation_activity()
            );
        };
        // Let asynchronous syntax folds and the centering animation finish.
        let mut first_visible = caret_visible(&app, definition).then(|| requested.elapsed());
        for _ in 0..60 {
            frame(&mut app, &mut context);
            if first_visible.is_none() && caret_visible(&app, definition) {
                first_visible = Some(requested.elapsed());
            }
        }
        let binding = app.world().get::<PanelBinding>(definition).unwrap().clone();
        let session = app.world().non_send::<DocumentSession>();
        let definition_path = session.snapshot(binding.document.unwrap()).unwrap().path;
        let contents = std::fs::read_to_string(&definition_path).unwrap();
        let expected_row = contents
            .lines()
            .position(|line| line.contains(declaration))
            .unwrap() as i32;
        let snapshot = session.view_snapshot(binding.view.unwrap()).unwrap();
        assert_eq!(snapshot.row, expected_row);
        assert!(snapshot.scroll_position[1] > 0.0);
        assert!(
            !snapshot.folds.ranges().is_empty(),
            "real syntax folds never arrived"
        );
        let destination_client = session
            .lsp()
            .unwrap()
            .client_for_document(binding.document.unwrap())
            .unwrap();
        assert!(
            std::rc::Rc::ptr_eq(&client, &destination_client),
            "dependency navigation forked a server"
        );
        let statuses = session.lsp().unwrap().server_statuses();
        let running: Vec<_> = statuses
            .iter()
            .filter(|status| status.process_id.is_some())
            .collect();
        assert_eq!(running.len(), 1, "unexpected extra servers: {running:?}");
        assert_eq!(running[0].process_id, Some(process));
        let surface = app
            .world()
            .get::<bed_api::workbench::PanelFrontend>(definition)
            .map(|frontend| frontend.0)
            .unwrap();
        let views = app.world().non_send::<bed_plugin_editor::EditorViews>();
        let caret = views.text(surface).unwrap().presentation().caret_position;
        let geometry = app
            .world()
            .get::<bed_workbench::api::PanelGeometry>(definition)
            .unwrap();
        assert!(
            caret[1] > geometry.content_min[1] && caret[1] < geometry.content_max[1],
            "definition is outside the actual rendered viewport: {snapshot:?}"
        );
        let timing = app
            .world()
            .non_send::<bed_plugin_editor::EditorRuntime>()
            .navigation_timing()
            .unwrap();
        let sent = timing.first_sent.unwrap().duration_since(timing.started);
        let response = timing
            .response_received
            .unwrap()
            .duration_since(timing.started);
        eprintln!(
            "Single action {step}: {suffix} row {expected_row}; action→send {sent:?}, action→response {response:?}, action→visible {:?}; caret {caret:?}; server PID {process}",
            first_visible.unwrap()
        );
        let notices = app
            .world_mut()
            .resource_mut::<Messages<bed_core::CoreNotice>>()
            .drain()
            .collect::<Vec<_>>();
        assert!(notices.is_empty(), "core notices: {notices:?}");
        if step == 0 {
            string_binding = Some(binding.clone());
            string_source = contents;
        }
    }
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .shutdown(ClosePolicy::Discard)
        .unwrap();
}
