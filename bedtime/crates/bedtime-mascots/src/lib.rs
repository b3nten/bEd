//! Bed and Ducky in the host's Bevy world, as workbench panels.
mod audio;
mod scene;
mod workbench;
pub use workbench::{BedPanel, DuckPanel, ShowBed};

use bedtime_ui::{UiComponent, UiContext, UiSystems, UiViewport, egui, viewport_image};
use bevy::{
    camera::visibility::RenderLayers,
    ecs::{lifecycle::HookContext, world::DeferredWorld},
    prelude::*,
};
use scene::Scene;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MascotKind {
    Bed,
    Duck,
}

/// Per-instance interaction and animation state. Spawn before drawing; despawn to close.
#[derive(Component)]
#[component(on_remove = remove_scene)]
#[require(Transform, Visibility)]
pub struct MascotScene {
    kind: MascotKind,
    pointer: [f32; 2],
    accent: [f32; 4],
    animations: bool,
    pressed: bool,
    time: f32,
    impulse: f32,
    elapsed: f32,
    failure: Option<String>,
    sound: Option<rodio::Sink>,
}
impl MascotScene {
    pub fn new(kind: MascotKind) -> Self {
        Self {
            kind,
            pointer: [0.0; 2],
            accent: [0.4, 0.65, 1.0, 1.0],
            animations: true,
            pressed: false,
            time: 0.0,
            impulse: 0.0,
            elapsed: 0.0,
            failure: None,
            sound: None,
        }
    }
    pub fn kind(&self) -> MascotKind {
        self.kind
    }
    pub fn error(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    fn advance(&mut self, visible: bool, dt: f32) -> bool {
        let pressed = std::mem::take(&mut self.pressed) && visible;
        if !visible {
            self.elapsed = 0.0;
            return false;
        }
        if !self.animations {
            self.pointer = [0.0; 2];
            self.impulse = 0.0;
            self.time = 0.0;
            self.elapsed = 0.0;
        } else {
            self.elapsed += dt.clamp(0.0, 0.1);
            if self.elapsed >= 1.0 / 30.0 {
                let elapsed = std::mem::take(&mut self.elapsed).min(0.1);
                self.time += elapsed;
                self.impulse = (self.impulse - elapsed * 3.0).max(0.0);
            }
            if pressed {
                self.impulse = 1.0;
            }
        }
        pressed
    }
}

fn remove_scene(mut world: DeferredWorld, context: HookContext) {
    let root = world.get::<Scene>(context.entity).map(|scene| scene.root);
    world.commands().queue(move |world: &mut World| {
        if let Some(root) = root.filter(|root| world.entities().contains(*root)) {
            world.despawn(root);
        }
        if let Ok(mut entity) = world.get_entity_mut(context.entity) {
            entity.remove::<(Scene, UiViewport)>();
        }
    });
}

/// Installs scheduled scene preparation and animation, without creating a window or view.
pub struct MascotPlugin;
impl Plugin for MascotPlugin {
    fn build(&self, app: &mut App) {
        use bedtime_workbench::{ActionAppExt, MenusAppExt};
        app.register_type::<BedPanel>()
            .register_type::<DuckPanel>()
            .add_action_handler::<ShowBed>(bedtime_workbench::Handler::new(
                bedtime_workbench::show_panel::<BedPanel>,
            ))
            .add_menu_item::<ShowBed>(bedtime_workbench::StandardMenu::View, "tools")
            .init_non_send::<audio::AudioOutput>()
            .add_systems(Update, prepare_scenes)
            .add_systems(
                PostUpdate,
                update_scenes
                    .in_set(UiSystems::SyncViewports)
                    .before(bevy::transform::TransformSystems::Propagate),
            );
    }
}

/// Embedded drawing adapter. Props is the entity carrying `MascotScene`.
pub struct Mascot;
impl UiComponent for Mascot {
    type Props<'a> = Entity;
    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, entity: Entity) {
        let state = world
            .get::<MascotScene>(entity)
            .expect("Mascot drawing requires MascotScene");
        if let Some(error) = state.error() {
            ui.colored_label(ui.visuals().error_fg_color, error);
            return;
        }
        if world.get::<Scene>(entity).is_none() {
            ui.spinner();
            ui.weak("Preparing mascot…");
            ui.ctx().request_repaint();
            return;
        }
        let response = UiViewport::show(ui, world, entity);
        let eligible = ui.input(|i| i.focused)
            && !ui.ctx().any_popup_open()
            && ui.memory(|m| m.top_modal_layer().is_none());
        let pointer = if eligible {
            ui.input(|i| i.pointer.hover_pos()).map_or([0.0; 2], |p| {
                [
                    ((p.x - response.rect.min.x) / response.rect.width() * 2.0 - 1.0)
                        .clamp(-1.0, 1.0),
                    ((p.y - response.rect.min.y) / response.rect.height() * 2.0 - 1.0)
                        .clamp(-1.0, 1.0),
                ]
            })
        } else {
            [0.0; 2]
        };
        let pressed = eligible
            && response.hovered()
            && ui.input(|i| {
                i.pointer.primary_pressed()
                    && i.pointer
                        .hover_pos()
                        .is_some_and(|p| response.rect.contains(p))
            });
        let mut state = world.get_mut::<MascotScene>(entity).unwrap();
        state.pointer = pointer;
        state.accent = context.theme.palette.accent;
        state.animations = context.motion.enabled;
        state.pressed |= pressed;
        if context.motion.enabled && ui.is_rect_visible(response.rect) {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f32(1.0 / 30.0));
        }
    }
}

fn prepare_scenes(world: &mut World) {
    if !world.contains_resource::<Assets<Image>>()
        || !world.contains_resource::<Assets<Mesh>>()
        || !world.contains_resource::<Assets<StandardMaterial>>()
    {
        return;
    }
    let pending: Vec<_> = world
        .query_filtered::<(Entity, &MascotScene), Without<Scene>>()
        .iter(world)
        .filter(|(_, state)| state.failure.is_none())
        .map(|(e, s)| (e, s.kind))
        .collect();
    for (entity, kind) in pending {
        // Choose a free non-default layer from the actual world. No allocation table
        // can retain a layer after its linked scene entities have been retired.
        let used: Vec<_> = world
            .query::<&RenderLayers>()
            .iter(world)
            .cloned()
            .collect();
        let layer = (1..)
            .find(|&layer| {
                used.iter()
                    .all(|layers| !layers.intersects(&RenderLayers::layer(layer)))
            })
            .unwrap();
        let image = world
            .resource_mut::<Assets<Image>>()
            .add(viewport_image(UVec2::ONE));
        match Scene::new(
            world,
            entity,
            kind,
            image.clone(),
            RenderLayers::layer(layer),
        ) {
            Ok(scene) => {
                world
                    .entity_mut(entity)
                    .insert((scene, UiViewport::new(image)));
            }
            Err(error) => {
                world.get_mut::<MascotScene>(entity).unwrap().failure = Some(error);
            }
        }
    }
}

fn update_scenes(world: &mut World) {
    let dt = world
        .get_resource::<Time>()
        .map_or(0.0, |time| time.delta_secs());
    let entities: Vec<_> = world
        .query_filtered::<Entity, (With<MascotScene>, With<Scene>)>()
        .iter(world)
        .collect();
    for entity in entities {
        let viewport = world.get::<UiViewport>(entity).unwrap();
        let (visible, size) = (viewport.visible, viewport.physical_size);
        let mut state = world.get_mut::<MascotScene>(entity).unwrap();
        let pressed = state.advance(visible, dt);
        let (kind, pointer, accent, time, impulse) = (
            state.kind,
            state.pointer,
            state.accent,
            state.time,
            state.impulse,
        );
        if pressed {
            audio::play(world, entity, kind);
        }
        let scene = world.get::<Scene>(entity).unwrap().clone();
        world.get_mut::<Camera>(scene.camera).unwrap().is_active = visible;
        if visible {
            scene.sync(world, size, pointer, accent, time, impulse);
        }
    }
}

#[cfg(test)]
mod tests;
