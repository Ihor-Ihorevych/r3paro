//! Brush polygons (`Polys`) and BSP models (`Model`).

use hp3_package::{ObjRef, Reader, Result};

use crate::common::{Bounds, PrimitiveBounds, Sphere, Vec3};

#[derive(Clone, Debug)]
pub struct Poly {
    pub base: Vec3,
    pub normal: Vec3,
    pub texture_u: Vec3,
    pub texture_v: Vec3,
    pub vertices: Vec<Vec3>,
    pub poly_flags: u32,
    pub actor: ObjRef,
    pub material: ObjRef,
    pub item_name: usize,
    pub link: i32,
    pub brush_poly: i32,
    pub light_map_scale: f32,
}

#[derive(Clone, Debug)]
pub struct Polys {
    pub polys: Vec<Poly>,
}

#[derive(Clone, Copy, Debug)]
pub struct Plane {
    pub normal: Vec3,
    pub w: f32,
}

impl Plane {
    pub fn read(r: &mut Reader<'_>) -> Result<Plane> {
        Ok(Plane { normal: Vec3::read(r)?, w: r.f32()? })
    }
}

#[derive(Clone, Debug)]
pub struct BspNode {
    pub plane: Plane,
    pub zone_mask: u64,
    pub node_flags: u8,
    pub vert_pool: i32,
    pub surf: i32,
    pub back: i32,
    pub front: i32,
    pub coplanar: i32,
    pub collision_bound: i32,
    pub render_bound: i32,
    pub exclusive_sphere: Sphere,
    pub unknown: [f32; 4],
    pub zone: [u8; 2],
    pub num_vertices: u8,
    pub leaf: [i32; 2],
    pub section: i32,
    pub first_vertex: i32,
    pub light_map: i32,
}

fn read_node(r: &mut Reader<'_>) -> Result<BspNode> {
    Ok(BspNode {
        plane: Plane::read(r)?,
        zone_mask: r.u64()?,
        node_flags: r.u8()?,
        vert_pool: r.ci()?,
        surf: r.ci()?,
        back: r.ci()?,
        front: r.ci()?,
        coplanar: r.ci()?,
        collision_bound: r.ci()?,
        render_bound: r.ci()?,
        exclusive_sphere: Sphere::read(r)?,
        unknown: [r.f32()?, r.f32()?, r.f32()?, r.f32()?],
        zone: [r.u8()?, r.u8()?],
        num_vertices: r.u8()?,
        leaf: [r.i32()?, r.i32()?],
        section: r.i32()?,
        first_vertex: r.i32()?,
        light_map: r.i32()?,
    })
}

#[derive(Clone, Debug)]
pub struct BspSurf {
    pub material: ObjRef,
    pub poly_flags: u32,
    pub base: i32,
    pub normal: i32,
    pub texture_u: i32,
    pub texture_v: i32,
    pub brush_poly: i32,
    pub actor: ObjRef,
    pub plane: Plane,
    pub light_map_scale: f32,
}

fn read_surf(r: &mut Reader<'_>) -> Result<BspSurf> {
    Ok(BspSurf {
        material: ObjRef(r.ci()?),
        poly_flags: r.u32()?,
        base: r.ci()?,
        normal: r.ci()?,
        texture_u: r.ci()?,
        texture_v: r.ci()?,
        brush_poly: r.ci()?,
        actor: ObjRef(r.ci()?),
        plane: Plane::read(r)?,
        light_map_scale: r.f32()?,
    })
}

#[derive(Clone, Copy, Debug)]
pub struct Vert {
    pub vertex: i32,
    pub side: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct Zone {
    pub actor: ObjRef,
    pub connectivity: u64,
    pub visibility: u64,
    pub unknown: f32,
}

#[derive(Clone, Debug)]
pub struct Model {
    pub bounds: PrimitiveBounds,
    pub vectors: Vec<Vec3>,
    pub points: Vec<Vec3>,
    pub nodes: Vec<BspNode>,
    pub surfs: Vec<BspSurf>,
    pub verts: Vec<Vert>,
    pub num_shared_sides: i32,
    pub zones: Vec<Zone>,
    pub polys: ObjRef,
    pub leaf_bounds: Vec<Bounds>,
    /// Per leaf: node indices (high bits are flags) ending in -1, then the hull's box as six floats.
    pub leaf_hulls: Vec<i32>,
    pub leaves: Vec<Leaf>,
    pub lights: Vec<ObjRef>,
    pub root_outside: i32,
    pub linked: i32,
    pub sections: Vec<BspSection>,
    pub light_maps: Vec<LightMap>,
    pub light_map_textures: Vec<LightMapTexture>,
}

/// Static lighting of one surface: its place in a lightmap texture and the shadow bitmap of each light.
#[derive(Clone, Debug)]
pub struct LightMap {
    pub flags: u8,
    pub surf: i32,
    pub unknown_index: i32,
    pub offset: [i32; 2],
    pub size: [i32; 2],
    pub world_to_light_map: [f32; 16],
    pub base: Vec3,
    pub axis_x: Vec3,
    pub axis_y: Vec3,
    pub bitmaps: Vec<LightBitmap>,
    pub trailer: (u8, i32),
}

/// One light's visibility over a lightmap, one bit per texel.
#[derive(Clone, Debug)]
pub struct LightBitmap {
    pub light: ObjRef,
    pub bits: Vec<u8>,
    pub size: [i32; 2],
    pub stride: i32,
    pub min: [i32; 2],
    pub max: [i32; 2],
}

/// A page the lightmaps are packed into; the texels themselves are built when the level loads.
#[derive(Clone, Debug)]
pub struct LightMapTexture {
    pub lead: u8,
    pub light_maps: Vec<i32>,
    pub cache_id: u64,
    pub unknown: i32,
    pub data_lens: [usize; 2],
    pub trailer: (u8, f32, f32, i32),
}

/// Render geometry for the nodes sharing one material and set of poly flags.
#[derive(Clone, Debug)]
pub struct BspSection {
    pub vertices: Vec<BspVertex>,
    pub num_nodes: i32,
    pub material: ObjRef,
    /// Equal to `num_nodes` in every section seen so far.
    pub num_nodes_again: i32,
    pub poly_flags: u32,
    pub light_map_texture: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct BspVertex {
    pub position: Vec3,
    pub uv: [f32; 2],
    pub light_map_uv: [f32; 2],
    pub normal: Vec3,
}

#[derive(Clone, Copy, Debug)]
pub struct Leaf {
    pub zone: i32,
    pub permeating: i32,
    pub volumetric: i32,
    pub visible_zones: u64,
}

pub fn read_model(r: &mut Reader<'_>) -> Result<Model> {
    let bounds = PrimitiveBounds::read(r)?;
    let vectors = r.array(Vec3::read)?;
    let points = r.array(Vec3::read)?;
    let nodes = r.array(read_node)?;
    let surfs = r.array(read_surf)?;
    let verts = r.array(|r| Ok(Vert { vertex: r.ci()?, side: r.ci()? }))?;
    let num_shared_sides = r.i32()?;
    let num_zones = r.i32()?;
    if !(0..=64).contains(&num_zones) {
        return Err(hp3_package::Error::Format(format!("bad zone count {num_zones}")));
    }
    let mut zones = Vec::with_capacity(num_zones as usize);
    for _ in 0..num_zones {
        zones.push(Zone { actor: ObjRef(r.ci()?), connectivity: r.u64()?, visibility: r.u64()?, unknown: r.f32()? });
    }
    let polys = ObjRef(r.ci()?);
    let leaf_bounds = r.array(Bounds::read)?;
    let leaf_hulls = r.array(|r| r.i32())?;
    let leaves = r.array(|r| {
        Ok(Leaf { zone: r.ci()?, permeating: r.ci()?, volumetric: r.ci()?, visible_zones: r.u64()? })
    })?;
    let lights = r.array(|r| Ok(ObjRef(r.ci()?)))?;
    let root_outside = r.i32()?;
    let linked = r.i32()?;
    let sections = r.array(|r| {
        Ok(BspSection {
            vertices: r.array(|r| {
                Ok(BspVertex {
                    position: Vec3::read(r)?,
                    uv: [r.f32()?, r.f32()?],
                    light_map_uv: [r.f32()?, r.f32()?],
                    normal: Vec3::read(r)?,
                })
            })?,
            num_nodes: r.i32()?,
            material: ObjRef(r.ci()?),
            num_nodes_again: r.i32()?,
            poly_flags: r.u32()?,
            light_map_texture: r.i32()?,
        })
    })?;
    let light_maps = r.array(|r| {
        let flags = r.u8()?;
        let surf = r.ci()?;
        let unknown_index = r.ci()?;
        let offset = [r.ci()?, r.ci()?];
        let size = [r.ci()?, r.ci()?];
        let mut world_to_light_map = [0f32; 16];
        for v in &mut world_to_light_map {
            *v = r.f32()?;
        }
        let base = Vec3::read(r)?;
        let axis_x = Vec3::read(r)?;
        let axis_y = Vec3::read(r)?;
        let bitmaps = r.array(|r| {
            Ok(LightBitmap {
                light: ObjRef(r.ci()?),
                bits: r.array(|r| r.u8())?,
                size: [r.i32()?, r.i32()?],
                stride: r.i32()?,
                min: [r.i32()?, r.i32()?],
                max: [r.i32()?, r.i32()?],
            })
        })?;
        Ok(LightMap {
            flags,
            surf,
            unknown_index,
            offset,
            size,
            world_to_light_map,
            base,
            axis_x,
            axis_y,
            bitmaps,
            trailer: (r.u8()?, r.i32()?),
        })
    })?;
    let light_map_textures = r.array(|r| {
        let lead = r.u8()?;
        let light_maps = r.array(|r| r.i32())?;
        let cache_id = r.u64()?;
        let unknown = r.i32()?;
        let data_a = r.lazy_bytes()?.len();
        let data_b = r.lazy_bytes()?.len();
        Ok(LightMapTexture {
            lead,
            light_maps,
            cache_id,
            unknown,
            data_lens: [data_a, data_b],
            trailer: (r.u8()?, r.f32()?, r.f32()?, r.i32()?),
        })
    })?;
    Ok(Model {
        light_maps,
        light_map_textures,
        sections,
        bounds,
        vectors,
        points,
        nodes,
        surfs,
        verts,
        num_shared_sides,
        zones,
        polys,
        leaf_bounds,
        leaf_hulls,
        leaves,
        lights,
        root_outside,
        linked,
    })
}

pub fn read_polys(r: &mut Reader<'_>) -> Result<Polys> {
    let num = r.i32()?;
    let _max = r.i32()?;
    if !(0..=1 << 20).contains(&num) {
        return Err(hp3_package::Error::Format(format!("bad poly count {num}")));
    }
    let mut polys = Vec::with_capacity(num as usize);
    for _ in 0..num {
        let count = r.ci()?;
        if !(0..=64).contains(&count) {
            return Err(hp3_package::Error::Format(format!("bad poly vertex count {count}")));
        }
        let base = Vec3::read(r)?;
        let normal = Vec3::read(r)?;
        let texture_u = Vec3::read(r)?;
        let texture_v = Vec3::read(r)?;
        let mut vertices = Vec::with_capacity(count as usize);
        for _ in 0..count {
            vertices.push(Vec3::read(r)?);
        }
        polys.push(Poly {
            base,
            normal,
            texture_u,
            texture_v,
            vertices,
            poly_flags: r.u32()?,
            actor: ObjRef(r.ci()?),
            material: ObjRef(r.ci()?),
            item_name: r.ci()? as usize,
            link: r.ci()?,
            brush_poly: r.ci()?,
            light_map_scale: r.f32()?,
        });
    }
    Ok(Polys { polys })
}
