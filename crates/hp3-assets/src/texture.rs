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

/// Pixel formats, as stored in a texture's `Format` property.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    P8,
    Rgba7,
    Rgb16,
    Dxt1,
    Rgb8,
    Rgba8,
    NoData,
    Dxt3,
    Dxt5,
    L8,
    G16,
    Rrrgggbbb,
}

impl Format {
    pub fn from_u8(v: u8) -> Option<Format> {
        use Format::*;
        Some(match v {
            0 => P8,
            1 => Rgba7,
            2 => Rgb16,
            3 => Dxt1,
            4 => Rgb8,
            5 => Rgba8,
            6 => NoData,
            7 => Dxt3,
            8 => Dxt5,
            9 => L8,
            10 => G16,
            11 => Rrrgggbbb,
            _ => return None,
        })
    }

    /// Bytes a mip of this size takes, or `None` for formats without a fixed size.
    pub fn data_len(self, width: usize, height: usize) -> Option<usize> {
        let blocks = width.div_ceil(4) * height.div_ceil(4);
        Some(match self {
            Format::P8 | Format::L8 => width * height,
            Format::Rgb16 | Format::G16 => width * height * 2,
            Format::Rgb8 => width * height * 3,
            Format::Rgba8 | Format::Rgba7 => width * height * 4,
            Format::Dxt1 => blocks * 8,
            Format::Dxt3 | Format::Dxt5 => blocks * 16,
            Format::NoData | Format::Rrrgggbbb => return None,
        })
    }
}

fn rgb565(c: u16) -> [u8; 3] {
    let r = ((c >> 11) & 0x1F) as u32;
    let g = ((c >> 5) & 0x3F) as u32;
    let b = (c & 0x1F) as u32;
    [((r * 255 + 15) / 31) as u8, ((g * 255 + 31) / 63) as u8, ((b * 255 + 15) / 31) as u8]
}

/// Four colors of a DXT color block; for DXT1, `c0 <= c1` selects three colors and transparent black.
fn dxt_palette(block: &[u8], dxt1: bool) -> [[u8; 4]; 4] {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let a = rgb565(c0);
    let b = rgb565(c1);
    let mix = |wa: u32, wb: u32, d: u32| -> [u8; 4] {
        [
            ((a[0] as u32 * wa + b[0] as u32 * wb) / d) as u8,
            ((a[1] as u32 * wa + b[1] as u32 * wb) / d) as u8,
            ((a[2] as u32 * wa + b[2] as u32 * wb) / d) as u8,
            255,
        ]
    };
    if c0 > c1 || !dxt1 {
        [[a[0], a[1], a[2], 255], [b[0], b[1], b[2], 255], mix(2, 1, 3), mix(1, 2, 3)]
    } else {
        [[a[0], a[1], a[2], 255], [b[0], b[1], b[2], 255], mix(1, 1, 2), [0, 0, 0, 0]]
    }
}

/// Decodes one mip to RGBA8. `palette` is needed for `P8`.
pub fn decode_rgba(format: Format, width: usize, height: usize, data: &[u8], palette: Option<&Palette>) -> Option<Vec<u8>> {
    let need = format.data_len(width, height)?;
    if data.len() < need {
        return None;
    }
    let mut out = vec![0u8; width * height * 4];
    match format {
        Format::Rgba8 | Format::Rgba7 => {
            for (o, i) in out.chunks_exact_mut(4).zip(data.chunks_exact(4)) {
                o.copy_from_slice(&[i[2], i[1], i[0], i[3]]);
            }
        }
        Format::Rgb8 => {
            for (o, i) in out.chunks_exact_mut(4).zip(data.chunks_exact(3)) {
                o.copy_from_slice(&[i[2], i[1], i[0], 255]);
            }
        }
        Format::L8 => {
            for (o, &l) in out.chunks_exact_mut(4).zip(data) {
                o.copy_from_slice(&[l, l, l, 255]);
            }
        }
        Format::G16 => {
            for (o, i) in out.chunks_exact_mut(4).zip(data.chunks_exact(2)) {
                let l = i[1];
                o.copy_from_slice(&[l, l, l, 255]);
            }
        }
        Format::Rgb16 => {
            for (o, i) in out.chunks_exact_mut(4).zip(data.chunks_exact(2)) {
                let c = rgb565(u16::from_le_bytes([i[0], i[1]]));
                o.copy_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        Format::P8 => {
            let pal = palette?;
            for (o, &p) in out.chunks_exact_mut(4).zip(data) {
                let c = pal.colors.get(p as usize).copied().unwrap_or_default();
                o.copy_from_slice(&[c.r, c.g, c.b, c.a]);
            }
        }
        Format::Dxt1 | Format::Dxt3 | Format::Dxt5 => {
            let block_size = if format == Format::Dxt1 { 8 } else { 16 };
            let bw = width.div_ceil(4);
            for (bi, block) in data[..need].chunks_exact(block_size).enumerate() {
                let (bx, by) = (bi % bw * 4, bi / bw * 4);
                let (alpha, color) = if format == Format::Dxt1 { (None, block) } else { (Some(&block[..8]), &block[8..]) };
                let pal = dxt_palette(color, format == Format::Dxt1);
                let bits = u32::from_le_bytes([color[4], color[5], color[6], color[7]]);
                let alphas: Option<[u8; 16]> = alpha.map(|a| {
                    let mut v = [255u8; 16];
                    if format == Format::Dxt3 {
                        for (i, x) in v.iter_mut().enumerate() {
                            let nib = (a[i / 2] >> ((i % 2) * 4)) & 0xF;
                            *x = nib * 17;
                        }
                    } else {
                        let (a0, a1) = (a[0] as u32, a[1] as u32);
                        let table: [u32; 8] = if a0 > a1 {
                            [a0, a1, (6 * a0 + a1) / 7, (5 * a0 + 2 * a1) / 7, (4 * a0 + 3 * a1) / 7, (3 * a0 + 4 * a1) / 7, (2 * a0 + 5 * a1) / 7, (a0 + 6 * a1) / 7]
                        } else {
                            [a0, a1, (4 * a0 + a1) / 5, (3 * a0 + 2 * a1) / 5, (2 * a0 + 3 * a1) / 5, (a0 + 4 * a1) / 5, 0, 255]
                        };
                        let idx = u64::from_le_bytes([a[2], a[3], a[4], a[5], a[6], a[7], 0, 0]);
                        for (i, x) in v.iter_mut().enumerate() {
                            *x = table[((idx >> (3 * i)) & 7) as usize] as u8;
                        }
                    }
                    v
                });
                for i in 0..16 {
                    let (x, y) = (bx + i % 4, by + i / 4);
                    if x >= width || y >= height {
                        continue;
                    }
                    let mut c = pal[((bits >> (2 * i)) & 3) as usize];
                    if let Some(a) = alphas {
                        c[3] = a[i];
                    }
                    let o = (y * width + x) * 4;
                    out[o..o + 4].copy_from_slice(&c);
                }
            }
        }
        Format::NoData | Format::Rrrgggbbb => return None,
    }
    Some(out)
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
