//! Static meshes: render streams, the collision triangles and their tree, and the raw source triangles.

use hp3_package::{ObjRef, Reader, Result};

use crate::common::{Bounds, Color, PrimitiveBounds, Vec3};

#[derive(Clone, Debug)]
pub struct Section {
    pub flags: i32,
    pub first_index: u16,
    pub first_vertex: u16,
    pub last_vertex: u16,
    pub unknown: u16,
    pub num_faces: u16,
}

#[derive(Clone, Copy, Debug)]
pub struct Vertex {
    pub position: Vec3,
    pub normal: Vec3,
}

#[derive(Clone, Debug)]
pub struct UvStream {
    pub uvs: Vec<[f32; 2]>,
    pub coordinate_index: i32,
    pub revision: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct CollisionTriangle {
    pub vertices: [u16; 3],
    pub material: u8,
}

/// One node of the collision tree; its layout is kept raw until the tree is decoded.
#[derive(Clone, Copy, Debug)]
pub struct CollisionNode(pub [u8; 20]);

#[derive(Clone, Debug)]
pub struct RawTriangle {
    pub vertices: [Vec3; 3],
    /// `uvs[channel][corner]`.
    pub uvs: Vec<[[f32; 2]; 3]>,
    pub colors: [Color; 3],
    pub material: i32,
    pub smoothing_mask: u32,
}

#[derive(Clone, Debug)]
pub struct StaticMesh {
    pub bounds: PrimitiveBounds,
    pub sections: Vec<Section>,
    pub mesh_bounds: Bounds,
    pub vertices: Vec<Vertex>,
    pub colors: Vec<Color>,
    pub alphas: Vec<Color>,
    pub uv_streams: Vec<UvStream>,
    pub indices: Vec<u16>,
    pub wireframe_indices: Vec<u16>,
    pub collision_model: ObjRef,
    pub collision_triangles: Vec<CollisionTriangle>,
    pub collision_nodes: Vec<CollisionNode>,
    pub collision_scale: Vec3,
    pub raw_triangles: Vec<RawTriangle>,
    pub internal_version: i32,
    /// Karma collision properties (a `KMeshProps` object).
    pub karma_props: ObjRef,
    pub unknown_trailer: i32,
}

fn indices(r: &mut Reader<'_>) -> Result<Vec<u16>> {
    let v = r.array(|r| r.u16())?;
    r.i32()?;
    Ok(v)
}

pub fn read_static_mesh(r: &mut Reader<'_>) -> Result<StaticMesh> {
    let bounds = PrimitiveBounds::read(r)?;
    let sections = r.array(|r| {
        Ok(Section {
            flags: r.i32()?,
            first_index: r.u16()?,
            first_vertex: r.u16()?,
            last_vertex: r.u16()?,
            unknown: r.u16()?,
            num_faces: r.u16()?,
        })
    })?;
    let mesh_bounds = Bounds::read(r)?;
    let vertices = r.array(|r| Ok(Vertex { position: Vec3::read(r)?, normal: Vec3::read(r)? }))?;
    r.i32()?;
    let colors = r.array(Color::read)?;
    r.i32()?;
    let alphas = r.array(Color::read)?;
    r.i32()?;
    let uv_streams = r.array(|r| {
        Ok(UvStream {
            uvs: r.array(|r| Ok([r.f32()?, r.f32()?]))?,
            coordinate_index: r.i32()?,
            revision: r.i32()?,
        })
    })?;
    let indices_ = indices(r)?;
    let wireframe_indices = indices(r)?;
    let collision_model = ObjRef(r.ci()?);
    let collision_triangles = r.array(|r| {
        Ok(CollisionTriangle { vertices: [r.u16()?, r.u16()?, r.u16()?], material: r.u8()? })
    })?;
    let collision_nodes = r.array(|r| Ok(CollisionNode(r.bytes(20)?.try_into().unwrap())))?;
    let collision_scale = Vec3::read(r)?;
    let raw_triangles = r.lazy_array(|r| {
        let vertices = [Vec3::read(r)?, Vec3::read(r)?, Vec3::read(r)?];
        let channels = r.i32()?;
        if !(0..=8).contains(&channels) {
            return Err(hp3_package::Error::Format(format!("bad uv channel count {channels}")));
        }
        let mut uvs = Vec::with_capacity(channels as usize);
        for _ in 0..channels {
            uvs.push([[r.f32()?, r.f32()?], [r.f32()?, r.f32()?], [r.f32()?, r.f32()?]]);
        }
        Ok(RawTriangle {
            vertices,
            uvs,
            colors: [Color::read(r)?, Color::read(r)?, Color::read(r)?],
            material: r.i32()?,
            smoothing_mask: r.u32()?,
        })
    })?;
    let internal_version = r.i32()?;
    let karma_props = ObjRef(r.ci()?);
    let unknown_trailer = r.i32()?;
    Ok(StaticMesh {
        bounds,
        sections,
        mesh_bounds,
        vertices,
        colors,
        alphas,
        uv_streams,
        indices: indices_,
        wireframe_indices,
        collision_model,
        collision_triangles,
        collision_nodes,
        collision_scale,
        raw_triangles,
        internal_version,
        karma_props,
        unknown_trailer,
    })
}
