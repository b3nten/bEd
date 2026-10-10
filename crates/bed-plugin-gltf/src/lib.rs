//! glTF and STL model viewer with a shared-device, windowless Bevy renderer.
use bed_editing::identity::DocumentId;
use bed_plugin::gpu::{Canvas, GpuContext, RenderOutput, RenderTarget};
use bed_plugin::{
    CommandContext, DocumentKind, HostContext, HostRequest, MenuSlot, Plugin, PluginPanel,
    Registrar, Revision, TextureHandle,
};
use bed_ui::util::popup_style::tooltip_text;
use dear_imgui_rs::{MouseButton, StyleColor, Ui};
use serde_json::Value;
use std::{
    any::Any,
    sync::{Arc, mpsc},
    thread,
};

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

pub const PLUGIN_ID: &str = "bed.gltf";
pub const PANEL_ID: &str = "bed.gltf.panel";
pub const VIEWER_ID: &str = "bed.gltf.viewer";
pub const OPEN_COMMAND: &str = "bed.gltf.open";
pub const OPEN_FILE_COMMAND: &str = "bed.gltf.open-file";

#[derive(Default)]
pub struct GltfPlugin;
impl Plugin for GltfPlugin {
    fn id(&self) -> &'static str {
        PLUGIN_ID
    }
    fn register(&self, registrar: &mut Registrar<'_>) {
        registrar.panel(PANEL_ID, "Model Viewer");
        registrar.viewer(
            VIEWER_ID,
            "Model Viewer",
            PANEL_ID,
            &["gltf", "glb", "stl"],
            DocumentKind::Bytes,
        );
        registrar.viewer_document_kinds(VIEWER_ID, &[DocumentKind::Text, DocumentKind::Bytes]);
        registrar.command(OPEN_COMMAND, "Open 3D Model…", Some("image"));
        registrar.command(OPEN_FILE_COMMAND, "Open in Model Viewer", Some("image"));
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
                    ["glb", "gltf", "stl"]
                        .iter()
                        .any(|ext| ext.eq_ignore_ascii_case(extension))
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
            return Err(format!("Unknown glTF panel: {panel_type}"));
        }
        let Some(document) = document else {
            return Ok(Box::new(EmptyPanel));
        };
        Ok(Box::new(GltfPanel::new(document, state)))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
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
                        stl::prepare(&job.bytes).map(PreparedModel::Stl)
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
    document: DocumentId,
    texture: TextureHandle,
    loader: Loader,
    requested: Option<Revision>,
    prepared: Option<PreparedModel>,
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
}
impl GltfPanel {
    fn new(document: DocumentId, state: &Value) -> Self {
        let (camera, fitted) = Camera::restore(state);
        Self {
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
        }
    }
    fn update(&mut self, host: &HostContext<'_>) {
        let Some(document) = host.document(self.document) else {
            self.prepared = None;
            self.scene = None;
            self.gpu = None;
            return;
        };
        while let Ok(result) = self.loader.receiver.try_recv() {
            if result.revision != document.revision {
                continue;
            }
            match result.prepared {
                Ok(prepared) => {
                    self.prepared = Some(prepared);
                    self.scene = None;
                    self.error = None;
                    self.gpu = None;
                    self.failed_generation = None;
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
            self.prepared = None;
            self.scene = None;
            self.gpu = None;
            self.error = None;
            self.failed_generation = None;
        }
    }
}
impl PluginPanel for GltfPanel {
    fn title(&self, host: &HostContext<'_>) -> String {
        host.document(self.document)
            .and_then(|d| d.path.rsplit(['/', '\\']).next())
            .unwrap_or("Model Viewer")
            .to_owned()
    }
    fn attached_document(&self) -> Option<DocumentId> {
        Some(self.document)
    }
    fn save_state(&self) -> Value {
        let mut state = self.camera.save();
        state["render"] = self.settings.save();
        state
    }
    fn close(&mut self, _: &mut Vec<HostRequest>) {
        self.loader.sender = None;
        // Disconnect results too: an in-flight worker must not stay blocked
        // sending into a full result queue after this panel stops polling.
        self.loader.receiver = mpsc::channel().1;
        self.prepared = None;
        self.scene = None;
        self.gpu = None;
    }
    fn render_output(&self) -> Option<RenderOutput> {
        self.prepared.as_ref()?;
        Some(RenderOutput {
            handle: self.texture,
            size: self.canvas?.pixels,
            depth: true,
            revision: self.output_revision,
        })
    }
    fn render(&mut self, gpu: &mut GpuContext<'_>, target: &RenderTarget) -> Result<(), String> {
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
        if self
            .gpu
            .as_ref()
            .is_none_or(|state| state.generation != gpu.generation)
        {
            match render::SceneGpu::new(gpu, prepared) {
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
        let result = self.gpu.as_mut().unwrap().render(
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
    fn draw(&mut self, ui: &Ui, host: &HostContext<'_>, requests: &mut Vec<HostRequest>) {
        let _controls = bed_ui::util::popup_style::controls_style(ui);
        self.update(host);
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
            let _popup = bed_ui::util::popup_style::context_menu_style(ui);
            if let Some(_popup) = ui.begin_popup("model-appearance") {
                let mut display = self.settings.display.index();
                if ui.combo_simple_string("View", &mut display, &debug_views::DisplayMode::NAMES) {
                    self.settings.display = debug_views::DisplayMode::from_index(display);
                }
                ui.checkbox("Normal vectors", &mut self.settings.normals);
                if ui.is_item_hovered() {
                    tooltip_text(ui, "Show vertex normals, sampled to at most 6,000 vectors.");
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
                            "Soften the background while keeping model lighting and reflections sharp.",
                        );
                    }
                    ui.slider_config("Horizon", -45.0, 45.0)
                        .try_display_format("%.0f°")
                        .expect("valid horizon format")
                        .build(&mut self.settings.horizon);
                    if ui.is_item_hovered() {
                        tooltip_text(ui, "Lower values move the background horizon down.");
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
            let _popup = bed_ui::util::popup_style::context_menu_style(ui);
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
                if let Some(PreparedModel::Gltf(prepared)) = &self.prepared {
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
                requests.push(HostRequest::OpenFile {
                    path: document.path.clone(),
                    viewer: Some("bed.hex".into()),
                });
            }
            return;
        }
        if self.prepared.is_none() {
            ui.text_disabled("Loading model…");
            return;
        }
        if self.scene.is_none() {
            ui.text_disabled("Loading model…");
        }
        let canvas = Canvas::show(ui, host, self.texture, "gltf-canvas");
        if let Some(scene) = &self.scene
            && (!self.fitted || frame_all)
        {
            self.camera.fit(scene, canvas.size[0] / canvas.size[1]);
            self.fitted = true;
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
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// An unloaded viewer owns no document, worker, or resource session.
struct EmptyPanel;

impl PluginPanel for EmptyPanel {
    fn title(&self, _: &HostContext<'_>) -> String {
        "Model Viewer".into()
    }

    fn draw(
        &mut self,
        ui: &dear_imgui_rs::Ui,
        _: &HostContext<'_>,
        requests: &mut Vec<HostRequest>,
    ) {
        ui.text_wrapped("Open a 3D model to view it.");
        if ui.button("Open Model…") {
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
        let mut plugin = GltfPlugin::default();
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
        assert_eq!(panel.title(&host), "Model Viewer");
        assert!(panel.is_input_empty(&host));
        assert!(panel.attached_document().is_none());
        assert_eq!(panel.save_state(), Value::Null);
        assert!(panel.render_output().is_none());
    }
}
