//! Meshes drawn with a per-frame transform (actors that move, animate or appear at run time).

use std::cell::RefCell;
use std::collections::HashMap;

use glam::Mat4;
use hp3_assets::loader::{Image, MaterialBlend, ObjKey};
use wgpu::util::DeviceExt;

use crate::gpu::{GpuScene, Renderer, DEPTH_FORMAT};
use crate::meshes::MeshGeom;
use crate::scene::{Camera, Vertex};

const SHADER: &str = r#"
struct Globals { view_proj: mat4x4<f32> };
// The model matrix and the light reaching the actor (halved, like vertex colors).
struct Model { model: mat4x4<f32>, light: vec4<f32> };
@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;
@group(2) @binding(0) var<uniform> m: Model;

struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex fn vs(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>, @location(2) light_uv: vec2<f32>,
              @location(3) color: vec3<f32>) -> VsOut {
    var o: VsOut;
    o.pos = globals.view_proj * m.model * vec4<f32>(position, 1.0);
    o.uv = uv;
    return o;
}

@fragment fn fs(i: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, i.uv);
    if (c.a < 0.5) { discard; }
    return vec4<f32>(c.rgb * m.light.rgb * 2.0, 1.0);
}

// Sections blended by their alpha (flame cards): nothing is cut out, the alpha goes to the blend.
@fragment fn fs_blend(i: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, i.uv);
    if (c.a < 0.01) { discard; }
    return vec4<f32>(c.rgb * m.light.rgb * 2.0, c.a);
}
"#;

/// Stride of one model matrix in the instance buffer (uniform offsets must be 256-aligned).
const STRIDE: u64 = 256;

struct MeshDraw {
    /// The `MeshGeom` batch this draw came from.
    batch: usize,
    bind: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
    blend: MaterialBlend,
}

/// A mesh uploaded once and drawn at any number of transforms.
pub struct GpuMesh {
    draws: Vec<MeshDraw>,
    local: Mat4,
}

pub struct Dynamic {
    pipeline: wgpu::RenderPipeline,
    /// Sections blended by alpha and added by alpha: depth-tested, not written.
    alpha_pipeline: wgpu::RenderPipeline,
    additive_pipeline: wgpu::RenderPipeline,
    model_layout: wgpu::BindGroupLayout,
    instances: RefCell<(wgpu::Buffer, wgpu::BindGroup, u64)>,
    images: RefCell<HashMap<ObjKey, wgpu::TextureView>>,
    posed: RefCell<Posed>,
}

/// Posed vertices waiting for the next draw: all of a frame's go up in one write and are copied into each mesh's
/// vertex buffer on the GPU. A write per mesh section costs little on Vulkan but a lot on OpenGL, where a frame with
/// eighty characters took 25 ms in writes alone.
#[derive(Default)]
struct Posed {
    bytes: Vec<u8>,
    /// (destination, offset in `bytes`, size)
    copies: Vec<(wgpu::Buffer, u64, u64)>,
    staging: Option<wgpu::Buffer>,
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
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(80),
                },
                count: None,
            }],
        });
        let layout = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("dynamic"),
            bind_group_layouts: &[r.globals_layout(), r.texture_layout(), &model_layout],
            push_constant_ranges: &[],
        });
        // `blend` None: opaque and cut-out sections (depth written); Some: blended with the given factors, depth only tested.
        let make = |label: &str, fs: &str, blend: Option<wgpu::BlendState>| {
            d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &crate::gpu::VERTEX_ATTRIBUTES,
                    }],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    targets: &[Some(wgpu::ColorTargetState { format: r.color_format, blend, write_mask: wgpu::ColorWrites::ALL })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: blend.is_none(),
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        };
        let pipeline = make("dynamic", "fs", None);
        let alpha_pipeline = make("dynamic alpha", "fs_blend", Some(wgpu::BlendState::ALPHA_BLENDING));
        let additive = wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::SrcAlpha, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent::OVER,
        };
        let additive_pipeline = make("dynamic additive", "fs_blend", Some(additive));
        let (buffer, bind) = Self::instance_buffer(r, &model_layout, 256);
        Dynamic { pipeline, alpha_pipeline, additive_pipeline, model_layout, instances: RefCell::new((buffer, bind, 256)), images: RefCell::new(HashMap::new()), posed: RefCell::default() }
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
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &buffer, offset: 0, size: wgpu::BufferSize::new(80) }),
            }],
        });
        (buffer, bind)
    }

    /// Uploads a mesh, sharing textures with earlier uploads.
    pub fn upload(&self, r: &Renderer, geom: &MeshGeom, images: &dyn Fn(&ObjKey) -> Option<std::rc::Rc<Image>>) -> GpuMesh {
        let mut draws = Vec::new();
        for (bi, b) in geom.batches.iter().enumerate() {
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
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
            let indices = r.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&b.indices),
                usage: wgpu::BufferUsages::INDEX,
            });
            draws.push(MeshDraw { batch: bi, bind, vertices, indices, count: b.indices.len() as u32, blend: b.blend });
        }
        GpuMesh { draws, local: geom.local }
    }

    /// Moves an uploaded skeletal mesh's vertices to posed `points` (indexed like the mesh's points), from the next draw on.
    pub fn write_points(&self, _r: &Renderer, mesh: &GpuMesh, geom: &MeshGeom, points: &[glam::Vec3]) {
        let mut posed = self.posed.borrow_mut();
        for d in &mesh.draws {
            let Some(b) = geom.batches.get(d.batch) else { continue };
            if b.points.len() != b.vertices.len() {
                continue;
            }
            let start = posed.bytes.len() as u64;
            for (v, &p) in b.vertices.iter().zip(&b.points) {
                let v = Vertex { position: points.get(p as usize).map(|p| p.to_array()).unwrap_or(v.position), ..*v };
                posed.bytes.extend_from_slice(bytemuck::bytes_of(&v));
            }
            let size = posed.bytes.len() as u64 - start;
            if size > 0 {
                posed.copies.push((d.vertices.clone(), start, size));
            }
        }
    }

    /// Sends the posed vertices written since the last draw: one write into a staging buffer, then a copy into each
    /// mesh, submitted ahead of the frame's commands.
    fn flush_posed(&self, r: &Renderer) {
        let mut posed = self.posed.borrow_mut();
        if posed.copies.is_empty() {
            posed.bytes.clear();
            return;
        }
        let needed = (posed.bytes.len() as u64).next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
        if posed.staging.as_ref().is_none_or(|s| s.size() < needed) {
            posed.staging = Some(r.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("posed vertices"),
                size: needed.next_power_of_two().max(1 << 16),
                usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        posed.bytes.resize(needed as usize, 0);
        let Posed { bytes, copies, staging } = &mut *posed;
        let staging = staging.as_ref().expect("made above");
        r.queue.write_buffer(staging, 0, bytes);
        let mut encoder = r.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("posed vertices") });
        for (dst, offset, size) in copies.drain(..) {
            encoder.copy_buffer_to_buffer(staging, offset, &dst, 0, size);
        }
        r.queue.submit([encoder.finish()]);
        bytes.clear();
    }

    /// Draws the static scene and the meshes at their transforms (actor space to world) in one pass.
    pub fn draw(
        &self,
        r: &Renderer,
        scene: &GpuScene,
        meshes: &[(&GpuMesh, Mat4, glam::Vec3)],
        camera: &Camera,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        aspect: f32,
    ) -> wgpu::CommandBuffer {
        self.draw_full(r, scene, meshes, None, camera, color, depth, aspect)
    }

    /// Like `draw`, with a frame of particles drawn last in the same pass.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_full(
        &self,
        r: &Renderer,
        scene: &GpuScene,
        meshes: &[(&GpuMesh, Mat4, glam::Vec3)],
        particles: Option<(&crate::particles::ParticleRenderer, &crate::particles::ParticleDraw)>,
        camera: &Camera,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        aspect: f32,
    ) -> wgpu::CommandBuffer {
        self.flush_posed(r);
        {
            let mut inst = self.instances.borrow_mut();
            if (meshes.len() as u64) > inst.2 {
                let cap = (meshes.len() as u64).next_power_of_two();
                let (b, g) = Self::instance_buffer(r, &self.model_layout, cap);
                *inst = (b, g, cap);
            }
            let mut data = vec![0u8; (meshes.len() as u64 * STRIDE) as usize];
            for (i, (mesh, world, light)) in meshes.iter().enumerate() {
                let m = (*world * mesh.local).to_cols_array();
                let off = i * STRIDE as usize;
                data[off..off + 64].copy_from_slice(bytemuck::cast_slice(&m));
                data[off + 64..off + 80].copy_from_slice(bytemuck::cast_slice(&light.extend(1.0).to_array()));
            }
            if !data.is_empty() {
                r.queue.write_buffer(&inst.0, 0, &data);
            }
        }
        let inst = self.instances.borrow();
        let sections = |pass: &mut wgpu::RenderPass<'_>, kinds: &[(MaterialBlend, &wgpu::RenderPipeline)]| {
            for &(blend, pipeline) in kinds {
                pass.set_pipeline(pipeline);
                for (i, (mesh, _, _)) in meshes.iter().enumerate() {
                    pass.set_bind_group(2, &inst.1, &[(i as u64 * STRIDE) as u32]);
                    for d in mesh.draws.iter().filter(|d| (d.blend == MaterialBlend::Cutout && blend == MaterialBlend::Opaque) || d.blend == blend) {
                        pass.set_bind_group(1, &d.bind, &[]);
                        pass.set_vertex_buffer(0, d.vertices.slice(..));
                        pass.set_index_buffer(d.indices.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..d.count, 0, 0..1);
                    }
                }
            }
        };
        r.draw_with(
            scene,
            camera,
            color,
            depth,
            aspect,
            // Opaque and cut-out sections go before the scene's blended surfaces, which test against their depth...
            |pass| sections(pass, &[(MaterialBlend::Opaque, &self.pipeline)]),
            // ...and the blended and added ones, then the particles, after them.
            |pass| {
                sections(pass, &[(MaterialBlend::Alpha, &self.alpha_pipeline), (MaterialBlend::Additive, &self.additive_pipeline)]);
                if let Some((renderer, frame)) = particles {
                    renderer.draw(pass, frame);
                }
            },
        )
    }
}
