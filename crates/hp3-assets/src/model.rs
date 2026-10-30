//! Brush polygons (`Polys`) and BSP models (`Model`).

use hp3_package::{ObjRef, Reader, Result};

use crate::common::Vec3;

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
