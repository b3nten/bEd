//! Windowless Bevy apps that export a GPU image to their presentation host.
use crate::{UiSystems, UiViewport, viewport_image};
use bevy::{
    asset::RenderAssetUsages,
    camera::{ManualTextureViewHandle, RenderTarget},
    prelude::*,
    render::{
        Extract, ExtractSchedule, Render, RenderApp, RenderPlugin, RenderSystems,
        render_asset::{RenderAssets, prepare_assets},
        render_resource::{
            FilterMode, SamplerDescriptor, TextureFormat, TextureView, TextureViewDescriptor,
        },
        renderer::{RenderAdapter, RenderAdapterInfo, RenderDevice, RenderInstance, RenderQueue},
        settings::{RenderCreation, RenderResources},
        texture::{GpuImage, ManualTextureView, ManualTextureViews},
    },
};
use std::{
    collections::HashSet,
    marker::PhantomData,
    sync::{Arc, Mutex, Weak},
};

/// Installs shared GPU output support in a presentation app, without Workbench.
/// UiPlugin includes this plugin. Outputs are created after plugin initialization.
pub struct ViewportHostPlugin;
impl Plugin for ViewportHostPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Outputs>()
            .add_systems(PreUpdate, observe_outputs.after(UiSystems::Draw));
        app.sub_app_mut(RenderApp)
            .init_resource::<ExportedImages>()
            .add_systems(ExtractSchedule, extract_outputs)
            .add_systems(
                Render,
                prepare_outputs
                    .in_set(RenderSystems::PrepareAssets)
                    .after(prepare_assets::<GpuImage>),
            );
    }

    fn finish(&self, app: &mut App) {
        let world = app.world();
        let gpu = RenderResources(
            world.resource::<RenderDevice>().clone(),
            world.resource::<RenderQueue>().clone(),
            world.resource::<RenderAdapterInfo>().clone(),
            world.resource::<RenderAdapter>().clone(),
            app.sub_app(RenderApp)
                .world()
                .resource::<RenderInstance>()
                .clone(),
        );
        app.insert_resource(HostGpu(gpu));
    }
}

#[derive(Resource)]
struct HostGpu(RenderResources);

struct SharedImage {
    gpu: GpuImage,
    render_view: TextureView,
    requested: UVec2,
    visible: bool,
}

/// A host image backed by the child app's render target on the same GPU device.
/// Asset handles remain host-local; no scene assets cross world boundaries.
#[derive(Resource, Clone)]
pub struct ViewportOutput {
    pub image: Handle<Image>,
    gpu: RenderResources,
    shared: Arc<Mutex<SharedImage>>,
}

impl ViewportOutput {
    pub fn new(host: &mut World, size: UVec2) -> Self {
        let gpu = host.resource::<HostGpu>().0.clone();
        let shared = Arc::new(Mutex::new(shared_image(&gpu.0, size)));
        let mut image = viewport_image(size);
        // The GPU image is exported directly, not uploaded by the host's asset
        // renderer. The CPU asset supplies identity, size and normal lifetime.
        image.asset_usage = RenderAssetUsages::MAIN_WORLD;
        let image = host.resource_mut::<Assets<Image>>().add(image);
        host.resource_mut::<Outputs>()
            .0
            .push((image.id(), Arc::downgrade(&shared)));
        Self { image, gpu, shared }
    }
}

#[derive(Resource, Default)]
struct Outputs(Vec<(AssetId<Image>, Weak<Mutex<SharedImage>>)>);

#[derive(Resource, Default)]
struct ExportedImages(Vec<(AssetId<Image>, GpuImage)>);

fn observe_outputs(mut outputs: ResMut<Outputs>, views: Query<&UiViewport>) {
    outputs.0.retain(|(id, weak)| {
        let Some(shared) = weak.upgrade() else {
            return false;
        };
        let mut shared = shared.lock().unwrap();
        shared.visible = false;
        if let Some(view) = views
            .iter()
            .find(|view| view.image.id() == *id && view.visible)
        {
            shared.requested = view.physical_size;
            shared.visible = true;
        }
        true
    });
}

fn extract_outputs(mut commands: Commands, outputs: Extract<Res<Outputs>>) {
    let images = outputs
        .0
        .iter()
        .filter_map(|(id, weak)| {
            let shared = weak.upgrade()?;
            let image = shared.lock().unwrap().gpu.clone();
            Some((*id, image))
        })
        .collect();
    commands.insert_resource(ExportedImages(images));
}

fn prepare_outputs(
    exported: Res<ExportedImages>,
    mut images: ResMut<RenderAssets<GpuImage>>,
    mut previous: Local<HashSet<AssetId<Image>>>,
) {
    let current: HashSet<_> = exported.0.iter().map(|(id, _)| *id).collect();
    for id in previous.difference(&current) {
        images.remove(*id);
    }
    for (id, image) in &exported.0 {
        images.insert(*id, image.clone());
    }
    *previous = current;
}

fn shared_image(device: &RenderDevice, size: UVec2) -> SharedImage {
    assert!(
        size.min_element() > 0 && size.max_element() <= device.limits().max_texture_dimension_2d,
        "Viewport dimensions must fit the GPU texture limits"
    );
    let image = viewport_image(size);
    let texture = device.create_texture(&image.texture_descriptor);
    let render_view = texture.create_view(&TextureViewDescriptor {
        format: Some(TextureFormat::Rgba8UnormSrgb),
        ..default()
    });
    let gpu = GpuImage {
        texture_view: texture.create_view(&default()),
        texture,
        sampler: device.create_sampler(&SamplerDescriptor {
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..default()
        }),
        texture_descriptor: image.texture_descriptor,
        texture_view_descriptor: None,
        had_data: false,
    };
    SharedImage {
        gpu,
        render_view,
        requested: size,
        visible: false,
    }
}

/// Standard Bevy plugins for a windowless child renderer using a host output.
/// C selects its output camera; other scene cameras keep their own targets.
/// One output is supported per app. Rendering stays sequential in the child.
pub struct EmbeddedRenderPlugin<C: Component> {
    output: ViewportOutput,
    marker: PhantomData<fn() -> C>,
}
impl<C: Component> EmbeddedRenderPlugin<C> {
    pub fn new(output: ViewportOutput) -> Self {
        Self {
            output,
            marker: PhantomData,
        }
    }
}
impl<C: Component> Plugin for EmbeddedRenderPlugin<C> {
    fn build(&self, app: &mut App) {
        assert!(
            !app.world().contains_resource::<ViewportOutput>(),
            "One viewport output per embedded app"
        );
        let mut plugins = DefaultPlugins
            .build()
            .disable::<bevy::log::LogPlugin>()
            .disable::<bevy::winit::WinitPlugin>()
            .disable::<bevy::app::PanicHandlerPlugin>()
            .set(bevy::window::WindowPlugin {
                primary_window: None,
                exit_condition: bevy::window::ExitCondition::DontExit,
                close_when_requested: false,
                ..default()
            })
            .set(RenderPlugin {
                render_creation: RenderCreation::Manual(Box::new(self.output.gpu.clone())),
                ..default()
            });
        if app.is_plugin_added::<bevy::time::TimePlugin>() {
            plugins = plugins.disable::<bevy::time::TimePlugin>();
        }
        #[cfg(any(unix, windows))]
        {
            plugins = plugins.disable::<bevy::app::TerminalCtrlCHandlerPlugin>();
        }
        #[cfg(not(target_arch = "wasm32"))]
        if plugins.enabled::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>() {
            plugins =
                plugins.disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>();
        }
        app.add_plugins(plugins)
            .insert_resource(self.output.clone())
            .add_systems(
                PostUpdate,
                sync_camera::<C>.before(bevy::camera::CameraUpdateSystems),
            );
    }
}

fn sync_camera<C: Component>(
    output: Res<ViewportOutput>,
    mut views: ResMut<ManualTextureViews>,
    mut cameras: Query<(&mut Camera, &mut RenderTarget, &mut Projection), With<C>>,
) {
    let mut shared = output.shared.lock().unwrap();
    let size = shared.requested;
    let visible = shared.visible;
    let resized = shared.gpu.texture_descriptor.size.width != size.x
        || shared.gpu.texture_descriptor.size.height != size.y;
    if visible && resized {
        *shared = shared_image(&output.gpu.0, size);
        shared.visible = visible;
    }
    let handle = ManualTextureViewHandle(0);
    let changed = views
        .get(&handle)
        .is_none_or(|view| view.texture_view.id() != shared.render_view.id());
    views.insert(
        handle,
        ManualTextureView {
            texture_view: shared.render_view.clone(),
            size: UVec2::new(
                shared.gpu.texture_descriptor.size.width,
                shared.gpu.texture_descriptor.size.height,
            ),
            view_format: TextureFormat::Rgba8UnormSrgb,
        },
    );
    for (mut camera, mut target, mut projection) in &mut cameras {
        camera.is_active = visible;
        if !matches!(*target, RenderTarget::TextureView(current) if current == handle) || changed {
            *target = RenderTarget::TextureView(handle);
            projection.set_changed();
        }
    }
}
