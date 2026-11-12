//! The Quidditch HUD's art: SVG drawings, rasterized once with `resvg` into run-time textures that the canvas
//! draws like any other tile. The drawings are ours (gold and parchment, soft glows), built here as strings
//! so a gauge frame or a lit wing is one format call, not one file.
//!
//! Nothing here decides what the HUD says (that is `hud`) or where it goes (`presentation::draw_hud`); this is
//! only the pictures. `HudArt::build` returns `None` if any of them fails to parse or draw, and the HUD then
//! falls back to its plain bars and words, so a bad drawing can never blank it.

use hp3_assets::loader::Image;
use hp3_script::{ObjId, Vm};

use resvg::{tiny_skia, usvg};

/// How many frames the catch dial's arc has (0 = empty .. `ARC_FRAMES` = full).
pub const ARC_FRAMES: usize = 32;
/// How many feathers each wing has.
pub const FEATHERS: usize = 5;

/// Sizes of the drawings in the HUD's own 720-high units.
pub const DIAL: f32 = 156.0;
pub const EMBLEM: (f32, f32) = (84.0, 56.0);
pub const WING: (f32, f32) = (190.0, 95.0);
pub const HOOP: f32 = 40.0;
pub const PIP: (f32, f32) = (28.0, 16.0);
pub const BROOM: (f32, f32) = (56.0, 20.0);
pub const SHIELD: (f32, f32) = (46.0, 56.0);
pub const KEYCAP: (f32, f32) = (110.0, 46.0);
pub const HAND: f32 = 56.0;
pub const ICON: f32 = 36.0;
pub const RIBBON: (f32, f32) = (640.0, 120.0);

const DEFS: &str = r##"<defs>
<linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#FBE59A"/><stop offset=".55" stop-color="#E8B84A"/><stop offset="1" stop-color="#B8862B"/></linearGradient>
<linearGradient id="gs" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#FFF1B8"/><stop offset="1" stop-color="#C9962F"/></linearGradient>
<filter id="glow" x="-30%" y="-30%" width="160%" height="160%"><feGaussianBlur stdDeviation="3"/></filter>
</defs>"##;

fn svg(w: f32, h: f32, body: &str) -> String {
    format!(r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}" height="{h}">{DEFS}{body}</svg>"##)
}

/// The dial's backing: a dark disc with the empty track of the arc.
fn dial_back() -> String {
    svg(160.0, 160.0, r##"
<defs><radialGradient id="b" cx=".5" cy=".5" r=".5"><stop offset="0" stop-color="#1e160a" stop-opacity=".92"/><stop offset=".82" stop-color="#0c0905" stop-opacity=".86"/><stop offset="1" stop-color="#000" stop-opacity="0"/></radialGradient></defs>
<circle cx="80" cy="80" r="79" fill="url(#b)"/>
<circle cx="80" cy="80" r="62" fill="none" stroke="#3b2d12" stroke-width="10"/>
<circle cx="80" cy="80" r="67.5" fill="none" stroke="url(#g)" stroke-width="1.6" stroke-opacity=".7"/>
<circle cx="80" cy="80" r="56.5" fill="none" stroke="url(#g)" stroke-width="1" stroke-opacity=".45"/>
"##)
}

/// One frame of the dial's arc: `frac` of the ring filled, clockwise from the top, with a glow under it.
fn dial_arc(frac: f32) -> String {
    let c = 2.0 * std::f32::consts::PI * 62.0;
    let on = (frac.clamp(0.0, 1.0) * c).max(0.0);
    if on < 0.5 {
        return svg(160.0, 160.0, "");
    }
    svg(160.0, 160.0, &format!(r##"
<g transform="rotate(-90 80 80)" fill="none" stroke-linecap="round">
<circle cx="80" cy="80" r="62" stroke="#FFD76A" stroke-opacity=".55" stroke-width="15" stroke-dasharray="{on:.2} {c:.2}" filter="url(#glow)"/>
<circle cx="80" cy="80" r="62" stroke="url(#gs)" stroke-width="9" stroke-dasharray="{on:.2} {c:.2}"/>
</g>"##))
}

/// The winged Snitch: a gold ball between two wings.
fn emblem() -> String {
    svg(96.0, 64.0, r##"
<g id="w"><path d="M42 32 C34 14 16 8 3 12 C14 16 22 22 26 31 C18 28 10 30 5 36 C17 37 31 38 42 36 Z" fill="url(#g)" stroke="#7a5a1a" stroke-width="1"/>
<path d="M40 33 C32 22 20 18 11 19" fill="none" stroke="#FFF1B8" stroke-opacity=".7" stroke-width="1"/></g>
<use href="#w" transform="translate(96 0) scale(-1 1)"/>
<circle cx="48" cy="32" r="13" fill="#FFD76A" opacity=".35" filter="url(#glow)"/>
<circle cx="48" cy="32" r="10.5" fill="url(#gs)" stroke="#7a5a1a" stroke-width="1"/>
<ellipse cx="45" cy="28" rx="3.4" ry="2.2" fill="#fff" opacity=".7"/>
"##)
}

/// One wing of the speed meter: five feathers fanning out from the dial, the first `lit` of them gold. A
/// `mirror`ed one is the right-hand wing.
fn wing(lit: usize, mirror: bool) -> String {
    // The order they light in: the middle one first and then outwards, so the wing opens evenly.
    const ORDER: [(f32, f32); FEATHERS] = [(0.0, 68.0), (-17.0, 62.0), (17.0, 62.0), (-34.0, 54.0), (34.0, 54.0)];
    let mut body = String::new();
    for (i, (angle, len)) in ORDER.iter().enumerate() {
        let (fill, stroke) = if i < lit { ("url(#g)", "#7a5a1a") } else { ("#fff\" fill-opacity=\".1", "#fff\" stroke-opacity=\".28") };
        body += &format!(
            r##"<g transform="rotate({angle} 195 50)"><ellipse cx="{cx}" cy="50" rx="{rx}" ry="8" fill="{fill}" stroke="{stroke}" stroke-width="1.2"/></g>"##,
            cx = 195.0 - len,
            rx = len - 4.0,
        );
        if i < lit {
            body += &format!(
                r##"<g transform="rotate({angle} 195 50)"><ellipse cx="{cx}" cy="48" rx="{rx}" ry="1.4" fill="#FFF6D0" opacity=".7"/></g>"##,
                cx = 195.0 - len,
                rx = len - 14.0,
            );
        }
    }
    let body = if mirror { format!(r##"<g transform="translate(200 0) scale(-1 1)">{body}</g>"##) } else { body };
    svg(200.0, 100.0, &body)
}

/// A flight ring for the run counter, empty or taken.
fn hoop(full: bool) -> String {
    if full {
        svg(48.0, 48.0, r##"
<circle cx="24" cy="24" r="17" fill="none" stroke="#FFD76A" stroke-opacity=".6" stroke-width="8" filter="url(#glow)"/>
<circle cx="24" cy="24" r="17" fill="none" stroke="url(#g)" stroke-width="5.5"/>
<circle cx="24" cy="24" r="14.4" fill="none" stroke="#FFF6D0" stroke-opacity=".6" stroke-width="1"/>
<path d="M16 24 L22 30 L33 17" fill="none" stroke="#fff" stroke-width="3" stroke-linecap="round" stroke-linejoin="round" opacity=".9"/>"##)
    } else {
        svg(48.0, 48.0, r##"
<circle cx="24" cy="24" r="17" fill="#000" fill-opacity=".35" stroke="#fff" stroke-opacity=".38" stroke-width="4"/>"##)
    }
}

/// A catch attempt left: a little Snitch.
fn pip() -> String {
    svg(28.0, 16.0, r##"
<path d="M13 8 C10 2 5 1 1 3 C5 5 8 7 9 10 C6 9 4 10 2 12 C6 13 10 12 13 11 Z" fill="url(#g)"/>
<path d="M15 8 C18 2 23 1 27 3 C23 5 20 7 19 10 C22 9 24 10 26 12 C22 13 18 12 15 11 Z" fill="url(#g)"/>
<circle cx="14" cy="9" r="4.2" fill="url(#gs)" stroke="#7a5a1a" stroke-width=".7"/>"##)
}

/// A broom for the Bludger hits left: whole, or lost (greyed and cracked across).
fn broom(ok: bool) -> String {
    if ok {
        svg(56.0, 20.0, r##"
<path d="M2 10 L40 10" stroke="#9b6a2b" stroke-width="3.2" stroke-linecap="round"/>
<path d="M2 9 L40 9" stroke="#e0b070" stroke-width="1" stroke-linecap="round" opacity=".8"/>
<path d="M38 10 C44 3 52 3 55 4 C52 7 52 13 55 16 C52 17 44 17 38 10 Z" fill="url(#g)" stroke="#7a5a1a" stroke-width="1"/>
<path d="M42 10 L53 7 M42 10 L54 10 M42 10 L53 13" stroke="#7a5a1a" stroke-width=".8" opacity=".7"/>"##)
    } else {
        svg(56.0, 20.0, r##"
<g opacity=".4"><path d="M2 10 L40 10" stroke="#888" stroke-width="3.2" stroke-linecap="round"/>
<path d="M38 10 C44 3 52 3 55 4 C52 7 52 13 55 16 C52 17 44 17 38 10 Z" fill="#888"/></g>
<path d="M22 3 L26 9 L21 11 L27 18" fill="none" stroke="#E0463C" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>"##)
    }
}

/// A score plaque: a shield in `color` with a gold rim and a sheen.
fn shield(color: &str, dark: &str) -> String {
    svg(56.0, 68.0, &format!(r##"
<defs><linearGradient id="sc" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{color}"/><stop offset="1" stop-color="{dark}"/></linearGradient></defs>
<path d="M4 4 H52 V36 C52 52 40 62 28 66 C16 62 4 52 4 36 Z" fill="url(#sc)" stroke="url(#g)" stroke-width="3"/>
<path d="M8 8 H48 V20 C36 24 20 24 8 20 Z" fill="#fff" opacity=".16"/>
<path d="M28 6 V64" stroke="url(#g)" stroke-width="1.2" opacity=".5"/>"##))
}

/// A key cap for the catch prompt, its label written over it by the HUD.
fn keycap() -> String {
    svg(110.0, 46.0, r##"
<rect x="3" y="6" width="104" height="36" rx="9" fill="#000" opacity=".4"/>
<rect x="3" y="3" width="104" height="36" rx="9" fill="#1d1608" stroke="url(#g)" stroke-width="2"/>
<rect x="7" y="6" width="96" height="9" rx="5" fill="#fff" opacity=".1"/>"##)
}

/// A reaching hand, for the catch prompt.
fn hand() -> String {
    svg(56.0, 56.0, r##"
<circle cx="28" cy="30" r="25" fill="#FFD76A" opacity=".25" filter="url(#glow)"/>
<g fill="url(#g)" stroke="#7a5a1a" stroke-width="1.2" stroke-linejoin="round">
<rect x="14" y="8" width="7" height="22" rx="3.5" transform="rotate(-14 17 30)"/>
<rect x="22" y="3" width="7" height="26" rx="3.5"/>
<rect x="30" y="4" width="7" height="25" rx="3.5"/>
<rect x="38" y="9" width="7" height="21" rx="3.5" transform="rotate(14 41 30)"/>
<path d="M12 30 C12 26 44 26 44 30 L42 44 C40 52 16 52 14 44 Z"/>
<rect x="3" y="26" width="7" height="19" rx="3.5" transform="rotate(-48 8 38)"/>
</g>"##)
}

/// The steering icon of the hints: four arrows round a dot.
fn fly_icon() -> String {
    svg(48.0, 48.0, r##"
<g fill="url(#g)" stroke="#7a5a1a" stroke-width="1" stroke-linejoin="round">
<path d="M24 4 L33 15 H27 V21 H21 V15 H15 Z"/>
<path d="M24 44 L33 33 H27 V27 H21 V33 H15 Z"/>
<path d="M4 24 L15 15 V21 H21 V27 H15 V33 Z"/>
<path d="M44 24 L33 15 V21 H27 V27 H33 V33 Z"/>
</g><circle cx="24" cy="24" r="3" fill="#FFF6D0"/>"##)
}

/// The banner the result line is written on: gold for a win, slate for a loss.
fn ribbon(won: bool) -> String {
    let (a, b, rim) = if won { ("#FBE59A", "#B8862B", "#7a5a1a") } else { ("#9aa3ad", "#4a525c", "#2b3037") };
    svg(640.0, 120.0, &format!(r##"
<defs><linearGradient id="rb" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{a}"/><stop offset="1" stop-color="{b}"/></linearGradient></defs>
<path d="M0 34 H48 L34 62 L48 90 H0 L16 62 Z" fill="{b}" stroke="{rim}" stroke-width="2"/>
<path d="M640 34 H592 L606 62 L592 90 H640 L624 62 Z" fill="{b}" stroke="{rim}" stroke-width="2"/>
<rect x="40" y="18" width="560" height="88" rx="6" fill="url(#rb)" stroke="{rim}" stroke-width="2.5"/>
<rect x="50" y="26" width="540" height="72" rx="4" fill="#120d05" opacity=".78"/>
<rect x="50" y="26" width="540" height="72" rx="4" fill="none" stroke="{a}" stroke-opacity=".7" stroke-width="1.2"/>
<path d="M60 31 H580" stroke="#fff" stroke-opacity=".15" stroke-width="2"/>"##))
}

/// A colour for an opponent's plaque, from its name: the same name always gets the same one.
fn team_colors(name: &str) -> (String, String) {
    let h = name.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    let hue = (h % 360) as f32;
    let hsl = |l: f32| {
        let (s, l) = (0.62f32, l);
        let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
        let x = c * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
        let m = l - c / 2.0;
        let (r, g, b) = match (hue / 60.0) as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let q = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
        format!("#{:02X}{:02X}{:02X}", q(r), q(g), q(b))
    };
    (hsl(0.42), hsl(0.2))
}

/// Draws `svg` to `w` x `h` pixels as an image with straight alpha (what the canvas expects).
pub fn raster(svg: &str, w: u32, h: u32) -> Option<Image> {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(w.max(1), h.max(1))?;
    let (sx, sy) = (w.max(1) as f32 / tree.size().width(), h.max(1) as f32 / tree.size().height());
    resvg::render(&tree, tiny_skia::Transform::from_scale(sx, sy), &mut pixmap.as_mut());
    let mut rgba = pixmap.take();
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a > 0 && a < 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    Some(Image { width: w.max(1), height: h.max(1), rgba, masked: false, translucent: true })
}

/// Every texture of the HUD, made for one screen size (`bucket`).
#[derive(Clone, Debug)]
pub struct HudArt {
    pub bucket: i32,
    pub dial: ObjId,
    pub emblem: ObjId,
    pub arc: Vec<ObjId>,
    pub wing_l: Vec<ObjId>,
    pub wing_r: Vec<ObjId>,
    pub hoop_off: ObjId,
    pub hoop_on: ObjId,
    pub pip: ObjId,
    pub broom_ok: ObjId,
    pub broom_lost: ObjId,
    pub shield_home: ObjId,
    pub shield_away: ObjId,
    pub keycap: ObjId,
    pub hand: ObjId,
    pub fly: ObjId,
    pub ribbon_win: ObjId,
    pub ribbon_lose: ObjId,
}

/// The size bucket of a screen of this height: the HUD scale in quarters, capped at 2 (a bigger screen
/// stretches the same textures rather than make bigger ones). Rebuilt only when it changes.
pub fn bucket_for(screen_h: f32) -> i32 {
    ((screen_h / 720.0).clamp(0.5, 2.0) * 4.0).round() as i32
}

impl HudArt {
    pub fn build(vm: &mut Vm<'_>, bucket: i32, opponent: &str) -> Option<HudArt> {
        let rs = bucket as f32 / 4.0;
        let px = |v: f32| (v * rs).round().max(1.0) as u32;
        let mut n = 0u32;
        let mut make = |vm: &mut Vm<'_>, what: &str, svg: String, size: (f32, f32)| -> Option<ObjId> {
            n += 1;
            let img = raster(&svg, px(size.0), px(size.1))?;
            crate::fonts::register_image(vm, &format!("QuidHud_{what}_{bucket}_{n}"), img)
        };
        let (c1, c2) = team_colors(opponent);
        let mut arc = Vec::new();
        for i in 0..=ARC_FRAMES {
            arc.push(make(vm, "arc", dial_arc(i as f32 / ARC_FRAMES as f32), (DIAL, DIAL))?);
        }
        let mut wing_l = Vec::new();
        let mut wing_r = Vec::new();
        for k in 0..=FEATHERS {
            wing_l.push(make(vm, "wl", wing(k, false), WING)?);
            wing_r.push(make(vm, "wr", wing(k, true), WING)?);
        }
        Some(HudArt {
            bucket,
            dial: make(vm, "dial", dial_back(), (DIAL, DIAL))?,
            emblem: make(vm, "emblem", emblem(), EMBLEM)?,
            arc,
            wing_l,
            wing_r,
            hoop_off: make(vm, "hoop0", hoop(false), (HOOP, HOOP))?,
            hoop_on: make(vm, "hoop1", hoop(true), (HOOP, HOOP))?,
            pip: make(vm, "pip", pip(), PIP)?,
            broom_ok: make(vm, "broom1", broom(true), BROOM)?,
            broom_lost: make(vm, "broom0", broom(false), BROOM)?,
            shield_home: make(vm, "shield_h", shield("#B3202C", "#5E0F16"), SHIELD)?,
            shield_away: make(vm, "shield_a", shield(&c1, &c2), SHIELD)?,
            keycap: make(vm, "key", keycap(), KEYCAP)?,
            hand: make(vm, "hand", hand(), (HAND, HAND))?,
            fly: make(vm, "fly", fly_icon(), (ICON, ICON))?,
            ribbon_win: make(vm, "ribbon_w", ribbon(true), RIBBON)?,
            ribbon_lose: make(vm, "ribbon_l", ribbon(false), RIBBON)?,
        })
    }
}
