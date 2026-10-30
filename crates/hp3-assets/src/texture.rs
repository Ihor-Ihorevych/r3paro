//! Palettes and textures (including cubemaps, whose faces are separate textures).

use hp3_package::{Reader, Result};

use crate::common::Color;

#[derive(Clone, Debug)]
pub struct Palette {
    pub colors: Vec<Color>,
}

pub fn read_palette(r: &mut Reader<'_>) -> Result<Palette> {
    Ok(Palette { colors: r.array(Color::read)? })
}

#[derive(Clone, Debug)]
pub struct Mip {
    /// File offset and length of the pixel data inside the package.
    pub data_offset: usize,
    pub data_len: usize,
    pub width: i32,
    pub height: i32,
    pub width_bits: u8,
    pub height_bits: u8,
}

#[derive(Clone, Debug)]
pub struct Texture {
    pub mips: Vec<Mip>,
}

/// Procedural textures (fire, water, ice): an ordinary mip chain plus, for fire, its spark sources.
#[derive(Clone, Debug)]
pub struct Procedural {
    pub texture: Texture,
    /// Fire sparks: type, heat, x, y and four type-specific bytes.
    pub sparks: Vec<[u8; 8]>,
}

pub fn read_procedural(class: &str, r: &mut Reader<'_>) -> Result<Procedural> {
    let texture = read_texture(r)?;
    let sparks = if class == "FireTexture" {
        r.array(|r| Ok(r.bytes(8)?.try_into().unwrap()))?
    } else {
        Vec::new()
    };
    Ok(Procedural { texture, sparks })
}

pub fn read_texture(r: &mut Reader<'_>) -> Result<Texture> {
    let mips = r.array(|r| {
        let data = r.lazy_bytes()?;
        let data_len = data.len();
        let data_offset = r.file_pos() - data_len;
        Ok(Mip {
            data_offset,
            data_len,
            width: r.i32()?,
            height: r.i32()?,
            width_bits: r.u8()?,
            height_bits: r.u8()?,
        })
    })?;
    Ok(Texture { mips })
}
