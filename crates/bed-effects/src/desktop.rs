//! Editor-specific effects composition in Bevy's desktop camera schedule.
//! The reusable desktop plugin owns the native window and presentation.
use crate::shader_manager::{ShaderManager, ShaderSettings};

use bevy::{
    prelude::*,
    render::{
        RenderApp,
        renderer::{PendingCommandBuffers, RenderDevice, RenderQueue},
    },
};
use dear_imgui_rs::render::FrameSnapshot;
use dear_imgui_wgpu::{FramebufferExtent, WgpuSnapshotRenderer};

/// Editor-only composition policy. The reusable desktop crate knows nothing
/// about CRT effects, scene history or editor settings.
#[derive(Resource, Clone, bevy::render::extract_resource::ExtractResource)]
#[extract_app(RenderApp)]
pub struct EffectsState {
    pub settings: ShaderSettings,
    /// Wall-clock seconds, independent of gameplay pausing or time scaling.
    pub time: f32,
    pub scene_generation: u64,
}

impl Default for EffectsState {
    fn default() -> Self {
        Self {
            settings: ShaderSettings::default(),
            time: 0.0,
            scene_generation: 0,
        }
    }
}

#[derive(Resource)]
struct EditorEffectsRenderer {
    effects: ShaderManager,
    size: [u32; 2],
    output_format: wgpu::TextureFormat,
    scene_generation: u64,
}

pub fn install(app: &mut App) {
    app.init_resource::<EffectsState>()
        .insert_resource(bed_desktop::DesktopComposition(compose_editor))
        .add_systems(
            Update,
            |time: Res<Time<Real>>, mut effects: ResMut<EffectsState>| {
                effects.time = time.elapsed_secs();
            },
        )
        .add_plugins(bevy::render::extract_resource::ExtractResourcePlugin::<
            EffectsState,
        >::default());
    app.sub_app_mut(RenderApp)
        .add_systems(bevy::render::RenderStartup, reset_editor_effects);
}

fn reset_editor_effects(world: &mut World) {
    world.remove_resource::<EditorEffectsRenderer>();
}

fn compose_editor(
    world: &mut World,
    imgui: &mut WgpuSnapshotRenderer,
    snapshot: FrameSnapshot,
    output: &wgpu::TextureView,
    output_format: wgpu::TextureFormat,
    size: [u32; 2],
) -> Result<(), String> {
    let device = world.resource::<RenderDevice>().wgpu_device().clone();
    let queue = world.resource::<RenderQueue>().clone();
    let policy = world.resource::<EffectsState>().clone();
    if !world
        .get_resource::<EditorEffectsRenderer>()
        .is_some_and(|renderer| renderer.size == size && renderer.output_format == output_format)
    {
        world.insert_resource(EditorEffectsRenderer {
            effects: ShaderManager::new(&device, size[0], size[1], output_format),
            size,
            output_format,
            scene_generation: policy.scene_generation,
        });
    }
    let mut renderer = world.resource_mut::<EditorEffectsRenderer>();
    if renderer.scene_generation != policy.scene_generation {
        renderer.effects.invalidate_history();
        renderer.scene_generation = policy.scene_generation;
    }
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("Bed editor composition"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Bed editor ImGui"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &renderer.effects.fb.view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        imgui
            .render_snapshot(
                snapshot,
                &mut pass,
                FramebufferExtent::from_texture(&renderer.effects.fb.texture),
            )
            .map_err(|error| error.to_string())?;
    }
    renderer.effects.render_with_effects(
        &mut encoder,
        &queue,
        output,
        policy.time,
        &policy.settings,
    );
    world
        .resource_mut::<PendingCommandBuffers>()
        .push([encoder.finish()]);
    Ok(())
}

/// Final desktop composition with the editor visual effects.
pub struct EffectsPlugin;
impl Plugin for EffectsPlugin {
    fn build(&self, app: &mut App) {
        install(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{
        ecs::schedule::{ScheduleLabel, SystemSet},
        extract::ExtractPlugin,
        render::{Render, RenderSystems},
        time::{TimePlugin, TimeUpdateStrategy},
    };
    use std::time::Duration;

    #[test]
    fn desktop_effects_extract_current_real_time_even_when_gameplay_is_paused() {
        let mut app = App::new();
        app.add_plugins(TimePlugin)
            .add_plugins(ExtractPlugin::<RenderApp>::new(
                |_, _| {},
                Render::base_schedule,
                Render.intern(),
                RenderSystems::ExtractCommands.intern(),
                RenderSystems::PostCleanup.intern(),
            ))
            .add_plugins(EffectsPlugin);
        app.sub_app_mut(RenderApp).update_schedule = Some(Render.intern());
        app.world_mut().resource_mut::<Time<Virtual>>().pause();
        app.update(); // The real clock starts at zero on its first update.

        let mut elapsed = Duration::ZERO;
        for (seconds, enabled) in [(1.0 / 60.0, true), (0.04, false), (0.01, true)] {
            let delta = Duration::from_secs_f64(seconds);
            elapsed += delta;
            app.insert_resource(TimeUpdateStrategy::ManualDuration(delta));
            app.world_mut()
                .resource_mut::<EffectsState>()
                .settings
                .enabled = enabled;
            app.update();

            let rendered = app.sub_app(RenderApp).world().resource::<EffectsState>();
            assert_eq!(rendered.time, elapsed.as_secs_f32());
            assert_eq!(rendered.settings.enabled, enabled);
            assert_eq!(app.world().resource::<Time>().elapsed(), Duration::ZERO);
        }
    }
}
