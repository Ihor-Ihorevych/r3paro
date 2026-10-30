//! Skeletal animation sets (`MeshAnimation`): bones, per-sequence key tracks and sequence metadata.

use hp3_package::{ObjRef, Reader, Result};

use crate::common::Vec3;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Quat {
    pub fn read(r: &mut Reader<'_>) -> Result<Quat> {
        Ok(Quat { x: r.f32()?, y: r.f32()?, z: r.f32()?, w: r.f32()? })
    }
}

#[derive(Clone, Debug)]
pub struct NamedBone {
    pub name: usize,
    pub flags: u32,
    pub parent: i32,
}

/// Keys of one bone: rotations, positions and their times (each array may have one key for "constant").
#[derive(Clone, Debug, Default)]
pub struct Track {
    pub flags: u32,
    pub rotations: Vec<Quat>,
    pub positions: Vec<Vec3>,
    pub times: Vec<f32>,
}

fn read_track(r: &mut Reader<'_>) -> Result<Track> {
    Ok(Track {
        flags: r.u32()?,
        rotations: r.array(Quat::read)?,
        positions: r.array(Vec3::read)?,
        times: r.array(|r| r.f32())?,
    })
}

/// The keys of one sequence for every bone.
#[derive(Clone, Debug)]
pub struct Motion {
    pub root_speed: Vec3,
    pub track_time: f32,
    pub start_bone: i32,
    pub flags: u32,
    pub bone_indices: Vec<i32>,
    pub tracks: Vec<Track>,
    pub root_track: Track,
    /// Empty in every animation seen so far.
    pub unknown: Vec<i32>,
}

#[derive(Clone, Debug)]
pub struct Notify {
    pub time: f32,
    pub function: usize,
    pub object: ObjRef,
}

#[derive(Clone, Debug)]
pub struct Sequence {
    pub unknown: f32,
    pub name: usize,
    pub groups: Vec<usize>,
    pub start_frame: i32,
    pub num_frames: i32,
    pub notifies: Vec<Notify>,
    pub rate: f32,
}

#[derive(Clone, Debug)]
pub struct MeshAnimation {
    pub internal_version: i32,
    pub bones: Vec<NamedBone>,
    pub motions: Vec<Motion>,
    pub sequences: Vec<Sequence>,
}

pub fn read_sequence(r: &mut Reader<'_>) -> Result<Sequence> {
    Ok(Sequence {
        unknown: r.f32()?,
        name: r.ci()? as usize,
        groups: r.array(|r| Ok(r.ci()? as usize))?,
        start_frame: r.i32()?,
        num_frames: r.i32()?,
        notifies: r.array(|r| Ok(Notify { time: r.f32()?, function: r.ci()? as usize, object: ObjRef(r.ci()?) }))?,
        rate: r.f32()?,
    })
}

pub fn read_mesh_animation(r: &mut Reader<'_>) -> Result<MeshAnimation> {
    let internal_version = r.i32()?;
    let bones = r.array(|r| Ok(NamedBone { name: r.ci()? as usize, flags: r.u32()?, parent: r.i32()? }))?;
    let motions = r.array(|r| {
        Ok(Motion {
            root_speed: Vec3::read(r)?,
            track_time: r.f32()?,
            start_bone: r.i32()?,
            flags: r.u32()?,
            bone_indices: r.array(|r| r.i32())?,
            tracks: r.array(read_track)?,
            root_track: read_track(r)?,
            unknown: r.array(|r| r.i32())?,
        })
    })?;
    let sequences = r.array(read_sequence)?;
    Ok(MeshAnimation { internal_version, bones, motions, sequences })
}
