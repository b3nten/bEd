//! The model viewer as a workbench panel. The document is the file's bytes;
//! each revision is prepared on a worker (reading a `.gltf`'s other files
//! beside it) and shown once ready, keeping the camera. The scene fills the
//! panel. Floating at the top right: frame it all, what it has, and how it's
//! shown; the right-click menu has the same.
use crate::{Appearance, DisplayMode, Lighting, ModelScene, ModelView, PreparedModel};
use bedtime_document::DocumentModel;
use bedtime_filesystem::FileModel;
use bedtime_ui::{BuiltinIcon, Floating, UiContext, egui};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
use bedtime_workbench::{
    ActionHandler, ActionInvocation, ActionResult, DocumentHandle, DocumentReference,
    DocumentStatus, Fit, Handler, ManagedTask, ManagedWork, Opener, OpenersAppExt, Panel,
    PanelInstance, ResetZoom, Viewer, ViewerFile, WorkbenchWorldExt, ZoomIn, ZoomOut,
    viewer_document,
};
use bevy::prelude::*;
use std::{path::Path, sync::Arc};

/// Models and the files they use are read up to this size.
const MAX_MODEL: u64 = 128 * 1024 * 1024;
const MAX_RESOURCE: u64 = 64 * 1024 * 1024;

/// The model viewer: `ModelPlugin`, and an opener for glTF, GLB and STL.
/// Register the model asset source (`register_asset_source`) before
/// `AssetPlugin`.
pub struct ModelViewerPlugin;

impl Plugin for ModelViewerPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<crate::ModelPlugin>() {
            app.add_plugins(crate::ModelPlugin);
        }
        app.register_type::<ModelPanel>()
            .register_opener(Opener::<ModelFile>::new::<ModelPanel>("3D Model").files(fit))
            .add_systems(Update, prepare_models);
    }
}

fn fit(path: &Path, _: &[u8]) -> Fit {
    let known = path.extension().is_some_and(|ext| {
        crate::SUPPORTED_EXTENSIONS
            .iter()
            .any(|known| ext.eq_ignore_ascii_case(known))
    });
    if known { Fit::Specific } else { Fit::No }
}

/// A model file's bytes.
#[derive(Clone, Default, PartialEq)]
pub struct ModelFile(pub Arc<[u8]>);

impl DocumentModel for ModelFile {
    type Change = std::convert::Infallible;
    type OpenState = ();
    fn apply(&mut self, change: &Self::Change) -> Result<Self::Change, String> {
        match *change {}
    }
}

impl FileModel for ModelFile {
    const MAX_LEN: u64 = MAX_MODEL;
    fn decode(bytes: &[u8]) -> Result<Self, String> {
        Ok(Self(bytes.into()))
    }
    fn encode(&self, out: &mut dyn std::io::Write) -> std::io::Result<()> {
        out.write_all(&self.0)
    }
}

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
#[require(
    PanelInstance = PanelInstance::of::<Self>(),
    ModelScene,
    // Zooming moves the camera, rather than the panel's zoom.
    ActionHandler<ZoomIn> = ActionHandler::new(Handler::new(|w, i| zoom_camera(w, i, Some(1.0)))),
    ActionHandler<ZoomOut> = ActionHandler::new(Handler::new(|w, i| zoom_camera(w, i, Some(-1.0)))),
    ActionHandler<ResetZoom> = ActionHandler::new(Handler::new(|w, i| zoom_camera(w, i, None))),
)]
pub struct ModelPanel {
    #[reflect(ignore)]
    preparing: Option<(u64, Preparing)>,
    #[reflect(ignore)]
    prepared: Option<u64>,
}

/// A model being prepared on a worker.
type Preparing = ManagedTask<Result<Arc<PreparedModel>, String>>;

impl Viewer<ModelFile> for ModelPanel {
    fn new(_: DocumentHandle<ModelFile>) -> Self {
        Self::default()
    }
    fn apply(_: &mut World, _: Entity, _: &()) {}
}

/// Prepare each panel's model when its document changes, on a worker.
fn prepare_models(world: &mut World) {
    let panels: Vec<(Entity, Option<std::path::PathBuf>)> = world
        .query_filtered::<(Entity, Option<&ViewerFile>), With<ModelPanel>>()
        .iter(world)
        .map(|(panel, file)| (panel, file.map(|file| file.0.clone())))
        .collect();
    for (panel, path) in panels {
        // A finished preparation.
        let finished = world
            .get::<ModelPanel>(panel)
            .and_then(|model| model.preparing.as_ref())
            .and_then(|(_, task)| match task.try_recv() {
                Ok(result) => Some(result),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Some(Err("Preparing the model stopped".into()))
                }
            });
        if let Some(result) = finished {
            let mut model = world.get_mut::<ModelPanel>(panel).unwrap();
            let (revision, _) = model.preparing.take().unwrap();
            model.prepared = Some(revision);
            let mut scene = world.get_mut::<ModelScene>(panel).unwrap();
            match result {
                Ok(prepared) => scene.set_model(prepared),
                Err(error) => scene.set_error(error),
            }
        }
        // A new revision to prepare.
        let Some(document) = world.get::<DocumentReference>(panel).map(|r| r.document()) else {
            continue;
        };
        let Ok(info) = world.workbench().inspect_document(document) else {
            continue;
        };
        let model = world.get::<ModelPanel>(panel).unwrap();
        if info.status != DocumentStatus::Ready
            || model.prepared == Some(info.revision)
            || model
                .preparing
                .as_ref()
                .is_some_and(|(revision, _)| *revision == info.revision)
        {
            continue;
        }
        let Some(path) = path else {
            continue;
        };
        let Some(bytes) = world
            .workbench()
            .cast_document::<ModelFile>(document)
            .ok()
            .and_then(|handle| world.workbench().read_document(handle).ok().cloned())
        else {
            continue;
        };
        let task = world.resource_mut::<ManagedWork>().spawn_task(
            Some(panel),
            "Prepare model",
            move |cancel| prepare(&bytes.0, &path, || cancel.is_cancelled()),
        );
        let mut model = world.get_mut::<ModelPanel>(panel).unwrap();
        match task {
            Ok(task) => model.preparing = Some((info.revision, task)),
            Err(error) => {
                model.prepared = Some(info.revision);
                world.get_mut::<ModelScene>(panel).unwrap().set_error(error);
            }
        }
    }
}

/// Prepare a model, reading the files it uses from beside it.
pub fn prepare(
    bytes: &[u8],
    path: &Path,
    cancelled: impl Fn() -> bool,
) -> Result<Arc<PreparedModel>, String> {
    let directory = path.parent().unwrap_or(Path::new("."));
    let model = PreparedModel::prepare(bytes, path, |relative| {
        if cancelled() {
            return Err("Cancelled".into());
        }
        let resource = directory.join(relative);
        bedtime_filesystem::FileSource::local(&resource)
            .read(MAX_RESOURCE)
            .map(|snapshot| snapshot.bytes)
            .map_err(|error| format!("{}: {error}", resource.display()))
    })?;
    if cancelled() {
        return Err("Cancelled".into());
    }
    Ok(Arc::new(model))
}

/// Zoom the camera in or out a step, or (`None`) put it back.
fn zoom_camera(world: &mut World, invocation: ActionInvocation, step: Option<f32>) -> ActionResult {
    let mut scene = invocation
        .entity
        .and_then(|panel| world.get_mut::<ModelScene>(panel))
        .ok_or("No model is focused")?;
    match step {
        Some(step) => scene.zoom(step),
        None => scene.view.camera = None,
    }
    Ok(())
}

impl Panel for ModelPanel {
    const LABEL: &'static str = "3D Model";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        if viewer_document::<ModelFile>(ui, world, panel).is_none() {
            return;
        }
        let Some(response) = ModelView::show(ui, world, panel) else {
            return;
        };
        if response.clicked() || response.secondary_clicked() {
            response.request_focus();
        }
        let rect = response.rect;
        let Some(mut scene) = world.get_mut::<ModelScene>(panel) else {
            return;
        };
        let id = egui::Id::new(("bedtime.model", panel));
        let mut frame = context.frame(ui);
        let loading = scene.is_loading() || (!scene.is_ready() && scene.error().is_none());
        // Loading, or what went wrong, in the middle.
        if loading || scene.error().is_some() {
            Floating::new(id.with("state"), rect, egui::Align2::CENTER_CENTER)
                .shown(true)
                .show(&mut frame, |f| {
                    if let Some(error) = scene.error() {
                        f.ui.label(error.to_owned());
                    } else {
                        f.ui.spinner();
                        f.floating_label("Loading model…");
                    }
                });
        }
        let mut view = scene.view;
        let ao = scene.ao_supported();
        Floating::new(id.with("bar"), rect, egui::Align2::RIGHT_TOP).show(&mut frame, |f| {
            if f.floating_button(BuiltinIcon::Fit, "Frame All (Double-Click)")
                .clicked()
            {
                view.camera = None;
            }
            let info = f.floating_button(BuiltinIcon::Info, "About This Model");
            egui::Popup::menu(&info).show(|ui| info_menu(ui, &scene));
            let appearance = f.floating_button(BuiltinIcon::Sliders, "Appearance");
            egui::Popup::menu(&appearance)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| appearance_menu(ui, &mut view.appearance, ao));
        });
        response.context_menu(|ui| {
            if ui.button("Frame All").clicked() {
                view.camera = None;
            }
            ui.separator();
            choices(ui, &mut view.appearance);
            ui.separator();
            ui.checkbox(&mut view.appearance.skybox, "Skybox");
            ui.checkbox(&mut view.appearance.shadows, "Shadows");
            ui.separator();
            if ui.button("Reset Appearance").clicked() {
                view.appearance = Appearance::default();
            }
        });
        scene.view = view;
    }
}

fn info_menu(ui: &mut egui::Ui, scene: &ModelScene) {
    match scene.info() {
        Some(info) => {
            ui.label(format!(
                "{} nodes · {} meshes · {} triangles",
                info.nodes, info.meshes, info.triangles
            ));
            ui.label(format!(
                "{} textures · {} animations",
                info.textures, info.animations
            ));
            let size: Vec<String> = (0..3)
                .map(|axis| format!("{:.3}", info.maximum[axis] - info.minimum[axis]))
                .collect();
            ui.label(format!("Size {}", size.join(" × ")));
            if info.draco_primitives > 0 {
                ui.weak(format!("{} Draco-compressed meshes", info.draco_primitives));
            }
            if info.meshopt_views > 0 {
                ui.weak(format!("{} meshopt-compressed buffers", info.meshopt_views));
            }
            if info.posed_meshes > 0 {
                ui.weak(format!(
                    "{} meshes in their skeleton's pose",
                    info.posed_meshes
                ));
            }
        }
        None => {
            ui.weak("Loading…");
        }
    }
    for warning in scene.warnings() {
        ui.weak(warning);
    }
    ui.separator();
    ui.weak("Drag to orbit · Right-drag to pan · Scroll to zoom");
}

/// Display and lighting, as submenus.
fn choices(ui: &mut egui::Ui, appearance: &mut Appearance) {
    ui.menu_button("Display", |ui| {
        for (index, name) in DisplayMode::NAMES.iter().enumerate() {
            let mode = DisplayMode::from_index(index);
            if ui.radio(appearance.display == mode, *name).clicked() {
                appearance.display = mode;
            }
        }
    });
    ui.menu_button("Lighting", |ui| {
        for (index, name) in Lighting::NAMES.iter().enumerate() {
            let lighting = Lighting::from_index(index);
            if ui.radio(appearance.lighting == lighting, *name).clicked() {
                appearance.lighting = lighting;
            }
        }
    });
}

fn appearance_menu(ui: &mut egui::Ui, a: &mut Appearance, ao: bool) {
    ui.set_min_width(220.0);
    choices(ui, a);
    ui.checkbox(&mut a.normals, "Normal Vectors")
        .on_hover_text("At most 6,000 are drawn");
    if a.normals {
        ui.add(egui::Slider::new(&mut a.normal_length, 0.01..=0.3).text("Length"));
    }
    ui.separator();
    ui.checkbox(&mut a.skybox, "Skybox");
    if a.skybox {
        ui.add(egui::Slider::new(&mut a.skybox_blur, 0.0..=1.0).text("Blur"));
        ui.add(egui::Slider::new(&mut a.horizon, -45.0..=45.0).text("Horizon"));
    }
    ui.checkbox(&mut a.shadows, "Shadows");
    ui.add_enabled(ao, egui::Checkbox::new(&mut a.ao, "Ambient Occlusion"))
        .on_disabled_hover_text("This GPU can't do ambient occlusion here");
    ui.add(egui::Slider::new(&mut a.exposure, -4.0..=4.0).text("Exposure"));
    ui.separator();
    if ui.button("Reset Appearance").clicked() {
        *a = Appearance::default();
    }
}
