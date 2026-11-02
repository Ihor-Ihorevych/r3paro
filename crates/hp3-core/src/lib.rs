//! Shared runtime types: configuration and localization files, and the game's file layout.

pub mod ini;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub use ini::Ini;

/// The configuration of an install: `Default.ini` with `User.ini` over it, and the localization files of the
/// configured language.
pub struct Config {
    pub system: PathBuf,
    pub ini: Ini,
    pub user: Ini,
    /// Localization file extension, e.g. `int` or `rus`: the player's choice, else the install's own.
    pub language: String,
    /// The install's own language (`Default.ini`'s `Language`): its texts stand in for what a chosen translation
    /// lacks (a Russian edition's dialogue has no other language).
    pub edition: String,
    localized: HashMap<String, Option<Ini>>,
    /// The other ini files of the System folder (`Spells.ini`...), by lower-case name without the extension.
    named: HashMap<String, Ini>,
}

impl Config {
    pub fn load(root: &Path) -> Config {
        let system = ["System", "system"].iter().map(|d| root.join(d)).find(|p| p.is_dir()).unwrap_or_else(|| root.join("System"));
        let find = |name: &str| -> Option<PathBuf> {
            let entries = std::fs::read_dir(&system).ok()?;
            entries.flatten().map(|e| e.path()).find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name)))
        };
        let ini = find("Default.ini").and_then(|p| Ini::load(&p).ok()).unwrap_or_default();
        let user = find("User.ini").and_then(|p| Ini::load(&p).ok()).unwrap_or_default();
        let edition = ini.get("Engine.Engine", "Language").unwrap_or("int").to_ascii_lowercase();
        // The language the player chose: `HP3_LANGUAGE`, or the one Flipendo's launcher wrote next to the game.
        let chosen = std::env::var("HP3_LANGUAGE")
            .ok()
            .or_else(|| std::fs::read_to_string(root.join(".flipendo-language")).ok())
            .map(|l| l.trim().to_ascii_lowercase())
            .filter(|l| !l.is_empty() && l.len() <= 8 && l.chars().all(|c| c.is_ascii_alphanumeric()));
        let language = chosen.unwrap_or_else(|| edition.clone());
        let mut named = HashMap::new();
        if let Ok(entries) = std::fs::read_dir(&system) {
            for path in entries.flatten().map(|e| e.path()) {
                let is_ini = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("ini"));
                let stem = path.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
                if is_ini && !matches!(stem.as_str(), "default" | "user" | "defuser" | "defunrealed") {
                    if let Ok(i) = Ini::load(&path) {
                        named.insert(stem, i);
                    }
                }
            }
        }
        Config { system, ini, user, language, edition, localized: HashMap::new(), named }
    }

    /// A value from the ini file a class's `config(File)` names. `System` (and no name) is the main
    /// configuration; any other name is that file of the System folder.
    pub fn get_in(&self, file: &str, section: &str, key: &str) -> Option<&str> {
        match file.to_ascii_lowercase().as_str() {
            "" | "none" | "system" | "user" | "default" => self.get(section, key),
            f => self.named.get(f).and_then(|i| i.get(section, key)),
        }
    }

    /// Every value of a repeated key in a class's config file.
    pub fn get_all_in(&self, file: &str, section: &str, key: &str) -> Vec<&str> {
        match file.to_ascii_lowercase().as_str() {
            "" | "none" | "system" | "user" | "default" => self.get_all(section, key),
            f => self.named.get(f).map(|i| i.get_all(section, key)).unwrap_or_default(),
        }
    }

    /// A configuration value: `User.ini` first, then `Default.ini`.
    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.user.get(section, key).or_else(|| self.ini.get(section, key))
    }

    /// Every value of a repeated key (dynamic arrays): `User.ini`'s when it has any, else `Default.ini`'s.
    pub fn get_all(&self, section: &str, key: &str) -> Vec<&str> {
        let user = self.user.get_all(section, key);
        if user.is_empty() { self.ini.get_all(section, key) } else { user }
    }

    /// A localized string from `<package>.<language>`, falling back to `<package>.int` and then to the edition's own
    /// language.
    pub fn localize(&mut self, package: &str, section: &str, key: &str) -> Option<String> {
        let mut langs = vec![self.language.clone()];
        for l in ["int".to_string(), self.edition.clone()] {
            if !langs.contains(&l) {
                langs.push(l);
            }
        }
        for lang in langs {
            let file = format!("{}.{}", package.to_ascii_lowercase(), lang);
            if !self.localized.contains_key(&file) {
                // A package may name a subfolder of System (`Cutscenes0Intro`); each part matches ignoring case.
                let parts: Vec<&str> = file.split(['\\', '/']).filter(|p| !p.is_empty()).collect();
                let mut dir = Some(self.system.clone());
                for (i, part) in parts.iter().enumerate() {
                    let is_last = i + 1 == parts.len();
                    dir = dir.and_then(|d| {
                        std::fs::read_dir(&d).ok()?.flatten().map(|e| e.path()).find(|p| {
                            (p.is_dir() != is_last) && p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(part))
                        })
                    });
                }
                let found = dir;
                self.localized.insert(file.clone(), found.and_then(|p| Ini::load(&p).ok()));
            }
            if let Some(Some(ini)) = self.localized.get(&file)
                && let Some(v) = ini.get(section, key)
            {
                return Some(v.to_string());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A game folder with a Russian edition: `Default.ini` says `Language=Rus`, `HpMenu.rus` has two texts.
    fn install(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("hp3-core-{name}-{}", std::process::id()));
        let system = root.join("System");
        std::fs::create_dir_all(&system).unwrap();
        std::fs::write(system.join("Default.ini"), "[Engine.Engine]\nLanguage=Rus\n").unwrap();
        std::fs::write(system.join("HpMenu.rus"), "[All]\nback=Nazad\nonly_rus=Tolko\n").unwrap();
        std::fs::write(system.join("HpMenu.ukr"), "[All]\nback=Nazad-uk\n").unwrap();
        root
    }

    #[test]
    fn the_installs_language_is_the_default() {
        let root = install("default");
        let mut c = Config::load(&root);
        assert_eq!((c.language.as_str(), c.edition.as_str()), ("rus", "rus"));
        assert_eq!(c.localize("HpMenu", "All", "back").as_deref(), Some("Nazad"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_launchers_choice_wins_and_falls_back_to_the_edition() {
        let root = install("chosen");
        std::fs::write(root.join(".flipendo-language"), "UKR\r\n").unwrap();
        let mut c = Config::load(&root);
        assert_eq!((c.language.as_str(), c.edition.as_str()), ("ukr", "rus"));
        assert_eq!(c.localize("HpMenu", "All", "back").as_deref(), Some("Nazad-uk"));
        // Not in the translation: the edition's text stands in.
        assert_eq!(c.localize("HpMenu", "All", "only_rus").as_deref(), Some("Tolko"));
        assert_eq!(c.localize("HpMenu", "All", "nowhere"), None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_garbled_choice_is_ignored() {
        let root = install("garbled");
        std::fs::write(root.join(".flipendo-language"), "../../etc\n").unwrap();
        assert_eq!(Config::load(&root).language, "rus");
        std::fs::remove_dir_all(root).unwrap();
    }
}