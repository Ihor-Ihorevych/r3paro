//! A static scene built from a map: BSP render sections and static mesh actors as textured triangle batches,
//! lit by lightmaps (BSP) and vertex colors (meshes) computed from the map's lights.
//!
//! The lighting follows the original's renderer: a light's color is `FGetHSV(hue, saturation, 255)` times its brightness
//! and the level's `Brightness`; radius 25 * (LightRadius + 1); a smooth-step falloff times the cosine to the surface;
//! sunlights directional without falloff; BSP texels lit only where the light's shadow bitmap allows, on top of the ambient
//! light of the surface's zone (halved). Meshes and terrain take the ambient light of the zone they stand in, the strongest
//! per channel where they reach into several. Values are stored halved and doubled in the shader. Lights that move
//! (`bDynamicLight`) do not light the BSP or baked meshes (the maps have none; only Lumos sparkles are dynamic).

use std::collections::HashMap;
use std::rc::Rc;

use glam::{Mat4, Vec3 as GVec3};
use hp3_assets::loader::{self, Image, Loader, ObjKey};
use hp3_assets::Asset;
use hp3_package::props;
use hp3_package::{ObjRef, Package};

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    /// Position in the batch's lightmap page (0..1).
    pub light_uv: [f32; 2],
    /// Light reaching the vertex, halved (1.0 means double brightness); the BSP uses its lightmap instead.
    pub color: [f32; 3],
}

impl Vertex {
    /// A vertex drawn without lighting (full brightness).
    pub fn unlit(position: [f32; 3], uv: [f32; 2]) -> Vertex {
        Vertex { position, uv, light_uv: [0.0, 0.0], color: [0.5, 0.5, 0.5] }
    }
}

/// Lightmap pages are this many texels square.
pub const LIGHTMAP_PAGE: usize = 512;

/// The lightmap position given to static mesh vertices: they sample the one-texel white lightmap anyway, and the shader
/// tells them from the BSP by it (moving lights fall on meshes as Direct3D lights, on the BSP as projected discs).
pub const MESH_LIGHT_UV: [f32; 2] = [-1.0, -1.0];

/// A light, as the scene builder needs it.
#[derive(Clone, Copy, Debug)]
pub struct LightSource {
    pub position: GVec3,
    /// Color times brightness (1.0 = full).
    pub color: GVec3,
    pub radius: f32,
    /// Sunlight: the direction light travels; no falloff.
    pub direction: Option<GVec3>,
    /// `LightEffect` (spotlights, non-incidence lights, cylinders), 0 for a plain point light.
    pub effect: u8,
    /// `LightCone` of a spotlight (256 = a needle).
    pub cone: f32,
    /// The way the light's rotation points (the axis of a spotlight).
    pub aim: GVec3,
}

/// `LightEffect` values of HP3's enum that change how a light falls off.
pub const LE_STATIC_SPOT: u8 = 8;
pub const LE_SPOTLIGHT: u8 = 12;
pub const LE_NON_INCIDENCE: u8 = 13;
pub const LE_CYLINDER: u8 = 17;
pub const LE_QUADRATIC_NON_INCIDENCE: u8 = 20;

/// Unreal's basic attenuation (`UnrealAttenuation`): a smooth step from 1 at the light to 0 at the radius, times the
/// cosine to the surface, times 2.
fn unreal_attenuation(distance: f32, radius: f32, to_light: GVec3, normal: GVec3) -> f32 {
    let facing = to_light.dot(normal);
    if facing > 0.0 && distance <= radius {
        let a = distance / radius;
        let b = 2.0 * a * a * a - 3.0 * a * a + 1.0;
        let c = (facing / radius).abs();
        return b / a * c * 2.0;
    }
    0.0
}

impl LightSource {
    /// How strongly the light shines on a surface point (Unreal's `FDynamicLight::SampleIntensity`: 2 at the light on
    /// a surface facing it); multiply by `color` for the light that point gets, shadowing aside. `wall` is the lightmap
    /// variant (a wall's texels take light on either side and the non-incidence kinds fall off a little differently).
    pub fn intensity(&self, p: GVec3, normal: GVec3, wall: bool) -> f32 {
        if let Some(d) = self.direction {
            let dot = d.dot(normal);
            return if dot < 0.0 { -dot * 2.0 } else { 0.0 };
        }
        let to = self.position - p;
        let dist = to.length();
        let facing = |n: GVec3| if wall { to.dot(n).abs() > 0.0 } else { to.dot(n) > 0.0 };
        match self.effect {
            LE_CYLINDER => {
                if dist < self.radius {
                    (1.0 - (to.x * to.x + to.y * to.y) / (self.radius * self.radius)).max(0.0) * 2.0
                } else {
                    0.0
                }
            }
            LE_NON_INCIDENCE => {
                if facing(normal) && dist < self.radius {
                    if wall { (1.0 - dist / self.radius).max(0.0) * 2.0 } else { (1.02 - dist / self.radius).max(0.0).sqrt() * 2.0 }
                } else {
                    0.0
                }
            }
            LE_QUADRATIC_NON_INCIDENCE => {
                let q = dist * dist / (self.radius * self.radius);
                if facing(normal) && q < 1.0 {
                    if wall { (1.0 - q).max(0.0) * 2.0 } else { (1.02 - q) * 2.0 }
                } else {
                    0.0
                }
            }
            LE_STATIC_SPOT | LE_SPOTLIGHT => {
                let base = if wall {
                    unreal_attenuation(dist, self.radius, to, if to.dot(normal) < 0.0 { -normal } else { normal })
                } else {
                    unreal_attenuation(dist, self.radius, to, normal)
                };
                if base > 0.0 {
                    let sine = 1.0 - self.cone / 256.0;
                    let rsine = 1.0 / (1.0 - sine);
                    let sine_rsine = sine * rsine;
                    let v_dot_v = (-to).dot(self.aim);
                    if v_dot_v > 0.0 && v_dot_v * v_dot_v > sine * sine * dist * dist {
                        let k = v_dot_v * rsine / dist - sine_rsine;
                        return k * k * base;
                    }
                }
                0.0
            }
            _ => {
                if wall {
                    unreal_attenuation(dist, self.radius, to, if to.dot(normal) < 0.0 { -normal } else { normal })
                } else {
                    unreal_attenuation(dist, self.radius, to, normal)
                }
            }
        }
    }

    /// Light this source adds at `p` on a surface facing `normal` (shadowing aside), in the units of an 8-bit light
    /// value (1.0 = 255, which the 2x modulate turns into double brightness).
    pub fn at(&self, p: GVec3, normal: GVec3) -> GVec3 {
        (self.color * self.intensity(p, normal, false)).min(GVec3::ONE)
    }

    /// The same for a wall texel: either side takes light, the sun's side by `two_sided`.
    pub fn at_wall(&self, p: GVec3, normal: GVec3, two_sided: bool) -> GVec3 {
        let i = match self.direction {
            Some(d) => {
                let dot = normal.dot(-d);
                (if two_sided { dot.abs() } else { dot.max(0.0) }) * 2.0
            }
            None => self.intensity(p, normal, true),
        };
        (self.color * i).min(GVec3::ONE)
    }
}

/// One texture layer of a terrain, as the running world has it.
#[derive(Clone, Debug)]
pub struct TerrainLayer {
    pub texture: ObjKey,
    pub alpha_map: Option<ObjKey>,
    /// World position to texture coordinates: u = p . x + w.x, v = p . y + w.y, with the matrix's planes as
    /// rows `[x, y, z, w]`.
    pub matrix: [[f32; 4]; 4],
}

/// One decoration layer of a terrain: a static mesh scattered over the quads where its density map is high.
#[derive(Clone, Debug)]
pub struct DecoLayer {
    pub mesh: ObjKey,
    pub density_map: ObjKey,
    /// Per-axis scale range: `[min, max]` for x, y and z.
    pub scale: [[f32; 2]; 3],
    /// A try succeeds with the density map's value (0..1) times a value drawn from this `[min, max]`.
    pub density: [f32; 2],
    pub max_per_quad: i32,
    pub seed: i32,
    pub align_to_terrain: bool,
    pub random_yaw: bool,
    pub show_on_invisible: bool,
    /// The meshes take a flat color instead of the terrain's light.
    pub flat_light: bool,
}

/// What the scene builder takes from the running world: lighting and terrain layers (by the terrain actor's
/// export index in the map).
#[derive(Default)]
pub struct MapInputs {
    pub lighting: Option<Lighting>,
    pub terrain_layers: HashMap<usize, Vec<TerrainLayer>>,
    /// Decoration layers (trees, grass) of each terrain, by the same index.
    pub deco_layers: HashMap<usize, Vec<DecoLayer>>,
    /// Water surfaces (`FluidSurfaceInfo`): lakes drawn as a flat grid.
    pub fluids: Vec<FluidSurface>,
    /// Static mesh actors whose mesh and look come from their class defaults, not from tags in the map (`SmartDesk`
    /// and the like), by export index.
    pub class_statics: HashMap<usize, ClassStatic>,
}

/// A baked static actor as the running world has it, class defaults applied.
#[derive(Clone, Debug)]
pub struct ClassStatic {
    pub mesh: ObjKey,
    pub scale: f32,
    pub scale3d: [f32; 3],
    /// Taken off the mesh's own origin before it is scaled and turned.
    pub prepivot: [f32; 3],
}

/// A water surface as the running world has it: a `nx` by `ny` grid of vertices `spacing` apart, centred on `at` and turned
/// by `yaw` (a script rotation), with its skin stretched over the whole grid `tiles` times.
#[derive(Clone, Debug)]
pub struct FluidSurface {
    pub texture: ObjKey,
    pub at: [f32; 3],
    pub yaw: i32,
    pub nx: usize,
    pub ny: usize,
    pub spacing: f32,
    pub tiles: [f32; 2],
}

/// The map's lights, by export index in the map package, and its ambient light.
#[derive(Default)]
pub struct Lighting {
    pub lights: HashMap<usize, LightSource>,
    /// Ambient light where no zone applies (meshes and actors, for now).
    pub ambient: GVec3,
    /// Ambient light of each zone, by the zone actor's export index in the map.
    pub zone_ambient: HashMap<usize, GVec3>,
    /// The lights found by place, for `actor_light`: made when first asked for (the lights are not changed after that).
    actor_grid: std::cell::OnceCell<ActorLightGrid>,
}

/// The lights that reach each square of the ground plane (each light by its place in `Lighting::lights`' order), and the
/// lights that reach everywhere (sunlight).
struct ActorLightGrid {
    cells: HashMap<(i32, i32), Vec<u32>>,
    everywhere: Vec<u32>,
    lights: Vec<LightSource>,
}

const ACTOR_LIGHT_CELL: f32 = 512.0;

impl Lighting {
    fn build_actor_grid(&self) -> ActorLightGrid {
        let lights: Vec<LightSource> = self.lights.values().copied().collect();
        let mut cells: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        let mut everywhere = Vec::new();
        let cell = |v: f32| (v / ACTOR_LIGHT_CELL).floor() as i32;
        for (i, l) in lights.iter().enumerate() {
            if l.direction.is_some() {
                everywhere.push(i as u32);
                continue;
            }
            for x in cell(l.position.x - l.radius)..=cell(l.position.x + l.radius) {
                for y in cell(l.position.y - l.radius)..=cell(l.position.y + l.radius) {
                    cells.entry((x, y)).or_default().push(i as u32);
                }
            }
        }
        ActorLightGrid { cells, everywhere, lights }
    }

    /// Light on an actor at `p`, from every direction (no surface angle): one level for the whole actor.
    pub fn actor_light(&self, p: GVec3) -> GVec3 {
        let grid = self.actor_grid.get_or_init(|| self.build_actor_grid());
        // Only the lights whose reach covers p's square can add anything (the rest add exactly zero), in the order
        // of the full list so that the sum comes out the same.
        let mut near: Vec<u32> = grid.everywhere.clone();
        if let Some(list) = grid.cells.get(&((p.x / ACTOR_LIGHT_CELL).floor() as i32, (p.y / ACTOR_LIGHT_CELL).floor() as i32)) {
            near.extend_from_slice(list);
        }
        near.sort_unstable();
        let mut sum = GVec3::ZERO;
        for i in near {
            let l = &grid.lights[i as usize];
            sum += match l.direction {
                Some(_) => l.color,
                None => {
                    let d = l.position.distance(p);
                    if d >= l.radius { GVec3::ZERO } else { let a = d / l.radius; l.color * (2.0 * a * a * a - 3.0 * a * a + 1.0) }
                }
            };
        }
        self.ambient + sum
    }

    /// Light at a point from every light in range (no shadows), plus ambient.
    pub fn at(&self, p: GVec3, normal: GVec3) -> GVec3 {
        (self.ambient + self.lights_at(p, normal)).min(GVec3::ONE)
    }

    /// What the lights alone add at a point on a surface facing `normal` (no ambient).
    pub fn lights_at(&self, p: GVec3, normal: GVec3) -> GVec3 {
        self.lights.values().map(|l| l.at(p, normal)).sum::<GVec3>()
    }
}

/// Triangles sharing one texture.
pub struct Batch {
    pub texture: Option<ObjKey>,
    /// Lightmap page (index into `Scene::lightmaps`), if lit by one.
    pub lightmap: Option<usize>,
    /// A terrain layer: blended over what is below by this alpha map (sampled with `light_uv`).
    pub blend: Option<ObjKey>,
    /// The picture slides over the surface at this speed (a `TexPanner`).
    pub pan: Option<loader::Pan>,
    /// Belongs to the sky zone: drawn only in the sky pass, from the sky zone's camera.
    pub sky: bool,
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub position: GVec3,
    /// Pitch, yaw, roll in Unreal units.
    pub rotation: [i32; 3],
    pub fov_degrees: f32,
}

impl Camera {
    pub fn forward(&self) -> GVec3 {
        let (p, y) = (loader::rot_to_rad(self.rotation[0]), loader::rot_to_rad(self.rotation[1]));
        GVec3::new(p.cos() * y.cos(), p.cos() * y.sin(), p.sin())
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        self.view_proj_far(aspect, 65536.0)
    }

    /// Like `view_proj` with the far clipping plane at `far` units.
    pub fn view_proj_far(&self, aspect: f32, far: f32) -> Mat4 {
        // Unreal's world is left-handed with Z up.
        let view = Mat4::look_to_lh(self.position, self.forward(), GVec3::Z);
        // Unreal's field of view is horizontal.
        let fov_y = 2.0 * ((self.fov_degrees.to_radians() / 2.0).tan() / aspect).atan();
        let proj = Mat4::perspective_lh(fov_y, aspect, 4.0, far);
        proj * view
    }
}

#[derive(Default)]
pub struct Scene {
    pub batches: Vec<Batch>,
    pub images: HashMap<ObjKey, Rc<Image>>,
    /// Lightmap pages, `LIGHTMAP_PAGE` square, RGBA.
    pub lightmaps: Vec<Image>,
    pub player_starts: Vec<(GVec3, [i32; 3])>,
    /// Where the sky zone's view is taken from (its `SkyZoneInfo`), when the map has one.
    pub sky: Option<GVec3>,
    pub stats: SceneStats,
}

#[derive(Default, Debug)]
pub struct SceneStats {
    pub bsp_triangles: usize,
    pub static_meshes: usize,
    pub mesh_triangles: usize,
    pub missing_textures: usize,
    pub terrain_triangles: usize,
    /// Terrain decoration instances (trees, grass).
    pub decorations: usize,
}

/// Unreal's rotation matrix: rows are the rotated X, Y and Z axes.
pub fn rotation_matrix(rot: [i32; 3]) -> Mat4 {
    let (sp, cp) = loader::rot_to_rad(rot[0]).sin_cos();
    let (sy, cy) = loader::rot_to_rad(rot[1]).sin_cos();
    let (sr, cr) = loader::rot_to_rad(rot[2]).sin_cos();
    let x = GVec3::new(cp * cy, cp * sy, sp);
    let y = GVec3::new(sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, -sr * cp);
    let z = GVec3::new(-(cr * sp * cy + sr * sy), cy * sr - cr * sp * sy, cr * cp);
    Mat4::from_cols(x.extend(0.0), y.extend(0.0), z.extend(0.0), glam::Vec4::W)
}

struct Batcher<'l, 's> {
    loader: &'l Loader<'s>,
    by_texture: HashMap<(Option<ObjKey>, Option<usize>, Option<(u32, u32)>, bool), usize>,
    /// Whether the batches `batch` hands out are the sky zone's.
    sky: bool,
    /// The scroll of the material `texture_for` looked at last; the next `batch` takes it.
    last_pan: Option<loader::Pan>,
    scene: Scene,
    /// The level's model (an `Asset::Model`), kept so that static meshes can find the zone they stand in.
    zone_model: Option<Rc<Asset>>,
}

/// The zone the BSP puts a point in: the zone the last plane on the way down holds on the side the point is.
fn zone_of(model: &hp3_assets::model::Model, p: GVec3) -> Option<usize> {
    if model.nodes.is_empty() {
        return None;
    }
    let mut node = 0usize;
    let mut zone = None;
    for _ in 0..model.nodes.len() {
        let nd = &model.nodes[node];
        let n = GVec3::new(nd.plane.normal.x, nd.plane.normal.y, nd.plane.normal.z);
        let front = n.dot(p) - nd.plane.w > 0.0;
        zone = Some(nd.zone[front as usize] as usize);
        let child = if front { nd.front } else { nd.back };
        if child < 0 {
            break;
        }
        node = child as usize;
    }
    zone
}

impl Batcher<'_, '_> {
    /// The ambient light of the zones a mesh at `p` reaches into (the strongest per channel), `fallback` where no zone is known.
    fn ambient_at(&self, lighting: &Lighting, p: GVec3, reach: f32, fallback: GVec3) -> GVec3 {
        let Some(Asset::Model(model)) = self.zone_model.as_deref() else { return fallback };
        let mut best: Option<GVec3> = None;
        for d in [GVec3::ZERO, GVec3::X, -GVec3::X, GVec3::Y, -GVec3::Y, GVec3::Z, -GVec3::Z] {
            let Some(zone) = zone_of(model, p + d * reach) else { continue };
            let Some(a) = model.zones.get(zone).and_then(|z| z.actor.export()).and_then(|e| lighting.zone_ambient.get(&e)) else { continue };
            best = Some(best.map_or(*a, |b| b.max(*a)));
        }
        best.unwrap_or(fallback)
    }

    fn texture_for(&mut self, pkg: &Rc<Package>, material: ObjRef) -> Option<ObjKey> {
        self.last_pan = self.loader.pan_of(pkg, material);
        let (tp, ti) = self.loader.base_texture(pkg, material)?;
        let k = loader::key(&tp, ti);
        if !self.scene.images.contains_key(&k) {
            match self.loader.image(&tp, ti) {
                Some(img) => {
                    self.scene.images.insert(k.clone(), img);
                }
                None => {
                    self.scene.stats.missing_textures += 1;
                    return None;
                }
            }
        }
        Some(k)
    }

    /// Makes sure a texture given by key is in the scene's images.
    fn texture_key(&mut self, k: &ObjKey) -> Option<ObjKey> {
        if !self.scene.images.contains_key(k) {
            let p = self.loader.set.get(&k.0).ok()?;
            let img = self.loader.image(&p, k.1)?;
            self.scene.images.insert(k.clone(), img);
        }
        Some(k.clone())
    }

    /// An alpha map's weights as an image whose every channel is the weight; under a key of its own.
    fn alpha_key(&mut self, k: &ObjKey) -> Option<ObjKey> {
        let key: ObjKey = (format!("{}#alpha", k.0), k.1);
        if !self.scene.images.contains_key(&key) {
            let p = self.loader.set.get(&k.0).ok()?;
            let img = self.loader.weight_image(&p, k.1)?;
            self.scene.images.insert(key.clone(), Rc::new(img));
        }
        Some(key)
    }

    fn batch(&mut self, texture: Option<ObjKey>, lightmap: Option<usize>) -> &mut Batch {
        let next = self.scene.batches.len();
        let pan = self.last_pan.take();
        let i = *self.by_texture.entry((texture.clone(), lightmap, pan.map(|p| (p.u.to_bits(), p.v.to_bits())), self.sky)).or_insert(next);
        if i == next {
            self.scene.batches.push(Batch { texture, lightmap, blend: None, pan, sky: self.sky, vertices: Vec::new(), indices: Vec::new() });
        }
        &mut self.scene.batches[i]
    }
}

/// Builds the scene of the level in `map` (a loaded map package).
pub fn build(loader: &Loader<'_>, map: &Rc<Package>) -> Scene {
    build_with(loader, map, &|_| true, None, &HashMap::new())
}

/// Like `build_with`, also drawing terrain with the given layers.
pub fn build_map(loader: &Loader<'_>, map: &Rc<Package>, include: &dyn Fn(usize) -> bool, inputs: &MapInputs) -> Scene {
    let mut scene = build_with(loader, map, include, inputs.lighting.as_ref(), &inputs.class_statics);
    let mut b = Batcher { loader, by_texture: HashMap::new(), sky: false, last_pan: None, scene: std::mem::take(&mut scene), zone_model: None };
    b.zone_model = level_model(&b, map).map(|(_, m)| m);
    add_terrain(&mut b, map, inputs);
    add_fluids(&mut b, inputs);
    b.scene
}

/// Water: each surface as a flat grid at its height (the waves are not simulated).
fn add_fluids(b: &mut Batcher<'_, '_>, inputs: &MapInputs) {
    for f in &inputs.fluids {
        if f.nx < 2 || f.ny < 2 {
            continue;
        }
        let Some(texture) = b.texture_key(&f.texture) else { continue };
        let (sin, cos) = (f.yaw as f32 * std::f32::consts::TAU / 65536.0).sin_cos();
        let at = GVec3::from(f.at);
        // Unlit: the reflection is its own light (the surroundings are too dark for it).
        let color = [0.5, 0.5, 0.5];
        let mut batch = Batch { texture: Some(texture), lightmap: None, blend: None, pan: None, sky: false, vertices: Vec::with_capacity(f.nx * f.ny), indices: Vec::new() };
        for j in 0..f.ny {
            for i in 0..f.nx {
                let (lx, ly) = ((i as f32 - (f.nx - 1) as f32 / 2.0) * f.spacing, (j as f32 - (f.ny - 1) as f32 / 2.0) * f.spacing);
                let p = at + GVec3::new(lx * cos - ly * sin, lx * sin + ly * cos, 0.0);
                let uv = [i as f32 / (f.nx - 1) as f32 * f.tiles[0], j as f32 / (f.ny - 1) as f32 * f.tiles[1]];
                batch.vertices.push(Vertex { position: p.to_array(), uv, light_uv: [0.0, 0.0], color });
            }
        }
        for j in 0..f.ny - 1 {
            for i in 0..f.nx - 1 {
                let (a, bb, c, d) = ((j * f.nx + i) as u32, (j * f.nx + i + 1) as u32, ((j + 1) * f.nx + i) as u32, ((j + 1) * f.nx + i + 1) as u32);
                // Both faces: seen from above or, from a camera below the surface, from under it.
                batch.indices.extend_from_slice(&[a, bb, d, a, d, c, a, d, bb, a, c, d]);
            }
        }
        b.scene.batches.push(batch);
    }
}

/// Terrain: each layer's triangles where its alpha map shows it, blended in order over the first.
fn add_terrain(b: &mut Batcher<'_, '_>, map: &Rc<Package>, inputs: &MapInputs) {
    for i in 0..map.exports.len() {
        if !map.export_class_name(i).eq_ignore_ascii_case("TerrainInfo") {
            continue;
        }
        let Some(layers) = inputs.terrain_layers.get(&i) else { continue };
        let Some(asset) = b.loader.asset(map, i) else { continue };
        let Asset::Terrain(t) = &*asset else { continue };
        let (w, h) = (t.heightmap_x.max(0) as usize, t.heightmap_y.max(0) as usize);
        if w < 2 || h < 2 || t.vertices.len() < w * h {
            continue;
        }
        // Holes: one bit per quad, set where the quad is drawn (an empty bitmap shows everything).
        let visible = b
            .loader
            .with_tags(map, i, |tags, _| {
                props::find(map, tags, "QuadVisibilityBitmap", 0).map(|t| {
                    let mut r = hp3_package::Reader::new(t.data);
                    let n = r.ci().unwrap_or(0).max(0) as usize;
                    (0..n).filter_map(|_| r.u32().ok()).collect::<Vec<u32>>()
                })
            })
            .flatten()
            .unwrap_or_default();
        // Quads whose diagonal the level turned (from (x + 1, y) to (x, y + 1)), by the same bit index.
        let turned = b
            .loader
            .with_tags(map, i, |tags, _| {
                props::find(map, tags, "EdgeTurnBitmap", 0).map(|t| {
                    let mut r = hp3_package::Reader::new(t.data);
                    let n = r.ci().unwrap_or(0).max(0) as usize;
                    (0..n).filter_map(|_| r.u32().ok()).collect::<Vec<u32>>()
                })
            })
            .flatten()
            .unwrap_or_default();
        let quad_visible = |x: usize, y: usize| {
            // One bit per heightmap cell (a 64x64 map has a 4096-bit bitmap), so a row is the full width, not width - 1.
            let q = y * w + x;
            visible.is_empty() || visible.get(q / 32).is_some_and(|b| b & (1 << (q % 32)) != 0)
        };
        // Vertex normals from the faces around each vertex; lighting per vertex.
        let pos = |k: usize| GVec3::new(t.vertices[k].x, t.vertices[k].y, t.vertices[k].z);
        let mut normals = vec![GVec3::ZERO; w * h];
        for y in 0..h - 1 {
            for x in 0..w - 1 {
                let (a, bb, c, d) = (y * w + x, y * w + x + 1, (y + 1) * w + x, (y + 1) * w + x + 1);
                let n = (pos(bb) - pos(a)).cross(pos(c) - pos(a));
                let n = if n.z < 0.0 { -n } else { n };
                for k in [a, bb, c, d] {
                    normals[k] += n;
                }
            }
        }
        // The terrain's own zone gives its ambient light (the original halves it for terrain).
        let terrain_amb = inputs.lighting.as_ref().map(|l| b.ambient_at(l, pos((h / 2) * w + w / 2), 0.0, l.ambient));
        let color: Vec<[f32; 3]> = (0..w * h)
            .map(|k| match (&inputs.lighting, terrain_amb) {
                (Some(l), Some(amb)) => (amb * 0.5 + (l.lights.values().map(|s| s.at(pos(k), normals[k].normalize_or(GVec3::Z))).fold(GVec3::ZERO, |a, b| a + b))).min(GVec3::ONE).to_array(),
                _ => [0.5, 0.5, 0.5],
            })
            .collect();
        for (li, layer) in layers.iter().enumerate() {
            let Some(texture) = b.texture_key(&layer.texture) else { continue };
            let alpha = if li == 0 { None } else { layer.alpha_map.as_ref().and_then(|k| b.alpha_key(k)) };
            // A layer whose alpha map cannot be read ends the drawn layers (the map's layer list already stops at
            // the first slot without a texture or an alpha map, as the original's renderer does).
            if li > 0 && alpha.is_none() {
                break;
            }
            let m = layer.matrix;
            let mut batch = Batch { texture: Some(texture), lightmap: None, blend: alpha, pan: None, sky: false, vertices: Vec::with_capacity(w * h), indices: Vec::new() };
            for k in 0..w * h {
                let p = pos(k);
                let u = p.x * m[0][0] + p.y * m[1][0] + p.z * m[2][0] + m[3][0];
                let v = p.x * m[0][1] + p.y * m[1][1] + p.z * m[2][1] + m[3][1];
                let (x, y) = (k % w, k / w);
                let a_uv = [(x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32];
                batch.vertices.push(Vertex { position: p.to_array(), uv: [u, v], light_uv: a_uv, color: color[k] });
            }
            for y in 0..h - 1 {
                for x in 0..w - 1 {
                    if !quad_visible(x, y) {
                        continue;
                    }
                    let (a, bb, c, d) = ((y * w + x) as u32, (y * w + x + 1) as u32, ((y + 1) * w + x) as u32, ((y + 1) * w + x + 1) as u32);
                    let q = y * w + x;
                    if turned.get(q / 32).is_some_and(|b| b & (1 << (q % 32)) != 0) {
                        batch.indices.extend_from_slice(&[a, bb, c, c, bb, d]);
                    } else {
                        batch.indices.extend_from_slice(&[a, bb, d, a, d, c]);
                    }
                }
            }
            b.scene.stats.terrain_triangles += batch.indices.len() / 3;
            b.scene.batches.push(batch);
        }
        if let Some(deco) = inputs.deco_layers.get(&i) {
            add_deco(b, map, t, deco, &color, &quad_visible);
        }
    }
}

/// The original's random numbers for decoration placement: a linear congruential generator whose top bits are discarded.
struct DecoRand(i32);

impl DecoRand {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(196314165).wrapping_add(907633515);
        (self.0 & 0x007f_ffff) as f32 / 8388608.0
    }

    /// A value from the range, as `Max + (Min - Max) * rand`.
    fn range(&mut self, r: [f32; 2]) -> f32 {
        r[1] + (r[0] - r[1]) * self.next()
    }
}

/// An Unreal rotator (pitch, yaw) turning +X to `n`.
fn rotation_of(n: GVec3) -> [i32; 3] {
    let yaw = n.y.atan2(n.x) * 65536.0 / std::f32::consts::TAU;
    let pitch = n.z.atan2((n.x * n.x + n.y * n.y).sqrt()) * 65536.0 / std::f32::consts::TAU;
    [pitch as i32, yaw as i32, 0]
}

/// Terrain decorations: per sector, per quad, the layer's mesh is placed at random spots of the quad with the original's
/// generator and the same order of draws, then baked into the scene like any static mesh.
fn add_deco(b: &mut Batcher<'_, '_>, map: &Rc<Package>, t: &hp3_assets::level::Terrain, layers: &[DecoLayer], color: &[[f32; 3]], quad_visible: &dyn Fn(usize, usize) -> bool) {
    let w = t.heightmap_x as usize;
    let pos = |x: usize, y: usize| {
        let v = t.vertices[y * w + x];
        GVec3::new(v.x, v.y, v.z)
    };
    for layer in layers {
        let Some(density_pkg) = b.loader.set.get(&layer.density_map.0).ok() else { continue };
        let Some((dw, dh, density)) = b.loader.density_map(&density_pkg, layer.density_map.1) else { continue };
        let Some(mesh_pkg) = b.loader.set.get(&layer.mesh.0).ok() else { continue };
        let Some(mesh_asset) = b.loader.asset(&mesh_pkg, layer.mesh.1) else { continue };
        let Asset::StaticMesh(sm) = &*mesh_asset else { continue };
        let materials = b.loader.static_mesh_materials(&mesh_pkg, layer.mesh.1);
        let uvs = sm.uv_streams.first().map(|s| s.uvs.as_slice()).unwrap_or(&[]);
        // One texture per mesh section, found once.
        let textures: Vec<Option<ObjKey>> = (0..sm.sections.len()).map(|si| materials.get(si).copied().and_then(|m| b.texture_for(&mesh_pkg, m))).collect();
        let (hx, hy) = (t.heightmap_x as usize, t.heightmap_y as usize);
        let mut placed = 0usize;
        for (sector_index, sector_ref) in t.sectors.iter().enumerate() {
            let Some((sp, si)) = b.loader.set.resolve(map, *sector_ref) else { continue };
            let Some(sector) = b.loader.asset(&sp, si) else { continue };
            let Asset::TerrainSector(sector) = &*sector else { continue };
            let mut rng = DecoRand(layer.seed.wrapping_add(sector_index as i32));
            for y in 0..sector.quads[1].max(0) as usize {
                for x in 0..sector.quads[0].max(0) as usize {
                    let (gx, gy) = (sector.offset[0] as usize + x, sector.offset[1] as usize + y);
                    if gx + 1 >= hx || gy + 1 >= hy || (!layer.show_on_invisible && !quad_visible(gx, gy)) {
                        continue;
                    }
                    for _ in 0..layer.max_per_quad.max(0) {
                        // The density map is read with its own resolution: texel = quad * size / heightmap size.
                        let (tx, ty) = (gx * dw / hx, gy * dh / hy);
                        let weight = density.get(ty * dw + tx).copied().unwrap_or(0) as f32 / 255.0;
                        if rng.next() >= weight * rng.range(layer.density) {
                            continue;
                        }
                        let (rx, ry) = (rng.next(), rng.next());
                        let (loc, normal) = if rx > ry {
                            let dir_x = pos(gx, gy) - pos(gx + 1, gy);
                            let dir_y = pos(gx + 1, gy + 1) - pos(gx + 1, gy);
                            (pos(gx + 1, gy) + dir_x * (1.0 - rx) + dir_y * ry, dir_x.cross(dir_y).normalize_or(GVec3::Z))
                        } else {
                            let dir_x = pos(gx, gy) - pos(gx, gy + 1);
                            let dir_y = pos(gx + 1, gy + 1) - pos(gx, gy + 1);
                            (pos(gx, gy + 1) + dir_x * rx + dir_y * (1.0 - ry), dir_x.cross(dir_y).normalize_or(GVec3::Z))
                        };
                        let normal = if normal.z < 0.0 { -normal } else { normal };
                        let normal = if layer.align_to_terrain { normal } else { GVec3::Z };
                        let mut rotation = rotation_of(normal);
                        // Meshes are built along Z, not along X.
                        rotation[0] -= 16384;
                        if layer.random_yaw {
                            rotation[1] = (65535.0 * rng.next()) as i32;
                        }
                        // The three scale factors are drawn last to first (the order the original's compiler evaluated them in).
                        let sz = rng.range(layer.scale[2]);
                        let sy = rng.range(layer.scale[1]);
                        let sx = rng.range(layer.scale[0]);
                        let light = if layer.flat_light { [0.5; 3] } else { color[gy * w + gx] };
                        let transform = Mat4::from_translation(loc) * rotation_matrix(rotation) * Mat4::from_scale(GVec3::new(sx, sy, sz));
                        for (si_, section) in sm.sections.iter().enumerate() {
                            let batch = b.batch(textures[si_].clone(), None);
                            let base = batch.vertices.len() as u32;
                            let first = section.first_index as usize;
                            let Some(idx) = sm.indices.get(first..first + section.num_faces as usize * 3) else { continue };
                            let mut remap: HashMap<u16, u32> = HashMap::new();
                            for &vi in idx {
                                let next = base + remap.len() as u32;
                                let out = *remap.entry(vi).or_insert_with(|| {
                                    let v = sm.vertices.get(vi as usize).map(|v| v.position).unwrap_or_default();
                                    let p = transform.transform_point3(GVec3::new(v.x, v.y, v.z));
                                    let uv = uvs.get(vi as usize).copied().unwrap_or([0.0, 0.0]);
                                    batch.vertices.push(Vertex { position: p.to_array(), uv, light_uv: MESH_LIGHT_UV, color: light });
                                    next
                                });
                                batch.indices.push(out);
                            }
                            b.scene.stats.mesh_triangles += section.num_faces as usize;
                        }
                        placed += 1;
                    }
                }
            }
        }
        b.scene.stats.decorations += placed;
    }
}

/// Builds the scene with only the static-mesh actors whose export index passes `include`; the others are drawn
/// per frame. With `lighting`, surfaces are lit; without, everything is drawn at full brightness. `class_statics`
/// gives the mesh of actors that have none of their own in the map.
pub fn build_with(
    loader: &Loader<'_>,
    map: &Rc<Package>,
    include: &dyn Fn(usize) -> bool,
    lighting: Option<&Lighting>,
    class_statics: &HashMap<usize, ClassStatic>,
) -> Scene {
    let mut b = Batcher { loader, by_texture: HashMap::new(), sky: false, last_pan: None, scene: Scene::default(), zone_model: None };
    add_bsp(&mut b, map, lighting);
    add_actors(&mut b, map, include, lighting, class_statics);
    b.scene
}

/// Computes a model's lightmap pages from its lights' shadow bitmaps.
fn build_lightmaps(model: &hp3_assets::model::Model, lighting: &Lighting) -> Vec<Image> {
    let n = LIGHTMAP_PAGE;
    // Each surface's zone (the front side of a node showing it) gives the ambient light of its texels.
    let mut surf_ambient: HashMap<i32, GVec3> = HashMap::new();
    for node in &model.nodes {
        if node.surf < 0 || surf_ambient.contains_key(&node.surf) {
            continue;
        }
        let zone = if node.zone[1] != 0 { node.zone[1] } else { node.zone[0] };
        if let Some(a) = model.zones.get(zone as usize).and_then(|z| z.actor.export()).and_then(|i| lighting.zone_ambient.get(&i)) {
            surf_ambient.insert(node.surf, *a);
        }
    }
    if std::env::var_os("HP3_LIGHT_DEBUG").is_some() {
        let mut refs: HashMap<usize, usize> = HashMap::new();
        let mut missing = 0;
        for lm in &model.light_maps {
            for bm in &lm.bitmaps {
                match bm.light.export() {
                    Some(i) if lighting.lights.contains_key(&i) => *refs.entry(i).or_default() += 1,
                    _ => missing += 1,
                }
            }
        }
        let suns = lighting.lights.values().filter(|l| l.direction.is_some()).count();
        eprintln!(
            "lighting: {} lights ({suns} suns), ambient {:?}; bitmaps: {} matched to {} lights, {missing} unmatched",
            lighting.lights.len(),
            lighting.ambient,
            refs.values().sum::<usize>(),
            refs.len()
        );
        let (mut sun_bits, mut sun_set, mut sun_maps) = (0usize, 0usize, 0usize);
        for lm in &model.light_maps {
            for bm in &lm.bitmaps {
                if bm.light.export().and_then(|i| lighting.lights.get(&i)).is_some_and(|l| l.direction.is_some()) {
                    sun_maps += 1;
                    sun_bits += (lm.size[0] * lm.size[1]).max(0) as usize;
                    sun_set += bm.bits.iter().map(|b| b.count_ones() as usize).sum::<usize>();
                }
            }
        }
        eprintln!("  sun bitmaps {sun_maps}: {sun_set} of {sun_bits} texels lit");
        let stored: usize = model.light_map_textures.iter().map(|t| t.data_lens[0] + t.data_lens[1]).sum();
        eprintln!("  lightmap pages {}: {stored} bytes of stored pixels", model.light_map_textures.len());
        if let Ok(spot) = std::env::var("HP3_LIGHT_AT") {
            let v: Vec<f32> = spot.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            let at = GVec3::new(v[0], v[1], v[2]);
            for (li, lm) in model.light_maps.iter().enumerate() {
                let base = GVec3::new(lm.base.x, lm.base.y, lm.base.z);
                let ax = GVec3::new(lm.axis_x.x, lm.axis_x.y, lm.axis_x.z);
                let ay = GVec3::new(lm.axis_y.x, lm.axis_y.y, lm.axis_y.z);
                let far = base + ax * lm.size[0] as f32 + ay * lm.size[1] as f32;
                let (lo, hi) = (base.min(far), base.max(far));
                if at.x < lo.x - 8.0 || at.x > hi.x + 8.0 || at.y < lo.y - 8.0 || at.y > hi.y + 8.0 || (at.z - lo.z).abs() > 120.0 {
                    continue;
                }
                let surf = &model.surfs[lm.surf as usize];
                let normal = model.vectors.get(surf.normal as usize);
                eprintln!("  lightmap {li} surf {} flags {:#x} normal {normal:?} base {base:?} size {:?} bitmaps:", lm.surf, surf.poly_flags, lm.size);
                for bm in &lm.bitmaps {
                    let l = bm.light.export().and_then(|i| lighting.lights.get(&i));
                    let set: usize = bm.bits.iter().map(|b| b.count_ones() as usize).sum();
                    eprintln!("    light {:?} sun {} bits {set}/{} size {:?} min {:?} max {:?}", bm.light.export(), l.is_some_and(|l| l.direction.is_some()), lm.size[0] * lm.size[1], bm.size, bm.min, bm.max);
                }
            }
        }
    }
    // HP3 builds a light's lightmap palette as level x color, without the doubling of UT2004's (its MergeLightMap
    // shifts by the light's factor, not one less): each light adds half of what the falloff below gives. The ambient
    // fill (half the zone's light) is the same as UT2004's.
    let hp3_bsp_amb_mul: f32 = std::env::var("HP3_BSP_AMB_MUL").ok().and_then(|v| v.parse().ok()).unwrap_or(0.25);
    let hp3_lightmap_scale: f32 = 0.5 * std::env::var("HP3_LM_MUL").ok().and_then(|v| v.parse().ok()).unwrap_or(0.6);
    let mut pages: Vec<Vec<f32>> = vec![Vec::new(); model.light_map_textures.len()];
    for (pi, page) in model.light_map_textures.iter().enumerate() {
        let mut texels = vec![0.0f32; n * n * 3];
        for &lmi in &page.light_maps {
            let Some(lm) = model.light_maps.get(lmi as usize) else { continue };
            let Some(surf) = model.surfs.get(lm.surf as usize) else { continue };
            let normal = model.vectors.get(surf.normal as usize).map(|v| GVec3::new(v.x, v.y, v.z)).unwrap_or(GVec3::Z);
            let two_sided = surf.poly_flags & 0x0000_0100 != 0;
            let base = GVec3::new(lm.base.x, lm.base.y, lm.base.z);
            let ax = GVec3::new(lm.axis_x.x, lm.axis_x.y, lm.axis_x.z);
            let ay = GVec3::new(lm.axis_y.x, lm.axis_y.y, lm.axis_y.z);
            // The lights of this lightmap, each with its visibility bits.
            let lights: Vec<(&LightSource, &hp3_assets::model::LightBitmap)> = lm
                .bitmaps
                .iter()
                .filter_map(|bm| {
                    let index = bm.light.export()?;
                    lighting.lights.get(&index).map(|l| (l, bm))
                })
                .collect();
            for y in 0..lm.size[1].max(0) as usize {
                for x in 0..lm.size[0].max(0) as usize {
                    let (px, py) = (lm.offset[0] as usize + x, lm.offset[1] as usize + y);
                    if px >= n || py >= n {
                        continue;
                    }
                    let p = base + ax * (x as f32 + 0.5) + ay * (y as f32 + 0.5);
                    let mut c = surf_ambient.get(&lm.surf).copied().unwrap_or(lighting.ambient) * 0.5 * hp3_bsp_amb_mul;
                    for (l, bm) in &lights {
                        let shadow = soft_shadow(bm, x as i32, y as i32);
                        if shadow > 0.0 {
                            c = (c + l.at_wall(p, normal, two_sided) * shadow * hp3_lightmap_scale).min(GVec3::ONE);
                        }
                    }
                    let t = (py * n + px) * 3;
                    texels[t..t + 3].copy_from_slice(&c.to_array());
                }
            }
        }
        pages[pi] = texels;
    }
    pages
        .into_iter()
        .map(|texels| {
            let mut rgba = vec![255u8; n * n * 4];
            for i in 0..n * n {
                for k in 0..3 {
                    rgba[i * 4 + k] = (texels[i * 3 + k] * 255.0).clamp(0.0, 255.0) as u8;
                }
            }
            Image { width: n as u32, height: n as u32, rgba, masked: false, translucent: false }
        })
        .collect()
}

/// The level's model asset (an `Asset::Model`) with the package it lives in.
fn level_model(b: &Batcher<'_, '_>, map: &Rc<Package>) -> Option<(Rc<Package>, Rc<Asset>)> {
    let level_index = (0..map.exports.len()).find(|&i| map.export_class_name(i).eq_ignore_ascii_case("Level"))?;
    let level_asset = b.loader.asset(map, level_index)?;
    let Asset::Level(level) = &*level_asset else { return None };
    let (mp, mi) = b.loader.set.resolve(map, level.model)?;
    let model_asset = b.loader.asset(&mp, mi)?;
    matches!(&*model_asset, Asset::Model(_)).then_some((mp, model_asset))
}

fn add_bsp(b: &mut Batcher<'_, '_>, map: &Rc<Package>, lighting: Option<&Lighting>) {
    let Some((mp, model_asset)) = level_model(b, map) else { return };
    let Asset::Model(model) = &*model_asset else { return };
    b.zone_model = Some(model_asset.clone());
    if let Some(l) = lighting {
        b.scene.lightmaps = build_lightmaps(model, l);
    }
    // Invisible, portal and fake-backdrop surfaces are not drawn.
    const HIDDEN: u32 = 0x0000_0001 | 0x0400_0000 | 0x0000_0080;
    // Unlit surfaces are drawn at full brightness.
    const UNLIT: u32 = 0x0040_0000;
    // The zones whose actor is a SkyZoneInfo: their surfaces are the sky room, drawn only in the sky pass.
    let sky_zones: Vec<bool> = model
        .zones
        .iter()
        .map(|z| z.actor.export().is_some_and(|i| b.loader.set.export_chain(map, i).first().is_some_and(|c| c.eq_ignore_ascii_case("SkyZoneInfo"))))
        .collect();
    for node in &model.nodes {
        if node.num_vertices < 3 || node.section < 0 {
            continue;
        }
        let zone = if node.zone[1] != 0 { node.zone[1] } else { node.zone[0] };
        b.sky = sky_zones.get(zone as usize).copied().unwrap_or(false);
        let Some(section) = model.sections.get(node.section as usize) else { continue };
        let surf_flags = model.surfs.get(node.surf as usize).map(|s| s.poly_flags).unwrap_or(0);
        if (surf_flags | section.poly_flags) & HIDDEN != 0 {
            continue;
        }
        let texture = b.texture_for(&mp, section.material);
        if std::env::var_os("HP3_BSP_MATS").is_some() {
            let kind = section.material.export().map(|i| b.loader.set.export_chain(&mp, i).first().cloned().unwrap_or_default()).unwrap_or_default();
            eprintln!("bspmat {} kind {} flags {:#x}/{:#x}", mp.ref_path(section.material), kind, surf_flags, section.poly_flags);
        }
        let first = node.first_vertex as usize;
        let n = node.num_vertices as usize;
        let Some(verts) = section.vertices.get(first..first + n) else { continue };
        let page = (section.light_map_texture as usize) < b.scene.lightmaps.len();
        // The window panes are backlit by the sky outside: HP3 shows them at their own brightness, not the room's.
        let backlit = {
            let path = mp.ref_path(section.material).to_ascii_lowercase();
            path.contains(".windows.") && (path.contains("window") || path.contains("shutter_light")) && std::env::var_os("HP3_NO_WINDOW_GLOW").is_none()
        };
        let lit = page && !backlit && (surf_flags | section.poly_flags) & UNLIT == 0;
        let lightmap = lit.then_some(section.light_map_texture as usize);
        // Lit surfaces take their light from the lightmap, the others from the white one; the vertex color is neutral.
        // A backlit pane shows 1.55 times its texture (measured: 95/95/110 in the original against 60/60/70 at the texture's own brightness).
        let glow = if backlit { std::env::var("HP3_WINDOW_GLOW").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.55) } else { 1.0 };
        let color = [0.5 * glow; 3];
        let batch = b.batch(texture, lightmap);
        let base = batch.vertices.len() as u32;
        for v in verts {
            batch.vertices.push(Vertex {
                position: [v.position.x, v.position.y, v.position.z],
                uv: v.uv,
                light_uv: v.light_map_uv,
                color,
            });
        }
        for k in 1..n as u32 - 1 {
            batch.indices.extend_from_slice(&[base, base + k, base + k + 1]);
        }
        b.scene.stats.bsp_triangles += n - 2;
    }
    b.sky = false;
}

fn add_actors(b: &mut Batcher<'_, '_>, map: &Rc<Package>, include: &dyn Fn(usize) -> bool, lighting: Option<&Lighting>, class_statics: &HashMap<usize, ClassStatic>) {
    for i in 0..map.exports.len() {
        let chain = b.loader.set.export_chain(map, i);
        let is_mesh_actor = chain.iter().any(|c| c.eq_ignore_ascii_case("Actor"));
        if !is_mesh_actor {
            continue;
        }
        let info = b.loader.with_tags(map, i, |tags, chain| {
            let f = |n: &str| props::find(map, tags, n, 0);
            if f("bDeleteMe").is_some_and(|t| t.bool_value) {
                return None;
            }
            let location = f("Location").and_then(loader::tag_vec3).unwrap_or_default();
            let rotation = f("Rotation").and_then(loader::tag_rotator).unwrap_or_default();
            if chain.iter().any(|c| c.eq_ignore_ascii_case("PlayerStart")) {
                return Some((None, location, rotation, 1.0, GVec3::ONE, Vec::new(), true, None));
            }
            if chain.first().is_some_and(|c| c.eq_ignore_ascii_case("SkyZoneInfo")) {
                return Some((None, location, rotation, -1.0, GVec3::ONE, Vec::new(), false, None));
            }
            // No mesh of its own in the map: it may come from the class defaults, which `class_statics` has.
            let Some(mesh) = f("StaticMesh").and_then(|t| t.as_obj()) else {
                return class_statics.contains_key(&i).then(|| (None, location, rotation, 1.0, GVec3::ONE, Vec::new(), false, None));
            };
            if f("bHidden").is_some_and(|t| t.bool_value) {
                return None;
            }
            let scale = f("DrawScale").and_then(|t| t.as_f32()).unwrap_or(1.0);
            let scale3d = f("DrawScale3D").and_then(loader::tag_vec3).map(|v| GVec3::new(v.x, v.y, v.z)).unwrap_or(GVec3::ONE);
            let skins: Vec<ObjRef> = tags
                .iter()
                .filter(|t| map.name(t.name).eq_ignore_ascii_case("Skins"))
                .filter_map(|t| {
                    let mut r = hp3_package::Reader::new(t.data);
                    let n = r.ci().ok()?;
                    (n > 0).then(|| r.ci().ok().map(ObjRef)).flatten()
                })
                .collect();
            let instance = f("StaticMeshInstance").and_then(|t| t.as_obj());
            Some((Some(mesh), location, rotation, scale, scale3d, skins, false, instance))
        });
        let Some(Some((mesh, location, rotation, mut scale, mut scale3d, skins, is_start, instance))) = info else { continue };
        let pos = GVec3::new(location.x, location.y, location.z);
        // Unreal draws a static mesh at translate(Location) * rotate * scale * translate(-PrePivot): the pivot is taken off
        // in the mesh's own space, so the scale applies to it.
        let mut prepivot = GVec3::ZERO;
        if mesh.is_none() && scale < 0.0 {
            b.scene.sky = Some(pos);
            continue;
        }
        if is_start {
            b.scene.player_starts.push((pos, rotation));
            continue;
        }
        let live = if mesh.is_none() { class_statics.get(&i) } else { None };
        if mesh.is_none() && live.is_none() {
            continue;
        }
        if !include(i) {
            continue;
        }
        let resolved = match (live, mesh) {
            (Some(cs), _) => {
                scale = cs.scale;
                scale3d = GVec3::from(cs.scale3d);
                prepivot = GVec3::from(cs.prepivot);
                b.loader.set.get(&cs.mesh.0).ok().map(|p| (p, cs.mesh.1))
            }
            (None, Some(mesh)) => b.loader.set.resolve(map, mesh),
            (None, None) => None,
        };
        let Some((sp, si)) = resolved else { continue };
        let Some(asset) = b.loader.asset(&sp, si) else { continue };
        let Asset::StaticMesh(sm) = &*asset else { continue };
        let materials = b.loader.static_mesh_materials(&sp, si);
        // The ambient light of the zone(s) the mesh stands in, as the original adds it to every static mesh's colors: HP3
        // adds half of it (UT2004 the whole), both when it batches the level's meshes and when it draws one alone.
        let mesh_ambient = lighting.map(|l| b.ambient_at(l, pos, 48.0, l.ambient) * 0.5).unwrap_or(GVec3::ZERO);
        // Lighting baked by the editor for this placement: one color per mesh vertex (shadows included).
        let baked = instance.and_then(|r| b.loader.set.resolve(map, r)).and_then(|(ip, ii)| b.loader.asset(&ip, ii)).and_then(|a| match &*a {
            Asset::StaticMeshInstance(smi) if smi.colors.len() == sm.vertices.len() => Some(smi.colors.clone()),
            _ => None,
        });
        if std::env::var_os("HP3_LIGHT_ALL").is_some() { eprintln!("meshlight {}", if baked.is_some() { "baked" } else if instance.is_some() { "instance-mismatch" } else { "none" }); }
        let mesh_mul: f32 = std::env::var("HP3_MESH_MUL").ok().and_then(|v| v.parse().ok()).unwrap_or(0.6);
        let mesh_amb_mul: f32 = std::env::var("HP3_MESH_AMB_MUL").ok().and_then(|v| v.parse().ok()).unwrap_or(0.25);
        let transform = Mat4::from_translation(pos) * rotation_matrix(rotation) * Mat4::from_scale(scale3d * scale) * Mat4::from_translation(-prepivot);
        let uvs = sm.uv_streams.first().map(|s| s.uvs.as_slice()).unwrap_or(&[]);
        for (si_, section) in sm.sections.iter().enumerate() {
            let texture = match skins.get(si_).copied().filter(|s| !s.is_none()) {
                Some(skin) => b.texture_for(map, skin),
                None => materials.get(si_).copied().and_then(|m| b.texture_for(&sp, m)),
            };
            // A section that lights itself (the orb on the Carpe Retractum font) is drawn at full brightness.
            let self_lit = skins.get(si_).copied().is_none_or(|s| s.is_none()) && materials.get(si_).copied().is_some_and(|m| b.loader.is_self_lit(&sp, m));
            let normal_matrix = transform.inverse().transpose();
            let batch = b.batch(texture, None);
            let base = batch.vertices.len() as u32;
            let first = section.first_index as usize;
            let count = section.num_faces as usize * 3;
            let Some(idx) = sm.indices.get(first..first + count) else { continue };
            let mut remap: HashMap<u16, u32> = HashMap::new();
            for &vi in idx {
                let next = base + remap.len() as u32;
                let out = *remap.entry(vi).or_insert_with(|| {
                    let sv = sm.vertices.get(vi as usize);
                    let v = sv.map(|v| v.position).unwrap_or_default();
                    let p = transform.transform_point3(GVec3::new(v.x, v.y, v.z));
                    let uv = uvs.get(vi as usize).copied().unwrap_or([0.0, 0.0]);
                    let color = match (lighting, &baked) {
                        _ if self_lit => [0.5, 0.5, 0.5],
                        (Some(_), Some(colors)) => {
                            let c = colors[vi as usize];
                            (mesh_ambient * mesh_amb_mul + GVec3::new(c.r as f32, c.g as f32, c.b as f32) / 255.0 * mesh_mul).min(GVec3::ONE).to_array()
                        }
                        (Some(l), None) => {
                            let nv = sv.map(|v| v.normal).unwrap_or_default();
                            let normal = normal_matrix.transform_vector3(GVec3::new(nv.x, nv.y, nv.z)).normalize_or(GVec3::Z);
                            // Static mesh normals are not trusted yet: either side of a face takes light.
                            (mesh_ambient + l.lights_at(p, normal).max(l.lights_at(p, -normal))).min(GVec3::ONE).to_array()
                        }
                        (None, _) => [0.5, 0.5, 0.5],
                    };
                    batch.vertices.push(Vertex { position: p.to_array(), uv, light_uv: MESH_LIGHT_UV, color });
                    next
                });
                batch.indices.push(out);
            }
            b.scene.stats.mesh_triangles += section.num_faces as usize;
        }
        b.scene.stats.static_meshes += 1;
    }
}

#[cfg(test)]
mod actor_light_tests {
    use super::*;

    /// The light on an actor as the plain loop over every light gives it.
    fn plain(lighting: &Lighting, p: GVec3) -> GVec3 {
        lighting.ambient
            + lighting
                .lights
                .values()
                .map(|l| match l.direction {
                    Some(_) => l.color,
                    None => {
                        let d = l.position.distance(p);
                        if d >= l.radius {
                            GVec3::ZERO
                        } else {
                            let a = d / l.radius;
                            l.color * (2.0 * a * a * a - 3.0 * a * a + 1.0)
                        }
                    }
                })
                .sum::<GVec3>()
    }

    #[test]
    fn grid_matches_the_plain_loop() {
        let mut seed = 12345u32;
        let mut next = |range: f32| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 * range
        };
        let mut lighting = Lighting { ambient: GVec3::new(0.1, 0.1, 0.1), ..Default::default() };
        for i in 0..400 {
            let position = GVec3::new(next(8000.0) - 4000.0, next(8000.0) - 4000.0, next(600.0));
            let direction = (i % 97 == 0).then(|| GVec3::new(0.0, 0.0, -1.0));
            lighting.lights.insert(i, LightSource { position, color: GVec3::new(next(1.0), next(1.0), next(1.0)), radius: 100.0 + next(1500.0), direction, effect: 0, cone: 0.0, aim: GVec3::X });
        }
        for _ in 0..2000 {
            let p = GVec3::new(next(9000.0) - 4500.0, next(9000.0) - 4500.0, next(600.0));
            let (a, b) = (lighting.actor_light(p), plain(&lighting, p));
            assert!((a - b).abs().max_element() < 1e-4, "{a} vs {b} at {p}");
        }
    }
}

/// The soft shadow of a light at lightmap texel (`x`, `y`): the light's visibility bits blurred with the original's 3x3
/// filter (weights 24 40 24 / 40 64 40 / 24 40 24 over 320), the bitmap's edge texels repeating outward.
fn soft_shadow(bm: &hp3_assets::model::LightBitmap, x: i32, y: i32) -> f32 {
    const W: [[u32; 3]; 3] = [[24, 40, 24], [40, 64, 40], [24, 40, 24]];
    let (bx, by) = (x - bm.min[0], y - bm.min[1]);
    if bx < 0 || by < 0 || bx >= bm.size[0] || by >= bm.size[1] {
        return 0.0;
    }
    let bit = |cx: i32, cy: i32| -> u32 {
        let (cx, cy) = (cx.clamp(0, bm.size[0] - 1) as usize, cy.clamp(0, bm.size[1] - 1) as usize);
        let byte = cy * bm.stride.max(0) as usize + cx / 8;
        bm.bits.get(byte).is_some_and(|b| b & (1 << (cx % 8)) != 0) as u32
    };
    let mut acc = 0;
    for (j, row) in W.iter().enumerate() {
        for (i, w) in row.iter().enumerate() {
            acc += w * bit(bx + i as i32 - 1, by + j as i32 - 1);
        }
    }
    acc as f32 / 320.0
}

/// Distance fog: surfaces fade linearly to `color` between `start` and `end` units from the camera; nothing is drawn past `end`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fog {
    pub color: GVec3,
    pub start: f32,
    pub end: f32,
    /// The empty background is the fog's color too (`bClearToFogColor`).
    pub clear: bool,
}
