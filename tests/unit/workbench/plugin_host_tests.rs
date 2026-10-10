use super::*;
use crate::test_support::TempDir;
use bed_document_session::editor_session::ByteEdit;
use std::fs;
use std::{any::Any, cell::RefCell, rc::Rc};

#[derive(Default)]
struct ProbeState {
    drawn_active: Vec<Option<DocumentId>>,
    closes: usize,
    commands: Vec<CommandContext>,
    panic_draw: bool,
    panic_action: bool,
    panic_close: bool,
    actions: usize,
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
    fn register(&self, registrar: &mut bed_workbench_api::Registrar<'_>) {
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
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}
impl PluginPanel for ProbePanel {
    fn title(&self, _: &HostContext<'_>) -> String {
        "Probe".into()
    }
    fn draw(&mut self, ui: &Ui, host: &HostContext<'_>, requests: &mut Vec<HostRequest>) {
        self.shared
            .borrow_mut()
            .drawn_active
            .push(host.active_document);
        if self.shared.borrow().panic_draw {
            requests.push(HostRequest::Notify {
                message: "request from failed draw".into(),
            });
            ui.with_bound_context(|| unsafe {
                sys::igBeginChild_Str(c"probe-child".as_ptr(), [100.0; 2].into(), 0, 0);
                sys::igBeginGroup();
                sys::igPushStyleVar_Float(sys::ImGuiStyleVar_Alpha as _, 0.5);
            });
            panic!("probe draw panic");
        }
        ui.text("Plugin context probe");
    }
    fn action(
        &mut self,
        _: PanelAction,
        _: &HostContext<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> Result<bool, String> {
        let mut shared = self.shared.borrow_mut();
        shared.actions += 1;
        if shared.panic_action {
            requests.push(HostRequest::Notify {
                message: "request from failed action".into(),
            });
            panic!("probe action panic");
        }
        Ok(false)
    }
    fn attached_document(&self) -> Option<DocumentId> {
        self.document
    }
    fn close(&mut self, _: &mut Vec<HostRequest>) {
        self.shared.borrow_mut().closes += 1;
        assert!(!self.shared.borrow().panic_close, "probe close panic");
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

#[test]
fn panel_action_panic_discards_requests_and_leaves_the_document_closable() {
    let dir = TempDir::new();
    let path = dir.write("project/text.txt", b"text");
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let document = workbench.active_document().unwrap();
    let shared = install_probe(&mut workbench);
    let state = json!({"restored_view": 42});
    let tab = workbench
        .open_plugin_panel("test.probe.panel", Some(document), &state, None)
        .unwrap();
    let index = workbench.tabs.iter().position(|t| t.id == tab).unwrap();
    shared.borrow_mut().panic_action = true;
    let error = workbench
        .plugin_panel_action(index, PanelAction::SelectAll)
        .unwrap_err();
    assert!(error.to_string().contains("probe action panic"));
    assert!(workbench.modules.requests.is_empty());
    assert_eq!(workbench.tabs[index].panel.instance.save_state(), state);
    assert_eq!(workbench.tabs[index].panel.document(), Some(document));
    assert!(workbench.close_tab(index).unwrap());
    assert_eq!(shared.borrow().actions, 1);
    assert_eq!(shared.borrow().closes, 0);
    assert_eq!(workbench.session.snapshot(document).unwrap().bytes, b"text");
}

#[test]
fn panel_close_panic_is_reported_and_does_not_trap_the_tab() {
    let dir = TempDir::new();
    let mut workbench = host(&dir);
    let shared = install_probe(&mut workbench);
    let tab = workbench
        .open_plugin_panel("test.probe.panel", None, &Value::Null, None)
        .unwrap();
    let index = workbench.tabs.iter().position(|t| t.id == tab).unwrap();
    shared.borrow_mut().panic_close = true;
    assert!(workbench.close_tab(index).unwrap());
    assert!(!workbench.tabs.iter().any(|t| t.id == tab));
    assert!(
        workbench
            .error
            .as_deref()
            .unwrap()
            .contains("probe close panic")
    );
    assert_eq!(shared.borrow().closes, 1);
}

#[test]
fn panel_draw_panic_restores_imgui_stacks_and_allows_other_panels_to_draw() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let mut workbench = host(&dir);
    let shared = install_probe(&mut workbench);
    let tab = workbench
        .open_plugin_panel("test.probe.panel", None, &Value::Null, None)
        .unwrap();
    shared.borrow_mut().panic_draw = true;
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
    assert_eq!(shared.borrow().drawn_active.len(), 1);
    assert_ne!(workbench.error.as_deref(), Some("request from failed draw"));
    shared.borrow_mut().panic_draw = false;
    let healthy = workbench
        .open_plugin_panel("test.probe.panel", None, &Value::Null, None)
        .unwrap();
    for _ in 0..3 {
        context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
            [1200.0, 800.0],
            1.0 / 60.0,
        ));
        workbench.render(context.frame()).unwrap();
        drop(context.render_legacy());
    }
    assert!(shared.borrow().drawn_active.len() > 1);
    let index = workbench.tabs.iter().position(|t| t.id == healthy).unwrap();
    assert!(workbench.close_tab(index).unwrap());
    let index = workbench.tabs.iter().position(|t| t.id == tab).unwrap();
    assert!(workbench.close_tab(index).unwrap());
    assert_eq!(shared.borrow().closes, 1);
    workbench.cleanup().unwrap();
}
fn install_probe(workbench: &mut Workbench) -> Rc<RefCell<ProbeState>> {
    let shared = Rc::new(RefCell::new(ProbeState::default()));
    let plugin = ProbePlugin(Rc::clone(&shared));
    workbench.modules.registry.register(&plugin).unwrap();
    workbench.modules.instances.push(Box::new(plugin));
    shared
}

fn host(dir: &TempDir) -> Workbench {
    let mut settings = Settings::with_paths(
        dir.path("config"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
    )
    .unwrap();
    settings.settings["terminal_visible"] = json!(false);
    settings.terminal_visible = false;
    settings.settings["treesitter"] = json!(false);
    settings.settings["git_changed_lines"] = json!(false);
    Workbench::with_settings(settings, crate::builtins::modules)
}

#[test]
fn targeted_host_requests_override_the_inherited_destination_before_focus_changes() {
    use crate::workspace::{tiling::Layout, tiling_state::TilingState};

    let dir = TempDir::new();
    let mut workbench = host(&dir);
    install_probe(&mut workbench);
    workbench
        .open_or_focus(&dir.write("previous.txt", b"previous"))
        .unwrap();
    let previous = workbench.active_panel_id().unwrap();
    workbench
        .open_or_focus(&dir.write("last.txt", b"last"))
        .unwrap();
    let last = workbench.active_panel_id().unwrap();
    let mut layout = Layout::default();
    let right = layout.split(1, 0, 6000, 100).unwrap();
    let bottom = layout.split(right, 1, 3000, 100).unwrap();
    workbench.tiling = TilingState::new(layout);
    workbench.pending_tiling = None;
    for id in workbench.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>() {
        workbench.place_panel_in_area(id, 1);
    }
    workbench.place_panel_in_area(previous, right);
    workbench.place_panel_in_area(last, 1);
    assert_eq!(workbench.focused_area(), Some(1));
    assert_eq!(workbench.previous_focused_area(), Some(right));
    let count = workbench.tabs.len();
    let path = dir.write("requested.txt", b"new file");
    workbench
        .process_plugin_requests_at(
            false,
            Some(bottom),
            vec![
                HostRequest::OpenPanel {
                    panel_type: "test.probe.panel".into(),
                    document: None,
                    state: Value::Null,
                },
                HostRequest::OpenFileIn {
                    path: path.to_string_lossy().into_owned(),
                    viewer: None,
                    target: PanelTarget::LastFocused,
                },
                HostRequest::ShowPanelIn {
                    panel_type: bed_module_settings::PANEL_ID.into(),
                    document: None,
                    state: Value::Null,
                    action: None,
                    target: PanelTarget::PreviousFocused,
                },
            ],
        )
        .unwrap();
    let created = &workbench.tabs[count..];
    assert_eq!(created.len(), 3);
    for (tab, expected) in created.iter().zip([bottom, 1, right]) {
        assert_eq!(workbench.tiling.area_for(tab.id), Some(expected));
    }
    assert_eq!(workbench.focused, Some(created[2].id));
    workbench.cleanup().unwrap();
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
fn content_classification_precedes_viewer_selection() {
    let dir = TempDir::new();
    let image = dir.write("project/picture.PnG", b"\x89PNG\0\0\0\0");
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
    assert_eq!(
        workbench.tabs.last().unwrap().panel.viewer.as_deref(),
        Some("bed.image.viewer")
    );
    workbench.open_or_focus(&text).unwrap();
    assert!(workbench.tabs.last().unwrap().panel.editor().is_some());
    workbench.open_or_focus(&binary).unwrap();
    assert!(workbench.tabs.last().unwrap().panel.hex().is_some());
    assert_eq!(workbench.active_snapshot().unwrap().bytes, binary_bytes);
    let count = workbench.tabs.len();
    workbench.open_or_focus(&image).unwrap();
    assert_eq!(workbench.tabs.len(), count);
    assert_eq!(workbench.active_document(), Some(image_document));
}

#[test]
fn audio_and_hex_use_the_same_byte_document_and_switch_back_to_the_audio_viewer() {
    let dir = TempDir::new();
    let path = dir.write("project/tone.WAV", b"RIFF\0\0\0\0WAVE snapshot");
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let document = workbench.active_document().unwrap();
    assert_eq!(
        workbench.session.document_kind(document).unwrap(),
        DocumentKind::Bytes
    );
    assert_eq!(
        workbench.tabs.last().unwrap().panel.viewer.as_deref(),
        Some(bed_plugin_audio::VIEWER_ID)
    );
    workbench
        .open_file_with_viewer(&path, Some("bed.hex"), true)
        .unwrap();
    assert_eq!(workbench.active_document(), Some(document));
    workbench
        .open_file_with_viewer(&path, Some(bed_plugin_audio::VIEWER_ID), true)
        .unwrap();
    assert_eq!(workbench.active_document(), Some(document));
    assert_eq!(
        workbench.active_snapshot().unwrap().bytes,
        b"RIFF\0\0\0\0WAVE snapshot"
    );
    assert_eq!(fs::read(path).unwrap(), b"RIFF\0\0\0\0WAVE snapshot");
}

#[test]
fn raster_images_use_bytes_and_svg_shares_its_text_source() {
    let dir = TempDir::new();
    let svg_bytes = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 3 2">
        <rect width="3" height="2" fill="red"/>
    </svg>"#;
    let svg = dir.write("project/picture.SVG", svg_bytes);
    let mut workbench = host(&dir);
    for path in [
        svg.clone(),
        dir.write("project/picture.WebP", b"image\0\0snapshot"),
        dir.write("project/picture.GIF", b"image\0\0snapshot"),
        dir.write("project/picture.TGA", b"image\0\0snapshot"),
    ] {
        workbench.open_or_focus(&path).unwrap();
        assert_eq!(
            workbench
                .session
                .document_kind(workbench.active_document().unwrap())
                .unwrap(),
            if path == svg {
                DocumentKind::Text
            } else {
                DocumentKind::Bytes
            }
        );
        assert_eq!(
            workbench.tabs.last().unwrap().panel.viewer.as_deref(),
            Some(bed_plugin_image::VIEWER_ID)
        );
    }
    workbench
        .open_file_with_viewer(&svg, Some("bed.text"), false)
        .unwrap();
    assert!(workbench.tabs.last().unwrap().panel.editor().is_some());
    assert_eq!(workbench.active_snapshot().unwrap().bytes, svg_bytes);
    assert_eq!(fs::read(&svg).unwrap(), svg_bytes);
}

#[test]
fn image_and_hex_share_edits_and_last_panel_close_saves() {
    let dir = TempDir::new();
    let path = dir.write("project/picture.png", b"origin\0\0");
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
    assert_eq!(fs::read(&path).unwrap(), b"origin\0\0");
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
    assert!(workbench.tabs.last().unwrap().panel.editor().is_some());
}

#[test]
fn binary_text_viewer_requests_preserve_existing_views_and_unsaved_bytes() {
    let dir = TempDir::new();
    let path = dir.write("project/data.csv", b"before");
    let mut workbench = host(&dir);
    workbench
        .open_file_with_viewer(&path, Some("bed.hex"), false)
        .unwrap();
    let document = workbench.active_document().unwrap();
    edit(&mut workbench, document, 0..6, &[0; 32]);
    let tabs: Vec<_> = workbench.tabs.iter().map(|tab| tab.id).collect();
    for viewer in ["bed.text", bed_plugin_csv::VIEWER_ID] {
        let error = workbench
            .open_file_with_viewer(&path, Some(viewer), true)
            .unwrap_err();
        assert!(error.to_string().contains("binary data"));
        assert_eq!(
            workbench.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
            tabs
        );
        assert_eq!(workbench.active_document(), Some(document));
        assert_eq!(workbench.session.snapshot(document).unwrap().bytes, [0; 32]);
        assert!(workbench.session.snapshot(document).unwrap().dirty);
        assert_eq!(fs::read(&path).unwrap(), b"before");
    }
}

#[test]
fn binary_edits_in_a_text_document_cannot_bypass_text_viewer_validation() {
    let dir = TempDir::new();
    let path = dir.write("project/notes.txt", b"before");
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let document = workbench.active_document().unwrap();
    edit(&mut workbench, document, 0..6, &[0; 32]);
    let count = workbench.tabs.len();
    let error = workbench
        .open_file_with_viewer(&path, Some("bed.text"), true)
        .unwrap_err();
    assert!(error.to_string().contains("binary data"));
    assert_eq!(workbench.tabs.len(), count);
    assert_eq!(workbench.active_document(), Some(document));
    assert!(workbench.session.snapshot(document).unwrap().dirty);
    assert_eq!(fs::read(&path).unwrap(), b"before");
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
            context: bed_workbench_api::CommandContext {
                path: Some(image_path.to_string_lossy().into_owned()),
                ..Default::default()
            },
        })
        .unwrap();
    assert_eq!(
        workbench.active_snapshot().unwrap().path,
        image_path.canonicalize().unwrap().to_string_lossy()
    );
    assert_eq!(
        workbench.tabs.last().unwrap().panel.viewer.as_deref(),
        Some(bed_plugin_image::VIEWER_ID)
    );
}

#[test]
fn workspace_restores_plugin_and_hex_views_with_shared_document_and_state() {
    let dir = TempDir::new();
    let path = dir.write("project/image.png", b"so\0\0ce");
    let mut first = host(&dir);
    first.set_project(&dir.path("project")).unwrap();
    first.open_or_focus(&path).unwrap();
    first
        .open_file_with_viewer(&path, Some("bed.hex"), true)
        .unwrap();
    first
        .tabs
        .last_mut()
        .unwrap()
        .panel
        .hex_mut()
        .unwrap()
        .restore_state(&json!({"cursor":2,"anchor":1,"insert":true}));
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
    let hex = second.tabs.iter().find_map(|tab| tab.panel.hex()).unwrap();
    assert_eq!(hex.state()["cursor"], 2);
    assert_eq!(hex.state()["insert"], true);
}

#[test]
fn workspace_restores_gltf_camera_and_shares_its_bytes_with_hex() {
    let dir = TempDir::new();
    let path = dir.write(
        "project/model.glb",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/gltf/cube.glb"
        )),
    );
    let camera = json!({"camera": {"target": [2.0, 3.0, 4.0], "yaw": 1.0,
        "pitch": 0.5, "distance": 12.0},
        "render": {"lighting": 4, "display": 3, "normals": true,
            "normal_length": 0.125, "skybox": true, "skybox_blur": 0.5, "horizon": -18.0,
            "shadows": false, "ao": false, "exposure": 1.5}});
    let mut first = host(&dir);
    first.set_project(&dir.path("project")).unwrap();
    first.open_or_focus(&path).unwrap();
    let document = first.active_document().unwrap();
    assert_eq!(
        first.session.document_kind(document).unwrap(),
        DocumentKind::Bytes
    );
    let panel = &mut first.tabs.last_mut().unwrap().panel;
    assert_eq!(panel.kind, bed_plugin_gltf::PANEL_ID);
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
        .map(|tab| &tab.panel)
        .find(|panel| panel.viewer.as_deref() == Some(bed_plugin_gltf::VIEWER_ID))
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
fn stl_viewer_routes_and_shares_byte_edits_with_hex() {
    let dir = TempDir::new();
    let bytes = b"solid triangle\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid triangle\n";
    let path = dir.write("project/triangle.STL", bytes);
    let mut workbench = host(&dir);
    workbench.set_project(&dir.path("project")).unwrap();
    workbench.open_or_focus(&path).unwrap();
    assert_eq!(
        workbench.tabs.last().unwrap().panel.kind,
        bed_plugin_gltf::PANEL_ID
    );
    workbench
        .open_file_with_viewer(&path, Some("bed.hex"), false)
        .unwrap();
    workbench
        .open_file_with_viewer(&path, Some(bed_plugin_gltf::VIEWER_ID), false)
        .unwrap();
    let document = workbench.active_document().unwrap();
    assert_eq!(
        workbench.session.document_kind(document).unwrap(),
        DocumentKind::Bytes
    );
    let panel = &workbench.tabs.last().unwrap().panel;
    assert_eq!(panel.kind, bed_plugin_gltf::PANEL_ID);
    assert_eq!(panel.viewer.as_deref(), Some(bed_plugin_gltf::VIEWER_ID));
    workbench
        .open_file_with_viewer(&path, Some("bed.hex"), false)
        .unwrap();
    assert_eq!(workbench.active_document(), Some(document));
    edit(&mut workbench, document, 0..1, b"S");
    workbench.refresh_plugins().unwrap();
    let snapshot = workbench
        .modules
        .frame
        .documents
        .iter()
        .find(|snapshot| snapshot.id == document)
        .unwrap();
    assert_eq!(snapshot.bytes[0], b'S');
    assert_eq!(&snapshot.bytes[1..], &bytes[1..]);
    workbench.session.undo_document(document).unwrap();
    workbench.persist_workspace().unwrap();

    let mut restored = host(&dir);
    restored.restore_last_workspace().unwrap();
    let documents: Vec<_> = restored
        .tabs
        .iter()
        .filter_map(|tab| tab.panel.document())
        .collect();
    assert_eq!(documents.len(), 2);
    assert_eq!(documents[0], documents[1]);
    assert_eq!(restored.panel_count(bed_plugin_gltf::PANEL_ID), 1);
    assert_eq!(restored.panel_count("hex"), 1);
}

#[test]
fn stl_default_viewer_respects_explicit_text_preferences() {
    let dir = TempDir::new();
    let bytes = b"solid triangle\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid triangle\n";
    let mut workbench = host(&dir);
    let path = dir.write("project/default.stl", bytes);
    workbench.open_or_focus(&path).unwrap();
    assert_eq!(
        workbench.tabs.last().unwrap().panel.kind,
        bed_plugin_gltf::PANEL_ID
    );

    workbench.settings.settings["default_viewers"]["stl"] = json!(bed_module_editor::TEXT_VIEWER);
    let path = dir.write("project/text.STL", bytes);
    workbench.open_or_focus(&path).unwrap();
    assert!(workbench.tabs.last().unwrap().panel.editor().is_some());
}

#[test]
fn font_viewer_routes_restores_state_and_shares_byte_edits_with_hex() {
    let dir = TempDir::new();
    let bytes = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/fonts/SourceCodePro-Regular.ttf"
    ));
    let path = dir.write("project/typeface.TTF", bytes);
    let mut first = host(&dir);
    first.set_project(&dir.path("project")).unwrap();
    first.open_or_focus(&path).unwrap();
    let document = first.active_document().unwrap();
    assert_eq!(
        first.session.document_kind(document).unwrap(),
        DocumentKind::Bytes
    );
    let panel = &mut first.tabs.last_mut().unwrap().panel;
    assert_eq!(panel.kind, bed_plugin_font::PANEL_ID);
    assert_eq!(panel.viewer.as_deref(), Some(bed_plugin_font::VIEWER_ID));
    panel.instance = bed_plugin_font::FontPlugin.create_panel(
        bed_plugin_font::PANEL_ID, Some(document),
        &json!({"view": 1, "size": 72, "sample": "office سلام", "selected": 36, "ligatures": false}),
    ).unwrap();
    let expected = panel.instance.save_state();
    first
        .open_file_with_viewer(&path, Some("bed.hex"), true)
        .unwrap();
    assert_eq!(first.active_document(), Some(document));
    edit(&mut first, document, 0..1, b"x");
    first.refresh_plugins().unwrap();
    let plugin_document = first
        .modules
        .frame
        .documents
        .iter()
        .find(|d| d.id == document)
        .unwrap();
    assert_eq!(plugin_document.bytes[0], b'x');
    first.session.undo_document(document).unwrap();
    first.persist_workspace().unwrap();
    let mut second = host(&dir);
    second.restore_last_workspace().unwrap();
    let font = second
        .tabs
        .iter()
        .map(|tab| &tab.panel)
        .find(|panel| panel.viewer.as_deref() == Some(bed_plugin_font::VIEWER_ID))
        .unwrap();
    assert_eq!(font.instance.save_state(), expected);
    let attached: Vec<_> = second
        .tabs
        .iter()
        .filter_map(|tab| tab.panel.document())
        .collect();
    assert_eq!(attached.len(), 2);
    assert_eq!(attached[0], attached[1]);
    assert_eq!(second.panel_count("hex"), 1);
}

#[test]
fn webfont_viewers_save_compressed_bytes_and_share_hex_undo() {
    let fonts: [(&str, &[u8]); 2] = [
        (
            "WOFF",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../crates/bed-plugin-font/tests/fixtures/SourceCodePro-Regular.woff"
            )),
        ),
        (
            "woff2",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../crates/bed-plugin-font/tests/fixtures/SourceCodePro-Regular.woff2"
            )),
        ),
    ];
    for (extension, original) in fonts {
        let dir = TempDir::new();
        let path = dir.write(&format!("project/typeface.{extension}"), original);
        let mut workbench = host(&dir);
        workbench.open_or_focus(&path).unwrap();
        let document = workbench.active_document().unwrap();
        let panel = &workbench.tabs.last().unwrap().panel;
        assert_eq!(panel.kind, bed_plugin_font::PANEL_ID);
        assert_eq!(panel.viewer.as_deref(), Some(bed_plugin_font::VIEWER_ID));
        workbench
            .open_file_with_viewer(&path, Some("bed.hex"), true)
            .unwrap();
        assert_eq!(workbench.active_document(), Some(document));
        edit(&mut workbench, document, 0..1, b"x");
        workbench.handle_action(HostAction::Save).unwrap();
        let mut edited = original.to_vec();
        edited[0] = b'x';
        assert_eq!(fs::read(&path).unwrap(), edited);
        workbench.session.undo_document(document).unwrap();
        workbench.handle_action(HostAction::Save).unwrap();
        assert_eq!(fs::read(&path).unwrap(), original);
        workbench.refresh_plugins().unwrap();
        let plugin_document = workbench
            .modules
            .frame
            .documents
            .iter()
            .find(|d| d.id == document)
            .unwrap();
        assert_eq!(plugin_document.bytes.as_ref(), original);
    }
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
fn rejected_module_close_preserves_panel_order_focus_and_view_until_retry() {
    struct RejectClosePanel {
        inner: Box<dyn PluginPanel>,
        reject: Rc<std::cell::Cell<bool>>,
        attempts: Rc<std::cell::Cell<usize>>,
    }
    impl PluginPanel for RejectClosePanel {
        fn title(&self, host: &HostContext<'_>) -> String {
            self.inner.title(host)
        }
        fn draw(&mut self, ui: &Ui, host: &HostContext<'_>, requests: &mut Vec<HostRequest>) {
            self.inner.draw(ui, host, requests);
        }
        fn attached_document(&self) -> Option<DocumentId> {
            self.inner.attached_document()
        }
        fn view_id(&self) -> Option<ViewId> {
            self.inner.view_id()
        }
        fn action_with_services(
            &mut self,
            action: PanelAction,
            host: &HostContext<'_>,
            services: &mut ModuleServices<'_>,
            requests: &mut Vec<HostRequest>,
        ) -> Result<bool, String> {
            self.inner
                .action_with_services(action, host, services, requests)
        }
        fn close_with_services(
            &mut self,
            services: &mut ModuleServices<'_>,
            requests: &mut Vec<HostRequest>,
        ) -> io::Result<()> {
            self.attempts.set(self.attempts.get() + 1);
            requests.push(HostRequest::Invalidate);
            if self.reject.get() {
                return Err(io::Error::other("Panel is not ready to close"));
            }
            self.inner.close_with_services(services, requests)
        }
        fn as_any(&self) -> &dyn Any {
            self.inner.as_any()
        }
        fn as_any_mut(&mut self) -> &mut dyn Any {
            self.inner.as_any_mut()
        }
    }

    let dir = TempDir::new();
    let first = dir.write("project/first.txt", b"first");
    let second = dir.write("project/second.txt", b"second");
    let mut workbench = host(&dir);
    workbench.open_or_focus(&first).unwrap();
    let first_panel = workbench.focused.unwrap();
    let document = workbench.active_document().unwrap();
    let view = workbench.active_view().unwrap();
    workbench.open_or_focus(&second).unwrap();
    let index = workbench
        .tabs
        .iter()
        .position(|tab| tab.id == first_panel)
        .unwrap();
    workbench.switch_to_tab(index);
    let reject = Rc::new(std::cell::Cell::new(true));
    let attempts = Rc::new(std::cell::Cell::new(0));
    let panel = &mut workbench.tabs[index].panel;
    assert_eq!(panel.kind, bed_module_editor::TEXT_PANEL_TYPE);
    let inner = std::mem::replace(
        &mut panel.instance,
        Box::new(ProbePanel {
            shared: Rc::new(RefCell::new(ProbeState::default())),
            document: Some(document),
        }),
    );
    panel.instance = Box::new(RejectClosePanel {
        inner,
        reject: reject.clone(),
        attempts: attempts.clone(),
    });
    workbench.tabs[index].dock_next = true;
    let context = workbench.command_context();
    workbench
        .modules
        .editor_runtime
        .borrow_mut()
        .set_context(view, context.clone());
    let tab_state = |workbench: &Workbench| {
        workbench
            .tabs
            .iter()
            .map(|tab| (tab.id, tab.focus, tab.dock_next, tab.viewport, tab.dock_id))
            .collect::<Vec<_>>()
    };
    let before_tabs = tab_state(&workbench);
    let before_tiling = workbench.tiling.clone();
    let before_focus = (workbench.focused, workbench.active, workbench.last_document);
    let before_scene = workbench.scene_generation();
    let before_view = workbench.session.view_snapshot(view).unwrap();
    let error = workbench.close_tab(index).unwrap_err();
    assert_eq!(error.to_string(), "Panel is not ready to close");
    assert_eq!(attempts.get(), 1);
    assert_eq!(tab_state(&workbench), before_tabs);
    assert_eq!(workbench.tiling, before_tiling);
    assert_eq!(
        (workbench.focused, workbench.active, workbench.last_document),
        before_focus
    );
    assert_eq!(workbench.scene_generation(), before_scene);
    assert_eq!(workbench.session.document_for_view(view), Some(document));
    assert_eq!(
        workbench.session.view_snapshot(view).unwrap().selections,
        before_view.selections
    );
    assert_eq!(
        workbench.modules.editor_runtime.borrow().context(view),
        Some(&context)
    );
    assert!(
        workbench.modules.requests.is_empty(),
        "requests from a failed close must not escape"
    );

    reject.set(false);
    assert!(workbench.close_tab(index).unwrap());
    assert_eq!(attempts.get(), 2);
    assert!(!workbench.tabs.iter().any(|tab| tab.id == first_panel));
    assert_eq!(workbench.session.document_for_view(view), None);
    assert!(workbench.session.document_kind(document).is_err());
    assert!(
        !workbench
            .modules
            .editor_runtime
            .borrow()
            .context(view)
            .is_some()
    );
    assert_ne!(workbench.active, Some(view));
    assert_ne!(workbench.last_document, Some(document));
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
        .position(|tab| tab.panel.view_id() == Some(view))
        .unwrap();
    let tab = workbench.tabs.remove(index);
    let editor = tab.panel.editor().unwrap();
    workbench.capture_editor_context(editor).unwrap();
    workbench.tabs.insert(index, tab);
    let context = workbench
        .modules
        .editor_runtime
        .borrow()
        .context(view)
        .unwrap()
        .clone();
    assert_eq!(context.selection.as_ref().unwrap().text, "selected text");
    assert_eq!(context.selection.as_ref().unwrap().ranges, vec![0..13]);
    edit(&mut workbench, document, 0..13, b"newer");
    workbench.open_or_focus(&other).unwrap();
    workbench.refresh_plugins().unwrap();
    workbench
        .run_module_command("test.probe.capture", &context)
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
    first.set_project(&dir.path("project")).unwrap();
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
    assert!(workbench.modules.frame.context().texture(handle).is_some());
    workbench.retain_plugin_textures(&HashSet::new());
    assert!(workbench.modules.frame.context().texture(handle).is_none());
}

#[test]
fn bundled_viewer_choices_match_file_content_and_preserve_format_defaults() {
    use bed_module_editor::{HEX_VIEWER, TEXT_VIEWER};

    let dir = TempDir::new();
    let workbench = host(&dir);
    let registry = &workbench.modules.registry;
    let binary = &[0; 32];
    let cases: &[(&str, &[u8], &[&str])] = &[
        ("main.rs", b"fn main() {}\n", &[TEXT_VIEWER, HEX_VIEWER]),
        ("README", b"hello\n", &[TEXT_VIEWER, HEX_VIEWER]),
        ("empty", b"", &[TEXT_VIEWER, HEX_VIEWER]),
        (
            "README.Md",
            b"# Document\n",
            &[TEXT_VIEWER, bed_plugin_markdown::VIEWER_ID, HEX_VIEWER],
        ),
        (
            "guide.markdown",
            b"A **document**\n",
            &[TEXT_VIEWER, bed_plugin_markdown::VIEWER_ID, HEX_VIEWER],
        ),
        (
            "data.JsOn",
            br#"{"answer":42}"#,
            &[TEXT_VIEWER, bed_plugin_json::VIEWER_ID, HEX_VIEWER],
        ),
        (
            "settings.JSONC",
            b"{\n// comment\n\"enabled\":true,\n}",
            &[TEXT_VIEWER, bed_plugin_json::VIEWER_ID, HEX_VIEWER],
        ),
        (
            "requests.HTTP",
            b"GET http://localhost:3000/health\n",
            &[TEXT_VIEWER, HEX_VIEWER],
        ),
        (
            "table.CsV",
            b"name,value\nfirst,1\n",
            &[TEXT_VIEWER, bed_plugin_csv::VIEWER_ID, HEX_VIEWER],
        ),
        (
            "table.TSV",
            b"name\tvalue\nfirst\t1\n",
            &[TEXT_VIEWER, bed_plugin_csv::VIEWER_ID, HEX_VIEWER],
        ),
        (
            "image.PnG",
            b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01",
            &[bed_plugin_image::VIEWER_ID, HEX_VIEWER],
        ),
        (
            "font.TTF",
            b"\0\x01\0\0\0\x12\x01\0\0\x04\0\x20",
            &[bed_plugin_font::VIEWER_ID, HEX_VIEWER],
        ),
        (
            "audio.WAV",
            b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0",
            &[bed_plugin_audio::VIEWER_ID, HEX_VIEWER],
        ),
        (
            "model.GLB",
            b"glTF\x02\0\0\0\x10\0\0\0",
            &[bed_plugin_gltf::VIEWER_ID, HEX_VIEWER],
        ),
        (
            "drawing.SVG",
            br#"<svg xmlns="http://www.w3.org/2000/svg"/>"#,
            &[TEXT_VIEWER, bed_plugin_image::VIEWER_ID, HEX_VIEWER],
        ),
        (
            "scene.gltf",
            br#"{"asset":{"version":"2.0"}}"#,
            &[TEXT_VIEWER, bed_plugin_gltf::VIEWER_ID, HEX_VIEWER],
        ),
        (
            "triangle.STL",
            b"solid triangle\nendsolid triangle\n",
            &[TEXT_VIEWER, bed_plugin_gltf::VIEWER_ID, HEX_VIEWER],
        ),
        (
            "triangle.stl",
            binary,
            &[bed_plugin_gltf::VIEWER_ID, HEX_VIEWER],
        ),
        ("unknown.data", binary, &[HEX_VIEWER]),
        ("misleading.txt", binary, &[HEX_VIEWER]),
        ("misleading.csv", binary, &[HEX_VIEWER]),
        ("misleading.md", binary, &[HEX_VIEWER]),
        ("misleading.json", binary, &[HEX_VIEWER]),
        ("misleading.http", binary, &[HEX_VIEWER]),
    ];
    for &(path, bytes, expected) in cases {
        let kind = Some(bed_files::files::classify_bytes(bytes));
        let viewers: Vec<_> = registry
            .compatible_viewers(path, kind)
            .iter()
            .map(|viewer| viewer.id)
            .collect();
        assert_eq!(viewers, expected, "viewer choices for {path}");
        assert_eq!(
            registry
                .default_viewer_for_file(path, kind)
                .map(|viewer| viewer.id),
            expected.first().copied(),
            "default viewer for {path}"
        );
    }
}

#[test]
fn bundled_format_commands_remain_available_without_duplicate_file_menu_entries() {
    let dir = TempDir::new();
    let workbench = host(&dir);
    let registry = &workbench.modules.registry;
    for (open, open_file) in [
        (
            bed_plugin_markdown::OPEN_COMMAND,
            bed_plugin_markdown::OPEN_FILE_COMMAND,
        ),
        (
            bed_plugin_json::OPEN_COMMAND,
            bed_plugin_json::OPEN_FILE_COMMAND,
        ),
        (
            bed_plugin_csv::OPEN_COMMAND,
            bed_plugin_csv::OPEN_FILE_COMMAND,
        ),
        (
            bed_plugin_image::OPEN_COMMAND,
            bed_plugin_image::OPEN_FILE_COMMAND,
        ),
        (
            bed_plugin_font::OPEN_COMMAND,
            bed_plugin_font::OPEN_FILE_COMMAND,
        ),
        (
            bed_plugin_audio::OPEN_COMMAND,
            bed_plugin_audio::OPEN_FILE_COMMAND,
        ),
        (
            bed_plugin_gltf::OPEN_COMMAND,
            bed_plugin_gltf::OPEN_FILE_COMMAND,
        ),
    ] {
        assert!(registry.command(open).is_some(), "missing command {open}");
        assert!(
            registry.command(open_file).is_some(),
            "missing command {open_file}"
        );
        assert!(
            registry
                .menus
                .iter()
                .any(|entry| { entry.slot == MenuSlot::Application && entry.command == open }),
            "missing Application menu command {open}"
        );
        assert!(
            !registry
                .menus
                .iter()
                .any(|entry| { entry.slot == MenuSlot::File && entry.command == open_file }),
            "duplicate File menu command {open_file}"
        );
    }
}

#[test]
fn document_previews_switch_in_place_and_share_unsaved_text_history() {
    for (name, original, changed, viewer) in [
        (
            "guide.md",
            b"# Original\n".as_slice(),
            b"# Changed\n".as_slice(),
            bed_plugin_markdown::VIEWER_ID,
        ),
        (
            "data.json",
            br#"{"value":1}"#.as_slice(),
            br#"{"value":2}"#.as_slice(),
            bed_plugin_json::VIEWER_ID,
        ),
    ] {
        let dir = TempDir::new();
        let path = dir.write(&format!("project/{name}"), original);
        let mut workbench = host(&dir);
        workbench.settings.settings["autosave"] = json!(false);
        workbench.open_or_focus(&path).unwrap();
        let document = workbench.active_document().unwrap();
        let tab = workbench.tabs.last().unwrap().id;
        assert!(workbench.tabs.last().unwrap().panel.editor().is_some());
        let count = workbench.tabs.len();
        edit(&mut workbench, document, 0..original.len(), changed);
        workbench
            .switch_document_viewer(tab, document, viewer)
            .unwrap();
        assert_eq!(workbench.tabs.len(), count);
        assert_eq!(workbench.active_document(), Some(document));
        assert_eq!(
            workbench.tabs.last().unwrap().panel.viewer.as_deref(),
            Some(viewer)
        );
        assert_eq!(workbench.active_snapshot().unwrap().bytes, changed);
        assert_eq!(fs::read(&path).unwrap(), original);
        workbench.session.undo_document(document).unwrap();
        assert_eq!(workbench.active_snapshot().unwrap().bytes, original);
        workbench.session.redo_document(document).unwrap();
        assert_eq!(workbench.active_snapshot().unwrap().bytes, changed);
        workbench
            .switch_document_viewer(tab, document, "bed.text")
            .unwrap();
        assert!(workbench.tabs.last().unwrap().panel.editor().is_some());
        assert_eq!(workbench.active_snapshot().unwrap().bytes, changed);
        assert_eq!(workbench.session.document_ids(), vec![document]);
    }
}

#[test]
fn document_previews_honor_configured_defaults_and_tree_command_paths() {
    let dir = TempDir::new();
    let markdown = dir.write("project/guide.md", b"# Guide\n");
    let json = dir.write("project/data.json", br#"{"value":1}"#);
    let mut workbench = host(&dir);
    workbench.settings.settings["default_viewers"]["md"] = json!(bed_plugin_markdown::VIEWER_ID);
    workbench.settings.settings["default_viewers"]["json"] = json!(bed_plugin_json::VIEWER_ID);
    for (path, viewer) in [
        (&markdown, bed_plugin_markdown::VIEWER_ID),
        (&json, bed_plugin_json::VIEWER_ID),
    ] {
        workbench.open_or_focus(path).unwrap();
        assert_eq!(
            workbench.tabs.last().unwrap().panel.viewer.as_deref(),
            Some(viewer)
        );
        assert_eq!(
            workbench
                .session
                .document_kind(workbench.active_document().unwrap())
                .unwrap(),
            DocumentKind::Text
        );
    }
    // File-tree actions carry the originating path even when another file has focus.
    workbench
        .handle_tree_action(FileTreeAction::Command {
            command: format!("bed.open_with:{}", bed_plugin_markdown::VIEWER_ID),
            context: CommandContext {
                path: Some(markdown.to_string_lossy().into_owned()),
                ..Default::default()
            },
        })
        .unwrap();
    assert_eq!(
        workbench.active_snapshot().unwrap().path,
        markdown.canonicalize().unwrap().to_string_lossy()
    );
    assert_eq!(
        workbench
            .tabs
            .iter()
            .find(|tab| Some(tab.id) == workbench.active_panel_id())
            .unwrap()
            .panel
            .viewer
            .as_deref(),
        Some(bed_plugin_markdown::VIEWER_ID)
    );
}

#[test]
fn document_previews_restore_alongside_their_shared_text_editors() {
    for (name, bytes, viewer) in [
        (
            "guide.md",
            b"# Guide\n".as_slice(),
            bed_plugin_markdown::VIEWER_ID,
        ),
        (
            "data.jsonc",
            b"{ /* note */ \"enabled\": true, }".as_slice(),
            bed_plugin_json::VIEWER_ID,
        ),
    ] {
        let dir = TempDir::new();
        let path = dir.write(&format!("project/{name}"), bytes);
        let mut first = host(&dir);
        first.set_project(&dir.path("project")).unwrap();
        first.open_or_focus(&path).unwrap();
        let document = first.active_document().unwrap();
        first
            .open_file_with_viewer(&path, Some(viewer), true)
            .unwrap();
        assert_eq!(first.active_document(), Some(document));
        first.persist_workspace().unwrap();
        let mut restored = host(&dir);
        restored.restore_last_workspace().unwrap();
        let views: Vec<_> = restored
            .tabs
            .iter()
            .filter(|tab| tab.panel.document().is_some())
            .collect();
        assert_eq!(views.len(), 2);
        assert_eq!(views[0].panel.document(), views[1].panel.document());
        assert!(
            views
                .iter()
                .any(|tab| tab.panel.viewer.as_deref() == Some(viewer))
        );
        assert!(views.iter().any(|tab| tab.panel.editor().is_some()));
        assert_eq!(
            restored
                .session
                .snapshot(views[0].panel.document().unwrap())
                .unwrap()
                .bytes,
            bytes
        );
    }
}

fn http_saved_requests(url: &str) -> Value {
    json!({"version":1,"requests":[{
        "id":7,"name":"Health","method":"GET","url":url,
        "params":[{"enabled":false,"name":"draft","value":"unfinished"}],
        "headers":[{"enabled":true,"name":"Accept","value":"application/json"}],
        "body":"","auth":{"type":"bearer","token":"fixture-token"}
    }],"selected":7})
}

fn http_workspace(workbench: &Workbench) -> Value {
    workbench
        .modules
        .instances
        .iter()
        .find(|module| module.id() == bed_plugin_http::MODULE_ID)
        .expect("HTTP module is bundled")
        .save_workspace()
}

fn restore_http_requests(workbench: &mut Workbench, state: &Value) {
    let root = workbench.project_root.clone();
    workbench
        .modules
        .instances
        .iter_mut()
        .find(|module| module.id() == bed_plugin_http::MODULE_ID)
        .expect("HTTP module is bundled")
        .restore_workspace(Some(state), &root);
}

#[test]
fn http_command_opens_one_workspace_panel_without_documents() {
    let dir = TempDir::new();
    let mut workbench = host(&dir);
    let descriptor = workbench
        .modules
        .registry
        .panel(bed_plugin_http::PANEL_ID)
        .unwrap();
    assert!(descriptor.singleton);
    assert_eq!(
        descriptor.open_placement,
        bed_workbench_api::PanelPlacement::Center
    );
    let count = workbench.tabs.len();
    assert!(
        workbench
            .dispatch_command(bed_plugin_http::OPEN_COMMAND)
            .unwrap()
    );
    let panel = workbench.active_panel_id().unwrap();
    assert_eq!(workbench.tabs.len(), count + 1);
    assert!(
        workbench
            .dispatch_command(bed_plugin_http::OPEN_COMMAND)
            .unwrap()
    );
    assert_eq!(workbench.active_panel_id(), Some(panel));
    assert_eq!(workbench.panel_count(bed_plugin_http::PANEL_ID), 1);
    let panel = &workbench
        .tabs
        .iter()
        .find(|tab| tab.id == panel)
        .unwrap()
        .panel;
    assert!(panel.document().is_none());
    assert!(panel.viewer.is_none());
    assert!(workbench.session.document_ids().is_empty());
}

#[test]
fn http_files_open_as_text_without_an_http_file_viewer() {
    let dir = TempDir::new();
    let bytes = b"GET http://localhost:3000/health\n";
    let path = dir.write("project/requests.HTTP", bytes);
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    assert!(workbench.tabs.last().unwrap().panel.editor().is_some());
    assert_eq!(workbench.active_snapshot().unwrap().bytes, bytes);
    assert_eq!(workbench.panel_count(bed_plugin_http::PANEL_ID), 0);
    assert!(
        workbench.settings.settings["default_viewers"]
            .get("http")
            .is_none()
    );
}

#[test]
fn http_project_requests_restore_with_the_panel_closed_and_stay_isolated() {
    let dir = TempDir::new();
    dir.write("first/source.txt", b"first");
    dir.write("second/source.txt", b"second");
    let saved = http_saved_requests("http://127.0.0.1:1/first");
    let mut first = host(&dir);
    first.set_project(&dir.path("first")).unwrap();
    restore_http_requests(&mut first, &saved);
    assert!(
        first
            .dispatch_command(bed_plugin_http::OPEN_COMMAND)
            .unwrap()
    );
    let index = first
        .tabs
        .iter()
        .position(|tab| tab.panel.kind == bed_plugin_http::PANEL_ID)
        .unwrap();
    assert!(first.close_tab(index).unwrap());
    first.persist_workspace().unwrap();
    let mut restored = host(&dir);
    restored.restore_last_workspace().unwrap();
    assert_eq!(restored.panel_count(bed_plugin_http::PANEL_ID), 0);
    assert_eq!(http_workspace(&restored), saved);
    assert!(
        restored
            .dispatch_command(bed_plugin_http::OPEN_COMMAND)
            .unwrap()
    );
    assert_eq!(http_workspace(&restored), saved);
    restored.set_project(&dir.path("second")).unwrap();
    assert_eq!(
        restored
            .take_workspace_windows()
            .into_iter()
            .map(|spec| PathBuf::from(spec.root))
            .collect::<Vec<_>>(),
        vec![dir.path("second").canonicalize().unwrap()]
    );
    assert_eq!(http_workspace(&restored), saved);
    let mut other = host(&dir);
    other.open_startup_workspace(&dir.path("second")).unwrap();
    assert_eq!(http_workspace(&other)["requests"], json!([]));
    let second = http_saved_requests("http://127.0.0.1:1/second");
    restore_http_requests(&mut other, &second);
    other.persist_workspace().unwrap();
    other.set_project(&dir.path("first")).unwrap();
    assert_eq!(
        other
            .take_workspace_windows()
            .into_iter()
            .map(|spec| PathBuf::from(spec.root))
            .collect::<Vec<_>>(),
        vec![dir.path("first").canonicalize().unwrap()]
    );
    assert_eq!(http_workspace(&other), second);
    assert_eq!(http_workspace(&restored), saved);
    let mut reopened = host(&dir);
    reopened
        .open_startup_workspace(&dir.path("second"))
        .unwrap();
    assert_eq!(http_workspace(&reopened), second);
}

#[test]
fn http_standalone_requests_stay_in_memory_and_do_not_run_when_opened() {
    use std::{
        io::ErrorKind,
        net::TcpListener,
        thread,
        time::{Duration, Instant},
    };

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let saved = http_saved_requests(&format!("http://{}/health", listener.local_addr().unwrap()));
    let dir = TempDir::new();
    let mut first = host(&dir);
    restore_http_requests(&mut first, &saved);
    assert!(
        first
            .dispatch_command(bed_plugin_http::OPEN_COMMAND)
            .unwrap()
    );
    let index = first
        .tabs
        .iter()
        .position(|tab| tab.panel.kind == bed_plugin_http::PANEL_ID)
        .unwrap();
    assert!(first.close_tab(index).unwrap());
    first.tick_plugins().unwrap();
    assert_eq!(http_workspace(&first), saved);
    first.persist_workspace().unwrap();
    assert!(!dir.path("config/workspaces.json").exists());
    let mut restored = host(&dir);
    assert_eq!(restored.panel_count(bed_plugin_http::PANEL_ID), 0);
    assert_eq!(http_workspace(&restored)["requests"], json!([]));
    assert!(
        restored
            .dispatch_command(bed_plugin_http::OPEN_COMMAND)
            .unwrap()
    );
    restored.tick_plugins().unwrap();
    assert_eq!(http_workspace(&restored)["requests"], json!([]));
    assert!(restored.session.document_ids().is_empty());
    let deadline = Instant::now() + Duration::from_millis(100);
    while Instant::now() < deadline {
        assert!(
            matches!(listener.accept(), Err(error) if error.kind() == ErrorKind::WouldBlock),
            "opening or restoring HTTP Client must not send requests"
        );
        thread::sleep(Duration::from_millis(5));
    }
    dir.write("project/source.txt", b"project");
    restored.set_project(&dir.path("project")).unwrap();
    assert_eq!(http_workspace(&restored)["requests"], json!([]));
    restored.persist_workspace().unwrap();
    assert_eq!(http_workspace(&host(&dir))["requests"], json!([]));
}

#[test]
fn http_module_settings_use_ssh_host_and_root_identity_without_a_connection() {
    let dir = TempDir::new();
    let mut workbench = host(&dir);
    let remote = |host: &str| WorkspaceSpec {
        name: "Remote".into(),
        target: WorkspaceTarget::Ssh { host: host.into() },
        root: "/srv/project".into(),
    };
    let first = remote("first-host");
    let second = remote("second-host");
    let saved = http_saved_requests("http://127.0.0.1:1/first-host");
    workbench.workspace_spec = Some(first.clone());
    workbench.project_root = first.root.clone();
    restore_http_requests(&mut workbench, &saved);
    workbench.persist_module_settings().unwrap();
    workbench.workspace_spec = Some(second.clone());
    workbench.restore_module_settings();
    assert_eq!(http_workspace(&workbench)["requests"], json!([]));
    let other = http_saved_requests("http://127.0.0.1:1/second-host");
    restore_http_requests(&mut workbench, &other);
    workbench.persist_module_settings().unwrap();
    workbench.workspace_spec = Some(first);
    workbench.restore_module_settings();
    assert_eq!(http_workspace(&workbench), saved);
    workbench.workspace_spec = Some(WorkspaceSpec::local("/srv/project"));
    workbench.restore_module_settings();
    assert_eq!(http_workspace(&workbench)["requests"], json!([]));
}
