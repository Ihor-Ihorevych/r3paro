//! A "QUIDDITCH" button over the game's front-end menu: the way into a match for a player who has no keyboard (the F12 cheat list
//! is the desktop's door, and the phone has none). A tap or click on it sends the `quidditch` command, which asks the league whom
//! Gryffindor plays next and loads the match; the match's own menu takes the player back out.

use hp3_render::overlay::Quad;

use crate::levelmenu::{pixel, text};

const LABEL: &str = "QUIDDITCH";

/// The button's rectangle (x, y, w, h) in render pixels and the font scale it is drawn at: a wide, thumb-sized box in the bottom-left
/// corner (the menu's own buttons sit in the middle and on the right).
fn layout(width: u32, height: u32) -> ([f32; 4], f32) {
    let s = (height as f32 / 200.0).floor().max(2.0);
    let (w, h) = (LABEL.len() as f32 * 6.0 * s + 6.0 * s, 12.0 * s);
    ([width as f32 * 0.03, height as f32 - h - height as f32 * 0.05, w, h], s)
}

/// Whether the point (render pixels) is on the button.
pub fn hit(x: f32, y: f32, width: u32, height: u32) -> bool {
    let ([bx, by, bw, bh], _) = layout(width, height);
    x >= bx && x <= bx + bw && y >= by && y <= by + bh
}

pub fn quads(width: u32, height: u32) -> Vec<Quad> {
    let ([bx, by, bw, bh], s) = layout(width, height);
    let mut out = Vec::new();
    pixel(&mut out, bx - s, by - s, bw + 2.0 * s, bh + 2.0 * s, [0.0, 0.0, 0.0, 0.75]);
    pixel(&mut out, bx, by, bw, bh, [0.45, 0.07, 0.07, 0.95]);
    text(&mut out, LABEL, bx + 3.0 * s, by + 2.0 * s, s, [1.0, 0.9, 0.5, 1.0]);
    out
}
