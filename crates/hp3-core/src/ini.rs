//! Ini-style files: the engine's configuration (`Default.ini`, `User.ini`) and localization (`*.int`,
//! `*.rus`...). Sections and keys compare case-insensitively; a key may repeat.

use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Debug, Default)]
pub struct Ini {
    /// Lower-case section name to its entries in file order (lower-case key, raw key, value).
    sections: HashMap<String, Vec<(String, String, String)>>,
}

/// Decodes a text file that may be UTF-16 (with a byte-order mark), UTF-8, or Latin-1.
pub fn decode_text(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let units: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        return String::from_utf16_lossy(&units);
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        return String::from_utf16_lossy(&units);
    }
    let body = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(body) {
        Ok(s) => s.to_string(),
        Err(_) => body.iter().map(|&b| b as char).collect(),
    }
}

impl Ini {
    pub fn parse(text: &str) -> Ini {
        let mut ini = Ini::default();
        let mut section = String::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                section = name.trim().to_ascii_lowercase();
                ini.sections.entry(section.clone()).or_default();
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                let k = k.trim();
                let mut v = v.trim();
                // Quoted values keep their content without the quotes.
                if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
                    v = &v[1..v.len() - 1];
                }
                ini.sections.entry(section.clone()).or_default().push((k.to_ascii_lowercase(), k.to_string(), v.to_string()));
            }
        }
        ini
    }

    pub fn load(path: &Path) -> std::io::Result<Ini> {
        Ok(Ini::parse(&decode_text(&std::fs::read(path)?)))
    }

    /// The last value of a key (later lines override earlier ones).
    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        let k = key.to_ascii_lowercase();
        self.sections.get(&section.to_ascii_lowercase())?.iter().rev().find(|e| e.0 == k).map(|e| e.2.as_str())
    }

    /// Every value of a repeated key, in order.
    pub fn get_all(&self, section: &str, key: &str) -> Vec<&str> {
        let k = key.to_ascii_lowercase();
        self.sections
            .get(&section.to_ascii_lowercase())
            .map(|v| v.iter().filter(|e| e.0 == k).map(|e| e.2.as_str()).collect())
            .unwrap_or_default()
    }

    pub fn set(&mut self, section: &str, key: &str, value: &str) {
        let entries = self.sections.entry(section.to_ascii_lowercase()).or_default();
        let k = key.to_ascii_lowercase();
        match entries.iter_mut().rev().find(|e| e.0 == k) {
            Some(e) => e.2 = value.to_string(),
            None => entries.push((k, key.to_string(), value.to_string())),
        }
    }

    /// Every entry of a section in file order, as (key as written, value).
    pub fn entries(&self, section: &str) -> Vec<(&str, &str)> {
        self.sections
            .get(&section.to_ascii_lowercase())
            .map(|v| v.iter().map(|e| (e.1.as_str(), e.2.as_str())).collect())
            .unwrap_or_default()
    }

    pub fn has_section(&self, section: &str) -> bool {
        self.sections.contains_key(&section.to_ascii_lowercase())
    }

    /// The names of the sections (lower case), sorted.
    pub fn section_names(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.sections.keys().map(String::as_str).collect();
        v.sort_unstable();
        v
    }

    /// Layers `other` over this file: its keys replace this file's.
    pub fn overlay(&mut self, other: &Ini) {
        for (sec, entries) in &other.sections {
            let mine = self.sections.entry(sec.clone()).or_default();
            mine.retain(|e| !entries.iter().any(|o| o.0 == e.0));
            mine.extend(entries.iter().cloned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_keys_and_overrides() {
        let ini = Ini::parse("[Engine.Engine]\nDefaultGame=HGame.HGame\n; comment\n[URL]\nMap=A.unr\nmap=B.unr\n");
        assert_eq!(ini.get("engine.engine", "defaultgame"), Some("HGame.HGame"));
        assert_eq!(ini.get("URL", "Map"), Some("B.unr"));
        assert_eq!(ini.get_all("URL", "MAP"), vec!["A.unr", "B.unr"]);
    }

    #[test]
    fn utf16_with_bom() {
        let mut bytes = vec![0xFF, 0xFE];
        for u in "[A]\nK=Привет\n".encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(Ini::parse(&decode_text(&bytes)).get("A", "K"), Some("Привет"));
    }
}
