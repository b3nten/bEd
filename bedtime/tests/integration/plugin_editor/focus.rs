//! Editor behavior composed with the shared document host, without bEd policy.
use crate::IMGUI_TEST_LOCK as CONTEXT_LOCK;
use crate::headless_host;
use bedtime_api::{
    documents::PanelBinding,
    features::HostAction,
    workbench::{PanelPresentation, WorkbenchAction},
};
use bedtime_core::{DocumentNavigation, HostOutcome};
use bedtime_documents::{DocumentId, DocumentSession, ViewId};
use bedtime_editing::text_commands::CursorReveal;
use bedtime_workbench::WorkbenchLayout;
use bevy::prelude::*;
use dear_imgui_rs::{Context, FramePrepareOptions};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
struct Tool;
impl bedtime_api::workbench::Panel for Tool {
    fn title(&self, _: &World) -> String {
        "Tool".into()
    }
    fn draw(
        &mut self,
        _: &dear_imgui_rs::Ui,
        _: &mut World,
        _: &mut Vec<WorkbenchAction>,
        _: &bedtime_api::workbench::ShortcutInput,
    ) {
    }
}
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
        headless_host::install(&mut app, &root.join("config"));
        app.add_plugins(bedtime_plugin_editor::EditorPlugin);
        bedtime_workbench::register(app.world_mut(), "fixture", |registrar| {
            registrar.panel("fixture.tool", "Tool", |_: In<bedtime_api::workbench::PanelCreation>| -> bedtime_api::workbench::PanelResult { Ok(Box::new(Tool)) });
        }).unwrap();
        headless_host::finish(&mut app, &root);
        Self { root, app }
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.root.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    fn request(&mut self, action: HostAction) -> HostOutcome {
        let source = self.app.world().resource::<WorkbenchLayout>().focused;
        headless_host::host(self.app.world_mut(), source, action).unwrap()
    }
    fn command(&mut self, command: &str) {
        let source = self.app.world().resource::<WorkbenchLayout>().focused;
        headless_host::invoke(self.app.world_mut(), command, source).unwrap();
    }
    fn open(&mut self, path: &Path) -> (Entity, DocumentId, ViewId) {
        let HostOutcome::Opened(panel) = self.request(HostAction::OpenFile {
            path: path.to_string_lossy().into_owned(),
            viewer: None,
        }) else {
            panic!("document open receipt")
        };
        let binding = self.app.world().get::<PanelBinding>(panel).unwrap();
        (panel, binding.document.unwrap(), binding.view.unwrap())
    }
    fn focus(&mut self, panel: Entity) {
        headless_host::workbench(self.app.world_mut(), WorkbenchAction::FocusPanel(panel)).unwrap();
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
        self.app.update();
        headless_host::prepare_ui(self.app.world_mut(), context).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        context.prepare_frame(FramePrepareOptions::new([1200.0, 800.0], 1.0 / 60.0));
        headless_host::draw_ui(self.app.world_mut(), context.frame()).unwrap();
        assert!(context.render_legacy().draw_data().total_vtx_count() > 0);
        bedtime_core::resolve_documents(self.app.world_mut());
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
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut fixture = Fixture::new();
    let path = fixture.write("shared.txt", b"abcd");
    let (first, document, view) = fixture.open(&path);
    fixture
        .app
        .world_mut()
        .non_send_mut::<DocumentSession>()
        .with_commands(view, |commands| {
            commands.set_cursor(0, 1, false, CursorReveal::Ensure)
        })
        .unwrap();
    fixture.command("bedtime.workbench.duplicate_view");
    let sibling = fixture
        .app
        .world()
        .resource::<DocumentNavigation>()
        .active_view
        .unwrap();
    assert_ne!(view, sibling);
    assert_eq!(
        fixture
            .app
            .world()
            .non_send::<DocumentSession>()
            .view_count(document),
        2
    );
    fixture
        .app
        .world_mut()
        .non_send_mut::<DocumentSession>()
        .with_commands(sibling, |commands| {
            commands.set_cursor(0, 4, false, CursorReveal::Ensure)
        })
        .unwrap();
    let revision = fixture
        .app
        .world()
        .non_send::<DocumentSession>()
        .document_revision(document)
        .unwrap();
    fixture.request(HostAction::ApplyEdits {
        document,
        revision,
        edits: vec![bedtime_documents::session::ByteEdit {
            range: 4..4,
            bytes: "🙂".as_bytes().to_vec(),
        }],
    });
    assert_eq!(fixture.bytes(document), "abcd🙂".as_bytes());
    assert_eq!(
        fixture
            .app
            .world()
            .non_send::<DocumentSession>()
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
            .resource::<DocumentNavigation>()
            .active_view,
        Some(view)
    );
}

#[test]
fn queued_source_commands_preserve_origin_after_focus_changes() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut fixture = Fixture::new();
    let first = fixture.write("first.txt", b"first");
    let (source, document, _) = fixture.open(&first);
    let request = bedtime_core::capture_host_request(
        fixture.app.world(),
        Some(source),
        HostAction::Command {
            command: "bedtime.workbench.duplicate_view".into(),
            context: Default::default(),
        },
    );
    let other = fixture.write("other.txt", b"other");
    let (_, other_document, _) = fixture.open(&other);
    let id = bedtime_core::submit_captured_host_request(fixture.app.world_mut(), request);
    bedtime_core::resolve_documents(fixture.app.world_mut());
    assert!(
        bedtime_core::take_host_result(fixture.app.world_mut(), id)
            .unwrap()
            .is_ok()
    );
    assert_eq!(
        fixture
            .app
            .world()
            .non_send::<DocumentSession>()
            .view_count(document),
        2
    );
    assert_eq!(
        fixture
            .app
            .world()
            .non_send::<DocumentSession>()
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
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut fixture = Fixture::new();
    let first = fixture.write("first.txt", b"first");
    let (source, _, _) = fixture.open(&first);
    let request = bedtime_core::capture_host_request(
        fixture.app.world(),
        Some(source),
        bedtime_api::features::HostAction::Command {
            command: "bedtime.workbench.duplicate_view".into(),
            context: Default::default(),
        },
    );
    headless_host::workbench(fixture.app.world_mut(), WorkbenchAction::ClosePanel(source)).unwrap();
    let other = fixture.write("other.txt", b"other");
    let (_, document, _) = fixture.open(&other);
    let id = bedtime_core::submit_captured_host_request(fixture.app.world_mut(), request);
    bedtime_core::resolve_documents(fixture.app.world_mut());
    assert_eq!(
        bedtime_core::take_host_result(fixture.app.world_mut(), id)
            .unwrap()
            .unwrap_err(),
        "Originating panel is unavailable"
    );
    assert_eq!(
        fixture
            .app
            .world()
            .non_send::<DocumentSession>()
            .view_count(document),
        1
    );
}

#[test]
fn split_uses_canonical_layout_and_preserves_document_view_ownership() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut fixture = Fixture::new();
    let path = fixture.write("split.txt", b"split");
    let (source, document, view) = fixture.open(&path);
    fixture.command("bedtime.workbench.split_right");
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
            .get::<PanelBinding>(destination)
            .unwrap()
            .document,
        Some(document)
    );
    assert_ne!(
        fixture
            .app
            .world()
            .get::<PanelBinding>(destination)
            .unwrap()
            .view,
        Some(view)
    );
}

#[test]
fn unicode_input_targets_selected_view_without_changing_other_documents() {
    let _lock = CONTEXT_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
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
    let _lock = CONTEXT_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut context = context();
    let mut fixture = Fixture::new();
    let path = fixture.write("stable.txt", b"stable");
    fixture.open(&path);
    let session = fixture.app.world().non_send::<DocumentSession>() as *const DocumentSession;
    fixture.frame(&mut context);
    fixture.frame(&mut context);
    assert_eq!(
        fixture.app.world().non_send::<DocumentSession>() as *const DocumentSession,
        session
    );
    assert!(
        fixture
            .app
            .world()
            .contains_resource::<bedtime_api::settings::SettingsSnapshot>()
    );
    assert!(
        fixture
            .app
            .world()
            .contains_resource::<bedtime_api::features::FeatureRegistry>()
    );
}

#[test]
fn focusing_a_tool_preserves_document_carets() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut fixture = Fixture::new();
    let path = fixture.write("caret.txt", b"abcdef");
    let (panel, _, view) = fixture.open(&path);
    fixture
        .app
        .world_mut()
        .non_send_mut::<DocumentSession>()
        .with_commands(view, |commands| {
            commands.set_cursor(0, 3, false, CursorReveal::Ensure)
        })
        .unwrap();
    headless_host::workbench(
        fixture.app.world_mut(),
        WorkbenchAction::OpenPanel {
            panel_type: "fixture.tool".into(),
            state: serde_json::Value::Null,
            area: None,
        },
    )
    .unwrap();
    fixture.focus(panel);
    assert_eq!(
        fixture
            .app
            .world()
            .non_send::<DocumentSession>()
            .view_snapshot(view)
            .unwrap()
            .column,
        3
    );
}

#[test]
fn byte_view_save_shortcut_uses_the_document_save_pipeline() {
    let _lock = CONTEXT_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut context = context();
    let mut fixture = Fixture::new();
    let path = fixture.write("shortcut.bin", &[0xff, 0, 1]);
    let HostOutcome::Opened(panel) = fixture.request(HostAction::OpenFile {
        path: path.to_string_lossy().into_owned(),
        viewer: Some(bedtime_plugin_editor::HEX_VIEWER.into()),
    }) else {
        panic!("byte document receipt")
    };
    let document = fixture
        .app
        .world()
        .get::<PanelBinding>(panel)
        .unwrap()
        .document
        .unwrap();
    assert!(
        fixture
            .app
            .world()
            .get::<PanelBinding>(panel)
            .unwrap()
            .view
            .is_none()
    );
    let revision = fixture
        .app
        .world()
        .non_send::<DocumentSession>()
        .document_revision(document)
        .unwrap();
    fixture.request(HostAction::ApplyEdits {
        document,
        revision,
        edits: vec![bedtime_documents::session::ByteEdit {
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
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .dirty
    );
}
