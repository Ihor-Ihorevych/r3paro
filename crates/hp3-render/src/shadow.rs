//! Shadows of the moving lights (a wand's glow, spells): the level's opaque geometry seen from a light into a depth cube map
//! (six faces, one draw each from one buffer holding the whole level), which the world shader compares surface points with.
//! A light that has not moved since its map was made keeps it.

use std::cell::RefCell;

use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::gpu::{MovingLight, DEPTH_FORMAT};

/// How many moving lights get shadows (the nearest ones).
pub const MAX_SHADOW_LIGHTS: usize = 2;

/// The near plane of the light's cube faces; the shader's `shadow_visibility` uses the same.
const NEAR: f32 = 2.0;

const SHADER: &str = r#"
struct Face { view_proj: mat4x4<f32> };
@group(0) @binding(0) var<uniform> face: Face;
@vertex fn vs(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    return face.view_proj * vec4<f32>(position, 1.0);
}
"#;

/// Where each cube face looks and which way is up on its image, in the order of the cube map's layers (+x, -x, +y, -y, +z, -z).
const FACES: [(Vec3, Vec3); 6] = [
    (Vec3::X, Vec3::Y),
    (Vec3::NEG_X, Vec3::Y),
    (Vec3::Y, Vec3::NEG_Z),
    (Vec3::NEG_Y, Vec3::Z),
    (Vec3::Z, Vec3::Y),
    (Vec3::NEG_Z, Vec3::Y),
];

/// The level's opaque triangles, positions only, in one buffer.
pub struct ShadowGeom {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
}

impl ShadowGeom {
    pub fn new(device: &wgpu::Device, positions: &[[f32; 3]], indices: &[u32]) -> Option<ShadowGeom> {
        if indices.is_empty() {
            return None;
        }
        Some(ShadowGeom {
            vertices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("shadow geometry"), contents: bytemuck::cast_slice(positions), usage: wgpu::BufferUsages::VERTEX }),
            indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("shadow geometry"), contents: bytemuck::cast_slice(indices), usage: wgpu::BufferUsages::INDEX }),
            count: indices.len() as u32,
        })
    }
}

pub struct ShadowMaps {
    /// Group 2 of the world pipelines: the cube maps and their comparison sampler.
    pub layout: wgpu::BindGroupLayout,
    pub bind: wgpu::BindGroup,
    /// One view per face, `MAX_SHADOW_LIGHTS * 6` of them.
    faces: Vec<wgpu::TextureView>,
    face_buffers: Vec<wgpu::Buffer>,
    face_binds: Vec<wgpu::BindGroup>,
    pipeline: wgpu::RenderPipeline,
    /// What each slot's map was made for: the light's position and radius, and the scene it was made from.
    made: RefCell<[Option<(Vec3, f32, u64)>; MAX_SHADOW_LIGHTS]>,
    pub enabled: bool,
}

impl ShadowMaps {
    pub fn new(device: &wgpu::Device) -> ShadowMaps {
        let enabled = std::env::var("HP3_SHADOWS").map_or(true, |v| v != "0");
        let size: u32 = std::env::var("HP3_SHADOW_SIZE").ok().and_then(|v| v.parse().ok()).unwrap_or(256);
        let cube_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::Cube,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow maps"),
            entries: &[
                cube_entry(0),
                cube_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
            ],
        });
        let mut cubes = Vec::new();
        let mut cube_views = Vec::new();
        let mut faces = Vec::new();
        for _ in 0..MAX_SHADOW_LIGHTS {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("shadow cube"),
                size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 6 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            cube_views.push(tex.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::Cube), ..Default::default() }));
            for face in 0..6 {
                faces.push(tex.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: face,
                    array_layer_count: Some(1),
                    ..Default::default()
                }));
            }
            cubes.push(tex);
        }
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow maps"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&cube_views[0]) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&cube_views[1]) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        let face_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow face"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let mut face_buffers = Vec::new();
        let mut face_binds = Vec::new();
        for _ in 0..faces.len() {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("shadow face"), size: 64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
            face_binds.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &face_layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
            }));
            face_buffers.push(buffer);
        }
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("shadow"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("shadow"), bind_group_layouts: &[&face_layout], push_constant_ranges: &[] });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 12,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                }],
                compilation_options: Default::default(),
            },
            fragment: None,
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        ShadowMaps { layout, bind, faces, face_buffers, face_binds, pipeline, made: RefCell::new([None; MAX_SHADOW_LIGHTS]), enabled }
    }

    /// Makes the cube maps of `lights` (the lights that have a slot, in slot order) from `geom`, unless a slot's map is already
    /// that of the same light in the same place from the same scene.
    pub fn render(&self, queue: &wgpu::Queue, enc: &mut wgpu::CommandEncoder, geom: &ShadowGeom, scene: u64, lights: &[MovingLight]) {
        let mut made = self.made.borrow_mut();
        for (slot, light) in lights.iter().enumerate().take(MAX_SHADOW_LIGHTS) {
            if made[slot] == Some((light.position, light.radius, scene)) {
                continue;
            }
            made[slot] = Some((light.position, light.radius, scene));
            let projection = Mat4::perspective_lh(std::f32::consts::FRAC_PI_2, 1.0, NEAR, light.radius.max(NEAR + 1.0));
            for (face, (forward, up)) in FACES.iter().enumerate() {
                let index = slot * 6 + face;
                let matrix = projection * Mat4::look_to_lh(light.position, *forward, *up);
                queue.write_buffer(&self.face_buffers[index], 0, bytemuck::cast_slice(&matrix.to_cols_array()));
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("shadow"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.faces[index],
                        depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.face_binds[index], &[]);
                pass.set_vertex_buffer(0, geom.vertices.slice(..));
                pass.set_index_buffer(geom.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..geom.count, 0, 0..1);
            }
        }
    }
}
