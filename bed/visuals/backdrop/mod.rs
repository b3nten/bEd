//! The window's backdrop: a fragment shader shown in place of the theme's
//! background colour. Each `<name>.wgsl` in the config folder's `backdrops`
//! is one, chosen in Settings; an edited file shows as it is saved. Or a 3D
//! scene of bEd's own (`flyover`, `internals`; what they share is in `scene`).
mod flyover;
mod internals;
pub mod scene;

use bedtime::ui::{BackdropStatus, SurfaceBackdrop, layouts::LayoutSurface};
use bedtime::workbench::{
    BedtimeWorldExt,
    persistence::PersistenceDirectory,
    settings::{
        SettingsAppExt, settings,
        ui::{combo, row},
    },
};
use bevy::prelude::*;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

/// The backdrops bEd ships: each is written to the folder when missing from it.
const SHIPPED: &[(&str, &str)] = &[
    ("aurora", include_str!("shaders/aurora.wgsl")),
    ("dots", include_str!("shaders/dots.wgsl")),
    ("flow", include_str!("shaders/flow.wgsl")),
    ("gradient", include_str!("shaders/gradient.wgsl")),
    ("grid", include_str!("shaders/grid.wgsl")),
    ("starfield", include_str!("shaders/starfield.wgsl")),
];
/// How often a moving backdrop redraws, per second.
pub const FRAME_RATE: f32 = 30.0;
const RESCAN: Duration = Duration::from_millis(500);
const NONE: &str = "None";

settings! {
    "Window" {
        /// A backdrop's name: a file in the backdrops folder, without `.wgsl`,
        /// or a scene's (`scene:flyover`). Empty is none.
        pub Backdrop: String {
            default: String::new(),
            key: "appearance.backdrop",
            label: "Backdrop",
            ui: custom(backdrop_row),
            apply: |world, name| world.resource_mut::<Backdrops>().chosen = name.clone(),
        }
    }
}

#[derive(Resource, Default)]
pub struct Backdrops {
    folder: Option<PathBuf>,
    chosen: String,
    /// The chosen backdrop's source, as last read.
    source: Option<String>,
    /// What was last read, and when.
    read: Option<(String, Instant)>,
}

impl Backdrops {
    /// The `Backdrop` setting.
    pub fn chosen(&self) -> &str {
        &self.chosen
    }

    /// The chosen backdrop's shader file, unless it is none or a scene.
    fn file(&self) -> Option<&str> {
        Some(self.chosen.as_str()).filter(|name| !name.is_empty() && !name.starts_with("scene:"))
    }

    fn names(&self) -> Vec<String> {
        let Some(entries) = self.folder.as_ref().and_then(|f| std::fs::read_dir(f).ok()) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "wgsl"))
            .filter_map(|path| Some(path.file_stem()?.to_str()?.to_owned()))
            .collect();
        names.sort();
        names
    }
}

pub struct BackdropPlugin;
impl Plugin for BackdropPlugin {
    fn build(&self, app: &mut App) {
        let folder = app
            .world()
            .get_resource::<PersistenceDirectory>()
            .and_then(|directory| directory.join("backdrops"));
        if let Some(folder) = &folder {
            let result = std::fs::create_dir_all(folder).and_then(|()| {
                SHIPPED.iter().try_for_each(|(name, source)| {
                    let path = folder.join(format!("{name}.wgsl"));
                    if path.exists() {
                        return Ok(());
                    }
                    std::fs::write(path, source)
                })
            });
            app.world_mut().bedtime_mut().notify_failure(
                &format!("Couldn't write backdrops to {}", folder.display()),
                result,
            );
        }
        app.insert_resource(Backdrops {
            folder,
            ..default()
        })
        .register_setting::<Backdrop>()
        .add_systems(
            Update,
            (read, flyover::update, internals::update, show, report).chain(),
        );
    }
}

fn backdrop_row(
    ui: &mut bedtime::ui::egui::Ui,
    world: &mut World,
    context: &bedtime::ui::UiContext<'_>,
    layer: bedtime::workbench::persistence::StoreId,
) {
    let names = world.resource::<Backdrops>().names();
    row::<Backdrop>(ui, world, context, layer, None, |ui, chosen| {
        let before = chosen.clone();
        let scene = |name: &str| {
            scene::SCENES
                .iter()
                .find(|(scene, _)| *scene == name)
                .map(|(_, label)| *label)
        };
        let shown = match chosen.as_str() {
            "" => NONE,
            name => scene(name).unwrap_or(name),
        }
        .to_owned();
        combo(ui, "backdrop", shown, |ui| {
            ui.selectable_value(chosen, String::new(), NONE);
            for (name, label) in scene::SCENES {
                ui.selectable_value(chosen, (*name).into(), *label);
            }
            for name in names {
                let label = name.clone();
                ui.selectable_value(chosen, name, label);
            }
        });
        before != *chosen
    });
}

/// Read the chosen backdrop's file: when it is chosen, and then twice a
/// second, for edits.
fn read(world: &mut World) {
    let mut backdrops = world.resource_mut::<Backdrops>();
    let backdrops = backdrops.bypass_change_detection();
    if backdrops.file().is_none() {
        backdrops.read = None;
        if backdrops.source.take().is_some() {
            world.resource_mut::<Backdrops>().set_changed();
        }
        return;
    }
    if backdrops
        .read
        .as_ref()
        .is_some_and(|(name, at)| *name == backdrops.chosen && at.elapsed() < RESCAN)
    {
        return;
    }
    let first = backdrops
        .read
        .as_ref()
        .is_none_or(|(name, _)| *name != backdrops.chosen);
    backdrops.read = Some((backdrops.chosen.clone(), Instant::now()));
    let Some(path) = backdrops
        .folder
        .as_ref()
        .map(|folder| folder.join(format!("{}.wgsl", backdrops.chosen)))
    else {
        return;
    };
    let result = std::fs::read_to_string(&path);
    let source = result.as_ref().ok().cloned();
    if source == backdrops.source && !first {
        return;
    }
    backdrops.source = source;
    world.resource_mut::<Backdrops>().set_changed();
    world
        .bedtime_mut()
        .notify_failure(&format!("Couldn't read {}", path.display()), result);
}

/// Put the backdrop on every layout surface, new ones too.
fn show(
    backdrops: Res<Backdrops>,
    scene: Option<Res<scene::SceneImage>>,
    surfaces: Query<(Entity, Option<&SurfaceBackdrop>), With<LayoutSurface>>,
    mut commands: Commands,
) {
    let backdrop = match scene {
        Some(scene) => Some(SurfaceBackdrop::Target(scene.0.clone())),
        None => backdrops
            .source
            .as_ref()
            .map(|source| SurfaceBackdrop::Shader {
                source: source.clone(),
                frame_rate: FRAME_RATE,
            }),
    };
    for (surface, shown) in &surfaces {
        match &backdrop {
            Some(backdrop) if shown != Some(backdrop) => {
                commands.entity(surface).insert(backdrop.clone());
            }
            None if shown.is_some() => {
                commands.entity(surface).remove::<SurfaceBackdrop>();
            }
            _ => {}
        }
    }
}

/// Say why a backdrop didn't compile.
fn report(world: &mut World) {
    let failures: Vec<String> = world
        .query_filtered::<&BackdropStatus, Changed<BackdropStatus>>()
        .iter(world)
        .filter_map(|status| match status {
            BackdropStatus::Failed(error) => Some(error.clone()),
            _ => None,
        })
        .collect();
    let name = world.resource::<Backdrops>().chosen.clone();
    for error in failures {
        world.bedtime_mut().notify_failure(
            &format!("Couldn't compile the backdrop {name}"),
            Err::<(), _>(error),
        );
    }
}
