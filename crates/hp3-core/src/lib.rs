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
    /// Localization file extension, e.g. `int` or `rus`.
    pub language: String,
    localized: HashMap<String, Option<Ini>>,
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
        let language = ini.get("Engine.Engine", "Language").unwrap_or("int").to_ascii_lowercase();
        Config { system, ini, user, language, localized: HashMap::new() }
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

    /// A localized string from `<package>.<language>` (falling back to `<package>.int`).
    pub fn localize(&mut self, package: &str, section: &str, key: &str) -> Option<String> {
        for lang in [self.language.clone(), "int".to_string()] {
            let file = format!("{}.{}", package.to_ascii_lowercase(), lang);
            if !self.localized.contains_key(&file) {
                let found = std::fs::read_dir(&self.system).ok().and_then(|entries| {
                    entries
                        .flatten()
                        .map(|e| e.path())
                        .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(&file)))
                });
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
