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

/// Decoded RGBA8 image.
#[derive(Clone, Debug)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Pixels with alpha below one half are cut out (masked) rather than blended.
    pub masked: bool,
}

/// A global identity for an export: package name (lower case) and export index.
pub type ObjKey = (String, usize);

pub fn key(pkg: &Package, index: usize) -> ObjKey {
    (pkg.name.to_ascii_lowercase(), index)
}

pub struct Loader<'s> {
    pub set: &'s PackageSet,
    images: RefCell<HashMap<ObjKey, Option<Rc<Image>>>>,
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
        Loader { set, images: RefCell::new(HashMap::new()), assets: RefCell::new(HashMap::new()) }
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

    fn decode_image(&self, pkg: &Rc<Package>, index: usize) -> Option<Image> {
        let (format, palette_ref, masked) = self.with_tags(pkg, index, |tags, _| {
            let format = props::find(pkg, tags, "Format", 0).and_then(|t| t.as_u8()).unwrap_or(0);
            let palette = props::find(pkg, tags, "Palette", 0).and_then(|t| t.as_obj());
            let masked = props::find(pkg, tags, "bMasked", 0).is_some_and(|t| t.bool_value);
            (format, palette, masked)
        })?;
        let format = Format::from_u8(format)?;
        let Asset::Texture(tex) = &*self.asset(pkg, index)? else { return None };
        let mip = tex.mips.first()?;
        let palette = match palette_ref.and_then(|p| self.set.resolve(pkg, p)) {
            Some((ppkg, pi)) => match self.asset(&ppkg, pi).as_deref() {
                Some(Asset::Palette(p)) => Some(p.clone()),
                _ => None,
            },
            None => None,
        };
        let data = pkg.data.get(mip.data_offset..mip.data_offset + mip.data_len)?;
        let (w, h) = (mip.width as usize, mip.height as usize);
        let mut rgba = texture::decode_rgba(format, w, h, data, palette.as_ref())?;
        let masked = masked || (format == Format::P8 && palette.is_some_and(|p| p.colors.first().is_some_and(|c| c.a == 0)));
        // Alpha is a cut-out only for masked textures; elsewhere it carries other data (specular, blending),
        // which the renderer does not use yet.
        if !masked {
            for px in rgba.chunks_exact_mut(4) {
                px[3] = 255;
            }
        }
        Some(Image { width: w as u32, height: h as u32, rgba, masked })
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
        Some(Image { width: w as u32, height: h as u32, rgba: out, masked: false })
    }

    /// Follows a material reference down to a texture: shaders' diffuse, combiners' first input, modifiers'
    /// material, the current entry of a switch or sequence.
    pub fn base_texture(&self, pkg: &Rc<Package>, r: ObjRef) -> Option<(Rc<Package>, usize)> {
        let (mut p, mut i) = self.set.resolve(pkg, r)?;
        for _ in 0..16 {
            let chain = self.set.export_chain(&p, i);
            if chain.iter().any(|c| c.eq_ignore_ascii_case("Texture")) {
                return Some((p, i));
            }
            let next = self.with_tags(&p, i, |tags, _| {
                for name in ["Diffuse", "Material", "Material1", "Material2", "SelfIllumination", "Materials"] {
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
            })??;
            let (np, ni) = self.set.resolve(&p, next)?;
            p = np;
            i = ni;
        }
        None
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
