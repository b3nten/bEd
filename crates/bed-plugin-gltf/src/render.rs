use crate::{
    camera::Camera,
    model::{Scene, Vertex},
};
use bed_plugin::gpu::{DEPTH_FORMAT, GpuContext, OUTPUT_FORMAT, RenderTarget, wgpu};
use wgpu::util::DeviceExt;

struct Mesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
    material: wgpu::BindGroup,
    pipeline: usize,
    center: glam::Vec3,
}
pub(super) struct SceneGpu {
    pub generation: u64,
    pipelines: Vec<wgpu::RenderPipeline>,
    camera_buffer: wgpu::Buffer,
    camera_bindings: wgpu::BindGroup,
    meshes: Vec<Mesh>,
}
impl SceneGpu {
    pub fn new(gpu: &GpuContext<'_>, scene: &Scene) -> Result<Self, String> {
        let device = gpu.device;
        let uniform_entry = |binding, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glTF camera layout"),
            entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT)],
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glTF material layout"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::FRAGMENT),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("glTF pipeline layout"),
            bind_group_layouts: &[Some(&camera_layout), Some(&material_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Bed glTF studio shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("scene.wgsl").into()),
        });
        let pipelines = (0..4)
            .map(|variant| {
                let blend = variant & 2 != 0;
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("Bed glTF mesh"),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &[Some(wgpu::VertexBufferLayout {
                            array_stride: std::mem::size_of::<Vertex>() as u64,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &wgpu::vertex_attr_array![
                                0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4
                            ],
                        })],
                    },
                    primitive: wgpu::PrimitiveState {
                        cull_mode: (variant & 1 == 0).then_some(wgpu::Face::Back),
                        ..Default::default()
                    },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: DEPTH_FORMAT,
                        depth_write_enabled: Some(!blend),
                        depth_compare: Some(wgpu::CompareFunction::LessEqual),
                        stencil: Default::default(),
                        bias: Default::default(),
                    }),
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some("fs_main"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: OUTPUT_FORMAT,
                            blend: blend.then_some(wgpu::BlendState::ALPHA_BLENDING),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            })
            .collect();
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("glTF camera"),
            size: 80,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("glTF camera"),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        let mut textures = Vec::new();
        for image in &scene.images {
            textures.push(gpu.upload_rgba(image.size, &image.rgba, true)?);
        }
        textures.push(gpu.upload_rgba([1, 1], &[255; 4], true)?);
        let wrap = |mode| match mode {
            gltf::texture::WrappingMode::ClampToEdge => wgpu::AddressMode::ClampToEdge,
            gltf::texture::WrappingMode::MirroredRepeat => wgpu::AddressMode::MirrorRepeat,
            gltf::texture::WrappingMode::Repeat => wgpu::AddressMode::Repeat,
        };
        let mut meshes = Vec::new();
        for primitive in &scene.primitives {
            let material = &primitive.material;
            let alpha = match material.alpha {
                gltf::material::AlphaMode::Opaque => 0.0,
                gltf::material::AlphaMode::Mask => 1.0,
                gltf::material::AlphaMode::Blend => 2.0,
            };
            let mut values = material.color.to_vec();
            values.extend([material.cutoff, f32::from(material.unlit), alpha, 0.0]);
            let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("glTF material"),
                contents: bytemuck::cast_slice(&values),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("glTF material sampling"),
                address_mode_u: wrap(material.wrap[0]),
                address_mode_v: wrap(material.wrap[1]),
                mag_filter: if material.nearest {
                    wgpu::FilterMode::Nearest
                } else {
                    wgpu::FilterMode::Linear
                },
                min_filter: if material.nearest {
                    wgpu::FilterMode::Nearest
                } else {
                    wgpu::FilterMode::Linear
                },
                ..Default::default()
            });
            let texture = &textures[material.texture.unwrap_or(textures.len() - 1)];
            let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("glTF material"),
                layout: &material_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniforms.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(
                            &texture.create_view(&Default::default()),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            });
            meshes.push(Mesh {
                vertices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("glTF vertices"),
                    contents: bytemuck::cast_slice(&primitive.vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                }),
                indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("glTF indices"),
                    contents: bytemuck::cast_slice(&primitive.indices),
                    usage: wgpu::BufferUsages::INDEX,
                }),
                count: primitive.indices.len() as u32,
                material: bindings,
                pipeline: usize::from(material.double_sided) | (usize::from(alpha == 2.0) << 1),
                center: primitive.center,
            });
        }
        Ok(Self {
            generation: gpu.generation,
            pipelines,
            camera_buffer,
            camera_bindings,
            meshes,
        })
    }
    pub fn render(
        &self,
        gpu: &mut GpuContext<'_>,
        target: &RenderTarget,
        camera: Camera,
        radius: f32,
        background: [f32; 4],
    ) -> Result<(), String> {
        let depth = target
            .depth
            .as_ref()
            .ok_or("glTF output requires a depth attachment")?;
        let mut values = camera
            .view_projection(target.size[0] as f32 / target.size[1] as f32, radius)
            .to_cols_array()
            .to_vec();
        values.extend(camera.eye().to_array());
        values.push(1.0);
        gpu.queue
            .write_buffer(&self.camera_buffer, 0, bytemuck::cast_slice(&values));
        let mut meshes: Vec<_> = self.meshes.iter().collect();
        let eye = camera.eye();
        meshes.sort_by(|a, b| {
            let a_blend = a.pipeline & 2 != 0;
            let b_blend = b.pipeline & 2 != 0;
            a_blend.cmp(&b_blend).then_with(|| {
                if a_blend {
                    eye.distance_squared(b.center)
                        .total_cmp(&eye.distance_squared(a.center))
                } else {
                    std::cmp::Ordering::Equal
                }
            })
        });
        let background = background.map(f64::from);
        let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Bed glTF output"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: background[0],
                        g: background[1],
                        b: background[2],
                        a: background[3],
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        pass.set_bind_group(0, &self.camera_bindings, &[]);
        for mesh in meshes {
            pass.set_pipeline(&self.pipelines[mesh.pipeline]);
            pass.set_bind_group(1, &mesh.material, &[]);
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.count, 0, 0..1);
        }
        Ok(())
    }
}
