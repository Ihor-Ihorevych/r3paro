//! The native side of the GUI package: menu pages and their components laid out, drawn and clicked, plus the
//! natives the GUI scripts call (styles, fonts, component sizes, automated controls).
//!
//! Written from the GUI scripts and their properties, following the UT2003-era GUI's behaviour as the
//! properties describe it; NOT yet verified against HP3's GUI.dll. In particular: positions and sizes of
//! magnitude <= 1 are fractions (of the parent with `bScaleToParent`, else of the screen), images of style
//! 0 draw at their own size and the others are stretched, styles draw their image stretched over the box.

use hp3_script::{ObjId, Value, Vm};

use crate::canvas;
use crate::{get, set, world};

/// Menu states, which index the styles' arrays.
pub const MSAT_BLURRY: u8 = 0;
pub const MSAT_WATCHED: u8 = 1;
pub const MSAT_PRESSED: u8 = 3;

/// Property flag of components a page creates from its templates (`automated`).
const CPF_AUTOMATED: u32 = 0x8000_0000;

fn is_a(vm: &Vm<'_>, o: ObjId, class: &str) -> bool {
    let Some(want) = vm.db.find(class) else { return false };
    vm.db.objects.get(o).class.is_some_and(|c| vm.is_child_of(c, want))
}

fn color_of(v: &Value) -> [u8; 4] {
    let f = v.fields();
    if f.len() == 4 { [f[0].as_byte(), f[1].as_byte(), f[2].as_byte(), f[3].as_byte()] } else { [255; 4] }
}

fn indexed(vm: &mut Vm<'_>, o: ObjId, name: &str, i: usize) -> Value {
    let Some(s) = crate::slot(vm, o, name) else { return Value::Empty };
    vm.db.objects.values(o).and_then(|v| v.get(s + i).cloned()).unwrap_or_default()
}

/// Calls a delegate of `o` (its assigned function, else the declaring class's default body).
pub fn call_delegate(vm: &mut Vm<'_>, o: ObjId, name: &str, args: &mut [Value]) -> Value {
    let (target, func) = match get(vm, o, &format!("__{name}__Delegate")) {
        Value::Delegate(t, n) if n != hp3_script::Name::NONE => (t, n),
        _ => (Some(o), vm.db.names.intern(name)),
    };
    let Some(f) = vm.find_function(target, None, func) else { return Value::Empty };
    match vm.invoke(f, target, args) {
        Ok(v) => v,
        Err(e) => {
            let at = format!("GUI delegate {name}: {e}");
            let w = world(vm);
            if w.errors.len() < 1000 {
                w.errors.push(at);
            }
            Value::Empty
        }
    }
}

/// Copies a template object into a new instance owned by `outer`.
fn instance_of(vm: &mut Vm<'_>, template: ObjId, outer: ObjId) -> Option<ObjId> {
    if !vm.ensure_loaded(template) {
        return None;
    }
    let e = vm.db.objects.get(template).clone();
    let class = e.class?;
    let mut values = vm.db.objects.values(template)?.clone();
    // Delegates bound to package objects (the templates' owner as declared) are bound to the new owner.
    for v in &mut values {
        if let Value::Delegate(Some(t), n) = v
            && vm.db.objects.get(*t).key.is_some()
        {
            *v = Value::Delegate(Some(outer), *n);
        }
    }
    let o = vm.db.objects.add(e.name, class, values);
    vm.set_intrinsics(o, Some(outer));
    Some(o)
}

/// A screen size, for layout.
#[derive(Clone, Copy)]
struct Screen {
    w: f32,
    h: f32,
}

fn rel(v: f32, size: f32) -> f32 {
    if v.abs() <= 1.0 { v * size } else { v }
}

/// The box of a part of a composite control (a combo box's edit box, button and list, a menu option's label and
/// control): their owner places them, so what their own window values say is not used.
fn part_layout(vm: &mut Vm<'_>, o: ObjId, owner: ObjId) -> Option<[f32; 4]> {
    let b: Vec<f32> = (0..4).map(|i| indexed(vm, owner, "Bounds", i).as_float()).collect();
    let (x, y, w, h) = (b[0], b[1], b[2] - b[0], b[3] - b[1]);
    if is_a(vm, owner, "GUIComboBox") {
        let is = |vm: &mut Vm<'_>, name: &str| get(vm, owner, name).as_obj() == Some(o);
        // The edit box takes the width but a square button's; the list drops below one row per item.
        if is(vm, "Edit") {
            return Some([x, y, (w - h).max(0.0), h]);
        }
        if is(vm, "MyShowListBtn") {
            return Some([x + w - h, y, h, h]);
        }
        if is(vm, "MyListBox") {
            let list = get(vm, owner, "MyListBox").as_obj().and_then(|l| get(vm, l, "List").as_obj());
            let count = list.map(|l| list_items(vm, l).len()).unwrap_or(0);
            let rows = count.min(get(vm, owner, "MaxVisibleItems").as_int().max(1) as usize).max(1);
            return Some([x, y + h, w, h * rows as f32]);
        }
    } else if is_a(vm, owner, "GUIListBox") {
        if get(vm, owner, "List").as_obj() == Some(o) {
            return Some([x, y, w, h]);
        }
    } else if is_a(vm, owner, "GUIMenuOption") {
        let label = get(vm, owner, "MyLabel").as_obj() == Some(o);
        if label || get(vm, owner, "MyComponent").as_obj() == Some(o) {
            let caption = get(vm, owner, "CaptionWidth").as_float();
            let label_w = if caption <= 1.0 { caption * w } else { caption }.clamp(0.0, w);
            let flipped = get(vm, owner, "bFlipped").as_bool();
            let (lx, cx) = if flipped { (x + w - label_w, x) } else { (x, x + label_w) };
            if label {
                return Some([lx, y, label_w, h]);
            }
            let cw = w - label_w;
            // A check box is a square, at the side of the label's opposite.
            if is_a(vm, o, "GUICheckBoxButton") {
                let s = cw.min(h);
                return Some([cx, y + (h - s) / 2.0, s, s]);
            }
            return Some([cx, y, cw, h]);
        }
    }
    None
}

/// A component's box on screen: left, top, width, height.
fn layout(vm: &mut Vm<'_>, o: ObjId, screen: Screen) -> [f32; 4] {
    let owner = get(vm, o, "MenuOwner").as_obj().filter(|&m| m != o);
    if let Some(owner) = owner
        && let Some(r) = part_layout(vm, o, owner)
    {
        return r;
    }
    let parent = owner.map(|m| {
        let b: Vec<f32> = (0..4).map(|i| indexed(vm, m, "Bounds", i).as_float()).collect();
        [b[0], b[1], b[2] - b[0], b[3] - b[1]]
    });
    let bound = get(vm, o, "bBoundToParent").as_bool();
    // The bound children of the trade, final exam and Yes/No pages are laid out in fractions of the page (as the
    // pages' scripts expect); taking them as fractions of the screen on top of the page's origin pushed the buttons
    // down past the parchment, off the screen. Other pages (the card pages) keep the screen-fraction rule.
    let in_vendor = owner.is_some_and(|m| ["HPPageVendor", "HPPageFinalExam", "HPPageYesNo"].iter().any(|c| is_a(vm, m, c)));
    let scale = get(vm, o, "bScaleToParent").as_bool() || (bound && in_vendor);
    let (pw, ph) = match (parent, scale) {
        (Some(p), true) => (p[2], p[3]),
        _ => (screen.w, screen.h),
    };
    let (ox, oy) = match (parent, bound) {
        (Some(p), true) => (p[0], p[1]),
        _ => (0.0, 0.0),
    };
    let left = ox + rel(get(vm, o, "WinLeft").as_float(), pw);
    let top = oy + rel(get(vm, o, "WinTop").as_float(), ph);
    let width = rel(get(vm, o, "WinWidth").as_float(), pw);
    let height = rel(get(vm, o, "WinHeight").as_float(), ph);
    [left, top, width, height]
}

fn store_bounds(vm: &mut Vm<'_>, o: ObjId, r: [f32; 4]) {
    let Some(s) = crate::slot(vm, o, "Bounds") else { return };
    let vals = [r[0], r[1], r[0] + r[2], r[1] + r[3]];
    if let Some(v) = vm.db.objects.values_mut(o) {
        for (i, x) in vals.iter().enumerate() {
            if let Some(slot) = v.get_mut(s + i) {
                *slot = Value::Float(*x);
            }
        }
    }
    if let Some(cs) = crate::slot(vm, o, "ClientBounds")
        && let Some(v) = vm.db.objects.values_mut(o)
    {
        for (i, x) in vals.iter().enumerate() {
            if let Some(slot) = v.get_mut(cs + i) {
                *slot = Value::Float(*x);
            }
        }
    }
}

/// The font object a GUI font name (`MenuFont`...) gives at this screen width.
fn menu_font(vm: &mut Vm<'_>, name: &str, screen_w: f32) -> Option<ObjId> {
    let gui = world(vm).viewport.gui?;
    let gf = get_menu_font(vm, gui, name)?;
    font_for(vm, gf, screen_w)
}

fn get_menu_font(vm: &mut Vm<'_>, gui: ObjId, name: &str) -> Option<ObjId> {
    let Value::Array(fonts) = get(vm, gui, "FontStack") else { return None };
    fonts.iter().filter_map(Value::as_obj).find(|&f| {
        vm.ensure_loaded(f);
        get(vm, f, "KeyName").as_str().eq_ignore_ascii_case(name)
    })
}

/// `GUIFont.GetFont`: the font for the largest configured resolution not above `xres`.
fn font_for(vm: &mut Vm<'_>, gui_font: ObjId, xres: f32) -> Option<ObjId> {
    let res: Vec<i32> = match get(vm, gui_font, "FontArrayResolutions") {
        Value::Array(v) => v.iter().map(Value::as_int).collect(),
        _ => Vec::new(),
    };
    let names: Vec<String> = match get(vm, gui_font, "FontArrayNames") {
        Value::Array(v) => v.iter().map(|x| x.as_str().to_string()).collect(),
        _ => Vec::new(),
    };
    let mut fonts: Vec<Option<ObjId>> = match get(vm, gui_font, "FontArrayFonts") {
        Value::Array(v) => v.iter().map(Value::as_obj).collect(),
        _ => Vec::new(),
    };
    let count = names.len().max(fonts.len());
    if count == 0 {
        return None;
    }
    let pick = if get(vm, gui_font, "bFixedSize").as_bool() {
        0
    } else {
        (0..count).rev().find(|&i| res.get(i).is_some_and(|&r| r as f32 <= xres)).unwrap_or(0)
    };
    // Languages HP3's bitmap fonts do not cover use system fonts of the same size class.
    if crate::fonts::need_sap_font(vm) {
        let class_name = names.get(pick).cloned().unwrap_or_default();
        let screen_h = world(vm).canvas.object.map(|c| get(vm, c, "SizeY").as_int() as f32).unwrap_or(600.0);
        if let Some(f) = crate::fonts::sap_font_for(vm, &class_name, screen_h.max(240.0)) {
            return Some(f);
        }
    }
    fonts.resize(count, None);
    if fonts[pick].is_none() && pick < names.len() {
        let f = crate::find_object_of(vm, &names[pick], Some("Font"));
        fonts[pick] = f;
        set(vm, gui_font, "FontArrayFonts", Value::Array(fonts.iter().map(|f| Value::Object(*f)).collect()));
    }
    fonts[pick]
}

fn get_style(vm: &mut Vm<'_>, gui: ObjId, name: &str) -> Option<ObjId> {
    let Value::Array(styles) = get(vm, gui, "StyleStack") else { return None };
    styles.iter().filter_map(Value::as_obj).find(|&s| get(vm, s, "KeyName").as_str().eq_ignore_ascii_case(name))
}

/// Draws a material over a box (whole texture, stretched).
fn draw_image(vm: &mut Vm<'_>, mat: Option<ObjId>, r: [f32; 4], color: [u8; 4], style: u8) {
    if mat.is_some() && r[2] > 0.0 && r[3] > 0.0 {
        canvas::tile(vm, mat, r, [0.0, 0.0, -1.0, -1.0], color, style);
    }
}

/// `GUIStyles.Draw`: the style's image for the menu state over the box.
fn style_draw(vm: &mut Vm<'_>, style: ObjId, state: u8, r: [f32; 4]) {
    let i = state.min(4) as usize;
    let img = indexed(vm, style, "Images", i).as_obj();
    let color = color_of(&indexed(vm, style, "ImgColors", i));
    let rstyle = indexed(vm, style, "RStyles", i).as_byte();
    draw_image(vm, img, r, color, rstyle);
}

/// `GUIStyles.DrawText`: text in the style's font and color for the state, justified in the box (0 left,
/// 1 center, 2 right) and centered vertically.
fn style_text(vm: &mut Vm<'_>, style: ObjId, state: u8, r: [f32; 4], just: u8, text: &str, screen_w: f32) {
    let i = state.min(4) as usize;
    let font = indexed(vm, style, "Fonts", i).as_obj().and_then(|gf| font_for(vm, gf, screen_w));
    let font = font.or_else(|| {
        let name = indexed(vm, style, "FontNames", i).as_str().to_string();
        menu_font(vm, &name, screen_w)
    });
    let Some(font) = font else { return };
    let color = color_of(&indexed(vm, style, "FontColors", i));
    draw_text(vm, font, text, r, just, color);
}

fn draw_text(vm: &mut Vm<'_>, font: ObjId, text: &str, r: [f32; 4], just: u8, color: [u8; 4]) {
    let (w, h) = canvas::measure(vm, font, text);
    // Text wider than its box wraps onto lines centered vertically as a block, instead of running off the screen.
    if w > r[2] && r[2] > 0.0 && text.contains(' ') {
        let lines = canvas::wrap_in_font(vm, font, text, r[2]);
        let top = r[1] + (r[3] - h * lines.len() as f32) / 2.0;
        for (i, l) in lines.iter().enumerate() {
            let lw = canvas::measure(vm, font, l).0;
            let x = match just {
                1 => r[0] + (r[2] - lw) / 2.0,
                2 => r[0] + r[2] - lw,
                _ => r[0],
            };
            canvas::text(vm, font, l, x, top + h * i as f32, r[0] + r[2].max(lw), color);
        }
        return;
    }
    let x = match just {
        1 => r[0] + (r[2] - w) / 2.0,
        2 => r[0] + r[2] - w,
        _ => r[0],
    };
    let y = r[1] + (r[3] - h) / 2.0;
    canvas::text(vm, font, text, x, y, r[0] + r[2].max(w), color);
}

/// A slider: its style's image as a thin track across the box and the button style's image as a square thumb whose
/// position follows `Value` between `MinValue` and `MaxValue`.
fn draw_slider(vm: &mut Vm<'_>, o: ObjId, r: [f32; 4]) {
    let state = get(vm, o, "MenuState").as_byte();
    let (min, max, value) = (get(vm, o, "MinValue").as_float(), get(vm, o, "MaxValue").as_float(), get(vm, o, "Value").as_float());
    let perc = if max > min { ((value - min) / (max - min)).clamp(0.0, 1.0) } else { 0.0 };
    // The styles of the sliders declare only `ImgStyle`; `RStyles` stays empty.
    let draw = |vm: &mut Vm<'_>, style: ObjId, r: [f32; 4]| {
        let i = state.min(4) as usize;
        let img = indexed(vm, style, "Images", i).as_obj();
        let color = color_of(&indexed(vm, style, "ImgColors", i));
        let mut rstyle = indexed(vm, style, "RStyles", i).as_byte();
        if rstyle == 0 {
            rstyle = indexed(vm, style, "ImgStyle", i).as_byte();
        }
        draw_image(vm, img, r, color, rstyle);
    };
    let thumb = r[3];
    if let Some(style) = get(vm, o, "Style").as_obj() {
        draw(vm, style, [r[0], r[1] + r[3] * 0.375, r[2], r[3] * 0.25]);
    }
    if let Some(style) = get(vm, o, "ButtonStyle").as_obj() {
        draw(vm, style, [r[0] + perc * (r[2] - thumb).max(0.0), r[1], thumb, thumb]);
    }
}

/// Lays out and draws a component and its children.
fn draw_component(vm: &mut Vm<'_>, o: ObjId, canvas_obj: ObjId, screen: Screen, depth: usize, late: &mut Vec<ObjId>) {
    if depth > 16 || !get(vm, o, "bVisible").as_bool() {
        return;
    }
    let r = layout(vm, o, screen);
    store_bounds(vm, o, r);
    call_delegate(vm, o, "OnPreDraw", &mut [Value::Object(Some(canvas_obj))]);
    let tiles_before = world(vm).canvas.tiles.len();
    let handled = call_delegate(vm, o, "OnDraw", &mut [Value::Object(Some(canvas_obj))]).as_bool();
    // What a page draws itself, as tile indices (moved below once the background goes under it).
    let mut own = tiles_before..world(vm).canvas.tiles.len();
    if !handled {
        if is_a(vm, o, "GUIPage") {
            let bg = get(vm, o, "Background").as_obj();
            let color = color_of(&get(vm, o, "BackgroundColor"));
            let style = get(vm, o, "BackgroundRStyle").as_byte();
            draw_image(vm, bg, r, color, style);
            // A page's own drawing (the stats, cards and portraits tabs) goes over the page background.
            let tiles = &mut world(vm).canvas.tiles;
            if tiles.len() > tiles_before + 1 {
                let bg_tile = tiles.pop().unwrap();
                tiles.insert(tiles_before, bg_tile);
                own = own.start + 1..own.end + 1;
            }
        } else if is_a(vm, o, "GUIImage") {
            let img = get(vm, o, "Image").as_obj();
            let color = color_of(&get(vm, o, "ImageColor"));
            let style = get(vm, o, "ImageRenderStyle").as_byte();
            let sub: Vec<i32> = ["x1", "y1", "x2", "y2"].iter().map(|n| get(vm, o, n).as_int()).collect();
            if sub.iter().all(|&v| v >= 0) {
                let uv = [sub[0] as f32, sub[1] as f32, (sub[2] - sub[0]) as f32, (sub[3] - sub[1]) as f32];
                if img.is_some() {
                    canvas::tile(vm, img, r, uv, color, style);
                }
            } else {
                draw_image(vm, img, r, color, style);
            }
        } else if is_a(vm, o, "GUILabel") {
            let caption = get(vm, o, "Caption").as_str().to_string();
            let font_name = get(vm, o, "TextFont").as_str().to_string();
            if let Some(font) = menu_font(vm, &font_name, screen.w) {
                let color = color_of(&get(vm, o, "TextColor"));
                let just = get(vm, o, "TextAlign").as_byte();
                draw_text(vm, font, &caption, r, just, color);
            }
        } else if is_a(vm, o, "GUIButton") {
            let state = get(vm, o, "MenuState").as_byte();
            if let Some(style) = get(vm, o, "Style").as_obj() {
                style_draw(vm, style, state, r);
                let caption = get(vm, o, "Caption").as_str().to_string();
                if !caption.is_empty() {
                    style_text(vm, style, state, r, 1, &caption, screen.w);
                }
            }
            // A check box shows its mark only while it is checked.
            if is_a(vm, o, "GUIGFXButton") && (!get(vm, o, "bCheckBox").as_bool() || get(vm, o, "bChecked").as_bool()) {
                let g = get(vm, o, "Graphic").as_obj();
                draw_image(vm, g, r, [255; 4], canvas::STY_ALPHA);
            }
        } else if is_a(vm, o, "GUISlider") {
            draw_slider(vm, o, r);
        } else if is_a(vm, o, "GUIEditBox") {
            let state = get(vm, o, "MenuState").as_byte();
            if let Some(style) = get(vm, o, "Style").as_obj() {
                style_draw(vm, style, state, r);
                let text = get(vm, o, "TextStr").as_str().to_string();
                if !text.is_empty() {
                    style_text(vm, style, state, [r[0] + r[3] * 0.3, r[1], r[2] - r[3] * 0.3, r[3]], 0, &text, screen.w);
                }
            }
        } else if is_a(vm, o, "GUIList") {
            draw_list(vm, o, r, screen);
        }
    }
    if let Value::Array(children) = get(vm, o, "Controls") {
        let mut kids: Vec<(f32, ObjId)> = children.iter().filter_map(Value::as_obj).map(|c| (0.0, c)).collect();
        for k in &mut kids {
            k.0 = get(vm, k.1, "RenderWeight").as_float();
        }
        kids.sort_by(|a, b| a.0.total_cmp(&b.0));
        let popup = if is_a(vm, o, "GUIComboBox") { get(vm, o, "MyListBox").as_obj() } else { None };
        for (_, c) in kids {
            // A combo box's open list drops over what is drawn after it, so it is drawn last.
            if Some(c) == popup && get(vm, c, "bVisible").as_bool() {
                late.push(c);
                continue;
            }
            draw_component(vm, c, canvas_obj, screen, depth + 1, late);
        }
    }
    // A page's own drawing ends up over its controls too: the vendor's price is written on the plate image under it
    // (HPPageVendor.Vendor_OnDraw over imgScroll), which would otherwise hide it.
    if !handled && is_a(vm, o, "GUIPage") && own.start < own.end {
        let tiles = &mut world(vm).canvas.tiles;
        if own.end <= tiles.len() {
            let mut mine: Vec<_> = tiles.drain(own).collect();
            // The vendor's price is centred at 272 of 800, a little right of its plate's middle (about 249 of 800):
            // set onto the plate's middle, as the user asked.
            if is_a(vm, o, "HPPageVendor") {
                let shift = 23.0 / 800.0 * screen.w;
                for t in &mut mine {
                    t.x -= shift;
                }
            }
            world(vm).canvas.tiles.extend(mine);
        }
    }
}

/// Draws the open menus (bottom of the stack first) through the canvas.
pub fn render(vm: &mut Vm<'_>, canvas_obj: ObjId, width: u32, height: u32) {
    let Some(gui) = world(vm).viewport.gui else { return };
    let Value::Array(pages) = get(vm, gui, "MenuStack") else { return };
    let screen = Screen { w: width as f32, h: height as f32 };
    let mut late = Vec::new();
    for p in pages.iter().filter_map(Value::as_obj) {
        draw_component(vm, p, canvas_obj, screen, 0, &mut late);
    }
    for c in late {
        draw_component(vm, c, canvas_obj, screen, 1, &mut Vec::new());
    }
}

/// The topmost component under the cursor that takes input, searching the top page.
fn hit(vm: &mut Vm<'_>, o: ObjId, x: f32, y: f32) -> Option<ObjId> {
    if !get(vm, o, "bVisible").as_bool() {
        return None;
    }
    if let Value::Array(children) = get(vm, o, "Controls") {
        let mut kids: Vec<(f32, ObjId)> = children.iter().filter_map(Value::as_obj).map(|c| (0.0, c)).collect();
        for k in &mut kids {
            k.0 = get(vm, k.1, "RenderWeight").as_float();
        }
        kids.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (_, c) in kids {
            if let Some(h) = hit(vm, c, x, y) {
                return Some(h);
            }
        }
    }
    let b: Vec<f32> = (0..4).map(|i| indexed(vm, o, "Bounds", i).as_float()).collect();
    let inside = x >= b[0] && x < b[2] && y >= b[1] && y < b[3];
    (inside && get(vm, o, "bAcceptsInput").as_bool()).then_some(o)
}

/// Whether a menu is open (it then takes the mouse and keys).
pub fn menu_open(vm: &mut Vm<'_>) -> bool {
    top_page(vm).is_some()
}

pub fn top_menu(vm: &mut Vm<'_>) -> Option<ObjId> {
    top_page(vm)
}

fn top_page(vm: &mut Vm<'_>) -> Option<ObjId> {
    let gui = world(vm).viewport.gui?;
    match get(vm, gui, "MenuStack") {
        Value::Array(p) => p.last().and_then(Value::as_obj),
        _ => None,
    }
}

/// The cursor moved: the component under it becomes watched.
pub fn mouse_move(vm: &mut Vm<'_>, x: f32, y: f32) {
    let Some(gui) = world(vm).viewport.gui else { return };
    let (dx, dy) = (x - get(vm, gui, "MouseX").as_float(), y - get(vm, gui, "MouseY").as_float());
    set(vm, gui, "MouseX", Value::Float(x));
    set(vm, gui, "MouseY", Value::Float(y));
    let Some(page) = top_page(vm) else { return };
    let prev = get(vm, gui, "ActiveControl").as_obj();
    // A pressed control that captures the mouse (a slider) keeps it while the cursor leaves its box.
    if let Some(p) = prev
        && get(vm, p, "MenuState").as_byte() == MSAT_PRESSED
        && get(vm, p, "bCaptureMouse").as_bool()
    {
        call_delegate(vm, p, "OnCapturedMouseMove", &mut [Value::Float(dx), Value::Float(dy)]);
        return;
    }
    let over = hit_top(vm, page, x, y);
    if prev != over {
        if let Some(p) = prev
            && get(vm, p, "MenuState").as_byte() != MSAT_PRESSED
        {
            set(vm, p, "MenuState", Value::Byte(MSAT_BLURRY));
        }
        if let Some(o) = over {
            set(vm, o, "MenuState", Value::Byte(MSAT_WATCHED));
        }
        set(vm, gui, "ActiveControl", Value::Object(over));
        // The page shows the hint of what the cursor is on (the card pages put a card's name and description there; an empty
        // hint makes the page show its default one). The scripts only set `Hint`, nothing else tells the page.
        let hint = over.map(|o| get(vm, o, "Hint").as_str().to_string()).unwrap_or_default();
        call_method_with(vm, page, "ChangeHint", &mut [Value::Str(hint)]);
    }
}

/// Runs a script function of an object with arguments, if it has one by that name.
fn call_method_with(vm: &mut Vm<'_>, o: ObjId, name: &str, args: &mut [Value]) {
    let n = vm.db.names.intern(name);
    if let Some(f) = vm.find_function(Some(o), None, n)
        && let Err(e) = vm.invoke(f, Some(o), args)
    {
        world(vm).errors.push(format!("GUI {name}: {e}"));
    }
}

/// A mouse button: pressing marks the component under the cursor, releasing over it clicks it.
pub fn mouse_button(vm: &mut Vm<'_>, pressed: bool) {
    let Some(gui) = world(vm).viewport.gui else { return };
    let over = get(vm, gui, "ActiveControl").as_obj();
    // A press anywhere but on an open drop-down list closes it.
    if pressed && let Some(page) = top_page(vm) {
        close_popups(vm, page, over);
    }
    let Some(over) = over else { return };
    if pressed {
        set(vm, over, "MenuState", Value::Byte(MSAT_PRESSED));
        call_delegate(vm, over, "OnMousePressed", &mut [Value::Object(Some(over)), Value::Bool(false)]);
        // A slider jumps to where it was pressed.
        if is_a(vm, over, "GUISlider") {
            call_delegate(vm, over, "OnCapturedMouseMove", &mut [Value::Float(0.0), Value::Float(0.0)]);
        }
    } else {
        let was_pressed = get(vm, over, "MenuState").as_byte() == MSAT_PRESSED;
        set(vm, over, "MenuState", Value::Byte(MSAT_WATCHED));
        call_delegate(vm, over, "OnMouseRelease", &mut [Value::Object(Some(over))]);
        if was_pressed {
            let in_list = is_a(vm, over, "GUIList");
            if in_list {
                pick_item(vm, gui, over);
            }
            call_delegate(vm, over, "OnClick", &mut [Value::Object(Some(over))]);
            // Choosing an item closes its combo box's list.
            if in_list && let Some(combo) = owning_combo(vm, over) {
                call_method(vm, combo, "HideListBox");
            }
        }
    }
}

/// Runs a script function of an object that takes no arguments.
fn call_method(vm: &mut Vm<'_>, o: ObjId, name: &str) {
    let n = vm.db.names.intern(name);
    if let Some(f) = vm.find_function(Some(o), None, n)
        && let Err(e) = vm.invoke(f, Some(o), &mut [])
    {
        world(vm).errors.push(format!("GUI {name}: {e}"));
    }
}

/// The combo box a control belongs to, searching a few owners up.
fn owning_combo(vm: &mut Vm<'_>, o: ObjId) -> Option<ObjId> {
    let mut at = Some(o);
    for _ in 0..4 {
        let c = at?;
        if is_a(vm, c, "GUIComboBox") {
            return Some(c);
        }
        at = get(vm, c, "MenuOwner").as_obj().filter(|&m| m != c);
    }
    None
}

/// Every combo box under `o` whose drop-down list is open.
fn open_combos(vm: &mut Vm<'_>, o: ObjId, out: &mut Vec<ObjId>, depth: usize) {
    if depth > 12 || !get(vm, o, "bVisible").as_bool() {
        return;
    }
    if is_a(vm, o, "GUIComboBox")
        && let Some(l) = get(vm, o, "MyListBox").as_obj()
        && get(vm, l, "bVisible").as_bool()
    {
        out.push(o);
    }
    if let Value::Array(children) = get(vm, o, "Controls") {
        for c in children.iter().filter_map(Value::as_obj) {
            open_combos(vm, c, out, depth + 1);
        }
    }
}

/// The component under the cursor, an open drop-down list before anything it covers.
fn hit_top(vm: &mut Vm<'_>, page: ObjId, x: f32, y: f32) -> Option<ObjId> {
    let mut combos = Vec::new();
    open_combos(vm, page, &mut combos, 0);
    for combo in combos {
        let list = get(vm, combo, "MyListBox").as_obj().and_then(|l| get(vm, l, "List").as_obj());
        if let Some(l) = list {
            let b: Vec<f32> = (0..4).map(|i| indexed(vm, l, "Bounds", i).as_float()).collect();
            if x >= b[0] && x < b[2] && y >= b[1] && y < b[3] {
                return Some(l);
            }
        }
    }
    hit(vm, page, x, y)
}

/// Closes the open drop-down lists the pressed component is not part of.
fn close_popups(vm: &mut Vm<'_>, page: ObjId, over: Option<ObjId>) {
    let mut combos = Vec::new();
    open_combos(vm, page, &mut combos, 0);
    for combo in combos {
        if over.and_then(|o| owning_combo(vm, o)) != Some(combo) {
            call_method(vm, combo, "HideListBox");
        }
    }
}

/// A press on a list chooses the row under the cursor.
fn pick_item(vm: &mut Vm<'_>, gui: ObjId, list: ObjId) {
    let count = list_items(vm, list).len();
    if count == 0 {
        return;
    }
    let b: Vec<f32> = (0..4).map(|i| indexed(vm, list, "Bounds", i).as_float()).collect();
    let rows = list_rows(vm, list, count);
    let row_h = (b[3] - b[1]) / rows as f32;
    let y = get(vm, gui, "MouseY").as_float();
    let row = (((y - b[1]) / row_h.max(1.0)).floor().max(0.0) as usize).min(rows - 1);
    let top = get(vm, list, "Top").as_int().max(0) as usize;
    let index = (top + row).min(count - 1);
    set(vm, list, "Index", Value::Int(index as i32));
    call_delegate(vm, list, "OnChange", &mut [Value::Object(Some(list))]);
}

/// A key for the top page (its `OnKeyEvent` delegate). Returns whether it was used.
pub fn key(vm: &mut Vm<'_>, key: u8, action: u8, delta: f32) -> bool {
    let Some(page) = top_page(vm) else { return false };
    // The trade page has no key handling: it closes only through its Yes and No buttons, which the page draws itself, and a
    // finger or a key that does not land on No leaves it up for good. Escape (the phone's menu and Back buttons send it) says No.
    if key == 0x1b && action == 1 && is_a(vm, page, "HPPageVendor") {
        if let Some(no) = get(vm, page, "btnNo").as_obj() {
            let name = vm.db.names.intern("InternalOnClick");
            if let Some(f) = vm.find_function(Some(page), None, name)
                && let Err(e) = vm.invoke(f, Some(page), &mut [Value::Object(Some(no))])
            {
                world(vm).errors.push(format!("trade page Escape: {e}"));
            }
            return true;
        }
    }
    let mut args = [Value::Byte(key), Value::Byte(action), Value::Float(delta)];
    call_delegate(vm, page, "OnKeyEvent", &mut args).as_bool()
}

fn arg_f(a: &[Value], i: usize) -> Option<f32> {
    match a.get(i) {
        None | Some(Value::Empty) => None,
        Some(v) => Some(v.as_float()),
    }
}

fn actual(vm: &mut Vm<'_>, this: Option<ObjId>, a: &[Value], horizontal: bool, is_pos: bool) -> f32 {
    let Some(o) = this else { return 0.0 };
    let canvas = world(vm).canvas.object;
    let (sw, sh) = canvas.map(|c| (get(vm, c, "SizeX").as_int() as f32, get(vm, c, "SizeY").as_int() as f32)).unwrap_or((640.0, 480.0));
    let screen = Screen { w: sw.max(1.0), h: sh.max(1.0) };
    let r = layout(vm, o, screen);
    let size = if horizontal { screen.w } else { screen.h };
    match arg_f(a, 0) {
        // A given value is laid out as the component's own would be.
        Some(val) if is_pos => {
            let field = if horizontal { "WinLeft" } else { "WinTop" };
            let own = rel(get(vm, o, field).as_float(), size);
            (if horizontal { r[0] } else { r[1] }) - own + rel(val, size)
        }
        Some(val) => rel(val, size),
        None => match (horizontal, is_pos) {
            (true, true) => r[0],
            (false, true) => r[1],
            (true, false) => r[2],
            (false, false) => r[3],
        },
    }
}

pub fn register(vm: &mut Vm<'_>) {
    vm.register("GUIMultiComponent.InitializeControls", |vm, this, _a| {
        let Some(o) = this else { return Ok(Value::Empty) };
        let Some(class) = vm.db.objects.get(o).class else { return Ok(Value::Empty) };
        // Templates already in Controls (from defaults) become instances too.
        let mut controls: Vec<ObjId> = match get(vm, o, "Controls") {
            Value::Array(v) => v.iter().filter_map(Value::as_obj).collect(),
            _ => Vec::new(),
        };
        for c in &mut controls {
            if vm.db.objects.get(*c).key.is_some()
                && let Some(i) = instance_of(vm, *c, o)
            {
                *c = i;
            }
        }
        let automated: Vec<_> = vm.db.get(class).props.iter().filter(|p| p.flags & CPF_AUTOMATED != 0).cloned().collect();
        for p in automated {
            let Some(template) = vm.db.objects.values(o).and_then(|v| v.get(p.slot)).and_then(Value::as_obj) else { continue };
            let inst = if vm.db.objects.get(template).key.is_some() { instance_of(vm, template, o) } else { Some(template) };
            let Some(inst) = inst else { continue };
            if let Some(v) = vm.db.objects.values_mut(o).and_then(|v| v.get_mut(p.slot)) {
                *v = Value::Object(Some(inst));
            }
            if !controls.contains(&inst) {
                controls.push(inst);
            }
        }
        set(vm, o, "Controls", Value::Array(controls.iter().map(|c| Value::Object(Some(*c))).collect()));
        Ok(Value::Empty)
    });
    vm.register("GUIController.GetStyle", |vm, this, a| {
        let name = a.first().map(|v| v.as_str().to_string()).unwrap_or_default();
        Ok(Value::Object(this.and_then(|g| get_style(vm, g, &name))))
    });
    vm.register("GUIController.GetMenuFont", |vm, this, a| {
        let name = a.first().map(|v| v.as_str().to_string()).unwrap_or_default();
        Ok(Value::Object(this.and_then(|g| get_menu_font(vm, g, &name))))
    });
    vm.register("GUIFont.GetFont", |vm, this, a| {
        let xres = arg_f(a, 0).unwrap_or(800.0);
        Ok(Value::Object(this.and_then(|f| font_for(vm, f, xres))))
    });
    // ActualLeft/Top/Width/Height(optional float Value): a position or size in pixels.
    vm.register("GUIComponent.ActualLeft", |vm, this, a| Ok(Value::Float(actual(vm, this, a, true, true))));
    vm.register("GUIComponent.ActualTop", |vm, this, a| Ok(Value::Float(actual(vm, this, a, false, true))));
    vm.register("GUIComponent.ActualWidth", |vm, this, a| Ok(Value::Float(actual(vm, this, a, true, false))));
    vm.register("GUIComponent.ActualHeight", |vm, this, a| Ok(Value::Float(actual(vm, this, a, false, false))));
    vm.register("GUIStyles.Draw", |vm, this, a| {
        if let Some(s) = this {
            let r = [arg_f(a, 2).unwrap_or(0.0), arg_f(a, 3).unwrap_or(0.0), arg_f(a, 4).unwrap_or(0.0), arg_f(a, 5).unwrap_or(0.0)];
            style_draw(vm, s, a.get(1).map(Value::as_byte).unwrap_or(0), r);
        }
        Ok(Value::Empty)
    });
    vm.register("GUIStyles.DrawText", |vm, this, a| {
        if let Some(s) = this {
            let r = [arg_f(a, 2).unwrap_or(0.0), arg_f(a, 3).unwrap_or(0.0), arg_f(a, 4).unwrap_or(0.0), arg_f(a, 5).unwrap_or(0.0)];
            let text = a.get(7).map(|v| v.as_str().to_string()).unwrap_or_default();
            let sw = world(vm).canvas.object.map(|c| get(vm, c, "SizeX").as_int() as f32).unwrap_or(800.0);
            style_text(vm, s, a.get(1).map(Value::as_byte).unwrap_or(0), r, a.get(6).map(Value::as_byte).unwrap_or(0), &text, sw);
        }
        Ok(Value::Empty)
    });
    vm.register("GUIController.GetCurrentRes", |vm, _this, _a| {
        let (w, h) = world(vm).canvas.object.map(|c| (get(vm, c, "SizeX").as_int(), get(vm, c, "SizeY").as_int())).unwrap_or((800, 600));
        Ok(Value::Str(format!("{}x{}", w.max(640), h.max(480))))
    });
    vm.register("GUIController.GetMainMenuClass", |_vm, _this, _a| Ok(Value::Str("HGame.HPPageFrontend".into())));
    for name in ["GUIController.ResetKeyboard", "GUIController.MouseEmulation", "GUIController.GetMapList", "GUIComponent.SetTimer", "GUIComponent.KillTimer"] {
        vm.register(name, |_vm, _this, _a| Ok(Value::Empty));
    }
    // Saved games and language: no saves yet; the configured language.
    vm.register("Actor.SaveGameExists", |vm, _this, a| Ok(Value::Bool(crate::save::exists(vm, a.first().map_or(0, Value::as_int)))));
    vm.register("Object.KWDeleteSaveGame", |vm, _this, a| Ok(Value::Bool(crate::save::delete(vm, a.first().map_or(0, Value::as_int)))));
    vm.register("Object.GetLanguage", |vm, _this, _a| Ok(Value::Str(world(vm).config.language.clone())));
    vm.register("Object.NeedSAPFont", |vm, _this, _a| Ok(Value::Bool(crate::fonts::need_sap_font(vm))));
}

/// A listing of the open menus and their controls (name, bounds, window fractions), for debugging layouts.
pub fn dump(vm: &mut Vm<'_>) -> Vec<String> {
    fn walk(vm: &mut Vm<'_>, o: ObjId, depth: usize, out: &mut Vec<String>) {
        let name = vm.db.names.get(vm.db.objects.get(o).name).to_string();
        let class = vm.db.objects.get(o).class.map(|c| vm.db.names.get(vm.db.get(c).name).to_string()).unwrap_or_default();
        let b: Vec<f32> = (0..4).map(|i| indexed(vm, o, "Bounds", i).as_float()).collect();
        let w: Vec<f32> = ["WinLeft", "WinTop", "WinWidth", "WinHeight"].iter().map(|n| get(vm, o, n).as_float()).collect();
        let vis = get(vm, o, "bVisible").as_bool();
        let bound = get(vm, o, "bBoundToParent").as_bool();
        out.push(format!("{}{name} ({class}) vis={vis} bound={bound} win={w:.3?} bounds={b:.0?}", "  ".repeat(depth)));
        if is_a(vm, o, "GUISlider") {
            let v: Vec<f32> = ["MinValue", "MaxValue", "Value"].iter().map(|n| get(vm, o, n).as_float()).collect();
            let ini = get(vm, o, "IniOption").as_str().to_string();
            out.push(format!("{}  slider min/max/value={v:.3?} ini={ini:?}", "  ".repeat(depth)));
        }
        if is_a(vm, o, "GUIComboBox") {
            let list = get(vm, o, "MyListBox").as_obj().and_then(|l| get(vm, l, "List").as_obj());
            let items = list.map(|l| list_items(vm, l)).unwrap_or_default();
            let index = list.map(|l| get(vm, l, "Index").as_int());
            let text = get(vm, o, "Edit").as_obj().map(|e| get(vm, e, "TextStr").as_str().to_string());
            out.push(format!("{}  combo index={index:?} text={text:?} items={items:?}", "  ".repeat(depth)));
        }
        if let Value::Array(children) = get(vm, o, "Controls") {
            for c in children.iter().filter_map(Value::as_obj) {
                walk(vm, c, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    if let Some(gui) = world(vm).viewport.gui
        && let Value::Array(pages) = get(vm, gui, "MenuStack")
    {
        for p in pages.iter().filter_map(Value::as_obj) {
            walk(vm, p, 0, &mut out);
        }
    }
    out
}

/// The items of a list, as their text.
fn list_items(vm: &mut Vm<'_>, list: ObjId) -> Vec<String> {
    match get(vm, list, "Elements") {
        // Each element is a struct whose first field is the item's text.
        Value::Array(items) => items.iter().map(|e| e.fields().first().map(|v| v.as_str().to_string()).unwrap_or_default()).collect(),
        _ => Vec::new(),
    }
}

/// A list: its style's image behind, the selected row marked, one row of text per item.
fn draw_list(vm: &mut Vm<'_>, o: ObjId, r: [f32; 4], screen: Screen) {
    let items = list_items(vm, o);
    if let Some(style) = get(vm, o, "Style").as_obj() {
        style_draw(vm, style, 0, r);
    }
    let count = items.len();
    if count == 0 {
        return;
    }
    // A row is as high as the box is for its items (one box height each when the list is a combo box's drop-down).
    let rows = list_rows(vm, o, count);
    let row_h = r[3] / rows as f32;
    let selected = get(vm, o, "Index").as_int();
    let top = get(vm, o, "Top").as_int().max(0) as usize;
    for (n, text) in items.iter().enumerate().skip(top).take(rows) {
        let row = [r[0], r[1] + row_h * (n - top) as f32, r[2], row_h];
        if n as i32 == selected {
            let img = get(vm, o, "SelectedImage").as_obj();
            let color = color_of(&get(vm, o, "SelectedBKColor"));
            draw_image(vm, img, row, color, canvas::STY_ALPHA);
        }
        if let Some(style) = get(vm, o, "Style").as_obj() {
            style_text(vm, style, 0, [row[0] + row[3] * 0.3, row[1], row[2] - row[3] * 0.3, row[3]], 0, text, screen.w);
        }
    }
}

/// How many rows a list shows: all its items, up to what its combo box allows or the list itself says.
fn list_rows(vm: &mut Vm<'_>, o: ObjId, count: usize) -> usize {
    let mut limit = get(vm, o, "ItemsPerPage").as_int();
    let mut owner = get(vm, o, "MenuOwner").as_obj();
    for _ in 0..3 {
        let Some(m) = owner else { break };
        if is_a(vm, m, "GUIComboBox") {
            limit = get(vm, m, "MaxVisibleItems").as_int();
            break;
        }
        owner = get(vm, m, "MenuOwner").as_obj().filter(|&x| x != m);
    }
    if limit > 0 { count.min(limit as usize) } else { count }
}
