//! wgpu rendering of a `Scene`: one textured pipeline. `GpuScene` holds the uploaded scene; it draws into a
//! window's surface or an offscreen target read back to RGBA.

use std::collections::HashMap;

use wgpu::util::DeviceExt;

use crate::scene::{Camera, Scene, Vertex};

const SHADER: &str = r#"
struct Globals { view_proj: mat4x4<f32> };
@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;
@group(1) @binding(2) var lightmap: texture_2d<f32>;
@group(1) @binding(3) var lsamp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) light_uv: vec2<f32>,
    @location(2) color: vec3<f32>,
};

@vertex fn vs(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>, @location(2) light_uv: vec2<f32>,
              @location(3) color: vec3<f32>) -> VsOut {
    var o: VsOut;
    o.pos = globals.view_proj * vec4<f32>(position, 1.0);
    o.uv = uv;
    o.light_uv = light_uv;
    o.color = color;
    return o;
}

// Light is stored halved (vertex color and lightmap): doubled here, so surfaces can be lit beyond their texture.
@fragment fn fs(i: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, i.uv);
    if (c.a < 0.5) { discard; }
    let light = textureSample(lightmap, lsamp, i.light_uv).rgb * i.color * 4.0;
    return vec4<f32>(c.rgb * light, 1.0);
}

// Alpha-blended surfaces: lit like the others, covering what is below by their own alpha.
@fragment fn fs_alpha(i: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, i.uv);
    let light = textureSample(lightmap, lsamp, i.light_uv).rgb * i.color * 4.0;
    return vec4<f32>(c.rgb * light, c.a);
}

// Terrain layers: lit by the vertex color, covering what is below by the alpha map in the lightmap slot.
@fragment fn fs_blend(i: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, i.uv);
    let weight = textureSample(lightmap, lsamp, i.light_uv).r;
    return vec4<f32>(c.rgb * i.color * 2.0, weight);
}
"#;

/// Vertex buffer layout of `Vertex`.
pub const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x2, 3 => Float32x3];

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

pub struct Renderer {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    /// Alpha-blended over the opaque scene, without depth writes (terrain layers).
    blend_pipeline: wgpu::RenderPipeline,
    /// Surfaces blended by their texture's alpha, after the rest.
    alpha_pipeline: wgpu::RenderPipeline,
    globals_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    clamp_sampler: wgpu::Sampler,
    /// Lightmap of surfaces lit by their vertex colors: one texel at half, which the shader doubles to one.
    white_lightmap: wgpu::TextureView,
    pub color_format: wgpu::TextureFormat,
    pub adapter_info: String,
}

impl Renderer {
    /// A renderer without a window, drawing to sRGB RGBA targets.
    pub fn new_headless() -> Result<Renderer, String> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .map_err(|e| format!("no GPU adapter: {e}"))?;
        Self::with_adapter(&adapter, wgpu::TextureFormat::Rgba8UnormSrgb)
    }

    /// A renderer for an adapter, drawing to targets of `color_format`.
    pub fn with_adapter(adapter: &wgpu::Adapter, color_format: wgpu::TextureFormat) -> Result<Renderer, String> {
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .map_err(|e| format!("device: {e}"))?;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("texture"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene"),
            bind_group_layouts: &[&globals_layout, &texture_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &VERTEX_ATTRIBUTES,
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                targets: &[Some(color_format.into())],
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
        let blend_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene-blend"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &VERTEX_ATTRIBUTES,
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_blend"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
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
        });
        let alpha_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene-alpha"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &VERTEX_ATTRIBUTES,
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_alpha"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
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
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let clamp_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let white_lightmap = upload_linear(&device, &queue, 1, 1, &[128, 128, 128, 255]);
        Ok(Renderer {
            device,
            queue,
            pipeline,
            blend_pipeline,
            alpha_pipeline,
            globals_layout,
            texture_layout,
            sampler,
            clamp_sampler,
            white_lightmap,
            color_format,
            adapter_info: format!("{} ({:?})", info.name, info.backend),
        })
    }

    pub fn globals_layout(&self) -> &wgpu::BindGroupLayout {
        &self.globals_layout
    }

    pub fn texture_layout(&self) -> &wgpu::BindGroupLayout {
        &self.texture_layout
    }

    /// A bind group sampling `view` with the shared repeat sampler, lit by its vertex colors only.
    pub fn texture_bind(&self, view: &wgpu::TextureView) -> wgpu::BindGroup {
        self.texture_bind_lit(view, None)
    }

    /// A bind group sampling `view`, lit by `lightmap` (times the vertex colors) when given.
    pub fn texture_bind_lit(&self, view: &wgpu::TextureView, lightmap: Option<&wgpu::TextureView>) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.texture_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(lightmap.unwrap_or(&self.white_lightmap)),
                },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.clamp_sampler) },
            ],
        })
    }

    pub fn upload_view(&self, width: u32, height: u32, rgba: &[u8]) -> wgpu::TextureView {
        self.upload(width, height, rgba)
    }

    fn upload(&self, width: u32, height: u32, rgba: &[u8]) -> wgpu::TextureView {
        let tex = self.device.create_texture_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            rgba,
        );
        tex.create_view(&Default::default())
    }

    pub fn depth_target(&self, width: u32, height: u32) -> wgpu::TextureView {
        self.device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("depth"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default())
    }

    /// Uploads a scene's textures and geometry.
    pub fn upload_scene(&self, scene: &Scene) -> GpuScene {
        let d = &self.device;
        let globals = d.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bg = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.globals_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }],
        });
        let white = self.upload(1, 1, &[200, 200, 200, 255]);
        let mut views = HashMap::new();
        for (k, img) in &scene.images {
            // Alpha maps hold weights, not colors: uploaded linear.
            let view = if k.0.ends_with("#alpha") {
                upload_linear(&self.device, &self.queue, img.width, img.height, &img.rgba)
            } else {
                self.upload(img.width, img.height, &img.rgba)
            };
            views.insert(k.clone(), view);
        }
        // Lightmaps hold light levels, not colors: uploaded linear.
        let lightmaps: Vec<wgpu::TextureView> =
            scene.lightmaps.iter().map(|img| upload_linear(&self.device, &self.queue, img.width, img.height, &img.rgba)).collect();
        let mut draws = Vec::new();
        for batch in &scene.batches {
            if batch.indices.is_empty() {
                continue;
            }
            let view = batch.texture.as_ref().and_then(|k| views.get(k)).unwrap_or(&white);
            let second = match &batch.blend {
                Some(k) => views.get(k),
                None => batch.lightmap.and_then(|i| lightmaps.get(i)),
            };
            let bind = self.texture_bind_lit(view, second);
            let vertices = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&batch.vertices),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
            let indices = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&batch.indices),
                usage: wgpu::BufferUsages::INDEX,
            });
            let translucent = batch.texture.as_ref().and_then(|k| scene.images.get(k)).is_some_and(|i| i.translucent);
            let pass = if batch.blend.is_some() { 1 } else if translucent { 2 } else { 0 };
            let pan = batch.pan.map(|p| (p, batch.vertices.clone()));
            draws.push(Draw { bind, vertices, indices, count: batch.indices.len() as u32, pass, pan });
        }
        // A second camera for the sky zone's view.
        let sky_globals = d.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sky globals"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sky_globals_bg = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.globals_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: sky_globals.as_entire_binding() }],
        });
        GpuScene { globals, globals_bg, draws, sky: scene.sky, sky_globals, sky_globals_bg }
    }

    /// Slides the pictures of scrolling materials (the scenery outside a train's windows) to where `seconds` of the level's
    /// time puts them.
    pub fn animate_scene(&self, scene: &GpuScene, seconds: f32) {
        for d in &scene.draws {
            let Some((pan, base)) = &d.pan else { continue };
            // The pan wraps every 8 repeats, as the game's own does, which keeps the numbers small.
            let (du, dv) = ((seconds * pan.u).rem_euclid(8.0), (seconds * pan.v).rem_euclid(8.0));
            let moved: Vec<Vertex> = base.iter().map(|v| Vertex { uv: [v.uv[0] + du, v.uv[1] + dv], ..*v }).collect();
            self.queue.write_buffer(&d.vertices, 0, bytemuck::cast_slice(&moved));
        }
    }

    /// Draws `scene` from `camera` into `color` (with `depth` of the same size).
    pub fn draw(&self, scene: &GpuScene, camera: &Camera, color: &wgpu::TextureView, depth: &wgpu::TextureView, aspect: f32) -> wgpu::CommandBuffer {
        self.draw_with(scene, camera, color, depth, aspect, |_| {})
    }

    /// Like `draw`, then runs `extra` in the same pass (with the camera's bind group still set at group 0).
    pub fn draw_with(
        &self,
        scene: &GpuScene,
        camera: &Camera,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        aspect: f32,
        extra: impl FnOnce(&mut wgpu::RenderPass<'_>),
    ) -> wgpu::CommandBuffer {
        let view_proj = camera.view_proj(aspect);
        self.queue.write_buffer(&scene.globals, 0, bytemuck::cast_slice(&view_proj.to_cols_array()));
        let mut enc = self.device.create_command_encoder(&Default::default());
        let clear = wgpu::LoadOp::Clear(wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 });
        // The sky first: the level seen from the sky zone with the camera's rotation, behind everything.
        if let Some(sky) = scene.sky {
            let sky_camera = Camera { position: sky, ..*camera };
            self.queue.write_buffer(&scene.sky_globals, 0, bytemuck::cast_slice(&sky_camera.view_proj(aspect).to_cols_array()));
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sky"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations { load: clear, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, &scene.sky_globals_bg, &[]);
            self.draw_batches(&mut pass, scene);
        }
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: if scene.sky.is_some() { wgpu::LoadOp::Load } else { clear },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, &scene.globals_bg, &[]);
            self.draw_batches(&mut pass, scene);
            extra(&mut pass);
        }
        enc.finish()
    }

    /// The scene's batches: opaque first, then blended ones in order (camera bind group already set).
    fn draw_batches(&self, pass: &mut wgpu::RenderPass<'_>, scene: &GpuScene) {
        // Opaque, then terrain layers, then alpha-blended surfaces.
        for which in 0..3u8 {
            pass.set_pipeline(match which {
                0 => &self.pipeline,
                1 => &self.blend_pipeline,
                _ => &self.alpha_pipeline,
            });
            for d in scene.draws.iter().filter(|d| d.pass == which) {
                pass.set_bind_group(1, &d.bind, &[]);
                pass.set_vertex_buffer(0, d.vertices.slice(..));
                pass.set_index_buffer(d.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..d.count, 0, 0..1);
            }
        }
    }

    /// Renders `scene` offscreen from `camera` and returns `width * height` RGBA pixels.
    pub fn render_to_rgba(&self, scene: &Scene, camera: &Camera, width: u32, height: u32) -> Vec<u8> {
        let gpu = self.upload_scene(scene);
        self.render_frame_to_rgba(width, height, |color, depth| self.draw(&gpu, camera, color, depth, width as f32 / height as f32))
    }

    /// Runs `draw` against an offscreen color and depth target and reads the color back as RGBA.
    pub fn render_frame_to_rgba(
        &self,
        width: u32,
        height: u32,
        draw: impl FnOnce(&wgpu::TextureView, &wgpu::TextureView) -> wgpu::CommandBuffer,
    ) -> Vec<u8> {
        self.render_frames_to_rgba(width, height, |c, d| vec![draw(c, d)])
    }

    /// Like `render_frame_to_rgba`, with several command buffers submitted in order.
    pub fn render_frames_to_rgba(
        &self,
        width: u32,
        height: u32,
        draw: impl FnOnce(&wgpu::TextureView, &wgpu::TextureView) -> Vec<wgpu::CommandBuffer>,
    ) -> Vec<u8> {
        let d = &self.device;
        let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let target = d.create_texture(&wgpu::TextureDescriptor {
            label: Some("target"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.color_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let depth = self.depth_target(width, height);
        let color = target.create_view(&Default::default());
        let cmd = draw(&color, &depth);
        let bytes_per_row = (width * 4).div_ceil(256) * 256;
        let readback = d.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (bytes_per_row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = d.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bytes_per_row), rows_per_image: Some(height) },
            },
            size,
        );
        self.queue.submit(cmd.into_iter().chain([enc.finish()]));
        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = d.poll(wgpu::PollType::Wait);
        let data = slice.get_mapped_range();
        let mut out = Vec::with_capacity((width * height * 4) as usize);
        for row in 0..height {
            let start = (row * bytes_per_row) as usize;
            out.extend_from_slice(&data[start..start + (width * 4) as usize]);
        }
        out
    }
}

fn upload_linear(device: &wgpu::Device, queue: &wgpu::Queue, width: u32, height: u32, rgba: &[u8]) -> wgpu::TextureView {
    let tex = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("lightmap"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        rgba,
    );
    tex.create_view(&Default::default())
}

struct Draw {
    bind: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
    /// 0 opaque, 1 terrain layer, 2 alpha-blended.
    pass: u8,
    /// A scrolling material: its speed and the vertices as built, which `animate_scene` shifts.
    pan: Option<(hp3_assets::loader::Pan, Vec<Vertex>)>,
}

/// A scene uploaded to the GPU.
pub struct GpuScene {
    globals: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    draws: Vec<Draw>,
    sky: Option<glam::Vec3>,
    sky_globals: wgpu::Buffer,
    sky_globals_bg: wgpu::BindGroup,
}
