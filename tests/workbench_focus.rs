//! Regression tests compose real Bevy plugins and their canonical resources.
#![cfg(feature = "editor")]
#[path = "support/editor_host.rs"]
mod editor_host;
use bed_document_session::editor_session::{DocumentId, EditorSession, ViewId};
use bed_editing::editor_commands::CursorReveal;
use bed_editor_api::{EditorPanelBinding, PanelTarget};
use bed_editor_workbench::{
    EditorAction, EditorNavigation, EditorPanel as PanelPresentation, EditorRequestOutcome,
    EditorWorkbenchOptions, EditorWorkbenchPlugin, WindowCommand, WorkbenchLayout,
    capture_editor_request, capture_editor_request_from, dispatch_editor_command,
    poll_editor_request, process_editor_requests, submit_editor_request,
};
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
            bed_config::Settings::with_paths(
                root.join("config"),
                PathBuf::from(env!("CARGO_MANIFEST_DIR")),
            )
            .unwrap(),
        )
        .unwrap();
        app.add_plugins(EditorWorkbenchPlugin {
            options: EditorWorkbenchOptions {
                working_directory: Some(root.clone()),
                ..Default::default()
            },
        });
        app.add_plugins(bed_editor::EditorPlugin);
        #[cfg(feature = "settings")]
        app.add_plugins(bed_settings::SettingsPlugin);
        #[cfg(feature = "explorer")]
        app.add_plugins(bed_explorer::ExplorerPlugin);
        app.update();
        Self { root, app }
    }
    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.root.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }
    fn request(&mut self, action: EditorAction) -> EditorRequestOutcome {
        let request = capture_editor_request(self.app.world(), action, PanelTarget::LastFocused);
        let id = submit_editor_request(self.app.world_mut(), request);
        process_editor_requests(self.app.world_mut()).unwrap();
        poll_editor_request(self.app.world_mut(), id)
            .expect("synchronous request receipt")
            .unwrap()
    }
    fn open(&mut self, path: &Path) -> (Entity, DocumentId, ViewId) {
        let EditorRequestOutcome::Opened {
            panel,
            document: Some(document),
        } = self.request(EditorAction::OpenFile(path.to_owned()))
        else {
            panic!("document open receipt")
        };
        (
            panel,
            document,
            self.app
                .world()
                .get::<EditorPanelBinding>(panel)
                .unwrap()
                .view
                .unwrap(),
        )
    }
    fn focus(&mut self, panel: Entity) {
        let request = capture_editor_request_from(
            self.app.world(),
            EditorAction::FocusPanel(panel),
            PanelTarget::LastFocused,
            Some(panel),
        );
        let id = submit_editor_request(self.app.world_mut(), request);
        process_editor_requests(self.app.world_mut()).unwrap();
        assert!(
            poll_editor_request(self.app.world_mut(), id)
                .unwrap()
                .is_ok()
        );
    }
    fn bytes(&self, document: DocumentId) -> Vec<u8> {
        self.app
            .world()
            .non_send::<EditorSession>()
            .snapshot(document)
            .unwrap()
            .bytes
    }
    fn frame(&mut self, context: &mut Context) {
        bed_editor_workbench::tick_editor_services(self.app.world_mut()).unwrap();
        editor_host::prepare_ui(self.app.world_mut(), context).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        context.prepare_frame(FramePrepareOptions::new([1200.0, 800.0], 1.0 / 60.0));
        bed_editor_workbench::draw_ui(self.app.world_mut(), context.frame()).unwrap();
        assert!(context.render_legacy().draw_data().total_vtx_count() > 0);
        process_editor_requests(self.app.world_mut()).unwrap();
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
fn duplicated_views_share_documents_and_transform_independent_carets() {
    let mut fixture = Fixture::new();
    let path = fixture.write("shared.txt", b"abcd");
    let (first, document, view) = fixture.open(&path);
    fixture
        .app
        .world_mut()
        .non_send_mut::<EditorSession>()
        .with_commands(view, |commands| {
            commands.set_cursor(0, 1, false, CursorReveal::Ensure)
        })
        .unwrap();
    fixture.request(EditorAction::Command(WindowCommand::DuplicateView));
    let sibling = fixture
        .app
        .world()
        .resource::<EditorNavigation>()
        .active_view
        .unwrap();
    assert_ne!(view, sibling);
    assert_eq!(
        fixture
            .app
            .world()
            .non_send::<EditorSession>()
            .view_count(document),
        2
    );
    fixture
        .app
        .world_mut()
        .non_send_mut::<EditorSession>()
        .with_commands(sibling, |commands| {
            commands.set_cursor(0, 4, false, CursorReveal::Ensure)
        })
        .unwrap();
    let revision = fixture
        .app
        .world()
        .non_send::<EditorSession>()
        .document_revision(document)
        .unwrap();
    fixture.request(EditorAction::ApplyEdits {
        document,
        revision,
        edits: vec![bed_document_session::editor_session::ByteEdit {
            range: 4..4,
            bytes: "🙂".as_bytes().to_vec(),
        }],
    });
    assert_eq!(fixture.bytes(document), "abcd🙂".as_bytes());
    assert_eq!(
        fixture
            .app
            .world()
            .non_send::<EditorSession>()
            .view_snapshot(view)
            .unwrap()
            .column,
        1
    );
    fixture.focus(first);
    assert_eq!(
        fixture
            .app
            .world()
            .resource::<EditorNavigation>()
            .active_view,
        Some(view)
    );
}
#[test]
fn panel_identity_is_live_entity_and_stable_persisted_number() {
    let mut fixture = Fixture::new();
    let path = fixture.write("first.txt", b"first");
    let (panel, _, _) = fixture.open(&path);
    let number = fixture
        .app
        .world()
        .get::<PanelPresentation>(panel)
        .unwrap()
        .id;
    let other = fixture.write("other.txt", b"other");
    let (sibling, _, _) = fixture.open(&other);
    assert_ne!(panel, sibling);
    assert_ne!(
        number,
        fixture
            .app
            .world()
            .get::<PanelPresentation>(sibling)
            .unwrap()
            .id
    );
    fixture.focus(panel);
    assert_eq!(
        fixture
            .app
            .world()
            .get::<PanelPresentation>(panel)
            .unwrap()
            .id,
        number
    );
    fixture.request(EditorAction::ClosePanel(sibling));
    assert!(
        fixture
            .app
            .world()
            .get::<PanelPresentation>(sibling)
            .is_none()
    );
    assert!(
        fixture
            .app
            .world()
            .get::<PanelPresentation>(panel)
            .is_some()
    );
}
#[test]
fn queued_source_commands_preserve_origin_after_focus_changes() {
    let mut fixture = Fixture::new();
    let first = fixture.write("first.txt", b"first");
    let (source, document, _) = fixture.open(&first);
    let request = capture_editor_request(
        fixture.app.world(),
        EditorAction::Command(WindowCommand::DuplicateView),
        PanelTarget::LastFocused,
    );
    let other = fixture.write("other.txt", b"other");
    let (_, other_document, _) = fixture.open(&other);
    let id = submit_editor_request(fixture.app.world_mut(), request);
    process_editor_requests(fixture.app.world_mut()).unwrap();
    assert!(
        poll_editor_request(fixture.app.world_mut(), id)
            .unwrap()
            .is_ok()
    );
    assert_eq!(
        fixture
            .app
            .world()
            .non_send::<EditorSession>()
            .view_count(document),
        2
    );
    assert_eq!(
        fixture
            .app
            .world()
            .non_send::<EditorSession>()
            .view_count(other_document),
        1
    );
    assert!(
        fixture
            .app
            .world()
            .get::<PanelPresentation>(source)
            .is_some()
    );
}
#[test]
fn closed_source_requests_do_not_retarget_current_document() {
    let mut fixture = Fixture::new();
    let first = fixture.write("first.txt", b"first");
    let (source, _, _) = fixture.open(&first);
    let request = capture_editor_request(
        fixture.app.world(),
        EditorAction::Command(WindowCommand::DuplicateView),
        PanelTarget::LastFocused,
    );
    fixture.request(EditorAction::ClosePanel(source));
    let other = fixture.write("other.txt", b"other");
    let (_, document, _) = fixture.open(&other);
    let id = submit_editor_request(fixture.app.world_mut(), request);
    process_editor_requests(fixture.app.world_mut()).unwrap();
    assert_eq!(
        bed_editor_workbench::poll_editor_request_result(fixture.app.world_mut(), id)
            .unwrap()
            .unwrap_err()
            .kind,
        std::io::ErrorKind::NotFound
    );
    assert_eq!(
        fixture
            .app
            .world()
            .non_send::<EditorSession>()
            .view_count(document),
        1
    );
}
#[test]
fn split_uses_canonical_layout_and_preserves_document_view_ownership() {
    let mut fixture = Fixture::new();
    let path = fixture.write("split.txt", b"split");
    let (source, document, view) = fixture.open(&path);
    fixture.request(EditorAction::Command(WindowCommand::SplitRight));
    let layout = fixture.app.world().resource::<WorkbenchLayout>();
    assert_eq!(layout.tiling.layout.areas.len(), 2);
    let destination = layout.focused.unwrap();
    assert_ne!(source, destination);
    let source_id = fixture
        .app
        .world()
        .get::<PanelPresentation>(source)
        .unwrap()
        .id;
    let dest_id = fixture
        .app
        .world()
        .get::<PanelPresentation>(destination)
        .unwrap()
        .id;
    assert_ne!(
        layout.tiling.area_for(source_id),
        layout.tiling.area_for(dest_id)
    );
    assert_eq!(
        fixture
            .app
            .world()
            .get::<EditorPanelBinding>(destination)
            .unwrap()
            .document,
        Some(document)
    );
    assert_ne!(
        fixture
            .app
            .world()
            .get::<EditorPanelBinding>(destination)
            .unwrap()
            .view,
        Some(view)
    );
}
#[test]
fn unicode_input_targets_selected_view_without_changing_other_documents() {
    let _lock = CONTEXT_LOCK.lock().unwrap();
    let mut context = context();
    let mut fixture = Fixture::new();
    let path = fixture.write("selected.txt", b"first");
    let (first, document, _) = fixture.open(&path);
    let path = fixture.write("inactive.txt", b"second");
    let (_, other, _) = fixture.open(&path);
    fixture.focus(first);
    fixture.frame(&mut context);
    fixture.frame(&mut context);
    context.io_mut().add_input_characters_utf8("雪");
    fixture.frame(&mut context);
    assert_eq!(fixture.bytes(document), "雪first".as_bytes());
    assert_eq!(fixture.bytes(other), b"second");
}
#[test]
fn presentation_drawing_keeps_session_settings_and_registry_installed() {
    let _lock = CONTEXT_LOCK.lock().unwrap();
    let mut context = context();
    let mut fixture = Fixture::new();
    let path = fixture.write("stable.txt", b"stable");
    fixture.open(&path);
    let session = fixture.app.world().non_send::<EditorSession>() as *const EditorSession;
    fixture.frame(&mut context);
    fixture.frame(&mut context);
    assert_eq!(
        fixture.app.world().non_send::<EditorSession>() as *const EditorSession,
        session
    );
    assert!(
        fixture
            .app
            .world()
            .contains_non_send::<bed_config::Settings>()
    );
    assert!(
        fixture
            .app
            .world()
            .contains_resource::<bed_editor_api::Registry>()
    );
}
#[test]
fn workspace_restore_focus_survives_first_presentation_frames() {
    let _lock = CONTEXT_LOCK.lock().unwrap();
    let mut context = context();
    let mut fixture = Fixture::new();
    fixture.request(EditorAction::AttachWorkspace(fixture.root.clone()));
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
    bed_editor_workbench::persist_workspace(fixture.app.world_mut()).unwrap();
    let mut restored = App::new();
    editor_host::install(
        &mut restored,
        bed_config::Settings::with_paths(
            fixture.root.join("config"),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        )
        .unwrap(),
    )
    .unwrap();
    restored
        .add_plugins(EditorWorkbenchPlugin {
            options: EditorWorkbenchOptions {
                working_directory: Some(fixture.root.clone()),
                workspace: Some(bed_editor_api::workspace::WorkspaceSpec::local(
                    fixture.root.to_str().unwrap(),
                )),
                ..Default::default()
            },
        })
        .add_plugins(bed_editor::EditorPlugin);
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
    bed_editor_workbench::draw_ui(restored.world_mut(), context.frame()).unwrap();
    drop(context.render_legacy());
    process_editor_requests(restored.world_mut()).unwrap();
    assert_eq!(
        restored.world().resource::<WorkbenchLayout>().focused,
        Some(focused)
    );
}
#[cfg(feature = "settings")]
#[test]
fn focusing_a_tool_preserves_document_carets() {
    let mut fixture = Fixture::new();
    let path = fixture.write("caret.txt", b"abcdef");
    let (panel, _, view) = fixture.open(&path);
    fixture
        .app
        .world_mut()
        .non_send_mut::<EditorSession>()
        .with_commands(view, |commands| {
            commands.set_cursor(0, 3, false, CursorReveal::Ensure)
        })
        .unwrap();
    dispatch_editor_command(
        fixture.app.world_mut(),
        WindowCommand::NewSettings,
        PanelTarget::LastFocused,
    )
    .unwrap();
    fixture.focus(panel);
    assert_eq!(
        fixture
            .app
            .world()
            .non_send::<EditorSession>()
            .view_snapshot(view)
            .unwrap()
            .column,
        3
    );
}

#[cfg(feature = "settings")]
#[test]
fn application_tab_shortcut_restores_editor_focus_from_a_tool() {
    let _lock = CONTEXT_LOCK.lock().unwrap();
    let mut context = context();
    let mut fixture = Fixture::new();
    let path = fixture.write("shortcut.txt", b"unchanged");
    let (panel, document, _) = fixture.open(&path);
    dispatch_editor_command(
        fixture.app.world_mut(),
        WindowCommand::NewSettings,
        PanelTarget::LastFocused,
    )
    .unwrap();
    fixture.frame(&mut context);
    fixture.frame(&mut context);
    let panels = bed_editor_workbench::panel_entities(fixture.app.world_mut());
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

#[test]
fn byte_view_save_shortcut_uses_the_document_save_pipeline() {
    let _lock = CONTEXT_LOCK.lock().unwrap();
    let mut context = context();
    let mut fixture = Fixture::new();
    let path = fixture.write("shortcut.bin", &[0xff, 0, 1]);
    let EditorRequestOutcome::Opened {
        panel,
        document: Some(document),
    } = fixture.request(EditorAction::OpenFileWithViewer {
        path: path.clone(),
        viewer: Some(bed_editor::HEX_VIEWER.into()),
        additional: false,
        move_existing: false,
    })
    else {
        panic!("byte document receipt")
    };
    assert!(
        fixture
            .app
            .world()
            .get::<EditorPanelBinding>(panel)
            .unwrap()
            .view
            .is_none()
    );
    let revision = fixture
        .app
        .world()
        .non_send::<EditorSession>()
        .document_revision(document)
        .unwrap();
    fixture.request(EditorAction::ApplyEdits {
        document,
        revision,
        edits: vec![bed_document_session::editor_session::ByteEdit {
            range: 1..2,
            bytes: vec![42],
        }],
    });
    fixture.frame(&mut context);
    fixture.frame(&mut context);
    context.io_mut().add_key_event(
        if cfg!(target_os = "macos") {
            dear_imgui_rs::Key::ModSuper
        } else {
            dear_imgui_rs::Key::ModCtrl
        },
        true,
    );
    context.io_mut().add_key_event(dear_imgui_rs::Key::S, true);
    fixture.frame(&mut context);
    assert_eq!(fs::read(path).unwrap(), [0xff, 42, 1]);
    assert!(
        !fixture
            .app
            .world()
            .non_send::<EditorSession>()
            .snapshot(document)
            .unwrap()
            .dirty
    );
}
