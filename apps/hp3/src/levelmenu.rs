//! The debug level list (F11): every map in the install, drawn with a small built-in bitmap font so it works
//! on any screen, whatever the game's own fonts are doing.

use std::path::Path;

use hp3_render::overlay::{Blend, Quad};

pub struct LevelMenu {
    pub open: bool,
    maps: Vec<String>,
    selected: usize,
}

/// 5x7 glyphs, one byte per row (the low five bits, most significant bit leftmost).
fn glyph(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1C, 0x12, 0x11, 0x11, 0x11, 0x12, 0x1C],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0A],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x11, 0x0A, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '_' => [0, 0, 0, 0, 0, 0, 0x1F],
        '-' => [0, 0, 0, 0x1F, 0, 0, 0],
        '.' => [0, 0, 0, 0, 0, 0x0C, 0x0C],
        ':' => [0, 0x0C, 0x0C, 0, 0x0C, 0x0C, 0],
        '>' => [0x08, 0x04, 0x02, 0x01, 0x02, 0x04, 0x08],
        '(' => [0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02],
        ')' => [0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08],
        '/' => [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10],
        _ => [0; 7],
    }
}

pub(crate) fn pixel(out: &mut Vec<Quad>, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
    out.push(Quad { texture: None, rect: [x, y, w, h], uv: [0.0, 0.0, 1.0, 1.0], color, blend: Blend::Alpha });
}

/// Draws `text` with its top-left at (`x`, `y`), each font pixel `s` screen pixels wide.
pub(crate) fn text(out: &mut Vec<Quad>, text: &str, x: f32, y: f32, s: f32, color: [f32; 4]) {
    for (i, c) in text.chars().enumerate() {
        let g = glyph(c);
        let cx = x + i as f32 * 6.0 * s;
        for (row, bits) in g.iter().enumerate() {
            // Runs of lit pixels in a row are one quad.
            let mut col = 0;
            while col < 5 {
                if bits & (0x10 >> col) == 0 {
                    col += 1;
                    continue;
                }
                let start = col;
                while col < 5 && bits & (0x10 >> col) != 0 {
                    col += 1;
                }
                pixel(out, cx + start as f32 * s, y + row as f32 * s, (col - start) as f32 * s, s, color);
            }
        }
    }
}

impl LevelMenu {
    /// The maps of the install (`Maps/*.unr`), in name order.
    pub fn new(root: &Path) -> LevelMenu {
        let dir = ["Maps", "maps"].iter().map(|d| root.join(d)).find(|p| p.is_dir());
        let mut maps: Vec<String> = dir
            .and_then(|d| std::fs::read_dir(d).ok())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                (p.extension()?.eq_ignore_ascii_case("unr")).then(|| p.file_stem().map(|s| s.to_string_lossy().into_owned()))?
            })
            .collect();
        maps.sort_by_key(|m| m.to_ascii_lowercase());
        LevelMenu { open: false, maps, selected: 0 }
    }

    /// Opens the list with `current` selected, or closes it.
    pub fn toggle(&mut self, current: &str) {
        self.open = !self.open;
        if self.open {
            self.selected = self.maps.iter().position(|m| m.eq_ignore_ascii_case(current)).unwrap_or(0);
        }
    }

    pub fn step(&mut self, delta: i32) {
        let n = self.maps.len() as i32;
        if n > 0 {
            self.selected = (self.selected as i32 + delta).clamp(0, n - 1) as usize;
        }
    }

    pub fn home(&mut self) {
        self.selected = 0;
    }

    pub fn end(&mut self) {
        self.selected = self.maps.len().saturating_sub(1);
    }

    /// Closes the list and returns the chosen map.
    pub fn choose(&mut self) -> Option<String> {
        self.open = false;
        self.maps.get(self.selected).cloned()
    }

    pub fn quads(&self, width: u32, height: u32, current: &str) -> Vec<Quad> {
        let mut out = Vec::new();
        if !self.open {
            return out;
        }
        let s = (height as f32 / 240.0).floor().max(2.0);
        let line = 10.0 * s;
        let visible = (((height as f32 - 6.0 * line) / line).floor() as usize).clamp(4, 30);
        let first = self.selected.saturating_sub(visible / 2).min(self.maps.len().saturating_sub(visible));
        let (x, y) = (width as f32 * 0.5 - 32.0 * 6.0 * s * 0.5, 2.0 * line);
        pixel(&mut out, x - 2.0 * line, y - line, 32.0 * 6.0 * s + 4.0 * line, (visible as f32 + 4.0) * line, [0.0, 0.0, 0.0, 0.8]);
        text(&mut out, "SELECT LEVEL", x, y, s, [1.0, 0.9, 0.5, 1.0]);
        text(&mut out, "UP/DOWN  ENTER GO  F11 CLOSE", x, y + line, s * 0.67, [0.7, 0.7, 0.7, 1.0]);
        for (row, name) in self.maps.iter().enumerate().skip(first).take(visible) {
            let yy = y + (row - first + 3) as f32 * line;
            let chosen = row == self.selected;
            let color = if chosen { [1.0, 1.0, 1.0, 1.0] } else { [0.65, 0.75, 0.9, 1.0] };
            if chosen {
                pixel(&mut out, x - 0.5 * line, yy - 0.2 * line, 32.0 * 6.0 * s + line, line, [0.25, 0.3, 0.55, 0.9]);
                text(&mut out, ">", x - 0.5 * line + 2.0, yy, s, color);
            }
            let label = if name.eq_ignore_ascii_case(current) { format!("{name} (HERE)") } else { name.clone() };
            text(&mut out, &label, x + 2.0 * 6.0 * s, yy, s, color);
        }
        out
    }
}
