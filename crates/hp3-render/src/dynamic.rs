//! Meshes drawn with a per-frame transform (actors that move, animate or appear at run time).

use std::cell::RefCell;
use std::collections::HashMap;

use glam::Mat4;
use hp3_assets::loader::{Image, MaterialBlend, ObjKey};
use wgpu::util::DeviceExt;

use crate::gpu::{GpuScene, Renderer, DEPTH_FORMAT};
use crate::meshes::MeshGeom;
use crate::scene::{Camera, Vertex};

/// Posed vertex bytes uploaded and actor draw calls made since the last `take_draw_counts` (for the perf log).
static POSED_BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static ACTOR_DRAWS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static UPLOAD_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// (posed vertex bytes, actor draw calls, nanoseconds spent uploading posed vertices) since the last call.
pub fn take_draw_counts() -> (u64, u64, u64) {
    use std::sync::atomic::Ordering::Relaxed;
    (POSED_BYTES.swap(0, Relaxed), ACTOR_DRAWS.swap(0, Relaxed), UPLOAD_NS.swap(0, Relaxed))
}

const SHADER: &str = r#"
struct Globals { view_proj: mat4x4<f32>, fog_color: vec4<f32>, fog: vec4<f32> };
// How much of the picture's own color is left at a fragment (1 outside the fog): linear fog by depth from `fog.x` to `fog.y`.
fn fog_factor(w: f32) -> f32 {
    if (globals.fog.z < 0.5) { return 1.0; }
    return clamp((globals.fog.y - 1.0 / w) / max(globals.fog.y - globals.fog.x, 1.0), 0.0, 1.0);
}
// One light as the original hands it to Direct3D: a = position and kind (0 point, 1 sun, 2 spot), b = direction and range,
// c = diffuse color and cos(phi/2), d = attenuation 0, 1, 2 and cos(theta/2).
struct Light { a: vec4<f32>, b: vec4<f32>, c: vec4<f32>, d: vec4<f32> };
// The model matrix, the actor's ambient light (w = how many lights), extra.x = how much of it is faded out (0 solid) and its lights.
struct Model { model: mat4x4<f32>, ambient: vec4<f32>, extra: vec4<f32>, lights: array<Light, 8> };
@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;
@group(2) @binding(0) var<uniform> m: Model;

struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32>, @location(1) light: vec3<f32>, @location(2) fade: f32 };

// Fixed-function style lighting per vertex (smooth, clamped to 0..1 before it modulates the texture, doubled).
@vertex fn vs(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>, @location(2) light_uv: vec2<f32>,
              @location(3) normal: vec3<f32>) -> VsOut {
    var o: VsOut;
    let wp = m.model * vec4<f32>(position, 1.0);
    o.pos = globals.view_proj * wp;
    o.uv = uv;
    o.fade = 1.0 - m.extra.x;
    let n = normalize((m.model * vec4<f32>(normal, 0.0)).xyz);
    var lit = m.ambient.rgb;
    let count = i32(m.ambient.w);
    for (var i = 0; i < count; i = i + 1) {
        let l = m.lights[i];
        let kind = u32(l.a.w);
        if (kind == 1u) {
            lit = lit + l.c.rgb * max(dot(n, -l.b.xyz), 0.0);
        } else {
            let to = l.a.xyz - wp.xyz;
            let d = length(to);
            if (d < l.b.w && d > 0.001) {
                let att = 1.0 / (l.d.x + l.d.y * d + l.d.z * d * d);
                let dir = to / d;
                var f = max(dot(n, dir), 0.0);
                if (kind == 2u) {
                    let rho = dot(-dir, l.b.xyz);
                    if (rho <= l.c.w) { f = 0.0; } else if (rho < l.d.w) { f = f * (rho - l.c.w) / (l.d.w - l.c.w); }
                }
                lit = lit + l.c.rgb * f * att;
            }
        }
    }
    o.light = clamp(lit, vec3<f32>(0.0), vec3<f32>(1.0));
    return o;
}

@fragment fn fs(i: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, i.uv);
    if (c.a < 0.5) { discard; }
    return vec4<f32>(mix(globals.fog_color.rgb, c.rgb * i.light * 2.0, fog_factor(i.pos.w)), 1.0);
}

// Sections blended by their alpha (flame cards): nothing is cut out, the alpha goes to the blend.
@fragment fn fs_blend(i: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, i.uv);
    if (c.a < 0.01) { discard; }
    return vec4<f32>(c.rgb * i.light * 2.0, c.a * i.fade * fog_factor(i.pos.w));
}
"#;

/// Stride of one model matrix in the instance buffer (uniform offsets must be 256-aligned).
const STRIDE: u64 = 768;

/// Bytes of one actor's uniform block: model matrix, ambient (with the light count) and eight lights.
const MODEL_BYTES: u64 = 64 + 16 + 16 + 8 * 64;


/// A soft shadow on the ground under a moving actor: a quad lying on the surface, darkest in the middle and fading out
/// to its edge (not part of the original game).
#[derive(Clone, Copy, Debug)]
pub struct BlobShadow {
    pub center: glam::Vec3,
    /// Half-extents of the quad as world vectors (the shadow is an ellipse inside it).
    pub axis_u: glam::Vec3,
    pub axis_v: glam::Vec3,
    /// Darkness in the middle, 0 to 1.
    pub opacity: f32,
}

const SHADOW_SHADER: &str = r#"
struct Globals { view_proj: mat4x4<f32>, fog_color: vec4<f32>, fog: vec4<f32> };
@group(0) @binding(0) var<uniform> globals: Globals;
struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32>, @location(1) opacity: f32 };
@vertex fn vs(@builtin(vertex_index) vi: u32, @location(0) c: vec4<f32>, @location(1) u: vec4<f32>, @location(2) v: vec4<f32>) -> VsOut {
    var corners = array<vec2<f32>, 4>(vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, 1.0));
    let q = corners[vi];
    var o: VsOut;
    o.pos = globals.view_proj * vec4<f32>(c.xyz + u.xyz * q.x + v.xyz * q.y, 1.0);
    o.uv = q;
    o.opacity = c.w;
    return o;
}
@fragment fn fs(i: VsOut) -> @location(0) vec4<f32> {
    let r = length(i.uv);
    if (r >= 1.0) { discard; }
    // A soft bell, fading to nothing at the edge (the output is blended in linear light, which makes a shadow look lighter, so it is strong).
    let s = 1.0 - r * r;
    var a = i.opacity * s * (0.75 + 0.25 * s);
    if (globals.fog.z > 0.5) {
        a = a * clamp((globals.fog.y - 1.0 / i.pos.w) / max(globals.fog.y - globals.fog.x, 1.0), 0.0, 1.0);
    }
    return vec4<f32>(0.0, 0.0, 0.0, a);
}
"#;

/// Bytes of one shadow in the instance buffer: centre and opacity, then the two axes.
const SHADOW_BYTES: u64 = 48;
const SHADOW_CAPACITY: u64 = 256;
struct MeshDraw {
    /// The `MeshGeom` batch this draw came from.
    batch: usize,
    bind: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
    blend: MaterialBlend,
    /// A skeletal mesh's vertices in its latest pose (empty until it is first posed): drawn from the frame's posed buffer.
    posed: RefCell<Vec<u8>>,
}

/// One light on an actor, fitted the way the original's Direct3D renderer gives it to the hardware.
#[derive(Clone, Copy, Debug, Default)]
pub struct ShaderLight {
    /// 0 point, 1 sunlight, 2 spotlight.
    pub kind: u32,
    pub position: glam::Vec3,
    /// A sun's or spotlight's direction (the way the light shines).
    pub direction: glam::Vec3,
    /// Diffuse color (already faded and scaled).
    pub color: glam::Vec3,
    /// Direct3D attenuation factors: the light reaches a vertex at 1 / (a0 + a1 d + a2 d^2).
    pub atten: [f32; 3],
    pub range: f32,
    /// Spotlight cones: cosine of half the outer and of half the inner angle.
    pub cos_phi: f32,
    pub cos_theta: f32,
}

/// What lights an actor: ambient light and up to eight lights.
#[derive(Clone, Debug, Default)]
pub struct ActorLight {
    pub ambient: glam::Vec3,
    pub lights: Vec<ShaderLight>,
    /// How much of the actor is faded out (0 solid, 1 gone): a faded actor is drawn blended, whatever its materials are.
    pub fade: f32,
}

/// A mesh uploaded once and drawn at any number of transforms.
pub struct GpuMesh {
    draws: Vec<MeshDraw>,
    pub local: Mat4,
    /// The middle of the mesh's bounding box in its own space: what the original lights it from (its picture sphere's centre).
    pub bounds_centre: glam::Vec3,
}

pub struct Dynamic {
    pipeline: wgpu::RenderPipeline,
    /// Sections blended by alpha and added by alpha: depth-tested, not written.
    alpha_pipeline: wgpu::RenderPipeline,
    additive_pipeline: wgpu::RenderPipeline,
    model_layout: wgpu::BindGroupLayout,
    instances: RefCell<(wgpu::Buffer, wgpu::BindGroup, u64)>,
    images: RefCell<HashMap<ObjKey, wgpu::TextureView>>,
    posed: RefCell<PosedRing>,
    shadow_pipeline: wgpu::RenderPipeline,
    shadow_buffer: wgpu::Buffer,
    shadow_count: std::cell::Cell<u32>,
}

/// The posed vertices of every skeletal mesh drawn in a frame go up in one write into one of three vertex buffers, used
/// in turn, and the draws read their part of it. Writing into a buffer the GPU may still be drawing from (each mesh's
/// own vertex buffer, as before) makes OpenGL drivers wait for the GPU: on a Mali phone that wait was 10 ms a frame.
#[derive(Default)]
struct PosedRing {
    buffers: [Option<wgpu::Buffer>; 3],
    next: usize,
    bytes: Vec<u8>,
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
                    min_binding_size: wgpu::BufferSize::new(MODEL_BYTES),
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
        let shadow_shader = d.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("blob shadow"), source: wgpu::ShaderSource::Wgsl(SHADOW_SHADER.into()) });
        let shadow_layout = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("blob shadow"), bind_group_layouts: &[r.globals_layout()], push_constant_ranges: &[] });
        let shadow_pipeline = d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blob shadow"),
            layout: Some(&shadow_layout),
            vertex: wgpu::VertexState {
                module: &shadow_shader,
                entry_point: Some("vs"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: SHADOW_BYTES,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shadow_shader,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState { format: r.color_format, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, cull_mode: None, ..Default::default() },
            // Tested against the picture, never written; pulled a little toward the viewer so the ground it lies on does not cut it.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: wgpu::DepthBiasState { constant: -4, slope_scale: -2.0, clamp: 0.0 },
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let shadow_buffer = d.create_buffer(&wgpu::BufferDescriptor { label: Some("blob shadows"), size: SHADOW_CAPACITY * SHADOW_BYTES, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let (buffer, bind) = Self::instance_buffer(r, &model_layout, 256);
        Dynamic { pipeline, alpha_pipeline, additive_pipeline, model_layout, instances: RefCell::new((buffer, bind, 256)), images: RefCell::new(HashMap::new()), posed: RefCell::default(), shadow_pipeline, shadow_buffer, shadow_count: std::cell::Cell::new(0) }
    }

    /// The soft shadows to draw this frame (replaces last frame's).
    pub fn set_shadows(&self, r: &Renderer, shadows: &[BlobShadow]) {
        let n = shadows.len().min(SHADOW_CAPACITY as usize);
        let mut data = Vec::with_capacity(n * 12);
        for s in &shadows[..n] {
            data.extend_from_slice(&[s.center.x, s.center.y, s.center.z, s.opacity, s.axis_u.x, s.axis_u.y, s.axis_u.z, 0.0, s.axis_v.x, s.axis_v.y, s.axis_v.z, 0.0]);
        }
        if n > 0 {
            r.queue.write_buffer(&self.shadow_buffer, 0, bytemuck::cast_slice(&data));
        }
        self.shadow_count.set(n as u32);
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
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &buffer, offset: 0, size: wgpu::BufferSize::new(MODEL_BYTES) }),
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
            draws.push(MeshDraw { batch: bi, bind, vertices, indices, count: b.indices.len() as u32, blend: b.blend, posed: RefCell::default() });
        }
        let (mut lo, mut hi) = (glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN));
        for v in geom.batches.iter().flat_map(|b| b.vertices.iter()) {
            let p = glam::Vec3::from(v.position);
            lo = lo.min(p);
            hi = hi.max(p);
        }
        let bounds_centre = if lo.x <= hi.x { (lo + hi) * 0.5 } else { glam::Vec3::ZERO };
        GpuMesh { draws, local: geom.local, bounds_centre }
    }

    /// Moves an uploaded skeletal mesh's vertices to posed `points` (indexed like the mesh's points), from the next draw on.
    pub fn write_points(&self, _r: &Renderer, mesh: &GpuMesh, geom: &MeshGeom, points: &[glam::Vec3]) {
        let normals = crate::meshes::point_normals(geom, points);
        for d in &mesh.draws {
            let Some(b) = geom.batches.get(d.batch) else { continue };
            if b.points.len() != b.vertices.len() {
                continue;
            }
            let mut out = d.posed.borrow_mut();
            out.clear();
            out.reserve(b.vertices.len() * std::mem::size_of::<Vertex>());
            for (v, &p) in b.vertices.iter().zip(&b.points) {
                let normal = normals.get(p as usize).copied().unwrap_or(glam::Vec3::Z);
                let v = Vertex { position: points.get(p as usize).map(|p| p.to_array()).unwrap_or(v.position), color: normal.to_array(), ..*v };
                out.extend_from_slice(bytemuck::bytes_of(&v));
            }
        }
    }

    /// Puts the posed vertices of this frame's meshes into the next buffer of the ring with one write. Returns the buffer and,
    /// for each mesh and each of its draws, the part of the buffer holding that draw's vertices (None: the draw uses the
    /// mesh's own vertex buffer).
    fn upload_posed(&self, r: &Renderer, meshes: &[(&GpuMesh, Mat4, ActorLight)]) -> (Option<wgpu::Buffer>, Vec<Vec<Option<(u64, u64)>>>) {
        let mut ring = self.posed.borrow_mut();
        let PosedRing { buffers, next, bytes } = &mut *ring;
        bytes.clear();
        let mut parts = Vec::with_capacity(meshes.len());
        for (mesh, _, _) in meshes {
            let mut mine = Vec::with_capacity(mesh.draws.len());
            for d in &mesh.draws {
                let posed = d.posed.borrow();
                if posed.is_empty() {
                    mine.push(None);
                } else {
                    let start = bytes.len() as u64;
                    bytes.extend_from_slice(&posed);
                    mine.push(Some((start, posed.len() as u64)));
                }
            }
            parts.push(mine);
        }
        if bytes.is_empty() {
            return (None, parts);
        }
        let started = std::time::Instant::now();
        let needed = (bytes.len() as u64).next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
        bytes.resize(needed as usize, 0);
        let slot = *next;
        *next = (slot + 1) % buffers.len();
        if buffers[slot].as_ref().is_none_or(|b| b.size() < needed) {
            buffers[slot] = Some(r.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("posed vertices"),
                size: needed.next_power_of_two().max(1 << 16),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let buffer = buffers[slot].clone().expect("made above");
        r.queue.write_buffer(&buffer, 0, bytes);
        POSED_BYTES.fetch_add(needed, std::sync::atomic::Ordering::Relaxed);
        UPLOAD_NS.fetch_add(started.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
        (Some(buffer), parts)
    }

    /// Draws the static scene and the meshes at their transforms (actor space to world) in one pass.
    pub fn draw(
        &self,
        r: &Renderer,
        scene: &GpuScene,
        meshes: &[(&GpuMesh, Mat4, ActorLight)],
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
        meshes: &[(&GpuMesh, Mat4, ActorLight)],
        particles: Option<(&crate::particles::ParticleRenderer, &crate::particles::ParticleDraw)>,
        camera: &Camera,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        aspect: f32,
    ) -> wgpu::CommandBuffer {
        let (posed_buffer, posed_parts) = self.upload_posed(r, meshes);
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
                let n = light.lights.len().min(8);
                data[off + 64..off + 80].copy_from_slice(bytemuck::cast_slice(&light.ambient.extend(n as f32).to_array()));
                data[off + 80..off + 96].copy_from_slice(bytemuck::cast_slice(&[light.fade, 0.0, 0.0, 0.0f32]));
                for (k, l) in light.lights.iter().take(8).enumerate() {
                    let at = off + 96 + k * 64;
                    let rows: [[f32; 4]; 4] = [
                        [l.position.x, l.position.y, l.position.z, l.kind as f32],
                        [l.direction.x, l.direction.y, l.direction.z, l.range],
                        [l.color.x, l.color.y, l.color.z, l.cos_phi],
                        [l.atten[0], l.atten[1], l.atten[2], l.cos_theta],
                    ];
                    data[at..at + 64].copy_from_slice(bytemuck::cast_slice(&rows));
                }
            }
            if !data.is_empty() {
                r.queue.write_buffer(&inst.0, 0, &data);
            }
        }
        let inst = self.instances.borrow();
        let sections = |pass: &mut wgpu::RenderPass<'_>, kinds: &[(MaterialBlend, &wgpu::RenderPipeline)]| {
            for &(blend, pipeline) in kinds {
                pass.set_pipeline(pipeline);
                for (i, (mesh, _, light)) in meshes.iter().enumerate() {
                    pass.set_bind_group(2, &inst.1, &[(i as u64 * STRIDE) as u32]);
                    // A faded actor (a wall under Lumos) is all blended: its sections go with the alpha ones.
                    let faded = light.fade > 0.0;
                    for (k, d) in mesh.draws.iter().enumerate() {
                        let wanted = if faded {
                            blend == MaterialBlend::Alpha
                        } else {
                            (d.blend == MaterialBlend::Cutout && blend == MaterialBlend::Opaque) || d.blend == blend
                        };
                        if !wanted {
                            continue;
                        }
                        pass.set_bind_group(1, &d.bind, &[]);
                        match (&posed_buffer, posed_parts[i][k]) {
                            (Some(buffer), Some((start, size))) => pass.set_vertex_buffer(0, buffer.slice(start..start + size)),
                            _ => pass.set_vertex_buffer(0, d.vertices.slice(..)),
                        }
                        pass.set_index_buffer(d.indices.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..d.count, 0, 0..1);
                        ACTOR_DRAWS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
                if self.shadow_count.get() > 0 {
                    pass.set_pipeline(&self.shadow_pipeline);
                    pass.set_vertex_buffer(0, self.shadow_buffer.slice(..));
                    pass.draw(0..4, 0..self.shadow_count.get());
                }
                sections(pass, &[(MaterialBlend::Alpha, &self.alpha_pipeline), (MaterialBlend::Additive, &self.additive_pipeline)]);
                if let Some((renderer, frame)) = particles {
                    renderer.draw(pass, frame);
                }
            },
        )
    }
}
