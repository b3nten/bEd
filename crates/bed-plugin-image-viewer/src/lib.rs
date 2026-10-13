//! A read-only raster and SVG viewer implemented entirely through the plugin API.
use bed_api::gpu::GpuContext;
use bed_api::{
    documents::{DocumentKind, Revision},
    features::{FeatureRegistrar, HostAction, HostContext},
};
use bed_editing::identity::DocumentId;
use bed_ui::TextureHandle;
use bed_ui::gpu::{Canvas, RenderOutput, RenderTarget};
use dear_imgui_rs::{MouseButton, Ui};
use serde_json::{Value, json};
use std::{
    sync::{Arc, mpsc},
    thread,
};

pub const PLUGIN_ID: &str = "bed.image";
pub const PANEL_ID: &str = "bed.image.panel";
pub const VIEWER_ID: &str = "bed.image.viewer";
pub const OPEN_COMMAND: &str = "bed.image.open";
pub const OPEN_FILE_COMMAND: &str = "bed.image.open-file";
pub const MAX_DECODED_BYTES: u64 = 64 * 1024 * 1024;

mod ecs;
mod formats;
mod render;
mod status;
pub use ecs::{ImageDocument, ImageStates, ImageSystems, ImageViewerPlugin};
use formats::{SUPPORTED_EXTENSIONS, decode, supported_path};

#[derive(Clone, Copy, PartialEq)]
struct CanvasState {
    size: [f32; 2],
    pixels: [u32; 2],
    scale: f32,
    pan: [f32; 2],
}

fn register_metadata(registrar: &mut FeatureRegistrar<'_>) {
    registrar.panel_ref(PANEL_ID);
    registrar.panel_requires_input(PANEL_ID);
    registrar.viewer(
        VIEWER_ID,
        "Image Viewer",
        PANEL_ID,
        SUPPORTED_EXTENSIONS,
        DocumentKind::Bytes,
    );
    registrar.viewer_document_kinds(VIEWER_ID, &[DocumentKind::Text, DocumentKind::Bytes]);
    registrar.command_ref(OPEN_COMMAND);
    registrar.command_ref(OPEN_FILE_COMMAND);
    registrar.settings_ui("bed.image.settings", "Image Viewer", image_settings);
}

fn image_settings(ui: &Ui, settings: &mut Value) -> bool {
    let mut fit = settings["fit_by_default"].as_bool().unwrap_or(true);
    if ui.checkbox("Fit images to panel by default", &mut fit) {
        if !settings.is_object() {
            *settings = json!({});
        }
        settings["fit_by_default"] = json!(fit);
        return true;
    }
    false
}

/// Bounded, straight-alpha RGBA pixels reusable by document previews.
#[derive(Clone, Debug)]
pub struct DecodedImage {
    pub size: [u32; 2],
    pub rgba: Arc<[u8]>,
}

/// Decode a raster or self-contained SVG using the image viewer's allocation limits.
/// Call this on a worker: decoding and SVG font discovery may be expensive.
pub fn decode_preview(bytes: &[u8], path: &str) -> Result<DecodedImage, String> {
    decode(bytes, path)
}

struct DecodeJob {
    revision: Revision,
    bytes: Arc<[u8]>,
    path: String,
}

struct DecodeResult {
    revision: Revision,
    result: Result<DecodedImage, String>,
}

/// Each panel has a bounded queue and one worker. Closing drops its sender;
/// the worker completes its current bounded decode and then exits.
struct Decoder {
    sender: Option<mpsc::SyncSender<DecodeJob>>,
    receiver: mpsc::Receiver<DecodeResult>,
}

impl Decoder {
    fn new() -> Self {
        let (sender, jobs) = mpsc::sync_channel::<DecodeJob>(1);
        let (results, receiver) = mpsc::sync_channel(1);
        thread::spawn(move || {
            while let Ok(mut job) = jobs.recv() {
                while let Ok(latest) = jobs.try_recv() {
                    job = latest;
                }
                let result = DecodeResult {
                    revision: job.revision,
                    result: decode(&job.bytes, &job.path),
                };
                if results.send(result).is_err() {
                    break;
                }
            }
        });
        Self {
            sender: Some(sender),
            receiver,
        }
    }

    fn submit(&self, job: DecodeJob) -> bool {
        self.sender
            .as_ref()
            .is_some_and(|sender| sender.try_send(job).is_ok())
    }
}

pub struct ImagePanel {
    shortcuts: bed_keybindings::ShortcutInput,
    document: DocumentId,
    texture: TextureHandle,
    decoder: Decoder,
    requested: Option<Revision>,
    decoded: Option<DecodedImage>,
    error: Option<String>,
    gpu: Option<render::ImageGpu>,
    canvas: Option<CanvasState>,
    output_revision: u64,
    fit: Option<bool>,
    zoom: f32,
    pan: [f32; 2],
}

impl ImagePanel {
    fn new(document: DocumentId, state: &Value) -> Self {
        let finite = |value: Option<f64>, fallback: f32| {
            value
                .filter(|v| v.is_finite() && v.abs() <= f32::MAX as f64)
                .map_or(fallback, |v| v as f32)
        };
        Self {
            shortcuts: bed_keybindings::ShortcutInput::new(bed_keybindings::Bindings::new(
                bed_keybindings::common_actions(),
                &Default::default(),
                bed_keybindings::Platform::current(),
            ))
            .with_scope(bed_keybindings::Scope::panel(PANEL_ID)),
            document,
            texture: TextureHandle::next(),
            decoder: Decoder::new(),
            requested: None,
            decoded: None,
            error: None,
            gpu: None,
            canvas: None,
            output_revision: 0,
            fit: state["fit"].as_bool(),
            zoom: finite(state["zoom"].as_f64(), 1.0).clamp(0.01, 32.0),
            pan: [
                finite(state["pan"][0].as_f64(), 0.0),
                finite(state["pan"][1].as_f64(), 0.0),
            ],
        }
    }

    fn update(&mut self, host: &HostContext<'_>) {
        let Some(document) = host.document(self.document) else {
            self.decoded = None;
            self.gpu = None;
            return;
        };
        while let Ok(result) = self.decoder.receiver.try_recv() {
            if result.revision != document.revision {
                continue;
            }
            match result.result {
                Ok(decoded) => {
                    self.decoded = Some(decoded);
                    self.error = None;
                    self.gpu = None;
                    self.output_revision += 1;
                }
                Err(error) => {
                    self.decoded = None;
                    self.error = Some(error);
                    self.gpu = None;
                }
            }
        }
        if self.requested != Some(document.revision)
            && self.decoder.submit(DecodeJob {
                revision: document.revision,
                bytes: Arc::clone(&document.bytes),
                path: document.path.clone(),
            })
        {
            self.requested = Some(document.revision);
            self.decoded = None;
            self.error = None;
            self.gpu = None;
        }
    }

    fn draw_with_icons(
        &mut self,
        ui: &Ui,
        host: &HostContext<'_>,
        requests: &mut Vec<HostAction>,
        icons: Option<&bed_ui::icons::Icons>,
    ) {
        self.update(host);
        self.draw_prepared(ui, host, requests, icons);
    }

    fn draw_prepared(
        &mut self,
        ui: &Ui,
        host: &HostContext<'_>,
        requests: &mut Vec<HostAction>,
        icons: Option<&bed_ui::icons::Icons>,
    ) {
        let _controls = bed_ui::components::controls_style(ui, &bed_ui::UiFrameContext::for_ui(ui));
        let fit = self.fit.get_or_insert_with(|| {
            host.settings_for(PLUGIN_ID)["fit_by_default"]
                .as_bool()
                .unwrap_or(true)
        });
        if ui.button("Fit") {
            *fit = true;
            self.pan = [0.0; 2];
        }
        ui.same_line();
        if ui.button("100%") {
            *fit = false;
            self.zoom = 1.0;
            self.pan = [0.0; 2];
        }
        ui.same_line();
        if bed_ui::components::icon_button(
            ui,
            &bed_ui::UiFrameContext::for_ui(ui),
            icons,
            "bed.core.minus",
            "zoom-out",
            "−",
            [0.0; 2],
        ) {
            *fit = false;
            self.zoom = (self.zoom / 1.25).max(0.01);
        }
        if ui.is_item_hovered() {
            bed_ui::components::tooltip_text(ui, &bed_ui::UiFrameContext::for_ui(ui), "Zoom out");
        }
        ui.same_line();
        if bed_ui::components::icon_button(
            ui,
            &bed_ui::UiFrameContext::for_ui(ui),
            icons,
            "bed.core.plus",
            "zoom-in",
            "+",
            [0.0; 2],
        ) {
            *fit = false;
            self.zoom = (self.zoom * 1.25).min(32.0);
        }
        if ui.is_item_hovered() {
            bed_ui::components::tooltip_text(ui, &bed_ui::UiFrameContext::for_ui(ui), "Zoom in");
        }
        ui.same_line();
        if ui.button("Info") {
            ui.open_popup("image-information");
        }
        {
            let _popup_style =
                bed_ui::components::context_menu_style(ui, &bed_ui::UiFrameContext::for_ui(ui));
            if let Some(_popup) = ui.begin_popup("image-information") {
                if let Some(document) = host.document(self.document) {
                    ui.text_wrapped(&document.path);
                    ui.text(format!("{} bytes", document.bytes.len()));
                }
                if let Some(decoded) = &self.decoded {
                    ui.text(format!("{} × {} pixels", decoded.size[0], decoded.size[1]));
                }
                ui.text_disabled("Read-only image viewer");
            }
        }
        ui.separator();
        if host.document(self.document).is_none() {
            ui.text_disabled("The image document is closed");
            return;
        }
        if let Some(error) = &self.error {
            ui.text_wrapped(format!("Could not display image: {error}"));
            if ui.button("Open in Hex Editor")
                && let Some(document) = host.document(self.document)
            {
                requests.push(HostAction::OpenFile {
                    path: document.path.clone(),
                    viewer: Some("bed.editor.hex".into()),
                });
            }
            return;
        }
        let Some(decoded) = &self.decoded else {
            ui.text_disabled("Loading image…");
            return;
        };
        let canvas = Canvas::show(ui, host.texture(self.texture), "image-canvas");
        let fit_scale =
            (canvas.size[0] / decoded.size[0] as f32).min(canvas.size[1] / decoded.size[1] as f32);
        let mut scale = if *fit { fit_scale } else { self.zoom };
        if !self.shortcuts.suppressed(ui)
            && !ui.io().want_text_input()
            && !ui.is_any_item_active()
            && !ui.is_popup_open_with_flags(
                "",
                dear_imgui_rs::PopupQueryFlags::ANY_POPUP_ID
                    | dear_imgui_rs::PopupQueryFlags::ANY_POPUP_LEVEL,
            )
            && ui.is_window_focused_with_flags(dear_imgui_rs::FocusedFlags::ROOT_AND_CHILD_WINDOWS)
        {
            for (action, factor) in [
                (bed_keybindings::ZOOM_IN, 1.25),
                (bed_keybindings::ZOOM_OUT, 1.0 / 1.25),
            ] {
                if self.shortcuts.take(ui, action) {
                    self.zoom = (scale * factor).clamp(0.01, 32.0);
                    *fit = false;
                    scale = self.zoom;
                }
            }
            if self.shortcuts.take(ui, bed_keybindings::RESET_ZOOM) {
                *fit = false;
                self.zoom = 1.0;
                self.pan = [0.0; 2];
                scale = self.zoom;
            }
        }
        if canvas.hovered {
            let wheel = ui.io().mouse_wheel();
            if wheel != 0.0 {
                self.zoom = (scale * 1.2f32.powf(wheel)).clamp(0.01, 32.0);
                *fit = false;
                scale = self.zoom;
            }
        }
        if canvas.active && ui.is_mouse_dragging(MouseButton::Left) {
            let delta = ui.io().mouse_delta();
            self.pan[0] += delta[0];
            self.pan[1] += delta[1];
        }
        let state = CanvasState {
            size: canvas.size,
            pixels: canvas.pixels,
            scale,
            pan: self.pan,
        };
        if self.canvas != Some(state) {
            self.canvas = Some(state);
            self.output_revision += 1;
        }
    }
}

impl ImagePanel {
    fn panel_title(&self, host: &HostContext<'_>) -> String {
        host.document(self.document)
            .and_then(|document| document.path.rsplit(['/', '\\']).next())
            .unwrap_or("Image Viewer")
            .to_owned()
    }

    fn saved_state(&self) -> Value {
        json!({ "fit": self.fit, "zoom": self.zoom, "pan": self.pan })
    }

    fn canvas_output(&self) -> Option<RenderOutput> {
        self.decoded.as_ref()?;
        Some(RenderOutput {
            handle: self.texture,
            size: self.canvas?.pixels,
            depth: false,
            revision: self.output_revision,
        })
    }

    fn record_render(
        &mut self,
        gpu: &mut GpuContext<'_>,
        target: &RenderTarget,
    ) -> Result<(), String> {
        let decoded = self.decoded.as_ref().ok_or("Image is not decoded")?;
        if self
            .gpu
            .as_ref()
            .is_none_or(|state| state.generation != gpu.generation)
        {
            match render::ImageGpu::new(gpu, decoded) {
                Ok(state) => self.gpu = Some(state),
                Err(error) => {
                    self.error = Some(error.clone());
                    self.decoded = None;
                    return Err(error);
                }
            }
        }
        self.gpu.as_ref().unwrap().render(
            gpu,
            target,
            self.canvas.ok_or("Image canvas is not sized")?,
        );
        Ok(())
    }
}

impl ImagePanel {
    pub fn title(&self, host: &HostContext<'_>) -> String {
        self.panel_title(host)
    }

    pub fn attached_document(&self) -> Option<DocumentId> {
        Some(self.document)
    }

    pub fn save_state(&self) -> Value {
        self.saved_state()
    }

    pub fn close(&mut self, _requests: &mut Vec<HostAction>) {
        self.decoder.sender = None;
        self.decoded = None;
        self.gpu = None;
    }

    pub fn render_output(&self) -> Option<RenderOutput> {
        self.canvas_output()
    }

    pub fn render(
        &mut self,
        gpu: &mut GpuContext<'_>,
        target: &RenderTarget,
    ) -> Result<(), String> {
        self.record_render(gpu, target)
    }

    pub fn draw(&mut self, ui: &Ui, host: &HostContext<'_>, requests: &mut Vec<HostAction>) {
        self.draw_with_icons(ui, host, requests, None);
    }
}

#[cfg(test)]
static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use bed_api::features::MenuSlot;
    use std::{
        collections::HashMap,
        io::Cursor,
        time::{Duration, Instant},
    };

    fn png(width: u32, height: u32) -> Arc<[u8]> {
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            width,
            height,
            image::Rgba([12, 34, 56, 255]),
        ));
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        bytes.into_inner().into()
    }

    #[test]
    fn common_zoom_overrides_target_focused_images_and_reset_actual_size() {
        use bed_keybindings::{Bindings, Overrides, Platform, Scope, ShortcutInput};
        use dear_imgui_rs::{Condition, Context, FramePrepareOptions, Key};
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut frame_data = bed_api::features::FrontendSnapshot::default();
        frame_data
            .documents
            .push(bed_api::documents::PluginDocument {
                id: DocumentId(1),
                path: "image.png".into(),
                kind: DocumentKind::Bytes,
                language_id: String::new(),
                revision: (1, 1),
                dirty: false,
                bytes: png(100, 100),
                text: None,
            });
        let actions = bed_keybindings::common_actions();
        let mut overrides = Overrides::default();
        overrides
            .replace(
                actions
                    .iter()
                    .find(|a| a.id == bed_keybindings::ZOOM_IN)
                    .unwrap(),
                Some(0),
                "Ctrl+J".parse().unwrap(),
            )
            .unwrap();
        let input = ShortcutInput::new(Bindings::new(actions, &overrides, Platform::current()))
            .with_scope(Scope::panel(PANEL_ID));
        let mut panels = [
            ImagePanel::new(DocumentId(1), &Value::Null),
            ImagePanel::new(DocumentId(1), &json!({"fit": false})),
        ];
        for panel in &mut panels {
            panel.shortcuts = input.clone();
            panel.decoded = Some(DecodedImage {
                size: [100, 100],
                rgba: vec![255; 40000].into(),
            });
        }
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context.io_mut().set_config_macosx_behaviors(false);
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let frame = |context: &mut Context,
                     panels: &mut [ImagePanel; 2],
                     first: bool,
                     suppressed: bool| {
            context.prepare_frame(FramePrepareOptions::new([800.0, 300.0], 1.0 / 60.0));
            let ui = context.frame();
            if suppressed {
                input.suppress(ui);
            }
            for index in [1, 0] {
                ui.window(if index == 0 {
                    "First image"
                } else {
                    "Second image"
                })
                .position([index as f32 * 400.0, 0.0], Condition::Always)
                .size([390.0, 290.0], Condition::Always)
                .focused(first == (index == 0))
                .build(|| {
                    panels[index].draw_prepared(ui, &frame_data.context(), &mut Vec::new(), None);
                });
            }
            drop(context.render_legacy());
        };
        for _ in 0..3 {
            frame(&mut context, &mut panels, true, false);
        }
        let fitted = panels[0].canvas.unwrap().scale;
        context.io_mut().add_key_event(Key::ModCtrl, true);
        context.io_mut().add_key_event(Key::J, true);
        frame(&mut context, &mut panels, true, false);
        assert!((panels[0].zoom - fitted * 1.25).abs() < 0.00001);
        assert_eq!(panels[0].fit, Some(false));
        assert_eq!(panels[1].zoom, 1.0);
        context.io_mut().add_key_event(Key::J, false);
        frame(&mut context, &mut panels, false, false);
        context.io_mut().add_key_event(Key::J, true);
        frame(&mut context, &mut panels, false, true);
        assert_eq!(panels[1].zoom, 1.0);
        context.io_mut().add_key_event(Key::J, false);
        frame(&mut context, &mut panels, false, false);
        context.io_mut().add_key_event(Key::J, true);
        frame(&mut context, &mut panels, false, false);
        assert_eq!(panels[1].zoom, 1.25);
        context.io_mut().add_key_event(Key::J, false);
        context.io_mut().add_key_event(Key::ModCtrl, false);
        frame(&mut context, &mut panels, true, false);
        panels[0].pan = [10.0, 20.0];
        context.io_mut().add_key_event(
            if cfg!(target_os = "macos") {
                Key::ModSuper
            } else {
                Key::ModCtrl
            },
            true,
        );
        context.io_mut().add_key_event(Key::Key0, true);
        frame(&mut context, &mut panels, true, false);
        assert_eq!(panels[0].zoom, 1.0);
        assert_eq!(panels[0].pan, [0.0; 2]);
        assert_eq!(panels[1].zoom, 1.25);
    }

    #[test]
    fn text_backed_svg_preview_tracks_unsaved_revisions() {
        let mut panel = ImagePanel::new(DocumentId(1), &Value::Null);
        let mut document = bed_api::documents::PluginDocument {
            id: DocumentId(1),
            path: "drawing.svg".into(),
            kind: DocumentKind::Text,
            language_id: "xml".into(),
            revision: (1, 1),
            dirty: true,
            text: None,
            bytes: Arc::from(
                &br#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="3"/>"#[..],
            ),
        };
        let textures = HashMap::new();
        for (revision, expected) in [(1, [2, 3]), (2, [4, 5])] {
            document.revision = (1, revision);
            if revision == 2 {
                document.bytes = Arc::from(
                    &br#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="5"/>"#[..],
                );
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                panel.update(&HostContext {
                    remote: false,
                    documents: std::slice::from_ref(&document),
                    active_document: Some(document.id),
                    settings: &Value::Null,
                    textures: &textures,
                    animations: false,
                    session_id: 1,
                    diagnostics: &Value::Null,
                    viewer_menu: None,
                    default_viewers: &Value::Null,
                });
                if panel
                    .decoded
                    .as_ref()
                    .is_some_and(|decoded| decoded.size == expected)
                {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "SVG decode failed: {:?}",
                    panel.error
                );
                thread::sleep(Duration::from_millis(2));
            }
        }
    }

    #[test]
    fn worker_discards_stale_results_and_close_releases_gpu_output() {
        let mut panel = ImagePanel::new(DocumentId(1), &Value::Null);
        let mut documents = [bed_api::documents::PluginDocument {
            id: DocumentId(1),
            path: "image.png".into(),
            kind: DocumentKind::Bytes,
            language_id: String::new(),
            revision: (1, 1),
            dirty: false,
            bytes: png(1, 1),
            text: None,
        }];
        let textures = HashMap::new();
        let settings = Value::Null;
        let mut requests = Vec::new();
        panel.update(&HostContext {
            remote: false,
            default_viewers: &Value::Null,
            viewer_menu: None,
            documents: &documents,
            active_document: Some(DocumentId(1)),
            settings: &settings,
            textures: &textures,
            animations: false,
            session_id: 1,
            diagnostics: &Value::Null,
        });
        documents[0].revision = (1, 2);
        documents[0].bytes = png(2, 3);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            panel.update(&HostContext {
                remote: false,
                default_viewers: &Value::Null,
                viewer_menu: None,
                documents: &documents,
                active_document: Some(DocumentId(1)),
                settings: &settings,
                textures: &textures,
                animations: false,
                session_id: 1,
                diagnostics: &Value::Null,
            });
            if panel.decoded.is_some() {
                break;
            }
            assert!(Instant::now() < deadline, "image worker timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(panel.decoded.as_ref().unwrap().size, [2, 3]);
        panel.canvas = Some(CanvasState {
            size: [100.0; 2],
            pixels: [200; 2],
            scale: 1.0,
            pan: [0.0; 2],
        });
        assert_eq!(panel.render_output().unwrap().size, [200; 2]);
        panel.close(&mut requests);
        assert!(panel.decoder.sender.is_none());
        assert!(panel.render_output().is_none());
        assert!(panel.gpu.is_none());
    }

    #[test]
    fn file_menu_command_keeps_captured_path_after_focus_changes() {
        let mut app = bevy::app::App::new();
        app.add_plugins(ImageViewerPlugin);
        let command = app
            .world()
            .resource::<bed_api::workbench::WorkbenchRegistry>()
            .command(OPEN_FILE_COMMAND)
            .unwrap()
            .handler;
        app.world_mut()
            .run_system_with(
                command,
                bed_api::workbench::CommandInvocation {
                    command: OPEN_FILE_COMMAND.into(),
                    source: None,
                    area: 4,
                    arguments: serde_json::json!({"path":"selected.png"}),
                },
            )
            .unwrap()
            .unwrap();
        let requests = app
            .world_mut()
            .resource_mut::<bed_api::features::HostEffects>()
            .take();
        assert!(
            matches!(requests.as_slice(),[bed_api::features::HostEffect{action:HostAction::OpenFile{path,viewer:Some(viewer)},..}] if path=="selected.png"&&viewer==VIEWER_ID)
        );
    }

    #[test]
    fn image_registration_exercises_contributions_and_extension_matching() {
        let mut registry = bed_api::features::FeatureRegistry::default();
        registry
            .register_with(PLUGIN_ID, register_metadata)
            .unwrap();
        for extension in SUPPORTED_EXTENSIONS {
            let path = format!("/images/a.{}", extension.to_ascii_uppercase());
            assert_eq!(registry.viewer_for_path(&path).unwrap().id, VIEWER_ID);
            assert!(supported_path(&path));
        }
        assert!(registry.viewer_for_path("file.rs").is_none());
        assert!(!supported_path("/images.svg/file"));
        assert!(!supported_path("C:\\images.svg\\file"));
        assert_eq!(registry.settings().len(), 1);
        assert!(registry.toolbar().is_empty());
        assert!(
            registry
                .menus()
                .iter()
                .all(|entry| entry.slot != MenuSlot::File)
        );
        assert!(
            !registry.menus().iter().any(|entry| {
                entry.slot == MenuSlot::Application && entry.command == OPEN_COMMAND
            })
        );
    }
}
