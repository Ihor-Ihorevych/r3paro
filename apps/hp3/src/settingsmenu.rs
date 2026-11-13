//! The Settings view: one screen, the same on the PC and the phone, drawn by the engine with the level list's built-in
//! bitmap font. Its tabs hold the graphics options, the PC's frame rate and field of view, the cheats, the Quidditch
//! entries, the level list and the debug actions. Every entry is a command `commands::run` carries out (the phone's touch
//! panel opens this view with `menu <tab>` instead of keeping lists of its own).
//!
//! Keyboard: F12 opens it (F11 on the level list), Left/Right or Tab change the tab, Up/Down/PageUp/PageDown/Home/End move,
//! Enter chooses, Escape or F12 closes. Mouse and touch: tap a tab, tap an entry, drag the list to scroll (a wheel scrolls
//! it too), tap CLOSE.

use std::time::Instant;

use hp3_render::overlay::Quad;
use hp3_render::quality;

use crate::levelmenu::{pixel, text};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Graphics,
    /// The PC's frame rate limit and field of view (the phone's touch panel has its own sliders for them).
    Game,
    Cheats,
    Quidditch,
    Levels,
    Debug,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Tab::Graphics => "GRAPHICS",
            Tab::Game => "GAME",
            Tab::Cheats => "CHEATS",
            Tab::Quidditch => "QUIDDITCH",
            Tab::Levels => "LEVELS",
            Tab::Debug => "DEBUG",
        }
    }

    /// `menu <name>` (the phone's panel, the F11 key).
    pub fn parse(name: &str) -> Option<Tab> {
        match name.to_ascii_lowercase().as_str() {
            "graphics" | "gfx" => Some(Tab::Graphics),
            "game" => Some(Tab::Game),
            "cheats" | "debug-cheats" => Some(Tab::Cheats),
            "quidditch" => Some(Tab::Quidditch),
            "levels" | "level" => Some(Tab::Levels),
            "debug" => Some(Tab::Debug),
            _ => None,
        }
    }
}

/// The tabs this platform shows.
fn tabs() -> &'static [Tab] {
    if cfg!(target_os = "android") {
        &[Tab::Graphics, Tab::Cheats, Tab::Quidditch, Tab::Levels, Tab::Debug]
    } else {
        &[Tab::Graphics, Tab::Game, Tab::Cheats, Tab::Quidditch, Tab::Levels, Tab::Debug]
    }
}

/// What the game knows that the entries show (read each frame while the view is open).
pub struct Status {
    pub carpe_crab: bool,
    pub fps_limit: u32,
    pub fov_offset: f32,
    pub current_map: String,
}

/// One entry: what it shows, the command it runs, and whether choosing it closes the view (it leaves the level).
struct Row {
    label: String,
    command: String,
    closes: bool,
}

fn row(label: impl Into<String>, command: impl Into<String>) -> Row {
    Row { label: label.into(), command: command.into(), closes: false }
}

fn leaving(label: impl Into<String>, command: impl Into<String>) -> Row {
    Row { label: label.into(), command: command.into(), closes: true }
}

fn on_off(on: bool) -> &'static str {
    if on { "ON" } else { "OFF" }
}

/// Characters across the panel.
const WIDTH_CHARS: f32 = 50.0;

struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Rect {
    fn hit(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

/// Where everything is on a screen of a given size.
struct Layout {
    /// Font pixel size and entry height, in screen pixels.
    s: f32,
    lh: f32,
    x: f32,
    w: f32,
    panel: Rect,
    title_y: f32,
    close: Rect,
    tabs: Vec<Rect>,
    hint_y: f32,
    rows_y: f32,
    visible: usize,
    footer_y: f32,
}

/// A finger or the mouse held on the list.
struct Drag {
    start_y: f32,
    start_scroll: usize,
    moved: bool,
}

pub struct Settings {
    pub open: bool,
    tab: usize,
    selected: usize,
    scroll: usize,
    maps: Vec<String>,
    /// The last thing run and when, shown under the list so a choice is seen to have happened.
    done: Option<(Instant, String)>,
    drag: Option<Drag>,
}

impl Settings {
    pub fn new(root: &std::path::Path) -> Settings {
        Settings { open: false, tab: 0, selected: 0, scroll: 0, maps: crate::levelmenu::list_maps(root), done: None, drag: None }
    }

    fn current_tab(&self) -> Tab {
        tabs()[self.tab.min(tabs().len() - 1)]
    }

    /// Opens the view on `tab` (on the level list with the level being played chosen).
    pub fn open_tab(&mut self, tab: Tab, current_map: &str) {
        let tab = if tabs().contains(&tab) { tab } else { Tab::Graphics };
        self.open = true;
        self.tab = tabs().iter().position(|&t| t == tab).unwrap_or(0);
        self.selected = 0;
        self.scroll = 0;
        self.drag = None;
        if tab == Tab::Levels {
            self.selected = self.maps.iter().position(|m| m.eq_ignore_ascii_case(current_map)).unwrap_or(0);
            self.scroll = self.selected.saturating_sub(4);
        }
    }

    /// F12: opens on the last tab, or closes.
    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.drag = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.drag = None;
    }

    fn rows(&self, st: &Status) -> Vec<Row> {
        match self.current_tab() {
            Tab::Graphics => {
                let aa = quality::aa();
                let aa_label = if aa == quality::Aa::Msaa4 && !quality::msaa_supported() { "MSAA 4X (USING FXAA)" } else { aa.label() };
                let soft = quality::soft_particles();
                let soft_label = if soft && !quality::soft_particles_active() { "ON (NOT WITH MSAA HERE)" } else { on_off(soft) };
                vec![
                    row(format!("ANTI-ALIASING: {aa_label}"), "gfx aa"),
                    row(format!("CHARACTER SHADOWS: {}", on_off(quality::actor_shadows())), "gfx shadows"),
                    row(format!("SHADOW QUALITY: {}", if quality::high_shadows() { "HIGH" } else { "LOW" }), "gfx quality"),
                    row(format!("TONE MAPPING: {}", on_off(quality::tonemap())), "gfx tonemap"),
                    row(format!("CHARACTER RIM LIGHT: {}", on_off(quality::rim_light())), "gfx rim"),
                    row(format!("HEIGHT FOG: {}", on_off(quality::height_fog())), "gfx heightfog"),
                    row(format!("SOFT PARTICLES: {soft_label}"), "gfx softparticles"),
                ]
            }
            Tab::Game => vec![
                row(format!("FPS LIMIT: {}", if st.fps_limit == 0 { "NONE".to_string() } else { st.fps_limit.to_string() }), "fps next"),
                row(format!("FIELD OF VIEW: {:+.0}", st.fov_offset), "fov next"),
            ],
            Tab::Cheats => vec![
                row("GOD MODE (ON/OFF)", "cheat god"),
                row("FLY", "cheat fly"),
                row("GHOST (THROUGH WALLS)", "cheat ghost"),
                row("WALK (NORMAL)", "cheat walk"),
                row("ALL SHIELDS/CARDS/PIXIES", "cheat giveall"),
                row("100 BEANS", "cheat beans"),
                row("10 CAULDRON CAKES", "cheat cakes"),
                row("10 PUMPKIN PASTIES", "cheat pasties"),
                row("BEAT THE BOSS", "cheat beatboss"),
                row("DEFEAT PEEVES", "cheat defeatpeeves"),
                row("HEAL", "cheat heal"),
                row(format!("CARPE PULLS CRABS: {}", on_off(st.carpe_crab)), "cheat carpecrab"),
                row("CARPE FOR CRABS (SELECT/DESELECT)", "cheat spellswitch"),
            ],
            Tab::Quidditch => vec![
                leaving("PLAY THE NEXT MATCH", "quidditch"),
                leaving("LEAVE THE MATCH", "quidditch stop"),
                row("LEAGUE TABLE (TO THE LOG)", "quidditch league"),
            ],
            Tab::Levels => self
                .maps
                .iter()
                .map(|m| leaving(if m.eq_ignore_ascii_case(&st.current_map) { format!("{m} (HERE)") } else { m.clone() }, format!("level {m}")))
                .collect(),
            Tab::Debug => vec![
                row("QUICK SAVE", "quicksave"),
                leaving("QUICK LOAD", "quickload"),
                row("RESET GAMMA/BRIGHTNESS/CONTRAST", "resetpicture"),
                row("LOG THE ACTORS NEARBY", "dumpnear"),
            ],
        }
    }

    fn layout(&self, width: u32, height: u32) -> Layout {
        let (wf, hf) = (width as f32, height as f32);
        // The phone's entries are tall enough for a finger; the PC's are a mouse's size.
        let touch = cfg!(target_os = "android");
        let s = if touch { (hf / 175.0).floor().max(3.0) } else { (hf / 240.0).floor().max(2.0) };
        // Never wider than the screen.
        let s = s.min(((wf * 0.96) / (WIDTH_CHARS * 6.0)).floor().max(1.0));
        let lh = if touch { 16.0 * s } else { 10.0 * s };
        let w = WIDTH_CHARS * 6.0 * s;
        let x = (wf - w) * 0.5;
        let top = lh * 0.4;
        let panel = Rect { x: x - lh * 0.4, y: top, w: w + lh * 0.8, h: hf - 2.0 * top };
        let title_y = top + lh * 0.4;
        let close_w = 7.0 * 6.0 * s;
        let close = Rect { x: x + w - close_w, y: title_y - lh * 0.25, w: close_w, h: lh };
        // The tab bar: each label with a little room on either side.
        let mut tabs_r = Vec::new();
        let mut tx = x;
        let tab_y = title_y + lh * 1.1;
        for t in tabs() {
            let tw = (t.label().len() as f32 + 1.5) * 6.0 * s;
            tabs_r.push(Rect { x: tx, y: tab_y, w: tw, h: lh });
            tx += tw + 6.0 * s;
        }
        let hint_y = tab_y + lh * 1.2;
        let rows_y = if touch { tab_y + lh * 1.3 } else { hint_y + lh };
        let footer_y = panel.y + panel.h - lh * 1.1;
        let visible = (((footer_y - rows_y) / lh).floor() as usize).max(3);
        Layout { s, lh, x, w, panel, title_y, close, tabs: tabs_r, hint_y, rows_y, visible, footer_y }
    }

    /// The first entry shown (the scroll, kept inside the list: opening on a level near the end asks for more).
    fn top(&self, count: usize, visible: usize) -> usize {
        self.scroll.min(count.saturating_sub(visible))
    }

    fn clamp_scroll(&mut self, count: usize, visible: usize) {
        self.scroll = self.scroll.min(count.saturating_sub(visible));
    }

    /// Keeps the chosen entry on screen.
    fn follow(&mut self, visible: usize) {
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + visible {
            self.scroll = self.selected + 1 - visible;
        }
    }

    fn switch_tab(&mut self, delta: i32) {
        let n = tabs().len() as i32;
        self.tab = ((self.tab as i32 + delta).rem_euclid(n)) as usize;
        self.selected = 0;
        self.scroll = 0;
    }

    /// A key while the view is open: what to run, if anything.
    pub fn key(&mut self, sc: sdl3::keyboard::Scancode, st: &Status, width: u32, height: u32) -> Option<String> {
        use sdl3::keyboard::Scancode as K;
        let count = self.rows(st).len();
        let visible = self.layout(width, height).visible;
        let last = count.saturating_sub(1);
        match sc {
            K::Up => self.selected = self.selected.saturating_sub(1),
            K::Down => self.selected = (self.selected + 1).min(last),
            K::PageUp => self.selected = self.selected.saturating_sub(visible),
            K::PageDown => self.selected = (self.selected + visible).min(last),
            K::Home => self.selected = 0,
            K::End => self.selected = last,
            K::Left => self.switch_tab(-1),
            K::Right | K::Tab => self.switch_tab(1),
            K::Return | K::KpEnter | K::Space => return self.choose(self.selected, st),
            K::Escape | K::AcBack | K::F12 => self.close(),
            _ => {}
        }
        self.scroll = self.top(count, visible);
        self.follow(visible);
        None
    }

    /// Runs entry `index` of the current tab.
    fn choose(&mut self, index: usize, st: &Status) -> Option<String> {
        let rows = self.rows(st);
        let r = rows.get(index)?;
        self.selected = index;
        self.done = Some((Instant::now(), r.label.clone()));
        if r.closes {
            self.close();
        }
        Some(r.command.clone())
    }

    /// The row under a point, if any.
    fn row_at(&self, l: &Layout, count: usize, px: f32, py: f32) -> Option<usize> {
        if px < l.x - l.lh * 0.4 || px > l.x + l.w + l.lh * 0.4 || py < l.rows_y {
            return None;
        }
        let k = ((py - l.rows_y) / l.lh).floor() as usize;
        (k < l.visible).then_some(self.top(count, l.visible) + k).filter(|&i| i < count)
    }

    /// The mouse button (or a finger) went down at (`px`, `py`) in screen pixels.
    pub fn press(&mut self, px: f32, py: f32, st: &Status, width: u32, height: u32) -> Option<String> {
        let l = self.layout(width, height);
        if l.close.hit(px, py) {
            self.close();
            return None;
        }
        if let Some(i) = l.tabs.iter().position(|r| r.hit(px, py)) {
            if i != self.tab {
                self.tab = i;
                self.selected = 0;
                self.scroll = 0;
            }
            return None;
        }
        if py >= l.rows_y && py < l.footer_y {
            self.drag = Some(Drag { start_y: py, start_scroll: self.top(self.rows(st).len(), l.visible), moved: false });
        }
        None
    }

    /// The pointer moved (with the button held or not).
    pub fn motion(&mut self, px: f32, py: f32, st: &Status, width: u32, height: u32) {
        let l = self.layout(width, height);
        let count = self.rows(st).len();
        match &mut self.drag {
            Some(d) => {
                let dy = py - d.start_y;
                if dy.abs() > l.lh * 0.35 {
                    d.moved = true;
                }
                if d.moved {
                    let rows = (dy / l.lh).round() as i64;
                    self.scroll = (d.start_scroll as i64 - rows).max(0) as usize;
                    self.clamp_scroll(count, l.visible);
                }
            }
            // A mouse over an entry picks it out (on the PC; a finger only taps).
            None if !cfg!(target_os = "android") => {
                if let Some(i) = self.row_at(&l, count, px, py) {
                    self.selected = i;
                }
            }
            None => {}
        }
    }

    /// The button (or finger) came up: a tap that did not drag chooses the entry under it.
    pub fn release(&mut self, px: f32, py: f32, st: &Status, width: u32, height: u32) -> Option<String> {
        let drag = self.drag.take()?;
        if drag.moved {
            return None;
        }
        let l = self.layout(width, height);
        let count = self.rows(st).len();
        let i = self.row_at(&l, count, px, py)?;
        self.choose(i, st)
    }

    /// A wheel notch: the list scrolls three entries.
    pub fn wheel(&mut self, y: f32, st: &Status, width: u32, height: u32) {
        let l = self.layout(width, height);
        let count = self.rows(st).len();
        self.scroll = if y > 0.0 { self.top(count, l.visible).saturating_sub(3) } else { self.scroll + 3 };
        self.clamp_scroll(count, l.visible);
    }

    pub fn quads(&self, width: u32, height: u32, st: &Status) -> Vec<Quad> {
        let mut out = Vec::new();
        if !self.open {
            return out;
        }
        let l = self.layout(width, height);
        let (s, lh) = (l.s, l.lh);
        // Text sits in the middle of its entry.
        let pad = (lh - 7.0 * s) * 0.5;
        pixel(&mut out, l.panel.x, l.panel.y, l.panel.w, l.panel.h, [0.0, 0.0, 0.0, 0.85]);
        text(&mut out, "SETTINGS", l.x, l.title_y, s, [1.0, 0.9, 0.5, 1.0]);
        pixel(&mut out, l.close.x, l.close.y, l.close.w, l.close.h, [0.45, 0.15, 0.15, 0.95]);
        text(&mut out, "CLOSE", l.close.x + s * 6.0, l.close.y + pad, s, [1.0, 1.0, 1.0, 1.0]);
        for (i, (r, t)) in l.tabs.iter().zip(tabs()).enumerate() {
            let on = i == self.tab;
            pixel(&mut out, r.x, r.y, r.w, r.h, if on { [0.3, 0.38, 0.7, 0.95] } else { [0.15, 0.17, 0.25, 0.9] });
            text(&mut out, t.label(), r.x + 0.75 * 6.0 * s, r.y + pad, s, if on { [1.0, 1.0, 1.0, 1.0] } else { [0.7, 0.75, 0.85, 1.0] });
        }
        if !cfg!(target_os = "android") {
            text(&mut out, "LEFT/RIGHT TAB  UP/DOWN  ENTER CHOOSE  F12 CLOSE", l.x, l.hint_y, s * 0.67, [0.7, 0.7, 0.7, 1.0]);
        }
        let rows = self.rows(st);
        let top = self.top(rows.len(), l.visible);
        for (k, r) in rows.iter().enumerate().skip(top).take(l.visible) {
            let y = l.rows_y + (k - top) as f32 * lh;
            let chosen = k == self.selected;
            let color = if chosen { [1.0, 1.0, 1.0, 1.0] } else { [0.65, 0.75, 0.9, 1.0] };
            if chosen {
                pixel(&mut out, l.x - 0.3 * lh, y, l.w + 0.6 * lh, lh * 0.92, [0.25, 0.3, 0.55, 0.9]);
                text(&mut out, ">", l.x, y + pad, s, color);
            } else if cfg!(target_os = "android") {
                // A faint band behind every entry, so the finger sees where each one is.
                pixel(&mut out, l.x - 0.3 * lh, y, l.w + 0.6 * lh, lh * 0.92, [0.12, 0.13, 0.18, 0.7]);
            }
            text(&mut out, &r.label, l.x + 2.0 * 6.0 * s, y + pad, s, color);
        }
        // A bar at the right edge shows where the list is scrolled to.
        if rows.len() > l.visible {
            let track = l.visible as f32 * lh;
            let bar = (track * l.visible as f32 / rows.len() as f32).max(lh * 0.5);
            let at = (track - bar) * top as f32 / (rows.len() - l.visible) as f32;
            pixel(&mut out, l.x + l.w + 0.1 * lh, l.rows_y, 0.15 * lh, track, [0.2, 0.2, 0.25, 0.9]);
            pixel(&mut out, l.x + l.w + 0.1 * lh, l.rows_y + at, 0.15 * lh, bar, [0.6, 0.65, 0.85, 1.0]);
        }
        if let Some((at, label)) = &self.done
            && at.elapsed().as_secs() < 3
        {
            text(&mut out, &format!("DONE: {label}"), l.x, l.footer_y + pad * 0.5, s * 0.8, [0.5, 1.0, 0.5, 1.0]);
        }
        out
    }
}
