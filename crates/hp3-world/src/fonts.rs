//! Native (system TrueType) fonts: what `Canvas.CreateNativeFont` returns and what the GUI uses for languages
//! HP3's bitmap fonts do not cover (`SAPFont.<language>` lists them). Glyphs are rasterized once into an atlas
//! texture made at run time.
//!
//! The fonts come from the user's system (Windows' Fonts folder, Android's /system/fonts, common Linux
//! paths); `HP3_FONT` names a font file to use instead. Sizes are scaled from 600 lines to the screen;
//! NOT yet matched to HP3's text sizes.

use std::rc::Rc;

use hp3_assets::font::{Font, Glyph};
use hp3_assets::loader::Image;
use hp3_script::fx::FxHashMap as HashMap;
use hp3_script::{ObjId, Vm};

use crate::world;

/// Characters every native font carries: printable ASCII, Latin-1, Cyrillic and common punctuation.
fn charset() -> Vec<char> {
    let mut v: Vec<char> = (0x20u32..0x7f).chain(0xa0..0x100).chain(0x400..0x460).filter_map(char::from_u32).collect();
    v.extend(['–', '—', '‘', '’', '“', '”', '„', '…', '«', '»', '№', '•', '™']);
    v
}

/// A font file for a family name.
fn font_file(family: &str) -> Option<Vec<u8>> {
    if let Ok(p) = std::env::var("HP3_FONT")
        && let Ok(d) = std::fs::read(p)
    {
        return Some(d);
    }
    let f = family.to_ascii_lowercase();
    let mut names: Vec<&str> = Vec::new();
    if f.contains("ms sans serif") || f.contains("microsoft sans") {
        names.push("micross.ttf");
    }
    if f.contains("arial") {
        names.push("arial.ttf");
    }
    names.extend(["tahoma.ttf", "arial.ttf", "segoeui.ttf"]);
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let dirs = [format!("{windows}\\Fonts"), "/system/fonts".into(), "/usr/share/fonts/truetype/dejavu".into()];
    for d in &dirs {
        for n in &names {
            if let Ok(data) = std::fs::read(std::path::Path::new(d).join(n)) {
                return Some(data);
            }
        }
    }
    for p in ["/system/fonts/Roboto-Regular.ttf", "/system/fonts/DroidSans.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"] {
        if let Ok(data) = std::fs::read(p) {
            return Some(data);
        }
    }
    None
}

#[derive(Default)]
pub struct NativeFonts {
    /// Font objects made here: their glyphs (on one page) and the page's texture object.
    pub fonts: HashMap<ObjId, Rc<(Font, ObjId)>>,
    /// Images of textures made at run time, by texture object.
    pub images: HashMap<ObjId, Rc<Image>>,
    by_name: HashMap<(String, u32), Option<ObjId>>,
}

/// A font object for a system font family at `px` pixels (cached).
pub fn create(vm: &mut Vm<'_>, family: &str, px: u32) -> Option<ObjId> {
    let px = px.clamp(6, 96);
    let key = (family.to_ascii_lowercase(), px);
    if let Some(f) = world(vm).native_fonts.by_name.get(&key) {
        return *f;
    }
    let made = rasterize(vm, family, px);
    world(vm).native_fonts.by_name.insert(key, made);
    made
}

fn rasterize(vm: &mut Vm<'_>, family: &str, px: u32) -> Option<ObjId> {
    let data = font_file(family)?;
    let face = fontdue::Font::from_bytes(data, fontdue::FontSettings::default()).ok()?;
    let size = px as f32;
    let line = face.horizontal_line_metrics(size)?;
    let height = (line.ascent - line.descent).ceil().max(1.0) as i32;
    // Each glyph gets a cell as wide as its advance and as tall as the line, the bitmap at its offsets.
    const WIDTH: i32 = 1024;
    let chars = charset();
    let mut cells = Vec::new();
    let (mut x, mut y) = (0i32, 0i32);
    for &c in &chars {
        let (m, bitmap) = face.rasterize(c, size);
        let advance = m.advance_width.round().max(1.0) as i32;
        if x + advance > WIDTH {
            x = 0;
            y += height + 1;
        }
        cells.push((c, x, y, advance, m, bitmap));
        x += advance + 1;
    }
    let atlas_h = ((y + height + 1) as u32).next_power_of_two() as i32;
    let mut rgba = vec![0u8; (WIDTH * atlas_h * 4) as usize];
    let mut glyphs = Vec::new();
    let mut remap = Vec::new();
    for (c, cx, cy, advance, m, bitmap) in cells {
        let ox = cx + m.xmin;
        let oy = cy + (line.ascent.round() as i32) - (m.height as i32 + m.ymin);
        for by in 0..m.height {
            for bx in 0..m.width {
                let (px_, py_) = (ox + bx as i32, oy + by as i32);
                if px_ < 0 || py_ < 0 || px_ >= WIDTH || py_ >= atlas_h {
                    continue;
                }
                let i = ((py_ * WIDTH + px_) * 4) as usize;
                rgba[i..i + 4].copy_from_slice(&[255, 255, 255, bitmap[by * m.width + bx]]);
            }
        }
        remap.push((c as u32 as u16, glyphs.len() as u16));
        glyphs.push(Glyph { u: cx, v: cy, width: advance, height, texture: 0 });
    }
    let font = Font { glyphs, textures: Vec::new(), kerning: 0, remap, is_remapped: 1 };
    // Objects scripts and the renderer can refer to: the font and its page texture.
    let font_class = vm.db.native_class("Font")?;
    let tex_class = vm.db.find("Texture")?;
    let name = vm.db.names.intern(&format!("NativeFont_{family}_{px}"));
    let font_obj = vm.new_object(font_class, name);
    let page = vm.new_object(tex_class, name);
    let image = Image { width: WIDTH as u32, height: atlas_h as u32, rgba, masked: false, translucent: false };
    let nf = &mut world(vm).native_fonts;
    nf.images.insert(page, Rc::new(image));
    nf.fonts.insert(font_obj, Rc::new((font, page)));
    Some(font_obj)
}

/// Whether the configured language draws its text with system fonts (its script is missing from HP3's
/// bitmap fonts).
pub fn need_sap_font(vm: &mut Vm<'_>) -> bool {
    let lang = world(vm).config.language.to_ascii_lowercase();
    matches!(lang.as_str(), "rus" | "cze" | "pol" | "tha" | "jap" | "kor" | "sim" | "tra")
}

/// The system font standing in for a bitmap font of the size class in `bitmap_name` (Tiny, Small, Med, Big,
/// Huge), from `SAPFont.<language>`.
pub fn sap_font_for(vm: &mut Vm<'_>, bitmap_name: &str, screen_h: f32) -> Option<ObjId> {
    let n = bitmap_name.to_ascii_lowercase();
    let class = if n.contains("tiny") {
        "Tiny"
    } else if n.contains("small") {
        "Small"
    } else if n.contains("med") {
        "Med"
    } else {
        "Big"
    };
    // Most languages name the sizes; Russian numbers them (1 big, 2 medium, 3 tiny, 4 small).
    let numbered = match class {
        "Big" => "Font1",
        "Med" => "Font2",
        "Tiny" => "Font3",
        _ => "Font4",
    };
    let cfg = &mut world(vm).config;
    let name = cfg.localize("SAPFont", "all", &format!("{class}FontName")).or_else(|| cfg.localize("SAPFont", "all", &format!("{numbered}Name")));
    let size = cfg.localize("SAPFont", "all", &format!("{class}FontSize")).or_else(|| cfg.localize("SAPFont", "all", &format!("{numbered}Size")));
    let family = name.unwrap_or_else(|| "Arial".into());
    let size: f32 = size.and_then(|s| s.trim().parse().ok()).unwrap_or(16.0);
    create(vm, &family, (size * screen_h / 600.0).round() as u32)
}
