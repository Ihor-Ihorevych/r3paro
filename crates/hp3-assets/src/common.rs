//! Small value types shared by the asset formats.

use hp3_package::{Reader, Result};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub fn read(r: &mut Reader<'_>) -> Result<Vec3> {
        Ok(Vec3 { x: r.f32()?, y: r.f32()?, z: r.f32()? })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bounds {
    pub min: Vec3,
    pub max: Vec3,
    pub valid: bool,
}

impl Bounds {
    pub fn read(r: &mut Reader<'_>) -> Result<Bounds> {
        Ok(Bounds { min: Vec3::read(r)?, max: Vec3::read(r)?, valid: r.u8()? != 0 })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sphere {
    pub center: Vec3,
    pub radius: f32,
}

impl Sphere {
    pub fn read(r: &mut Reader<'_>) -> Result<Sphere> {
        Ok(Sphere { center: Vec3::read(r)?, radius: r.f32()? })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Color {
    pub b: u8,
    pub g: u8,
    pub r: u8,
    pub a: u8,
}

impl Color {
    pub fn read(r: &mut Reader<'_>) -> Result<Color> {
        let v = r.bytes(4)?;
        Ok(Color { b: v[0], g: v[1], r: v[2], a: v[3] })
    }
}

/// The bounding box and sphere every primitive (meshes, models) starts with.
#[derive(Clone, Copy, Debug, Default)]
pub struct PrimitiveBounds {
    pub bounding_box: Bounds,
    pub bounding_sphere: Sphere,
}

impl PrimitiveBounds {
    pub fn read(r: &mut Reader<'_>) -> Result<PrimitiveBounds> {
        Ok(PrimitiveBounds { bounding_box: Bounds::read(r)?, bounding_sphere: Sphere::read(r)? })
    }
}
