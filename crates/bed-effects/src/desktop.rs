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
