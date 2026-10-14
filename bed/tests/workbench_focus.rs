//! Regression tests compose real Bevy plugins and their canonical resources.
#[path = "support/application_requests.rs"]
mod application_requests;
use application_requests::*;

#[path = "support/editor_host.rs"]
mod editor_host;
use application_requests::dispatch_app_command;
use bed::application::{AppOptions, AppRuntimePlugin, WorkbenchLayout};
use bedtime_documents::session::{DocumentId, DocumentSession, ViewId};
use bevy::prelude::*;
use dear_imgui_rs::{Context, FramePrepareOptions};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use {
    bedtime_api::documents::PanelBinding,
    bedtime_api::workbench::{PanelPresentation, PanelTarget},
};
static CONTEXT_LOCK: Mutex<()> = Mutex::new(());

struct Fixture {
    root: PathBuf,
    app: App,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "bed-ecs-focus-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let mut app = App::new();
        editor_host::install(
            &mut app,
            bed::config::AppConfig::with_paths(
                root.join("config"),
                PathBuf::from(env!("CARGO_MANIFEST_DIR")),
            )
            .unwrap(),
        )
        .unwrap();
        app.add_plugins(AppRuntimePlugin {
            options: AppOptions {
                working_directory: Some(root.clone()),
                ..Default::default()
            },
        });
        app.add_plugins(bedtime_plugin_editor::EditorPlugin);
        app.add_plugins(bed::settings_view::BedSettingsPagesPlugin);
        app.add_plugins(bedtime_plugin_file_explorer::FileExplorerPlugin);
        app.update();
        Self { root, app }
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.root.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    fn request(&mut self, action: AppAction) -> AppRequestOutcome {
        let request = capture_app_request(self.app.world(), action, PanelTarget::LastFocused);
        let id = submit_app_request(self.app.world_mut(), request);
        process_app_requests(self.app.world_mut()).unwrap();
        poll_app_request(self.app.world_mut(), id)
            .expect("synchronous request receipt")
            .unwrap()
    }

    fn open(&mut self, path: &Path) -> (Entity, DocumentId, ViewId) {
        let AppRequestOutcome::Opened {
            panel,
            document: Some(document),
        } = self.request(AppAction::OpenFile(path.to_owned()))
        else {
            panic!("document open receipt")
        };
        (
            panel,
            document,
            self.app
                .world()
                .get::<PanelBinding>(panel)
                .unwrap()
                .view
                .unwrap(),
        )
    }

    fn focus(&mut self, panel: Entity) {
        let request = capture_app_request_from(
            self.app.world(),
            AppAction::FocusPanel(panel),
            PanelTarget::LastFocused,
            Some(panel),
        );
        let id = submit_app_request(self.app.world_mut(), request);
        process_app_requests(self.app.world_mut()).unwrap();
        assert!(poll_app_request(self.app.world_mut(), id).unwrap().is_ok());
    }

    fn bytes(&self, document: DocumentId) -> Vec<u8> {
        self.app
            .world()
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes
    }

    fn frame(&mut self, context: &mut Context) {
        application_requests::tick_app_services(self.app.world_mut()).unwrap();
        editor_host::prepare_ui(self.app.world_mut(), context).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        context.prepare_frame(FramePrepareOptions::new([1200.0, 800.0], 1.0 / 60.0));
        application_requests::draw_ui(self.app.world_mut(), context.frame()).unwrap();
        assert!(context.render_legacy().draw_data().total_vtx_count() > 0);
        process_app_requests(self.app.world_mut()).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn context() -> Context {
    let mut context = Context::create();
    context.set_ini_filename(None::<PathBuf>).unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    context
}

#[test]
fn workspace_restore_focus_survives_first_presentation_frames() {
    let _lock = CONTEXT_LOCK.lock().unwrap();
    let mut context = context();
    let mut fixture = Fixture::new();
    fixture.request(AppAction::AttachWorkspace(fixture.root.clone()));
    let path = fixture.write("first.txt", b"first");
    let (first, _, _) = fixture.open(&path);
    let path = fixture.write("last.txt", b"last");
    fixture.open(&path);
    fixture.focus(first);
    let number = fixture
        .app
        .world()
        .get::<PanelPresentation>(first)
        .unwrap()
        .id;
    bed::application::persist_workspace(fixture.app.world_mut()).unwrap();
    let mut restored = App::new();
    editor_host::install(
        &mut restored,
        bed::config::AppConfig::with_paths(
            fixture.root.join("config"),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        )
        .unwrap(),
    )
    .unwrap();
    restored
        .add_plugins(AppRuntimePlugin {
            options: AppOptions {
                working_directory: Some(fixture.root.clone()),
                workspace: Some(bedtime_plugin_workspaces::WorkspaceSpec::local(
                    fixture.root.to_str().unwrap(),
                )),
                ..Default::default()
            },
        })
        .add_plugins(bedtime_plugin_editor::EditorPlugin);
    restored.update();
    let focused = restored
        .world()
        .resource::<WorkbenchLayout>()
        .focused
        .unwrap();
    assert_eq!(
        restored
            .world()
            .get::<PanelPresentation>(focused)
            .unwrap()
            .id,
        number
    );
    editor_host::prepare_ui(restored.world_mut(), &mut context).unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    context.prepare_frame(FramePrepareOptions::new([1200.0, 800.0], 1.0 / 60.0));
    application_requests::draw_ui(restored.world_mut(), context.frame()).unwrap();
    drop(context.render_legacy());
    process_app_requests(restored.world_mut()).unwrap();
    assert_eq!(
        restored.world().resource::<WorkbenchLayout>().focused,
        Some(focused)
    );
}

#[test]
fn application_tab_shortcut_restores_editor_focus_from_a_tool() {
    let _lock = CONTEXT_LOCK.lock().unwrap();
    let mut context = context();
    let mut fixture = Fixture::new();
    let path = fixture.write("shortcut.txt", b"unchanged");
    let (panel, document, _) = fixture.open(&path);
    dispatch_app_command(
        fixture.app.world_mut(),
        WindowCommand::NewSettings,
        PanelTarget::LastFocused,
    )
    .unwrap();
    fixture.frame(&mut context);
    fixture.frame(&mut context);
    let panels = bedtime_core::panel_entities(fixture.app.world_mut());
    let index = panels
        .iter()
        .position(|candidate| *candidate == panel)
        .unwrap();
    let key = [
        dear_imgui_rs::Key::Key1,
        dear_imgui_rs::Key::Key2,
        dear_imgui_rs::Key::Key3,
        dear_imgui_rs::Key::Key4,
        dear_imgui_rs::Key::Key5,
        dear_imgui_rs::Key::Key6,
        dear_imgui_rs::Key::Key7,
        dear_imgui_rs::Key::Key8,
        dear_imgui_rs::Key::Key9,
    ][index];
    context.io_mut().add_key_event(
        if cfg!(target_os = "macos") {
            dear_imgui_rs::Key::ModSuper
        } else {
            dear_imgui_rs::Key::ModCtrl
        },
        true,
    );
    context
        .io_mut()
        .add_key_event(dear_imgui_rs::Key::ModShift, true);
    context.io_mut().add_key_event(key, true);
    fixture.frame(&mut context);
    assert_eq!(
        fixture.app.world().resource::<WorkbenchLayout>().focused,
        Some(panel)
    );
    context.io_mut().add_key_event(key, false);
    context.io_mut().add_key_event(
        if cfg!(target_os = "macos") {
            dear_imgui_rs::Key::ModSuper
        } else {
            dear_imgui_rs::Key::ModCtrl
        },
        false,
    );
    context
        .io_mut()
        .add_key_event(dear_imgui_rs::Key::ModShift, false);
    fixture.frame(&mut context);
    context.io_mut().add_input_characters_utf8("雪");
    fixture.frame(&mut context);
    assert_eq!(fixture.bytes(document), "雪unchanged".as_bytes());
}
