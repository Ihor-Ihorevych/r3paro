//! The header shared by skeletal and vertex-animated meshes, and vertex-animated meshes themselves.

use hp3_package::{ObjRef, Reader, Result};

use crate::common::{Bounds, PrimitiveBounds, Sphere, Vec3};

#[derive(Clone, Copy, Debug)]
pub struct Wedge {
    pub vertex: u16,
    pub u: f32,
    pub v: f32,
}

pub fn read_wedge(r: &mut Reader<'_>) -> Result<Wedge> {
    Ok(Wedge { vertex: r.u16()?, u: r.f32()?, v: r.f32()? })
}

#[derive(Clone, Copy, Debug)]
pub struct Face {
    pub wedges: [u16; 3],
    pub material: u16,
}

pub fn read_face(r: &mut Reader<'_>) -> Result<Face> {
    Ok(Face { wedges: [r.u16()?, r.u16()?, r.u16()?], material: r.u16()? })
}

#[derive(Clone, Debug)]
pub struct LodMeshHeader {
    pub bounds: PrimitiveBounds,
    pub version: i32,
    pub vertex_count: i32,
    /// Packed positions (11/11/10 bits), `vertex_count` per animation frame.
    pub verts: Vec<u32>,
    pub textures: Vec<ObjRef>,
    pub mesh_scale: Vec3,
    pub mesh_origin: Vec3,
    pub rot_origin: [i32; 3],
    pub face_level: Vec<u16>,
    pub faces: Vec<Face>,
    pub collapse_wedge_thus: Vec<u16>,
    pub wedges: Vec<Wedge>,
    pub materials: Vec<(u32, i32)>,
    pub mesh_scale_max: f32,
    /// Hysteresis, strength, minimum vertices, morph and Z displacement.
    pub lod: [f32; 5],
}

pub fn read_lod_header(r: &mut Reader<'_>) -> Result<LodMeshHeader> {
    let h = LodMeshHeader {
        bounds: PrimitiveBounds::read(r)?,
        version: r.i32()?,
        vertex_count: r.i32()?,
        verts: r.array(|r| r.u32())?,
        textures: r.array(|r| Ok(ObjRef(r.ci()?)))?,
        mesh_scale: Vec3::read(r)?,
        mesh_origin: Vec3::read(r)?,
        rot_origin: [r.i32()?, r.i32()?, r.i32()?],
        face_level: r.array(|r| r.u16())?,
        faces: r.array(read_face)?,
        collapse_wedge_thus: r.array(|r| r.u16())?,
        wedges: r.array(read_wedge)?,
        materials: r.array(|r| Ok((r.u32()?, r.i32()?)))?,
        mesh_scale_max: r.f32()?,
        lod: [r.f32()?, r.f32()?, r.f32()?, r.f32()?, r.f32()?],
    };
    // Zero in every mesh seen so far.
    r.skip(62)?;
    Ok(h)
}

#[derive(Clone, Debug)]
pub struct VertMesh {
    pub header: LodMeshHeader,
    pub sequences: Vec<crate::anim::Sequence>,
    /// Packed positions of every frame.
    pub anim_verts: Vec<u32>,
    pub frame_verts: i32,
    pub anim_frames: i32,
    pub frame_bounds: Vec<Bounds>,
    pub frame_spheres: Vec<Sphere>,
}

/// A packed vertex of a vertex-animated mesh: three signed fields of 11, 11 and 10 bits (the original's `FMeshVert`).
pub fn unpack_vert(v: u32) -> [f32; 3] {
    let x = ((v & 0x7FF) as i32) << 21 >> 21;
    let y = (((v >> 11) & 0x7FF) as i32) << 21 >> 21;
    let z = (((v >> 22) & 0x3FF) as i32) << 22 >> 22;
    [x as f32, y as f32, z as f32]
}

impl VertMesh {
    /// The packed positions of every frame, `header.vertex_count` to a frame: the lod header's array when it holds all the frames,
    /// else the mesh's own.
    fn frame_data(&self) -> &[u32] {
        let need = self.header.vertex_count.max(0) as usize * self.anim_frames.max(1) as usize;
        if self.header.verts.len() >= need || self.header.verts.len() >= self.anim_verts.len() { &self.header.verts } else { &self.anim_verts }
    }

    /// The positions of one animation frame (in mesh units, before the mesh's scale and origin); empty when the frame is not there.
    pub fn frame_points(&self, frame: usize) -> Vec<[f32; 3]> {
        let count = self.header.vertex_count.max(0) as usize;
        self.frame_data().get(frame * count..(frame + 1) * count).map(|f| f.iter().map(|&v| unpack_vert(v)).collect()).unwrap_or_default()
    }

    /// The positions a sequence shows at `frame` (frames into it, fractions blending with the next, wrapping round), as the
    /// original's `UVertMeshInstance::GetFrame` does.
    pub fn sequence_points(&self, sequence: usize, frame: f32) -> Vec<[f32; 3]> {
        let Some(s) = self.sequences.get(sequence) else { return self.frame_points(0) };
        let n = s.num_frames.max(1) as usize;
        let f = frame.max(0.0);
        let whole = f.floor();
        let alpha = f - whole;
        let first = s.start_frame.max(0) as usize + whole as usize % n;
        let a = self.frame_points(first);
        if alpha <= 0.0 {
            return a;
        }
        let b = self.frame_points(s.start_frame.max(0) as usize + (whole as usize + 1) % n);
        a.iter().zip(&b).map(|(p, q)| [p[0] + (q[0] - p[0]) * alpha, p[1] + (q[1] - p[1]) * alpha, p[2] + (q[2] - p[2]) * alpha]).collect()
    }
}

pub fn read_vert_mesh(r: &mut Reader<'_>) -> Result<VertMesh> {
    let header = read_lod_header(r)?;
    let _ = r.array(|r| r.u32())?;
    let _ = r.i32()?;
    let _ = r.array(|r| r.u32())?;
    let sequences = r.array(crate::anim::read_sequence)?;
    let anim_verts = r.array(|r| r.u32())?;
    let frame_verts = r.i32()?;
    let anim_frames = r.i32()?;
    let frame_bounds = r.array(Bounds::read)?;
    let frame_spheres = r.array(Sphere::read)?;
    Ok(VertMesh { header, sequences, anim_verts, frame_verts, anim_frames, frame_bounds, frame_spheres })
}
