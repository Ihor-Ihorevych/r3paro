//! Particle quads: world-space quads with a texture, colour and blend, drawn after the scene and meshes, tested
//! against the depth buffer but not writing to it.
//!
//! Soft particles (`quality::soft_particles`): the quads are drawn in a pass of their own over the finished picture, with the
//! scene's depth bound as a texture instead of attached. Each fragment reads the depth behind it: a quad behind a surface is
//! not drawn (the depth test, done by hand), and one in front fades out over the last `fx.w` units before the surface, so
//! smoke and glows no longer cut into the floor and walls along a hard line.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use hp3_assets::loader::{Image, ObjKey};
use wgpu::util::DeviceExt;

use crate::gpu::{Renderer, DEPTH_FORMAT};

const SHADER: &str = r#"
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32>, @location(1) color: vec4<f32> };

@vertex fn vs(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>) -> VsOut {
    var o: VsOut;
    o.pos = squeeze(globals.view_proj * vec4<f32>(position, 1.0));
    o.uv = uv;
    o.color = color;
    return o;
}

// Alpha and additive blending use the texture's colour times the particle's, with alpha from both.
@fragment fn fs(i: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, i.uv) * i.color * vec4<f32>(1.0, 1.0, 1.0, fog_factor(i.pos.w) * soft_factor(i.pos));
    if (c.a < 0.004) { discard; }
    return c;
}

// A blob shadow decal: a round soft disc worked out from the quad's own coordinates (no picture), darkest in the middle and
// fading to no change at the edge, with the falloff of the original's blobshadow picture. Modulation is neutral at 0.5.
@fragment fn fs_decal(i: VsOut) -> @location(0) vec4<f32> {
    let r = clamp(length(i.uv - vec2<f32>(0.5)) * 2.0, 0.0, 1.0);
    let dark = pow(1.0 - r, 3.0) * i.color.a * soft_factor(i.pos);
    let c = vec3<f32>(0.5 * (1.0 - dark));
    return vec4<f32>(mix(vec3<f32>(0.5), c, fog_factor(i.pos.w)), 1.0);
}

// Modulation scales what is behind by the particle's colour, fading to no change with its alpha.
@fragment fn fs_modulate(i: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, i.uv) * i.color;
    return vec4<f32>(mix(vec3<f32>(0.5), mix(vec3<f32>(0.5), c.rgb, c.a), fog_factor(i.pos.w) * soft_factor(i.pos)), 1.0);
}

// Darken and brighten: the picture's colour times the particle's, scaled by its alpha, is the colour taken from (or added over) what is behind.
@fragment fn fs_premul(i: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, i.uv) * i.color;
    return vec4<f32>(c.rgb * c.a * fog_factor(i.pos.w) * soft_factor(i.pos), 1.0);
}
"#;

/// In the scene's pass the hardware depth test does the work: nothing fades.
const HARD: &str = "fn soft_factor(p: vec4<f32>) -> f32 { return 1.0; }\n";

/// How much of a fragment is left by the scene's depth behind it: 0 behind a surface, rising to 1 over `fx.w` units in front of
/// it; 1 over the sky or nothing. The depth read back is the squeezed one (`fx`), turned back into a distance along the view.
const SOFT: &str = r#"
fn soft_factor(p: vec4<f32>) -> f32 {
    let d = textureLoad(scene_depth, vec2<i32>(p.xy), 0).r;
    let ndc = (d - globals.dr.x) / globals.dr.y;
    if (ndc >= 1.0) { return 1.0; }
    let n = globals.dr.z;
    let f = globals.cam.w;
    let behind = n * f / (f - ndc * (f - n));
    return clamp((behind - 1.0 / p.w) / globals.fx.w, 0.0, 1.0);
}
"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParticleBlend {
    Alpha,
    Additive,
    Modulate,
    /// `dest * (1 - source)`.
    Darken,
    /// `source + dest * (1 - source)`.
    Brighten,
    /// Modulate by the picture's colour only, faded by the quad's alpha (the picture's alpha is ignored).
    Decal,
}

/// A particle quad in world space (corners top-left, top-right, bottom-right, bottom-left).
#[derive(Clone, Debug)]
pub struct ParticleQuad {
    pub texture: Option<ObjKey>,
    pub blend: ParticleBlend,
    pub corners: [[f32; 3]; 4],
    /// u0, v0, u1, v1.
    pub uv: [f32; 4],
    pub color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct V {
    pos: [f32; 3],
    uv: [f32; 2],
    color: [f32; 4],
}

/// The quads of one frame, ready to draw.
pub struct ParticleDraw {
    vertices: wgpu::Buffer,
    runs: Vec<(ParticleBlend, Rc<wgpu::BindGroup>, u32, u32)>,
}

/// Which pipelines: in the scene's pass (with its sample count), or the soft pass reading a single or a multisampled depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Variant {
    Hard(u32),
    Soft { multisampled: bool },
}

pub struct ParticleRenderer {
    hard_layout: wgpu::PipelineLayout,
    hard_shader: wgpu::ShaderModule,
    /// Index 0: the soft pass over a single-sampled depth, 1: over a multisampled one.
    soft_layouts: [wgpu::PipelineLayout; 2],
    soft_shaders: [wgpu::ShaderModule; 2],
    depth_layouts: [wgpu::BindGroupLayout; 2],
    format: wgpu::TextureFormat,
    pipelines: RefCell<HashMap<(Variant, ParticleBlend), Rc<wgpu::RenderPipeline>>>,
    binds: RefCell<HashMap<Option<ObjKey>, Rc<wgpu::BindGroup>>>,
}

fn blend_state(b: ParticleBlend) -> wgpu::BlendState {
    use wgpu::{BlendComponent, BlendFactor as F, BlendOperation::Add};
    match b {
        ParticleBlend::Alpha => wgpu::BlendState::ALPHA_BLENDING,
        ParticleBlend::Additive => wgpu::BlendState {
            color: BlendComponent { src_factor: F::SrcAlpha, dst_factor: F::One, operation: Add },
            alpha: BlendComponent { src_factor: F::Zero, dst_factor: F::One, operation: Add },
        },
        ParticleBlend::Darken => wgpu::BlendState {
            color: BlendComponent { src_factor: F::Zero, dst_factor: F::OneMinusSrc, operation: Add },
            alpha: BlendComponent { src_factor: F::Zero, dst_factor: F::One, operation: Add },
        },
        ParticleBlend::Brighten => wgpu::BlendState {
            color: BlendComponent { src_factor: F::One, dst_factor: F::OneMinusSrc, operation: Add },
            alpha: BlendComponent { src_factor: F::Zero, dst_factor: F::One, operation: Add },
        },
        // result = 2 * source * destination (the source being centred on 0.5).
        ParticleBlend::Modulate | ParticleBlend::Decal => wgpu::BlendState {
            color: BlendComponent { src_factor: F::Dst, dst_factor: F::Src, operation: Add },
            alpha: BlendComponent { src_factor: F::Zero, dst_factor: F::One, operation: Add },
        },
    }
}

fn fragment_of(b: ParticleBlend) -> &'static str {
    match b {
        ParticleBlend::Alpha | ParticleBlend::Additive => "fs",
        ParticleBlend::Modulate => "fs_modulate",
        ParticleBlend::Darken | ParticleBlend::Brighten => "fs_premul",
        ParticleBlend::Decal => "fs_decal",
    }
}

impl ParticleRenderer {
    pub fn new(r: &Renderer) -> ParticleRenderer {
        let d = &r.device;
        let module = |label: &str, extra: &str| {
            d.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(format!("{}{extra}{SHADER}", crate::fx::globals_wgsl()).into()),
            })
        };
        let depth_layout = |multisampled: bool| {
            d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("particles depth"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled },
                    count: None,
                }],
            })
        };
        let depth_layouts = [depth_layout(false), depth_layout(true)];
        let soft_layout = |depth: &wgpu::BindGroupLayout| {
            d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("soft particles"),
                bind_group_layouts: &[r.globals_layout(), r.texture_layout(), depth],
                push_constant_ranges: &[],
            })
        };
        let soft_layouts = [soft_layout(&depth_layouts[0]), soft_layout(&depth_layouts[1])];
        let hard_layout = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("particles"),
            bind_group_layouts: &[r.globals_layout(), r.texture_layout()],
            push_constant_ranges: &[],
        });
        let single = format!("@group(2) @binding(0) var scene_depth: texture_2d<f32>;\n{SOFT}");
        let multi = format!("@group(2) @binding(0) var scene_depth: texture_multisampled_2d<f32>;\n{SOFT}");
        ParticleRenderer {
            hard_layout,
            hard_shader: module("particles", HARD),
            soft_layouts,
            soft_shaders: [module("soft particles", &single), module("soft particles msaa", &multi)],
            depth_layouts,
            format: r.color_format,
            pipelines: RefCell::new(HashMap::new()),
            binds: RefCell::new(HashMap::new()),
        }
    }

    /// The pipeline for one blend in one variant, made the first time it is wanted.
    fn pipeline(&self, r: &Renderer, variant: Variant, blend: ParticleBlend) -> Rc<wgpu::RenderPipeline> {
        if let Some(p) = self.pipelines.borrow().get(&(variant, blend)) {
            return p.clone();
        }
        let attributes = [
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 12, shader_location: 1 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 20, shader_location: 2 },
        ];
        let (layout, shader, samples, depth_stencil) = match variant {
            Variant::Hard(samples) => (
                &self.hard_layout,
                &self.hard_shader,
                samples,
                Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: false,
                    depth_compare: wgpu::CompareFunction::LessEqual,
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
            ),
            // Over the resolved picture, with no depth attached (it is a texture the shader reads).
            Variant::Soft { multisampled } => (&self.soft_layouts[multisampled as usize], &self.soft_shaders[multisampled as usize], 1, None),
        };
        let pipeline = Rc::new(r.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("particles"),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs"),
                buffers: &[wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<V>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attributes }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some(fragment_of(blend)),
                targets: &[Some(wgpu::ColorTargetState { format: self.format, blend: Some(blend_state(blend)), write_mask: wgpu::ColorWrites::ALL })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil,
            multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
            multiview: None,
            cache: None,
        }));
        self.pipelines.borrow_mut().insert((variant, blend), pipeline.clone());
        pipeline
    }

    fn bind(&self, r: &Renderer, key: &Option<ObjKey>, images: &dyn Fn(&ObjKey) -> Option<Rc<Image>>) -> Rc<wgpu::BindGroup> {
        if let Some(b) = self.binds.borrow().get(key) {
            return b.clone();
        }
        let view = match key.as_ref().and_then(images) {
            Some(i) => r.upload_view(i.width, i.height, &i.rgba),
            None => {
                if std::env::var_os("HP3_PARTICLE_STYLE").is_some() { eprintln!("particle texture missing: {key:?}"); }
                r.upload_view(1, 1, &[255, 255, 255, 255])
            }
        };
        let bind = Rc::new(r.texture_bind(&view));
        self.binds.borrow_mut().insert(key.clone(), bind.clone());
        bind
    }

    /// Uploads the frame's quads, grouped in runs of the same texture and blend (the order is kept).
    pub fn prepare(&self, r: &Renderer, quads: &[ParticleQuad], images: &dyn Fn(&ObjKey) -> Option<Rc<Image>>) -> Option<ParticleDraw> {
        if quads.is_empty() {
            return None;
        }
        let mut verts: Vec<V> = Vec::with_capacity(quads.len() * 6);
        let mut runs: Vec<(ParticleBlend, Rc<wgpu::BindGroup>, u32, u32)> = Vec::new();
        let mut last: Option<(&Option<ObjKey>, ParticleBlend)> = None;
        for q in quads {
            let [u0, v0, u1, v1] = q.uv;
            let uv = [[u0, v0], [u1, v0], [u1, v1], [u0, v1]];
            for k in [0, 1, 2, 0, 2, 3] {
                verts.push(V { pos: q.corners[k], uv: uv[k], color: q.color });
            }
            if last != Some((&q.texture, q.blend)) {
                let start = verts.len() as u32 - 6;
                runs.push((q.blend, self.bind(r, &q.texture, images), start, 0));
                last = Some((&q.texture, q.blend));
            }
            if let Some(run) = runs.last_mut() {
                run.3 += 6;
            }
        }
        let vertices = r.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("particles"),
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        Some(ParticleDraw { vertices, runs })
    }

    fn draw_runs(&self, r: &Renderer, pass: &mut wgpu::RenderPass<'_>, draw: &ParticleDraw, variant: Variant) {
        pass.set_vertex_buffer(0, draw.vertices.slice(..));
        for (blend, bind, start, count) in &draw.runs {
            pass.set_pipeline(&self.pipeline(r, variant, *blend));
            pass.set_bind_group(1, bind.as_ref(), &[]);
            pass.draw(*start..*start + *count, 0..1);
        }
    }

    /// Draws a prepared frame into the scene's pass (`samples` a pixel), which already has the globals bound at group 0.
    pub fn draw(&self, r: &Renderer, pass: &mut wgpu::RenderPass<'_>, draw: &ParticleDraw, samples: u32) {
        self.draw_runs(r, pass, draw, Variant::Hard(samples));
    }

    /// Draws a prepared frame as soft particles: a pass of its own over `target.color`, reading `target.depth` (kept by the
    /// scene's pass), with the camera's bind group `globals`.
    pub fn draw_soft(&self, r: &Renderer, enc: &mut wgpu::CommandEncoder, globals: &wgpu::BindGroup, draw: &ParticleDraw, target: &crate::gpu::Target<'_>) {
        let multisampled = target.msaa.is_some();
        let depth = r.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("particles depth"),
            layout: &self.depth_layouts[multisampled as usize],
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(target.depth) }],
        });
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("soft particles"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target.color,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_bind_group(0, globals, &[]);
        pass.set_bind_group(2, &depth, &[]);
        self.draw_runs(r, &mut pass, draw, Variant::Soft { multisampled });
    }
}
