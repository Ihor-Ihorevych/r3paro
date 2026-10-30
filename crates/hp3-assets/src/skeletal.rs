//! Skeletal meshes: the LOD mesh header, the reference skeleton, per-LOD render data and the source mesh.

use hp3_package::{Reader, Result};

use crate::anim::Quat;
use crate::common::Vec3;
use crate::lod_mesh::{Face, LodMeshHeader, Wedge, read_face, read_lod_header, read_wedge};

#[derive(Clone, Copy, Debug)]
pub struct Influence {
    pub weight: f32,
    pub vertex: u16,
    pub bone: u16,
}

fn read_influence(r: &mut Reader<'_>) -> Result<Influence> {
    Ok(Influence { weight: r.f32()?, vertex: r.u16()?, bone: r.u16()? })
}

#[derive(Clone, Copy, Debug)]
pub struct Triangle {
    pub wedges: [u16; 3],
    pub material: u8,
    pub aux_material: u8,
    pub smoothing_groups: u32,
}

#[derive(Clone, Debug)]
pub struct Bone {
    pub name: usize,
    pub flags: u32,
    pub orientation: Quat,
    pub position: Vec3,
    pub length: f32,
    pub size: Vec3,
    pub num_children: i32,
    pub parent: i32,
}

/// A run of triangles drawn with one material; the nine fields are not named yet.
#[derive(Clone, Debug)]
pub struct Section {
    pub fields: [u16; 9],
}

#[derive(Clone, Debug)]
pub struct LodModel {
    pub skinning_data: Vec<u32>,
    pub skin_points: Vec<[u8; 16]>,
    pub num_dyn_wedges: i32,
    pub smooth_sections: Vec<Section>,
    pub rigid_sections: Vec<Section>,
    pub smooth_indices: Vec<u16>,
    pub rigid_indices: Vec<u16>,
    pub vertex_stream: (i32, usize, i32, i32),
    pub influences: Vec<Influence>,
    pub wedges: Vec<Wedge>,
    pub faces: Vec<Face>,
    pub points: Vec<Vec3>,
    pub distance_factor: f32,
    pub hysteresis: f32,
    pub num_shared_verts: i32,
    pub max_influences: i32,
    pub unknown: [i32; 2],
}

#[derive(Clone, Debug)]
pub struct SkeletalMesh {
    pub header: LodMeshHeader,
    pub ref_skeleton: Vec<Bone>,
    /// The animation set used when an actor links none (a `MeshAnimation`).
    pub default_anim: hp3_package::ObjRef,
    pub skeletal_depth: i32,
    pub lod_models: Vec<LodModel>,
    pub points: Vec<Vec3>,
    pub wedges: Vec<Wedge>,
    pub triangles: Vec<Triangle>,
    pub influences: Vec<Influence>,
    pub collapse_wedges: Vec<u16>,
    pub face_level: Vec<u16>,
    pub trailer: [i32; 2],
}

fn indices(r: &mut Reader<'_>) -> Result<Vec<u16>> {
    let v = r.array(|r| r.u16())?;
    r.i32()?;
    Ok(v)
}

fn read_section(r: &mut Reader<'_>) -> Result<Section> {
    let mut fields = [0u16; 9];
    for f in &mut fields {
        *f = r.u16()?;
    }
    Ok(Section { fields })
}

fn read_lod_model(r: &mut Reader<'_>) -> Result<LodModel> {
    Ok(LodModel {
        skinning_data: r.array(|r| r.u32())?,
        skin_points: r.array(|r| Ok(r.bytes(16)?.try_into().unwrap()))?,
        num_dyn_wedges: r.i32()?,
        smooth_sections: r.array(read_section)?,
        rigid_sections: r.array(read_section)?,
        smooth_indices: indices(r)?,
        rigid_indices: indices(r)?,
        vertex_stream: (r.i32()?, r.count()?, r.i32()?, r.i32()?),
        influences: r.lazy_array(read_influence)?,
        wedges: r.lazy_array(read_wedge)?,
        faces: r.lazy_array(read_face)?,
        points: r.lazy_array(Vec3::read)?,
        distance_factor: r.f32()?,
        hysteresis: r.f32()?,
        num_shared_verts: r.i32()?,
        max_influences: r.i32()?,
        unknown: [r.i32()?, r.i32()?],
    })
}

pub fn read_skeletal_mesh(r: &mut Reader<'_>) -> Result<SkeletalMesh> {
    let header = read_lod_header(r)?;
    let ref_skeleton = r.array(|r| {
        Ok(Bone {
            name: r.ci()? as usize,
            flags: r.u32()?,
            orientation: Quat::read(r)?,
            position: Vec3::read(r)?,
            length: r.f32()?,
            size: Vec3::read(r)?,
            num_children: r.i32()?,
            parent: r.i32()?,
        })
    })?;
    let default_anim = hp3_package::ObjRef(r.ci()?);
    let skeletal_depth = r.i32()?;
    let _ = r.i32()?;
    let _ = r.array(|r| r.i32())?;
    let lod_models = r.array(read_lod_model)?;
    let _ = r.array(|r| r.i32())?;
    Ok(SkeletalMesh {
        header,
        ref_skeleton,
        default_anim,
        skeletal_depth,
        lod_models,
        points: r.lazy_array(Vec3::read)?,
        wedges: r.lazy_array(read_wedge)?,
        triangles: r.lazy_array(|r| {
            Ok(Triangle {
                wedges: [r.u16()?, r.u16()?, r.u16()?],
                material: r.u8()?,
                aux_material: r.u8()?,
                smoothing_groups: r.u32()?,
            })
        })?,
        influences: r.lazy_array(read_influence)?,
        collapse_wedges: r.lazy_array(|r| r.u16())?,
        face_level: r.lazy_array(|r| r.u16())?,
        trailer: [r.i32()?, r.i32()?],
    })
}
