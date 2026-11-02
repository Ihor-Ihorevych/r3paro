//! `hp3-run lang <HP3 folder> <language> [folder]`: checks a translation of the game's interface texts against the
//! edition's own (`HpMenu`, `SaveTips` in the edition's language): every key is there and nothing else, the keys that
//! are not text (`Picture0`, `Count`) are unchanged, nothing is left in the edition's language, and every character
//! can be drawn (the system fonts' characters; plain ASCII for English, which keeps the bitmap fonts).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use hp3_core::Ini;

/// The packages the interface translations cover.
const PACKAGES: [&str; 2] = ["HpMenu", "SaveTips"];

/// A file of `dir` by name, ignoring case.
fn find(dir: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name)))
}

/// Whether a key holds text to translate (`SaveTips` also has picture names and counts).
fn is_text(package: &str, key: &str) -> bool {
    package != "SaveTips" || key.to_ascii_lowercase().starts_with("line")
}

pub fn check(root: &Path, language: &str, from: Option<&Path>) -> Result<ExitCode, Box<dyn std::error::Error>> {
    let config = hp3_core::Config::load(root);
    let edition = config.edition.clone();
    let language = language.to_ascii_lowercase();
    let dir = from.map(Path::to_path_buf).unwrap_or_else(|| config.system.clone());
    let allowed: std::collections::HashSet<char> = if language == "int" {
        (0x20u8..0x7f).map(char::from).collect()
    } else {
        hp3_world::fonts::charset(&language).into_iter().collect()
    };
    let mut problems = 0usize;
    let mut report = |text: String| {
        problems += 1;
        if problems <= 60 {
            println!("  {text}");
        }
    };
    for package in PACKAGES {
        let base_path = find(&config.system, &format!("{package}.{edition}")).ok_or(format!("no {package}.{edition} in {}", config.system.display()))?;
        let Some(path) = find(&dir, &format!("{package}.{language}")) else {
            report(format!("{package}.{language}: missing"));
            continue;
        };
        let (base, tr) = (Ini::load(&base_path)?, Ini::load(&path)?);
        let mut keys = 0;
        for section in base.section_names() {
            for (key, original) in base.entries(section) {
                keys += 1;
                let Some(value) = tr.get(section, key) else {
                    report(format!("{package} [{section}] {key}: missing"));
                    continue;
                };
                if !is_text(package, key) {
                    if value != original {
                        report(format!("{package} [{section}] {key}: is {value:?}, must stay {original:?}"));
                    }
                    continue;
                }
                if value.trim().is_empty() && !original.trim().is_empty() {
                    report(format!("{package} [{section}] {key}: empty"));
                }
                // Left in the edition's language: Cyrillic in a language that does not use it, or the very same text.
                let cyrillic = |s: &str| s.chars().any(|c| ('\u{400}'..'\u{500}').contains(&c));
                // Ukrainian and Belarusian share words with Russian, so they are caught by letters their alphabets lack.
                let russian_only = match language.as_str() {
                    "ukr" => value.chars().any(|c| "ыэъёЫЭЪЁ".contains(c)),
                    "bel" => value.chars().any(|c| "иИщЩъЪ".contains(c)),
                    _ => false,
                };
                if (cyrillic(value) && matches!(language.as_str(), "int" | "geo" | "lav")) || russian_only {
                    report(format!("{package} [{section}] {key}: not translated: {value}"));
                }
                let bad: String = value.chars().filter(|c| !allowed.contains(c)).collect();
                if !bad.is_empty() {
                    report(format!("{package} [{section}] {key}: characters the font lacks: {bad:?}"));
                }
            }
        }
        for section in tr.section_names() {
            for (key, _) in tr.entries(section) {
                if base.get(section, key).is_none() {
                    report(format!("{package} [{section}] {key}: not in the edition's file"));
                }
            }
        }
        println!("{package}.{language}: {keys} keys checked against {}", base_path.display());
    }
    if problems > 60 {
        println!("  ... {} more", problems - 60);
    }
    println!("{problems} problems");
    Ok(if problems == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}
