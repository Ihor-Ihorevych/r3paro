//! Geometry of individual meshes in their own space, for actors drawn with a per-frame transform: static
//! meshes, and skeletal meshes in their reference pose (until skeletal animation exists).

use std::rc::Rc;

use glam::{Mat4, Vec3};
use hp3_assets::loader::{self, Loader, MaterialBlend, ObjKey};
use hp3_assets::Asset;
use hp3_package::{ObjRef, Package};

use crate::scene::{rotation_matrix, Vertex};

/// Triangles of one mesh section, with the texture its material resolves to.
pub struct MeshBatch {
    pub texture: Option<ObjKey>,
    /// How the section uses its texture's alpha channel.
    pub blend: MaterialBlend,
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    /// Skeletal meshes: the mesh point each vertex is placed at, for re-posing.
    pub points: Vec<u32>,
    /// The section's skin is a video that is not there to play: it is drawn in a plain stand-in grey, darker than the usual one.
    pub placeholder: bool,
}

pub struct MeshGeom {
    pub batches: Vec<MeshBatch>,
    /// Mesh space to actor space (origin, scale and rotation offsets of vertex and skeletal meshes).
    pub local: Mat4,
    /// Skeletal meshes: -1 when the face normals must be turned round to point out of the mesh.
    pub normal_flip: f32,
}

fn texture(loader: &Loader<'_>, pkg: &Rc<Package>, material: ObjRef) -> Option<(ObjKey, MaterialBlend)> {
    let (tp, ti) = loader.base_texture(pkg, material)?;
    // A `MovieTexture` is a buffer the game fills by playing a Bink video (the animals' fur, the newspaper's photo). Without the
    // video it is blank, which would draw black: it counts as no texture, and the mesh takes the plain grey instead.
    if tp.export_class_name(ti).eq_ignore_ascii_case("MovieTexture") && loader.image(&tp, ti).is_none_or(|i| i.rgba.chunks_exact(4).all(|p| p[0] == 0 && p[1] == 0 && p[2] == 0)) {
        return None;
    }
    // A material that uses the texture's alpha channel (Harry's hair, the salamanders' flames) keeps it.
    let blend = loader.material_blend(pkg, material);
    // A shader with an opacity map blends by that map: its own combined picture.
    if blend == MaterialBlend::Alpha
        && let Some((sp, si)) = loader.shader_with_opacity(pkg, material)
    {
        return Some((loader::key(&sp, si), blend));
    }
    if blend == MaterialBlend::Opaque {
        loader.image(&tp, ti)?;
    } else {
        loader.cutout_image(&tp, ti)?;
    }
    Some((loader::key(&tp, ti), blend))
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
                // A section that lights itself is given a normal a thousand long, which the actor shader reads as "full brightness".
                let self_lit = skin(si).is_none() && materials.get(si).copied().is_some_and(|m| loader.is_self_lit(pkg, m));
                for &vi in idx {
                    let next = vertices.len() as u32;
                    let out = *remap.entry(vi).or_insert_with(|| {
                        let p = sm.vertices.get(vi as usize).map(|v| v.position).unwrap_or_default();
                        let uv = uvs.get(vi as usize).copied().unwrap_or([0.0, 0.0]);
                        let n = sm.vertices.get(vi as usize).map(|v| v.normal).unwrap_or_default();
                        let color = if self_lit { [0.0, 0.0, 1000.0] } else { [n.x, n.y, n.z] };
                        vertices.push(Vertex { color, ..Vertex::unlit([p.x, p.y, p.z], uv) });
                        next
                    });
                    indices.push(out);
                }
                if std::env::var_os("HP3_MESH_DEBUG").is_some() {
                    let name = materials.get(si).map(|m| pkg.ref_path(*m));
                    let resolved = skin(si).is_some() || materials.get(si).copied().and_then(|m| texture(loader, pkg, m)).is_some();
                    let zero = idx.iter().filter(|&&vi| sm.vertices.get(vi as usize).is_none_or(|v| (v.normal.x * v.normal.x + v.normal.y * v.normal.y + v.normal.z * v.normal.z) < 1e-6)).count();
                    eprintln!("mesh section {si}: zero-normal corners {zero}/{} faces", idx.len());
                    let sum = idx.iter().filter_map(|&vi| sm.vertices.get(vi as usize)).fold(Vec3::ZERO, |a, v| a + Vec3::new(v.normal.x, v.normal.y, v.normal.z)) / idx.len().max(1) as f32;
                    eprintln!("mesh section {si}: average normal {sum:.2} of {} corners", idx.len());
                    eprintln!("mesh section {si}: faces {} skin override {} material {name:?} resolved {resolved}", section.num_faces, skins.get(si).is_some_and(|(_, r)| !r.is_none()));
                }
                let (tex, blend) = skin(si).or_else(|| materials.get(si).copied().and_then(|m| texture(loader, pkg, m))).unzip();
                batches.push(MeshBatch { texture: tex, blend: blend.unwrap_or_default(), vertices, indices, points: Vec::new(), placeholder: false });
            }
            Some(MeshGeom { batches, local: Mat4::IDENTITY, normal_flip: 1.0 })
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
                let (tex, blend) = skin(mi).or_else(|| h.textures.get(tex_index).and_then(|t| texture(loader, pkg, *t))).unzip();
                batches.push(MeshBatch { texture: tex, blend: blend.unwrap_or_default(), vertices, indices, points, placeholder: false });
            }
            let scale = Vec3::new(h.mesh_scale.x, h.mesh_scale.y, h.mesh_scale.z);
            let origin = Vec3::new(h.mesh_origin.x, h.mesh_origin.y, h.mesh_origin.z);
            let local = rotation_matrix(h.rot_origin) * Mat4::from_scale(scale) * Mat4::from_translation(-origin);
            let mut geom = MeshGeom { batches, local, normal_flip: 1.0 };
            let rest: Vec<Vec3> = match points {
                Some(pts) => pts.to_vec(),
                None => m.points.iter().map(|p| Vec3::new(p.x, p.y, p.z)).collect(),
            };
            geom.normal_flip = normal_flip(&geom, &rest);
            let normals = point_normals(&geom, &rest);
            for b in &mut geom.batches {
                for (v, &p) in b.vertices.iter_mut().zip(&b.points) {
                    v.color = normals.get(p as usize).copied().unwrap_or(Vec3::Z).to_array();
                }
            }
            Some(geom)
        }
        // A vertex-animated mesh: the faces and wedges of its lod header, with each point taken from an animation frame.
        Asset::VertMesh(m) => {
            let h = &m.header;
            let rest: Vec<Vec3> = match points {
                Some(pts) => pts.to_vec(),
                None => m.frame_points(0).into_iter().map(Vec3::from).collect(),
            };
            let mut per_material: Vec<(Vec<Vertex>, Vec<u32>, std::collections::HashMap<u16, u32>, Vec<u32>)> = Vec::new();
            for f in &h.faces {
                let mi = f.material as usize;
                while per_material.len() <= mi {
                    per_material.push(Default::default());
                }
                let (verts, idx, remap, src) = &mut per_material[mi];
                for &w in &f.wedges {
                    let next = verts.len() as u32;
                    let out = *remap.entry(w).or_insert_with(|| {
                        let wedge = h.wedges.get(w as usize);
                        let vi = wedge.map(|w| w.vertex as usize).unwrap_or(0);
                        let p = rest.get(vi).copied().unwrap_or(Vec3::ZERO);
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
                let tex_index = h.materials.get(mi).map(|m| m.1 as usize).unwrap_or(mi).min(h.textures.len().saturating_sub(1));
                let (tex, blend) = skin(mi).or_else(|| h.textures.get(tex_index).and_then(|t| texture(loader, pkg, *t))).unzip();
                // A skin that was given but is a video the install lacks (the animals' fur) leaves a plain stand-in colour.
                let placeholder = tex.is_none() && skins.get(mi).is_some_and(|(_, r)| !r.is_none());
                batches.push(MeshBatch { texture: tex, blend: blend.unwrap_or_default(), vertices, indices, points, placeholder });
            }
            let scale = Vec3::new(h.mesh_scale.x, h.mesh_scale.y, h.mesh_scale.z);
            let origin = Vec3::new(h.mesh_origin.x, h.mesh_origin.y, h.mesh_origin.z);
            let local = rotation_matrix(h.rot_origin) * Mat4::from_scale(scale) * Mat4::from_translation(-origin);
            let mut geom = MeshGeom { batches, local, normal_flip: 1.0 };
            geom.normal_flip = normal_flip(&geom, &rest);
            let normals = point_normals(&geom, &rest);
            for b in &mut geom.batches {
                for (v, &p) in b.vertices.iter_mut().zip(&b.points) {
                    v.color = normals.get(p as usize).copied().unwrap_or(Vec3::Z).to_array();
                }
            }
            if std::env::var_os("HP3_MESH_DEBUG").is_some() {
                eprintln!(
                    "vertmesh: {} vertices, {} frames, header verts {} anim verts {}, {} faces, textures {}, scale {scale} origin {origin}",
                    h.vertex_count, m.anim_frames, h.verts.len(), m.anim_verts.len(), h.faces.len(), h.textures.len()
                );
                let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
                for w in &h.wedges {
                    lo = [lo[0].min(w.u), lo[1].min(w.v)];
                    hi = [hi[0].max(w.u), hi[1].max(w.v)];
                }
                eprintln!("vertmesh: wedge uv from {lo:?} to {hi:?} ({} wedges)", h.wedges.len());
            }
            Some(geom)
        }
        _ => None,
    }
}

/// Smooth vertex normals of a skeletal mesh per mesh point, from the faces of every batch around it (area weighted), turned
/// the way `geom.normal_flip` says (worked out once from the rest pose, as the winding of the source faces is not relied on).
pub fn point_normals(geom: &MeshGeom, points: &[Vec3]) -> Vec<Vec3> {
    let flip = geom.normal_flip;
    face_sums(geom, points).into_iter().map(|n| (n * flip).normalize_or(Vec3::Z)).collect()
}

/// The sum of the (area weighted) normals of the faces around each mesh point.
fn face_sums(geom: &MeshGeom, points: &[Vec3]) -> Vec<Vec3> {
    let mut sum = vec![Vec3::ZERO; points.len()];
    for b in &geom.batches {
        if b.points.len() != b.vertices.len() {
            continue;
        }
        for t in b.indices.chunks_exact(3) {
            let id = |k: usize| b.points[t[k] as usize] as usize;
            let (i0, i1, i2) = (id(0), id(1), id(2));
            let (Some(&p0), Some(&p1), Some(&p2)) = (points.get(i0), points.get(i1), points.get(i2)) else { continue };
            let n = (p1 - p0).cross(p2 - p0);
            sum[i0] += n;
            sum[i1] += n;
            sum[i2] += n;
        }
    }
    sum
}

/// -1 when the faces' normals mostly point into the middle of the mesh (its faces are wound the other way), else 1.
fn normal_flip(geom: &MeshGeom, points: &[Vec3]) -> f32 {
    let sum = face_sums(geom, points);
    let used: Vec<usize> = (0..sum.len()).filter(|&i| sum[i] != Vec3::ZERO).collect();
    let centre = used.iter().map(|&i| points[i]).sum::<Vec3>() / used.len().max(1) as f32;
    let outward: f32 = used.iter().map(|&i| sum[i].normalize_or_zero().dot((points[i] - centre).normalize_or_zero())).sum();
    if outward < 0.0 { -1.0 } else { 1.0 }
}
