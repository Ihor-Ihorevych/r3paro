//! Native data of HP3's asset classes, decoded from the bytes that follow an export's tagged properties.
//!
//! `read_native` dispatches on the native class `hp3_package::object` picked for the export.

pub mod anim;
pub mod common;
pub mod font;
pub mod karma;
pub mod level;
pub mod loader;
pub mod lod_mesh;
pub mod model;
pub mod skeletal;
pub mod sound;
pub mod static_mesh;
pub mod texture;

use hp3_package::{Package, Reader, Result};

pub use common::{Bounds, Sphere, Vec3};

#[derive(Clone, Debug)]
pub enum Asset {
    Palette(texture::Palette),
    Texture(texture::Texture),
    Sound(sound::Sound),
    StaticMesh(Box<static_mesh::StaticMesh>),
    Polys(model::Polys),
    Model(Box<model::Model>),
    StaticMeshInstance(level::StaticMeshInstance),
    Level(Box<level::Level>),
    TerrainSector(level::TerrainSector),
    Terrain(Box<level::Terrain>),
    ConvexVolume(model::ConvexVolume),
    MeshProps(karma::MeshProps),
    Procedural(texture::Procedural),
    Font(font::Font),
    MeshAnimation(Box<anim::MeshAnimation>),
    SkeletalMesh(Box<skeletal::SkeletalMesh>),
    VertMesh(Box<lod_mesh::VertMesh>),
}

/// Decodes the native data of `class` from `r`. `None` when the class's layout is not known yet.
pub fn read_native(class: &str, pkg: &Package, r: &mut Reader<'_>) -> Option<Result<Asset>> {
    Some(match class {
        "Palette" => texture::read_palette(r).map(Asset::Palette),
        "Texture" | "Cubemap" | "MovieTexture" => texture::read_texture(r).map(Asset::Texture),
        "FireTexture" | "WetTexture" | "IceTexture" | "WaterTexture" | "WaveTexture" => {
            texture::read_procedural(class, r).map(Asset::Procedural)
        }
        "Sound" => sound::read_sound(pkg, r).map(Asset::Sound),
        "StaticMesh" => static_mesh::read_static_mesh(r).map(|m| Asset::StaticMesh(Box::new(m))),
        "Polys" => model::read_polys(r).map(Asset::Polys),
        "Model" => model::read_model(r).map(|m| Asset::Model(Box::new(m))),
        "StaticMeshInstance" => level::read_static_mesh_instance(r).map(Asset::StaticMeshInstance),
        "Level" => level::read_level(r).map(|l| Asset::Level(Box::new(l))),
        "TerrainSector" => level::read_terrain_sector(r).map(Asset::TerrainSector),
        "VertMesh" => lod_mesh::read_vert_mesh(r).map(|m| Asset::VertMesh(Box::new(m))),
        "SkeletalMesh" => skeletal::read_skeletal_mesh(r).map(|m| Asset::SkeletalMesh(Box::new(m))),
        "MeshAnimation" => anim::read_mesh_animation(r).map(|a| Asset::MeshAnimation(Box::new(a))),
        "Font" => font::read_font(pkg.summary.version, r).map(Asset::Font),
        "KMeshProps" => karma::read_mesh_props(r).map(Asset::MeshProps),
        "ConvexVolume" => model::read_convex_volume(r).map(Asset::ConvexVolume),
        "TerrainInfo" => level::read_terrain(r).map(|t| Asset::Terrain(Box::new(t))),
        _ => return None,
    })
}
