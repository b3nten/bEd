//! The model viewer as a workbench panel. The document is the file's bytes;
//! each revision is prepared on a worker (reading a `.gltf`'s other files
//! beside it) and shown once ready, keeping the camera. The scene fills the
//! panel. Floating at the top right: frame it all, what it has, and how it's
//! shown; the right-click menu has the same.
use crate::{Appearance, DisplayMode, Lighting, ModelScene, ModelView, PreparedModel};
use bedtime_document::DocumentModel;
use bedtime_filesystem::{ByteModel, FileModel};
use bedtime_ui::{BuiltinIcon, Floating, UiContext, egui};
use bedtime_ui::{Button, MenuItem, Separator, Space, Spinner, Submenu, Text, Tip};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
use bedtime_workbench::{
    ActionHandler, ActionInvocation, ActionResult, BedtimeWorldExt, DocumentReference,
    DocumentStatus, Handler, ManagedTask, ManagedWork, Panel, PanelInstance, ResetZoom,
    ViewersAppExt, ZoomIn, ZoomOut, viewer_document,
};
use bevy::prelude::*;
use std::{path::Path, sync::Arc};

/// Models and the files they use are read up to this size.
const MAX_MODEL: u64 = 128 * 1024 * 1024;
const MAX_RESOURCE: u64 = 64 * 1024 * 1024;

/// The model viewer: `ModelPlugin`, and an opener for glTF, GLB and STL.
pub struct ModelViewerPlugin;

impl Plugin for ModelViewerPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<crate::ModelPlugin>() {
            app.add_plugins(crate::ModelPlugin);
        }
        use bedtime_workbench::{Format, FormatId, FormatsAppExt};
        app.register_type::<ModelPanel>()
            .register_format(Format::new(MODEL))
            // glTF's text form is JSON too, so the editor opens it.
            .register_format(
                Format::new(FormatId("gltf"))
                    .extensions(["gltf"])
                    .conforms_to([MODEL, FormatId("json")]),
            )
            .register_format(
                Format::new(FormatId("glb"))
                    .extensions(["glb"])
                    .conforms_to([MODEL]),
            )
            .register_format(
                Format::new(FormatId("stl"))
                    .extensions(["stl"])
                    .conforms_to([MODEL]),
            )
            .register_viewer::<ModelFile, ModelPanel>("3D Model")
            .formats([MODEL])
            .add_systems(Update, prepare_models);
    }
}

/// 3D models: glTF (`.gltf`, `.glb`) and STL.
pub const MODEL: bedtime_workbench::FormatId = bedtime_workbench::FormatId("3d-model");

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

impl ByteModel for ModelFile {
    fn decode(bytes: &[u8]) -> Result<Self, String> {
        Ok(Self(bytes.into()))
    }
    fn encode(&self, out: &mut dyn std::io::Write) -> std::io::Result<()> {
        out.write_all(&self.0)
    }
}

impl FileModel for ModelFile {
    const MAX_LEN: u64 = MAX_MODEL;
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

/// Prepare each panel's model when its document changes, on a worker.
fn prepare_models(world: &mut World) {
    let panels: Vec<Entity> = world
        .query_filtered::<Entity, With<ModelPanel>>()
        .iter(world)
        .collect();
    let panels: Vec<(Entity, Option<std::path::PathBuf>)> = panels
        .into_iter()
        .map(|panel| (panel, world.bedtime().holder_file(panel)))
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
        let Ok(info) = world.bedtime().inspect_document(document) else {
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
            .bedtime()
            .cast_document::<ModelFile>(document)
            .ok()
            .and_then(|handle| world.bedtime().read_document(handle).ok().cloned())
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
                        f.ui.add(Text::new(error.to_owned()));
                    } else {
                        f.ui.add(Spinner::new());
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
            // Hovering shows what's in the model; there's nothing to click.
            f.ui.add(Button::icon_only(BuiltinIcon::Info, "Info").ghost())
                .tip_ui(|ui| info_menu(ui, &scene));
            let appearance = f.floating_button(BuiltinIcon::Sliders, "Appearance");
            egui::Popup::menu(&appearance)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| appearance_menu(ui, &mut view.appearance, ao));
        });
        response.context_menu(|ui| {
            if ui.add(MenuItem::new("Frame All")).clicked() {
                view.camera = None;
            }
            ui.add(Separator::new().space(Space::Xs));
            choices(ui, &mut view.appearance);
            ui.add(Separator::new().space(Space::Xs));
            {
                let item = ui.add(
                    MenuItem::new("Skybox")
                        .checked(view.appearance.skybox)
                        .keep_open(),
                );
                if item.clicked() {
                    view.appearance.skybox = !view.appearance.skybox;
                }
                item
            };
            {
                let item = ui.add(
                    MenuItem::new("Shadows")
                        .checked(view.appearance.shadows)
                        .keep_open(),
                );
                if item.clicked() {
                    view.appearance.shadows = !view.appearance.shadows;
                }
                item
            };
            ui.add(Separator::new().space(Space::Xs));
            if ui.add(MenuItem::new("Reset Appearance")).clicked() {
                view.appearance = Appearance::default();
            }
        });
        scene.view = view;
    }
}

/// What's in the model, for the info icon's tooltip.
fn info_menu(ui: &mut egui::Ui, scene: &ModelScene) {
    match scene.info() {
        Some(info) => {
            ui.add(Text::new(format!(
                "{} nodes · {} meshes · {} triangles",
                info.nodes, info.meshes, info.triangles
            )));
            ui.add(Text::new(format!(
                "{} textures · {} animations",
                info.textures, info.animations
            )));
            let size: Vec<String> = (0..3)
                .map(|axis| format!("{:.3}", info.maximum[axis] - info.minimum[axis]))
                .collect();
            ui.add(Text::new(format!("Size {}", size.join(" × "))));
            if info.draco_primitives > 0 {
                ui.add(
                    Text::new(format!("{} Draco-compressed meshes", info.draco_primitives)).muted(),
                );
            }
            if info.meshopt_views > 0 {
                ui.add(
                    Text::new(format!("{} meshopt-compressed buffers", info.meshopt_views)).muted(),
                );
            }
            if info.posed_meshes > 0 {
                ui.add(
                    Text::new(format!(
                        "{} meshes in their skeleton's pose",
                        info.posed_meshes
                    ))
                    .muted(),
                );
            }
        }
        None => {
            ui.add(Text::new("Loading…").muted());
        }
    }
    for warning in scene.warnings() {
        ui.add(Text::new(warning).muted());
    }
    ui.add(Separator::new());
    ui.add(Text::new("Drag to orbit · Right-drag to pan · Scroll to zoom").muted());
}

/// Display and lighting, as submenus.
fn choices(ui: &mut egui::Ui, appearance: &mut Appearance) {
    Submenu::new("Display").show(ui, |ui| {
        for (index, name) in DisplayMode::NAMES.iter().enumerate() {
            let mode = DisplayMode::from_index(index);
            if ui
                .add(
                    MenuItem::new(*name)
                        .checked(appearance.display == mode)
                        .keep_open(),
                )
                .clicked()
            {
                appearance.display = mode;
            }
        }
    });
    Submenu::new("Lighting").show(ui, |ui| {
        for (index, name) in Lighting::NAMES.iter().enumerate() {
            let lighting = Lighting::from_index(index);
            if ui
                .add(
                    MenuItem::new(*name)
                        .checked(appearance.lighting == lighting)
                        .keep_open(),
                )
                .clicked()
            {
                appearance.lighting = lighting;
            }
        }
    });
}

fn appearance_menu(ui: &mut egui::Ui, a: &mut Appearance, ao: bool) {
    ui.set_min_width(220.0);
    choices(ui, a);
    {
        let item = ui.add(
            MenuItem::new("Normal Vectors")
                .checked(a.normals)
                .keep_open(),
        );
        if item.clicked() {
            a.normals = !a.normals;
        }
        item
    }
    .on_hover_text("At most 6,000 are drawn");
    if a.normals {
        slider(ui, "Length", &mut a.normal_length, 0.01..=0.3);
    }
    ui.add(Separator::new().space(Space::Xs));
    {
        let item = ui.add(MenuItem::new("Skybox").checked(a.skybox).keep_open());
        if item.clicked() {
            a.skybox = !a.skybox;
        }
        item
    };
    if a.skybox {
        slider(ui, "Blur", &mut a.skybox_blur, 0.0..=1.0);
        slider(ui, "Horizon", &mut a.horizon, -45.0..=45.0);
    }
    {
        let item = ui.add(MenuItem::new("Shadows").checked(a.shadows).keep_open());
        if item.clicked() {
            a.shadows = !a.shadows;
        }
        item
    };
    let occlusion = MenuItem::new("Ambient Occlusion").checked(a.ao).keep_open();
    let occlusion = ui.add_enabled(ao, occlusion);
    if occlusion.clicked() {
        a.ao = !a.ao;
    }
    occlusion.on_disabled_hover_text("This GPU can't do ambient occlusion here");
    slider(ui, "Exposure", &mut a.exposure, -4.0..=4.0);
    ui.add(Separator::new().space(Space::Xs));
    if ui.add(MenuItem::new("Reset Appearance")).clicked() {
        *a = Appearance::default();
    }
}

/// A slider in the appearance menu, its name after it.
fn slider(ui: &mut egui::Ui, name: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>) {
    bedtime_ui::Row::new().show(ui, |ui| {
        ui.add(bedtime_ui::Slider::new(value, range).label(name));
        ui.add(Text::new(name));
    });
}
