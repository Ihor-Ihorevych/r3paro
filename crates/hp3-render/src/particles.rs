//! Particle quads: world-space quads with a texture, colour and blend, drawn after the scene and meshes, tested
//! against the depth buffer but not writing to it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use hp3_assets::loader::{Image, ObjKey};
use wgpu::util::DeviceExt;

use crate::gpu::{Renderer, DEPTH_FORMAT};

const SHADER: &str = r#"
struct Globals { view_proj: mat4x4<f32>, fog_color: vec4<f32>, fog: vec4<f32> };
// How much of the picture's own color is left at a fragment (1 outside the fog): linear fog by depth from `fog.x` to `fog.y`.
fn fog_factor(w: f32) -> f32 {
    if (globals.fog.z < 0.5) { return 1.0; }
    return clamp((globals.fog.y - 1.0 / w) / max(globals.fog.y - globals.fog.x, 1.0), 0.0, 1.0);
}
@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32>, @location(1) color: vec4<f32> };

@vertex fn vs(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>) -> VsOut {
    var o: VsOut;
    o.pos = globals.view_proj * vec4<f32>(position, 1.0);
    o.uv = uv;
    o.color = color;
    return o;
}

// Alpha and additive blending use the texture's colour times the particle's, with alpha from both.
@fragment fn fs(i: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, i.uv) * i.color * vec4<f32>(1.0, 1.0, 1.0, fog_factor(i.pos.w));
    if (c.a < 0.004) { discard; }
    return c;
}

// A blob shadow decal: a round soft disc worked out from the quad's own coordinates (no picture), darkest in the middle and
// fading to no change at the edge, with the falloff of the original's blobshadow picture. Modulation is neutral at 0.5.
@fragment fn fs_decal(i: VsOut) -> @location(0) vec4<f32> {
    let r = clamp(length(i.uv - vec2<f32>(0.5)) * 2.0, 0.0, 1.0);
    let dark = pow(1.0 - r, 3.0) * i.color.a;
    let c = vec3<f32>(0.5 * (1.0 - dark));
    return vec4<f32>(mix(vec3<f32>(0.5), c, fog_factor(i.pos.w)), 1.0);
}

// Modulation scales what is behind by the particle's colour, fading to no change with its alpha.
@fragment fn fs_modulate(i: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, i.uv) * i.color;
    return vec4<f32>(mix(vec3<f32>(0.5), mix(vec3<f32>(0.5), c.rgb, c.a), fog_factor(i.pos.w)), 1.0);
}
"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParticleBlend {
    Alpha,
    Additive,
    Modulate,
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

pub struct ParticleRenderer {
    alpha: wgpu::RenderPipeline,
    additive: wgpu::RenderPipeline,
    modulate: wgpu::RenderPipeline,
    decal: wgpu::RenderPipeline,
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
        // result = 2 * source * destination (the source being centred on 0.5).
        ParticleBlend::Modulate | ParticleBlend::Decal => wgpu::BlendState {
            color: BlendComponent { src_factor: F::Dst, dst_factor: F::Src, operation: Add },
            alpha: BlendComponent { src_factor: F::Zero, dst_factor: F::One, operation: Add },
        },
    }
}

impl ParticleRenderer {
    pub fn new(r: &Renderer) -> ParticleRenderer {
        let d = &r.device;
        let shader = d.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("particles"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let layout = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("particles"),
            bind_group_layouts: &[r.globals_layout(), r.texture_layout()],
            push_constant_ranges: &[],
        });
        let attributes = [
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 12, shader_location: 1 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 20, shader_location: 2 },
        ];
        let pipeline = |blend: ParticleBlend, fragment: &str| {
            d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("particles"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    buffers: &[wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<V>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attributes }],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fragment),
                    targets: &[Some(wgpu::ColorTargetState { format: r.color_format, blend: Some(blend_state(blend)), write_mask: wgpu::ColorWrites::ALL })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: false,
                    depth_compare: wgpu::CompareFunction::LessEqual,
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        };
        ParticleRenderer {
            alpha: pipeline(ParticleBlend::Alpha, "fs"),
            additive: pipeline(ParticleBlend::Additive, "fs"),
            modulate: pipeline(ParticleBlend::Modulate, "fs_modulate"),
            decal: pipeline(ParticleBlend::Decal, "fs_decal"),
            binds: RefCell::new(HashMap::new()),
        }
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

    /// Draws a prepared frame into a pass that already has the globals bound at group 0.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, draw: &ParticleDraw) {
        pass.set_vertex_buffer(0, draw.vertices.slice(..));
        for (blend, bind, start, count) in &draw.runs {
            pass.set_pipeline(match blend {
                ParticleBlend::Alpha => &self.alpha,
                ParticleBlend::Additive => &self.additive,
                ParticleBlend::Modulate => &self.modulate,
                ParticleBlend::Decal => &self.decal,
            });
            pass.set_bind_group(1, bind.as_ref(), &[]);
            pass.draw(*start..*start + *count, 0..1);
        }
    }
}
