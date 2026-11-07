//! Cached access to decoded objects across an install: actors' properties, textures as RGBA, the base texture
//! behind a material.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use hp3_package::object::{self, Body};
use hp3_package::props::{self, Tag, TagType};
use hp3_package::{ObjRef, Package, PackageSet, Reader};

use crate::common::Vec3;
use crate::texture::{self, Format};
use crate::{Asset, read_native};

/// A steady scroll of a material's picture, in texture repeats per second.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pan {
    pub u: f32,
    pub v: f32,
}

/// Decoded RGBA8 image.
#[derive(Clone, Debug)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Pixels with alpha below one half are cut out (masked) rather than blended.
    pub masked: bool,
    /// The texture is blended by its alpha (`bAlphaTexture`): drawn after opaque surfaces.
    pub translucent: bool,
}

/// How a material uses its texture's alpha channel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MaterialBlend {
    #[default]
    Opaque,
    /// Pixels with alpha below one half are cut out.
    Cutout,
    /// Blended by the texture's alpha.
    Alpha,
    /// Added to what is behind, by the texture's alpha.
    Additive,
}

/// A global identity for an export: package name (lower case) and export index.
pub type ObjKey = (String, usize);

pub fn key(pkg: &Package, index: usize) -> ObjKey {
    (pkg.name.to_ascii_lowercase(), index)
}

pub struct Loader<'s> {
    pub set: &'s PackageSet,
    images: RefCell<HashMap<ObjKey, Option<Rc<Image>>>>,
    raw_images: RefCell<HashMap<ObjKey, Option<Rc<Image>>>>,
    assets: RefCell<HashMap<ObjKey, Option<Rc<Asset>>>>,
}

/// Unreal rotation units: 65536 per turn.
pub fn rot_to_rad(v: i32) -> f32 {
    v as f32 * std::f32::consts::TAU / 65536.0
}

pub fn tag_vec3(tag: &Tag<'_>) -> Option<Vec3> {
    let mut r = Reader::new(tag.data);
    Vec3::read(&mut r).ok()
}

/// Pitch, yaw, roll.
pub fn tag_rotator(tag: &Tag<'_>) -> Option<[i32; 3]> {
    let mut r = Reader::new(tag.data);
    Some([r.i32().ok()?, r.i32().ok()?, r.i32().ok()?])
}

impl<'s> Loader<'s> {
    pub fn new(set: &'s PackageSet) -> Self {
        Loader { set, images: RefCell::new(HashMap::new()), raw_images: RefCell::new(HashMap::new()), assets: RefCell::new(HashMap::new()) }
    }

    /// The native asset decoded from an export, if its class has one.
    pub fn asset(&self, pkg: &Package, index: usize) -> Option<Rc<Asset>> {
        let k = key(pkg, index);
        if let Some(a) = self.assets.borrow().get(&k) {
            return a.clone();
        }
        let chain = self.set.export_chain(pkg, index);
        let a = object::read_export(pkg, index, &chain).ok().and_then(|o| {
            if !matches!(o.body, Body::Unparsed) {
                return None;
            }
            let base = pkg.exports[index].serial_offset + pkg.exports[index].serial_size - o.tail.len();
            let mut r = Reader::with_base(o.tail, base);
            read_native(o.native_class, pkg, &mut r)?.ok().map(Rc::new)
        });
        self.assets.borrow_mut().insert(k, a.clone());
        a
    }

    /// Calls `f` with an export's tagged properties and its class chain.
    pub fn with_tags<T>(&self, pkg: &Package, index: usize, f: impl FnOnce(&[Tag<'_>], &[String]) -> T) -> Option<T> {
        let chain = self.set.export_chain(pkg, index);
        let o = object::read_export(pkg, index, &chain).ok()?;
        Some(f(&o.tags, &chain))
    }

    /// RGBA pixels of a texture's first mip.
    pub fn image(&self, pkg: &Rc<Package>, index: usize) -> Option<Rc<Image>> {
        let k = key(pkg, index);
        if let Some(i) = self.images.borrow().get(&k) {
            return i.clone();
        }
        let img = self.decode_image(pkg, index).map(Rc::new);
        self.images.borrow_mut().insert(k, img.clone());
        img
    }

    /// A `Shader` with an `Opacity` map is blended by that map (the Spongify rug: a spiral and a frame show, the rest is see-through):
    /// its diffuse picture with the opacity map as alpha (the map's own alpha when it has one, else its brightness), kept in the
    /// image cache under an index no export has. Returns where to find that image.
    pub fn shader_with_opacity(&self, pkg: &Rc<Package>, r: ObjRef) -> Option<(Rc<Package>, usize)> {
        let (mut p, mut i) = self.set.resolve(pkg, r)?;
        for _ in 0..16 {
            let chain = self.set.export_chain(&p, i);
            if chain.iter().any(|c| c.eq_ignore_ascii_case("Texture")) {
                return None;
            }
            let kind = chain.first().map(String::as_str).unwrap_or("");
            let (pair, next) = self
                .with_tags(&p, i, |tags, _| {
                    if kind.eq_ignore_ascii_case("Shader") {
                        let opacity = props::find(&p, tags, "Opacity", 0).and_then(|t| t.as_obj());
                        let diffuse = props::find(&p, tags, "Diffuse", 0).and_then(|t| t.as_obj());
                        if let (Some(o), Some(d)) = (opacity, diffuse) {
                            return (Some((o, d)), None);
                        }
                    }
                    let next = ["Diffuse", "Material"].iter().find_map(|n| props::find(&p, tags, n, 0).and_then(|t| t.as_obj()));
                    (None, next)
                })
                .unwrap_or((None, None));
            if let Some((o, d)) = pair {
                let (dp, di) = self.base_texture(&p, d)?;
                let (op, oi) = self.base_texture(&p, o)?;
                let synthetic = (1usize << 40) | (oi << 20) | di;
                if self.images.borrow().get(&key(&dp, synthetic)).is_none() {
                    let diffuse = self.decode_image_alpha(&dp, di, true)?;
                    let map = self.decode_image_alpha(&op, oi, true)?;
                    let has_alpha = map.rgba.chunks_exact(4).any(|px| px[3] != 255);
                    let mut rgba = diffuse.rgba.clone();
                    for y in 0..diffuse.height as usize {
                        for x in 0..diffuse.width as usize {
                            let (mx, my) = (x * map.width as usize / diffuse.width as usize, y * map.height as usize / diffuse.height as usize);
                            let m = &map.rgba[(my * map.width as usize + mx) * 4..][..4];
                            let a = if has_alpha { m[3] } else { ((m[0] as u32 * 299 + m[1] as u32 * 587 + m[2] as u32 * 114) / 1000) as u8 };
                            rgba[(y * diffuse.width as usize + x) * 4 + 3] = a;
                        }
                    }
                    let image = Image { width: diffuse.width, height: diffuse.height, rgba, masked: false, translucent: true };
                    self.images.borrow_mut().insert(key(&dp, synthetic), Some(Rc::new(image)));
                }
                return Some((dp, synthetic));
            }
            let (np, ni) = self.set.resolve(&p, next?)?;
            p = np;
            i = ni;
        }
        None
    }

    /// Whether a material lights itself: a `Combiner` (the Carpe Retractum orb: two blue fogs blended by a pulsing mask) glows
    /// by its own light, whatever the room's light is.
    pub fn is_self_lit(&self, pkg: &Rc<Package>, r: ObjRef) -> bool {
        let Some((p, i)) = self.set.resolve(pkg, r) else { return false };
        let chain = self.set.export_chain(&p, i);
        chain.first().is_some_and(|k| k.eq_ignore_ascii_case("Combiner"))
    }

    /// How the wrappers on the way from a material reference down to its texture use the texture's alpha channel:
    /// a `FinalBlend` with `AlphaTest` (Harry's hair) or a `Shader` with masked output cuts the surface out; a `Shader`
    /// with an `Opacity` map blends it (the salamanders' flame cards); translucent output adds it.
    pub fn material_blend(&self, pkg: &Rc<Package>, r: ObjRef) -> MaterialBlend {
        let Some((mut p, mut i)) = self.set.resolve(pkg, r) else { return MaterialBlend::Opaque };
        for _ in 0..16 {
            let chain = self.set.export_chain(&p, i);
            if chain.iter().any(|c| c.eq_ignore_ascii_case("Texture")) {
                return MaterialBlend::Opaque;
            }
            let kind = chain.first().map(String::as_str).unwrap_or("");
            let (found, next) = self
                .with_tags(&p, i, |tags, _| {
                    let found = if kind.eq_ignore_ascii_case("FinalBlend") {
                        props::find(&p, tags, "AlphaTest", 0).is_some_and(|t| t.bool_value).then_some(MaterialBlend::Cutout)
                    } else if kind.eq_ignore_ascii_case("Shader") {
                        let output = props::find(&p, tags, "OutputBlending", 0).and_then(|t| t.as_u8()).unwrap_or(0);
                        let opacity = props::find(&p, tags, "Opacity", 0).is_some();
                        match output {
                            1 => Some(MaterialBlend::Cutout),
                            3 | 5 => Some(MaterialBlend::Additive),
                            0 if opacity => Some(MaterialBlend::Alpha),
                            _ => None,
                        }
                    } else {
                        None
                    };
                    let next = ["Diffuse", "Material"].iter().find_map(|n| props::find(&p, tags, n, 0).and_then(|t| t.as_obj()));
                    (found, next)
                })
                .unwrap_or((None, None));
            if let Some(found) = found {
                return found;
            }
            let Some(next) = next else { return MaterialBlend::Opaque };
            let Some((np, ni)) = self.set.resolve(&p, next) else { return MaterialBlend::Opaque };
            p = np;
            i = ni;
        }
        MaterialBlend::Opaque
    }

    /// Like `image`, for a texture whose alpha channel is used (see `material_blend`): the alpha is kept.
    pub fn cutout_image(&self, pkg: &Rc<Package>, index: usize) -> Option<Rc<Image>> {
        let k = key(pkg, index);
        if let Some(Some(i)) = self.images.borrow().get(&k)
            && i.masked
        {
            return Some(i.clone());
        }
        let img = self.decode_image_alpha(pkg, index, true).map(|mut i| {
            i.masked = true;
            i.translucent = false;
            Rc::new(i)
        });
        self.images.borrow_mut().insert(k, img.clone());
        img
    }

    /// Like `image`, keeping the texture's own alpha channel (2D drawing: HUD icons, font pages).
    pub fn image_with_alpha(&self, pkg: &Rc<Package>, index: usize) -> Option<Rc<Image>> {
        let k = key(pkg, index);
        if let Some(i) = self.raw_images.borrow().get(&k) {
            return i.clone();
        }
        let img = self.decode_image_alpha(pkg, index, true).map(Rc::new);
        self.raw_images.borrow_mut().insert(k, img.clone());
        img
    }

    fn decode_image(&self, pkg: &Rc<Package>, index: usize) -> Option<Image> {
        self.decode_image_alpha(pkg, index, false)
    }

    fn decode_image_alpha(&self, pkg: &Rc<Package>, index: usize, keep_alpha: bool) -> Option<Image> {
        let (format, palette_ref, masked) = self.with_tags(pkg, index, |tags, _| {
            let format = props::find(pkg, tags, "Format", 0).and_then(|t| t.as_u8()).unwrap_or(0);
            let palette = props::find(pkg, tags, "Palette", 0).and_then(|t| t.as_obj());
            let masked = props::find(pkg, tags, "bMasked", 0).is_some_and(|t| t.bool_value);
            let translucent = props::find(pkg, tags, "bAlphaTexture", 0).is_some_and(|t| t.bool_value);
            (format, palette, (masked, translucent))
        })?;
        let format0 = format;
        let format = Format::from_u8(format);
        if std::env::var_os("HP3_PARTICLE_STYLE").is_some() && format.is_none() { eprintln!("tex {}#{index}: unknown format byte {format0}", pkg.name); }
        let format = format?;
        // A procedural texture (fire, water...) keeps the picture it was saved with, which stands in for it.
        let asset = self.asset(pkg, index);
        if std::env::var_os("HP3_PARTICLE_STYLE").is_some() && asset.is_none() { eprintln!("tex {}#{index}: asset unreadable", pkg.name); }
        let asset = asset?;
        let tex = match &*asset {
            Asset::Texture(t) => t,
            Asset::Procedural(p) => &p.texture,
            _ => return None,
        };
        let palette = match palette_ref.and_then(|p| self.set.resolve(pkg, p)) {
            Some((ppkg, pi)) => match self.asset(&ppkg, pi).as_deref() {
                Some(Asset::Palette(p)) => Some(p.clone()),
                _ => None,
            },
            None => None,
        };
        // A procedural texture can be saved with no picture at all (fire sparks): it is then blank, and gets the glow below.
        let is_procedural = matches!(&*asset, Asset::Procedural(_));
        let mip = tex.mips.first();
        let (w, h, data): (usize, usize, &[u8]) = match mip.and_then(|m| Some((m, pkg.data.get(m.data_offset..m.data_offset + m.data_len)?))) {
            Some((m, d)) => (m.width as usize, m.height as usize, d),
            None if is_procedural => (64, 64, &[]),
            None => return None,
        };
        let mut rgba = match texture::decode_rgba(format, w, h, data, palette.as_ref()) {
            Some(r) => r,
            None if is_procedural => vec![0u8; w * h * 4],
            None => return None,
        };
        // A procedural texture saved with an empty picture (a fire spark: the engine draws its sparks at run time)
        // is shown as a soft round glow in the colours of its palette, dark at the edge to bright at the middle.
        let blank_procedural = is_procedural && (data.is_empty() || (format == Format::P8 && data.iter().take(w * h).all(|&b| b == 0)));
        if blank_procedural
            && let Some(pal) = palette.as_ref().filter(|p| p.colors.len() >= 2)
        {
            for y in 0..h {
                for x in 0..w {
                    let (dx, dy) = ((x as f32 + 0.5) / w as f32 * 2.0 - 1.0, (y as f32 + 0.5) / h as f32 * 2.0 - 1.0);
                    let glow = (1.0 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0).powi(2);
                    let c = pal.colors[((glow * (pal.colors.len() - 1) as f32) as usize).min(pal.colors.len() - 1)];
                    let o = (y * w + x) * 4;
                    rgba[o..o + 4].copy_from_slice(&[c.r, c.g, c.b, (glow * 255.0) as u8]);
                }
            }
        }
        if std::env::var_os("HP3_PARTICLE_STYLE").is_some() && keep_alpha {
            let (lo, hi) = rgba.chunks_exact(4).fold((255u8, 0u8), |(l, h), p| (l.min(p[3]), h.max(p[3])));
            let corner = &rgba[..4];
            eprintln!("tex {}#{index}: {format:?} {w}x{h} masked {} alphatex {} alpha {lo}..{hi} corner {corner:?}", pkg.name, masked.0, masked.1);
        }
        let (masked, translucent) = masked;
        let masked = masked && !blank_procedural;
        let translucent = translucent || blank_procedural;
        let masked = !blank_procedural && (masked || (format == Format::P8 && palette.is_some_and(|p| p.colors.first().is_some_and(|c| c.a == 0))));
        // Masked palettised textures: index 0 is transparent whatever its color (often a key color).
        if masked && format == Format::P8 {
            for (i, &index) in data.iter().take(w * h).enumerate() {
                if index == 0 {
                    // Clear the key color too, so filtering does not bleed it into the edges.
                    rgba[i * 4..i * 4 + 4].copy_from_slice(&[0, 0, 0, 0]);
                }
            }
        }
        // Alpha is a cut-out only for masked textures; elsewhere it carries other data (specular, blending),
        // which the renderer does not use yet.
        if !masked && !translucent && !keep_alpha {
            for px in rgba.chunks_exact_mut(4) {
                px[3] = 255;
            }
        }
        Some(Image { width: w as u32, height: h as u32, rgba, masked, translucent: translucent && !masked })
    }

    /// A terrain density map as `(width, height, one weight per texel)`: the alpha channel of an RGBA texture, the
    /// palette's red of a palettised one, the value of a gray one. No texel is treated as transparent.
    pub fn density_map(&self, pkg: &Rc<Package>, index: usize) -> Option<(usize, usize, Vec<u8>)> {
        let (format, palette_ref) = self.with_tags(pkg, index, |tags, _| {
            (props::find(pkg, tags, "Format", 0).and_then(|t| t.as_u8()).unwrap_or(0), props::find(pkg, tags, "Palette", 0).and_then(|t| t.as_obj()))
        })?;
        let format = Format::from_u8(format)?;
        let Asset::Texture(tex) = &*self.asset(pkg, index)? else { return None };
        let mip = tex.mips.first()?;
        let data = pkg.data.get(mip.data_offset..mip.data_offset + mip.data_len)?;
        let (w, h) = (mip.width as usize, mip.height as usize);
        let weights = match format {
            Format::L8 => data.get(..w * h)?.to_vec(),
            Format::P8 => {
                let (ppkg, pi) = self.set.resolve(pkg, palette_ref?)?;
                let Asset::Palette(p) = &*self.asset(&ppkg, pi)? else { return None };
                data.get(..w * h)?.iter().map(|&i| p.colors.get(i as usize).map_or(0, |c| c.r)).collect()
            }
            _ => {
                let rgba = texture::decode_rgba(format, w, h, data, None)?;
                let has_alpha = matches!(format, Format::Rgba8 | Format::Dxt3 | Format::Dxt5);
                rgba.chunks_exact(4).map(|px| if has_alpha { px[3] } else { px[0] }).collect()
            }
        };
        Some((w, h, weights))
    }

    /// A terrain alpha map's weights, spread to all four channels: the texture's alpha channel when its format
    /// has one, its red channel otherwise.
    pub fn weight_image(&self, pkg: &Rc<Package>, index: usize) -> Option<Image> {
        let format = self.with_tags(pkg, index, |tags, _| props::find(pkg, tags, "Format", 0).and_then(|t| t.as_u8()).unwrap_or(0))?;
        let format = Format::from_u8(format)?;
        let Asset::Texture(tex) = &*self.asset(pkg, index)? else { return None };
        let mip = tex.mips.first()?;
        let data = pkg.data.get(mip.data_offset..mip.data_offset + mip.data_len)?;
        let (w, h) = (mip.width as usize, mip.height as usize);
        let rgba = texture::decode_rgba(format, w, h, data, None)?;
        let has_alpha = matches!(format, Format::Rgba8 | Format::Dxt3 | Format::Dxt5);
        let mut out = vec![0u8; w * h * 4];
        for (i, px) in rgba.chunks_exact(4).enumerate() {
            let a = if has_alpha { px[3] } else { px[0] };
            out[i * 4..i * 4 + 4].copy_from_slice(&[a, a, a, a]);
        }
        Some(Image { width: w as u32, height: h as u32, rgba: out, masked: false, translucent: false })
    }

    /// Follows a material reference down to a texture: shaders' diffuse, combiners' first input, modifiers'
    /// material, the current entry of a switch or sequence.
    pub fn base_texture(&self, pkg: &Rc<Package>, r: ObjRef) -> Option<(Rc<Package>, usize)> {
        let (mut p, mut i) = self.set.resolve(pkg, r)?;
        for _ in 0..16 {
            let chain = self.set.export_chain(&p, i);
            // A procedural texture of the Fire package (wet, water, fire, ice...) has no pixels of its own; the
            // image it works on (`SourceTexture`) stands in for it, without the animation.
            let procedural = chain.iter().any(|c| c.eq_ignore_ascii_case("FractalTexture"));
            // A cubemap has no picture, only six faces (+x, -x, +y, -y, +z, -z): the +z one, facing up, is what
            // a lake reflects.
            if chain.iter().any(|c| c.eq_ignore_ascii_case("Cubemap")) {
                let up = self.with_tags(&p, i, |tags, _| props::find(&p, tags, "Faces", 4).and_then(|t| t.as_obj())).flatten()?;
                let (np, ni) = self.set.resolve(&p, up)?;
                p = np;
                i = ni;
                continue;
            }
            if !procedural && chain.iter().any(|c| c.eq_ignore_ascii_case("Texture")) {
                return Some((p, i));
            }
            // A `MaterialSwitch` shows `Materials[Current]` (its own `Material` is only filled in at run time), and a
            // `MaterialSequence` starts with its first item's material.
            let off = std::env::var_os("HP3_NO_SWITCH").is_some();
            let switch = !off && chain.iter().any(|c| c.eq_ignore_ascii_case("MaterialSwitch"));
            let sequence = !off && chain.iter().any(|c| c.eq_ignore_ascii_case("MaterialSequence"));
            let next = self.with_tags(&p, i, |tags, _| {
                if switch
                    && let Some(t) = props::find(&p, tags, "Materials", 0)
                    && t.ty == TagType::Array
                {
                    let mut rd = Reader::new(t.data);
                    let count = rd.ci().ok()?.max(0) as usize;
                    let items: Vec<i32> = (0..count).filter_map(|_| rd.ci().ok()).collect();
                    let current = props::find(&p, tags, "Current", 0).and_then(|c| c.as_i32()).unwrap_or(0).max(0) as usize;
                    if let Some(v) = items.get(current).copied().filter(|&v| v != 0).or_else(|| items.iter().copied().find(|&v| v != 0)) {
                        return Some(ObjRef(v));
                    }
                }
                if sequence
                    && let Some(t) = props::find(&p, tags, "SequenceItems", 0)
                    && t.ty == TagType::Array
                {
                    // Items are structs whose first member is the material.
                    let mut rd = Reader::new(t.data);
                    if rd.ci().ok()? > 0
                        && let Ok(v) = rd.ci()
                        && v != 0
                    {
                        return Some(ObjRef(v));
                    }
                }
                for name in ["SourceTexture", "Diffuse", "Material", "Material1", "Material2", "SelfIllumination", "Materials"] {
                    if let Some(t) = props::find(&p, tags, name, 0) {
                        match t.ty {
                            TagType::Object => return t.as_obj(),
                            // Arrays of structs or objects: take the first element's first object.
                            TagType::Array => {
                                let mut rd = Reader::new(t.data);
                                if rd.ci().ok()? > 0 {
                                    if let Ok(v) = rd.ci()
                                        && v != 0
                                        && name == "Materials"
                                    {
                                        return Some(ObjRef(v));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                None
            })
            .flatten();
            // A procedural texture with no image to work on is its own baked picture.
            let Some(next) = next else { return procedural.then_some((p, i)) };
            let (np, ni) = self.set.resolve(&p, next)?;
            p = np;
            i = ni;
        }
        None
    }

    /// How fast a material's picture slides across a surface (texture repeats per second, `u` and `v`), when a
    /// `TexPanner` is on the way down to the texture: the scenery outside a train's windows scrolls this way.
    pub fn pan_of(&self, pkg: &Rc<Package>, r: ObjRef) -> Option<Pan> {
        let (mut p, mut i) = self.set.resolve(pkg, r)?;
        let mut pan: Option<Pan> = None;
        for _ in 0..16 {
            let chain = self.set.export_chain(&p, i);
            if chain.iter().any(|c| c.eq_ignore_ascii_case("Texture")) {
                break;
            }
            // A triggered panner starts when the level triggers it; it is not a steady scroll.
            let steady = chain.first().is_some_and(|c| c.eq_ignore_ascii_case("TexPanner"));
            let (found, next) = self.with_tags(&p, i, |tags, _| {
                let mut found = None;
                if steady {
                    let rate = props::find(&p, tags, "PanRate", 0).and_then(|t| t.as_f32()).unwrap_or(0.0);
                    // `PanDirection` is a rotator (pitch, yaw, roll); the default is straight along u.
                    let (pitch, yaw) = props::find(&p, tags, "PanDirection", 0)
                        .and_then(|t| {
                            let a = i32::from_le_bytes(t.data.get(0..4)?.try_into().ok()?);
                            let b = i32::from_le_bytes(t.data.get(4..8)?.try_into().ok()?);
                            Some((a, b))
                        })
                        .unwrap_or((0, 0));
                    let (pitch, yaw) = (rot_to_rad(pitch), rot_to_rad(yaw));
                    found = Some(Pan { u: rate * pitch.cos() * yaw.cos(), v: rate * pitch.cos() * yaw.sin() });
                }
                let mut next = None;
                for name in ["SourceTexture", "Diffuse", "Material", "Material1", "Material2", "SelfIllumination"] {
                    if let Some(t) = props::find(&p, tags, name, 0)
                        && t.ty == TagType::Object
                    {
                        next = t.as_obj();
                        break;
                    }
                }
                (found, next)
            })?;
            if let Some(f) = found {
                pan = Some(match pan {
                    Some(o) => Pan { u: o.u + f.u, v: o.v + f.v },
                    None => f,
                });
            }
            let (np, ni) = self.set.resolve(&p, next?)?;
            p = np;
            i = ni;
        }
        pan.filter(|p| p.u != 0.0 || p.v != 0.0)
    }

    /// Material references of a static mesh's sections, from its `Materials` array of structs.
    pub fn static_mesh_materials(&self, pkg: &Package, index: usize) -> Vec<ObjRef> {
        self.with_tags(pkg, index, |tags, _| {
            let Some(t) = props::find(pkg, tags, "Materials", 0) else { return Vec::new() };
            let mut r = Reader::new(t.data);
            let Ok(n) = r.ci() else { return Vec::new() };
            let mut out = Vec::new();
            for _ in 0..n.max(0) {
                let Ok(elem) = props::read_tags(pkg, &mut r) else { break };
                out.push(props::find(pkg, &elem, "Material", 0).and_then(|t| t.as_obj()).unwrap_or_default());
            }
            out
        })
        .unwrap_or_default()
    }
}
