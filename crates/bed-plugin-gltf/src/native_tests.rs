//! Pixel assertions on the real GPU; opt in on machines with a native adapter.
use super::{
    camera::Camera,
    model::{Image, Material, Primitive, Scene, TextureInfo, TextureSampler, Vertex},
    render::{Lighting, SETTLE_FRAMES, SceneGpu, Settings},
};
use bed_plugin::{
    TextureHandle,
    gpu::{GpuContext, RenderOutput, RenderTarget, renderer_device_descriptor, wgpu},
};
use glam::Vec3;
use std::{
    future::Future,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
    time::Duration,
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
}
impl TestGpu {
    fn context<'a>(&'a self, encoder: &'a mut wgpu::CommandEncoder) -> GpuContext<'a> {
        GpuContext {
            instance: &self.instance,
            adapter: &self.adapter,
            device: &self.device,
            queue: &self.queue,
            encoder,
            generation: 1,
            error_handlers: None,
        }
    }

    fn renderer(&self, scene: &Scene) -> SceneGpu {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let renderer = SceneGpu::new(&self.context(&mut encoder), scene).unwrap();
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
    let TestGpu { device, queue, .. } = gpu;
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
    for _ in 0..SETTLE_FRAMES + 4 {
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
    }
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
    let mapped = buffer.slice(..).get_mapped_range();
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
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../resources/models/LittlestTokyo.glb"),
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
        minimum: Vec3::new(-1.0, -1.0, 0.0),
        maximum: Vec3::ONE,
        nodes: 2,
        animations: 0,
        draco_primitives: 0,
        posed_meshes: 0,
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
    a.chunks_exact(4)
        .zip(b.chunks_exact(4))
        .filter(|(a, b)| (0..3).any(|channel| a[channel].abs_diff(b[channel]) > 10))
        .count()
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
            .chunks_exact(4)
            .any(|pixel| pixel[0] > 30 && pixel[1] > 30)
    );
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn native_stl_renders_in_shared_bevy_viewer() {
    let scene = super::stl::load(
        br#"solid tetrahedron
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
"#,
    )
    .unwrap();
    assert_eq!(scene.triangles(), 4);
    let gpu = device();
    let (mut camera, _) = Camera::restore(&serde_json::Value::Null);
    camera.fit(&scene, 1.0);
    let values = pixels(&gpu, &scene, camera, [128; 2]);
    assert!(
        values
            .chunks_exact(4)
            .filter(|pixel| pixel[..3].iter().all(|channel| *channel > 20))
            .count()
            > 1000,
        "STL geometry must render with the shared lit material"
    );
}
