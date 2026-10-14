//! glTF and STL model viewer using the application's shared Bevy renderer.
//!
//! Install [`ModelViewerPlugin`] after the desktop/core plugin group. The host
//! supplies a neutral in-memory asset source; model loading registers afterward.
//! Headless apps can register `bedtime_desktop::MemoryAssets` before `AssetPlugin`,
//! or install [`ModelAssetsPlugin`] first as a convenience.
use bedtime_api::gpu::GpuContext;
use bedtime_api::{
    documents::{DocumentKind, Revision},
    features::{FeatureRegistrar, HostAction, HostContext},
};
use bedtime_editing::identity::DocumentId;
use bedtime_ui::TextureHandle;
use bedtime_ui::components::tooltip_text;
use bedtime_ui::gpu::{Canvas, RenderOutput, RenderTarget};
use dear_imgui_rs::{MouseButton, StyleColor, Ui};
use serde_json::Value;
use std::{
    sync::{Arc, mpsc},
    thread,
};

mod ecs;
pub use ecs::{GltfDocument, GltfStates, GltfSystems, ModelAssetsPlugin, ModelViewerPlugin};

mod bevy_scene;
mod camera;
mod debug_views;
mod draco;
mod environment;
#[cfg(test)]
mod fixtures;
mod mesh_compat;
mod meshopt;
mod model;
#[cfg(test)]
mod native_tests;
mod prepare;
mod render;
mod skin;
mod skybox_blur;
mod stl;
#[cfg(test)]
mod tests;
use camera::Camera;
use model::{PreparedModel, Scene};

pub const PLUGIN_ID: &str = "bedtime.gltf";
pub const PANEL_ID: &str = "bedtime.gltf.panel";
pub const VIEWER_ID: &str = "bedtime.gltf.viewer";
pub const OPEN_COMMAND: &str = "bedtime.gltf.open";
pub const OPEN_FILE_COMMAND: &str = "bedtime.gltf.open-file";

fn register_metadata(registrar: &mut FeatureRegistrar<'_>) {
    registrar.panel_ref(PANEL_ID);
    registrar.panel_requires_input(PANEL_ID);
    registrar.viewer(
        VIEWER_ID,
        "Model Viewer",
        PANEL_ID,
        &["gltf", "glb", "stl"],
        DocumentKind::Bytes,
    );
    registrar.viewer_document_kinds(VIEWER_ID, &[DocumentKind::Text, DocumentKind::Bytes]);
    registrar.command_ref(OPEN_COMMAND);
    registrar.command_ref(OPEN_FILE_COMMAND);
}

fn supported_path(path: &str) -> bool {
    path.rsplit(['/', '\\'])
        .next()
        .and_then(|n| n.rsplit_once('.'))
        .is_some_and(|(_, e)| {
            ["glb", "gltf", "stl"]
                .iter()
                .any(|x| x.eq_ignore_ascii_case(e))
        })
}

struct LoadJob {
    revision: Revision,
    bytes: Arc<[u8]>,
    stl: bool,
}

struct LoadResult {
    revision: Revision,
    prepared: Result<PreparedModel, String>,
}

struct Loader {
    sender: Option<mpsc::SyncSender<LoadJob>>,
    receiver: mpsc::Receiver<LoadResult>,
}

impl Loader {
    fn new() -> Self {
        let (sender, jobs) = mpsc::sync_channel::<LoadJob>(1);
        let (results, receiver) = mpsc::sync_channel(1);
        thread::spawn(move || {
            while let Ok(mut job) = jobs.recv() {
                while let Ok(latest) = jobs.try_recv() {
                    job = latest;
                }
                let result = LoadResult {
                    revision: job.revision,
                    prepared: if job.stl {
                        stl::prepare(&job.bytes)
                            .map(|prepared| PreparedModel::Stl(Box::new(prepared)))
                    } else {
                        prepare::load(&job.bytes).map(PreparedModel::Gltf)
                    },
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
}

#[derive(Clone, Copy, PartialEq)]
struct CanvasState {
    pixels: [u32; 2],
    camera: Camera,
    background: [f32; 4],
    settings: render::Settings,
}

pub struct GltfPanel {
    shortcuts: bedtime_keybindings::ShortcutInput,
    document: DocumentId,
    texture: TextureHandle,
    loader: Loader,
    requested: Option<Revision>,
    prepared: Option<Arc<PreparedModel>>,
    scene: Option<Scene>,
    error: Option<String>,
    camera: Camera,
    fitted: bool,
    canvas: Option<CanvasState>,
    output_revision: u64,
    gpu: Option<render::SceneGpu>,
    settings: render::Settings,
    settle_frames: u8,
    failed_generation: Option<u64>,
    presentation_generation: Option<u64>,
}

/// The rendering transition owns only GPU work, while documents and workers stay in World.
struct ModelRenderWork {
    prepared: Option<Arc<PreparedModel>>,
    texture: TextureHandle,
    canvas: Option<CanvasState>,
    gpu: Option<render::SceneGpu>,
    scene: Option<Scene>,
    error: Option<String>,
    output_revision: u64,
    settle_frames: u8,
    failed_generation: Option<u64>,
}

impl ModelRenderWork {
    fn record_render(
        &mut self,
        gpu: &mut GpuContext<'_>,
        target: &RenderTarget,
    ) -> Result<(), String> {
        if self.failed_generation == Some(gpu.generation) {
            // Acknowledge this revision once so the host can cache the failure.
            // Retain the scene to retry when the host replaces the GPU device.
            return Ok(());
        }
        if self.failed_generation.take().is_some() {
            self.error = None;
        }
        let prepared = self
            .prepared
            .as_ref()
            .ok_or("Model document is not prepared")?;
        if self.gpu.as_ref().is_none() {
            match render::SceneGpu::new(gpu, prepared, self.texture.0) {
                Ok(state) => {
                    self.gpu = Some(state);
                    self.settle_frames = render::SETTLE_FRAMES;
                }
                Err(error) => {
                    self.error = Some(error.clone());
                    self.failed_generation = Some(gpu.generation);
                    self.gpu = None;
                    return Err(error);
                }
            }
        }
        let canvas = self.canvas.ok_or("glTF canvas is not sized")?;
        let result = self.gpu.as_mut().unwrap().sync(
            gpu,
            target,
            canvas.camera,
            self.scene.as_ref().map_or(1.0, Scene::radius),
            canvas.background,
            canvas.settings,
        );
        if let Some(scene) = self.gpu.as_mut().and_then(|state| state.take_snapshot()) {
            self.scene = Some(scene);
            self.output_revision += 1;
            self.settle_frames = render::SETTLE_FRAMES;
        }
        if let Err(error) = &result {
            self.error = Some(error.clone());
            self.failed_generation = Some(gpu.generation);
            self.gpu = None;
        }
        result
    }
}

impl GltfPanel {
    fn new(document: DocumentId, state: &Value) -> Self {
        let (camera, fitted) = Camera::restore(state);
        Self {
            shortcuts: bedtime_keybindings::ShortcutInput::new(bedtime_keybindings::Bindings::new(
                bedtime_keybindings::common_actions(),
                &Default::default(),
                bedtime_keybindings::Platform::current(),
            ))
            .with_scope(bedtime_keybindings::Scope::panel(PANEL_ID)),
            document,
            texture: TextureHandle::next(),
            loader: Loader::new(),
            requested: None,
            prepared: None,
            scene: None,
            error: None,
            camera,
            fitted,
            canvas: None,
            output_revision: 0,
            gpu: None,
            settings: render::Settings::restore(state),
            settle_frames: render::SETTLE_FRAMES,
            failed_generation: None,
            presentation_generation: None,
        }
    }

    fn update(&mut self, host: &HostContext<'_>) {
        let Some(document) = host.document(self.document) else {
            self.prepared = None;
            self.scene = None;
            self.gpu = None;
            return;
        };
        if self.requested != Some(document.revision) {
            // A full worker queue must not leave an older scene renderable.
            self.prepared = None;
            self.scene = None;
            self.gpu = None;
            self.error = None;
            self.failed_generation = None;
            self.presentation_generation = None;
        }
        while let Ok(result) = self.loader.receiver.try_recv() {
            if result.revision != document.revision {
                continue;
            }
            match result.prepared {
                Ok(prepared) => {
                    self.prepared = Some(Arc::new(prepared));
                    self.scene = None;
                    self.error = None;
                    self.gpu = None;
                    self.failed_generation = None;
                    self.presentation_generation = None;
                    self.output_revision += 1;
                    self.settle_frames = render::SETTLE_FRAMES;
                }
                Err(error) => {
                    self.prepared = None;
                    self.scene = None;
                    self.error = Some(error);
                    self.gpu = None;
                }
            }
        }
        if self.requested != Some(document.revision)
            && self.loader.sender.as_ref().is_some_and(|sender| {
                sender
                    .try_send(LoadJob {
                        revision: document.revision,
                        bytes: Arc::clone(&document.bytes),
                        stl: document
                            .path
                            .rsplit_once('.')
                            .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("stl")),
                    })
                    .is_ok()
            })
        {
            self.requested = Some(document.revision);
        }
    }
}

impl GltfPanel {
    pub fn title(&self, host: &HostContext<'_>) -> String {
        self.panel_title(host)
    }

    pub fn attached_document(&self) -> Option<DocumentId> {
        Some(self.document)
    }

    pub fn save_state(&self) -> Value {
        self.saved_state()
    }

    pub fn close(&mut self, _: &mut Vec<HostAction>) {
        self.loader.sender = None;
        // Disconnect results too: an in-flight worker must not stay blocked
        // sending into a full result queue after this panel stops polling.
        self.loader.receiver = mpsc::channel().1;
        self.prepared = None;
        self.scene = None;
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
        self.update(host);
        self.draw_prepared(ui, host, requests);
    }
}

impl GltfPanel {
    fn panel_title(&self, host: &HostContext<'_>) -> String {
        host.document(self.document)
            .and_then(|d| d.path.rsplit(['/', '\\']).next())
            .unwrap_or("Model Viewer")
            .to_owned()
    }

    fn saved_state(&self) -> Value {
        let mut state = self.camera.save();
        state["render"] = self.settings.save();
        state
    }

    fn canvas_output(&self) -> Option<RenderOutput> {
        self.prepared.as_ref()?;
        Some(RenderOutput {
            handle: self.texture,
            size: self.canvas?.pixels,
            depth: true,
            revision: self.output_revision,
        })
    }

    fn take_render_work(&mut self) -> ModelRenderWork {
        ModelRenderWork {
            prepared: self.prepared.clone(),
            texture: self.texture,
            canvas: self.canvas,
            gpu: self.gpu.take(),
            scene: self.scene.take(),
            error: self.error.take(),
            output_revision: self.output_revision,
            settle_frames: self.settle_frames,
            failed_generation: self.failed_generation,
        }
    }

    fn finish_render_work(&mut self, work: ModelRenderWork) {
        self.gpu = work.gpu;
        self.scene = work.scene;
        self.error = work.error;
        self.output_revision = work.output_revision;
        self.settle_frames = work.settle_frames;
        self.failed_generation = work.failed_generation;
    }

    fn record_render(
        &mut self,
        gpu: &mut GpuContext<'_>,
        target: &RenderTarget,
    ) -> Result<(), String> {
        let mut work = self.take_render_work();
        let result = work.record_render(gpu, target);
        self.finish_render_work(work);
        result
    }

    fn draw_prepared(&mut self, ui: &Ui, host: &HostContext<'_>, requests: &mut Vec<HostAction>) {
        let _controls =
            bedtime_ui::components::controls_style(ui, &bedtime_ui::UiFrameContext::for_ui(ui));
        let frame_all = ui.button("Frame All");
        ui.same_line();
        if ui.button("Info") {
            ui.open_popup("gltf-information");
        }
        ui.same_line();
        if ui.button("Appearance") {
            ui.open_popup("model-appearance");
        }
        {
            let _popup = bedtime_ui::components::context_menu_style(
                ui,
                &bedtime_ui::UiFrameContext::for_ui(ui),
            );
            if let Some(_popup) = ui.begin_popup("model-appearance") {
                let mut display = self.settings.display.index();
                if ui.combo_simple_string("View", &mut display, &debug_views::DisplayMode::NAMES) {
                    self.settings.display = debug_views::DisplayMode::from_index(display);
                }
                ui.checkbox("Normal vectors", &mut self.settings.normals);
                if ui.is_item_hovered() {
                    tooltip_text(
                        ui,
                        &bedtime_ui::UiFrameContext::for_ui(ui),
                        "Show vertex normals, sampled to at most 6,000 vectors.",
                    );
                }
                if self.settings.normals {
                    ui.slider_config("Normal length", 0.01, 0.3)
                        .build(&mut self.settings.normal_length);
                }
                ui.separator();
                let mut lighting = self.settings.lighting.index();
                if ui.combo_simple_string("Lighting", &mut lighting, &render::Lighting::NAMES) {
                    self.settings.lighting = render::Lighting::from_index(lighting);
                }
                ui.checkbox("Skybox", &mut self.settings.skybox);
                if self.settings.skybox {
                    ui.slider_config("Skybox blur", 0.0, 1.0)
                        .try_display_format("%.2f")
                        .expect("valid blur format")
                        .build(&mut self.settings.skybox_blur);
                    if ui.is_item_hovered() {
                        tooltip_text(
                            ui,
                            &bedtime_ui::UiFrameContext::for_ui(ui),
                            "Soften the background while keeping model lighting and reflections sharp.",
                        );
                    }
                    ui.slider_config("Horizon", -45.0, 45.0)
                        .try_display_format("%.0f°")
                        .expect("valid horizon format")
                        .build(&mut self.settings.horizon);
                    if ui.is_item_hovered() {
                        tooltip_text(
                            ui,
                            &bedtime_ui::UiFrameContext::for_ui(ui),
                            "Lower values move the background horizon down.",
                        );
                    }
                }
                ui.checkbox("Shadows", &mut self.settings.shadows);
                let ao_supported = self.gpu.as_ref().is_none_or(|gpu| gpu.ao_supported);
                {
                    let _disabled = ui.begin_disabled_with_cond(!ao_supported);
                    ui.checkbox("Ambient occlusion", &mut self.settings.ao);
                }
                if !ao_supported {
                    ui.text_disabled("Ambient occlusion is unavailable on this GPU");
                }
                ui.slider_config("Exposure", -4.0, 4.0)
                    .build(&mut self.settings.exposure);
                if ui.button("Reset Appearance") {
                    self.settings = render::Settings::default();
                }
            }
        }
        ui.same_line();
        ui.text_disabled("Drag to orbit · Right-drag to pan · Scroll to zoom");
        {
            let _popup = bedtime_ui::components::context_menu_style(
                ui,
                &bedtime_ui::UiFrameContext::for_ui(ui),
            );
            if let Some(_popup) = ui.begin_popup("gltf-information") {
                if let Some(document) = host.document(self.document) {
                    ui.text_wrapped(&document.path);
                }
                if let Some(scene) = &self.scene {
                    ui.text(format!(
                        "{} nodes · {} meshes · {} triangles",
                        scene.nodes,
                        scene.primitives.len(),
                        scene.triangles()
                    ));
                    ui.text(format!(
                        "{} textures · {} animations",
                        scene.textures, scene.animations
                    ));
                    if scene.draco_primitives != 0 {
                        ui.text(format!(
                            "{} Draco primitives decoded",
                            scene.draco_primitives
                        ));
                    }
                    if scene.meshopt_views != 0 {
                        ui.text(format!(
                            "{} meshopt buffer views decoded",
                            scene.meshopt_views
                        ));
                    }
                    if scene.posed_meshes != 0 {
                        ui.text(format!(
                            "{} meshes in the authored skeleton pose",
                            scene.posed_meshes
                        ));
                    }
                }
                if let Some(PreparedModel::Gltf(prepared)) = self.prepared.as_deref() {
                    for warning in &prepared.warnings {
                        ui.text_wrapped(warning);
                    }
                }
            }
        }
        ui.separator();
        if host.document(self.document).is_none() {
            ui.text_disabled("The model document is closed");
            return;
        }
        if let Some(error) = &self.error {
            ui.text_wrapped(format!("Could not display model: {error}"));
            if ui.button("Open in Hex Editor")
                && let Some(document) = host.document(self.document)
            {
                requests.push(HostAction::OpenFile {
                    path: document.path.clone(),
                    viewer: Some("bedtime.editor.hex".into()),
                });
            }
            return;
        }
        if self.prepared.is_none() {
            ui.text_disabled("Loading model…");
            return;
        }
        if self.fitted
            && self.scene.is_some()
            && let Some(gpu) = self.gpu.as_ref().filter(|gpu| gpu.ready())
        {
            self.presentation_generation = Some(gpu.generation);
        }
        // Keep a valid frame visible while other pipelines compile. A
        // replacement device must produce a ready frame again.
        let display_ready = self
            .gpu
            .as_ref()
            .is_some_and(|gpu| self.presentation_generation == Some(gpu.generation));
        if !display_ready {
            ui.text_disabled("Loading model…");
        }
        // Keep sizing/rendering the canvas during initialization, but expose
        // its texture only after a fitted scene and completed shader frame.
        let canvas = Canvas::show(
            ui,
            display_ready.then(|| host.texture(self.texture)).flatten(),
            "gltf-canvas",
        );
        if let Some(scene) = &self.scene
            && (!self.fitted || frame_all)
        {
            self.camera.fit(scene, canvas.size[0] / canvas.size[1]);
            self.fitted = true;
        }
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
            for (action, wheel) in [
                (bedtime_keybindings::ZOOM_IN, 1.0),
                (bedtime_keybindings::ZOOM_OUT, -1.0),
            ] {
                if self.shortcuts.take(ui, action) {
                    self.camera
                        .zoom(wheel, self.scene.as_ref().map_or(1.0, Scene::radius));
                }
            }
            if self.shortcuts.take(ui, bedtime_keybindings::RESET_ZOOM)
                && let Some(scene) = &self.scene
            {
                self.camera.fit(scene, canvas.size[0] / canvas.size[1]);
            }
        }
        if canvas.hovered {
            self.camera.zoom(
                ui.io().mouse_wheel(),
                self.scene.as_ref().map_or(1.0, Scene::radius),
            );
            if ui.is_mouse_double_clicked(MouseButton::Left)
                && let Some(scene) = &self.scene
            {
                self.camera.fit(scene, canvas.size[0] / canvas.size[1]);
            }
        }
        if canvas.active {
            let delta = ui.io().mouse_delta();
            if ui.is_mouse_dragging(MouseButton::Right) || ui.is_mouse_dragging(MouseButton::Middle)
            {
                self.camera.pan(delta, canvas.size[1]);
            } else if ui.is_mouse_dragging(MouseButton::Left) {
                self.camera.orbit(delta);
            }
        }
        let state = CanvasState {
            pixels: canvas.pixels,
            camera: self.camera,
            background: ui.style_color(StyleColor::WindowBg),
            settings: self.settings,
        };
        if self.canvas != Some(state) {
            self.canvas = Some(state);
            self.output_revision += 1;
            self.settle_frames = render::SETTLE_FRAMES;
        } else if self.settle_frames > 0 || self.gpu.as_ref().is_some_and(|gpu| gpu.pending()) {
            // Bevy's environment filtering and temporal AA need a few frames.
            // Return to revision-cached rendering after the image settles.
            self.output_revision += 1;
            self.settle_frames = self.settle_frames.saturating_sub(1);
        }
    }
}
