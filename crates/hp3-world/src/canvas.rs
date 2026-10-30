//! The 2D canvas: each frame the HUD (and later the GUI) draws through `Canvas` natives into a list of
//! textured quads, which the renderer draws over the scene.
//!
//! Written from the script declarations and standard Unreal canvas behaviour (drawing at the cursor
//! `OrgX + CurX, OrgY + CurY`, tiles advancing the cursor, `DrawColor`, `Style`); NOT yet verified against
//! HP3's Engine.dll. Text uses the font objects' glyphs; `StrLen` does not wrap.

use std::rc::Rc;

use hp3_assets::font::Font;
use hp3_assets::Asset;
use hp3_script::fx::FxHashMap as HashMap;
use hp3_script::{ObjId, Value, Vm};

use crate::{fire, get, set, world};

/// Unreal canvas styles.
pub const STY_NORMAL: u8 = 1;
pub const STY_MASKED: u8 = 2;
pub const STY_TRANSLUCENT: u8 = 3;
pub const STY_MODULATED: u8 = 4;
pub const STY_ALPHA: u8 = 5;

/// One textured quad in screen pixels; texture coordinates in texels of `material`'s texture.
#[derive(Clone, Debug)]
pub struct Tile {
    pub material: Option<ObjId>,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub u: f32,
    pub v: f32,
    pub ul: f32,
    pub vl: f32,
    pub color: [u8; 4],
    pub style: u8,
}

/// The Windows-1251 code of a Cyrillic character.
fn cp1251(c: char) -> Option<u8> {
    let u = c as u32;
    Some(match u {
        0x0410..=0x044F => (u - 0x0410 + 0xC0) as u8,
        0x0401 => 0xA8,
        0x0451 => 0xB8,
        _ => return None,
    })
}

/// A font ready to lay out text: glyphs, the material of each page, the code-to-glyph remap.
pub struct FontInfo {
    font: Font,
    pages: Vec<Option<ObjId>>,
    remap: HashMap<u16, u16>,
    height: f32,
}

impl FontInfo {
    fn glyph(&self, c: char) -> Option<&hp3_assets::font::Glyph> {
        let code = c as u32;
        let index = if self.font.is_remapped != 0 || code > 255 {
            match self.remap.get(&(code.min(0xffff) as u16)) {
                Some(&i) => i as usize,
                // Fonts without a remap keep Cyrillic at its Windows-1251 position.
                None => cp1251(c)? as usize,
            }
        } else {
            code as usize
        };
        self.font.glyphs.get(index).filter(|g| g.width > 0)
    }

    /// Width of a line of text in pixels.
    pub fn width(&self, text: &str) -> f32 {
        text.chars().map(|c| self.glyph(c).map(|g| g.width + self.font.kerning).unwrap_or(0) as f32).sum()
    }
}

#[derive(Default)]
pub struct CanvasState {
    pub object: Option<ObjId>,
    pub tiles: Vec<Tile>,
    fonts: HashMap<ObjId, Option<Rc<FontInfo>>>,
}

fn font_info(vm: &mut Vm<'_>, font: ObjId) -> Option<Rc<FontInfo>> {
    if let Some(f) = world(vm).canvas.fonts.get(&font) {
        return f.clone();
    }
    if let Some(nf) = world(vm).native_fonts.fonts.get(&font).cloned() {
        let (f, page) = (&nf.0, nf.1);
        let remap = f.remap.iter().copied().collect();
        let height = f.glyphs.iter().map(|g| g.height).max().unwrap_or(0) as f32;
        let info = Some(Rc::new(FontInfo { font: f.clone(), pages: vec![Some(page)], remap, height }));
        world(vm).canvas.fonts.insert(font, info.clone());
        return info;
    }
    // Languages HP3's bitmap fonts do not cover draw every text with a system font of the same height.
    if crate::fonts::need_sap_font(vm) && vm.db.objects.get(font).key.is_some() {
        let height = (|| {
            let (pkg, index) = vm.db.objects.get(font).key.clone()?;
            let p = vm.db.set.get(&pkg).ok()?;
            let chain = vm.db.set.export_chain(&p, index);
            let Some(Asset::Font(f)) = crate::native_asset(&p, index, &chain) else { return None };
            Some(f.glyphs.iter().map(|g| g.height).max().unwrap_or(16))
        })();
        let family = world(vm).config.localize("SAPFont", "all", "Font1Name").or_else(|| world(vm).config.localize("SAPFont", "all", "BigFontName"));
        if let Some(h) = height
            && let Some(native) = crate::fonts::create(vm, family.as_deref().unwrap_or("Arial"), (h as f32 * 0.8).round().max(8.0) as u32)
            && native != font
        {
            let info = font_info(vm, native);
            world(vm).canvas.fonts.insert(font, info.clone());
            return info;
        }
    }
    let info = (|| {
        let (pkg, index) = vm.db.objects.get(font).key.clone()?;
        let p = vm.db.set.get(&pkg).ok()?;
        let chain = vm.db.set.export_chain(&p, index);
        let Some(Asset::Font(f)) = crate::native_asset(&p, index, &chain) else { return None };
        let pages = f.textures.iter().map(|r| vm.db.resolve_obj(&p, *r)).collect();
        let remap = f.remap.iter().copied().collect();
        let height = f.glyphs.iter().map(|g| g.height).max().unwrap_or(0) as f32;
        Some(Rc::new(FontInfo { font: f, pages, remap, height }))
    })();
    world(vm).canvas.fonts.insert(font, info.clone());
    info
}

fn canvas_font(vm: &mut Vm<'_>, canvas: ObjId) -> Option<Rc<FontInfo>> {
    let f = get(vm, canvas, "Font").as_obj()?;
    font_info(vm, f)
}

fn f(vm: &mut Vm<'_>, o: ObjId, name: &str) -> f32 {
    get(vm, o, name).as_float()
}

fn draw_color(vm: &mut Vm<'_>, o: ObjId) -> [u8; 4] {
    let c = get(vm, o, "DrawColor");
    let f = c.fields();
    if f.len() == 4 { [f[0].as_byte(), f[1].as_byte(), f[2].as_byte(), f[3].as_byte()] } else { [255; 4] }
}

fn push_tile(vm: &mut Vm<'_>, canvas: ObjId, material: Option<ObjId>, x: f32, y: f32, w: f32, h: f32, uv: [f32; 4]) {
    let color = draw_color(vm, canvas);
    let style = get(vm, canvas, "Style").as_byte();
    world(vm).canvas.tiles.push(Tile { material, x, y, w, h, u: uv[0], v: uv[1], ul: uv[2], vl: uv[3], color, style });
}

/// Draws a line of text at (x, y); returns its width and height.
fn draw_line(vm: &mut Vm<'_>, canvas: ObjId, text: &str, x: f32, y: f32, clip_right: Option<f32>) -> (f32, f32) {
    let Some(font) = canvas_font(vm, canvas) else { return (0.0, 0.0) };
    let mut cx = x;
    for c in text.chars() {
        let Some(g) = font.glyph(c).copied() else { continue };
        if clip_right.is_some_and(|r| cx + g.width as f32 > r) {
            break;
        }
        let page = font.pages.get(g.texture as usize).copied().flatten();
        push_tile(vm, canvas, page, cx, y, g.width as f32, g.height as f32, [g.u as f32, g.v as f32, g.width as f32, g.height as f32]);
        cx += (g.width + font.font.kerning) as f32;
    }
    (cx - x, font.height)
}

fn text_size(vm: &mut Vm<'_>, canvas: ObjId, text: &str) -> (f32, f32) {
    match canvas_font(vm, canvas) {
        Some(font) => (font.width(text), font.height),
        None => (0.0, 0.0),
    }
}

/// Splits text into lines no wider than `width`, at spaces (and at `eol` markers).
fn wrap(vm: &mut Vm<'_>, canvas: ObjId, text: &str, width: f32, eol: &str) -> Vec<String> {
    let Some(font) = canvas_font(vm, canvas) else { return vec![text.to_string()] };
    let mut out = Vec::new();
    let paragraphs: Vec<&str> = if eol.is_empty() { vec![text] } else { text.split(eol).collect() };
    for para in paragraphs {
        let mut line = String::new();
        for word in para.split(' ') {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if !line.is_empty() && font.width(&candidate) > width {
                out.push(std::mem::take(&mut line));
                line = word.to_string();
            } else {
                line = candidate;
            }
        }
        out.push(line);
    }
    out
}

/// A tile with an explicit color and style (GUI drawing, which does not go through the canvas state).
#[allow(clippy::too_many_arguments)]
pub fn tile(vm: &mut Vm<'_>, material: Option<ObjId>, rect: [f32; 4], uv: [f32; 4], color: [u8; 4], style: u8) {
    world(vm).canvas.tiles.push(Tile {
        material,
        x: rect[0],
        y: rect[1],
        w: rect[2],
        h: rect[3],
        u: uv[0],
        v: uv[1],
        ul: uv[2],
        vl: uv[3],
        color,
        style,
    });
}

/// Width and height of a line of text in a font object.
pub fn measure(vm: &mut Vm<'_>, font: ObjId, text: &str) -> (f32, f32) {
    match font_info(vm, font) {
        Some(f) => (f.width(text), f.height),
        None => (0.0, 0.0),
    }
}

/// Draws a line of text in a font object at (x, y), clipped at `right`.
pub fn text(vm: &mut Vm<'_>, font: ObjId, s: &str, x: f32, y: f32, right: f32, color: [u8; 4]) {
    let Some(f) = font_info(vm, font) else { return };
    let mut cx = x;
    for c in s.chars() {
        let Some(g) = f.glyph(c).copied() else { continue };
        if cx + g.width as f32 > right + 0.5 {
            break;
        }
        let page = f.pages.get(g.texture as usize).copied().flatten();
        tile(vm, page, [cx, y, g.width as f32, g.height as f32], [g.u as f32, g.v as f32, g.width as f32, g.height as f32], color, STY_ALPHA);
        cx += (g.width + f.font.kerning) as f32;
    }
}

/// Draws one frame of the HUD into the tile list for a `width` x `height` screen.
pub fn render(vm: &mut Vm<'_>, width: u32, height: u32) -> Vec<Tile> {
    world(vm).canvas.tiles.clear();
    let Some(pc) = world(vm).player else { return Vec::new() };
    let canvas = match world(vm).canvas.object {
        Some(c) => c,
        None => {
            let Some(class) = vm.db.find("Canvas") else { return Vec::new() };
            let name = vm.db.names.intern("Canvas");
            let c = vm.new_object(class, name);
            world(vm).canvas.object = Some(c);
            c
        }
    };
    set(vm, canvas, "SizeX", Value::Int(width as i32));
    set(vm, canvas, "SizeY", Value::Int(height as i32));
    fire(vm, canvas, "Reset", &mut []);
    set(vm, canvas, "ClipX", Value::Float(width as f32));
    set(vm, canvas, "ClipY", Value::Float(height as f32));
    crate::interactions::pre_render(vm, canvas);
    if let Some(hud) = get(vm, pc, "myHUD").as_obj() {
        fire(vm, hud, "PostRender", &mut [Value::Object(Some(canvas))]);
    }
    // Interactions (the GUI's menus, the console) draw over the HUD.
    fire(vm, canvas, "Reset", &mut []);
    set(vm, canvas, "ClipX", Value::Float(width as f32));
    set(vm, canvas, "ClipY", Value::Float(height as f32));
    crate::interactions::post_render(vm, canvas);
    crate::gui::render(vm, canvas, width, height);
    std::mem::take(&mut world(vm).canvas.tiles)
}

fn arg_f(a: &[Value], i: usize) -> f32 {
    a.get(i).map(Value::as_float).unwrap_or(0.0)
}

pub fn register(vm: &mut Vm<'_>) {
    // StrLen / TextSize(String, out XL, out YL)
    for name in ["Canvas.StrLen", "Canvas.TextSize"] {
        vm.register(name, |vm, this, a| {
            if let Some(c) = this {
                let (w, h) = text_size(vm, c, &a[0].as_str().to_string());
                if a.len() >= 3 {
                    a[1] = Value::Float(w);
                    a[2] = Value::Float(h);
                }
            }
            Ok(Value::Empty)
        });
    }
    // DrawText(String, optional bool CR)
    vm.register("Canvas.DrawText", |vm, this, a| {
        let Some(c) = this else { return Ok(Value::Empty) };
        let text = a[0].as_str().to_string();
        let (org_x, org_y, cur_x, cur_y) = (f(vm, c, "OrgX"), f(vm, c, "OrgY"), f(vm, c, "CurX"), f(vm, c, "CurY"));
        let x = if get(vm, c, "bCenter").as_bool() {
            let (w, _) = text_size(vm, c, &text);
            org_x + (f(vm, c, "ClipX") - w) / 2.0
        } else {
            org_x + cur_x
        };
        let (w, h) = draw_line(vm, c, &text, x, org_y + cur_y, None);
        if a.get(1).is_some_and(Value::as_bool) {
            set(vm, c, "CurX", Value::Float(0.0));
            set(vm, c, "CurY", Value::Float(cur_y + h));
        } else {
            set(vm, c, "CurX", Value::Float(cur_x + w));
        }
        set(vm, c, "CurYL", Value::Float(h));
        Ok(Value::Empty)
    });
    vm.register("Canvas.DrawTextClipped", |vm, this, a| {
        let Some(c) = this else { return Ok(Value::Empty) };
        let text = a[0].as_str().to_string().replace('&', "");
        let (x, y) = (f(vm, c, "OrgX") + f(vm, c, "CurX"), f(vm, c, "OrgY") + f(vm, c, "CurY"));
        let right = f(vm, c, "OrgX") + f(vm, c, "ClipX");
        draw_line(vm, c, &text, x, y, Some(right));
        Ok(Value::Empty)
    });
    // DrawTextJustified(String, byte Justification, x1, y1, x2, y2): 0 left, 1 center, 2 right; centered
    // vertically.
    vm.register("Canvas.DrawTextJustified", |vm, this, a| {
        let Some(c) = this else { return Ok(Value::Empty) };
        let text = a[0].as_str().to_string();
        let (x1, y1, x2, y2) = (arg_f(a, 2), arg_f(a, 3), arg_f(a, 4), arg_f(a, 5));
        let (w, h) = text_size(vm, c, &text);
        let x = match a[1].as_byte() {
            1 => x1 + (x2 - x1 - w) / 2.0,
            2 => x2 - w,
            _ => x1,
        };
        let y = y1 + (y2 - y1 - h) / 2.0;
        draw_line(vm, c, &text, x, y, None);
        Ok(Value::Empty)
    });
    // DrawTile(Material, XL, YL, U, V, UL, VL): at the cursor, which moves right by XL.
    for name in ["Canvas.DrawTile", "Canvas.DrawTileClipped"] {
        vm.register(name, |vm, this, a| {
            let Some(c) = this else { return Ok(Value::Empty) };
            let mat = a.first().and_then(Value::as_obj);
            let (xl, yl) = (arg_f(a, 1), arg_f(a, 2));
            let uv = [arg_f(a, 3), arg_f(a, 4), arg_f(a, 5), arg_f(a, 6)];
            let (cur_x, cur_y) = (f(vm, c, "CurX"), f(vm, c, "CurY"));
            let (x, y) = (f(vm, c, "OrgX") + cur_x, f(vm, c, "OrgY") + cur_y);
            push_tile(vm, c, mat, x, y, xl, yl, uv);
            set(vm, c, "CurX", Value::Float(cur_x + xl));
            let cur_yl = f(vm, c, "CurYL").max(yl);
            set(vm, c, "CurYL", Value::Float(cur_yl));
            Ok(Value::Empty)
        });
    }
    // DrawTileStretched(Material, XL, YL) and DrawTileScaled(Material, XScale, YScale): the whole texture.
    vm.register("Canvas.DrawTileStretched", |vm, this, a| {
        let Some(c) = this else { return Ok(Value::Empty) };
        let mat = a.first().and_then(Value::as_obj);
        let (x, y) = (f(vm, c, "OrgX") + f(vm, c, "CurX"), f(vm, c, "OrgY") + f(vm, c, "CurY"));
        push_tile(vm, c, mat, x, y, arg_f(a, 1), arg_f(a, 2), [0.0, 0.0, -1.0, -1.0]);
        Ok(Value::Empty)
    });
    vm.register("Canvas.DrawTileScaled", |vm, this, a| {
        let Some(c) = this else { return Ok(Value::Empty) };
        let mat = a.first().and_then(Value::as_obj);
        let (x, y) = (f(vm, c, "OrgX") + f(vm, c, "CurX"), f(vm, c, "OrgY") + f(vm, c, "CurY"));
        // Negative sizes ask the renderer to use the texture's size times the scale.
        push_tile(vm, c, mat, x, y, -arg_f(a, 1), -arg_f(a, 2), [0.0, 0.0, -1.0, -1.0]);
        Ok(Value::Empty)
    });
    // DrawTileJustified(Material, byte Justification, XL, YL): the whole texture in the box at the cursor.
    vm.register("Canvas.DrawTileJustified", |vm, this, a| {
        let Some(c) = this else { return Ok(Value::Empty) };
        let mat = a.first().and_then(Value::as_obj);
        let (x, y) = (f(vm, c, "OrgX") + f(vm, c, "CurX"), f(vm, c, "OrgY") + f(vm, c, "CurY"));
        push_tile(vm, c, mat, x, y, arg_f(a, 2), arg_f(a, 3), [0.0, 0.0, -1.0, -1.0]);
        Ok(Value::Empty)
    });
    // WrapStringToArray(Text, out array<string> OutArray, float dx, optional string EOL)
    vm.register("Canvas.WrapStringToArray", |vm, this, a| {
        let Some(c) = this else { return Ok(Value::Empty) };
        let text = a[0].as_str().to_string();
        let eol = a.get(3).map(|v| v.as_str().to_string()).unwrap_or_default();
        let lines = wrap(vm, c, &text, arg_f(a, 2), &eol);
        if a.len() >= 2 {
            a[1] = Value::Array(lines.into_iter().map(Value::Str).collect());
        }
        Ok(Value::Empty)
    });
    // CreateNativeFont(FontName, Height, ...): a system font.
    vm.register("Canvas.CreateNativeFont", |vm, _this, a| {
        let name = a.first().map(|v| v.as_str().to_string()).unwrap_or_default();
        let px = a.get(1).map(Value::as_int).unwrap_or(16).max(1) as u32;
        Ok(Value::Object(crate::fonts::create(vm, &name, px)))
    });
    // 3D in the HUD (actors, portals, lines): not drawn yet.
    for name in ["Canvas.DrawActor", "Canvas.DrawActorOnHud", "Canvas.DrawPortal", "HUD.Draw3DLine"] {
        vm.register(name, |_vm, _this, _a| Ok(Value::Empty));
    }
}
