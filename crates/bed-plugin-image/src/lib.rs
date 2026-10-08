//! A read-only raster and SVG viewer implemented entirely through the plugin API.
use bed_core::identity::DocumentId;
use bed_plugin::gpu::{Canvas, GpuContext, RenderOutput, RenderTarget};
use bed_plugin::{
    CommandContext, DocumentKind, HostContext, HostRequest, MenuSlot, Plugin, PluginPanel,
    Registrar, Revision, TextureHandle,
};
use dear_imgui_rs::{MouseButton, StyleColor, Ui};
use serde_json::{Value, json};
use std::{
    any::Any,
    sync::{Arc, mpsc},
    thread,
};

pub const PLUGIN_ID: &str = "bed.image";
pub const PANEL_ID: &str = "bed.image.panel";
pub const VIEWER_ID: &str = "bed.image.viewer";
pub const OPEN_COMMAND: &str = "bed.image.open";
pub const OPEN_FILE_COMMAND: &str = "bed.image.open-file";
pub const MAX_DECODED_BYTES: u64 = 64 * 1024 * 1024;

mod formats;
mod render;
use formats::{SUPPORTED_EXTENSIONS, decode, supported_path};

#[derive(Clone, Copy, PartialEq)]
struct CanvasState {
    size: [f32; 2],
    pixels: [u32; 2],
    scale: f32,
    pan: [f32; 2],
    background: [f32; 4],
}

#[derive(Default)]
pub struct ImagePlugin {}
impl Plugin for ImagePlugin {
    fn id(&self) -> &'static str {
        PLUGIN_ID
    }
    fn register(&self, registrar: &mut Registrar<'_>) {
        registrar.panel(PANEL_ID, "Image Viewer");
        registrar.viewer(
            VIEWER_ID,
            "Image Viewer",
            PANEL_ID,
            SUPPORTED_EXTENSIONS,
            DocumentKind::Bytes,
        );
        registrar.command(OPEN_COMMAND, "Open Image…", Some("image"));
        registrar.command(OPEN_FILE_COMMAND, "Open in Image Viewer", Some("image"));
        registrar.menu(MenuSlot::Application, OPEN_COMMAND);
        registrar.menu(MenuSlot::File, OPEN_FILE_COMMAND);
        registrar.settings("bed.image.settings", "Image Viewer");
    }
    fn command_enabled(
        &self,
        command: &str,
        context: &CommandContext,
        _host: &HostContext<'_>,
    ) -> bool {
        command != OPEN_FILE_COMMAND
            || context
                .path
                .as_ref()
                .is_some_and(|path| supported_path(path))
    }
    fn command(
        &mut self,
        command: &str,
        context: &CommandContext,
        _host: &HostContext<'_>,
        requests: &mut Vec<HostRequest>,
    ) {
        match command {
            OPEN_COMMAND => requests.push(HostRequest::OpenFileDialog {
                viewer: Some(VIEWER_ID.into()),
            }),
            OPEN_FILE_COMMAND => {
                if let Some(path) = &context.path {
                    requests.push(HostRequest::OpenFile {
                        path: path.clone(),
                        viewer: Some(VIEWER_ID.into()),
                    });
                }
            }
            _ => {}
        }
    }
    fn create_panel(
        &mut self,
        panel_type: &str,
        document: Option<DocumentId>,
        state: &Value,
    ) -> Result<Box<dyn PluginPanel>, String> {
        if panel_type != PANEL_ID {
            return Err(format!("Unknown image panel: {panel_type}"));
        }
        Ok(Box::new(ImagePanel::new(
            document.ok_or("Image panels require a document")?,
            state,
        )))
    }
    fn draw_settings(&mut self, section: &str, ui: &Ui, settings: &mut Value) -> bool {
        if section != "bed.image.settings" {
            return false;
        }
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
    fn as_any(&self) -> &dyn Any {
        self
    }
}
#[derive(Clone)]
struct DecodedImage {
    size: [u32; 2],
    rgba: Arc<[u8]>,
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
}
impl PluginPanel for ImagePanel {
    fn title(&self, host: &HostContext<'_>) -> String {
        host.document(self.document)
            .and_then(|document| document.path.rsplit(['/', '\\']).next())
            .unwrap_or("Image Viewer")
            .to_owned()
    }
    fn attached_document(&self) -> Option<DocumentId> {
        Some(self.document)
    }
    fn save_state(&self) -> Value {
        json!({ "fit": self.fit, "zoom": self.zoom, "pan": self.pan })
    }
    fn close(&mut self, _requests: &mut Vec<HostRequest>) {
        self.decoder.sender = None;
        self.decoded = None;
        self.gpu = None;
    }
    fn render_output(&self) -> Option<RenderOutput> {
        self.decoded.as_ref()?;
        Some(RenderOutput {
            handle: self.texture,
            size: self.canvas?.pixels,
            depth: false,
            revision: self.output_revision,
        })
    }
    fn render(&mut self, gpu: &mut GpuContext<'_>, target: &RenderTarget) -> Result<(), String> {
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
    fn draw(&mut self, ui: &Ui, host: &HostContext<'_>, requests: &mut Vec<HostRequest>) {
        let _controls = bed_ui::util::popup_style::controls_style(ui);
        self.update(host);
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
        if ui.button("−") {
            *fit = false;
            self.zoom = (self.zoom / 1.25).max(0.01);
        }
        ui.same_line();
        if ui.button("+") {
            *fit = false;
            self.zoom = (self.zoom * 1.25).min(32.0);
        }
        ui.same_line();
        if ui.button("Info") {
            ui.open_popup("image-information");
        }
        {
            let _popup_style = bed_ui::util::popup_style::context_menu_style(ui);
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
                requests.push(HostRequest::OpenFile {
                    path: document.path.clone(),
                    viewer: Some("bed.hex".into()),
                });
            }
            return;
        }
        let Some(decoded) = &self.decoded else {
            ui.text_disabled("Loading image…");
            return;
        };
        let canvas = Canvas::show(ui, host, self.texture, "image-canvas");
        let fit_scale =
            (canvas.size[0] / decoded.size[0] as f32).min(canvas.size[1] / decoded.size[1] as f32);
        let mut scale = if *fit { fit_scale } else { self.zoom };
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
            background: ui.style_color(StyleColor::WindowBg),
        };
        if self.canvas != Some(state) {
            self.canvas = Some(state);
            self.output_revision += 1;
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn worker_discards_stale_results_and_close_releases_gpu_output() {
        let mut panel = ImagePanel::new(DocumentId(1), &Value::Null);
        let mut documents = [bed_plugin::PluginDocument {
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
            documents: &documents,
            active_document: Some(DocumentId(1)),
            settings: &settings,
            textures: &textures,
            animations: false,
            workspace: 1,
            diagnostics: &Value::Null,
        });
        documents[0].revision = (1, 2);
        documents[0].bytes = png(2, 3);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            panel.update(&HostContext {
                documents: &documents,
                active_document: Some(DocumentId(1)),
                settings: &settings,
                textures: &textures,
                animations: false,
                workspace: 1,
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
            background: [0.0; 4],
        });
        assert_eq!(panel.render_output().unwrap().size, [200; 2]);
        panel.close(&mut requests);
        assert!(panel.decoder.sender.is_none());
        assert!(panel.render_output().is_none());
        assert!(panel.gpu.is_none());
    }
    #[test]
    fn file_menu_command_uses_captured_path_after_focus_changes() {
        let mut plugin = ImagePlugin::default();
        let settings = Value::Null;
        let textures = HashMap::new();
        let host = HostContext {
            documents: &[],
            active_document: None,
            settings: &settings,
            textures: &textures,
            animations: false,
            workspace: 1,
            diagnostics: &Value::Null,
        };
        let context = CommandContext {
            path: Some("selected.png".into()),
            ..Default::default()
        };
        let mut requests = Vec::new();
        plugin.command(OPEN_FILE_COMMAND, &context, &host, &mut requests);
        assert!(
            matches!(requests.as_slice(), [HostRequest::OpenFile { path, viewer: Some(viewer) }] if path == "selected.png" && viewer == VIEWER_ID)
        );
    }
    #[test]
    fn image_registration_exercises_contributions_and_extension_matching() {
        let mut registry = bed_plugin::Registry::default();
        registry.register(&ImagePlugin::default()).unwrap();
        for extension in SUPPORTED_EXTENSIONS {
            let path = format!("/images/a.{}", extension.to_ascii_uppercase());
            assert_eq!(registry.viewer_for_path(&path).unwrap().id, VIEWER_ID);
            assert!(supported_path(&path));
        }
        assert!(registry.viewer_for_path("file.rs").is_none());
        assert!(!supported_path("/images.svg/file"));
        assert!(!supported_path("C:\\images.svg\\file"));
        assert_eq!(registry.settings.len(), 1);
        assert!(registry.toolbar.is_empty());
        assert!(
            registry
                .menus
                .iter()
                .any(|entry| entry.slot == MenuSlot::File)
        );
    }
}
