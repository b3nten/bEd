//! Pixel assertions on the real GPU; opt in on machines with a native adapter.
use super::{
    camera::Camera,
    model::{Material, Primitive, Scene, Vertex},
    render::SceneGpu,
};
use bed_plugin::{
    TextureHandle,
    gpu::{GpuContext, RenderOutput, RenderTarget, wgpu},
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
fn device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        block_on(instance.request_adapter(&Default::default())).expect("native GPU adapter");
    block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("glTF GPU test"),
        ..Default::default()
    }))
    .unwrap()
}
fn pixels(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene: &Scene,
    camera: Camera,
    size: [u32; 2],
) -> Vec<u8> {
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
    let mut gpu = GpuContext {
        device,
        queue,
        encoder: &mut encoder,
        generation: 1,
    };
    let renderer = SceneGpu::new(&gpu, scene).unwrap();
    renderer
        .render(
            &mut gpu,
            &target,
            camera,
            scene.radius(),
            [0.0, 0.0, 0.0, 1.0],
        )
        .unwrap();
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
    let mapped = buffer.slice(..).get_mapped_range().unwrap();
    let pixels = mapped
        .chunks_exact(stride as usize)
        .flat_map(|row| row[..size[0] as usize * 4].iter().copied())
        .collect();
    drop(mapped);
    buffer.unmap();
    pixels
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn native_textured_scene_camera_and_resized_output() {
    let (device, queue) = device();
    let scene =
        super::model::load(include_bytes!("../../../tests/fixtures/gltf/cube.glb")).unwrap();
    let (mut camera, _) = Camera::restore(&serde_json::Value::Null);
    camera.fit(&scene, 1.0);
    let before = pixels(&device, &queue, &scene, camera, [128; 2]);
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
    let after = pixels(&device, &queue, &scene, camera, [128; 2]);
    assert_ne!(
        before, after,
        "camera interaction must update rendered pixels"
    );
    let resized = pixels(&device, &queue, &scene, camera, [256, 128]);
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
    let (device, queue) = device();
    let (mut camera, _) = Camera::restore(&serde_json::Value::Null);
    camera.fit(&scene, 1.0);
    let values = pixels(&device, &queue, &scene, camera, [512; 2]);
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
                uv: [0.0; 2],
                color: [1.0; 4],
            })
            .to_vec(),
        indices: vec![0, 1, 2],
        center: Vec3::new(0.0, 0.0, z),
        material: Material {
            color,
            texture: None,
            wrap: [gltf::texture::WrappingMode::Repeat; 2],
            nearest: false,
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
    let (device, queue) = device();
    let camera = Camera {
        target: Vec3::ZERO,
        yaw: 0.0,
        pitch: 0.0,
        distance: 4.0,
    };
    let center = |scene: &Scene| {
        let values = pixels(&device, &queue, scene, camera, [64; 2]);
        values[(32 * 64 + 32) * 4..(32 * 64 + 32) * 4 + 4].to_vec()
    };
    assert_eq!(
        center(&overlap(gltf::material::AlphaMode::Opaque, 1.0)),
        [255, 0, 0, 255],
        "near mesh must occlude a later far mesh"
    );
    assert_eq!(
        center(&overlap(gltf::material::AlphaMode::Mask, 0.2)),
        [0, 0, 255, 255],
        "masked mesh must discard pixels below cutoff"
    );
    let blend = center(&overlap(gltf::material::AlphaMode::Blend, 0.5));
    assert!(
        (126..=129).contains(&blend[0]) && (126..=129).contains(&blend[2]) && blend[3] == 255,
        "transparent mesh must composite after opaque geometry: {blend:?}"
    );
}
