//! Host-owned preparation and isolated model scenes in the application's Bevy world.
use bedtime_ui::{UiContext, UiSystems, UiViewport, egui, viewport_image};
use bevy::{
    ecs::{lifecycle::HookContext, world::DeferredWorld},
    prelude::*,
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

mod bevy_scene;
mod camera;
mod debug_views;
mod draco;
mod environment;
mod memory;
mod mesh_compat;
mod meshopt;
mod model;
mod prepare;
mod render;
mod skin;
mod skybox_blur;
mod stl;
#[cfg(test)]
mod tests;
#[cfg(feature = "workbench")]
pub mod workbench;

pub use camera::Camera as OrbitCamera;
pub use debug_views::DisplayMode;
pub use render::{Lighting, Settings as Appearance};
pub const SUPPORTED_EXTENSIONS: &[&str] = &["gltf", "glb", "stl"];

/// Immutable result of CPU preparation. Construct on a host-owned worker.
pub struct PreparedModel {
    model: model::PreparedModel,
    environments: Arc<[Image; 5]>,
}
impl std::fmt::Debug for PreparedModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedModel").finish_non_exhaustive()
    }
}
impl PreparedModel {
    pub fn prepare(
        bytes: &[u8],
        path: &Path,
        mut resolve: impl FnMut(&Path) -> Result<Vec<u8>, String>,
    ) -> Result<Self, String> {
        let model = if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("stl"))
        {
            model::PreparedModel::Stl(Box::new(stl::prepare(bytes)?))
        } else {
            model::PreparedModel::Gltf(prepare::load_with(bytes, &mut resolve)?)
        };
        static ENVIRONMENTS: OnceLock<Result<Arc<[Image; 5]>, String>> = OnceLock::new();
        let environments = ENVIRONMENTS
            .get_or_init(|| {
                Ok(Arc::new([
                    render::environment(Lighting::Studio),
                    render::environment(Lighting::Outdoor),
                    render::environment(Lighting::Neutral),
                    environment::bundled(0)?,
                    environment::bundled(1)?,
                ]))
            })
            .clone()?;
        Ok(Self {
            model,
            environments,
        })
    }
    pub fn warnings(&self) -> &[String] {
        match &self.model {
            model::PreparedModel::Gltf(source) => &source.warnings,
            _ => &[],
        }
    }
}

/// Decode a relative glTF resource URI. The host resolves it from the model directory.
pub fn relative_resource(uri: &str) -> Result<PathBuf, String> {
    if uri.contains(['?', '#', '\\']) {
        return Err("Invalid local glTF resource URI".into());
    }
    let mut decoded = Vec::with_capacity(uri.len());
    let mut bytes = uri.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let hex = |b: Option<u8>| {
                b.and_then(|b| (b as char).to_digit(16))
                    .ok_or("Invalid URI escape")
            };
            let hi = hex(bytes.next())?;
            let lo = hex(bytes.next())?;
            decoded.push((hi * 16 + lo) as u8);
        } else {
            decoded.push(byte);
        }
    }
    let value = String::from_utf8(decoded).map_err(|_| "Resource URI is not UTF-8")?;
    let path = PathBuf::from(&value);
    if value.is_empty() || value.contains([':', '\\', '\0']) || path.is_absolute() {
        return Err(
            "glTF resources must use relative local paths; URLs and absolute paths are unsupported"
                .into(),
        );
    }
    Ok(path)
}

/// Inspection statistics for the currently displayed model, in authored coordinates.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ModelInfo {
    pub nodes: usize,
    pub meshes: usize,
    pub triangles: usize,
    pub textures: usize,
    pub animations: usize,
    pub posed_meshes: usize,
    pub draco_primitives: usize,
    pub meshopt_views: usize,
    pub minimum: [f32; 3],
    pub maximum: [f32; 3],
}

/// Register before AssetPlugin; ModelPlugin itself installs after the host renderer.
pub fn register_asset_source(app: &mut App) {
    memory::MemoryAssets::register(app);
}

pub struct ModelPlugin;
impl Plugin for ModelPlugin {
    fn build(&self, app: &mut App) {
        bevy_scene::SceneLoading::register(app);
        if app.get_sub_app(bevy::render::RenderApp).is_some() {
            app.add_plugins(skybox_blur::SkyboxBlurPlugin);
        }
        app.add_systems(Update, prepare_scenes).add_systems(
            PostUpdate,
            sync_scenes
                .in_set(UiSystems::SyncViewports)
                .before(bevy::transform::TransformSystems::Propagate),
        );
    }
}
#[derive(Resource)]
struct Environments([Handle<Image>; 5]);

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ViewState {
    pub camera: Option<OrbitCamera>,
    pub appearance: Appearance,
}
impl ViewState {
    pub fn validate(&self) -> Result<(), String> {
        let a = self.appearance;
        if !a.normal_length.is_finite()
            || !(0.01..=0.3).contains(&a.normal_length)
            || !a.skybox_blur.is_finite()
            || !(0.0..=1.0).contains(&a.skybox_blur)
            || !a.horizon.is_finite()
            || !(-45.0..=45.0).contains(&a.horizon)
            || !a.exposure.is_finite()
            || !(-4.0..=4.0).contains(&a.exposure)
        {
            return Err("Invalid model appearance".into());
        }
        if let Some(c) = self.camera {
            if !c.target.is_finite()
                || c.target.abs().max_element() > 1e15
                || !c.yaw.is_finite()
                || !c.pitch.is_finite()
                || !(-1.5..=1.5).contains(&c.pitch)
                || !c.distance.is_finite()
                || !(1e-6..=1e15).contains(&c.distance)
            {
                return Err("Invalid model camera".into());
            }
        }
        Ok(())
    }
}

#[derive(Component, Default)]
#[require(Transform, Visibility)]
#[component(on_remove = remove_scene)]
pub struct ModelScene {
    pub view: ViewState,
    queued: Option<Arc<PreparedModel>>,
    content: Option<Arc<PreparedModel>>,
    active: Option<render::SceneGpu>,
    pending: Option<render::SceneGpu>,
    failure: Option<String>,
    generation: u64,
}
impl ModelScene {
    /// Queue a replacement; the last valid scene stays until the new scene is ready.
    pub fn set_model(&mut self, model: Arc<PreparedModel>) {
        self.queued = Some(model);
        self.failure = None;
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn info(&self) -> Option<ModelInfo> {
        let scene = self.active.as_ref()?.snapshot()?;
        Some(ModelInfo {
            nodes: scene.nodes,
            meshes: scene.primitives.len(),
            triangles: scene.triangles(),
            textures: scene.textures,
            animations: scene.animations,
            posed_meshes: scene.posed_meshes,
            draco_primitives: scene.draco_primitives,
            meshopt_views: scene.meshopt_views,
            minimum: scene.minimum.to_array(),
            maximum: scene.maximum.to_array(),
        })
    }
    pub fn error(&self) -> Option<&str> {
        self.failure.as_deref()
    }
    pub fn is_ready(&self) -> bool {
        self.active.as_ref().is_some_and(|s| s.ready())
    }
    pub fn is_loading(&self) -> bool {
        self.queued.is_some() || self.pending.is_some()
    }
    pub fn zoom(&mut self, steps: f32) {
        if let Some(radius) = self
            .active
            .as_ref()
            .and_then(|scene| scene.snapshot())
            .map(|scene| scene.radius())
        {
            if let Some(camera) = self.view.camera.as_mut() {
                camera.zoom(steps, radius);
            }
        }
    }
    pub fn set_error(&mut self, error: String) {
        self.failure = Some(error);
    }
}
fn remove_scene(mut world: DeferredWorld, context: HookContext) {
    let Some(state) = world.get::<ModelScene>(context.entity) else {
        return;
    };
    let roots: Vec<_> = state
        .active
        .iter()
        .chain(state.pending.iter())
        .map(|scene| scene.root)
        .collect();
    for root in roots {
        world.commands().entity(root).try_despawn();
    }
    let entity = context.entity;
    world.commands().queue(move |world: &mut World| {
        if world.get_entity(entity).is_ok() && world.get::<ModelScene>(entity).is_none() {
            world.entity_mut(entity).remove::<UiViewport>();
        }
    });
}
fn prepare_scenes(world: &mut World) {
    if !world.contains_resource::<Assets<Image>>()
        || !world.contains_resource::<Assets<StandardMaterial>>()
    {
        return;
    }
    let entities: Vec<_> = world
        .query_filtered::<Entity, With<ModelScene>>()
        .iter(world)
        .collect();
    for entity in entities {
        let queued = world.get_mut::<ModelScene>(entity).unwrap().queued.take();
        if let Some(content) = queued {
            if !world.contains_resource::<Environments>() {
                let handles = std::array::from_fn(|index| {
                    world
                        .resource_mut::<Assets<Image>>()
                        .add(content.environments[index].clone())
                });
                world.insert_resource(Environments(handles));
            }
            let old = world.get_mut::<ModelScene>(entity).unwrap().pending.take();
            if let Some(mut old) = old {
                old.clear(world);
            }
            let image = world
                .get::<UiViewport>(entity)
                .map(|v| v.image.clone())
                .unwrap_or_else(|| {
                    world
                        .resource_mut::<Assets<Image>>()
                        .add(viewport_image(UVec2::ONE))
                });
            let scene = render::SceneGpu::new(world, entity, &content.model, image.clone());
            world.entity_mut(scene.root).insert(Visibility::Hidden);
            world.entity_mut(entity).insert(UiViewport::new(image));
            let mut state = world.get_mut::<ModelScene>(entity).unwrap();
            state.pending = Some(scene);
            state.content = Some(content);
        }
    }
}
fn sync_scenes(world: &mut World) {
    let entities: Vec<_> = world
        .query_filtered::<Entity, (With<ModelScene>, With<UiViewport>)>()
        .iter(world)
        .collect();
    for entity in entities {
        let viewport = world.get::<UiViewport>(entity).unwrap();
        let (visible, size) = (viewport.visible, viewport.physical_size);
        let (mut active, mut pending, mut view) = {
            let mut state = world.get_mut::<ModelScene>(entity).unwrap();
            (state.active.take(), state.pending.take(), state.view)
        };
        let mut error = None;
        let mut promoted = false;
        if let Some(scene) = pending.as_mut() {
            let camera = view.camera.unwrap_or_else(|| OrbitCamera::default());
            match scene.sync(world, camera, view.appearance) {
                Ok(()) if scene.ready() => {
                    if let Some(mut old) = active.take() {
                        old.clear(world);
                    }
                    active = pending.take();
                    promoted = true;
                }
                Err(failure) => {
                    error = Some(failure);
                    pending.take().unwrap().clear(world);
                }
                _ => {}
            }
        }
        if let Some(scene) = active.as_mut() {
            let snapshot = scene.snapshot().unwrap();
            let camera = view.camera.get_or_insert_with(|| {
                let mut camera = OrbitCamera::default();
                camera.fit(snapshot, size.x as f32 / size.y.max(1) as f32);
                camera
            });
            if visible {
                if let Err(failure) = scene.sync(world, *camera, view.appearance) {
                    error = Some(failure);
                }
            }
            world.get_mut::<Camera>(scene.camera).unwrap().is_active = visible;
            world.entity_mut(scene.root).insert(if visible {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            });
        }
        if let Some(scene) = pending.as_ref() {
            world.get_mut::<Camera>(scene.camera).unwrap().is_active = false;
        }
        let mut state = world.get_mut::<ModelScene>(entity).unwrap();
        state.active = active;
        state.pending = pending;
        state.view = view;
        if promoted {
            state.generation += 1;
        }
        if let Some(error) = error {
            state.failure = Some(error);
        }
    }
}

pub struct ModelView;
impl ModelView {
    /// Draw the controls and viewport. Return the canvas response for host context menus.
    pub fn show(ui: &mut egui::Ui, world: &mut World, entity: Entity) -> Option<egui::Response> {
        world.get::<ModelScene>(entity)?;
        let response = if world.get::<UiViewport>(entity).is_some() {
            UiViewport::show(ui, world, entity)
        } else {
            ui.allocate_exact_size(
                ui.available_size().max(egui::Vec2::splat(1.0)),
                egui::Sense::click_and_drag(),
            )
            .1
        };
        let state = world.get::<ModelScene>(entity).unwrap();
        let mut overlay = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("model-controls")
                .max_rect(response.rect.shrink(8.0))
                .layout(egui::Layout::top_down(egui::Align::LEFT)),
        );
        let mut view = state.view;
        let ready = state.is_ready();
        let loading = state.is_loading();
        let error = state.failure.clone();
        let ao_supported = state.active.as_ref().is_none_or(|scene| scene.ao_supported);
        let mut frame = false;
        overlay.horizontal_wrapped(|ui| {
            frame = ui.button("Frame All").clicked();
            ui.menu_button("Info", |ui| {
                if let Some(scene) = state.active.as_ref().and_then(|s| s.snapshot()) {
                    ui.label(format!(
                        "{} nodes · {} meshes · {} triangles",
                        scene.nodes,
                        scene.primitives.len(),
                        scene.triangles()
                    ));
                    ui.label(format!(
                        "{} textures · {} animations",
                        scene.textures, scene.animations
                    ));
                    if scene.draco_primitives > 0 {
                        ui.label(format!(
                            "{} Draco primitives decoded",
                            scene.draco_primitives
                        ));
                    }
                    if scene.meshopt_views > 0 {
                        ui.label(format!(
                            "{} meshopt buffer views decoded",
                            scene.meshopt_views
                        ));
                    }
                    if scene.posed_meshes > 0 {
                        ui.label(format!(
                            "{} meshes in authored skeleton pose",
                            scene.posed_meshes
                        ));
                    }
                }
                if let Some(content) = &state.content {
                    for warning in content.warnings() {
                        ui.label(warning);
                    }
                }
            });
            let appearance = ui.button("Appearance");
            egui::Popup::menu(&appearance)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| {
                    let a = &mut view.appearance;
                    ui.label("View");
                    ui.menu_button(DisplayMode::NAMES[a.display.index()], |ui| {
                        for (i, name) in DisplayMode::NAMES.iter().enumerate() {
                            ui.selectable_value(&mut a.display, DisplayMode::from_index(i), *name);
                        }
                    });
                    ui.checkbox(&mut a.normals, "Normal vectors")
                        .on_hover_text("Sampled to at most 6,000 vectors");
                    if a.normals {
                        ui.add(
                            egui::Slider::new(&mut a.normal_length, 0.01..=0.3)
                                .text("Normal length"),
                        );
                    }
                    ui.separator();
                    ui.label("Lighting");
                    ui.menu_button(Lighting::NAMES[a.lighting.index()], |ui| {
                        for (i, name) in Lighting::NAMES.iter().enumerate() {
                            ui.selectable_value(&mut a.lighting, Lighting::from_index(i), *name);
                        }
                    });
                    ui.checkbox(&mut a.skybox, "Skybox");
                    if a.skybox {
                        ui.add(
                            egui::Slider::new(&mut a.skybox_blur, 0.0..=1.0).text("Skybox blur"),
                        );
                        ui.add(egui::Slider::new(&mut a.horizon, -45.0..=45.0).text("Horizon"));
                    }
                    ui.checkbox(&mut a.shadows, "Shadows");
                    ui.add_enabled(
                        ao_supported,
                        egui::Checkbox::new(&mut a.ao, "Ambient occlusion"),
                    );
                    if !ao_supported {
                        ui.weak("Ambient occlusion is unavailable on this GPU");
                    }
                    ui.add(egui::Slider::new(&mut a.exposure, -4.0..=4.0).text("Exposure"));
                    if ui.button("Reset Appearance").clicked() {
                        *a = Appearance::default();
                    }
                });
        });
        if let Some(error) = error {
            overlay.colored_label(overlay.visuals().error_fg_color, error);
        }
        if loading || (!ready && state.failure.is_none()) {
            overlay.horizontal(|ui| {
                ui.spinner();
                ui.weak("Loading model…");
            });
        }
        let mut hint_font = egui::TextStyle::Small.resolve(ui.style());
        hint_font.size *= 0.85;
        ui.painter().with_clip_rect(response.rect).text(
            response.rect.right_bottom() - egui::vec2(8.0, 6.0),
            egui::Align2::RIGHT_BOTTOM,
            "Drag to orbit · Right/middle-drag to pan · Scroll to zoom",
            hint_font,
            ui.visuals().weak_text_color(),
        );
        if ready {
            let scene = world
                .get::<ModelScene>(entity)
                .unwrap()
                .active
                .as_ref()
                .unwrap()
                .snapshot()
                .unwrap();
            let radius = scene.radius();
            let camera = view.camera.get_or_insert_with(|| OrbitCamera::default());
            let eligible =
                ui.is_enabled() && ui.input(|i| i.focused) && !egui::Popup::is_any_open(ui.ctx());
            if frame || (eligible && response.double_clicked()) {
                camera.fit(scene, response.rect.aspect_ratio());
            }
            if eligible {
                let delta = response.drag_delta();
                if response.dragged_by(egui::PointerButton::Secondary)
                    || response.dragged_by(egui::PointerButton::Middle)
                {
                    camera.pan([delta.x, delta.y], response.rect.height());
                } else if response.dragged_by(egui::PointerButton::Primary) {
                    camera.orbit([delta.x, delta.y]);
                }
                if response.hovered() {
                    let wheel = ui.input_mut(|i| {
                        let wheel = i.smooth_scroll_delta.y;
                        i.smooth_scroll_delta = egui::Vec2::ZERO;
                        wheel
                    });
                    camera.zoom(wheel / 40.0, radius);
                }
            }
        }
        world.get_mut::<ModelScene>(entity).unwrap().view = view;
        Some(response)
    }
}
impl bedtime_ui::Component for ModelView {
    type Props<'a> = Entity;
    fn draw(ui: &mut egui::Ui, world: &mut World, _: &UiContext<'_>, entity: Entity) {
        Self::show(ui, world, entity);
    }
}

#[cfg(feature = "workbench")]
pub mod document;
