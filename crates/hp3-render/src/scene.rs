//! A static scene built from a map: BSP render sections and static mesh actors as textured triangle batches,
//! lit by lightmaps (BSP) and vertex colors (meshes) computed from the map's lights.
//!
//! The lighting model is an approximation of Unreal's, NOT yet matched to HP3's renderer: light color from
//! hue, saturation and brightness; radius 25 * (LightRadius + 1); falloff 1 - (d/R)^2 times the cosine to the
//! surface; sunlights directional without falloff; BSP texels lit only where the light's shadow bitmap allows;
//! the zone's ambient light added everywhere. Values are stored halved and doubled in the shader.

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

/// A light, as the scene builder needs it.
#[derive(Clone, Copy, Debug)]
pub struct LightSource {
    pub position: GVec3,
    /// Color times brightness (1.0 = full).
    pub color: GVec3,
    pub radius: f32,
    /// Sunlight: the direction light travels; no falloff.
    pub direction: Option<GVec3>,
}

impl LightSource {
    /// Light this source adds at `p` on a surface facing `normal` (shadowing aside).
    pub fn at(&self, p: GVec3, normal: GVec3) -> GVec3 {
        match self.direction {
            Some(d) => self.color * normal.dot(-d).max(0.0),
            None => {
                let to = self.position - p;
                let d = to.length();
                if d >= self.radius {
                    return GVec3::ZERO;
                }
                let falloff = 1.0 - (d / self.radius).powi(2);
                let cos = if d > 1e-3 { normal.dot(to / d).max(0.0) } else { 1.0 };
                self.color * falloff * cos
            }
        }
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

/// What the scene builder takes from the running world: lighting and terrain layers (by the terrain actor's
/// export index in the map).
#[derive(Default)]
pub struct MapInputs {
    pub lighting: Option<Lighting>,
    pub terrain_layers: HashMap<usize, Vec<TerrainLayer>>,
}

/// The map's lights, by export index in the map package, and its ambient light.
#[derive(Default)]
pub struct Lighting {
    pub lights: HashMap<usize, LightSource>,
    /// Ambient light where no zone applies (meshes and actors, for now).
    pub ambient: GVec3,
    /// Ambient light of each zone, by the zone actor's export index in the map.
    pub zone_ambient: HashMap<usize, GVec3>,
}

impl Lighting {
    /// Light on an actor at `p`, from every direction (no surface angle): one level for the whole actor.
    pub fn actor_light(&self, p: GVec3) -> GVec3 {
        self.ambient
            + self
                .lights
                .values()
                .map(|l| match l.direction {
                    Some(_) => l.color * 0.5,
                    None => {
                        let d = l.position.distance(p);
                        if d >= l.radius { GVec3::ZERO } else { l.color * (1.0 - (d / l.radius).powi(2)) }
                    }
                })
                .sum::<GVec3>()
    }

    /// Light at a point from every light in range (no shadows), plus ambient.
    pub fn at(&self, p: GVec3, normal: GVec3) -> GVec3 {
        self.ambient + self.lights.values().map(|l| l.at(p, normal)).sum::<GVec3>()
    }
}

/// Triangles sharing one texture.
pub struct Batch {
    pub texture: Option<ObjKey>,
    /// Lightmap page (index into `Scene::lightmaps`), if lit by one.
    pub lightmap: Option<usize>,
    /// A terrain layer: blended over what is below by this alpha map (sampled with `light_uv`).
    pub blend: Option<ObjKey>,
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
        // Unreal's world is left-handed with Z up.
        let view = Mat4::look_to_lh(self.position, self.forward(), GVec3::Z);
        // Unreal's field of view is horizontal.
        let fov_y = 2.0 * ((self.fov_degrees.to_radians() / 2.0).tan() / aspect).atan();
        let proj = Mat4::perspective_lh(fov_y, aspect, 4.0, 65536.0);
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
    by_texture: HashMap<(Option<ObjKey>, Option<usize>), usize>,
    scene: Scene,
}

impl Batcher<'_, '_> {
    fn texture_for(&mut self, pkg: &Rc<Package>, material: ObjRef) -> Option<ObjKey> {
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
        let i = *self.by_texture.entry((texture.clone(), lightmap)).or_insert(next);
        if i == next {
            self.scene.batches.push(Batch { texture, lightmap, blend: None, vertices: Vec::new(), indices: Vec::new() });
        }
        &mut self.scene.batches[i]
    }
}

/// Builds the scene of the level in `map` (a loaded map package).
pub fn build(loader: &Loader<'_>, map: &Rc<Package>) -> Scene {
    build_with(loader, map, &|_| true, None)
}

/// Like `build_with`, also drawing terrain with the given layers.
pub fn build_map(loader: &Loader<'_>, map: &Rc<Package>, include: &dyn Fn(usize) -> bool, inputs: &MapInputs) -> Scene {
    let mut scene = build_with(loader, map, include, inputs.lighting.as_ref());
    let mut b = Batcher { loader, by_texture: HashMap::new(), scene: std::mem::take(&mut scene) };
    add_terrain(&mut b, map, inputs);
    b.scene
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
        let quad_visible = |x: usize, y: usize| {
            let q = y * (w - 1) + x;
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
        let color: Vec<[f32; 3]> = (0..w * h)
            .map(|k| match &inputs.lighting {
                Some(l) => (l.at(pos(k), normals[k].normalize_or(GVec3::Z)) * 0.5).to_array(),
                None => [0.5, 0.5, 0.5],
            })
            .collect();
        for (li, layer) in layers.iter().enumerate() {
            let Some(texture) = b.texture_key(&layer.texture) else { continue };
            let alpha = if li == 0 { None } else { layer.alpha_map.as_ref().and_then(|k| b.alpha_key(k)) };
            if li > 0 && alpha.is_none() {
                continue;
            }
            let m = layer.matrix;
            let mut batch = Batch { texture: Some(texture), lightmap: None, blend: alpha, vertices: Vec::with_capacity(w * h), indices: Vec::new() };
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
                    batch.indices.extend_from_slice(&[a, bb, d, a, d, c]);
                }
            }
            b.scene.stats.terrain_triangles += batch.indices.len() / 3;
            b.scene.batches.push(batch);
        }
    }
}

/// Builds the scene with only the static-mesh actors whose export index passes `include`; the others are drawn
/// per frame.
/// per frame. With `lighting`, surfaces are lit; without, everything is drawn at full brightness.
pub fn build_with(loader: &Loader<'_>, map: &Rc<Package>, include: &dyn Fn(usize) -> bool, lighting: Option<&Lighting>) -> Scene {
    let mut b = Batcher { loader, by_texture: HashMap::new(), scene: Scene::default() };
    add_bsp(&mut b, map, lighting);
    add_actors(&mut b, map, include, lighting);
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
    let mut pages: Vec<Vec<f32>> = vec![Vec::new(); model.light_map_textures.len()];
    for (pi, page) in model.light_map_textures.iter().enumerate() {
        let mut texels = vec![0.0f32; n * n * 3];
        for &lmi in &page.light_maps {
            let Some(lm) = model.light_maps.get(lmi as usize) else { continue };
            let Some(surf) = model.surfs.get(lm.surf as usize) else { continue };
            let normal = model.vectors.get(surf.normal as usize).map(|v| GVec3::new(v.x, v.y, v.z)).unwrap_or(GVec3::Z);
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
                    let mut c = surf_ambient.get(&lm.surf).copied().unwrap_or(lighting.ambient);
                    for (l, bm) in &lights {
                        // A bitmap covers only the texels from `min` to `max`, one bit each, `stride` bytes a row.
                        let (bx, by) = (x as i32 - bm.min[0], y as i32 - bm.min[1]);
                        if bx < 0 || by < 0 || bx >= bm.size[0] || by >= bm.size[1] {
                            continue;
                        }
                        let (bx, by) = (bx as usize, by as usize);
                        let byte = by * bm.stride.max(0) as usize + bx / 8;
                        let visible = bm.bits.get(byte).is_some_and(|b| b & (1 << (bx % 8)) != 0);
                        if visible {
                            c += l.at(p, normal);
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
                    rgba[i * 4 + k] = (texels[i * 3 + k] * 0.5 * 255.0).clamp(0.0, 255.0) as u8;
                }
            }
            Image { width: n as u32, height: n as u32, rgba, masked: false }
        })
        .collect()
}

fn add_bsp(b: &mut Batcher<'_, '_>, map: &Rc<Package>, lighting: Option<&Lighting>) {
    let Some(level_index) = (0..map.exports.len()).find(|&i| map.export_class_name(i).eq_ignore_ascii_case("Level"))
    else {
        return;
    };
    let Some(level_asset) = b.loader.asset(map, level_index) else { return };
    let Asset::Level(level) = &*level_asset else { return };
    let Some((mp, mi)) = b.loader.set.resolve(map, level.model) else { return };
    let Some(model_asset) = b.loader.asset(&mp, mi) else { return };
    let Asset::Model(model) = &*model_asset else { return };
    if let Some(l) = lighting {
        b.scene.lightmaps = build_lightmaps(model, l);
    }
    // Invisible, portal and fake-backdrop surfaces are not drawn.
    const HIDDEN: u32 = 0x0000_0001 | 0x0400_0000 | 0x0000_0080;
    // Unlit surfaces are drawn at full brightness.
    const UNLIT: u32 = 0x0040_0000;
    for node in &model.nodes {
        if node.num_vertices < 3 || node.section < 0 {
            continue;
        }
        let Some(section) = model.sections.get(node.section as usize) else { continue };
        let surf_flags = model.surfs.get(node.surf as usize).map(|s| s.poly_flags).unwrap_or(0);
        if (surf_flags | section.poly_flags) & HIDDEN != 0 {
            continue;
        }
        let texture = b.texture_for(&mp, section.material);
        let first = node.first_vertex as usize;
        let n = node.num_vertices as usize;
        let Some(verts) = section.vertices.get(first..first + n) else { continue };
        let page = (section.light_map_texture as usize) < b.scene.lightmaps.len();
        let lit = page && (surf_flags | section.poly_flags) & UNLIT == 0;
        let lightmap = lit.then_some(section.light_map_texture as usize);
        // Lit surfaces take their light from the lightmap, the others from the white one; the vertex color is neutral.
        let color = [0.5, 0.5, 0.5];
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
}

fn add_actors(b: &mut Batcher<'_, '_>, map: &Rc<Package>, include: &dyn Fn(usize) -> bool, lighting: Option<&Lighting>) {
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
                return Some((None, location, rotation, 1.0, GVec3::ONE, Vec::new(), true));
            }
            if chain.first().is_some_and(|c| c.eq_ignore_ascii_case("SkyZoneInfo")) {
                return Some((None, location, rotation, -1.0, GVec3::ONE, Vec::new(), false));
            }
            let mesh = f("StaticMesh").and_then(|t| t.as_obj())?;
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
            Some((Some(mesh), location, rotation, scale, scale3d, skins, false))
        });
        let Some(Some((mesh, location, rotation, scale, scale3d, skins, is_start))) = info else { continue };
        let pos = GVec3::new(location.x, location.y, location.z);
        if mesh.is_none() && scale < 0.0 {
            b.scene.sky = Some(pos);
            continue;
        }
        if is_start {
            b.scene.player_starts.push((pos, rotation));
            continue;
        }
        let Some(mesh) = mesh else { continue };
        if !include(i) {
            continue;
        }
        let Some((sp, si)) = b.loader.set.resolve(map, mesh) else { continue };
        let Some(asset) = b.loader.asset(&sp, si) else { continue };
        let Asset::StaticMesh(sm) = &*asset else { continue };
        let materials = b.loader.static_mesh_materials(&sp, si);
        let transform = Mat4::from_translation(pos) * rotation_matrix(rotation) * Mat4::from_scale(scale3d * scale);
        let uvs = sm.uv_streams.first().map(|s| s.uvs.as_slice()).unwrap_or(&[]);
        for (si_, section) in sm.sections.iter().enumerate() {
            let texture = match skins.get(si_).copied().filter(|s| !s.is_none()) {
                Some(skin) => b.texture_for(map, skin),
                None => materials.get(si_).copied().and_then(|m| b.texture_for(&sp, m)),
            };
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
                    let color = match lighting {
                        Some(l) => {
                            let nv = sv.map(|v| v.normal).unwrap_or_default();
                            let normal = normal_matrix.transform_vector3(GVec3::new(nv.x, nv.y, nv.z)).normalize_or(GVec3::Z);
                            // Static mesh normals are not trusted yet: either side of a face takes light.
                            (l.at(p, normal).max(l.at(p, -normal)) * 0.5).to_array()
                        }
                        None => [0.5, 0.5, 0.5],
                    };
                    batch.vertices.push(Vertex { position: p.to_array(), uv, light_uv: [0.0, 0.0], color });
                    next
                });
                batch.indices.push(out);
            }
            b.scene.stats.mesh_triangles += section.num_faces as usize;
        }
        b.scene.stats.static_meshes += 1;
    }
}
