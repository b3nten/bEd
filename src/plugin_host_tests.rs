use super::*;
use crate::files::test_support::TempDir;
use bed_session::editor_session::ByteEdit;
use std::fs;
use std::{any::Any, cell::RefCell, rc::Rc};

#[derive(Default)]
struct ProbeState {
    drawn_active: Vec<Option<DocumentId>>,
    closes: usize,
    commands: Vec<CommandContext>,
}
struct ProbePlugin(Rc<RefCell<ProbeState>>);
struct ProbePanel {
    shared: Rc<RefCell<ProbeState>>,
    document: Option<DocumentId>,
}
impl Plugin for ProbePlugin {
    fn id(&self) -> &'static str {
        "test.probe"
    }
    fn register(&self, registrar: &mut bed_plugin::Registrar<'_>) {
        registrar.panel("test.probe.panel", "Probe");
        registrar.command("test.probe.capture", "Capture", None);
        registrar.menu(MenuSlot::File, "test.probe.capture");
        registrar.menu(MenuSlot::Folder, "test.probe.capture");
        registrar.menu(MenuSlot::TreeBackground, "test.probe.capture");
        registrar.menu(MenuSlot::TextSelection, "test.probe.capture");
    }
    fn command(
        &mut self,
        _: &str,
        context: &CommandContext,
        _: &HostContext<'_>,
        _: &mut Vec<HostRequest>,
    ) {
        self.0.borrow_mut().commands.push(context.clone());
    }
    fn create_panel(
        &mut self,
        _: &str,
        document: Option<DocumentId>,
        _: &Value,
    ) -> Result<Box<dyn PluginPanel>, String> {
        Ok(Box::new(ProbePanel {
            shared: Rc::clone(&self.0),
            document,
        }))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
impl PluginPanel for ProbePanel {
    fn title(&self, _: &HostContext<'_>) -> String {
        "Probe".into()
    }
    fn draw(&mut self, ui: &Ui, host: &HostContext<'_>, _: &mut Vec<HostRequest>) {
        self.shared
            .borrow_mut()
            .drawn_active
            .push(host.active_document);
        ui.text("Plugin context probe");
    }
    fn attached_document(&self) -> Option<DocumentId> {
        self.document
    }
    fn close(&mut self, _: &mut Vec<HostRequest>) {
        self.shared.borrow_mut().closes += 1;
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
fn install_probe(workbench: &mut Workbench) -> Rc<RefCell<ProbeState>> {
    let shared = Rc::new(RefCell::new(ProbeState::default()));
    let plugin = ProbePlugin(Rc::clone(&shared));
    workbench.plugins.registry.register(&plugin).unwrap();
    workbench.plugins.instances.push(Box::new(plugin));
    shared
}

fn host(dir: &TempDir) -> Workbench {
    let mut settings = Settings::with_paths(
        dir.path("config"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")),
    )
    .unwrap();
    settings.settings["terminal_visible"] = json!(false);
    settings.terminal_visible = false;
    settings.settings["treesitter"] = json!(false);
    settings.settings["git_changed_lines"] = json!(false);
    Workbench::with_settings(settings)
}
fn edit(
    workbench: &mut Workbench,
    document: DocumentId,
    range: std::ops::Range<usize>,
    bytes: &[u8],
) {
    let revision = workbench.session.document_revision(document).unwrap();
    workbench
        .session
        .apply_edits(
            document,
            revision,
            &[ByteEdit {
                range,
                bytes: bytes.to_vec(),
            }],
        )
        .unwrap();
}

#[test]
fn registered_viewers_precede_content_detection_and_core_fallbacks() {
    let dir = TempDir::new();
    let image = dir.write("project/picture.PnG", b"this even looks like text");
    let text = dir.write("project/readme", b"hello\n");
    let binary_bytes = b"\xef\xbb\xbf\0\0\0\0\r\n\r\xff";
    let binary = dir.write("project/data.unknown", binary_bytes);
    let mut workbench = host(&dir);
    workbench.open_or_focus(&image).unwrap();
    let image_document = workbench.active_document().unwrap();
    assert_eq!(
        workbench.session.document_kind(image_document).unwrap(),
        DocumentKind::Bytes
    );
    assert!(
        matches!(&workbench.tabs.last().unwrap().panel, Panel::Plugin(panel) if panel.viewer.as_deref() == Some("bed.image.viewer"))
    );
    workbench.open_or_focus(&text).unwrap();
    assert!(matches!(
        workbench.tabs.last().unwrap().panel,
        Panel::Document(_)
    ));
    workbench.open_or_focus(&binary).unwrap();
    assert!(matches!(
        workbench.tabs.last().unwrap().panel,
        Panel::Hex(_)
    ));
    assert_eq!(workbench.active_snapshot().unwrap().bytes, binary_bytes);
    let count = workbench.tabs.len();
    workbench.open_or_focus(&image).unwrap();
    assert_eq!(workbench.tabs.len(), count);
    assert_eq!(workbench.active_document(), Some(image_document));
}

#[test]
fn image_and_hex_share_edits_and_last_panel_close_saves() {
    let dir = TempDir::new();
    let path = dir.write("project/picture.png", b"original");
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let image_panel = workbench.tabs.last().unwrap().id;
    let document = workbench.active_document().unwrap();
    workbench
        .open_file_with_viewer(&path, Some("bed.hex"), true)
        .unwrap();
    assert_eq!(workbench.active_document(), Some(document));
    edit(&mut workbench, document, 0..8, b"changed\0");
    let index = workbench
        .tabs
        .iter()
        .position(|tab| tab.id == image_panel)
        .unwrap();
    workbench.close_tab(index).unwrap();
    assert!(workbench.session.snapshot(document).unwrap().dirty);
    assert_eq!(fs::read(&path).unwrap(), b"original");
    let index = workbench
        .tabs
        .iter()
        .position(|tab| tab.panel.document() == Some(document))
        .unwrap();
    workbench.close_tab(index).unwrap();
    assert!(workbench.session.snapshot(document).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"changed\0");
}

#[test]
fn explicit_mode_switch_saves_closes_and_reopens_one_authoritative_document() {
    let dir = TempDir::new();
    let path = dir.write("project/readme.txt", b"before");
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let text_document = workbench.active_document().unwrap();
    workbench.dispatch(WindowCommand::DuplicateView).unwrap();
    edit(&mut workbench, text_document, 0..6, b"after");
    workbench
        .open_file_with_viewer(&path, Some("bed.hex"), false)
        .unwrap();
    let byte_document = workbench.active_document().unwrap();
    assert_ne!(text_document, byte_document);
    assert!(workbench.session.snapshot(text_document).is_err());
    assert_eq!(
        workbench
            .tabs
            .iter()
            .filter(|tab| tab.panel.document() == Some(byte_document))
            .count(),
        1
    );
    assert_eq!(
        workbench.session.snapshot(byte_document).unwrap().bytes,
        b"after"
    );
    assert_eq!(fs::read(&path).unwrap(), b"after");
    workbench
        .open_file_with_viewer(&path, Some("bed.text"), false)
        .unwrap();
    let count = workbench.tabs.len();
    workbench
        .open_file_with_viewer(&path, Some("bed.text"), false)
        .unwrap();
    assert_eq!(workbench.tabs.len(), count);
    assert!(matches!(
        workbench.tabs.last().unwrap().panel,
        Panel::Document(_)
    ));
}

#[test]
fn focused_hex_save_does_not_write_the_previous_text_editor() {
    let dir = TempDir::new();
    let text_path = dir.write("project/readme.txt", b"text");
    let byte_path = dir.write("project/data", &[0; 32]);
    let mut workbench = host(&dir);
    workbench.open_or_focus(&text_path).unwrap();
    let text = workbench.active_document().unwrap();
    edit(&mut workbench, text, 0..4, b"edited text");
    workbench.open_or_focus(&byte_path).unwrap();
    let bytes = workbench.active_document().unwrap();
    edit(&mut workbench, bytes, 0..1, &[0xff]);
    workbench.handle_action(HostAction::Save).unwrap();
    assert_eq!(fs::read(&text_path).unwrap(), b"text");
    assert!(workbench.session.snapshot(text).unwrap().dirty);
    assert_eq!(fs::read(&byte_path).unwrap()[0], 0xff);
    assert!(!workbench.session.snapshot(bytes).unwrap().dirty);
}

#[test]
fn file_menu_command_keeps_its_source_path_when_focus_changes() {
    let dir = TempDir::new();
    let image_path = dir.write("project/image.png", b"source");
    let text_path = dir.write("project/readme.txt", b"other");
    let mut workbench = host(&dir);
    workbench.open_or_focus(&text_path).unwrap();
    workbench
        .handle_tree_action(FileTreeAction::Command {
            command: "bed.image.open-file".into(),
            context: bed_plugin::CommandContext {
                path: Some(image_path.to_string_lossy().into_owned()),
                ..Default::default()
            },
        })
        .unwrap();
    assert_eq!(
        workbench.active_snapshot().unwrap().path,
        image_path.canonicalize().unwrap().to_string_lossy()
    );
    assert!(matches!(
        workbench.tabs.last().unwrap().panel,
        Panel::Plugin(_)
    ));
}

#[test]
fn workspace_restores_plugin_and_hex_views_with_shared_document_and_state() {
    let dir = TempDir::new();
    let path = dir.write("project/image.png", b"source");
    let mut first = host(&dir);
    first.open_or_focus(&path).unwrap();
    first
        .open_file_with_viewer(&path, Some("bed.hex"), true)
        .unwrap();
    if let Panel::Hex(view) = &mut first.tabs.last_mut().unwrap().panel {
        view.restore_state(&json!({"cursor":2,"anchor":1,"insert":true}));
    }
    first.dispatch(WindowCommand::NewStructure).unwrap();
    first.persist_workspace().unwrap();
    let mut second = host(&dir);
    second.restore_last_workspace().unwrap();
    let attached: Vec<_> = second
        .tabs
        .iter()
        .filter_map(|tab| tab.panel.document())
        .collect();
    assert_eq!(attached.len(), 2);
    assert_eq!(attached[0], attached[1]);
    assert_eq!(second.panel_count("structure"), 1);
    assert_eq!(second.panel_count("hex"), 1);
    let hex = second
        .tabs
        .iter()
        .find_map(|tab| {
            if let Panel::Hex(view) = &tab.panel {
                Some(view)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(hex.state()["cursor"], 2);
    assert_eq!(hex.state()["insert"], true);
}

#[test]
fn workspace_restores_gltf_camera_and_shares_its_bytes_with_hex() {
    let dir = TempDir::new();
    let path = dir.write(
        "project/model.glb",
        include_bytes!("../tests/fixtures/gltf/cube.glb"),
    );
    let camera = json!({"camera": {"target": [2.0, 3.0, 4.0], "yaw": 1.0,
        "pitch": 0.5, "distance": 12.0}});
    let mut first = host(&dir);
    first.open_or_focus(&path).unwrap();
    let document = first.active_document().unwrap();
    assert_eq!(
        first.session.document_kind(document).unwrap(),
        DocumentKind::Bytes
    );
    let Panel::Plugin(panel) = &mut first.tabs.last_mut().unwrap().panel else {
        panic!("GLB must open in the glTF plugin");
    };
    panel.instance = bed_plugin_gltf::GltfPlugin
        .create_panel(bed_plugin_gltf::PANEL_ID, Some(document), &camera)
        .unwrap();
    first
        .open_file_with_viewer(&path, Some("bed.hex"), true)
        .unwrap();
    first.persist_workspace().unwrap();
    let mut second = host(&dir);
    second.restore_last_workspace().unwrap();
    let panel = second
        .tabs
        .iter()
        .find_map(|tab| match &tab.panel {
            Panel::Plugin(panel) if panel.viewer.as_deref() == Some(bed_plugin_gltf::VIEWER_ID) => {
                Some(panel)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(panel.instance.save_state(), camera);
    let documents: Vec<_> = second
        .tabs
        .iter()
        .filter_map(|tab| tab.panel.document())
        .collect();
    assert_eq!(documents.len(), 2);
    assert_eq!(documents[0], documents[1]);
    assert_eq!(second.panel_count("hex"), 1);
}

#[test]
fn tools_retain_the_last_byte_document_for_save_and_commands() {
    let dir = TempDir::new();
    let text_path = dir.write("project/text.txt", b"text");
    let byte_path = dir.write("project/bytes", &[0; 32]);
    let mut workbench = host(&dir);
    workbench.open_or_focus(&text_path).unwrap();
    let text = workbench.active_document().unwrap();
    edit(&mut workbench, text, 0..4, b"unsaved text");
    workbench.open_or_focus(&byte_path).unwrap();
    let binary = workbench.active_document().unwrap();
    edit(&mut workbench, binary, 0..1, &[0xff]);
    workbench.dispatch(WindowCommand::NewSettings).unwrap();
    assert_eq!(workbench.active_document(), Some(binary));
    workbench.handle_action(HostAction::Save).unwrap();
    assert_eq!(fs::read(&byte_path).unwrap()[0], 0xff);
    assert_eq!(workbench.active_document(), Some(binary));
    assert_eq!(fs::read(&text_path).unwrap(), b"text");
    assert!(workbench.session.snapshot(text).unwrap().dirty);
}

#[test]
fn plugin_draw_keeps_its_focused_document_in_host_context() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let text_path = dir.write("project/text.txt", b"text");
    let byte_path = dir.write("project/bytes", &[0; 32]);
    let mut workbench = host(&dir);
    workbench.open_or_focus(&text_path).unwrap();
    workbench.open_or_focus(&byte_path).unwrap();
    let document = workbench.active_document().unwrap();
    let shared = install_probe(&mut workbench);
    workbench
        .open_plugin_panel("test.probe.panel", Some(document), &Value::Null, None)
        .unwrap();
    let mut context = Context::create();
    context.set_ini_filename(None::<PathBuf>).unwrap();
    workbench
        .initialize(&mut context, WorkbenchHostMode::Floating)
        .unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    for _ in 0..3 {
        context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
            [1200.0, 800.0],
            1.0 / 60.0,
        ));
        workbench.render(context.frame()).unwrap();
        drop(context.render_legacy());
    }
    assert!(!shared.borrow().drawn_active.is_empty());
    assert!(
        shared
            .borrow()
            .drawn_active
            .iter()
            .all(|active| *active == Some(document))
    );
    workbench.cleanup().unwrap();
}

#[test]
fn bulk_close_calls_plugin_panel_lifecycle_once() {
    let dir = TempDir::new();
    let path = dir.write("project/text.txt", b"text");
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let shared = install_probe(&mut workbench);
    workbench
        .open_plugin_panel("test.probe.panel", None, &Value::Null, None)
        .unwrap();
    workbench
        .open_plugin_panel("test.probe.panel", None, &Value::Null, None)
        .unwrap();
    assert!(workbench.request_close_all().unwrap());
    assert_eq!(shared.borrow().closes, 2);
    assert!(workbench.request_close_all().unwrap());
    assert_eq!(shared.borrow().closes, 2);
}

#[test]
fn failed_save_aborts_mode_switch_without_detaching_shared_views() {
    let dir = TempDir::new();
    let path = dir.write("project/text.txt", b"before");
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let document = workbench.active_document().unwrap();
    workbench.dispatch(WindowCommand::DuplicateView).unwrap();
    edit(&mut workbench, document, 0..6, b"after");
    let panels: Vec<_> = workbench.tabs.iter().map(|tab| tab.id).collect();
    // A destination which cannot be opened for writing produces a real save failure.
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(
        workbench
            .open_file_with_viewer(&path, Some("bed.hex"), false)
            .is_err()
    );
    assert_eq!(
        workbench.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
        panels
    );
    assert_eq!(workbench.active_document(), Some(document));
    assert_eq!(
        workbench.session.document_kind(document).unwrap(),
        DocumentKind::Text
    );
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        b"after"
    );
    assert!(workbench.session.snapshot(document).unwrap().dirty);
}

#[test]
fn queued_tree_command_keeps_document_and_revision_from_invocation() {
    let dir = TempDir::new();
    let path = dir.write("project/text.txt", b"before");
    let other = dir.write("project/other.txt", b"other");
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let document = workbench.active_document().unwrap();
    let shared = install_probe(&mut workbench);
    let context = CommandContext {
        path: Some(path.to_string_lossy().into_owned()),
        document: Some(document),
        revision: Some(workbench.session.document_revision(document).unwrap()),
        selection: None,
    };
    edit(&mut workbench, document, 0..6, b"newer");
    workbench.open_or_focus(&other).unwrap();
    workbench
        .handle_tree_action(FileTreeAction::Command {
            command: "test.probe.capture".into(),
            context: context.clone(),
        })
        .unwrap();
    assert_eq!(shared.borrow().commands, vec![context]);
}

#[test]
fn captured_text_selection_survives_later_edits_and_focus_changes() {
    let dir = TempDir::new();
    let path = dir.write("project/text.txt", b"selected text");
    let other = dir.write("project/other.txt", b"other");
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let document = workbench.active_document().unwrap();
    let view = workbench.active_view().unwrap();
    let shared = install_probe(&mut workbench);
    workbench
        .session
        .with_commands(view, |commands| commands.select_all())
        .unwrap();
    let index = workbench
        .tabs
        .iter()
        .position(|tab| matches!(&tab.panel, Panel::Document(editor) if editor.id() == view))
        .unwrap();
    let tab = workbench.tabs.remove(index);
    let Panel::Document(editor) = &tab.panel else {
        unreachable!()
    };
    workbench.capture_editor_context(editor).unwrap();
    workbench.tabs.insert(index, tab);
    let context = workbench.editor_menu_context[&view].clone();
    assert_eq!(context.selection.as_ref().unwrap().text, "selected text");
    assert_eq!(context.selection.as_ref().unwrap().ranges, vec![0..13]);
    edit(&mut workbench, document, 0..13, b"newer");
    workbench.open_or_focus(&other).unwrap();
    workbench.refresh_plugins().unwrap();
    workbench
        .plugins
        .command("test.probe.capture", &context)
        .unwrap();
    assert_eq!(shared.borrow().commands, vec![context]);
}

#[test]
fn invalid_plugin_document_attachment_is_rejected_before_inserting_a_panel() {
    let dir = TempDir::new();
    let path = dir.write("project/text.txt", b"text");
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    install_probe(&mut workbench);
    let count = workbench.tabs.len();
    assert!(
        workbench
            .open_plugin_panel(
                "test.probe.panel",
                Some(DocumentId::next()),
                &Value::Null,
                None
            )
            .is_err()
    );
    assert_eq!(workbench.tabs.len(), count);
}

#[test]
fn restoring_a_focused_tool_keeps_the_last_shared_text_view() {
    let dir = TempDir::new();
    let path = dir.write("project/text.txt", b"text");
    let mut first = host(&dir);
    first.open_or_focus(&path).unwrap();
    first.dispatch(WindowCommand::DuplicateView).unwrap();
    let last_editor_panel = first.active_panel_id().unwrap();
    first.dispatch(WindowCommand::NewStructure).unwrap();
    let structure_panel = first.active_panel_id().unwrap();
    first.persist_workspace().unwrap();
    let mut second = host(&dir);
    second.restore_last_workspace().unwrap();
    assert_eq!(second.active_panel_id(), Some(structure_panel));
    assert_eq!(
        second.tabs[second.active_tab_index().unwrap()].id,
        last_editor_panel
    );
}

#[test]
fn closed_panels_do_not_leave_registered_output_handles() {
    let dir = TempDir::new();
    let mut workbench = host(&dir);
    let handle = TextureHandle::next();
    workbench.set_plugin_texture(handle, dear_imgui_rs::TextureId::new(42));
    assert!(workbench.plugins.frame.context().texture(handle).is_some());
    workbench.retain_plugin_textures(&HashSet::new());
    assert!(workbench.plugins.frame.context().texture(handle).is_none());
}
