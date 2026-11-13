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

/// WGSL shared by every shader that receives the characters' shadow (the world's and the dynamic actors'): the comparison
/// sampler, the map and its uniform at bindings 2-4 of group `GROUP`, and `actor_shadow_term`. Use `actor_shadow_wgsl(group)`.
const ACTOR_SHADOW_WGSL: &str = r#"
@group(GROUP) @binding(2) var shadow_samp: sampler_comparison;
// The characters' shadow: nested orthographic depth maps (one layer each, the first the finest) from their strongest light.
// view_proj = each map's matrix; a = strength (0: none this frame), 1 / map size, depth bias in units, how many maps are in use;
// b = direction to the light; c = half the width in units each map covers; d = the depth range in units of each map.
struct ActorShadow { view_proj: array<mat4x4<f32>, 3>, a: vec4<f32>, b: vec4<f32>, c: vec4<f32>, d: vec4<f32> };
@group(GROUP) @binding(3) var actor_map: texture_depth_2d_array;
@group(GROUP) @binding(4) var<uniform> actor_shadow: ActorShadow;

// How much of the light a character's shadow leaves at `world` (1 none), for a surface with normal `n_in` (any side; the
// shader turns it to the light). The finest map that holds the point (and its depth) is used; the last one fades out at its
// edge. Sixteen compare taps on a rotated disc whose radius grows with the receiver's distance past the casters (the maps are
// centred on them), so the shadow is sharp at the feet and soft further out; the sampler's own 2x2 comparison smooths each
// tap. `push` scales the offset along the normal that keeps a surface from shadowing itself, in texels (1.5 for the level;
// actors, whose normals are smooth, take more).
fn actor_shadow_term(world: vec3<f32>, n_in: vec3<f32>, frag: vec2<f32>, push: f32) -> f32 {
    let strength = actor_shadow.a.x;
    if (strength <= 0.0) { return 1.0; }
    var n = n_in;
    let to_light = actor_shadow.b.xyz;
    if (dot(n, to_light) < 0.0) { n = -n; }
    let nl = dot(n, to_light);
    let count = max(i32(actor_shadow.a.w), 1);
    var cascade = 0;
    var uv = vec2<f32>(0.0);
    var cz = 0.0;
    var texel = 1.0;
    var edge = -1.0;
    for (var i = 0; i < count; i = i + 1) {
        let t = 2.0 * actor_shadow.c[i] * actor_shadow.a.y;
        let p = actor_shadow.view_proj[i] * vec4<f32>(world + n * (t * push), 1.0);
        let u = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
        cascade = i;
        uv = u;
        cz = p.z;
        texel = t;
        edge = min(min(u.x, 1.0 - u.x), min(u.y, 1.0 - u.y));
        if (edge > 0.04 && p.z > 0.0 && p.z < 1.0) { break; }
    }
    if (edge <= 0.0 || cz <= 0.0 || cz >= 1.0) { return 1.0; }
    let range = actor_shadow.d[cascade];
    // The depth bias grows with the texel (a coarse map needs more).
    let recv = cz - actor_shadow.a.z * max(texel * 2.56, 1.0) / range;
    // Units past the casters' plane (the middle of the depth range) the receiver lies, and the blur it earns.
    let past = max((cz - 0.5) * range, 0.0);
    let radius = clamp((1.2 + past * 0.07) / texel, 1.5, 18.0) * actor_shadow.a.y;
    let noise = fract(52.9829189 * fract(dot(frag, vec2<f32>(0.06711056, 0.00583715))));
    let ang = noise * 6.2831853;
    let rot = vec2<f32>(cos(ang), sin(ang));
    var vis = 0.0;
    for (var k = 0; k < 16; k = k + 1) {
        // A sunflower spiral: even coverage of the disc without a table.
        let r = sqrt((f32(k) + 0.5) / 16.0);
        let t = f32(k) * 2.3999632;
        let d = vec2<f32>(cos(t), sin(t)) * r;
        let o = vec2<f32>(d.x * rot.x - d.y * rot.y, d.x * rot.y + d.y * rot.x) * radius;
        vis = vis + textureSampleCompareLevel(actor_map, shadow_samp, uv + o, cascade, recv);
    }
    vis = vis / 16.0;
    if (cascade == count - 1) { vis = mix(1.0, vis, smoothstep(0.0, 0.06, edge)); }
    return 1.0 - strength * smoothstep(0.0, 0.3, nl) * (1.0 - vis);
}
"#;

/// The shared receiver code with its bind group number filled in.
pub fn actor_shadow_wgsl(group: u32) -> String {
    ACTOR_SHADOW_WGSL.replace("GROUP", &group.to_string())
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
    /// The characters' shadow: one orthographic depth map of the pawns near the player seen from their strongest light.
    /// Rendered by `Dynamic` (which owns the skinned vertices), sampled by the world shader at group 2 binding 3, with the
    /// light's matrix and settings in the uniform at binding 4 (`ACTOR_UNIFORM_BYTES`; strength 0 means no shadow this frame).
    pub actor_layers: Vec<wgpu::TextureView>,
    pub actor_uniform: wgpu::Buffer,
    pub actor_size: u32,
}

/// Bytes of the character shadow's uniform: three view-projections (one per nested map), then `a` (strength, 1 / map size,
/// depth bias in units, maps in use), `b` (direction to the light), `c` (half width in units of each map) and `d` (depth range in
/// units of each map).
pub const ACTOR_UNIFORM_BYTES: u64 = 256;

/// Layers of the characters' shadow texture, the most nested maps there can be (the Low tier uses only the first).
pub const ACTOR_LAYERS: u32 = 3;

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
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: wgpu::BufferSize::new(ACTOR_UNIFORM_BYTES) },
                    count: None,
                },
            ],
        });
        let actor_size: u32 = std::env::var("HP3_ACTOR_SHADOW_SIZE").ok().and_then(|v| v.parse().ok()).unwrap_or(2048);
        let actor_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("character shadow"),
            size: wgpu::Extent3d { width: actor_size, height: actor_size, depth_or_array_layers: ACTOR_LAYERS },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let actor_array = actor_tex.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        let actor_layers: Vec<wgpu::TextureView> = (0..ACTOR_LAYERS)
            .map(|layer| actor_tex.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2), base_array_layer: layer, array_layer_count: Some(1), ..Default::default() }))
            .collect();
        let actor_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("character shadow"),
            contents: &[0u8; ACTOR_UNIFORM_BYTES as usize],
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
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
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&actor_array) },
                wgpu::BindGroupEntry { binding: 4, resource: actor_uniform.as_entire_binding() },
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
        ShadowMaps { layout, bind, faces, face_buffers, face_binds, pipeline, made: RefCell::new([None; MAX_SHADOW_LIGHTS]), enabled, actor_layers, actor_uniform, actor_size }
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
