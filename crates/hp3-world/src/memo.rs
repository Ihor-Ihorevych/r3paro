//! Per-actor answers that depend only on the actor's own `Actor` properties (flags, size, place), kept between frames and
//! worked out again only for actors written since the last look (the object table stamps every write; see
//! `ObjectTable::changed_since`). The per-frame loops over thousands of level actors, nearly all of them scenery that never
//! changes, then touch one stamp each instead of the actor's property block (a cache miss apiece).

use hp3_script::objects::ObjectTable;
use hp3_script::value::ObjId;

/// The answers of one loop. 0 is "not worked out yet"; the loop stores its own small codes (1 and up).
#[derive(Default)]
pub struct Memo {
    seen: u32,
    answers: Vec<u8>,
}

impl Memo {
    /// Starts a frame's loop: what it reads through `get` is valid for actors nobody has written since the last `begin`.
    pub fn begin(&mut self, objects: &mut ObjectTable) -> MemoPass {
        let seen = std::mem::replace(&mut self.seen, 0);
        self.seen = objects.begin_pass();
        if self.answers.len() < objects.len() {
            self.answers.resize(objects.len(), 0);
        }
        MemoPass { seen }
    }

    /// Forgets every answer (a level was loaded over the old one).
    pub fn clear(&mut self) {
        self.seen = 0;
        self.answers.clear();
    }

    /// The remembered answer of `a`, if it is still good.
    #[inline]
    pub fn get(&self, pass: &MemoPass, objects: &ObjectTable, a: ObjId) -> Option<u8> {
        match self.answers.get(a.0 as usize) {
            Some(&v) if v != 0 && !objects.changed_since(a, pass.seen) => Some(v),
            _ => None,
        }
    }

    #[inline]
    pub fn set(&mut self, a: ObjId, answer: u8) {
        let i = a.0 as usize;
        if i >= self.answers.len() {
            self.answers.resize(i + 1, 0);
        }
        self.answers[i] = answer;
    }
}

/// What `Memo::begin` hands to the loop: the stamp threshold for this frame.
#[derive(Clone, Copy)]
pub struct MemoPass {
    seen: u32,
}
