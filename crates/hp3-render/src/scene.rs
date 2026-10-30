//! A static scene built from a map: BSP render sections and static mesh actors as textured triangle batches.

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
}

/// Triangles sharing one texture.
pub struct Batch {
    pub texture: Option<ObjKey>,
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
    pub player_starts: Vec<(GVec3, [i32; 3])>,
    pub stats: SceneStats,
}

#[derive(Default, Debug)]
pub struct SceneStats {
    pub bsp_triangles: usize,
    pub static_meshes: usize,
    pub mesh_triangles: usize,
    pub missing_textures: usize,
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
    by_texture: HashMap<Option<ObjKey>, usize>,
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

    fn batch(&mut self, texture: Option<ObjKey>) -> &mut Batch {
        let next = self.scene.batches.len();
        let i = *self.by_texture.entry(texture.clone()).or_insert(next);
        if i == next {
            self.scene.batches.push(Batch { texture, vertices: Vec::new(), indices: Vec::new() });
        }
        &mut self.scene.batches[i]
    }
}

/// Builds the scene of the level in `map` (a loaded map package).
pub fn build(loader: &Loader<'_>, map: &Rc<Package>) -> Scene {
    let mut b = Batcher { loader, by_texture: HashMap::new(), scene: Scene::default() };
    add_bsp(&mut b, map);
    add_actors(&mut b, map);
    b.scene
}

fn add_bsp(b: &mut Batcher<'_, '_>, map: &Rc<Package>) {
    let Some(level_index) = (0..map.exports.len()).find(|&i| map.export_class_name(i).eq_ignore_ascii_case("Level"))
    else {
        return;
    };
    let Some(level_asset) = b.loader.asset(map, level_index) else { return };
    let Asset::Level(level) = &*level_asset else { return };
    let Some((mp, mi)) = b.loader.set.resolve(map, level.model) else { return };
    let Some(model_asset) = b.loader.asset(&mp, mi) else { return };
    let Asset::Model(model) = &*model_asset else { return };
    // Invisible, portal and fake-backdrop surfaces are not drawn.
    const HIDDEN: u32 = 0x0000_0001 | 0x0400_0000 | 0x0000_0080;
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
        let batch = b.batch(texture);
        let base = batch.vertices.len() as u32;
        for v in verts {
            batch.vertices.push(Vertex { position: [v.position.x, v.position.y, v.position.z], uv: v.uv });
        }
        for k in 1..n as u32 - 1 {
            batch.indices.extend_from_slice(&[base, base + k, base + k + 1]);
        }
        b.scene.stats.bsp_triangles += n - 2;
    }
}

fn add_actors(b: &mut Batcher<'_, '_>, map: &Rc<Package>) {
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
        if is_start {
            b.scene.player_starts.push((pos, rotation));
            continue;
        }
        let Some(mesh) = mesh else { continue };
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
            let batch = b.batch(texture);
            let base = batch.vertices.len() as u32;
            let first = section.first_index as usize;
            let count = section.num_faces as usize * 3;
            let Some(idx) = sm.indices.get(first..first + count) else { continue };
            let mut remap: HashMap<u16, u32> = HashMap::new();
            for &vi in idx {
                let next = base + remap.len() as u32;
                let out = *remap.entry(vi).or_insert_with(|| {
                    let v = sm.vertices.get(vi as usize).map(|v| v.position).unwrap_or_default();
                    let p = transform.transform_point3(GVec3::new(v.x, v.y, v.z));
                    let uv = uvs.get(vi as usize).copied().unwrap_or([0.0, 0.0]);
                    batch.vertices.push(Vertex { position: p.to_array(), uv });
                    next
                });
                batch.indices.push(out);
            }
            b.scene.stats.mesh_triangles += section.num_faces as usize;
        }
        b.scene.stats.static_meshes += 1;
    }
}
