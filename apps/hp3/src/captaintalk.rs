//! Talking to the Quidditch captain (`hp3_world::quidditch::captain`): a prompt over the game when the player stands near them,
//! and a speech box with the offer of the next match. Drawn with the debug lists' built-in bitmap font. The game's own vendor
//! pages are script-driven and tied to the vendors they belong to, so this is a small separate box in the same spirit: the
//! speaker's name, what they say, and the answers as two buttons.
//!
//! The keys (E to talk, Y or Enter to say yes, N or Escape to say no) and a click or tap on an answer's button are handled by the
//! main loop, which owns the events. On the phone the touch controls' TALK button sends E, and while the box is open the game
//! counts as being in a menu, so a finger is the mouse and taps the buttons.

use hp3_render::overlay::Quad;

use crate::levelmenu::{pixel, text};

/// What the captain offers, depending on the league.
pub enum Offer {
    /// The next fixture, against this house.
    Match(String),
    /// The season is over (nothing is left to play).
    SeasonOver,
}

/// What the player said.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
}

pub struct CaptainTalk {
    pub open: bool,
}

/// Where everything in the box goes, in screen pixels.
struct Layout {
    s: f32,
    line: f32,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    said: Vec<String>,
    /// Each answer's button: what it says, what it means, and its rectangle (x, y, width, height).
    buttons: Vec<(&'static str, Answer, [f32; 4])>,
}

fn layout(width: u32, height: u32, offer: &Offer) -> Layout {
    let s = (height as f32 / 240.0).floor().max(2.0);
    let line = 10.0 * s;
    let cols = 40usize;
    let w = cols as f32 * 6.0 * s;
    let (said, answers): (Vec<String>, Vec<(&'static str, Answer)>) = match offer {
        Offer::Match(house) => (
            wrap(&format!("Potter! The pitch is empty and the Snitch is restless. Fancy a match against {house}? I have put you down as Seeker, as ever."), cols),
            vec![(if cfg!(target_os = "android") { "PLAY THE MATCH" } else { "Y: PLAY THE MATCH" }, Answer::Yes), (if cfg!(target_os = "android") { "NOT NOW" } else { "N: NOT NOW" }, Answer::No)],
        ),
        Offer::SeasonOver => (
            wrap("We have played every fixture this season. Well flown, Potter! Come back when the new term starts.", cols),
            vec![(if cfg!(target_os = "android") { "GOODBYE" } else { "N: GOODBYE" }, Answer::No)],
        ),
    };
    let x = width as f32 * 0.5 - w * 0.5;
    // The speech, a gap, then a row of buttons two lines tall (big enough for a thumb).
    let h = (said.len() as f32 + 5.5) * line;
    let y = height as f32 * 0.6 - h * 0.5;
    let gap = line;
    let bw = (w - gap * (answers.len() as f32 - 1.0)) / answers.len() as f32;
    let by = y + (said.len() as f32 + 2.6) * line;
    let buttons = answers.into_iter().enumerate().map(|(i, (label, a))| (label, a, [x + i as f32 * (bw + gap), by, bw, 2.2 * line])).collect();
    Layout { s, line, x, y, w, h, said, buttons }
}

impl CaptainTalk {
    pub fn new() -> CaptainTalk {
        CaptainTalk { open: false }
    }

    /// The little prompt shown while the player is within reach and the box is shut.
    pub fn prompt(&self, width: u32, height: u32) -> Vec<Quad> {
        let mut out = Vec::new();
        let s = (height as f32 / 240.0).floor().max(2.0);
        let label = if cfg!(target_os = "android") { "TAP TALK: THE QUIDDITCH CAPTAIN" } else { "E: TALK TO THE QUIDDITCH CAPTAIN" };
        let w = label.len() as f32 * 6.0 * s;
        let (x, y) = (width as f32 * 0.5 - w * 0.5, height as f32 * 0.82);
        pixel(&mut out, x - 3.0 * s, y - 2.0 * s, w + 6.0 * s, 11.0 * s, [0.0, 0.0, 0.0, 0.7]);
        text(&mut out, label, x, y, s, [1.0, 0.9, 0.5, 1.0]);
        out
    }

    /// The speech box: the captain's line (word-wrapped to the box) and the answers' buttons beneath.
    pub fn quads(&self, width: u32, height: u32, offer: &Offer) -> Vec<Quad> {
        let mut out = Vec::new();
        if !self.open {
            return out;
        }
        let l = layout(width, height, offer);
        let (s, line) = (l.s, l.line);
        pixel(&mut out, l.x - 2.0 * line, l.y - line, l.w + 4.0 * line, l.h + line, [0.0, 0.0, 0.0, 0.82]);
        pixel(&mut out, l.x - 2.0 * line, l.y - line, l.w + 4.0 * line, s, [0.8, 0.6, 0.2, 1.0]);
        text(&mut out, "OLIVER, QUIDDITCH CAPTAIN", l.x, l.y, s, [1.0, 0.8, 0.35, 1.0]);
        for (i, t) in l.said.iter().enumerate() {
            text(&mut out, t, l.x, l.y + (i as f32 + 1.8) * line, s, [0.95, 0.95, 0.95, 1.0]);
        }
        for (label, answer, [bx, by, bw, bh]) in &l.buttons {
            let (fill, ink) = match answer {
                Answer::Yes => ([0.12, 0.38, 0.14, 0.95], [0.75, 1.0, 0.75, 1.0]),
                Answer::No => ([0.38, 0.14, 0.12, 0.95], [1.0, 0.8, 0.75, 1.0]),
            };
            // A frame one font pixel wide around the button.
            pixel(&mut out, *bx, *by, *bw, *bh, [0.8, 0.6, 0.2, 1.0]);
            pixel(&mut out, bx + s, by + s, bw - 2.0 * s, bh - 2.0 * s, fill);
            let tw = label.len() as f32 * 6.0 * s;
            text(&mut out, label, bx + (bw - tw) * 0.5, by + (bh - 7.0 * s) * 0.5, s, ink);
        }
        out
    }

    /// The answer whose button is under (`x`, `y`) in screen pixels, if any.
    pub fn hit(&self, x: f32, y: f32, width: u32, height: u32, offer: &Offer) -> Option<Answer> {
        if !self.open {
            return None;
        }
        layout(width, height, offer).buttons.iter().find(|(_, _, [bx, by, bw, bh])| x >= *bx && x <= bx + bw && y >= *by && y <= by + bh).map(|b| b.1)
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
    use super::*;

    #[test]
    fn lines_fit_the_box() {
        for l in wrap("Potter! The pitch is empty and the Snitch is restless. Fancy a match against Hufflepuff?", 30) {
            assert!(l.len() <= 30, "{l}");
        }
    }

    /// A tap on the middle of each button gives its answer; outside the buttons gives nothing; a shut box takes no taps.
    #[test]
    fn buttons_answer_taps() {
        let mut talk = CaptainTalk::new();
        let offer = Offer::Match("RAVENCLAW".to_string());
        let (w, h) = (1920, 1080);
        assert_eq!(talk.hit(960.0, 540.0, w, h, &offer), None);
        talk.open = true;
        let l = layout(w, h, &offer);
        assert_eq!(l.buttons.len(), 2);
        for (_, answer, [bx, by, bw, bh]) in &l.buttons {
            assert_eq!(talk.hit(bx + bw * 0.5, by + bh * 0.5, w, h, &offer), Some(*answer));
        }
        assert_eq!(talk.hit(5.0, 5.0, w, h, &offer), None);
        let over = layout(w, h, &Offer::SeasonOver);
        assert_eq!(over.buttons.iter().map(|b| b.1).collect::<Vec<_>>(), vec![Answer::No]);
    }
}
