//! A small read-only PNG/JPEG viewer implemented entirely through the plugin API.
use bed_core::identity::DocumentId;
use bed_plugin::{
    CommandContext, DocumentKind, HostContext, HostRequest, MenuSlot, Plugin, PluginPanel,
    Registrar, Revision, TextureHandle,
};
use dear_imgui_rs::{MouseButton, Ui};
use serde_json::{Value, json};
use std::{
    any::Any,
    io::Cursor,
    sync::{Arc, mpsc},
    thread,
};

pub const PLUGIN_ID: &str = "bed.image";
pub const PANEL_ID: &str = "bed.image.panel";
pub const VIEWER_ID: &str = "bed.image.viewer";
pub const OPEN_COMMAND: &str = "bed.image.open";
pub const OPEN_FILE_COMMAND: &str = "bed.image.open-file";
pub const MAX_DECODED_BYTES: u64 = 64 * 1024 * 1024;

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
            &["png", "jpg", "jpeg"],
            DocumentKind::Bytes,
        );
        registrar.command(OPEN_COMMAND, "Open Image…", Some("image"));
        registrar.command(OPEN_FILE_COMMAND, "Open in Image Viewer", Some("image"));
        registrar.toolbar(OPEN_COMMAND);
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
fn supported_path(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, extension)| {
        ["png", "jpg", "jpeg"]
            .iter()
            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
    })
}

#[derive(Clone)]
struct DecodedImage {
    size: [u32; 2],
    rgba: Arc<[u8]>,
}
struct DecodeJob {
    revision: Revision,
    bytes: Arc<[u8]>,
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
                    result: decode(&job.bytes),
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
    upload: bool,
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
            upload: false,
            fit: state["fit"].as_bool(),
            zoom: finite(state["zoom"].as_f64(), 1.0).clamp(0.01, 32.0),
            pan: [
                finite(state["pan"][0].as_f64(), 0.0),
                finite(state["pan"][1].as_f64(), 0.0),
            ],
        }
    }
    fn update(&mut self, host: &HostContext<'_>, requests: &mut Vec<HostRequest>) {
        let Some(document) = host.document(self.document) else {
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
                    self.upload = true;
                }
                Err(error) => {
                    self.decoded = None;
                    self.error = Some(error);
                    requests.push(HostRequest::ReleaseTexture {
                        handle: self.texture,
                    });
                }
            }
        }
        if self.requested != Some(document.revision)
            && self.decoder.submit(DecodeJob {
                revision: document.revision,
                bytes: Arc::clone(&document.bytes),
            })
        {
            self.requested = Some(document.revision);
            self.decoded = None;
            self.error = None;
            requests.push(HostRequest::ReleaseTexture {
                handle: self.texture,
            });
        }
        if let Some(decoded) = &self.decoded
            && (self.upload || host.texture(self.texture).is_none())
        {
            requests.push(HostRequest::UploadTexture {
                handle: self.texture,
                size: decoded.size,
                rgba: Arc::clone(&decoded.rgba),
            });
            self.upload = false;
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
    fn close(&mut self, requests: &mut Vec<HostRequest>) {
        self.decoder.sender = None;
        requests.push(HostRequest::ReleaseTexture {
            handle: self.texture,
        });
    }
    fn draw(&mut self, ui: &Ui, host: &HostContext<'_>, requests: &mut Vec<HostRequest>) {
        let _controls = bed_ui::util::popup_style::controls_style(ui);
        self.update(host, requests);
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
        let Some(texture) = host.texture(self.texture) else {
            ui.text_disabled("Preparing image…");
            return;
        };
        let canvas = ui.content_region_avail();
        let canvas = [canvas[0].max(1.0), canvas[1].max(1.0)];
        let origin = ui.cursor_screen_pos();
        let fit_scale =
            (canvas[0] / decoded.size[0] as f32).min(canvas[1] / decoded.size[1] as f32);
        let mut scale = if *fit { fit_scale } else { self.zoom };
        ui.invisible_button("image-canvas", canvas);
        if ui.is_item_hovered() {
            let wheel = ui.io().mouse_wheel();
            if wheel != 0.0 {
                self.zoom = (scale * 1.2f32.powf(wheel)).clamp(0.01, 32.0);
                *fit = false;
                scale = self.zoom;
            }
        }
        if ui.is_item_active() && ui.is_mouse_dragging(MouseButton::Left) {
            let delta = ui.io().mouse_delta();
            self.pan[0] += delta[0];
            self.pan[1] += delta[1];
        }
        let size = [
            decoded.size[0] as f32 * scale,
            decoded.size[1] as f32 * scale,
        ];
        let minimum = [
            origin[0] + (canvas[0] - size[0]) * 0.5 + self.pan[0],
            origin[1] + (canvas[1] - size[1]) * 0.5 + self.pan[1],
        ];
        let _clip = ui.push_clip_rect(origin, [origin[0] + canvas[0], origin[1] + canvas[1]], true);
        ui.get_window_draw_list().add_image(
            texture,
            minimum,
            [minimum[0] + size[0], minimum[1] + size[1]],
            [0.0; 2],
            [1.0; 2],
            [1.0; 4],
        );
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn validate_dimensions(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_DECODED_BYTES / 4 {
        return Err("Image exceeds the 64 MiB decoded-pixel limit".into());
    }
    Ok(())
}
fn reader(bytes: &[u8]) -> Result<image::ImageReader<Cursor<&[u8]>>, String> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    if !matches!(
        reader.format(),
        Some(image::ImageFormat::Png | image::ImageFormat::Jpeg)
    ) {
        return Err("Only PNG and JPEG images are supported".into());
    }
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_DECODED_BYTES);
    limits.max_image_width = Some((MAX_DECODED_BYTES / 4) as u32);
    limits.max_image_height = Some((MAX_DECODED_BYTES / 4) as u32);
    reader.limits(limits);
    Ok(reader)
}
fn decode(bytes: &[u8]) -> Result<DecodedImage, String> {
    let (width, height) = reader(bytes)?
        .into_dimensions()
        .map_err(|error| error.to_string())?;
    validate_dimensions(width, height)?;
    let rgba = reader(bytes)?
        .decode()
        .map_err(|error| error.to_string())?
        .into_rgba8();
    Ok(DecodedImage {
        size: [width, height],
        rgba: rgba.into_raw().into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::HashMap,
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
    fn worker_discards_stale_results_and_close_releases_texture() {
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
        panel.update(
            &HostContext {
                documents: &documents,
                active_document: Some(DocumentId(1)),
                settings: &settings,
                textures: &textures,
                animations: false,
                workspace: 1,
                diagnostics: &Value::Null,
            },
            &mut requests,
        );
        documents[0].revision = (1, 2);
        documents[0].bytes = png(2, 3);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            panel.update(
                &HostContext {
                    documents: &documents,
                    active_document: Some(DocumentId(1)),
                    settings: &settings,
                    textures: &textures,
                    animations: false,
                    workspace: 1,
                    diagnostics: &Value::Null,
                },
                &mut requests,
            );
            if panel.decoded.is_some() {
                break;
            }
            assert!(Instant::now() < deadline, "image worker timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(panel.decoded.as_ref().unwrap().size, [2, 3]);
        assert!(requests.iter().all(
            |request| !matches!(request, HostRequest::UploadTexture { size, .. } if *size != [2, 3])
        ));
        requests.clear();
        panel.close(&mut requests);
        assert!(panel.decoder.sender.is_none());
        assert!(matches!(
            requests.as_slice(),
            [HostRequest::ReleaseTexture { .. }]
        ));
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
    fn png_and_jpeg_decode_with_exact_dimensions_and_rgba_size() {
        for format in [image::ImageFormat::Png, image::ImageFormat::Jpeg] {
            let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                3,
                2,
                image::Rgb([255, 64, 32]),
            ));
            let mut bytes = Cursor::new(Vec::new());
            image.write_to(&mut bytes, format).unwrap();
            let decoded = decode(bytes.get_ref()).unwrap();
            assert_eq!(decoded.size, [3, 2]);
            assert_eq!(decoded.rgba.len(), 24);
            assert!(decoded.rgba.chunks(4).all(|pixel| pixel[3] == 255));
        }
    }
    #[test]
    fn invalid_and_oversized_images_are_rejected() {
        assert!(decode(b"not an image").is_err());
        assert!(validate_dimensions(4096, 4096).is_ok());
        assert!(validate_dimensions(4097, 4096).is_err());
        assert!(validate_dimensions(u32::MAX, u32::MAX).is_err());
        assert!(validate_dimensions(0, 1).is_err());
    }
    #[test]
    fn image_registration_exercises_contributions_and_extension_matching() {
        let mut registry = bed_plugin::Registry::default();
        registry.register(&ImagePlugin::default()).unwrap();
        assert_eq!(
            registry.viewer_for_path("/images/a.JPEG").unwrap().id,
            VIEWER_ID
        );
        assert!(registry.viewer_for_path("file.rs").is_none());
        assert_eq!(registry.settings.len(), 1);
        assert_eq!(registry.toolbar.len(), 1);
        assert!(
            registry
                .menus
                .iter()
                .any(|entry| entry.slot == MenuSlot::File)
        );
    }
}
