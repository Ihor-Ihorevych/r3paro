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

/// A component's box on screen: left, top, width, height.
fn layout(vm: &mut Vm<'_>, o: ObjId, screen: Screen) -> [f32; 4] {
    let owner = get(vm, o, "MenuOwner").as_obj().filter(|&m| m != o);
    let parent = owner.map(|m| {
        let b: Vec<f32> = (0..4).map(|i| indexed(vm, m, "Bounds", i).as_float()).collect();
        [b[0], b[1], b[2] - b[0], b[3] - b[1]]
    });
    let bound = get(vm, o, "bBoundToParent").as_bool();
    let scale = get(vm, o, "bScaleToParent").as_bool();
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
    let x = match just {
        1 => r[0] + (r[2] - w) / 2.0,
        2 => r[0] + r[2] - w,
        _ => r[0],
    };
    let y = r[1] + (r[3] - h) / 2.0;
    canvas::text(vm, font, text, x, y, r[0] + r[2].max(w), color);
}

/// Lays out and draws a component and its children.
fn draw_component(vm: &mut Vm<'_>, o: ObjId, canvas_obj: ObjId, screen: Screen, depth: usize) {
    if depth > 16 || !get(vm, o, "bVisible").as_bool() {
        return;
    }
    let r = layout(vm, o, screen);
    store_bounds(vm, o, r);
    call_delegate(vm, o, "OnPreDraw", &mut [Value::Object(Some(canvas_obj))]);
    let handled = call_delegate(vm, o, "OnDraw", &mut [Value::Object(Some(canvas_obj))]).as_bool();
    if !handled {
        if is_a(vm, o, "GUIPage") {
            let bg = get(vm, o, "Background").as_obj();
            let color = color_of(&get(vm, o, "BackgroundColor"));
            let style = get(vm, o, "BackgroundRStyle").as_byte();
            draw_image(vm, bg, r, color, style);
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
            if is_a(vm, o, "GUIGFXButton") {
                let g = get(vm, o, "Graphic").as_obj();
                draw_image(vm, g, r, [255; 4], canvas::STY_ALPHA);
            }
        }
    }
    if let Value::Array(children) = get(vm, o, "Controls") {
        let mut kids: Vec<(f32, ObjId)> = children.iter().filter_map(Value::as_obj).map(|c| (0.0, c)).collect();
        for k in &mut kids {
            k.0 = get(vm, k.1, "RenderWeight").as_float();
        }
        kids.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, c) in kids {
            draw_component(vm, c, canvas_obj, screen, depth + 1);
        }
    }
}

/// Draws the open menus (bottom of the stack first) through the canvas.
pub fn render(vm: &mut Vm<'_>, canvas_obj: ObjId, width: u32, height: u32) {
    let Some(gui) = world(vm).viewport.gui else { return };
    let Value::Array(pages) = get(vm, gui, "MenuStack") else { return };
    let screen = Screen { w: width as f32, h: height as f32 };
    for p in pages.iter().filter_map(Value::as_obj) {
        draw_component(vm, p, canvas_obj, screen, 0);
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
    set(vm, gui, "MouseX", Value::Float(x));
    set(vm, gui, "MouseY", Value::Float(y));
    let Some(page) = top_page(vm) else { return };
    let over = hit(vm, page, x, y);
    let prev = get(vm, gui, "ActiveControl").as_obj();
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
    }
}

/// A mouse button: pressing marks the component under the cursor, releasing over it clicks it.
pub fn mouse_button(vm: &mut Vm<'_>, pressed: bool) {
    let Some(gui) = world(vm).viewport.gui else { return };
    let Some(over) = get(vm, gui, "ActiveControl").as_obj() else { return };
    if pressed {
        set(vm, over, "MenuState", Value::Byte(MSAT_PRESSED));
        call_delegate(vm, over, "OnMousePressed", &mut [Value::Object(Some(over)), Value::Bool(false)]);
    } else {
        let was_pressed = get(vm, over, "MenuState").as_byte() == MSAT_PRESSED;
        set(vm, over, "MenuState", Value::Byte(MSAT_WATCHED));
        call_delegate(vm, over, "OnMouseRelease", &mut [Value::Object(Some(over))]);
        if was_pressed {
            call_delegate(vm, over, "OnClick", &mut [Value::Object(Some(over))]);
        }
    }
}

/// A key for the top page (its `OnKeyEvent` delegate). Returns whether it was used.
pub fn key(vm: &mut Vm<'_>, key: u8, action: u8, delta: f32) -> bool {
    let Some(page) = top_page(vm) else { return false };
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
    vm.register("Actor.SaveGameExists", |_vm, _this, _a| Ok(Value::Bool(false)));
    vm.register("Object.KWDeleteSaveGame", |_vm, _this, _a| Ok(Value::Bool(true)));
    vm.register("Object.GetLanguage", |vm, _this, _a| Ok(Value::Str(world(vm).config.language.clone())));
    vm.register("Object.NeedSAPFont", |vm, _this, _a| Ok(Value::Bool(crate::fonts::need_sap_font(vm))));
}
