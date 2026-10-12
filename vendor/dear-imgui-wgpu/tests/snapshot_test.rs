//! Detached rendering must work without binding the UI Context on the render thread.

use dear_imgui_rs::{
    Context,
    render::{FrameSnapshot, TextureOp},
    texture::{OwnedTextureData, TextureFormat as ImGuiTextureFormat},
};
use dear_imgui_wgpu::{
    FramebufferExtent, RendererError, WgpuInitInfo, WgpuSnapshotContext, WgpuSnapshotRenderer,
    wgpu::*,
};
use static_assertions::{assert_impl_all, assert_not_impl_any};

fn request_test_device() -> Option<(Device, Queue)> {
    #[cfg(any(feature = "wgpu-27", feature = "wgpu-28"))]
    let instance = Instance::new(&InstanceDescriptor::default());
    #[cfg(any(feature = "wgpu-29", feature = "wgpu-30"))]
    let instance = Instance::new(InstanceDescriptor::new_without_display_handle());
    let request = |force_fallback_adapter| {
        pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
            force_fallback_adapter,
            ..Default::default()
        }))
    };
    let adapter = request(true).or_else(|_| request(false)).ok();
    let device = adapter.and_then(|adapter| {
        pollster::block_on(adapter.request_device(&DeviceDescriptor::default())).ok()
    });
    assert!(
        device.is_some() || std::env::var_os("DEAR_IMGUI_REQUIRE_WGPU_ADAPTER").is_none(),
        "detached renderer tests require a working headless GPU adapter",
    );
    device
}

fn init_context(context: &mut Context) {
    context.io_mut().set_display_size([64.0, 64.0]);
    context.io_mut().set_delta_time(1.0 / 60.0);
}

fn capture(context: &mut Context, snapshot_context: &mut WgpuSnapshotContext) -> FrameSnapshot {
    init_context(context);
    context.frame().text("managed atlas feedback");
    snapshot_context.render_snapshot(context).unwrap()
}

#[test]
fn detached_renderer_can_live_in_a_render_world() {
    assert_impl_all!(WgpuSnapshotRenderer: Send, Sync);
    assert_impl_all!(FrameSnapshot: Send, Sync);
    assert_not_impl_any!(WgpuSnapshotContext: Send, Sync);
}

#[test]
fn snapshots_draw_managed_fonts_and_external_images_on_a_separate_thread() {
    let Some((device, queue)) = request_test_device() else {
        return;
    };
    let mut context = Context::create();
    let (mut renderer, mut snapshot_context) = WgpuSnapshotRenderer::new(
        WgpuInitInfo::new(device.clone(), queue.clone(), TextureFormat::Rgba8Unorm),
        &mut context,
    )
    .unwrap();
    let external = device.create_texture(&TextureDescriptor {
        label: Some("snapshot external green pixel"),
        size: Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        TexelCopyTextureInfo {
            texture: &external,
            mip_level: 0,
            origin: Origin3d::ZERO,
            aspect: TextureAspect::All,
        },
        &[0, 255, 0, 255],
        TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: Some(1),
        },
        Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let external_id = renderer
        .register_external_texture(&external.create_view(&TextureViewDescriptor::default()))
        .unwrap();
    let reset = context
        .platform_io()
        .draw_callback_reset_render_state_raw()
        .unwrap();
    let nearest = context
        .platform_io()
        .draw_callback_set_sampler_nearest_raw()
        .unwrap();
    let linear = context
        .platform_io()
        .draw_callback_set_sampler_linear_raw()
        .unwrap();
    init_context(&mut context);
    {
        let ui = context.frame();
        let draw = ui.get_foreground_draw_list();
        unsafe { draw.add_callback(reset, std::ptr::null_mut(), 0) };
        draw.with_clip_rect([8.0, 8.0], [24.0, 24.0], || {
            draw.add_rect(
                [0.0, 0.0],
                [32.0, 32.0],
                dear_imgui_rs::Color::new(1.0, 0.0, 0.0, 1.0),
            )
            .filled(true)
            .build();
        });
        unsafe { draw.add_callback(nearest, std::ptr::null_mut(), 0) };
        draw.add_image(
            external_id.texture_id(),
            [32.0, 8.0],
            [48.0, 24.0],
            [0.0, 0.0],
            [1.0, 1.0],
            dear_imgui_rs::Color::WHITE,
        );
        unsafe { draw.add_callback(linear, std::ptr::null_mut(), 0) };
    }
    let snapshot = snapshot_context.render_snapshot(&mut context).unwrap();
    assert!(
        snapshot
            .texture_requests()
            .iter()
            .any(|request| matches!(request.operation(), TextureOp::Create { .. }))
    );

    let (mut renderer, pixels) = std::thread::spawn(move || {
        let target = device.create_texture(&TextureDescriptor {
            label: Some("snapshot pixel target"),
            size: Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&TextureViewDescriptor::default());
        let readback = device.create_buffer(&BufferDescriptor {
            label: Some("snapshot pixel readback"),
            size: 64 * 256,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor::default());
        {
            let attachments = [Some(RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Clear(Color::BLACK),
                    store: StoreOp::Store,
                },
                depth_slice: None,
            })];
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("detached snapshot pass"),
                color_attachments: &attachments,
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                #[cfg(any(feature = "wgpu-28", feature = "wgpu-29", feature = "wgpu-30"))]
                multiview_mask: None,
            });
            renderer
                .render_snapshot(snapshot, &mut pass, FramebufferExtent::new(64, 64))
                .unwrap();
        }
        encoder.copy_texture_to_buffer(
            TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            TexelCopyBufferInfo {
                buffer: &readback,
                layout: TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(64),
                },
            },
            Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        let (sender, receiver) = std::sync::mpsc::channel();
        readback
            .slice(..)
            .map_async(MapMode::Read, move |result| sender.send(result).unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        receiver.recv().unwrap().unwrap();
        let mapped = readback.slice(..).get_mapped_range();
        #[cfg(feature = "wgpu-30")]
        let mapped = mapped.unwrap();
        let pixels = mapped.to_vec();
        drop(mapped);
        readback.unmap();
        (renderer, pixels)
    })
    .join()
    .unwrap();
    let pixel = |x: usize, y: usize| &pixels[y * 256 + x * 4..y * 256 + x * 4 + 4];
    assert_eq!(pixel(16, 16), [255, 0, 0, 255]);
    assert_eq!(pixel(4, 16), [0, 0, 0, 255]);
    assert_eq!(pixel(40, 16), [0, 255, 0, 255]);
    snapshot_context.poll_completions(&mut context).unwrap();
    renderer.unregister_external_texture(external_id).unwrap();
    renderer
        .shutdown(&mut snapshot_context, &mut context)
        .unwrap();
}

#[test]
fn skipped_frames_acknowledge_uploads_and_shutdown_requeues_for_replacement() {
    let Some((device, queue)) = request_test_device() else {
        return;
    };
    let mut context = Context::create();
    let (mut renderer, mut snapshot_context) = WgpuSnapshotRenderer::new(
        WgpuInitInfo::new(device.clone(), queue.clone(), TextureFormat::Rgba8Unorm),
        &mut context,
    )
    .unwrap();
    let managed = context.register_texture(
        OwnedTextureData::from_pixels(ImGuiTextureFormat::RGBA32, 1, 1, &[255; 4]).unwrap(),
    );
    renderer
        .reconcile_snapshot(capture(&mut context, &mut snapshot_context))
        .unwrap();
    snapshot_context.poll_completions(&mut context).unwrap();
    assert!(
        !context
            .with_texture(managed, |texture| texture.texture_id())
            .unwrap()
            .is_null()
    );
    context
        .try_with_texture_mut(managed, |mut texture| {
            texture.replace_pixels(&[0, 255, 0, 255])
        })
        .unwrap();
    let update = capture(&mut context, &mut snapshot_context);
    assert!(
        update
            .texture_requests()
            .iter()
            .any(|request| matches!(request.operation(), TextureOp::Update { .. }))
    );
    renderer.reconcile_snapshot(update).unwrap();
    renderer
        .shutdown(&mut snapshot_context, &mut context)
        .unwrap();
    assert!(
        context
            .with_texture(managed, |texture| texture.texture_id())
            .unwrap()
            .is_null()
    );
    assert!(context.io().backend_renderer_name().is_none());
    let (mut replacement, mut replacement_context) = WgpuSnapshotRenderer::new(
        WgpuInitInfo::new(device, queue, TextureFormat::Rgba8Unorm),
        &mut context,
    )
    .unwrap();
    let create = capture(&mut context, &mut replacement_context);
    assert!(
        create
            .texture_requests()
            .iter()
            .any(|request| matches!(request.operation(), TextureOp::Create { .. }))
    );
    replacement.reconcile_snapshot(create).unwrap();
    replacement
        .shutdown(&mut replacement_context, &mut context)
        .unwrap();
}

#[test]
fn lifecycle_rejects_outstanding_frames_before_releasing_resources() {
    let Some((device, queue)) = request_test_device() else {
        return;
    };
    let mut context = Context::create();
    let (mut renderer, mut snapshot_context) = WgpuSnapshotRenderer::new(
        WgpuInitInfo::new(device, queue, TextureFormat::Rgba8Unorm),
        &mut context,
    )
    .unwrap();
    let snapshot = capture(&mut context, &mut snapshot_context);
    assert!(matches!(
        renderer.shutdown(&mut snapshot_context, &mut context),
        Err(RendererError::RendererConsumer(_))
    ));
    assert!(context.io().backend_renderer_name().is_some());
    renderer.reconcile_snapshot(snapshot).unwrap();
    renderer
        .shutdown(&mut snapshot_context, &mut context)
        .unwrap();
}

#[test]
fn snapshots_reject_out_of_order_delivery() {
    let Some((device, queue)) = request_test_device() else {
        return;
    };
    let mut context = Context::create();
    let (mut renderer, mut snapshot_context) = WgpuSnapshotRenderer::new(
        WgpuInitInfo::new(device, queue, TextureFormat::Rgba8Unorm),
        &mut context,
    )
    .unwrap();
    let first = capture(&mut context, &mut snapshot_context);
    let second = capture(&mut context, &mut snapshot_context);
    renderer.reconcile_snapshot(second).unwrap();
    assert!(matches!(
        renderer.reconcile_snapshot(first),
        Err(RendererError::FrameEpochOutOfOrder { .. })
    ));
    snapshot_context.poll_completions(&mut context).unwrap();
    renderer
        .shutdown(&mut snapshot_context, &mut context)
        .unwrap();
}
