//! Headless GPU timings for the same postprocessing used by native viewports.
use bed_effects::{
    shader_manager::{ShaderManager, ShaderSettings},
    shader_types::OFFSCREEN_FORMAT,
};
use std::{error::Error, future::Future, sync::Arc, task::Wake, time::Duration};

fn block_on<T>(future: impl Future<Output = T>) -> T {
    struct ThreadWake(std::thread::Thread);
    impl Wake for ThreadWake {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = std::task::Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let mut context = std::task::Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            std::task::Poll::Ready(value) => return value,
            std::task::Poll::Pending => std::thread::park(),
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let width: u32 = args.next().as_deref().unwrap_or("3024").parse()?;
    let height: u32 = args.next().as_deref().unwrap_or("1964").parse()?;
    let profile = args.next();
    if width == 0 || height == 0 || args.next().is_some() {
        return Err(
            "Usage: profile_effects [WIDTH HEIGHT [PROFILE.json]] (physical pixels, both positive)"
                .into(),
        );
    }
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        ..Default::default()
    }))?;
    let features = wgpu::Features::TIMESTAMP_QUERY;
    if !adapter.features().contains(features) {
        return Err("This adapter does not support GPU timestamps".into());
    }
    let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: features,
        ..Default::default()
    }))?;
    println!(
        "Adapter: {} ({:?})",
        adapter.get_info().name,
        adapter.get_info().backend
    );
    println!("Physical resolution: {width} x {height}; sRGB output");
    println!("Postprocessing only; excludes UI, presentation, CPU time and power consumption.");

    let format = OFFSCREEN_FORMAT.add_srgb_suffix();
    let mut manager = ShaderManager::new(&device, width, height, format);
    // High-frequency text-like marks over a dark background, uploaded once.
    let pixels: Vec<u8> = (0..height)
        .flat_map(|y| {
            (0..width).flat_map(move |x| {
                let value = if y % 24 < 14 && x % 14 < 8 && x % 200 < 150 {
                    180
                } else {
                    24
                };
                [value, value, value, 255]
            })
        })
        .collect();
    queue.write_texture(
        manager.fb.texture.as_image_copy(),
        &pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        manager.fb.texture.size(),
    );
    let output = device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("Profile output"),
            size: manager.fb.texture.size(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default());
    const FRAMES: u32 = 60;
    let queries = device.create_query_set(&wgpu::QuerySetDescriptor {
        label: Some("Profile timestamps"),
        ty: wgpu::QueryType::Timestamp,
        count: FRAMES * 2,
    });
    let size = u64::from(FRAMES) * 2 * 8;
    let resolved = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Profile resolved timestamps"),
        size,
        usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Profile timestamp readback"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let sharp = ShaderSettings::subtle();
    let legacy = ShaderSettings::legacy();
    let mut cases = vec![
        (
            "Off",
            ShaderSettings {
                enabled: false,
                ..sharp
            },
        ),
        (
            "Sharp without bloom",
            ShaderSettings {
                bloom_intensity: 0.0,
                ..sharp
            },
        ),
        ("Sharp", sharp),
        (
            "Legacy without bloom",
            ShaderSettings {
                bloom_intensity: 0.0,
                ..legacy
            },
        ),
        ("Legacy", legacy),
    ];
    if let Some(path) = profile {
        let profile: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
        let settings = ShaderSettings::from_json(&profile);
        cases.push((
            "Profile without bloom",
            ShaderSettings {
                bloom_intensity: 0.0,
                ..settings
            },
        ));
        cases.push(("Profile", settings));
    }
    for (name, settings) in cases {
        manager.invalidate_history();
        let mut samples = Vec::new();
        // Discard one warmup batch, then measure 300 frames. Queue time and
        // readback waits are outside the GPU timestamp interval.
        for batch in 0..6 {
            let mut encoder = device.create_command_encoder(&Default::default());
            for frame in 0..FRAMES {
                manager.timestamp_next_frame(&queries, frame * 2);
                manager.render_with_effects(&mut encoder, &queue, &output, 2.0, &settings);
            }
            encoder.resolve_query_set(&queries, 0..FRAMES * 2, &resolved, 0);
            encoder.copy_buffer_to_buffer(&resolved, 0, &readback, 0, size);
            queue.submit([encoder.finish()]);
            let (sender, receiver) = std::sync::mpsc::channel();
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let _ = sender.send(result);
                });
            device.poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(30)),
            })?;
            receiver.recv_timeout(Duration::from_secs(30))??;
            let data = readback.slice(..).get_mapped_range()?;
            if batch > 0 {
                for pair in data.as_chunks::<16>().0 {
                    let start = u64::from_ne_bytes(pair[..8].try_into()?);
                    let end = u64::from_ne_bytes(pair[8..].try_into()?);
                    samples
                        .push((end - start) as f64 * f64::from(queue.get_timestamp_period()) / 1e6);
                }
            }
            drop(data);
            readback.unmap();
        }
        samples.sort_by(f64::total_cmp);
        let median = samples[samples.len() / 2];
        let p95 = samples[samples.len() * 95 / 100];
        println!("{name:22} median {median:7.3} ms  p95 {p95:7.3} ms  (300 frames)");
    }
    Ok(())
}
