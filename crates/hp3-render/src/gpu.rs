//! wgpu rendering of a `Scene`: one textured pipeline. `GpuScene` holds the uploaded scene; it draws into a
//! window's surface or an offscreen target read back to RGBA.

use std::collections::HashMap;

use wgpu::util::DeviceExt;

use crate::scene::{Camera, Scene, Vertex};

const SHADER: &str = r#"
// fog.w: how many dynamic lights follow; each is two vectors: position and radius, color and kind (1 = quadratic non-incidence).
struct Globals { view_proj: mat4x4<f32>, fog_color: vec4<f32>, fog: vec4<f32>, lights: array<vec4<f32>, 8> };
// How much of the picture's own color is left at a fragment (1 outside the fog): linear fog by depth from `fog.x` to `fog.y`.
fn fog_factor(w: f32) -> f32 {
    if (globals.fog.z < 0.5) { return 1.0; }
    return clamp((globals.fog.y - 1.0 / w) / max(globals.fog.y - globals.fog.x, 1.0), 0.0, 1.0);
}
@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;
@group(1) @binding(2) var lightmap: texture_2d<f32>;
@group(1) @binding(3) var lsamp: sampler;
// Depth cube maps of the static level seen from the two nearest moving lights (the nearest is slot 1), for their shadows.
@group(2) @binding(0) var shadow0: texture_depth_cube;
@group(2) @binding(1) var shadow1: texture_depth_cube;
@group(2) @binding(2) var shadow_samp: sampler_comparison;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) light_uv: vec2<f32>,
    @location(2) color: vec3<f32>,
    @location(3) world: vec3<f32>,
};

@vertex fn vs(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>, @location(2) light_uv: vec2<f32>,
              @location(3) color: vec3<f32>) -> VsOut {
    var o: VsOut;
    o.pos = globals.view_proj * vec4<f32>(position, 1.0);
    o.uv = uv;
    o.light_uv = light_uv;
    o.color = color;
    o.world = position;
    return o;
}

// How much of a light reaches `v`, the vector from the light to a surface point: the depth stored in the light's cube map
// along the dominant axis against the point's, a little in front of it, softened by the sampler's 2x2 comparison.
fn shadow_visibility(slot: i32, v: vec3<f32>, far: f32) -> f32 {
    let m = max(max(abs(v.x), abs(v.y)), abs(v.z));
    let near = 2.0;
    let mm = max(m - 1.5 - 0.015 * m, near + 0.01);
    let depth = far * (mm - near) / (mm * (far - near));
    if (slot == 1) { return textureSampleCompareLevel(shadow0, shadow_samp, v, depth); }
    return textureSampleCompareLevel(shadow1, shadow_samp, v, depth);
}

// What the moving lights (a wand's glow, spells) add over the static lighting, times the texture. The face's normal comes
// from the screen derivatives, turned to the viewer; a light lights only the side it is on. Walls take the original's
// projected disc: the color dimmed by the light's distance from the plane, fading out to the edge of the disc the light's
// sphere cuts from it. Static meshes (light_uv.x < 0) take a Direct3D point light: 1 / (8 d^2 / R^2) for the quadratic kind,
// otherwise Unreal's curve, by the cosine, doubled by the modulate.
fn moving_light(world: vec3<f32>, mesh: bool) -> vec3<f32> {
    let count = i32(globals.fog.w);
    if (count == 0) { return vec3<f32>(0.0); }
    let n = normalize(cross(dpdx(world), dpdy(world)));
    var sum = vec3<f32>(0.0);
    for (var k = 0; k < count; k = k + 1) {
        let a = globals.lights[k * 2];
        let b = globals.lights[k * 2 + 1];
        let to_light = a.xyz - world;
        let r = a.w;
        let d2 = dot(to_light, to_light);
        if (d2 >= r * r) { continue; }
        // b.w: 1 for the quadratic kind, plus 2 times the light's shadow slot (0 for none).
        let code = i32(b.w + 0.5);
        let quad = (code & 1) == 1;
        let slot = code >> 1;
        var vis = 1.0;
        if (slot > 0) {
            // The point is moved a little off its surface towards the light, so the surface does not shadow itself.
            let side = select(-1.0, 1.0, dot(to_light, n) > 0.0);
            vis = shadow_visibility(slot, world + n * side * 1.0 - a.xyz, r);
            if (vis <= 0.0) { continue; }
        }
        // The normal's sign from the derivatives depends on the screen's handedness: either way, the face lit is the light's side.
        let h = abs(dot(to_light, n));
        if (mesh) {
            let d = sqrt(d2);
            let cosine = h / max(d, 0.001);
            var att = 0.0;
            if (quad) {
                att = min(r * r / max(8.0 * d2, 0.001), 1.0);
            } else {
                let x = d / r;
                att = min((2.0 * x * x * x - 3.0 * x * x + 1.0) * 2.0, 1.0);
            }
            sum = sum + min(b.rgb * att * cosine, vec3<f32>(1.0)) * 2.0 * vis;
        } else {
            let disc = sqrt(max(r * r - h * h, 0.0));
            let along = sqrt(max(d2 - h * h, 0.0));
            let edge = max(1.0 - along / max(disc, 0.001), 0.0);
            if (quad) {
                sum = sum + min(b.rgb * sqrt(max(1.0 - h * h / (r * r), 0.0)), vec3<f32>(1.0)) * edge * vis;
            } else {
                sum = sum + min(b.rgb * sqrt(max(1.0 - h / r, 0.0)), vec3<f32>(1.0)) * edge * 2.0 * vis;
            }
        }
    }
    return sum;
}

// Light is stored halved (vertex color and lightmap): doubled here, so surfaces can be lit beyond their texture.
@fragment fn fs(i: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, i.uv);
    if (c.a < 0.5) { discard; }
    let light = textureSample(lightmap, lsamp, i.light_uv).rgb * i.color * 4.0 + moving_light(i.world, i.light_uv.x < 0.0);
    return vec4<f32>(mix(globals.fog_color.rgb, c.rgb * light, fog_factor(i.pos.w)), 1.0);
}

// Alpha-blended surfaces: lit like the others, covering what is below by their own alpha.
@fragment fn fs_alpha(i: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, i.uv);
    let light = textureSample(lightmap, lsamp, i.light_uv).rgb * i.color * 4.0 + moving_light(i.world, i.light_uv.x < 0.0);
    return vec4<f32>(c.rgb * light, c.a * fog_factor(i.pos.w));
}

// Terrain layers: lit by the vertex color, covering what is below by the alpha map in the lightmap slot.
@fragment fn fs_blend(i: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, i.uv);
    let weight = textureSample(lightmap, lsamp, i.light_uv).r;
    let light = i.color * 2.0 + moving_light(i.world, false);
    return vec4<f32>(mix(globals.fog_color.rgb, c.rgb * light, fog_factor(i.pos.w)), weight);
}
"#;

/// How many moving lights the level's surfaces take at once.
pub const MAX_MOVING_LIGHTS: usize = 4;

/// A light that moves or changes while the level runs (`bDynamicLight`): the level's surfaces take it each frame on top of
/// their static lighting, the way the original adds a pass per such light.
#[derive(Clone, Copy, Debug)]
pub struct MovingLight {
    pub position: glam::Vec3,
    pub radius: f32,
    /// The light's color now (brightness, LightType wave and level brightness applied).
    pub color: glam::Vec3,
    /// `LE_QuadraticNonIncidence`, which falls off by the square of the distance (the wand's glow and spells).
    pub quadratic: bool,
}

/// Size of the globals uniform: view-projection, fog color, fog settings, and two vectors per moving light.
const GLOBALS_FLOATS: usize = 24 + MAX_MOVING_LIGHTS * 8;

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
    /// The distance fog of the next frame (set from the camera's zone).
    fog: std::cell::Cell<Option<crate::scene::Fog>>,
    /// The moving lights that light the level's surfaces in the next frame.
    moving_lights: std::cell::Cell<[Option<MovingLight>; MAX_MOVING_LIGHTS]>,
    /// Lightmap of surfaces lit by their vertex colors: one texel at half, which the shader doubles to one.
    white_lightmap: wgpu::TextureView,
    /// The cube maps that shadow the nearest moving lights.
    shadows: crate::shadow::ShadowMaps,
    /// Numbers the uploaded scenes, so a shadow map made for one is not kept for the next.
    scene_count: std::cell::Cell<u64>,
    pub color_format: wgpu::TextureFormat,
    pub adapter_info: String,
}

/// The graphics APIs to try, best first, from `HP3_BACKEND`: `vulkan`, `gl` (OpenGL, OpenGL ES on a phone), `dx12`,
/// `metal`, or `auto` (the default: Vulkan, then the platform's own API, then OpenGL).
pub fn backend_order() -> Vec<wgpu::Backends> {
    let auto = || {
        let mut order = vec![wgpu::Backends::VULKAN];
        if cfg!(windows) {
            order.push(wgpu::Backends::DX12);
        }
        if cfg!(target_vendor = "apple") {
            order.push(wgpu::Backends::METAL);
        }
        order.push(wgpu::Backends::GL);
        order
    };
    match std::env::var("HP3_BACKEND").unwrap_or_default().to_ascii_lowercase().as_str() {
        "vulkan" | "vk" => vec![wgpu::Backends::VULKAN],
        "gl" | "gles" | "opengl" => vec![wgpu::Backends::GL],
        "dx12" => vec![wgpu::Backends::DX12],
        "metal" => vec![wgpu::Backends::METAL],
        "" | "auto" => auto(),
        other => {
            eprintln!("HP3_BACKEND={other}: unknown (vulkan, gl, dx12, metal, auto); using auto");
            auto()
        }
    }
}

/// An instance for one graphics API.
pub fn instance_for(backends: wgpu::Backends) -> wgpu::Instance {
    wgpu::Instance::new(&wgpu::InstanceDescriptor { backends, ..wgpu::InstanceDescriptor::from_env_or_default() })
}

impl Renderer {
    /// A renderer without a window, drawing to sRGB RGBA targets. Tries the APIs of `backend_order` in turn.
    pub fn new_headless() -> Result<Renderer, String> {
        let mut errors = Vec::new();
        for backends in backend_order() {
            let instance = instance_for(backends);
            let adapter = match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })) {
                Ok(a) => a,
                Err(e) => {
                    errors.push(format!("{backends:?}: no adapter: {e}"));
                    continue;
                }
            };
            match Self::with_adapter(&adapter, wgpu::TextureFormat::Rgba8UnormSrgb) {
                Ok(r) => return Ok(r),
                Err(e) => errors.push(format!("{backends:?}: {e}")),
            }
        }
        Err(format!("no GPU: {}", errors.join("; ")))
    }

    /// A renderer for an adapter, drawing to targets of `color_format`.
    pub fn with_adapter(adapter: &wgpu::Adapter, color_format: wgpu::TextureFormat) -> Result<Renderer, String> {
        let info = adapter.get_info();
        // The adapter's own limits, not WebGPU's defaults: those are more than many older phones' Vulkan 1.1 drivers and
        // every OpenGL ES adapter offer, and asking for them fails the device.
        let descriptor = wgpu::DeviceDescriptor { label: Some("hp3"), required_limits: adapter.limits(), ..Default::default() };
        let (device, queue) = pollster::block_on(adapter.request_device(&descriptor)).map_err(|e| format!("device: {e}"))?;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
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
        let shadows = crate::shadow::ShadowMaps::new(&device);
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene"),
            bind_group_layouts: &[&globals_layout, &texture_layout, &shadows.layout],
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
            fog: std::cell::Cell::new(None),
            moving_lights: std::cell::Cell::new([None; MAX_MOVING_LIGHTS]),
            white_lightmap,
            shadows,
            scene_count: std::cell::Cell::new(0),
            color_format,
            adapter_info: {
                let missing = wgpu::DownlevelFlags::compliant() - adapter.get_downlevel_capabilities().flags;
                let driver = [info.driver.as_str(), info.driver_info.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" ");
                format!("{} ({:?}, {driver}){}", info.name, info.backend, if missing.is_empty() { String::new() } else { format!(", lacks {missing:?}") })
            },
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
            size: (GLOBALS_FLOATS * 4) as u64,
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
        // The opaque triangles of the level, as the moving lights' shadows see them.
        let mut shadow_positions: Vec<[f32; 3]> = Vec::new();
        let mut shadow_indices: Vec<u32> = Vec::new();
        for batch in &scene.batches {
            if batch.indices.is_empty() {
                continue;
            }
            if batch.blend.is_none() && !batch.sky && !batch.texture.as_ref().and_then(|k| scene.images.get(k)).is_some_and(|i| i.translucent && !mostly_binary_alpha(i)) {
                let base = shadow_positions.len() as u32;
                shadow_positions.extend(batch.vertices.iter().map(|v| v.position));
                shadow_indices.extend(batch.indices.iter().map(|&i| i + base));
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
            // A texture flagged as alpha-blended whose alpha is all or nothing (a cabinet with open sides, a fully opaque wall) is
            // drawn as a cut-out instead: blended surfaces write no depth and are not sorted, so one cabinet showed through another.
            let translucent = batch.texture.as_ref().and_then(|k| scene.images.get(k)).is_some_and(|i| i.translucent && (!mostly_binary_alpha(i) || std::env::var_os("HP3_NO_BINARY_ALPHA").is_some()));
            let pass = if batch.blend.is_some() { 1 } else if translucent { 2 } else { 0 };
            let pan = batch.pan.map(|p| (p, batch.vertices.clone()));
            draws.push(Draw { bind, vertices, indices, count: batch.indices.len() as u32, pass, sky: batch.sky, pan });
        }
        // A second camera for the sky zone's view.
        let sky_globals = d.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sky globals"),
            size: (GLOBALS_FLOATS * 4) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sky_globals_bg = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.globals_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: sky_globals.as_entire_binding() }],
        });
        let id = self.scene_count.get() + 1;
        self.scene_count.set(id);
        let shadow_geom = crate::shadow::ShadowGeom::new(&self.device, &shadow_positions, &shadow_indices);
        GpuScene { globals, globals_bg, draws, sky: scene.sky, sky_globals, sky_globals_bg, shadow_geom, id }
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
        self.draw_with(scene, camera, color, depth, aspect, |_| {}, |_| {})
    }

    /// Like `draw`, with two hooks in the same pass (the camera's bind group still set at group 0): `opaque` runs after the
    /// scene's opaque surfaces and terrain layers, so what it draws is hidden by them and writes the depth that the
    /// alpha-blended surfaces are tested against (they write none, so actors behind a wall of such a surface would show
    /// through it if drawn after); `blended` runs last, over everything.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_with(
        &self,
        scene: &GpuScene,
        camera: &Camera,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        aspect: f32,
        opaque: impl FnOnce(&mut wgpu::RenderPass<'_>),
        blended: impl FnOnce(&mut wgpu::RenderPass<'_>),
    ) -> wgpu::CommandBuffer {
        let fog = self.fog.get();
        let far = fog.map_or(65536.0, |f| f.end.clamp(64.0, 65536.0));
        let view_proj = camera.view_proj_far(aspect, far);
        let mut globals = [0.0f32; GLOBALS_FLOATS];
        globals[..16].copy_from_slice(&view_proj.to_cols_array());
        if let Some(f) = fog {
            globals[16..19].copy_from_slice(&f.color.to_array());
            globals[20..23].copy_from_slice(&[f.start, f.end, 1.0]);
        }
        let mut count = 0;
        // The nearest lights (they come sorted so) that reach the camera's view get shadows, in slots 1 and 2.
        let mut shadowed: Vec<MovingLight> = Vec::new();
        for l in self.moving_lights.get().iter().flatten() {
            let at = 24 + count * 8;
            let slot = if self.shadows.enabled && scene.shadow_geom.is_some() && shadowed.len() < crate::shadow::MAX_SHADOW_LIGHTS && l.position.distance(camera.position) < l.radius + 1500.0 {
                shadowed.push(*l);
                shadowed.len()
            } else {
                0
            };
            globals[at..at + 4].copy_from_slice(&[l.position.x, l.position.y, l.position.z, l.radius]);
            globals[at + 4..at + 8].copy_from_slice(&[l.color.x, l.color.y, l.color.z, (if l.quadratic { 1 } else { 0 } + 2 * slot) as f32]);
            count += 1;
        }
        globals[23] = count as f32;
        self.queue.write_buffer(&scene.globals, 0, bytemuck::cast_slice(&globals));
        let mut enc = self.device.create_command_encoder(&Default::default());
        if let Some(geom) = scene.shadow_geom.as_ref().filter(|_| !shadowed.is_empty()) {
            self.shadows.render(&self.queue, &mut enc, geom, scene.id, &shadowed);
        }
        let clear_color = match fog {
            Some(f) if f.clear => f.color,
            _ => glam::Vec3::ZERO,
        };
        let clear = wgpu::LoadOp::Clear(wgpu::Color { r: clear_color.x as f64, g: clear_color.y as f64, b: clear_color.z as f64, a: 1.0 });
        // The sky first: the level seen from the sky zone with the camera's rotation, behind everything.
        if let Some(sky) = scene.sky {
            let sky_camera = Camera { position: sky, ..*camera };
            let mut sky_globals = [0.0f32; GLOBALS_FLOATS];
            sky_globals[..16].copy_from_slice(&sky_camera.view_proj(aspect).to_cols_array());
            self.queue.write_buffer(&scene.sky_globals, 0, bytemuck::cast_slice(&sky_globals));
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
            self.draw_batches(&mut pass, scene, true, 0..3);
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
            self.draw_batches(&mut pass, scene, false, 0..2);
            opaque(&mut pass);
            self.draw_batches(&mut pass, scene, false, 2..3);
            blended(&mut pass);
        }
        enc.finish()
    }

    /// The scene's batches: opaque first, then blended ones in order (camera bind group already set).
    fn draw_batches(&self, pass: &mut wgpu::RenderPass<'_>, scene: &GpuScene, sky: bool, passes: std::ops::Range<u8>) {
        // Opaque, then terrain layers, then alpha-blended surfaces.
        for which in passes {
            pass.set_pipeline(match which {
                0 => &self.pipeline,
                1 => &self.blend_pipeline,
                _ => &self.alpha_pipeline,
            });
            // Group 2 is set here, not once: actors drawn between the passes bind their own.
            pass.set_bind_group(2, &self.shadows.bind, &[]);
            for d in scene.draws.iter().filter(|d| d.pass == which && d.sky == sky) {
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

/// Whether nearly every pixel of an image is fully clear or fully solid (at most 3% partial alpha).
fn mostly_binary_alpha(image: &hp3_assets::loader::Image) -> bool {
    let partial = image.rgba.chunks_exact(4).filter(|p| p[3] > 8 && p[3] < 247).count();
    partial * 100 <= image.rgba.len() / 4 * 3
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
    /// The sky zone's geometry: drawn only in the sky pass.
    sky: bool,
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
    /// What the moving lights' shadows are made from (none for a scene without opaque geometry).
    shadow_geom: Option<crate::shadow::ShadowGeom>,
    id: u64,
}

impl Renderer {
    /// Sets the distance fog for the frames drawn from now on (`None` for none).
    pub fn set_fog(&self, fog: Option<crate::scene::Fog>) {
        self.fog.set(fog);
    }

    /// Sets the moving lights the level's surfaces take in the frames drawn from now on (the first `MAX_MOVING_LIGHTS`).
    pub fn set_moving_lights(&self, lights: &[MovingLight]) {
        let mut slots = [None; MAX_MOVING_LIGHTS];
        for (slot, l) in slots.iter_mut().zip(lights) {
            *slot = Some(*l);
        }
        self.moving_lights.set(slots);
    }
}
