//! Native render test support; production composition belongs to bedtime-plugin-effects.
use crate::desktop::{EffectsState, install};
use crate::shader_manager::ShaderSettings;
use bedtime_desktop::bevy;
use bevy::{
    prelude::*,
    render::{
        RenderApp,
        renderer::{PendingCommandBuffers, RenderDevice, RenderQueue},
    },
};
use dear_imgui_rs::render::FrameSnapshot;
use dear_imgui_wgpu::WgpuSnapshotRenderer;

#[cfg(test)]
mod tests {
    use bevy::render::renderer::{RenderGraph, RenderGraphSystems};
    #[derive(Resource, Default)]

    pub(crate) struct RenderRecovery {
        pub error: Option<(bool, String)>,
        pub replacement: Option<bevy::render::settings::RenderCreation>,
        reported: bool,
    }

    pub(crate) fn install_error_handler(app: &mut App) {
        use bevy::render::error_handler::{ErrorType, RenderErrorHandler, RenderErrorPolicy};
        app.init_resource::<RenderRecovery>()
            .insert_resource(RenderErrorHandler(|error, world, _| {
                let mut recovery = world.resource_mut::<RenderRecovery>();
                if let Some(replacement) = recovery.replacement.take() {
                    recovery.reported = false;
                    return RenderErrorPolicy::Recover(replacement);
                }
                if !recovery.reported {
                    recovery.error =
                        Some((error.ty == ErrorType::DeviceLost, error.description.clone()));
                    recovery.reported = true;
                }
                RenderErrorPolicy::StopRendering
            }));
    }

    /// Run the renderer's device lifecycle before host GPU allocations, without
    /// advancing simulation, input, or extracting the main world. The host uses
    /// synchronous rendering and calls this only after plugins finish/cleanup.
    pub(crate) fn update_device_lifecycle(app: &mut App) {
        use bevy::{
            app::{AppLabel, SubApps},
            extract::ExtractPlugin,
        };
        let lifecycle = app
            .get_added_plugins::<ExtractPlugin<RenderApp>>()
            .first()
            .expect("Bevy renderer installed")
            .pre_extract;
        let SubApps { main, sub_apps } = app.sub_apps_mut();
        let render = sub_apps
            .get_mut(&RenderApp.intern())
            .expect("Bevy render app installed");
        lifecycle(main.world_mut(), render.world_mut());
    }

    pub(crate) fn manual_resources(
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> bevy::render::settings::RenderResources {
        use bevy::render::{
            renderer::{RenderAdapter, RenderAdapterInfo, RenderInstance},
            settings::RenderResources,
        };
        RenderResources(
            RenderDevice::from(device.clone()),
            RenderQueue::new(queue.clone()),
            RenderAdapterInfo::new(adapter.get_info()),
            RenderAdapter::new(adapter.clone()),
            RenderInstance::new(instance.clone()),
        )
    }

    #[derive(Resource)]
    pub(crate) struct AppRenderer {
        pub imgui: WgpuSnapshotRenderer,
    }

    #[derive(Resource)]
    pub(crate) struct AppRenderFrame {
        pub snapshot: Option<FrameSnapshot>,
        pub output: Option<wgpu::TextureView>,
        pub size: [u32; 2],
        pub time: f32,
        pub settings: ShaderSettings,
        pub scene_generation: u64,
        pub canvases: Vec<wgpu::CommandBuffer>,
        pub error: Option<String>,
    }

    #[cfg(test)]
    fn install_manual(
        app: &mut App,
        imgui: WgpuSnapshotRenderer,
        size: [u32; 2],
        settings: ShaderSettings,
    ) {
        let composition = app
            .world()
            .resource::<bedtime_desktop::DesktopComposition>()
            .0;
        replace(app, imgui, size, settings);
        app.sub_app_mut(RenderApp)
            .insert_resource(bedtime_desktop::DesktopComposition(composition));
        app.sub_app_mut(RenderApp).add_systems(
            RenderGraph,
            render_editor
                // Bevy 0.20 uses weak ordering between its built-in sets.
                // Texture producers must finish before ImGui samples them.
                .after(RenderGraphSystems::Render)
                .before(RenderGraphSystems::Submit),
        );
    }

    pub(crate) fn replace(
        app: &mut App,
        imgui: WgpuSnapshotRenderer,
        size: [u32; 2],
        settings: ShaderSettings,
    ) {
        app.sub_app_mut(RenderApp)
            .insert_resource(AppRenderer { imgui })
            .insert_resource(AppRenderFrame {
                snapshot: None,
                output: None,
                size,
                time: 0.0,
                settings,
                scene_generation: 0,
                canvases: Vec::new(),
                error: None,
            });
    }

    fn render_editor(world: &mut World) {
        let (snapshot, output, size, policy, canvases) = {
            let mut frame = world.resource_mut::<AppRenderFrame>();
            let policy = EffectsState {
                settings: frame.settings,
                time: frame.time,
                scene_generation: frame.scene_generation,
            };
            (
                frame.snapshot.take(),
                frame.output.take(),
                frame.size,
                policy,
                std::mem::take(&mut frame.canvases),
            )
        };
        world.resource_mut::<PendingCommandBuffers>().push(canvases);
        let Some(snapshot) = snapshot else {
            return;
        };
        world.insert_resource(policy);
        let mut renderer = world.remove_resource::<AppRenderer>().unwrap();
        let result = if let Some(output) = output {
            let composition = world.resource::<bedtime_desktop::DesktopComposition>().0;
            composition(
                world,
                &mut renderer.imgui,
                snapshot,
                &output,
                output.texture().format(),
                size,
            )
        } else {
            renderer
                .imgui
                .reconcile_snapshot(snapshot)
                .map_err(|error| error.to_string())
        };
        world.insert_resource(renderer);
        if let Err(error) = result {
            world.resource_mut::<AppRenderFrame>().error = Some(error);
        }
    }

    pub(crate) fn renderer(app: &mut App) -> Mut<'_, AppRenderer> {
        app.sub_app_mut(RenderApp)
            .world_mut()
            .resource_mut::<AppRenderer>()
    }

    pub(crate) fn frame(app: &mut App) -> Mut<'_, AppRenderFrame> {
        app.sub_app_mut(RenderApp)
            .world_mut()
            .resource_mut::<AppRenderFrame>()
    }

    use super::*;
    use bedtime_desktop::preview::PreviewRenderPlugin;
    use bedtime_ui::{
        TextureHandle,
        gpu::{RenderOutput, RenderTarget, renderer_device_descriptor},
    };
    use bevy::{
        camera::{ClearColorConfig, ManualTextureViewHandle, RenderTarget as CameraTarget},
        render::{
            RenderStartup,
            texture::{ManualTextureView, ManualTextureViews},
            view::Tonemapping,
        },
    };
    use dear_imgui_rs::{
        Context,
        texture::{OwnedTextureData, TextureFormat},
    };
    use dear_imgui_wgpu::{ExternalTextureId, WgpuInitInfo, WgpuSnapshotContext};
    use std::time::Duration;

    const OUTPUT_SIZE: [u32; 2] = [128, 64];
    const PANELS: [ManualTextureViewHandle; 2] =
        [ManualTextureViewHandle(101), ManualTextureViewHandle(102)];

    #[derive(Component)]
    struct GameEntity;

    #[derive(Resource, Default)]
    struct RenderStarts(u32);

    fn targets(device: &wgpu::Device, app: &mut App) -> ([RenderTarget; 2], wgpu::Texture) {
        let panels = std::array::from_fn(|index| {
            let target = RenderTarget::new(
                device,
                RenderOutput {
                    handle: TextureHandle::next(),
                    size: [32, 32],
                    depth: false,
                    revision: 1,
                },
            )
            .unwrap();
            app.world_mut().resource_mut::<ManualTextureViews>().insert(
                PANELS[index],
                ManualTextureView {
                    texture_view: target
                        .texture
                        .create_view(&wgpu::TextureViewDescriptor {
                            format: Some(wgpu::TextureFormat::Rgba8UnormSrgb),
                            ..Default::default()
                        })
                        .into(),
                    view_format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    size: UVec2::new(32, 32),
                },
            );
            target
        });
        let output = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Unified editor composition test output"),
            size: wgpu::Extent3d {
                width: OUTPUT_SIZE[0],
                height: OUTPUT_SIZE[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        (panels, output)
    }

    fn snapshot(
        context: &mut Context,
        owner: &mut WgpuSnapshotContext,
        ids: [ExternalTextureId; 2],
    ) -> FrameSnapshot {
        owner.poll_completions(context).unwrap();
        context
            .io_mut()
            .set_display_size(OUTPUT_SIZE.map(|size| size as f32));
        context.io_mut().set_delta_time(1.0 / 60.0);
        {
            let ui = context.frame();
            let draw = ui.get_foreground_draw_list();
            for (index, id) in ids.into_iter().enumerate() {
                let x = index as f32 * 64.0;
                draw.add_image(
                    id.texture_id(),
                    [x, 0.0],
                    [x + 64.0, 64.0],
                    [0.0; 2],
                    [1.0; 2],
                    [1.0; 4],
                );
            }
        }
        owner.render_snapshot(context).unwrap()
    }

    fn draw_frame(app: &mut App, snapshot: FrameSnapshot, output: Option<&wgpu::Texture>) {
        {
            let mut frame = frame(app);
            frame.snapshot = Some(snapshot);
            frame.output = output.map(|output| output.create_view(&Default::default()));
            frame.error = None;
        }
        app.update();
        assert!(
            frame(app).error.is_none(),
            "composition failed: {:?}",
            frame(app).error
        );
    }

    fn pixels(device: &wgpu::Device, queue: &wgpu::Queue, output: &wgpu::Texture) -> Vec<u8> {
        let stride = OUTPUT_SIZE[0] * 4;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Unified composition readback"),
            size: u64::from(stride) * u64::from(OUTPUT_SIZE[1]),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            output.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(OUTPUT_SIZE[1]),
                },
            },
            output.size(),
        );
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
        let mapped = readback
            .slice(..)
            .get_mapped_range()
            .expect("output is mapped");
        let result = mapped.to_vec();
        drop(mapped);
        readback.unmap();
        result
    }

    fn assert_colours(pixels: &[u8], expected: [[u8; 3]; 2]) {
        for (index, colour) in expected.into_iter().enumerate() {
            let x = index * 64 + 32;
            let offset = (32 * OUTPUT_SIZE[0] as usize + x) * 4;
            let actual = &pixels[offset..offset + 4];
            for channel in 0..3 {
                assert!(
                    actual[channel].abs_diff(colour[channel]) <= 2,
                    "panel {index} colour/gamma changed: expected {colour:?}, got {actual:?}"
                );
            }
            assert_eq!(actual[3], 255);
        }
    }

    #[test]
    #[ignore = "requires a native GPU adapter; run with --ignored on desktop CI"]
    fn unified_composition_samples_current_scenes_and_recovers_in_the_same_world() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = bevy::tasks::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) =
            bevy::tasks::block_on(adapter.request_device(&renderer_device_descriptor(&adapter)))
                .unwrap();
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut app = App::new();
        app.add_plugins((bevy::MinimalPlugins, bevy::input::InputPlugin))
            .add_plugins(PreviewRenderPlugin {
                resources: manual_resources(&instance, &adapter, &device, &queue),
            });
        install(&mut app);
        install_error_handler(&mut app);
        app.sub_app_mut(RenderApp)
            .init_resource::<RenderStarts>()
            .add_systems(RenderStartup, |mut starts: ResMut<RenderStarts>| {
                starts.0 += 1
            });
        app.finish();
        app.cleanup();
        let game_entity = app.world_mut().spawn(GameEntity).id();
        let (panels, output) = targets(&device, &mut app);
        let cameras = PANELS.map(|target| {
            app.world_mut()
                .spawn((
                    Camera3d::default(),
                    CameraTarget::TextureView(target),
                    Camera {
                        clear_color: ClearColorConfig::Custom(Color::BLACK),
                        ..Default::default()
                    },
                    Msaa::Off,
                    Tonemapping::None,
                    Transform::default(),
                ))
                .id()
        });
        let mut context = Context::create();
        let (imgui, mut owner) = WgpuSnapshotRenderer::new(
            WgpuInitInfo::new(
                device.clone(),
                queue.clone(),
                bedtime_ui::gpu::OUTPUT_FORMAT,
            ),
            &mut context,
        )
        .unwrap();
        let settings = ShaderSettings::default();
        install_manual(&mut app, imgui, OUTPUT_SIZE, settings);
        let ids = panels.each_ref().map(|panel| {
            renderer(&mut app)
                .imgui
                .register_external_texture(&panel.view)
                .unwrap()
        });
        eprintln!("composition fixture: initial camera warm-up");
        // Warm the camera pipelines once. Subsequent changed colours must reach ImGui in the
        // same frame, which detects accidentally weak producer/compositor ordering.
        for _ in 0..4 {
            draw_frame(
                &mut app,
                snapshot(&mut context, &mut owner, ids),
                Some(&output),
            );
        }
        for colours in [
            [[64, 128, 192], [192, 96, 48]],
            [[96, 48, 160], [48, 160, 96]],
        ] {
            for (camera, colour) in cameras.into_iter().zip(colours) {
                app.world_mut()
                    .get_mut::<Camera>(camera)
                    .unwrap()
                    .clear_color =
                    ClearColorConfig::Custom(Color::srgb_u8(colour[0], colour[1], colour[2]));
            }
            draw_frame(
                &mut app,
                snapshot(&mut context, &mut owner, ids),
                Some(&output),
            );
            assert_colours(&pixels(&device, &queue, &output), colours);
        }
        {
            let mut frame = frame(&mut app);
            frame.settings = ShaderSettings {
                enabled: true,
                pulse_intensity: 0.5,
                scanline_intensity: 0.0,
                vignet_intensity: 0.0,
                bloom_intensity: 0.0,
                static_intensity: 0.0,
                colorshift_intensity: 0.0,
                jitter_intensity: 0.0,
                curvature_intensity: 0.0,
                pixelation_intensity: 0.0,
                pixel_width: 1.0,
                burnin_intensity: 0.0,
            };
            frame.time = std::f32::consts::PI * 3.0;
        }
        draw_frame(
            &mut app,
            snapshot(&mut context, &mut owner, ids),
            Some(&output),
        );
        assert_colours(
            &pixels(&device, &queue, &output),
            [[48, 24, 80], [24, 80, 48]],
        );
        {
            let mut frame = frame(&mut app);
            frame.settings = settings;
            frame.time = 0.0;
        }
        // Losing the surface must still apply font/user texture work and complete its epoch.
        let managed = context.register_texture(
            OwnedTextureData::from_pixels(TextureFormat::RGBA32, 1, 1, &[255; 4]).unwrap(),
        );
        draw_frame(&mut app, snapshot(&mut context, &mut owner, ids), None);
        owner.poll_completions(&mut context).unwrap();
        assert!(
            !context
                .with_texture(managed, |texture| texture.texture_id())
                .unwrap()
                .is_null()
        );
        renderer(&mut app)
            .imgui
            .shutdown(&mut owner, &mut context)
            .unwrap();
        assert!(
            bevy::tasks::block_on(validation.pop()).is_none(),
            "initial rendering produced a validation error"
        );

        // Request a fresh device through Bevy's recovery path without rebuilding the App.
        eprintln!(
            "composition fixture: colours, effects and skipped-frame feedback passed; destroy device"
        );
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(10)),
            })
            .unwrap();
        device.destroy();
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(10)),
            })
            .unwrap();
        let game_time = app.world().resource::<Time>().elapsed();
        update_device_lifecycle(&mut app);
        assert!(
            app.world_mut()
                .resource_mut::<RenderRecovery>()
                .error
                .take()
                .unwrap()
                .0
        );
        eprintln!("composition fixture: lost device detected; prepare replacement");
        let (replacement_device, replacement_queue) =
            bevy::tasks::block_on(adapter.request_device(&renderer_device_descriptor(&adapter)))
                .unwrap();
        let validation = replacement_device.push_error_scope(wgpu::ErrorFilter::Validation);
        let (replacement_panels, replacement_output) = targets(&replacement_device, &mut app);
        let (imgui, mut replacement_owner) = WgpuSnapshotRenderer::new(
            WgpuInitInfo::new(
                replacement_device.clone(),
                replacement_queue.clone(),
                bedtime_ui::gpu::OUTPUT_FORMAT,
            ),
            &mut context,
        )
        .unwrap();
        replace(&mut app, imgui, OUTPUT_SIZE, settings);
        app.world_mut().resource_mut::<RenderRecovery>().replacement = Some(
            manual_resources(&instance, &adapter, &replacement_device, &replacement_queue).into(),
        );
        eprintln!("composition fixture: enter Bevy recovery and RenderStartup");
        update_device_lifecycle(&mut app); // Select recovery.
        update_device_lifecycle(&mut app); // Install replacement resources.
        update_device_lifecycle(&mut app); // Run RenderStartup.
        assert_eq!(
            app.world().resource::<Time>().elapsed(),
            game_time,
            "GPU lifecycle advanced gameplay time"
        );
        let replacement_ids = replacement_panels.each_ref().map(|panel| {
            renderer(&mut app)
                .imgui
                .register_external_texture(&panel.view)
                .unwrap()
        });
        eprintln!("composition fixture: replacement camera warm-up");
        for _ in 0..4 {
            draw_frame(
                &mut app,
                snapshot(&mut context, &mut replacement_owner, replacement_ids),
                Some(&replacement_output),
            );
        }
        assert_colours(
            &pixels(&replacement_device, &replacement_queue, &replacement_output),
            [[96, 48, 160], [48, 160, 96]],
        );
        assert!(
            app.world().get::<GameEntity>(game_entity).is_some(),
            "GPU recovery discarded gameplay state"
        );
        assert!(
            cameras
                .into_iter()
                .all(|camera| app.world().get::<Camera>(camera).is_some())
        );
        assert!(app.sub_app(RenderApp).world().resource::<RenderStarts>().0 >= 2);
        assert!(app.world().resource::<RenderRecovery>().error.is_none());
        assert!(
            !context
                .with_texture(managed, |texture| texture.texture_id())
                .unwrap()
                .is_null(),
            "recovery did not reupload the managed texture"
        );
        renderer(&mut app)
            .imgui
            .shutdown(&mut replacement_owner, &mut context)
            .unwrap();
        assert!(
            bevy::tasks::block_on(validation.pop()).is_none(),
            "replacement rendering produced a validation error"
        );
    }
}
