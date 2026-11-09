//! The debug cheat list (F12): the same entries as the phone's HP3 debug menu (`GameActivity.DEBUG_NAMES`), drawn with the
//! level list's built-in bitmap font. Each entry is a command `commands::run` carries out.

use hp3_render::overlay::Quad;

use crate::levelmenu::{pixel, text};

/// What choosing an entry does.
pub enum Pick {
    /// A command for the game loop (`cheat god`, ...).
    Command(&'static str),
    /// Opens the level list (F11).
    Levels,
}

/// The entries, in the phone menu's order. The font has no comma, so the labels avoid it.
const ENTRIES: [(&str, Option<&str>); 12] = [
    ("GO TO LEVEL...", None),
    ("GOD MODE (ON/OFF)", Some("cheat god")),
    ("FLY", Some("cheat fly")),
    ("GHOST (THROUGH WALLS)", Some("cheat ghost")),
    ("WALK (NORMAL)", Some("cheat walk")),
    ("ALL SHIELDS/CARDS/PIXIES", Some("cheat giveall")),
    ("100 BEANS", Some("cheat beans")),
    ("10 CAULDRON CAKES (CUPCAKES)", Some("cheat cakes")),
    ("10 PUMPKIN PASTIES", Some("cheat pasties")),
    ("BEAT THE BOSS", Some("cheat beatboss")),
    ("DEFEAT PEEVES", Some("cheat defeatpeeves")),
    ("HEAL", Some("cheat heal")),
];

pub struct CheatMenu {
    pub open: bool,
    selected: usize,
    /// The last thing run and when, shown under the list so a choice is seen to have happened.
    done: Option<(std::time::Instant, &'static str)>,
}

impl CheatMenu {
    pub fn new() -> CheatMenu {
        CheatMenu { open: false, selected: 0, done: None }
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    pub fn step(&mut self, delta: i32) {
        self.selected = (self.selected as i32 + delta).clamp(0, ENTRIES.len() as i32 - 1) as usize;
    }

    pub fn home(&mut self) {
        self.selected = 0;
    }

    pub fn end(&mut self) {
        self.selected = ENTRIES.len() - 1;
    }

    /// The chosen entry. The list stays open after a cheat (several can be given in a row); the level list closes it.
    pub fn choose(&mut self) -> Pick {
        let (label, command) = ENTRIES[self.selected];
        match command {
            Some(c) => {
                self.done = Some((std::time::Instant::now(), label));
                Pick::Command(c)
            }
            None => {
                self.open = false;
                Pick::Levels
            }
        }
    }

    pub fn quads(&self, width: u32, height: u32) -> Vec<Quad> {
        let mut out = Vec::new();
        if !self.open {
            return out;
        }
        let s = (height as f32 / 240.0).floor().max(2.0);
        let line = 10.0 * s;
        let w = 34.0 * 6.0 * s;
        let (x, y) = (width as f32 * 0.5 - w * 0.5, 2.0 * line);
        pixel(&mut out, x - 2.0 * line, y - line, w + 4.0 * line, (ENTRIES.len() as f32 + 6.0) * line, [0.0, 0.0, 0.0, 0.8]);
        text(&mut out, "CHEATS", x, y, s, [1.0, 0.9, 0.5, 1.0]);
        text(&mut out, "UP/DOWN  ENTER GIVE  F12 CLOSE", x, y + line, s * 0.67, [0.7, 0.7, 0.7, 1.0]);
        for (row, (label, _)) in ENTRIES.iter().enumerate() {
            let yy = y + (row + 3) as f32 * line;
            let chosen = row == self.selected;
            let color = if chosen { [1.0, 1.0, 1.0, 1.0] } else { [0.65, 0.75, 0.9, 1.0] };
            if chosen {
                pixel(&mut out, x - 0.5 * line, yy - 0.2 * line, w + line, line, [0.25, 0.3, 0.55, 0.9]);
                text(&mut out, ">", x - 0.5 * line + 2.0, yy, s, color);
            }
            text(&mut out, label, x + 2.0 * 6.0 * s, yy, s, color);
        }
        if let Some((at, label)) = self.done
            && at.elapsed().as_secs() < 3
        {
            text(&mut out, &format!("DONE: {label}"), x, y + (ENTRIES.len() + 4) as f32 * line, s * 0.8, [0.5, 1.0, 0.5, 1.0]);
        }
        out
    }
}
