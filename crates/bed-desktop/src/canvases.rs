//! Shared canvas presentation. Feature plugins own source state/render
//! systems; this plugin owns targets, registration, icon uploads and submissions.
use crate::{DesktopFrame, DesktopGpu, DesktopSystems};
use bed_ui::{
    TextureHandle,
    gpu::{
        CanvasOutput, CanvasRenderInvocation, CanvasRenderResult, CanvasRenderSystem,
        CanvasRenderer, CanvasTextures, CanvasVisible, RenderTarget,
    },
};
use bevy::prelude::*;
use dear_imgui_wgpu::ExternalTextureId;
use std::collections::{HashMap, HashSet};

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DesktopCanvasSystems {
    Prepare,
    Render,
}

pub struct DesktopCanvasPlugin;

impl Plugin for DesktopCanvasPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CanvasTextures>()
            .init_resource::<DesktopCanvasTargets>()
            .init_resource::<DesktopCanvasErrors>()
            .add_systems(
                PreUpdate,
                prepare
                    .in_set(DesktopCanvasSystems::Prepare)
                    .after(DesktopSystems::Input)
                    .after(DesktopSystems::Prepare)
                    .before(DesktopSystems::Ui),
            )
            .add_systems(
                PreUpdate,
                render
                    .in_set(DesktopCanvasSystems::Render)
                    .after(DesktopSystems::AfterUi)
                    .before(DesktopSystems::RouteInput),
            );
    }
}

struct CanvasTexture {
    external: ExternalTextureId,
    target: RenderTarget,
}

struct IconTexture {
    source: bed_ui::icons::RgbaImage,
    _texture: wgpu::Texture,
    external: ExternalTextureId,
}

/// Target handles are readable for diagnostics; only DesktopCanvasPlugin changes them.
#[derive(Resource, Default)]
pub struct DesktopCanvasTargets {
    generation: u64,
    targets: HashMap<TextureHandle, CanvasTexture>,
    icons: HashMap<String, IconTexture>,
}

impl DesktopCanvasTargets {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn target(&self, handle: TextureHandle) -> Option<&RenderTarget> {
        self.targets.get(&handle).map(|texture| &texture.target)
    }
}

#[derive(Component, Clone, Debug)]
pub struct CanvasFailure(pub String);

#[derive(Debug)]
pub struct DesktopCanvasError {
    pub entity: Option<Entity>,
    pub message: String,
}

#[derive(Resource, Default)]
pub struct DesktopCanvasErrors(pub Vec<DesktopCanvasError>);

fn report(world: &mut World, entity: Option<Entity>, error: String) {
    world
        .resource_mut::<DesktopCanvasErrors>()
        .0
        .push(DesktopCanvasError {
            entity,
            message: error,
        });
}

fn outputs(world: &mut World) -> Vec<(Entity, bed_ui::gpu::RenderOutput)> {
    world
        .query::<(Entity, &CanvasOutput)>()
        .iter(world)
        .map(|(entity, output)| (entity, output.0))
        .collect()
}

fn sync(world: &mut World, gpu: &DesktopGpu, outputs: &[(Entity, bed_ui::gpu::RenderOutput)]) {
    for &(entity, output) in outputs {
        let current = world
            .resource::<DesktopCanvasTargets>()
            .targets
            .get(&output.handle);
        if current.is_some_and(|texture| {
            texture.target.size == output.size && texture.target.depth.is_some() == output.depth
        }) {
            continue;
        }
        let result = RenderTarget::new(&gpu.device, output).and_then(|target| {
            let external = if let Some(texture) = current {
                gpu.update_external_texture(texture.external, &target.view)?;
                texture.external
            } else {
                gpu.register_external_texture(&target.view)?
            };
            Ok(CanvasTexture { external, target })
        });
        match result {
            Ok(texture) => {
                world
                    .resource_mut::<CanvasTextures>()
                    .0
                    .insert(output.handle, texture.external.texture_id());
                world
                    .resource_mut::<DesktopCanvasTargets>()
                    .targets
                    .insert(output.handle, texture);
            }
            Err(error) => report(world, Some(entity), error),
        }
    }
}

fn prepare(world: &mut World) {
    let Some(gpu) = world.get_resource::<DesktopGpu>().cloned() else {
        return;
    };
    let replaced = world.resource::<DesktopCanvasTargets>().generation != gpu.generation;
    if replaced {
        *world.resource_mut::<DesktopCanvasTargets>() = DesktopCanvasTargets {
            generation: gpu.generation,
            ..default()
        };
        world.resource_mut::<CanvasTextures>().0.clear();
        if let Some(mut icons) = world.get_non_send_mut::<bed_ui::icons::Icons>() {
            icons.textures.clear();
        }
    }
    let outputs = outputs(world);
    let live: HashSet<_> = outputs.iter().map(|(_, output)| output.handle).collect();
    let retired: Vec<_> = world
        .resource::<DesktopCanvasTargets>()
        .targets
        .keys()
        .filter(|handle| !live.contains(handle))
        .copied()
        .collect();
    // Retirement runs before a new snapshot. The previous one was consumed or
    // reconciled by the serial render app; never remove an ID after UI drawing.
    for handle in retired {
        let texture = world
            .resource_mut::<DesktopCanvasTargets>()
            .targets
            .remove(&handle)
            .unwrap();
        if let Err(error) = gpu.unregister_external_texture(texture.external) {
            report(world, None, error);
        }
        world.resource_mut::<CanvasTextures>().0.remove(&handle);
    }
    sync(world, &gpu, &outputs);
    upload_icons(world, &gpu);
}

fn upload_icons(world: &mut World, gpu: &DesktopGpu) {
    if world.get_non_send::<bed_ui::icons::Icons>().is_none() {
        return;
    }
    let mut state = bevy::ecs::system::SystemState::<(
        NonSendMut<bed_ui::icons::Icons>,
        ResMut<DesktopCanvasTargets>,
        ResMut<DesktopCanvasErrors>,
    )>::new(world);
    let (mut icons, mut cache, mut errors) = state
        .get_mut(world)
        .expect("canonical canvas/icon resources installed");
    let bed_ui::icons::Icons { images, textures } = &mut *icons;
    let retired: Vec<_> = cache
        .icons
        .keys()
        .filter(|key| !images.contains_key(*key))
        .cloned()
        .collect();
    for key in retired {
        let icon = cache.icons.remove(&key).unwrap();
        if let Err(message) = gpu.unregister_external_texture(icon.external) {
            errors.0.push(DesktopCanvasError {
                entity: None,
                message,
            });
        }
        textures.remove(&key);
    }
    for (key, image) in images.iter() {
        let cached = cache.icons.get(key);
        if cached.is_some_and(|cached| cached.source == *image) {
            continue;
        }
        let result = gpu
            .upload_rgba([image.width, image.height], &image.pixels, false)
            .and_then(|texture| {
                let view = texture.create_view(&Default::default());
                let external = if let Some(cached) = cached {
                    gpu.update_external_texture(cached.external, &view)?;
                    cached.external
                } else {
                    gpu.register_external_texture(&view)?
                };
                Ok(IconTexture {
                    source: image.clone(),
                    _texture: texture,
                    external,
                })
            });
        match result {
            Ok(icon) => {
                textures.insert(key.clone(), icon.external.texture_id());
                cache.icons.insert(key.clone(), icon);
            }
            Err(message) => errors.0.push(DesktopCanvasError {
                entity: None,
                message,
            }),
        }
    }
}

fn render(world: &mut World) {
    let Some(gpu) = world.get_resource::<DesktopGpu>().cloned() else {
        return;
    };
    let outputs = outputs(world);
    sync(world, &gpu, &outputs);
    let renderers: Vec<_> = world
        .query::<(Entity, &CanvasOutput, &CanvasRenderer, &CanvasVisible)>()
        .iter(world)
        .filter(|(_, _, _, visible)| visible.0)
        .map(|(entity, output, renderer, _)| (entity, output.0.handle, renderer.0))
        .collect();
    for (entity, handle, renderer) in renderers {
        let Some(target) = world
            .resource::<DesktopCanvasTargets>()
            .target(handle)
            .cloned()
        else {
            continue;
        };
        let result = run_canvas_system(
            world,
            renderer,
            CanvasRenderInvocation {
                entity,
                instance: gpu.instance.clone(),
                adapter: gpu.adapter.clone(),
                device: gpu.device.clone(),
                queue: gpu.queue.clone(),
                target,
                generation: gpu.generation,
            },
        );
        match result {
            Ok(commands) => world
                .resource_mut::<DesktopFrame>()
                .canvases
                .extend(commands),
            Err(error) => report(world, Some(entity), error),
        }
    }
}

/// A panicking renderer is quarantined on its presentation entity. Its command
/// batch is dropped; another panel's registered renderer remains callable.
fn run_canvas_system(
    world: &mut World,
    renderer: CanvasRenderSystem,
    invocation: CanvasRenderInvocation,
) -> CanvasRenderResult {
    let entity = invocation.entity;
    if let Some(failure) = world.get::<CanvasFailure>(entity) {
        return Err(failure.0.clone());
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        world.run_system_with(renderer, invocation)
    }));
    match result {
        Ok(result) => result
            .map_err(|error| error.to_string())
            .and_then(|result| result),
        Err(payload) => {
            let detail = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("non-text panic payload");
            let error = format!("Canvas panel {entity:?} panicked during rendering: {detail}");
            if let Ok(mut panel) = world.get_entity_mut(entity) {
                panel.insert(CanvasFailure(error.clone()));
            }
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    #[ignore = "requires a native GPU adapter; run with --ignored on desktop CI"]
    fn registered_canvas_panic_discards_its_gpu_batch_and_preserves_sibling_rendering() {
        use bed_ui::gpu::{
            CanvasRenderInvocation, CanvasRenderResult, RenderOutput, renderer_device_descriptor,
        };

        #[derive(Resource, Default)]
        struct Calls {
            failed: usize,
            healthy: usize,
        }

        fn record_clear(
            invocation: &CanvasRenderInvocation,
            colour: wgpu::Color,
        ) -> wgpu::CommandBuffer {
            let mut encoder = invocation
                .device
                .create_command_encoder(&Default::default());
            {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &invocation.target.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(colour),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
            }
            encoder.finish()
        }
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = bevy::tasks::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) =
            bevy::tasks::block_on(adapter.request_device(&renderer_device_descriptor(&adapter)))
                .unwrap();
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut world = World::new();
        world.init_resource::<Calls>();
        world.init_resource::<DesktopFrame>();
        let failed = world.spawn_empty().id();
        let healthy = world.spawn_empty().id();
        let failed_system = world.register_system(
            |In(invocation): In<CanvasRenderInvocation>,
             mut calls: ResMut<Calls>|
             -> CanvasRenderResult {
                calls.failed += 1;
                let _unsubmitted = record_clear(&invocation, wgpu::Color::RED);
                panic!("canvas failed after recording commands");
            },
        );
        let healthy_system = world.register_system(
            |In(invocation): In<CanvasRenderInvocation>,
             mut calls: ResMut<Calls>|
             -> CanvasRenderResult {
                calls.healthy += 1;
                Ok(vec![record_clear(&invocation, wgpu::Color::GREEN)])
            },
        );
        let target = |handle| {
            RenderTarget::new(
                &device,
                RenderOutput {
                    handle: bed_ui::TextureHandle(handle),
                    size: [1, 1],
                    depth: false,
                    revision: 0,
                },
            )
            .unwrap()
        };
        let failed_target = target(1);
        let healthy_target = target(2);
        let invocation = |entity, target: &RenderTarget| CanvasRenderInvocation {
            entity,
            instance: instance.clone(),
            adapter: adapter.clone(),
            device: device.clone(),
            queue: queue.clone(),
            target: target.clone(),
            generation: 1,
        };
        let error = run_canvas_system(
            &mut world,
            failed_system,
            invocation(failed, &failed_target),
        )
        .unwrap_err();
        assert!(error.contains("canvas failed after recording commands"));
        assert_eq!(
            run_canvas_system(
                &mut world,
                failed_system,
                invocation(failed, &failed_target)
            )
            .unwrap_err(),
            error
        );
        let commands = run_canvas_system(
            &mut world,
            healthy_system,
            invocation(healthy, &healthy_target),
        )
        .unwrap();
        world
            .resource_mut::<DesktopFrame>()
            .canvases
            .extend(commands);
        assert_eq!(
            world.resource::<Calls>().failed,
            1,
            "the damaged renderer is quarantined"
        );
        assert_eq!(world.resource::<Calls>().healthy, 1);
        assert_eq!(world.resource::<DesktopFrame>().canvases.len(), 1);
        queue.submit(std::mem::take(
            &mut world.resource_mut::<DesktopFrame>().canvases,
        ));

        // The failed batch never paints its target; the healthy batch reaches the
        // same queue and produces actual pixels after the sibling's panic.
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Canvas quarantine readback"),
            size: 512,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        for (index, target) in [&failed_target, &healthy_target].into_iter().enumerate() {
            encoder.copy_texture_to_buffer(
                target.texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: (index * 256) as u64,
                        bytes_per_row: Some(256),
                        rows_per_image: Some(1),
                    },
                },
                target.texture.size(),
            );
        }
        queue.submit([encoder.finish()]);
        let (sender, receiver) = std::sync::mpsc::channel();
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                sender.send(result).unwrap()
            });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(10)),
            })
            .unwrap();
        receiver
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        let bytes = readback.slice(..).get_mapped_range().unwrap();
        assert_eq!(&bytes[..4], &[0, 0, 0, 0]);
        assert_eq!(&bytes[256..260], &[0, 255, 0, 255]);
        drop(bytes);
        readback.unmap();
        assert!(bevy::tasks::block_on(validation.pop()).is_none());
    }
}
