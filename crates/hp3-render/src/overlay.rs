//! 2D drawing over the scene: the HUD's and menus' textured quads, in screen pixels, with Unreal's canvas
//! blend styles (alpha, translucent = additive, modulated).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use hp3_assets::loader::{Image, ObjKey};
use wgpu::util::DeviceExt;

use crate::gpu::Renderer;

const SHADER: &str = r#"
struct Screen { size: vec4<f32> };
@group(0) @binding(0) var<uniform> screen: Screen;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32>, @location(1) color: vec4<f32> };

@vertex fn vs(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>) -> VsOut {
    var o: VsOut;
    o.pos = vec4<f32>(pos.x / screen.size.x * 2.0 - 1.0, 1.0 - pos.y / screen.size.y * 2.0, 0.0, 1.0);
    o.uv = uv;
    o.color = color;
    return o;
}

@fragment fn fs(i: VsOut) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, i.uv) * i.color;
}
"#;

/// How a quad combines with what is below.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Blend {
    Alpha,
    Additive,
    Modulate,
}

/// A quad in screen pixels with texture coordinates in 0..1.
#[derive(Clone, Debug)]
pub struct Quad {
    pub texture: Option<ObjKey>,
    pub rect: [f32; 4],
    pub uv: [f32; 4],
    pub color: [f32; 4],
    pub blend: Blend,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct V {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

pub struct Overlay {
    pipelines: HashMap<Blend, wgpu::RenderPipeline>,
    screen: wgpu::Buffer,
    screen_bg: wgpu::BindGroup,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    textures: RefCell<HashMap<Option<ObjKey>, Rc<wgpu::BindGroup>>>,
}

impl Overlay {
    pub fn new(r: &Renderer) -> Overlay {
        let d = &r.device;
        let shader = d.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("overlay"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let screen_layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("screen"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let texture_layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay"),
            bind_group_layouts: &[&screen_layout, &texture_layout],
            push_constant_ranges: &[],
        });
        let component = |src, dst| wgpu::BlendComponent { src_factor: src, dst_factor: dst, operation: wgpu::BlendOperation::Add };
        use wgpu::BlendFactor as F;
        let blends = [
            (Blend::Alpha, wgpu::BlendState::ALPHA_BLENDING),
            (Blend::Additive, wgpu::BlendState { color: component(F::SrcAlpha, F::One), alpha: component(F::Zero, F::One) }),
            (Blend::Modulate, wgpu::BlendState { color: component(F::Dst, F::Src), alpha: component(F::Zero, F::One) }),
        ];
        let mut pipelines = HashMap::new();
        for (kind, blend) in blends {
            let p = d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("overlay"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<V>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4],
                    }],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    targets: &[Some(wgpu::ColorTargetState { format: r.color_format, blend: Some(blend), write_mask: wgpu::ColorWrites::ALL })],
                    compilation_options: Default::default(),
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview: None,
                cache: None,
            });
            pipelines.insert(kind, p);
        }
        let screen = d.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screen"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let screen_bg = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &screen_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: screen.as_entire_binding() }],
        });
        let sampler = d.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Overlay { pipelines, screen, screen_bg, texture_layout, sampler, textures: RefCell::new(HashMap::new()) }
    }

    fn bind(&self, r: &Renderer, key: &Option<ObjKey>, images: &dyn Fn(&ObjKey) -> Option<Rc<Image>>) -> Rc<wgpu::BindGroup> {
        if let Some(b) = self.textures.borrow().get(key) {
            return b.clone();
        }
        let img = key.as_ref().and_then(images);
        let view = match &img {
            Some(i) => r.upload_view(i.width, i.height, &i.rgba),
            None => r.upload_view(1, 1, &[255, 255, 255, 255]),
        };
        let bg = Rc::new(r.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.texture_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        }));
        self.textures.borrow_mut().insert(key.clone(), bg.clone());
        bg
    }

    /// Draws `quads` in order over `color` (`width` x `height` pixels).
    pub fn draw(
        &self,
        r: &Renderer,
        quads: &[Quad],
        images: &dyn Fn(&ObjKey) -> Option<Rc<Image>>,
        color: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) -> wgpu::CommandBuffer {
        r.queue.write_buffer(&self.screen, 0, bytemuck::cast_slice(&[width as f32, height as f32, 0.0, 0.0]));
        let mut verts: Vec<V> = Vec::with_capacity(quads.len() * 6);
        for q in quads {
            let [x, y, w, h] = q.rect;
            let [u0, v0, u1, v1] = q.uv;
            let c = q.color;
            let corners = [(x, y, u0, v0), (x + w, y, u1, v0), (x + w, y + h, u1, v1), (x, y + h, u0, v1)];
            for k in [0, 1, 2, 0, 2, 3] {
                let (px, py, u, v) = corners[k];
                verts.push(V { pos: [px, py], uv: [u, v], color: c });
            }
        }
        let vb = (!verts.is_empty()).then(|| {
            r.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("overlay"), contents: bytemuck::cast_slice(&verts), usage: wgpu::BufferUsages::VERTEX })
        });
        // Runs of quads sharing texture and blend are drawn together, in order.
        let mut runs: Vec<(Option<ObjKey>, Blend, u32, u32)> = Vec::new();
        for (i, q) in quads.iter().enumerate() {
            match runs.last_mut() {
                Some(run) if run.0 == q.texture && run.1 == q.blend => run.3 += 6,
                _ => runs.push((q.texture.clone(), q.blend, i as u32 * 6, 6)),
            }
        }
        let binds: Vec<Rc<wgpu::BindGroup>> = runs.iter().map(|run| self.bind(r, &run.0, images)).collect();
        let mut enc = r.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("overlay"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if let Some(vb) = &vb {
                pass.set_bind_group(0, &self.screen_bg, &[]);
                pass.set_vertex_buffer(0, vb.slice(..));
                for (run, bind) in runs.iter().zip(&binds) {
                    pass.set_pipeline(&self.pipelines[&run.1]);
                    pass.set_bind_group(1, bind.as_ref(), &[]);
                    pass.draw(run.2..run.2 + run.3, 0..1);
                }
            }
        }
        enc.finish()
    }
}
