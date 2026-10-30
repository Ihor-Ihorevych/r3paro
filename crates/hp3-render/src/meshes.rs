//! Geometry of individual meshes in their own space, for actors drawn with a per-frame transform: static
//! meshes, and skeletal meshes in their reference pose (until skeletal animation exists).

use std::rc::Rc;

use glam::{Mat4, Vec3};
use hp3_assets::loader::{self, Loader, ObjKey};
use hp3_assets::Asset;
use hp3_package::{ObjRef, Package};

use crate::scene::{rotation_matrix, Vertex};

/// Triangles of one mesh section, with the texture its material resolves to.
pub struct MeshBatch {
    pub texture: Option<ObjKey>,
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    /// Skeletal meshes: the mesh point each vertex is placed at, for re-posing.
    pub points: Vec<u32>,
}

pub struct MeshGeom {
    pub batches: Vec<MeshBatch>,
    /// Mesh space to actor space (origin, scale and rotation offsets of vertex and skeletal meshes).
    pub local: Mat4,
}

fn texture(loader: &Loader<'_>, pkg: &Rc<Package>, material: ObjRef) -> Option<ObjKey> {
    let (tp, ti) = loader.base_texture(pkg, material)?;
    loader.image(&tp, ti)?;
    Some(loader::key(&tp, ti))
}

/// Builds the geometry of a static or skeletal mesh export; `skins` override section materials by index.
pub fn build(loader: &Loader<'_>, pkg: &Rc<Package>, index: usize, skins: &[(Rc<Package>, ObjRef)]) -> Option<MeshGeom> {
    build_posed(loader, pkg, index, skins, None)
}

/// Like `build`; for skeletal meshes, `points` replaces the reference positions (a skinned pose).
pub fn build_posed(
    loader: &Loader<'_>,
    pkg: &Rc<Package>,
    index: usize,
    skins: &[(Rc<Package>, ObjRef)],
    points: Option<&[Vec3]>,
) -> Option<MeshGeom> {
    let asset = loader.asset(pkg, index)?;
    let skin = |i: usize| skins.get(i).filter(|(_, r)| !r.is_none()).and_then(|(p, r)| texture(loader, p, *r));
    match &*asset {
        Asset::StaticMesh(sm) => {
            let materials = loader.static_mesh_materials(pkg, index);
            let uvs = sm.uv_streams.first().map(|s| s.uvs.as_slice()).unwrap_or(&[]);
            let mut batches = Vec::new();
            for (si, section) in sm.sections.iter().enumerate() {
                let first = section.first_index as usize;
                let count = section.num_faces as usize * 3;
                let Some(idx) = sm.indices.get(first..first + count) else { continue };
                let mut vertices = Vec::new();
                let mut indices = Vec::new();
                let mut remap = std::collections::HashMap::new();
                for &vi in idx {
                    let next = vertices.len() as u32;
                    let out = *remap.entry(vi).or_insert_with(|| {
                        let p = sm.vertices.get(vi as usize).map(|v| v.position).unwrap_or_default();
                        let uv = uvs.get(vi as usize).copied().unwrap_or([0.0, 0.0]);
                        vertices.push(Vertex::unlit([p.x, p.y, p.z], uv));
                        next
                    });
                    indices.push(out);
                }
                let tex = skin(si).or_else(|| materials.get(si).copied().and_then(|m| texture(loader, pkg, m)));
                batches.push(MeshBatch { texture: tex, vertices, indices, points: Vec::new() });
            }
            Some(MeshGeom { batches, local: Mat4::IDENTITY })
        }
        Asset::SkeletalMesh(m) => {
            let h = &m.header;
            // One batch per material, from the reference triangles.
            let mut per_material: Vec<(Vec<Vertex>, Vec<u32>, std::collections::HashMap<u16, u32>, Vec<u32>)> = Vec::new();
            for t in &m.triangles {
                let mi = t.material as usize;
                while per_material.len() <= mi {
                    per_material.push(Default::default());
                }
                let (verts, idx, remap, src) = &mut per_material[mi];
                for &w in &t.wedges {
                    let next = verts.len() as u32;
                    let out = *remap.entry(w).or_insert_with(|| {
                        let wedge = m.wedges.get(w as usize);
                        let vi = wedge.map(|w| w.vertex as usize).unwrap_or(0);
                        let p = match points {
                            Some(pts) => pts.get(vi).copied().unwrap_or(Vec3::ZERO),
                            None => m.points.get(vi).map(|p| Vec3::new(p.x, p.y, p.z)).unwrap_or(Vec3::ZERO),
                        };
                        let uv = wedge.map(|w| [w.u, w.v]).unwrap_or([0.0, 0.0]);
                        verts.push(Vertex::unlit(p.to_array(), uv));
                        src.push(vi as u32);
                        next
                    });
                    idx.push(out);
                }
            }
            let mut batches = Vec::new();
            for (mi, (vertices, indices, _, points)) in per_material.into_iter().enumerate() {
                if indices.is_empty() {
                    continue;
                }
                // A texture index past the mesh's textures takes the last one (Hermione's hair has ten material
                // slots and nine textures; the last is the hair). A heuristic, not checked against HP3.
                let tex_index = h.materials.get(mi).map(|m| m.1 as usize).unwrap_or(mi).min(h.textures.len().saturating_sub(1));
                let tex = skin(mi).or_else(|| h.textures.get(tex_index).and_then(|t| texture(loader, pkg, *t)));
                batches.push(MeshBatch { texture: tex, vertices, indices, points });
            }
            let scale = Vec3::new(h.mesh_scale.x, h.mesh_scale.y, h.mesh_scale.z);
            let origin = Vec3::new(h.mesh_origin.x, h.mesh_origin.y, h.mesh_origin.z);
            let local = rotation_matrix(h.rot_origin) * Mat4::from_scale(scale) * Mat4::from_translation(-origin);
            Some(MeshGeom { batches, local })
        }
        _ => None,
    }
}
