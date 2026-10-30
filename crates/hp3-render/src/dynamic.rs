//! Meshes drawn with a per-frame transform (actors that move, animate or appear at run time).

use std::cell::RefCell;
use std::collections::HashMap;

use glam::Mat4;
use hp3_assets::loader::{Image, ObjKey};
use wgpu::util::DeviceExt;

use crate::gpu::{GpuScene, Renderer, DEPTH_FORMAT};
use crate::meshes::MeshGeom;
use crate::scene::{Camera, Vertex};

const SHADER: &str = r#"
struct Globals { view_proj: mat4x4<f32> };
struct Model { model: mat4x4<f32> };
@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;
@group(2) @binding(0) var<uniform> m: Model;

struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex fn vs(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>) -> VsOut {
    var o: VsOut;
    o.pos = globals.view_proj * m.model * vec4<f32>(position, 1.0);
    o.uv = uv;
    return o;
}

@fragment fn fs(i: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, i.uv);
    if (c.a < 0.5) { discard; }
    return vec4<f32>(c.rgb, 1.0);
}
"#;

/// Stride of one model matrix in the instance buffer (uniform offsets must be 256-aligned).
const STRIDE: u64 = 256;

struct MeshDraw {
    bind: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
}

/// A mesh uploaded once and drawn at any number of transforms.
pub struct GpuMesh {
    draws: Vec<MeshDraw>,
    local: Mat4,
}

pub struct Dynamic {
    pipeline: wgpu::RenderPipeline,
    model_layout: wgpu::BindGroupLayout,
    instances: RefCell<(wgpu::Buffer, wgpu::BindGroup, u64)>,
    images: RefCell<HashMap<ObjKey, wgpu::TextureView>>,
}

impl Dynamic {
    pub fn new(r: &Renderer) -> Dynamic {
        let d = &r.device;
        let shader = d.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("dynamic"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let model_layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("model"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(64),
                },
                count: None,
            }],
        });
        let layout = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("dynamic"),
            bind_group_layouts: &[r.globals_layout(), r.texture_layout(), &model_layout],
            push_constant_ranges: &[],
        });
        let pipeline = d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("dynamic"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                targets: &[Some(r.color_format.into())],
                compilation_options: Default::default(),
            }),
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
        let (buffer, bind) = Self::instance_buffer(r, &model_layout, 256);
        Dynamic { pipeline, model_layout, instances: RefCell::new((buffer, bind, 256)), images: RefCell::new(HashMap::new()) }
    }

    fn instance_buffer(r: &Renderer, layout: &wgpu::BindGroupLayout, capacity: u64) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = r.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instances"),
            size: capacity * STRIDE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = r.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &buffer, offset: 0, size: wgpu::BufferSize::new(64) }),
            }],
        });
        (buffer, bind)
    }

    /// Uploads a mesh, sharing textures with earlier uploads.
    pub fn upload(&self, r: &Renderer, geom: &MeshGeom, images: &dyn Fn(&ObjKey) -> Option<std::rc::Rc<Image>>) -> GpuMesh {
        let mut draws = Vec::new();
        for b in &geom.batches {
            if b.indices.is_empty() {
                continue;
            }
            let view = {
                let mut cache = self.images.borrow_mut();
                match &b.texture {
                    Some(k) => {
                        if !cache.contains_key(k)
                            && let Some(img) = images(k)
                        {
                            cache.insert(k.clone(), r.upload_view(img.width, img.height, &img.rgba));
                        }
                        cache.get(k).cloned()
                    }
                    None => None,
                }
            };
            let view = view.unwrap_or_else(|| r.upload_view(1, 1, &[200, 200, 200, 255]));
            let bind = r.texture_bind(&view);
            let vertices = r.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&b.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let indices = r.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&b.indices),
                usage: wgpu::BufferUsages::INDEX,
            });
            draws.push(MeshDraw { bind, vertices, indices, count: b.indices.len() as u32 });
        }
        GpuMesh { draws, local: geom.local }
    }

    /// Draws the static scene and the meshes at their transforms (actor space to world) in one pass.
    pub fn draw(
        &self,
        r: &Renderer,
        scene: &GpuScene,
        meshes: &[(&GpuMesh, Mat4)],
        camera: &Camera,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        aspect: f32,
    ) -> wgpu::CommandBuffer {
        {
            let mut inst = self.instances.borrow_mut();
            if (meshes.len() as u64) > inst.2 {
                let cap = (meshes.len() as u64).next_power_of_two();
                let (b, g) = Self::instance_buffer(r, &self.model_layout, cap);
                *inst = (b, g, cap);
            }
            let mut data = vec![0u8; (meshes.len() as u64 * STRIDE) as usize];
            for (i, (mesh, world)) in meshes.iter().enumerate() {
                let m = (*world * mesh.local).to_cols_array();
                let off = i * STRIDE as usize;
                data[off..off + 64].copy_from_slice(bytemuck::cast_slice(&m));
            }
            if !data.is_empty() {
                r.queue.write_buffer(&inst.0, 0, &data);
            }
        }
        let inst = self.instances.borrow();
        r.draw_with(scene, camera, color, depth, aspect, |pass| {
            pass.set_pipeline(&self.pipeline);
            for (i, (mesh, _)) in meshes.iter().enumerate() {
                pass.set_bind_group(2, &inst.1, &[(i as u64 * STRIDE) as u32]);
                for d in &mesh.draws {
                    pass.set_bind_group(1, &d.bind, &[]);
                    pass.set_vertex_buffer(0, d.vertices.slice(..));
                    pass.set_index_buffer(d.indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..d.count, 0, 0..1);
                }
            }
        })
    }
}
