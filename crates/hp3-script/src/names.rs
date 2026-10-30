//! Global name table: case-insensitive interned strings, as script names compare.

use crate::fx::FxHashMap as HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Name(pub u32);

impl Name {
    /// `None`, always index 0.
    pub const NONE: Name = Name(0);
}

pub struct Names {
    strings: Vec<String>,
    index: HashMap<String, Name>,
    /// Spellings already looked up, as written: finds them without lowercasing.
    exact: HashMap<String, Name>,
}

impl Default for Names {
    fn default() -> Self {
        let mut n = Names { strings: Vec::new(), index: HashMap::default(), exact: HashMap::default() };
        n.intern("None");
        n
    }
}

impl Names {
    pub fn intern(&mut self, s: &str) -> Name {
        if let Some(&n) = self.exact.get(s) {
            return n;
        }
        let key = s.to_ascii_lowercase();
        let n = match self.index.get(&key) {
            Some(&n) => n,
            None => {
                let n = Name(self.strings.len() as u32);
                self.strings.push(s.to_string());
                self.index.insert(key, n);
                n
            }
        };
        self.exact.insert(s.to_string(), n);
        n
    }

    pub fn find(&self, s: &str) -> Option<Name> {
        self.index.get(&s.to_ascii_lowercase()).copied()
    }

    pub fn get(&self, n: Name) -> &str {
        self.strings.get(n.0 as usize).map(String::as_str).unwrap_or("<bad name>")
    }

    pub fn len(&self) -> usize {
        self.strings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_insensitive() {
        let mut n = Names::default();
        let a = n.intern("Pawn");
        assert_eq!(n.intern("PAWN"), a);
        assert_eq!(n.get(a), "Pawn");
        assert_eq!(n.find("none"), Some(Name::NONE));
    }
}
