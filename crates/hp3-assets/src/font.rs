//! Bitmap fonts: glyph rectangles on one or more texture pages.

use hp3_package::{ObjRef, Reader, Result};

#[derive(Clone, Copy, Debug)]
pub struct Glyph {
    pub u: i32,
    pub v: i32,
    pub width: i32,
    pub height: i32,
    pub texture: u8,
}

#[derive(Clone, Debug)]
pub struct Font {
    pub glyphs: Vec<Glyph>,
    pub textures: Vec<ObjRef>,
    pub kerning: i32,
    /// Character code to glyph index, for fonts with more than 256 characters.
    pub remap: Vec<(u16, u16)>,
    pub is_remapped: i32,
}

/// Package versions before this store fonts as pages, each with its texture and its glyphs.
const GLYPHS_DISPLACED_VERSION: u16 = 122;
/// First package version that stores a kerning value.
const KERNING_VERSION: u16 = 119;

pub fn read_font(version: u16, r: &mut Reader<'_>) -> Result<Font> {
    let (glyphs, textures) = if version < GLYPHS_DISPLACED_VERSION {
        let mut glyphs = Vec::new();
        let mut textures = Vec::new();
        let pages = r.count()?;
        for page in 0..pages {
            textures.push(ObjRef(r.ci()?));
            let n = r.count()?;
            for _ in 0..n {
                glyphs.push(Glyph {
                    u: r.i32()?,
                    v: r.i32()?,
                    width: r.i32()?,
                    height: r.i32()?,
                    texture: page as u8,
                });
            }
        }
        let _per_page = r.i32()?;
        (glyphs, textures)
    } else {
        let glyphs = r.array(|r| {
            Ok(Glyph { u: r.i32()?, v: r.i32()?, width: r.i32()?, height: r.i32()?, texture: r.u8()? })
        })?;
        (glyphs, r.array(|r| Ok(ObjRef(r.ci()?)))?)
    };
    Ok(Font {
        glyphs,
        textures,
        kerning: if version >= KERNING_VERSION { r.i32()? } else { 0 },
        remap: r.array(|r| Ok((r.u16()?, r.u16()?)))?,
        is_remapped: r.i32()?,
    })
}
