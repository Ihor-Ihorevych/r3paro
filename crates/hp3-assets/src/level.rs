//! Per-level data: static mesh lighting instances, terrain and the level object.

use hp3_package::{ObjRef, Reader, Result};

use crate::common::{Bounds, Color, Vec3};

/// Baked lighting of one placed static mesh.
#[derive(Clone, Debug)]
pub struct StaticMeshInstance {
    pub colors: Vec<Color>,
    pub unknown: i32,
    pub lights: Vec<StaticMeshLight>,
}

#[derive(Clone, Debug)]
pub struct StaticMeshLight {
    pub light: ObjRef,
    /// One bit per vertex: whether the light reaches it.
    pub visibility: Vec<u8>,
    pub apply: i32,
}

#[derive(Clone, Debug, Default)]
pub struct Url {
    pub protocol: String,
    pub host: String,
    pub map: String,
    pub portal: String,
    pub options: Vec<String>,
    pub port: i32,
    pub valid: i32,
}

impl Url {
    pub fn read(r: &mut Reader<'_>) -> Result<Url> {
        Ok(Url {
            protocol: r.fstring()?,
            host: r.fstring()?,
            map: r.fstring()?,
            portal: r.fstring()?,
            options: r.array(|r| r.fstring())?,
            port: r.i32()?,
            valid: r.i32()?,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Level {
    pub actors: Vec<ObjRef>,
    pub url: Url,
    pub model: ObjRef,
    pub time_seconds: f32,
    pub first_deleted: ObjRef,
    pub text_blocks: [ObjRef; 16],
    pub travel_info: Vec<(String, String)>,
}

pub fn read_level(r: &mut Reader<'_>) -> Result<Level> {
    let num = r.i32()?;
    let _max = r.i32()?;
    if !(0..=1 << 20).contains(&num) {
        return Err(hp3_package::Error::Format(format!("bad actor count {num}")));
    }
    let mut actors = Vec::with_capacity(num as usize);
    for _ in 0..num {
        actors.push(ObjRef(r.ci()?));
    }
    let url = Url::read(r)?;
    let model = ObjRef(r.ci()?);
    let time_seconds = r.f32()?;
    let first_deleted = ObjRef(r.ci()?);
    let mut text_blocks = [ObjRef::NONE; 16];
    for t in &mut text_blocks {
        *t = ObjRef(r.ci()?);
    }
    let travel_info = r.array(|r| Ok((r.fstring()?, r.fstring()?)))?;
    Ok(Level { actors, url, model, time_seconds, first_deleted, text_blocks, travel_info })
}

#[derive(Clone, Debug)]
pub struct TerrainSector {
    pub info: ObjRef,
    pub quads: [i32; 2],
    pub offset: [i32; 2],
    pub bounds: Bounds,
    pub lights: Vec<TerrainSectorLight>,
}

#[derive(Clone, Debug)]
pub struct TerrainSectorLight {
    pub light: ObjRef,
    /// One bit per vertex of the sector.
    pub visibility: Vec<u8>,
}

pub fn read_terrain_sector(r: &mut Reader<'_>) -> Result<TerrainSector> {
    Ok(TerrainSector {
        info: ObjRef(r.ci()?),
        quads: [r.i32()?, r.i32()?],
        offset: [r.i32()?, r.i32()?],
        bounds: Bounds::read(r)?,
        lights: r.array(|r| Ok(TerrainSectorLight { light: ObjRef(r.ci()?), visibility: r.array(|r| r.u8())? }))?,
    })
}

/// Origin and three axes.
#[derive(Clone, Copy, Debug, Default)]
pub struct Coords {
    pub origin: Vec3,
    pub x: Vec3,
    pub y: Vec3,
    pub z: Vec3,
}

impl Coords {
    pub fn read(r: &mut Reader<'_>) -> Result<Coords> {
        Ok(Coords { origin: Vec3::read(r)?, x: Vec3::read(r)?, y: Vec3::read(r)?, z: Vec3::read(r)? })
    }
}

/// Native data of a terrain actor, after its tagged properties.
#[derive(Clone, Debug)]
pub struct Terrain {
    pub sectors: Vec<ObjRef>,
    pub vertices: Vec<Vec3>,
    pub sectors_x: i32,
    pub sectors_y: i32,
    /// Two triangle normals per quad.
    pub face_normals: Vec<[Vec3; 2]>,
    pub to_world: Coords,
    pub to_heightmap: Coords,
    pub heightmap_x: i32,
    pub heightmap_y: i32,
    pub vertex_colors: Vec<Color>,
}

pub fn read_terrain(r: &mut Reader<'_>) -> Result<Terrain> {
    Ok(Terrain {
        sectors: r.array(|r| Ok(ObjRef(r.ci()?)))?,
        vertices: r.array(Vec3::read)?,
        sectors_x: r.i32()?,
        sectors_y: r.i32()?,
        face_normals: r.array(|r| Ok([Vec3::read(r)?, Vec3::read(r)?]))?,
        to_world: Coords::read(r)?,
        to_heightmap: Coords::read(r)?,
        heightmap_x: r.i32()?,
        heightmap_y: r.i32()?,
        vertex_colors: r.array(Color::read)?,
    })
}

pub fn read_static_mesh_instance(r: &mut Reader<'_>) -> Result<StaticMeshInstance> {
    let colors = r.array(Color::read)?;
    let unknown = r.i32()?;
    let lights = r.array(|r| {
        Ok(StaticMeshLight { light: ObjRef(r.ci()?), visibility: r.array(|r| r.u8())?, apply: r.i32()? })
    })?;
    Ok(StaticMeshInstance { colors, unknown, lights })
}
