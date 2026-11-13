//! Talking to the Quidditch captain (`hp3_world::quidditch::captain`): a prompt over the game when the player stands near them,
//! and a speech box with the offer of the next match. Drawn with the debug lists' built-in bitmap font. The game's own vendor
//! pages are script-driven and tied to the vendors they belong to, so this is a small separate box in the same spirit: the
//! speaker's name, what they say, and the two answers.
//!
//! The keys (E to talk, Y or Enter to say yes, N or Escape to say no) are handled by the main loop, which owns the events.

use hp3_render::overlay::Quad;

use crate::levelmenu::{pixel, text};

/// What the captain offers, depending on the league.
pub enum Offer {
    /// The next fixture, against this house.
    Match(String),
    /// The season is over (nothing is left to play).
    SeasonOver,
}

pub struct CaptainTalk {
    pub open: bool,
}

impl CaptainTalk {
    pub fn new() -> CaptainTalk {
        CaptainTalk { open: false }
    }

    /// The little prompt shown while the player is within reach and the box is shut.
    pub fn prompt(&self, width: u32, height: u32) -> Vec<Quad> {
        let mut out = Vec::new();
        let s = (height as f32 / 240.0).floor().max(2.0);
        let label = "E: TALK TO THE QUIDDITCH CAPTAIN";
        let w = label.len() as f32 * 6.0 * s;
        let (x, y) = (width as f32 * 0.5 - w * 0.5, height as f32 * 0.82);
        pixel(&mut out, x - 3.0 * s, y - 2.0 * s, w + 6.0 * s, 11.0 * s, [0.0, 0.0, 0.0, 0.7]);
        text(&mut out, label, x, y, s, [1.0, 0.9, 0.5, 1.0]);
        out
    }

    /// The speech box: the captain's line (word-wrapped to the box) and the answers beneath.
    pub fn quads(&self, width: u32, height: u32, offer: &Offer) -> Vec<Quad> {
        let mut out = Vec::new();
        if !self.open {
            return out;
        }
        let s = (height as f32 / 240.0).floor().max(2.0);
        let line = 10.0 * s;
        let cols = 40usize;
        let w = cols as f32 * 6.0 * s;
        let (said, answers): (Vec<String>, &str) = match offer {
            Offer::Match(house) => (
                wrap(&format!("Potter! The pitch is empty and the Snitch is restless. Fancy a match against {house}? I have put you down as Seeker, as ever."), cols),
                "Y: PLAY THE MATCH      N: NOT NOW",
            ),
            Offer::SeasonOver => (
                wrap("We have played every fixture this season. Well flown, Potter! Come back when the new term starts.", cols),
                "N: GOODBYE",
            ),
        };
        let x = width as f32 * 0.5 - w * 0.5;
        let h = (said.len() as f32 + 4.5) * line;
        let y = height as f32 * 0.62 - h * 0.5;
        pixel(&mut out, x - 2.0 * line, y - line, w + 4.0 * line, h + line, [0.0, 0.0, 0.0, 0.82]);
        pixel(&mut out, x - 2.0 * line, y - line, w + 4.0 * line, s, [0.8, 0.6, 0.2, 1.0]);
        text(&mut out, "OLIVER, QUIDDITCH CAPTAIN", x, y, s, [1.0, 0.8, 0.35, 1.0]);
        for (i, l) in said.iter().enumerate() {
            text(&mut out, l, x, y + (i as f32 + 1.8) * line, s, [0.95, 0.95, 0.95, 1.0]);
        }
        text(&mut out, answers, x, y + (said.len() as f32 + 3.0) * line, s, [0.55, 1.0, 0.55, 1.0]);
        out
    }
}

/// Breaks `s` into lines of at most `cols` characters at spaces.
fn wrap(s: &str, cols: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in s.split_whitespace() {
        if !cur.is_empty() && cur.len() + 1 + word.len() > cols {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::wrap;

    #[test]
    fn lines_fit_the_box() {
        for l in wrap("Potter! The pitch is empty and the Snitch is restless. Fancy a match against Hufflepuff?", 30) {
            assert!(l.len() <= 30, "{l}");
        }
    }
}
