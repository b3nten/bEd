//! Convert the completed UI image to AppKit's display-space premultiplication.
//! This belongs after all UI blending, immediately before Bevy's output blit.
use bevy::{
    asset::{embedded_asset, load_embedded_asset},
    camera::NormalizedRenderTarget,
    core_pipeline::{
        Core2d, Core3d,
        blit::{BlitPipeline, BlitPipelineKey, init_blit_pipeline},
        upscaling::upscaling,
    },
    prelude::*,
    render::{
        RenderApp, RenderStartup,
        camera::ExtractedCamera,
        render_resource::*,
        renderer::{RenderContext, ViewQuery},
        sync_world::MainEntity,
        view::{ViewTarget, window::ExtractedWindow},
    },
    window::CompositeAlphaMode,
};
use bevy_egui::render::{EguiCameraView, egui_pass};

pub(crate) fn install(app: &mut App) {
    embedded_asset!(app, "macos_output.wesl");
    app.sub_app_mut(RenderApp)
        .add_systems(RenderStartup, prepare.after(init_blit_pipeline))
        .add_systems(
            Core2d,
            convert
                .in_set(crate::UiSystems::PrepareOutput)
                .after(egui_pass)
                .before(upscaling),
        )
        .add_systems(
            Core3d,
            convert
                .in_set(crate::UiSystems::PrepareOutput)
                .after(egui_pass)
                .before(upscaling),
        );
}

#[derive(Resource)]
struct OutputPipeline(CachedRenderPipelineId);

fn prepare(
    mut commands: Commands,
    blit: Res<BlitPipeline>,
    assets: Res<AssetServer>,
    cache: Res<PipelineCache>,
) {
    let mut descriptor = blit.specialize(BlitPipelineKey {
        target_format: TextureFormat::Rgba8UnormSrgb,
        blend_state: None,
        samples: 1,
        source_space: None,
    });
    descriptor.label = Some("UI AppKit alpha conversion".into());
    descriptor.fragment.as_mut().unwrap().shader =
        load_embedded_asset!(assets.as_ref(), "macos_output.wesl");
    commands.insert_resource(OutputPipeline(cache.queue_render_pipeline(descriptor)));
}

fn convert(
    view: ViewQuery<(&ViewTarget, &ExtractedCamera, &EguiCameraView)>,
    windows: Query<(MainEntity, &ExtractedWindow)>,
    output: Res<OutputPipeline>,
    blit: Res<BlitPipeline>,
    cache: Res<PipelineCache>,
    mut ctx: RenderContext,
) {
    let (target, camera, _) = view.into_inner();
    let Some(NormalizedRenderTarget::Window(window)) = &camera.target else {
        return;
    };
    // wgpu's Metal backend calls its transparent CAMetalLayer mode
    // PostMultiplied; AppKit nevertheless expects display-premultiplied bytes.
    if !windows.iter().any(|(entity, w)| {
        entity == window.entity() && w.alpha_mode == CompositeAlphaMode::PostMultiplied
    }) || target.main_texture_format() != TextureFormat::Rgba8UnormSrgb
        || matches!(
            camera.output_mode,
            bevy::camera::CameraOutputMode::Skip
                | bevy::camera::CameraOutputMode::Write {
                    clear_color: ClearColorConfig::None,
                    ..
                }
        )
    {
        return;
    }
    let Some(pipeline) = cache.get_render_pipeline(output.0) else {
        return;
    };
    let post = target.post_process_write();
    let binding = blit.create_bind_group(ctx.render_device(), post.source, &cache);
    let mut pass = ctx
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("UI AppKit alpha conversion"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: post.destination,
                depth_slice: None,
                resolve_target: None,
                ops: Operations::default(),
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, &binding, &[]);
    pass.draw(0..3, 0..1);
}
