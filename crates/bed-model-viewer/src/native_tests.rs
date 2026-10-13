//! Pixel assertions on the real GPU; opt in on machines with a native adapter.
use super::{
    camera::Camera,
    debug_views::DisplayMode,
    model::{Image, Material, Primitive, Scene, TextureInfo, TextureSampler, Vertex},
    render::{Lighting, SETTLE_FRAMES, SceneGpu as SceneState, Settings},
};
use bed_editor_api::gpu::GpuContext;
use bed_ui::TextureHandle;
use bed_ui::gpu::{RenderOutput, RenderTarget, renderer_device_descriptor, wgpu};
use glam::Vec3;
use std::{
    future::Future,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
    time::{Duration, Instant},
};

fn block_on<T>(future: impl Future<Output = T>) -> T {
    struct ThreadWake(std::thread::Thread);

    impl Wake for ThreadWake {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park(),
        }
    }
}

struct TestGpu {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    generation: u64,
}

impl TestGpu {
    fn context<'a>(&'a self, encoder: &'a mut wgpu::CommandEncoder) -> GpuContext<'a> {
        GpuContext {
            world: None,
            instance: &self.instance,
            adapter: &self.adapter,
            device: &self.device,
            queue: &self.queue,
            encoder,
            generation: self.generation,
            error_handlers: None,
        }
    }

    fn render_resources(&self) -> bevy::render::settings::RenderResources {
        bevy::render::settings::RenderResources(
            bevy::render::renderer::RenderDevice::from(self.device.clone()),
            bevy::render::renderer::RenderQueue::new(self.queue.clone()),
            bevy::render::renderer::RenderAdapterInfo::new(self.adapter.get_info()),
            bevy::render::renderer::RenderAdapter::new(self.adapter.clone()),
            bevy::render::renderer::RenderInstance::new(self.instance.clone()),
        )
    }

    fn renderer(&self, scene: &Scene) -> SceneGpu {
        let bytes = scene
            .source_bytes
            .clone()
            .unwrap_or_else(|| super::fixtures::scene_bytes(scene).into());
        let prepared = super::prepare::load(&bytes).unwrap();
        self.prepared_renderer(&super::model::PreparedModel::Gltf(prepared))
    }

    fn prepared_renderer(&self, prepared: &super::model::PreparedModel) -> SceneGpu {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let mut app = bevy::app::App::new();
        app.add_plugins(bed_runtime::BedRuntimePlugin);
        app.add_plugins(super::ModelViewerPlugin);
        app.add_plugins(bed_runtime::render::BedRenderPlugin {
            resources: self.render_resources(),
        });
        app.finish();
        app.cleanup();
        let mut context = self.context(&mut encoder);
        context.world = Some(app.world_mut());
        let state = SceneState::new(&mut context, prepared, TextureHandle::next().0).unwrap();
        let renderer = SceneGpu { state, app };
        self.assert_valid(scope);
        renderer
    }

    fn assert_valid(&self, scope: wgpu::ErrorScopeGuard) {
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(30)),
            })
            .unwrap();
        let error = block_on(scope.pop());
        assert!(error.is_none(), "Bevy GPU validation error: {error:?}");
    }
}

struct SceneGpu {
    state: SceneState,
    app: bevy::app::App,
}

impl SceneGpu {
    fn pending(&self) -> bool {
        self.state.pending()
    }

    pub fn render(
        &mut self,
        gpu: &mut GpuContext<'_>,
        target: &RenderTarget,
        camera: Camera,
        radius: f32,
        background: [f32; 4],
        settings: Settings,
    ) -> Result<(), String> {
        bed_runtime::render::begin_preview_frame(self.app.world_mut());
        let mut context = GpuContext {
            world: Some(self.app.world_mut()),
            instance: gpu.instance,
            adapter: gpu.adapter,
            device: gpu.device,
            queue: gpu.queue,
            encoder: gpu.encoder,
            generation: gpu.generation,
            error_handlers: gpu.error_handlers,
        };
        self.state
            .sync(&mut context, target, camera, radius, background, settings)?;
        self.app.update();
        Ok(())
    }
}

fn device() -> TestGpu {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        block_on(instance.request_adapter(&Default::default())).expect("native GPU adapter");
    let (device, queue) =
        block_on(adapter.request_device(&renderer_device_descriptor(&adapter))).unwrap();
    TestGpu {
        instance,
        adapter,
        device,
        queue,
        generation: 1,
    }
}

fn pixels(gpu: &TestGpu, scene: &Scene, camera: Camera, size: [u32; 2]) -> Vec<u8> {
    let mut renderer = gpu.renderer(scene);
    render_pixels(gpu, &mut renderer, scene, camera, size, Settings::default())
}

fn render_pixels(
    gpu: &TestGpu,
    renderer: &mut SceneGpu,
    scene: &Scene,
    camera: Camera,
    size: [u32; 2],
    settings: Settings,
) -> Vec<u8> {
    let TestGpu { device, .. } = gpu;
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let target = RenderTarget::new(
        device,
        RenderOutput {
            handle: TextureHandle(1),
            size,
            depth: true,
            revision: 1,
        },
    )
    .unwrap();
    let mut encoder = device.create_command_encoder(&Default::default());
    // Embedded shader assets, generated environment maps and temporal effects
    // become ready across several updates, as they do in the live panel.
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut settled = 0;
    while settled < SETTLE_FRAMES + 4 {
        renderer
            .render(
                &mut gpu.context(&mut encoder),
                &target,
                camera,
                scene.radius(),
                [0.0, 0.0, 0.0, 1.0],
                settings,
            )
            .unwrap();
        if renderer.pending() {
            assert!(
                Instant::now() < deadline,
                "Bevy scene or rendering pipelines timed out"
            );
            std::thread::sleep(Duration::from_millis(1));
        } else {
            settled += 1;
        }
    }
    gpu.assert_valid(scope);
    capture_target(gpu, &target)
}

fn capture_target(gpu: &TestGpu, target: &RenderTarget) -> Vec<u8> {
    let TestGpu { device, queue, .. } = gpu;
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let size = target.size;
    let mut encoder = device.create_command_encoder(&Default::default());
    let stride = (size[0] * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("glTF pixels"),
        size: u64::from(stride) * u64::from(size[1]),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        target.texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(size[1]),
            },
        },
        target.texture.size(),
    );
    queue.submit([encoder.finish()]);
    let (sender, receiver) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            sender.send(result).unwrap();
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
    let mapped = buffer
        .slice(..)
        .get_mapped_range()
        .expect("readback buffer is mapped");
    let pixels = mapped
        .chunks_exact(stride as usize)
        .flat_map(|row| row[..size[0] as usize * 4].iter().copied())
        .collect();
    drop(mapped);
    buffer.unmap();
    gpu.assert_valid(scope);
    pixels
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn native_registered_model_renderer_shares_game_world_and_retires_its_view() {
    use bed_editor_api::{EditorFrame, EditorPanelBinding, PanelInput, PluginDocument};
    use bed_ui::gpu::{CanvasRenderInvocation, CanvasRenderer};
    use bevy::prelude::*;

    #[derive(Component)]
    struct GameState(u32);

    let gpu = device();
    let validation = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let bytes: Arc<[u8]> = include_bytes!("../../../tests/fixtures/gltf/cube.glb")
        .as_slice()
        .into();
    let scene = super::model::load(&bytes).unwrap();
    let mut app = App::new();
    app.add_plugins((bed_runtime::BedRuntimePlugin, super::ModelViewerPlugin));
    app.add_plugins(bed_runtime::render::BedRenderPlugin {
        resources: gpu.render_resources(),
    });
    app.finish();
    app.cleanup();
    let game = app.world_mut().spawn(GameState(42)).id();
    let document = bed_editing::identity::DocumentId(4);
    app.world_mut().resource_mut::<EditorFrame>().documents = vec![PluginDocument {
        id: document,
        path: "cube.glb".into(),
        kind: bed_editor_api::DocumentKind::Bytes,
        language_id: String::new(),
        revision: (1, 1),
        dirty: true,
        bytes,
        text: None,
    }];
    let entity = app
        .world_mut()
        .spawn(EditorPanelBinding {
            input: PanelInput::Document(document),
            document: Some(document),
            ..Default::default()
        })
        .id();
    let factory = app
        .world()
        .resource::<bed_workbench_api::Registry>()
        .panel(super::PANEL_ID)
        .unwrap()
        .factory;
    let _frontend = app
        .world_mut()
        .run_system_with(
            factory,
            bed_workbench_api::PanelCreation {
                entity,
                state: serde_json::Value::Null,
            },
        )
        .unwrap()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while app
        .world()
        .non_send::<super::GltfStates>()
        .get(entity)
        .unwrap()
        .prepared
        .is_none()
    {
        app.update();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    let output = {
        let mut states = app.world_mut().non_send_mut::<super::GltfStates>();
        let panel = states.get_mut(entity).unwrap();
        panel.camera.fit(&scene, 1.0);
        panel.scene = Some(scene);
        panel.canvas = Some(super::CanvasState {
            pixels: [128; 2],
            camera: panel.camera,
            background: [0.0, 0.0, 0.0, 1.0],
            settings: panel.settings,
        });
        panel.canvas_output().unwrap()
    };
    let target = RenderTarget::new(&gpu.device, output).unwrap();
    let render = app.world().get::<CanvasRenderer>(entity).unwrap().0;
    let mut settled = 0;
    while settled < SETTLE_FRAMES + 4 {
        bed_runtime::render::begin_preview_frame(app.world_mut());
        let commands = app
            .world_mut()
            .run_system_with(
                render,
                CanvasRenderInvocation {
                    entity,
                    instance: gpu.instance.clone(),
                    adapter: gpu.adapter.clone(),
                    device: gpu.device.clone(),
                    queue: gpu.queue.clone(),
                    target: RenderTarget {
                        texture: target.texture.clone(),
                        view: target.view.clone(),
                        depth: target.depth.clone(),
                        size: target.size,
                    },
                    generation: gpu.generation,
                },
            )
            .unwrap()
            .unwrap();
        gpu.queue.submit(commands);
        app.update();
        if app
            .world()
            .non_send::<super::GltfStates>()
            .get(entity)
            .unwrap()
            .gpu
            .as_ref()
            .unwrap()
            .pending()
        {
            assert!(
                Instant::now() < deadline,
                "registered model renderer did not settle"
            );
            std::thread::sleep(Duration::from_millis(1));
        } else {
            settled += 1;
        }
    }
    let pixels = capture_target(&gpu, &target);
    assert!(
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| pixel[0] > 20 && pixel[1] > 20 && pixel[2] > 20)
            .count()
            > 2000
    );
    assert_eq!(app.world().get::<GameState>(game).unwrap().0, 42);
    assert!(
        app.world_mut()
            .query::<&bed_runtime::render::PreviewView>()
            .iter(app.world())
            .any(|view| view.0 == output.handle.0)
    );
    app.world_mut().entity_mut(entity).despawn();
    app.update();
    assert!(
        app.world()
            .non_send::<super::GltfStates>()
            .get(entity)
            .is_none()
    );
    bed_runtime::render::retire_previews(app.world_mut(), &Default::default());
    assert_eq!(
        app.world_mut()
            .query::<&bed_runtime::render::PreviewView>()
            .iter(app.world())
            .count(),
        0
    );
    assert_eq!(app.world().get::<GameState>(game).unwrap().0, 42);
    gpu.assert_valid(validation);
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn native_gltf_load_error_preserves_device() {
    let gpu = device();
    // This valid document reaches Bevy, but scene inspection rejects its lack
    // of triangle geometry after the renderer has opened its error scopes.
    let mut source: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../../tests/fixtures/gltf/cube.gltf")).unwrap();
    source["scenes"][0]["nodes"] = serde_json::json!([]);
    let prepared = super::prepare::load(&serde_json::to_vec(&source).unwrap()).unwrap();
    let mut renderer = gpu.prepared_renderer(&super::model::PreparedModel::Gltf(prepared));
    let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let target = RenderTarget::new(
        &gpu.device,
        RenderOutput {
            handle: TextureHandle(1),
            size: [32; 2],
            depth: true,
            revision: 1,
        },
    )
    .unwrap();
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    let (mut camera, _) = Camera::restore(&serde_json::Value::Null);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Err(error) = renderer.render(
            &mut gpu.context(&mut encoder),
            &target,
            camera,
            1.0,
            [0.0, 0.0, 0.0, 1.0],
            Settings::default(),
        ) {
            assert_eq!(error, "glTF scene has no triangle geometry");
            break;
        }
        assert!(Instant::now() < deadline, "glTF loading did not finish");
        std::thread::sleep(Duration::from_millis(1));
    }
    gpu.assert_valid(scope);

    // A failed model must leave the shared device usable by subsequent panels.
    let scene =
        super::model::load(include_bytes!("../../../tests/fixtures/gltf/cube.glb")).unwrap();
    camera.fit(&scene, 1.0);
    assert!(
        pixels(&gpu, &scene, camera, [32; 2])
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[0] > 20)
    );
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn native_textured_scene_camera_and_resized_output() {
    let gpu = device();
    let scene =
        super::model::load(include_bytes!("../../../tests/fixtures/gltf/cube.glb")).unwrap();
    let (mut camera, _) = Camera::restore(&serde_json::Value::Null);
    camera.fit(&scene, 1.0);
    let mut renderer = gpu.renderer(&scene);
    let before = render_pixels(
        &gpu,
        &mut renderer,
        &scene,
        camera,
        [128; 2],
        Settings::default(),
    );
    assert!(
        before
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[0] > 20 && p[1] > 20 && p[2] > 20)
            .count()
            > 2000
    );
    camera.orbit([60.0, 20.0]);
    camera.zoom(1.0, scene.radius());
    let after = render_pixels(
        &gpu,
        &mut renderer,
        &scene,
        camera,
        [128; 2],
        Settings::default(),
    );
    assert_ne!(
        before, after,
        "camera interaction must update rendered pixels"
    );
    let resized = render_pixels(
        &gpu,
        &mut renderer,
        &scene,
        camera,
        [256, 128],
        Settings::default(),
    );
    assert!(resized.as_chunks::<4>().0.iter().any(|p| p[0] > 100));
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn native_littlest_tokyo_draco_and_static_skin() {
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/LittlestTokyo.glb"),
    )
    .unwrap();
    let scene = super::model::load(&bytes).unwrap();
    assert_eq!(scene.draco_primitives, 71);
    assert_eq!(scene.posed_meshes, 8);
    let gpu = device();
    let (mut camera, _) = Camera::restore(&serde_json::Value::Null);
    camera.fit(&scene, 1.0);
    let values = pixels(&gpu, &scene, camera, [512; 2]);
    assert!(
        values
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]) > 60)
            .count()
            > 10_000,
        "decoded Tokyo must render substantial visible geometry"
    );
}

#[test]
#[ignore = "requires a native GPU adapter and full-resolution house textures"]
fn native_house2_draco_extra_uv_sets_and_full_resolution_textures() {
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/house2.glb"),
    )
    .unwrap();
    let mut scene = super::model::load(&bytes).unwrap();
    assert_eq!(scene.draco_primitives, 21);
    assert_eq!(scene.nodes, 32);
    assert_eq!(scene.primitives.len(), 21);
    assert_eq!(scene.textures, 20);
    assert!(scene.images.iter().any(|image| image.size == [2048, 2048]));
    // The native renderer reads the original prepared GLB. Release the large
    // test inspection copy of texture pixels before loading its GPU assets.
    scene.images.clear();
    let gpu = device();
    let (mut camera, _) = Camera::restore(&serde_json::Value::Null);
    camera.fit(&scene, 1.0);
    let values = pixels(&gpu, &scene, camera, [256; 2]);
    assert!(
        values
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]) > 60)
            .count()
            > 1000,
        "the house must render substantial visible geometry"
    );
}

#[test]
#[ignore = "requires BED_MODEL_CHECK_PATH and a native GPU adapter"]
fn external_model_render_check() {
    let path = std::env::var_os("BED_MODEL_CHECK_PATH").expect("set BED_MODEL_CHECK_PATH");
    let stl = std::path::Path::new(&path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("stl"));
    let bytes = std::fs::read(path).unwrap();
    let mut scene = if stl {
        super::stl::load(&bytes)
    } else {
        super::model::load(&bytes)
    }
    .unwrap();
    eprintln!(
        "{} nodes, {} primitives, {} triangles, {} meshopt views, {} Draco primitives; bounds {:?} to {:?}",
        scene.nodes,
        scene.primitives.len(),
        scene.triangles(),
        scene.meshopt_views,
        scene.draco_primitives,
        scene.minimum,
        scene.maximum,
    );
    scene.images.clear();
    let gpu = device();
    let (mut camera, _) = Camera::restore(&serde_json::Value::Null);
    camera.fit(&scene, 1.0);
    let mut renderer = if stl {
        gpu.prepared_renderer(&super::model::PreparedModel::Stl(Box::new(
            super::stl::prepare(&bytes).unwrap(),
        )))
    } else {
        gpu.renderer(&scene)
    };
    for (name, orbit) in [
        ("external-model", [0.0, 0.0]),
        ("external-model-orbit", [120.0, -60.0]),
    ] {
        camera.orbit(orbit);
        let values = render_pixels(
            &gpu,
            &mut renderer,
            &scene,
            camera,
            [512; 2],
            Settings::default(),
        );
        capture(name, &values, [512; 2]);
        assert!(
            values
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|p| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]) > 60)
                .count()
                > 1000,
            "the model must render visible geometry"
        );
    }
}

fn overlap(alpha: gltf::material::AlphaMode, opacity: f32) -> Scene {
    let triangle = |z, color, mode| Primitive {
        vertices: [[-1.0, -1.0, z], [1.0, -1.0, z], [0.0, 1.0, z]]
            .map(|position| Vertex {
                position,
                normal: [0.0, 0.0, 1.0],
                uv0: [0.0; 2],
                uv1: [0.0; 2],
                tangent: [0.0; 4],
                color: [1.0; 4],
            })
            .to_vec(),
        indices: vec![0, 1, 2],
        has_tangents: false,
        material: Material {
            color,
            texture: None,
            metallic: 1.0,
            roughness: 1.0,
            metallic_roughness_texture: None,
            normal_texture: None,
            normal_scale: 1.0,
            occlusion_texture: None,
            occlusion_strength: 1.0,
            emissive: [0.0; 3],
            emissive_texture: None,
            alpha: mode,
            cutoff: 0.5,
            double_sided: false,
            unlit: true,
        },
    };
    Scene {
        primitives: vec![
            triangle(1.0, [1.0, 0.0, 0.0, opacity], alpha),
            triangle(0.0, [0.0, 0.0, 1.0, 1.0], gltf::material::AlphaMode::Opaque),
        ],
        images: Vec::new(),
        textures: 0,
        minimum: Vec3::new(-1.0, -1.0, 0.0),
        maximum: Vec3::ONE,
        nodes: 2,
        animations: 0,
        draco_primitives: 0,
        meshopt_views: 0,
        posed_meshes: 0,
        source_bytes: None,
    }
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn native_depth_mask_and_transparency() {
    let gpu = device();
    let camera = Camera {
        target: Vec3::ZERO,
        yaw: 0.0,
        pitch: 0.0,
        distance: 4.0,
    };
    let center = |scene: &Scene| {
        let values = pixels(&gpu, scene, camera, [64; 2]);
        values[(32 * 64 + 32) * 4..(32 * 64 + 32) * 4 + 4].to_vec()
    };
    let opaque = center(&overlap(gltf::material::AlphaMode::Opaque, 1.0));
    assert!(
        opaque[0] > opaque[1].saturating_add(20)
            && opaque[0] > opaque[2].saturating_add(20)
            && opaque[3] == 255,
        "near red mesh must occlude a later blue mesh: {opaque:?}"
    );
    let masked = center(&overlap(gltf::material::AlphaMode::Mask, 0.2));
    assert!(
        masked[2] > masked[0].saturating_add(20)
            && masked[2] > masked[1].saturating_add(20)
            && masked[3] == 255,
        "masked mesh must discard pixels below cutoff: {masked:?}"
    );
    let blend = center(&overlap(gltf::material::AlphaMode::Blend, 0.5));
    assert!(
        blend[0] > 40
            && blend[2] > 40
            && blend[1].saturating_add(20) < blend[0].min(blend[2])
            && blend[3] == 255,
        "transparent mesh must composite after opaque geometry: {blend:?}"
    );
    let mut textured = overlap(gltf::material::AlphaMode::Opaque, 1.0);
    textured.primitives.truncate(1);
    let primitive = &mut textured.primitives[0];
    primitive.material.color = [1.0; 4];
    primitive.material.texture = Some(TextureInfo {
        image: 0,
        tex_coord: 1,
        sampler: TextureSampler {
            wrap: [gltf::texture::WrappingMode::Repeat; 2],
            mag_filter: Some(gltf::texture::MagFilter::Nearest),
            min_filter: Some(gltf::texture::MinFilter::Nearest),
        },
    });
    for vertex in &mut primitive.vertices {
        vertex.uv0 = [0.25, 0.5];
        vertex.uv1 = [0.75, 0.5];
    }
    textured.images.push(Image {
        size: [2, 1],
        rgba: Arc::from([255, 0, 0, 255, 0, 255, 0, 255]),
    });
    let texture = center(&textured);
    assert!(
        texture[1] > texture[0].saturating_add(20)
            && texture[1] > texture[2].saturating_add(20)
            && texture[3] == 255,
        "base-color texture must select its image and UV1 coordinates: {texture:?}"
    );
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn native_base_color_texture_transform_matches_baked_uvs() {
    use gltf::texture::{MagFilter, MinFilter, WrappingMode};
    use serde_json::json;
    let mut scene = overlap(gltf::material::AlphaMode::Opaque, 1.0);
    scene.primitives.truncate(1);
    scene.minimum.z = 1.0;
    let primitive = &mut scene.primitives[0];
    primitive.material.color = [1.0; 4];
    primitive.material.texture = Some(TextureInfo {
        image: 0,
        tex_coord: 0,
        sampler: TextureSampler {
            wrap: [WrappingMode::ClampToEdge; 2],
            mag_filter: Some(MagFilter::Nearest),
            min_filter: Some(MinFilter::Nearest),
        },
    });
    for vertex in &mut primitive.vertices {
        vertex.uv0 = [0.25, 0.5];
    }
    scene.images.push(Image {
        size: [2, 1],
        rgba: Arc::from([255, 0, 0, 255, 0, 255, 0, 255]),
    });
    let mut source = super::fixtures::embedded_scene(&scene);
    source["extensionsUsed"] = json!(["KHR_materials_unlit", "KHR_texture_transform"]);
    source["materials"][0]["pbrMetallicRoughness"]["baseColorTexture"]["extensions"] =
        json!({"KHR_texture_transform": {"offset": [0.5, 0.0]}});
    scene.source_bytes = Some(serde_json::to_vec(&source).unwrap().into());
    for vertex in &mut scene.primitives[0].vertices {
        vertex.uv0 = [0.75, 0.5];
    }
    let gpu = device();
    let (mut camera, _) = Camera::restore(&serde_json::Value::Null);
    camera.fit(&scene, 1.0);
    let transformed = pixels(&gpu, &scene, camera, [128; 2]);
    scene.source_bytes = None;
    let baked = pixels(&gpu, &scene, camera, [128; 2]);
    capture("texture-transform", &transformed, [128; 2]);
    capture("texture-transform-baked", &baked, [128; 2]);
    let green_count = |pixels: &[u8]| {
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| {
                pixel[1] > 120
                    && u16::from(pixel[1]) > 2 * u16::from(pixel[0])
                    && u16::from(pixel[1]) > 2 * u16::from(pixel[2])
            })
            .count()
    };
    let center = |pixels: &[u8]| pixels.as_chunks::<4>().0[64 * 128 + 64];
    let transformed_green = green_count(&transformed);
    let baked_green = green_count(&baked);
    let differences = changed_pixels(&transformed, &baked);
    assert!(
        transformed_green > 1000,
        "the transform must select the green half of the image: green {transformed_green}, center {:?}; baked green {baked_green}, center {:?}; changed {differences}",
        center(&transformed),
        center(&baked)
    );
    assert!(
        differences < 50,
        "Bevy texture transforms must match equivalent baked UV coordinates: {differences} changed pixels"
    );
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn native_normal_mapped_reflected_static_skin_matches_baked_geometry() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use gltf::texture::{MagFilter, MinFilter, WrappingMode};
    use serde_json::json;
    let mut scene = overlap(gltf::material::AlphaMode::Opaque, 1.0);
    scene.primitives.truncate(1);
    let primitive = &mut scene.primitives[0];
    primitive.material.unlit = false;
    primitive.material.metallic = 0.0;
    primitive.material.roughness = 0.85;
    primitive.material.color = [0.7, 0.3, 0.12, 1.0];
    primitive.material.normal_texture = Some(TextureInfo {
        image: 0,
        tex_coord: 0,
        sampler: TextureSampler {
            wrap: [WrappingMode::Repeat; 2],
            mag_filter: Some(MagFilter::Nearest),
            min_filter: Some(MinFilter::Nearest),
        },
    });
    primitive.has_tangents = true;
    for vertex in &mut primitive.vertices {
        vertex.uv0 = [0.5; 2];
        vertex.tangent = [1.0, 0.0, 0.0, 1.0];
    }
    scene.images.push(Image {
        size: [1, 1],
        // A substantial tangent-space Y component makes the bitangent
        // handedness observable in the resulting lit pixels.
        rgba: Arc::from([128, 224, 224, 255]),
    });
    let mut source = super::fixtures::embedded_scene(&scene);
    source["nodes"] = json!([
        {"mesh": 0, "skin": 0, "translation": [1000.0, 0.0, 0.0], "scale": [-1.0, 1.0, 1.0]},
        {"translation": [0.3, -0.2, 0.0], "scale": [-1.0, 1.0, 1.0]}
    ]);
    source["scenes"][0]["nodes"] = json!([0, 1]);
    source["skins"] = json!([{"joints": [1]}]);
    // Deliberately non-unit weights preserve the previous loader's normalized
    // skinning behavior. The mesh node's large offset must not affect the pose.
    for (semantic, component_type, bytes) in [
        ("JOINTS_0", 5123, vec![0_u8; 3 * 4 * 2]),
        (
            "WEIGHTS_0",
            5126,
            [2.0_f32, 0.0, 0.0, 0.0]
                .repeat(3)
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect(),
        ),
    ] {
        let buffer = source["buffers"].as_array().unwrap().len();
        let view = source["bufferViews"].as_array().unwrap().len();
        let accessor = source["accessors"].as_array().unwrap().len();
        source["buffers"].as_array_mut().unwrap().push(json!({
            "byteLength": bytes.len(),
            "uri": format!("data:application/octet-stream;base64,{}", STANDARD.encode(bytes))
        }));
        source["bufferViews"].as_array_mut().unwrap().push(json!({
            "buffer": buffer, "byteLength": 3 * 4 * if component_type == 5123 { 2 } else { 4 }
        }));
        source["accessors"].as_array_mut().unwrap().push(json!({
            "bufferView": view, "componentType": component_type, "count": 3, "type": "VEC4"
        }));
        source["meshes"][0]["primitives"][0]["attributes"][semantic] = json!(accessor);
    }
    for vertex in &mut scene.primitives[0].vertices {
        vertex.position[0] = -vertex.position[0] + 0.3;
        vertex.position[1] -= 0.2;
        vertex.tangent = [-1.0, 0.0, 0.0, -1.0];
    }
    scene.primitives[0].indices.swap(1, 2);
    scene.minimum = Vec3::new(-0.7, -1.2, 1.0);
    scene.maximum = Vec3::new(1.3, 0.8, 1.0);
    let gpu = device();
    let (mut camera, _) = Camera::restore(&serde_json::Value::Null);
    camera.fit(&scene, 1.0);
    let baked = pixels(&gpu, &scene, camera, [128; 2]);
    for mesh_node_scale in [1.0, -1.0] {
        source["nodes"][0]["scale"] = json!([mesh_node_scale, 1.0, 1.0]);
        scene.source_bytes = Some(serde_json::to_vec(&source).unwrap().into());
        let skinned = pixels(&gpu, &scene, camera, [128; 2]);
        assert!(
            skinned
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|pixel| pixel[..3]
                    .iter()
                    .map(|&value| u32::from(value))
                    .sum::<u32>()
                    > 30)
                .count()
                > 1000,
            "the reflected authored skin pose must stay visible with mesh-node scale {mesh_node_scale}"
        );
        assert!(
            changed_pixels(&skinned, &baked) < 50,
            "GPU skinning and normal-map handedness must match posed geometry regardless of mesh-node scale {mesh_node_scale}: {} changed pixels",
            changed_pixels(&skinned, &baked)
        );
    }
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn native_meshopt_scene_matches_uncompressed_geometry() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde_json::json;
    let mut scene = overlap(gltf::material::AlphaMode::Opaque, 1.0);
    scene.primitives.truncate(1);
    scene.minimum.z = 1.0;
    let positions: Vec<_> = scene.primitives[0]
        .vertices
        .iter()
        .map(|vertex| vertex.position)
        .collect();
    let compressed_positions = meshopt::encode_vertex_buffer(&positions).unwrap();
    let compressed_indices = meshopt::encode_index_buffer(&scene.primitives[0].indices, 3).unwrap();
    let gpu = device();
    let (mut camera, _) = Camera::restore(&serde_json::Value::Null);
    camera.fit(&scene, 1.0);
    let ordinary = pixels(&gpu, &scene, camera, [128; 2]);
    for extension in ["EXT_meshopt_compression", "KHR_meshopt_compression"] {
        let mut source = super::fixtures::embedded_scene(&scene);
        source["extensionsUsed"] = json!(["KHR_materials_unlit", extension]);
        source["extensionsRequired"] = json!([extension]);
        let position = source["meshes"][0]["primitives"][0]["attributes"]["POSITION"]
            .as_u64()
            .unwrap() as usize;
        let indices = source["meshes"][0]["primitives"][0]["indices"]
            .as_u64()
            .unwrap() as usize;
        for (accessor, stride, mode, bytes) in [
            (position, 12, "ATTRIBUTES", &compressed_positions),
            (indices, 4, "TRIANGLES", &compressed_indices),
        ] {
            let view = source["accessors"][accessor]["bufferView"]
                .as_u64()
                .unwrap() as usize;
            let buffer = source["buffers"].as_array().unwrap().len();
            source["buffers"].as_array_mut().unwrap().push(json!({
                "byteLength": bytes.len(),
                "uri": format!("data:application/octet-stream;base64,{}", STANDARD.encode(bytes))
            }));
            source["bufferViews"][view]["extensions"] = json!({extension: {
                "buffer": buffer, "byteOffset": 0, "byteLength": bytes.len(),
                "byteStride": stride, "count": 3, "mode": mode, "filter": "NONE"
            }});
        }
        let bytes = serde_json::to_vec(&source).unwrap();
        let prepared = super::prepare::load(&bytes).unwrap();
        assert_eq!(prepared.meshopt_views, 2);
        scene.source_bytes = Some(bytes.into());
        let compressed = pixels(&gpu, &scene, camera, [128; 2]);
        assert!(
            changed_pixels(&ordinary, &compressed) < 50,
            "{extension} must render like equivalent ordinary glTF: {} changed pixels",
            changed_pixels(&ordinary, &compressed)
        );
    }
}

fn sphere(metallic: f32, roughness: f32) -> Scene {
    let mut scene = overlap(gltf::material::AlphaMode::Opaque, 1.0);
    scene.primitives.truncate(1);
    let primitive = &mut scene.primitives[0];
    primitive.vertices.clear();
    primitive.indices.clear();
    primitive.material.color = [0.7, 0.3, 0.12, 1.0];
    primitive.material.unlit = false;
    primitive.material.metallic = metallic;
    primitive.material.roughness = roughness;
    let (rings, segments) = (16_u32, 24_u32);
    for ring in 0..=rings {
        let latitude = ring as f32 / rings as f32 * std::f32::consts::PI;
        for segment in 0..=segments {
            let longitude = segment as f32 / segments as f32 * std::f32::consts::TAU;
            let position = Vec3::new(
                latitude.sin() * longitude.cos(),
                latitude.cos(),
                latitude.sin() * longitude.sin(),
            );
            let uv = [segment as f32 / segments as f32, ring as f32 / rings as f32];
            primitive.vertices.push(Vertex {
                position: position.to_array(),
                normal: position.to_array(),
                uv0: uv,
                uv1: uv,
                tangent: [0.0; 4],
                color: [1.0; 4],
            });
        }
    }
    for ring in 0..rings {
        for segment in 0..segments {
            let a = ring * (segments + 1) + segment;
            let (b, c, d) = (a + 1, a + segments + 1, a + segments + 2);
            if ring != 0 {
                primitive.indices.extend([a, b, c]);
            }
            if ring + 1 != rings {
                primitive.indices.extend([b, d, c]);
            }
        }
    }
    scene.minimum = -Vec3::ONE;
    scene.maximum = Vec3::ONE;
    scene.nodes = 1;
    scene
}

fn changed_pixels(a: &[u8], b: &[u8]) -> usize {
    assert_eq!(a.len(), b.len());
    a.as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0)
        .filter(|(a, b)| (0..3).any(|channel| a[channel].abs_diff(b[channel]) > 10))
        .count()
}

fn capture(name: &str, pixels: &[u8], size: [u32; 2]) {
    let Some(directory) = std::env::var_os("BED_MODEL_CAPTURE_DIR") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    image::save_buffer(
        directory.join(format!("{name}.png")),
        pixels,
        size[0],
        size[1],
        image::ColorType::Rgba8,
    )
    .unwrap();
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn native_pbr_lighting_skybox_and_target_resize() {
    let gpu = device();
    let diffuse = sphere(0.0, 0.85);
    let glossy = sphere(1.0, 0.08);
    let (mut camera, _) = Camera::restore(&serde_json::Value::Null);
    camera.fit(&glossy, 1.0);
    let matte_pixels = pixels(&gpu, &diffuse, camera, [128; 2]);
    let mut renderer = gpu.renderer(&glossy);
    let studio = render_pixels(
        &gpu,
        &mut renderer,
        &glossy,
        camera,
        [128; 2],
        Settings::default(),
    );
    assert_eq!(studio[3], 0, "skybox-free background must be transparent");
    assert_eq!(
        studio[(64 * 128 + 64) * 4 + 3],
        255,
        "opaque model must remain visible over the transparent background"
    );
    assert!(
        changed_pixels(&matte_pixels, &studio) > 500,
        "metallic and roughness factors must visibly change lit material shading"
    );
    let outdoor_settings = Settings {
        lighting: Lighting::Outdoor,
        ..Settings::default()
    };
    let outdoor = render_pixels(
        &gpu,
        &mut renderer,
        &glossy,
        camera,
        [128; 2],
        outdoor_settings,
    );
    assert!(
        changed_pixels(&studio, &outdoor) > 300,
        "lighting presets must update the existing renderer's model shading"
    );
    let skybox_settings = Settings {
        skybox: true,
        ..outdoor_settings
    };
    let skybox = render_pixels(
        &gpu,
        &mut renderer,
        &glossy,
        camera,
        [128; 2],
        skybox_settings,
    );
    assert!(
        changed_pixels(&outdoor, &skybox) > 1000,
        "skybox toggle must fill the visible background"
    );
    assert!(skybox[..3].iter().any(|channel| *channel > 10));
    assert_eq!(skybox[3], 255, "enabled skybox must fill background alpha");
    let transparent = render_pixels(
        &gpu,
        &mut renderer,
        &glossy,
        camera,
        [128; 2],
        outdoor_settings,
    );
    assert_eq!(
        transparent[3], 0,
        "disabling the skybox must restore background transparency"
    );
    let resized = render_pixels(
        &gpu,
        &mut renderer,
        &glossy,
        camera,
        [192, 128],
        skybox_settings,
    );
    assert_eq!(resized.len(), 192 * 128 * 4);
    assert!(
        resized
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[0] > 30 && pixel[1] > 30)
    );
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn native_stl_renders_in_shared_bevy_viewer() {
    let bytes = br#"solid tetrahedron
facet normal 0 0 -1
outer loop
vertex -1 -1 -1
vertex 0 1 -1
vertex 1 -1 -1
endloop
endfacet
facet normal 0 -1 0
outer loop
vertex -1 -1 -1
vertex 1 -1 -1
vertex 0 0 1
endloop
endfacet
facet normal 0.872872 0.436436 0.218218
outer loop
vertex 1 -1 -1
vertex 0 1 -1
vertex 0 0 1
endloop
endfacet
facet normal -0.872872 0.436436 0.218218
outer loop
vertex 0 1 -1
vertex -1 -1 -1
vertex 0 0 1
endloop
endfacet
endsolid tetrahedron
"#;
    let scene = super::stl::load(bytes).unwrap();
    assert_eq!(scene.triangles(), 4);
    let gpu = device();
    let (mut camera, _) = Camera::restore(&serde_json::Value::Null);
    camera.fit(&scene, 1.0);
    let prepared = super::model::PreparedModel::Stl(Box::new(super::stl::prepare(bytes).unwrap()));
    let mut renderer = gpu.prepared_renderer(&prepared);
    let values = render_pixels(
        &gpu,
        &mut renderer,
        &scene,
        camera,
        [128; 2],
        Settings::default(),
    );
    assert!(
        values
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| pixel[..3].iter().all(|channel| *channel > 20))
            .count()
            > 1000,
        "STL geometry must render with the shared lit material"
    );
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn native_debug_modes_overlay_and_normal_lines() {
    let gpu = device();
    let mut scene = overlap(gltf::material::AlphaMode::Opaque, 1.0);
    scene.primitives.truncate(1);
    scene.minimum = Vec3::new(-1.0, -1.0, 1.0);
    scene.maximum = Vec3::new(1.0, 1.0, 1.0);
    let camera = Camera {
        target: Vec3::Z,
        yaw: 0.0,
        pitch: 0.0,
        distance: 4.0,
    };
    let mut renderer = gpu.renderer(&scene);
    let center = |values: &[u8]| values[(64 * 128 + 64) * 4..(64 * 128 + 64) * 4 + 3].to_vec();
    let render = |renderer: &mut SceneGpu, display| {
        render_pixels(
            &gpu,
            renderer,
            &scene,
            camera,
            [128; 2],
            Settings {
                display,
                ..Settings::default()
            },
        )
    };
    let shaded = render(&mut renderer, DisplayMode::Shaded);
    capture("shaded", &shaded, [128; 2]);
    assert!(
        center(&shaded)[0] > 80,
        "the triangle must cover the center"
    );
    let wireframe = render(&mut renderer, DisplayMode::Wireframe);
    capture("wireframe", &wireframe, [128; 2]);
    assert!(
        center(&wireframe).iter().all(|channel| *channel < 10),
        "wireframe must remove the filled triangle interior"
    );
    assert!(
        wireframe
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| pixel[..3].iter().any(|channel| *channel > 40))
            .count()
            > 30,
        "wireframe must still draw the triangle edges"
    );
    let overlay = render(&mut renderer, DisplayMode::WireframeOverlay);
    capture("wireframe-overlay", &overlay, [128; 2]);
    assert!(
        center(&shaded)
            .iter()
            .zip(center(&overlay))
            .all(|(a, b)| a.abs_diff(b) < 15),
        "wireframe overlay must retain the shaded triangle interior"
    );
    assert!(
        changed_pixels(&shaded, &overlay) > 30,
        "wireframe overlay must visibly add triangle edges"
    );
    let normals = render(&mut renderer, DisplayMode::Normals);
    capture("normals", &normals, [128; 2]);
    let normal = center(&normals);
    assert!(
        normal[2] > normal[0].saturating_add(20) && normal[2] > normal[1].saturating_add(20),
        "a +Z-facing normal must use the blue normal visualization: {normal:?}"
    );
    let restored = render(&mut renderer, DisplayMode::Shaded);
    assert!(
        center(&shaded)
            .iter()
            .zip(center(&restored))
            .all(|(a, b)| a.abs_diff(b) < 15),
        "returning to Shaded must restore the imported material"
    );
    let oblique = Camera {
        yaw: 0.5,
        pitch: 0.35,
        ..camera
    };
    let without_lines = render_pixels(
        &gpu,
        &mut renderer,
        &scene,
        oblique,
        [128; 2],
        Settings::default(),
    );
    let with_lines = render_pixels(
        &gpu,
        &mut renderer,
        &scene,
        oblique,
        [128; 2],
        Settings {
            normals: true,
            normal_length: 0.25,
            ..Settings::default()
        },
    );
    capture("normal-vectors", &with_lines, [128; 2]);
    assert!(
        changed_pixels(&without_lines, &with_lines) > 5,
        "normal lines must extend visibly from the mesh in an oblique view"
    );
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn native_environment_resolution_transitions() {
    let gpu = device();
    let glossy = sphere(1.0, 0.08);
    let (mut camera, _) = Camera::restore(&serde_json::Value::Null);
    camera.fit(&glossy, 1.0);
    let mut renderer = gpu.renderer(&glossy);

    // The normal viewer starts with a 64-pixel procedural environment. Bundled
    // HDRIs use 256-pixel cubemaps, so switching either way must replace the
    // filtered maps and their mip chains on this same renderer.
    let mut previous = render_pixels(
        &gpu,
        &mut renderer,
        &glossy,
        camera,
        [128; 2],
        Settings::default(),
    );
    for lighting in [
        Lighting::StudioSmall08,
        Lighting::KiaraDawn,
        Lighting::Outdoor,
        Lighting::StudioSmall08,
    ] {
        let current = render_pixels(
            &gpu,
            &mut renderer,
            &glossy,
            camera,
            [128; 2],
            Settings {
                lighting,
                ..Settings::default()
            },
        );
        assert!(
            changed_pixels(&previous, &current) > 200,
            "switching to {lighting:?} must update model reflections"
        );
        previous = current;
    }

    for lighting in [Lighting::Outdoor, Lighting::KiaraDawn] {
        let current = render_pixels(
            &gpu,
            &mut renderer,
            &glossy,
            camera,
            [128; 2],
            Settings {
                lighting,
                skybox: true,
                ..Settings::default()
            },
        );
        assert!(
            changed_pixels(&previous, &current) > 1000,
            "switching to {lighting:?} must update the visible environment"
        );
        previous = current;
    }

    // Also change resolution before the initial environment's filtering has
    // settled, as when the user quickly selects another lighting preset.
    let mut renderer = gpu.renderer(&glossy);
    let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let target = RenderTarget::new(
        &gpu.device,
        RenderOutput {
            handle: TextureHandle(1),
            size: [128; 2],
            depth: true,
            revision: 1,
        },
    )
    .unwrap();
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    for lighting in [
        Lighting::Studio,
        Lighting::StudioSmall08,
        Lighting::KiaraDawn,
        Lighting::Outdoor,
        Lighting::KiaraDawn,
    ] {
        renderer
            .render(
                &mut gpu.context(&mut encoder),
                &target,
                camera,
                glossy.radius(),
                [0.0, 0.0, 0.0, 1.0],
                Settings {
                    lighting,
                    skybox: true,
                    ..Settings::default()
                },
            )
            .unwrap();
    }
    gpu.assert_valid(scope);
    let settled = render_pixels(
        &gpu,
        &mut renderer,
        &glossy,
        camera,
        [128; 2],
        Settings {
            lighting: Lighting::KiaraDawn,
            skybox: true,
            ..Settings::default()
        },
    );
    assert!(
        changed_pixels(&previous, &settled) < 300,
        "rapid preset changes must settle on the selected HDRI"
    );
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn native_embedded_hdri_lighting_and_lowered_horizon() {
    let gpu = device();
    let glossy = sphere(1.0, 0.08);
    let (mut camera, _) = Camera::restore(&serde_json::Value::Null);
    camera.fit(&glossy, 1.0);
    let mut renderer = gpu.renderer(&glossy);
    let studio_settings = Settings {
        lighting: Lighting::StudioSmall08,
        ..Settings::default()
    };
    let studio = render_pixels(
        &gpu,
        &mut renderer,
        &glossy,
        camera,
        [128; 2],
        studio_settings,
    );
    capture("studio-hdri-reflections", &studio, [128; 2]);
    let dawn_settings = Settings {
        lighting: Lighting::KiaraDawn,
        ..Settings::default()
    };
    let dawn = render_pixels(
        &gpu,
        &mut renderer,
        &glossy,
        camera,
        [128; 2],
        dawn_settings,
    );
    capture("dawn-hdri-reflections", &dawn, [128; 2]);
    assert!(
        changed_pixels(&studio, &dawn) > 200,
        "the bundled studio and dawn HDRIs must produce distinct reflections"
    );
    let dawn_sky = render_pixels(
        &gpu,
        &mut renderer,
        &glossy,
        camera,
        [128; 2],
        Settings {
            skybox: true,
            ..dawn_settings
        },
    );
    capture("dawn-hdri-skybox", &dawn_sky, [128; 2]);
    assert!(
        changed_pixels(&dawn, &dawn_sky) > 1000,
        "the bundled HDRI must also render as the skybox"
    );

    // An unlit surface isolates horizon changes from changes to reflections.
    // Its center stays in place while the visible environment rotates lower.
    let mut scene = overlap(gltf::material::AlphaMode::Opaque, 1.0);
    scene.primitives.truncate(1);
    let mut renderer = gpu.renderer(&scene);
    let camera = Camera {
        target: Vec3::ZERO,
        yaw: 0.0,
        pitch: 0.0,
        distance: 4.0,
    };
    let horizon_settings = Settings {
        skybox: true,
        horizon: 30.0,
        ..dawn_settings
    };
    let high = render_pixels(
        &gpu,
        &mut renderer,
        &scene,
        camera,
        [128; 2],
        horizon_settings,
    );
    let low = render_pixels(
        &gpu,
        &mut renderer,
        &scene,
        camera,
        [128; 2],
        Settings {
            horizon: -30.0,
            ..horizon_settings
        },
    );
    capture("horizon-raised", &high, [128; 2]);
    capture("horizon-lowered", &low, [128; 2]);
    assert!(
        changed_pixels(&high, &low) > 1000,
        "lowering the horizon must visibly change the environment background"
    );
    let center = (64 * 128 + 64) * 4;
    assert!(
        high[center..center + 3]
            .iter()
            .zip(&low[center..center + 3])
            .all(|(a, b)| a.abs_diff(*b) < 10),
        "lowering the horizon must preserve the model's position and unlit color"
    );
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn native_skybox_blur_preserves_reflections_and_survives_environment_switching() {
    let gpu = device();
    let glossy = sphere(1.0, 0.08);
    let (mut camera, _) = Camera::restore(&serde_json::Value::Null);
    camera.fit(&glossy, 1.0);
    let mut renderer = gpu.renderer(&glossy);
    let settings = Settings {
        lighting: Lighting::KiaraDawn,
        skybox: true,
        ..Settings::default()
    };
    // Warm shader compilation and environment filtering before comparison.
    render_pixels(&gpu, &mut renderer, &glossy, camera, [128; 2], settings);
    let sharp = render_pixels(&gpu, &mut renderer, &glossy, camera, [128; 2], settings);
    let soft = render_pixels(
        &gpu,
        &mut renderer,
        &glossy,
        camera,
        [128; 2],
        Settings {
            skybox_blur: 0.25,
            ..settings
        },
    );
    capture("skybox-soft", &soft, [128; 2]);
    let blurred_settings = Settings {
        skybox_blur: 0.75,
        ..settings
    };
    let blurred = render_pixels(
        &gpu,
        &mut renderer,
        &glossy,
        camera,
        [128; 2],
        blurred_settings,
    );
    capture("skybox-sharp", &sharp, [128; 2]);
    capture("skybox-blurred", &blurred, [128; 2]);
    assert!(
        changed_pixels(&sharp, &blurred) > 1000,
        "skybox blur must visibly change the HDRI background"
    );

    // Only sample the outer border, well away from the sphere and its edge.
    // Neighbor differences measure background detail independently of brightness.
    let background_detail = |pixels: &[u8]| {
        let mut detail = 0_u64;
        let background = |x: usize, y: usize| !(20..108).contains(&x) || !(20..108).contains(&y);
        for y in 0..127 {
            for x in 0..127 {
                if !background(x, y) {
                    continue;
                }
                let pixel = (y * 128 + x) * 4;
                for (nx, ny) in [(x + 1, y), (x, y + 1)] {
                    if background(nx, ny) {
                        let neighbor = (ny * 128 + nx) * 4;
                        for channel in 0..3 {
                            detail += u64::from(
                                pixels[pixel + channel].abs_diff(pixels[neighbor + channel]),
                            );
                        }
                    }
                }
            }
        }
        detail
    };
    let sharp_detail = background_detail(&sharp);
    let soft_detail = background_detail(&soft);
    let blurred_detail = background_detail(&blurred);
    assert!(
        blurred_detail * 4 < sharp_detail * 3,
        "blur must reduce background detail: sharp {sharp_detail}, blurred {blurred_detail}"
    );
    assert!(
        blurred_detail < soft_detail && soft_detail < sharp_detail,
        "intermediate blur must preserve more detail: sharp {sharp_detail}, soft {soft_detail}, blurred {blurred_detail}"
    );

    // The center lies entirely inside the glossy sphere. Blurring the skybox
    // must leave its PBR reflection image and sampler unchanged.
    let mut reflection_difference = 0_u64;
    for y in 48..80 {
        for x in 48..80 {
            let pixel = (y * 128 + x) * 4;
            for channel in 0..3 {
                reflection_difference +=
                    u64::from(sharp[pixel + channel].abs_diff(blurred[pixel + channel]));
            }
        }
    }
    assert!(
        reflection_difference < 32 * 32 * 3 * 3,
        "skybox blur must preserve model shading and reflections: total difference {reflection_difference}"
    );
    let restored = render_pixels(&gpu, &mut renderer, &glossy, camera, [128; 2], settings);
    capture("skybox-restored", &restored, [128; 2]);
    let restored_detail = background_detail(&restored);
    let restored_difference: u64 = sharp
        .as_chunks::<4>()
        .0
        .iter()
        .zip(restored.as_chunks::<4>().0)
        .map(|(a, b)| {
            (0..3)
                .map(|channel| u64::from(a[channel].abs_diff(b[channel])))
                .sum::<u64>()
        })
        .sum();
    // TAA jitter changes fine rock edges between captures. Check restoration
    // by its recovered detail and mean color error instead of pixel identity.
    assert!(
        restored_detail * 5 > sharp_detail * 4
            && restored_detail * 4 < sharp_detail * 5
            && restored_difference < 128 * 128 * 3 * 4,
        "zero blur must restore the sharp HDRI: sharp detail {sharp_detail}, restored {restored_detail}, total RGB difference {restored_difference}"
    );

    // Procedural cubemaps have seven mip levels; bundled HDRIs have nine.
    // Exercise both directions and slider limits on the same live renderer.
    for (lighting, skybox_blur) in [
        (Lighting::Outdoor, 1.0),
        (Lighting::StudioSmall08, 0.25),
        (Lighting::KiaraDawn, 1.0),
        (Lighting::Studio, 0.0),
        (Lighting::KiaraDawn, 0.75),
    ] {
        render_pixels(
            &gpu,
            &mut renderer,
            &glossy,
            camera,
            [128; 2],
            Settings {
                lighting,
                skybox_blur,
                ..settings
            },
        );
    }

    // Switch again before environment filtering finishes, as when the user
    // moves the blur slider while choosing a different HDRI.
    let mut renderer = gpu.renderer(&glossy);
    let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let target = RenderTarget::new(
        &gpu.device,
        RenderOutput {
            handle: TextureHandle(1),
            size: [128; 2],
            depth: true,
            revision: 1,
        },
    )
    .unwrap();
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    for (lighting, skybox_blur) in [
        (Lighting::Studio, 1.0),
        (Lighting::StudioSmall08, 0.5),
        (Lighting::Outdoor, 0.0),
        (Lighting::KiaraDawn, 1.0),
        (Lighting::KiaraDawn, 0.75),
    ] {
        renderer
            .render(
                &mut gpu.context(&mut encoder),
                &target,
                camera,
                glossy.radius(),
                [0.0, 0.0, 0.0, 1.0],
                Settings {
                    lighting,
                    skybox_blur,
                    ..settings
                },
            )
            .unwrap();
    }
    gpu.assert_valid(scope);
    let settled = render_pixels(
        &gpu,
        &mut renderer,
        &glossy,
        camera,
        [128; 2],
        blurred_settings,
    );
    assert!(
        changed_pixels(&blurred, &settled) < 300,
        "rapid HDRI and blur changes must settle on the selected background"
    );
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn native_model_environment_recovers_twice_in_the_same_world() {
    use bed_runtime::render::PreviewView;
    use bevy::{
        app::{AppLabel, SubApps},
        extract::ExtractPlugin,
        pbr::generate::{GeneratorBindGroups, IntermediateTextures, RenderEnvironmentMap},
        prelude::{App, Camera3d, Component, Entity, Mesh3d, Or, Resource, With},
        render::{
            RenderApp,
            error_handler::{ErrorType, RenderErrorHandler, RenderErrorPolicy},
            settings::RenderCreation,
        },
    };

    #[derive(Resource, Default)]
    struct Recovery {
        lost: bool,
        replacement: Option<RenderCreation>,
    }

    #[derive(Component)]
    struct GameplayState;

    fn register_extra_target(app: &mut App, gpu: &TestGpu) {
        use bevy::{
            math::UVec2,
            render::texture::{ManualTextureView, ManualTextureViews},
        };
        let target = RenderTarget::new(
            &gpu.device,
            RenderOutput {
                handle: TextureHandle::next(),
                size: [32; 2],
                depth: false,
                revision: 1,
            },
        )
        .unwrap();
        app.world_mut().resource_mut::<ManualTextureViews>().insert(
            bevy::camera::ManualTextureViewHandle(u32::MAX),
            ManualTextureView {
                texture_view: target
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor {
                        format: Some(wgpu::TextureFormat::Rgba8UnormSrgb),
                        ..Default::default()
                    })
                    .into(),
                view_format: wgpu::TextureFormat::Rgba8UnormSrgb,
                size: UVec2::splat(32),
            },
        );
    }

    fn lifecycle(app: &mut App) {
        let lifecycle = app.get_added_plugins::<ExtractPlugin<RenderApp>>()[0].pre_extract;
        let SubApps { main, sub_apps } = app.sub_apps_mut();
        lifecycle(
            main.world_mut(),
            sub_apps.get_mut(&RenderApp.intern()).unwrap().world_mut(),
        );
    }

    let mut gpu = device();
    let scene =
        super::model::load(include_bytes!("../../../tests/fixtures/gltf/cube.glb")).unwrap();
    let mut renderer = gpu.renderer(&scene);
    renderer
        .app
        .init_resource::<Recovery>()
        .insert_resource(RenderErrorHandler(|error, world, _| {
            let mut recovery = world.resource_mut::<Recovery>();
            if let Some(replacement) = recovery.replacement.take() {
                return RenderErrorPolicy::Recover(replacement);
            }
            assert_eq!(error.ty, ErrorType::DeviceLost);
            recovery.lost = true;
            RenderErrorPolicy::StopRendering
        }));
    let game = renderer.app.world_mut().spawn(GameplayState).id();
    renderer
        .app
        .insert_resource(bevy::light::PointLightShadowMap { size: 64 })
        .insert_resource(bevy::light::DirectionalLightShadowMap { size: 64 });
    let point_light = renderer
        .app
        .world_mut()
        .spawn((
            bevy::light::PointLight {
                shadow_maps_enabled: true,
                ..Default::default()
            },
            bevy::transform::components::Transform::from_xyz(2.0, 3.0, 4.0),
        ))
        .id();
    let spot_light = renderer
        .app
        .world_mut()
        .spawn((
            bevy::light::SpotLight {
                shadow_maps_enabled: true,
                ..Default::default()
            },
            bevy::transform::components::Transform::from_xyz(3.0, 4.0, 5.0),
        ))
        .id();
    // A persistent camera that was rendered, then becomes inactive, retains
    // per-view buffers that Bevy 0.20 still visits during GPU uploads.
    register_extra_target(&mut renderer.app, &gpu);
    let inactive_camera = renderer
        .app
        .world_mut()
        .spawn((
            Camera3d::default(),
            bevy::pbr::ScreenSpaceAmbientOcclusion::default(),
            bevy::camera::RenderTarget::TextureView(bevy::camera::ManualTextureViewHandle(
                u32::MAX,
            )),
            bevy::render::view::Msaa::Off,
        ))
        .id();
    let (mut camera, _) = Camera::restore(&serde_json::Value::Null);
    camera.fit(&scene, 1.0);
    let settings = Settings {
        lighting: Lighting::KiaraDawn,
        display: DisplayMode::WireframeOverlay,
        normals: true,
        normal_length: 0.2,
        skybox: true,
        skybox_blur: 0.65,
        ao: true,
        shadows: false,
        ..Settings::default()
    };
    // Populate the extra camera's GPU caches, then compare model pixels with
    // the same active-view set used after recovery.
    render_pixels(&gpu, &mut renderer, &scene, camera, [64; 2], settings);
    renderer
        .app
        .world_mut()
        .get_mut::<bevy::camera::Camera>(inactive_camera)
        .unwrap()
        .is_active = false;
    let shaded = render_pixels(
        &gpu,
        &mut renderer,
        &scene,
        camera,
        [64; 2],
        Settings {
            display: DisplayMode::Shaded,
            normals: false,
            ..settings
        },
    );
    let before = render_pixels(&gpu, &mut renderer, &scene, camera, [64; 2], settings);
    capture("recovery-before", &before, [64; 2]);
    assert!(
        changed_pixels(&before, &shaded) > 20,
        "recovery baseline must contain rendered debug geometry"
    );
    let cluster_buffers = renderer
        .app
        .sub_app(RenderApp)
        .world()
        .components()
        .iter_registered()
        .find(|(_, info)| &*info.name() == "bevy_pbr::cluster::gpu::ViewGpuClusteringBuffers")
        .map(|(id, _)| id)
        .expect("native recovery fixture requires GPU clustering and component names");
    let render_camera = **renderer
        .app
        .world()
        .get::<bevy::render::sync_world::RenderEntity>(inactive_camera)
        .unwrap();
    assert!(
        renderer
            .app
            .sub_app(RenderApp)
            .world()
            .entity(render_camera)
            .contains_id(cluster_buffers)
    );
    assert!(
        renderer
            .app
            .sub_app(RenderApp)
            .world()
            .get::<bevy::pbr::SsaoBindGroups>(render_camera)
            .is_some(),
        "native recovery fixture must prepare SSAO on its persistent camera"
    );
    let shadow_views: Vec<_> = renderer
        .app
        .sub_app_mut(RenderApp)
        .world_mut()
        .query::<&bevy::pbr::PointAndSpotLightViewEntities>()
        .iter(renderer.app.sub_app(RenderApp).world())
        .flat_map(|views| views.iter().copied())
        .collect();
    assert_eq!(
        shadow_views.len(),
        7,
        "point and spot shadow views must exist"
    );
    assert!(shadow_views.iter().all(|view| {
        renderer
            .app
            .sub_app(RenderApp)
            .world()
            .get::<bevy::pbr::ShadowView>(*view)
            .is_some()
    }));
    let entities: Vec<_> = renderer
        .app
        .world_mut()
        .query_filtered::<Entity, With<PreviewView>>()
        .iter(renderer.app.world())
        .collect();
    assert!(!entities.is_empty());
    assert!(
        !renderer
            .app
            .world_mut()
            .query_filtered::<Entity, With<Mesh3d>>()
            .iter(renderer.app.world())
            .collect::<Vec<_>>()
            .is_empty()
    );
    assert!(
        !renderer
            .app
            .sub_app_mut(RenderApp)
            .world_mut()
            .query::<&GeneratorBindGroups>()
            .iter(renderer.app.sub_app(RenderApp).world())
            .collect::<Vec<_>>()
            .is_empty(),
        "environment filtering never became ready"
    );

    for generation in 2..=3 {
        renderer
            .app
            .world_mut()
            .get_mut::<bevy::camera::Camera>(inactive_camera)
            .unwrap()
            .is_active = false;
        renderer.app.update();
        assert!(
            renderer
                .app
                .sub_app(RenderApp)
                .world()
                .entity(render_camera)
                .contains_id(cluster_buffers),
            "inactive camera did not retain the buffer needed to reproduce the bug"
        );
        assert!(
            renderer
                .app
                .sub_app(RenderApp)
                .world()
                .get::<bevy::pbr::ScreenSpaceAmbientOcclusionResources>(render_camera)
                .is_some(),
            "inactive camera did not retain SSAO textures needed to reproduce the bug"
        );
        renderer.app.world_mut().resource_mut::<Recovery>().lost = false;
        gpu.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(30)),
            })
            .unwrap();
        gpu.device.destroy();
        gpu.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(30)),
            })
            .unwrap();
        lifecycle(&mut renderer.app);
        assert!(renderer.app.world().resource::<Recovery>().lost);

        // Rendering is stopped, but Bevy still extracts each application
        // frame. Environment-map commands now retain the old GPU images.
        // These commands must be drained before recovery removes GPU state,
        // rather than resurrecting it on the first ready render frame.
        renderer.app.update();
        renderer.app.update();

        let mut replacement = device();
        replacement.generation = generation;
        renderer
            .app
            .world_mut()
            .resource_mut::<Recovery>()
            .replacement = Some(replacement.render_resources().into());
        // Select recovery, install the manual GPU resources, then run RenderStartup.
        lifecycle(&mut renderer.app);
        lifecycle(&mut renderer.app);
        lifecycle(&mut renderer.app);
        let world = renderer.app.sub_app_mut(RenderApp).world_mut();
        // This is the deferred-command step at the start of the ready render
        // schedule. Assert before GPU preparation, so a stale texture produces
        // a useful failure instead of a native backend panic.
        world.schedule_scope(bevy::render::ExtractSchedule, |world, schedule| {
            schedule.apply_deferred(world);
        });
        assert!(
            !world.entity(render_camera).contains_id(cluster_buffers),
            "old-device GPU clustering buffers survived on an inactive camera after RenderStartup"
        );
        assert!(
            world
                .get::<bevy::pbr::ScreenSpaceAmbientOcclusionResources>(render_camera)
                .is_none()
                && world
                    .get::<bevy::core_pipeline::prepass::ViewPrepassTextures>(render_camera)
                    .is_none()
                && world
                    .get::<bevy::pbr::SsaoBindGroups>(render_camera)
                    .is_none(),
            "old-device SSAO or prepass GPU state survived on an inactive camera after RenderStartup"
        );
        assert!(
            world
                .query_filtered::<Entity, Or<(
                    With<RenderEnvironmentMap>,
                    With<IntermediateTextures>,
                    With<GeneratorBindGroups>,
                )>>()
                .iter(world)
                .next()
                .is_none(),
            "old-device light-probe components survived RenderStartup"
        );
        assert!(
            shadow_views
                .iter()
                .all(|view| world.get::<bevy::pbr::ShadowView>(*view).is_none())
        );

        gpu = replacement;
        // No 3D view can rebuild attachments during these ready frames. The
        // point/spot request must survive until a view becomes active again.
        let world = renderer.app.world_mut();
        for mut view in world
            .query_filtered::<&mut bevy::camera::Camera, With<Camera3d>>()
            .iter_mut(world)
        {
            view.is_active = false;
        }
        let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
        renderer.app.update();
        renderer.app.update();
        gpu.assert_valid(scope);
        let after = render_pixels(&gpu, &mut renderer, &scene, camera, [64; 2], settings);
        capture(&format!("recovery-after-{generation}"), &after, [64; 2]);
        let difference: usize = before
            .iter()
            .zip(&after)
            .map(|(a, b)| usize::from(a.abs_diff(*b)))
            .sum();
        assert!(
            difference < before.len() * 3,
            "generation {generation} changed model lighting or skybox pixels: total difference {difference}"
        );
        assert!(
            entities
                .iter()
                .all(|entity| renderer.app.world().get_entity(*entity).is_ok())
        );
        assert!(renderer.app.world().get::<GameplayState>(game).is_some());
        assert!(renderer.app.world().get_entity(point_light).is_ok());
        assert!(renderer.app.world().get_entity(spot_light).is_ok());
        assert!(renderer.app.world().get_entity(inactive_camera).is_ok());
        assert!(
            !renderer
                .app
                .world()
                .get::<bevy::camera::Camera>(inactive_camera)
                .unwrap()
                .is_active
        );
        // Restore this same camera on a current-device target, proving that
        // normal preparation can rebuild its removed GPU state.
        register_extra_target(&mut renderer.app, &gpu);
        renderer
            .app
            .world_mut()
            .get_mut::<bevy::camera::Camera>(inactive_camera)
            .unwrap()
            .is_active = true;
        let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
        renderer.app.update();
        gpu.assert_valid(scope);
        assert!(
            shadow_views.iter().all(|view| renderer
                .app
                .sub_app(RenderApp)
                .world()
                .get::<bevy::pbr::ShadowView>(*view)
                .is_some()),
            "point and spot shadows must rebuild on their existing view entities when their view is active"
        );
        assert!(
            renderer
                .app
                .sub_app(RenderApp)
                .world()
                .entity(render_camera)
                .contains_id(cluster_buffers)
        );
    }
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn native_shared_views_isolate_lighting_assets_and_closure() {
    use bed_runtime::render::{PreviewView, begin_preview_frame, retire_previews};
    use bevy::prelude::{Entity, With};
    let gpu = device();
    let bytes = include_bytes!("../../../tests/fixtures/gltf/cube.glb");
    let scene = super::model::load(bytes).unwrap();
    let mut first = gpu.renderer(&scene);
    let first_id = first
        .app
        .world_mut()
        .query_filtered::<&PreviewView, With<bevy::camera::Camera>>()
        .single(first.app.world())
        .unwrap()
        .0;
    let second_id = TextureHandle::next().0;
    let prepared = super::prepare::load(bytes).unwrap();
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    let mut context = gpu.context(&mut encoder);
    context.world = Some(first.app.world_mut());
    let mut second = SceneState::new(
        &mut context,
        &super::model::PreparedModel::Gltf(prepared),
        second_id,
    )
    .unwrap();
    let target = |id| {
        RenderTarget::new(
            &gpu.device,
            RenderOutput {
                handle: TextureHandle(id),
                size: [64; 2],
                depth: true,
                revision: 1,
            },
        )
        .unwrap()
    };
    let targets = [target(first_id), target(second_id)];
    let (mut camera, _) = Camera::restore(&serde_json::Value::Null);
    camera.fit(&scene, 1.0);
    let stationary = Settings {
        lighting: Lighting::Neutral,
        skybox: true,
        skybox_blur: 0.4,
        ao: false,
        ..Settings::default()
    };
    let render_both = |first: &mut SceneGpu, second: &mut SceneState, settings: Settings| {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut settled = 0;
        while settled < SETTLE_FRAMES + 8 {
            begin_preview_frame(first.app.world_mut());
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            let mut context = gpu.context(&mut encoder);
            context.world = Some(first.app.world_mut());
            first
                .state
                .sync(
                    &mut context,
                    &targets[0],
                    camera,
                    scene.radius(),
                    [0.0, 0.0, 0.0, 1.0],
                    settings,
                )
                .unwrap();
            second
                .sync(
                    &mut context,
                    &targets[1],
                    camera,
                    scene.radius(),
                    [0.0, 0.0, 0.0, 1.0],
                    stationary,
                )
                .unwrap();
            first.app.update();
            if first.state.pending() || second.pending() {
                assert!(Instant::now() < deadline, "shared models did not settle");
                std::thread::sleep(Duration::from_millis(1));
            } else {
                settled += 1;
            }
        }
    };
    render_both(
        &mut first,
        &mut second,
        Settings {
            skybox: true,
            ..Settings::default()
        },
    );
    let before_first = capture_target(&gpu, &targets[0]);
    let before_second = capture_target(&gpu, &targets[1]);
    render_both(
        &mut first,
        &mut second,
        Settings {
            lighting: Lighting::Outdoor,
            skybox: true,
            skybox_blur: 0.85,
            ao: false,
            ..Settings::default()
        },
    );
    let after_first = capture_target(&gpu, &targets[0]);
    let after_second = capture_target(&gpu, &targets[1]);
    assert_ne!(
        before_first, after_first,
        "changing one view must update its lighting"
    );
    let difference: usize = before_second
        .iter()
        .zip(&after_second)
        .map(|(a, b)| usize::from(a.abs_diff(*b)))
        .sum();
    assert!(
        difference < before_second.len(),
        "another view's lighting or blur leaked into the stationary output"
    );
    retire_previews(
        first.app.world_mut(),
        &std::collections::BTreeSet::from([second_id]),
    );
    assert!(
        first
            .app
            .world_mut()
            .query::<&PreviewView>()
            .iter(first.app.world())
            .all(|owner| owner.0 == second_id)
    );
    assert!(
        !first
            .app
            .world_mut()
            .query_filtered::<Entity, With<bevy::camera::Camera>>()
            .iter(first.app.world())
            .collect::<Vec<_>>()
            .is_empty()
    );
}
