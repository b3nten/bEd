//! Read-only font inspection with HarfRust shaping and FreeType rasterization.
use bed_editing::identity::DocumentId;
use bed_plugin::gpu::{Canvas, GpuContext, RenderOutput, RenderTarget};
use bed_plugin::{
    CommandContext, DocumentKind, HostContext, HostRequest, MenuSlot, Plugin, PluginPanel,
    Registrar, Revision, TextureHandle,
};
use dear_imgui_rs::{MouseButton, StyleColor, StyleVar, Ui, WindowFlags};
use serde_json::{Value, json};
use std::{
    any::Any,
    sync::{Arc, mpsc},
    thread,
};

mod font;
mod raster;
mod render;
#[cfg(test)]
mod tests;
mod webfont;
use font::{FontEngine, FontInfo};
use raster::{GridLayout, Mode, Raster, RasterRequest};

pub const PLUGIN_ID: &str = "bed.font";
pub const PANEL_ID: &str = "bed.font.panel";
pub const VIEWER_ID: &str = "bed.font.viewer";
pub const OPEN_COMMAND: &str = "bed.font.open";
pub const OPEN_FILE_COMMAND: &str = "bed.font.open-file";
const EXTENSIONS: &[&str] = &["ttf", "otf", "ttc", "otc", "woff", "woff2"];
const DEFAULT_SAMPLE: &str = "The quick brown fox jumps over the lazy dog.\nABCDEFGHIJKLMNOPQRSTUVWXYZ\nabcdefghijklmnopqrstuvwxyz 0123456789\nOffice affinity — fi fl ffi — AVATAR To Wa\n! @ # $ % & * ( ) [ ] { } < > / + =";
const MAX_SAMPLE_BYTES: usize = 16 * 1024;

#[derive(Default)]
pub struct FontPlugin;
impl Plugin for FontPlugin {
    fn id(&self) -> &'static str {
        PLUGIN_ID
    }
    fn register(&self, registrar: &mut Registrar<'_>) {
        registrar.panel(PANEL_ID, "Font Viewer");
        registrar.viewer(
            VIEWER_ID,
            "Font Viewer",
            PANEL_ID,
            EXTENSIONS,
            DocumentKind::Bytes,
        );
        registrar.command(OPEN_COMMAND, "Open Font…", Some("font"));
        registrar.command(OPEN_FILE_COMMAND, "Open in Font Viewer", Some("font"));
        registrar.menu(MenuSlot::Application, OPEN_COMMAND);
    }
    fn command_enabled(
        &self,
        command: &str,
        context: &CommandContext,
        _: &HostContext<'_>,
    ) -> bool {
        command != OPEN_FILE_COMMAND
            || context.path.as_deref().is_some_and(|path| {
                path.rsplit_once('.').is_some_and(|(_, extension)| {
                    EXTENSIONS.iter().any(|e| e.eq_ignore_ascii_case(extension))
                })
            })
    }
    fn command(
        &mut self,
        command: &str,
        context: &CommandContext,
        _: &HostContext<'_>,
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
            return Err(format!("Unknown font panel: {panel_type}"));
        }
        let Some(document) = document else {
            return Ok(Box::new(EmptyPanel));
        };
        Ok(Box::new(FontPanel::new(document, state)))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

#[derive(Clone, PartialEq)]
struct JobKey {
    revision: Revision,
    face: u32,
    request: RasterRequest,
}
struct Job {
    serial: u64,
    key: JobKey,
    bytes: Arc<[u8]>,
}
struct Preview {
    info: Arc<FontInfo>,
    raster: Raster,
    request: RasterRequest,
}
struct JobResult {
    serial: u64,
    revision: Revision,
    result: Result<Preview, String>,
}
struct Worker {
    sender: Option<mpsc::SyncSender<Job>>,
    receiver: mpsc::Receiver<JobResult>,
}
impl Worker {
    fn new() -> Self {
        let (sender, jobs) = mpsc::sync_channel::<Job>(1);
        let (results, receiver) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let mut loaded = None;
            let mut engine: Result<FontEngine, String> = Err("No font loaded".into());
            while let Ok(mut job) = jobs.recv() {
                while let Ok(latest) = jobs.try_recv() {
                    job = latest;
                }
                let key = (job.key.revision, job.key.face);
                if loaded != Some(key) {
                    engine = FontEngine::new(job.bytes, job.key.face);
                    loaded = Some(key);
                }
                let result = match &mut engine {
                    Ok(engine) => {
                        let mut request = job.key.request;
                        request.selected =
                            request.selected.min(engine.info.glyphs.len() as u32 - 1);
                        raster::render(engine, &request).map(|raster| Preview {
                            info: Arc::clone(&engine.info),
                            raster,
                            request,
                        })
                    }
                    Err(error) => Err(error.clone()),
                };
                if results
                    .send(JobResult {
                        serial: job.serial,
                        revision: job.key.revision,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            sender: Some(sender),
            receiver,
        }
    }
}

pub struct FontPanel {
    document: DocumentId,
    texture: TextureHandle,
    worker: Worker,
    requested: Option<JobKey>,
    serial: u64,
    revision: Option<Revision>,
    preview: Option<Preview>,
    error: Option<String>,
    gpu: Option<render::FontGpu>,
    uploaded: Option<u64>,
    output_revision: u64,
    canvas: Option<[u32; 2]>,
    face: u32,
    view: usize,
    size: f32,
    sample: String,
    ligatures: bool,
    kerning: bool,
    direction: usize,
    pan: [f32; 2],
    glyph_scroll: f32,
    restore_scroll: bool,
    reveal_glyph: Option<usize>,
    selected: u32,
    jump: String,
    jump_error: Option<String>,
}
impl FontPanel {
    fn new(document: DocumentId, state: &Value) -> Self {
        let size = state["size"]
            .as_f64()
            .filter(|v| v.is_finite())
            .unwrap_or(36.0)
            .clamp(8.0, 160.0) as f32;
        let mut sample = state["sample"]
            .as_str()
            .unwrap_or(DEFAULT_SAMPLE)
            .to_owned();
        truncate_sample(&mut sample);
        let pan = |axis| {
            state["pan"][axis]
                .as_f64()
                .filter(|v| v.is_finite())
                .unwrap_or(0.0)
                .clamp(0.0, 1_000_000.0) as f32
        };
        Self {
            document,
            texture: TextureHandle::next(),
            worker: Worker::new(),
            requested: None,
            serial: 0,
            revision: None,
            preview: None,
            error: None,
            gpu: None,
            uploaded: None,
            output_revision: 0,
            canvas: None,
            face: state["face"].as_u64().unwrap_or(0).min(u32::MAX as u64) as u32,
            view: state["view"].as_u64().unwrap_or(0).min(2) as usize,
            size,
            sample,
            ligatures: state["ligatures"].as_bool().unwrap_or(true),
            kerning: state["kerning"].as_bool().unwrap_or(true),
            direction: state["direction"].as_u64().unwrap_or(0).min(2) as usize,
            pan: [pan(0), pan(1)],
            glyph_scroll: state["glyph_scroll"]
                .as_f64()
                .filter(|v| v.is_finite())
                .unwrap_or(0.0)
                .clamp(0.0, 1_000_000.0) as f32,
            restore_scroll: true,
            // Preserve the approximate position of older paged workspaces.
            reveal_glyph: state["glyph_scroll"]
                .is_null()
                .then(|| state["first"].as_u64().unwrap_or(0).min(1_000_000) as usize),
            selected: state["selected"].as_u64().unwrap_or(0).min(1_000_000) as u32,
            jump: String::new(),
            jump_error: None,
        }
    }
    fn poll(&mut self, revision: Revision) {
        if self.revision != Some(revision) {
            self.revision = Some(revision);
            self.preview = None;
            self.error = None;
            self.gpu = None;
        }
        while let Ok(result) = self.worker.receiver.try_recv() {
            if result.serial != self.serial || result.revision != revision {
                continue;
            }
            let Some(requested) = &self.requested else {
                continue;
            };
            if self.face != requested.face {
                continue;
            }
            match result.result {
                Ok(preview) => {
                    self.face = preview.info.face_index;
                    // A full worker queue may have delayed submission of a
                    // newer click. Only normalize values the user has not
                    // changed since this result was requested.
                    if self.selected == requested.request.selected {
                        self.selected = preview.request.selected;
                    }
                    self.preview = Some(preview);
                    self.error = None;
                    self.output_revision += 1;
                    self.uploaded = None;
                }
                Err(error) => {
                    self.error = Some(error);
                    self.preview = None;
                    self.gpu = None;
                }
            }
        }
    }
    fn submit(&mut self, revision: Revision, bytes: &Arc<[u8]>, request: RasterRequest) {
        let key = JobKey {
            revision,
            face: self.face,
            request,
        };
        if self.requested.as_ref() == Some(&key) {
            return;
        }
        let serial = self.serial + 1;
        if self.worker.sender.as_ref().is_some_and(|sender| {
            sender
                .try_send(Job {
                    serial,
                    key: key.clone(),
                    bytes: Arc::clone(bytes),
                })
                .is_ok()
        }) {
            self.serial = serial;
            self.requested = Some(key);
            self.error = None;
        }
    }
    fn information(&self, ui: &Ui, info: &FontInfo, path: &str, bytes: usize) {
        ui.text_wrapped(path);
        ui.text(format!(
            "{bytes} bytes · Face {} of {}",
            info.face_index + 1,
            info.face_count
        ));
        ui.separator();
        ui.text(format!(
            "{} glyphs · {} Unicode mappings",
            info.glyphs.len(),
            info.mappings.len()
        ));
        ui.text(format!(
            "{} · {} units per em",
            if info.monospace {
                "Monospaced"
            } else {
                "Proportional"
            },
            info.units_per_em
        ));
        ui.text(format!(
            "Ascender {} · Descender {} · Line height {}",
            info.ascender, info.descender, info.line_height
        ));
        for (label, value) in &info.names {
            ui.spacing();
            ui.text_disabled(label);
            ui.text_wrapped(value);
        }
    }
    fn glyph_controls(&mut self, ui: &Ui, info: &FontInfo) {
        ui.set_next_item_width(180.0);
        let enter = ui
            .input_text("##glyph-jump", &mut self.jump)
            .hint("U+0041, A, or ID")
            .enter_returns_true(true)
            .build();
        ui.same_line();
        if ui.button("Go") || enter {
            match find_glyph(info, &self.jump) {
                Some(id) => {
                    self.selected = id;
                    self.reveal_glyph = Some(id as usize);
                    self.jump_error = None;
                }
                None => self.jump_error = Some("No matching glyph in this font".into()),
            }
        }
        ui.same_line();
        ui.text_disabled(format!("{} glyphs", info.glyphs.len()));
        if let Some(error) = &self.jump_error {
            ui.text_disabled(error);
        }
        if let Some(preview) = &self.preview {
            let detail = &preview.raster.selected;
            // Keep the displayed inspector's metrics visible while a newer
            // selection loads, so the canvas does not resize on every click.
            ui.text(format!(
                "Glyph {} {} · Advance {} · Bearing {}, {} · Size {} × {} font units",
                detail.id,
                detail.name,
                detail.advance,
                detail.bearing[0],
                detail.bearing[1],
                detail.size[0],
                detail.size[1]
            ));
            if let Some(character) = detail.codepoint.and_then(char::from_u32) {
                if ui.small_button("Copy character")
                    && let Ok(text) = std::ffi::CString::new(character.to_string())
                {
                    // SAFETY: the current Ui owns the bound context;
                    // ImGui copies this string into the host clipboard.
                    ui.with_bound_context(|| unsafe {
                        dear_imgui_sys::igSetClipboardText(text.as_ptr())
                    });
                }
                ui.same_line();
                ui.text_disabled(format!("U+{:04X}", character as u32));
            } else {
                ui.text_disabled("Unencoded glyph");
            }
        }
    }
    fn glyph_overlay(&mut self, ui: &Ui, origin: [f32; 2], logical: [f32; 2], hovered: bool) {
        let Some(preview) = &self.preview else {
            return;
        };
        if preview.request.mode != Mode::Glyphs {
            return;
        }
        // Labels and hit-testing follow the displayed raster while a newer
        // scroll position or size is being produced by the worker.
        let request = &preview.request;
        let grid = GridLayout::new(request.logical, request.label_height, request.size);
        let scale = [
            logical[0] / request.logical[0],
            logical[1] / request.logical[1],
        ];
        let end = [origin[0] + logical[0], origin[1] + logical[1]];
        let _clip = ui.push_clip_rect(origin, end, true);
        let draw = ui.get_window_draw_list();
        let border = ui.style_color(StyleColor::Border);
        let text = bed_ui::presentation::muted_text_color(ui);
        let scroll = request
            .glyph_scroll
            .clamp(0.0, grid.scroll_max(preview.info.glyphs.len()));
        {
            let _grid_clip = ui.push_clip_rect(
                origin,
                [
                    origin[0] + grid.viewport[0] * scale[0],
                    origin[1] + grid.viewport[1] * scale[1],
                ],
                true,
            );
            for id in grid.visible(scroll, preview.info.glyphs.len()) {
                let codepoint = preview.info.glyphs[id];
                let cell = grid.cell_origin(id, scroll);
                let min = [
                    origin[0] + cell[0] * scale[0],
                    origin[1] + cell[1] * scale[1],
                ];
                let max = [
                    min[0] + grid.cell[0] * scale[0],
                    min[1] + grid.cell[1] * scale[1],
                ];
                let color = if id as u32 == self.selected {
                    ui.style_color(StyleColor::CheckMark)
                } else {
                    border
                };
                draw.add_rect(min, max, color).build();
                let label =
                    codepoint.map_or_else(|| format!("gid {id}"), |cp| format!("U+{cp:04X}"));
                draw.add_text(
                    [min[0] + 6.0, max[1] - ui.current_font_size() - 6.0],
                    text,
                    label,
                );
            }
        }
        let detail = grid.detail;
        let label = preview.raster.selected.codepoint.map_or_else(
            || format!("Glyph {}", preview.raster.selected.id),
            |cp| format!("U+{cp:04X} · Glyph {}", preview.raster.selected.id),
        );
        draw.add_text(
            [
                origin[0] + detail[0] * scale[0] + 8.0,
                origin[1] + detail[1] * scale[1] + 8.0,
            ],
            text,
            label,
        );
        if hovered {
            let mouse = ui.io().mouse_pos();
            let local = [
                (mouse[0] - origin[0]) / scale[0],
                (mouse[1] - origin[1]) / scale[1],
            ];
            if let Some(id) = grid.glyph_at(local, scroll, preview.info.glyphs.len())
                && ui.is_mouse_clicked(MouseButton::Left)
            {
                self.selected = id as u32;
            }
        }
    }
    fn glyph_canvas(
        &mut self,
        ui: &Ui,
        host: &HostContext<'_>,
        info: Option<&FontInfo>,
    ) -> Option<Canvas> {
        let available = ui.content_region_avail().map(|v| v.max(1.0));
        let logical = [
            (available[0] - ui.clone_style().scrollbar_size()).max(1.0),
            available[1],
        ];
        let grid = GridLayout::new(logical, ui.current_font_size(), self.size);
        let glyphs = info.map_or(0, |info| info.glyphs.len());
        let _padding = ui.push_style_var(StyleVar::WindowPadding([0.0; 2]));
        let _border = ui.push_style_var(StyleVar::ChildBorderSize(0.0));
        // Native scrolling owns the full grid extent; only the visible pixels
        // are rasterized. The nested viewport stays fixed, as does its inspector.
        ui.with_bound_context(|| unsafe {
            dear_imgui_sys::igSetNextWindowContentSize(
                [0.0, logical[1] + grid.scroll_max(glyphs)].into(),
            );
            if info.is_some() && (self.restore_scroll || self.reveal_glyph.is_some()) {
                let scroll = self
                    .reveal_glyph
                    .take()
                    .map_or(self.glyph_scroll, |id| grid.scroll_to(id, glyphs));
                dear_imgui_sys::igSetNextWindowScroll([-1.0, scroll].into());
                self.restore_scroll = false;
            }
        });
        ui.child_window("glyph-scroll")
            .size(available)
            .flags(WindowFlags::ALWAYS_VERTICAL_SCROLLBAR)
            .build(ui, || {
                if info.is_some() {
                    self.glyph_scroll = ui.scroll_y().clamp(0.0, grid.scroll_max(glyphs));
                }
                let origin = ui.cursor_screen_pos();
                ui.set_cursor_screen_pos([origin[0], origin[1] + ui.scroll_y()]);
                ui.child_window("glyph-viewport")
                    .size(logical)
                    .flags(WindowFlags::NO_SCROLLBAR | WindowFlags::NO_SCROLL_WITH_MOUSE)
                    .build(ui, || {
                        let origin = ui.cursor_screen_pos();
                        let canvas = Canvas::show(ui, host, self.texture, "font-canvas");
                        self.glyph_overlay(ui, origin, canvas.size, canvas.hovered);
                        canvas
                    })
            })
            .flatten()
    }
}

impl PluginPanel for FontPanel {
    fn title(&self, host: &HostContext<'_>) -> String {
        host.document(self.document)
            .and_then(|d| d.path.rsplit(['/', '\\']).next())
            .unwrap_or("Font Viewer")
            .to_owned()
    }
    fn attached_document(&self) -> Option<DocumentId> {
        Some(self.document)
    }
    fn save_state(&self) -> Value {
        json!({ "face": self.face, "view": self.view, "size": self.size, "sample": self.sample,
            "ligatures": self.ligatures, "kerning": self.kerning, "direction": self.direction,
            "pan": self.pan, "glyph_scroll": self.glyph_scroll, "selected": self.selected })
    }
    fn close(&mut self, _: &mut Vec<HostRequest>) {
        self.worker.sender = None;
        self.preview = None;
        self.gpu = None;
        self.canvas = None;
    }
    fn render_output(&self) -> Option<RenderOutput> {
        if self.view == 2 {
            return None;
        }
        self.preview.as_ref()?;
        Some(RenderOutput {
            handle: self.texture,
            size: self.canvas?,
            depth: false,
            revision: self.output_revision,
        })
    }
    fn render(&mut self, gpu: &mut GpuContext<'_>, target: &RenderTarget) -> Result<(), String> {
        let preview = self.preview.as_ref().ok_or("Font preview is not ready")?;
        if self
            .gpu
            .as_ref()
            .is_none_or(|state| state.generation != gpu.generation)
        {
            self.gpu = Some(render::FontGpu::new(gpu));
            self.uploaded = None;
        }
        let renderer = self.gpu.as_mut().unwrap();
        if self.uploaded != Some(self.output_revision) {
            renderer.upload(gpu, &preview.raster)?;
            self.uploaded = Some(self.output_revision);
        }
        renderer.render(gpu, target);
        Ok(())
    }
    fn draw(&mut self, ui: &Ui, host: &HostContext<'_>, requests: &mut Vec<HostRequest>) {
        let _controls = bed_ui::util::popup_style::controls_style(ui);
        let Some(document) = host.document(self.document) else {
            self.preview = None;
            self.canvas = None;
            self.gpu = None;
            ui.text_disabled("The font document is closed");
            return;
        };
        self.poll(document.revision);
        let info = self
            .preview
            .as_ref()
            .map(|preview| Arc::clone(&preview.info));
        if let Some(info) = &info {
            ui.text(format!("{} · {}", info.family, info.style));
            if info.face_count > 1 {
                ui.set_next_item_width(200.0);
                let mut face = self.face as usize;
                let faces: Vec<_> = (0..info.face_count.min(4096))
                    .map(|i| format!("Face {}", i + 1))
                    .collect();
                if ui.combo_simple_string("Collection", &mut face, &faces) {
                    self.face = face as u32;
                    self.glyph_scroll = 0.0;
                    self.restore_scroll = true;
                    self.reveal_glyph = None;
                    self.selected = 0;
                    self.pan = [0.0; 2];
                }
            }
        }
        ui.set_next_item_width(140.0);
        ui.combo_simple_string(
            "View",
            &mut self.view,
            &["Preview", "Glyphs", "Information"],
        );
        if self.view != 2 {
            ui.same_line();
            ui.set_next_item_width(180.0);
            ui.slider_config("Size", 8.0, 160.0)
                .try_display_format("%.0f px")
                .expect("valid format")
                .build(&mut self.size);
        }
        ui.separator();
        if let Some(error) = &self.error {
            ui.text_wrapped(format!("Could not display font: {error}"));
            if ui.button("Open in Hex Editor") {
                requests.push(HostRequest::OpenFile {
                    path: document.path.clone(),
                    viewer: Some("bed.hex".into()),
                });
            }
            return;
        }
        if self.view == 2
            && let Some(info) = &info
        {
            self.canvas = None;
            self.information(ui, info, &document.path, document.bytes.len());
            return;
        }
        if self.view == 0 {
            ui.text_disabled("Sample text");
            let height = (ui.content_region_avail()[1] * 0.22).clamp(50.0, 110.0);
            ui.input_text_multiline(
                "##font-sample",
                &mut self.sample,
                [ui.content_region_avail()[0].max(1.0), height],
            )
            .build();
            truncate_sample(&mut self.sample);
            ui.checkbox("Ligatures", &mut self.ligatures);
            ui.same_line();
            ui.checkbox("Kerning", &mut self.kerning);
            ui.same_line();
            ui.set_next_item_width(140.0);
            ui.combo_simple_string(
                "Direction",
                &mut self.direction,
                &["Automatic", "Left to right", "Right to left"],
            );
            if let Some(preview) = &self.preview {
                ui.text_disabled(format!(
                    "{} shaped glyphs · {} missing · Drag or scroll to pan",
                    preview.raster.shaped_glyphs, preview.raster.missing_glyphs
                ));
            }
        } else if self.view == 1
            && let Some(info) = &info
        {
            self.glyph_controls(ui, info);
        }
        ui.separator();
        if info.is_none() {
            ui.text_disabled("Loading font…");
        }
        let canvas = if self.view == 1 {
            let Some(canvas) = self.glyph_canvas(ui, host, info.as_deref()) else {
                return;
            };
            canvas
        } else {
            Canvas::show(ui, host, self.texture, "font-canvas")
        };
        if self.canvas != Some(canvas.pixels) {
            self.canvas = Some(canvas.pixels);
            self.output_revision += 1;
        }
        if self.view != 1 {
            if canvas.hovered {
                self.pan[1] -= ui.io().mouse_wheel() * self.size * 1.5;
            }
            if canvas.active && ui.is_mouse_dragging(MouseButton::Left) {
                let delta = ui.io().mouse_delta();
                self.pan[0] -= delta[0];
                self.pan[1] -= delta[1];
            }
            if let Some(preview) = &self.preview {
                for axis in 0..2 {
                    self.pan[axis] = self.pan[axis].clamp(
                        0.0,
                        (preview.raster.content[axis] - canvas.size[axis]).max(0.0),
                    );
                }
            }
        }
        self.submit(
            document.revision,
            &document.bytes,
            RasterRequest {
                mode: if self.view == 1 {
                    Mode::Glyphs
                } else {
                    Mode::Preview
                },
                pixels: canvas.pixels,
                logical: canvas.size,
                label_height: ui.current_font_size(),
                size: self.size,
                sample: self.sample.clone(),
                ligatures: self.ligatures,
                kerning: self.kerning,
                direction: self.direction,
                pan: self.pan,
                glyph_scroll: self.glyph_scroll,
                selected: self.selected,
                ink: bed_ui::presentation::readable_color(ui, ui.style_color(StyleColor::Text)),
            },
        );
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

fn truncate_sample(sample: &mut String) {
    sample.retain(|ch| ch != '\0');
    if let Some((end, _)) = sample.match_indices('\n').nth(255) {
        sample.truncate(end);
    }
    if sample.len() > MAX_SAMPLE_BYTES {
        let mut end = MAX_SAMPLE_BYTES;
        while !sample.is_char_boundary(end) {
            end -= 1;
        }
        sample.truncate(end);
    }
}
fn find_glyph(info: &FontInfo, query: &str) -> Option<u32> {
    let query = query.trim();
    let codepoint = if let Some(hex) = query
        .strip_prefix("U+")
        .or_else(|| query.strip_prefix("u+"))
    {
        u32::from_str_radix(hex, 16).ok()
    } else if query.chars().count() == 1 && !query.as_bytes()[0].is_ascii_digit() {
        query.chars().next().map(u32::from)
    } else {
        return query
            .parse::<u32>()
            .ok()
            .filter(|id| (*id as usize) < info.glyphs.len());
    }?;
    let index = info
        .mappings
        .binary_search_by_key(&codepoint, |&(cp, _)| cp)
        .ok()?;
    Some(info.mappings[index].1)
}

/// An unloaded viewer owns no document, worker, or resource session.
struct EmptyPanel;

impl PluginPanel for EmptyPanel {
    fn title(&self, _: &HostContext<'_>) -> String {
        "Font Viewer".into()
    }

    fn draw(
        &mut self,
        ui: &dear_imgui_rs::Ui,
        _: &HostContext<'_>,
        requests: &mut Vec<HostRequest>,
    ) {
        ui.text_wrapped("Open a font to preview it.");
        if ui.button("Open Font…") {
            requests.push(HostRequest::OpenFileDialog {
                viewer: Some(VIEWER_ID.into()),
            });
        }
    }

    fn is_input_empty(&self, _: &HostContext<'_>) -> bool {
        true
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod default_panel_tests {
    use super::*;

    #[test]
    fn no_input_creates_an_unloaded_viewer() {
        let mut plugin = FontPlugin::default();
        let panel = plugin.create_panel(PANEL_ID, None, &Value::Null).unwrap();
        let textures = Default::default();
        let host = HostContext {
            remote: false,
            documents: &[],
            active_document: None,
            settings: &Value::Null,
            textures: &textures,
            animations: false,
            workspace: 1,
            diagnostics: &Value::Null,
            viewer_menu: None,
            default_viewers: &Value::Null,
        };
        assert_eq!(panel.title(&host), "Font Viewer");
        assert!(panel.is_input_empty(&host));
        assert!(panel.attached_document().is_none());
        assert_eq!(panel.save_state(), Value::Null);
        assert!(panel.render_output().is_none());
    }
}
