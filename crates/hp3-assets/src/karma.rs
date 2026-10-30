//! Rigid-body collision properties attached to static meshes (`KMeshProps`).

use hp3_package::{Reader, Result};

use crate::common::Vec3;

fn matrix(r: &mut Reader<'_>) -> Result<[f32; 16]> {
    let mut m = [0f32; 16];
    for v in &mut m {
        *v = r.f32()?;
    }
    Ok(m)
}

#[derive(Clone, Debug)]
pub struct SphereElem {
    pub transform: [f32; 16],
    pub radius: f32,
}

#[derive(Clone, Debug)]
pub struct BoxElem {
    pub transform: [f32; 16],
    pub size: Vec3,
}

#[derive(Clone, Debug)]
pub struct CylinderElem {
    pub transform: [f32; 16],
    pub radius: f32,
    pub height: f32,
}

#[derive(Clone, Debug)]
pub struct ConvexElem {
    pub transform: [f32; 16],
    pub vertices: Vec<Vec3>,
}

#[derive(Clone, Debug)]
pub struct MeshProps {
    pub inertia_tensor: [f32; 6],
    pub com_offset: Vec3,
    pub spheres: Vec<SphereElem>,
    pub boxes: Vec<BoxElem>,
    pub cylinders: Vec<CylinderElem>,
    pub convexes: Vec<ConvexElem>,
}

pub fn read_mesh_props(r: &mut Reader<'_>) -> Result<MeshProps> {
    let mut inertia_tensor = [0f32; 6];
    for v in &mut inertia_tensor {
        *v = r.f32()?;
    }
    Ok(MeshProps {
        inertia_tensor,
        com_offset: Vec3::read(r)?,
        spheres: r.array(|r| Ok(SphereElem { transform: matrix(r)?, radius: r.f32()? }))?,
        boxes: r.array(|r| Ok(BoxElem { transform: matrix(r)?, size: Vec3::read(r)? }))?,
        cylinders: r.array(|r| Ok(CylinderElem { transform: matrix(r)?, radius: r.f32()?, height: r.f32()? }))?,
        convexes: r.array(|r| Ok(ConvexElem { transform: matrix(r)?, vertices: r.array(Vec3::read)? }))?,
    })
}
