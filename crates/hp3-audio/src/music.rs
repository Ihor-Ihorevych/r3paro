//! Music: Ogg Vorbis tracks from the game's `Music` folder, and matching a song name to a file.

use std::path::{Path, PathBuf};

use lewton::inside_ogg::OggStreamReader;

use crate::mixer::Sound;

/// Decodes a whole Ogg Vorbis file to 16-bit samples.
pub fn decode_ogg(data: &[u8]) -> Result<Sound, String> {
    let mut reader = OggStreamReader::new(std::io::Cursor::new(data)).map_err(|e| format!("ogg: {e}"))?;
    let rate = reader.ident_hdr.audio_sample_rate;
    let channels = reader.ident_hdr.audio_channels as usize;
    let mut samples = Vec::new();
    while let Some(packet) = reader.read_dec_packet_itl().map_err(|e| format!("ogg: {e}"))? {
        samples.extend_from_slice(&packet);
    }
    Ok(Sound { rate, channels, samples })
}

/// The file in `dir` a song name refers to: an exact name, else one ending in `_<song>`, else one containing
/// it (ignoring case and extension); shorter names win ties.
pub fn find_song(dir: &Path, song: &str) -> Option<PathBuf> {
    let want = song.trim().to_ascii_lowercase();
    let want = want.strip_suffix(".ogg").unwrap_or(&want).to_string();
    if want.is_empty() {
        return None;
    }
    let mut best: Option<(u8, usize, PathBuf)> = None;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let path = e.path();
        if path.extension().is_none_or(|x| !x.eq_ignore_ascii_case("ogg")) {
            continue;
        }
        let stem = path.file_stem()?.to_string_lossy().to_ascii_lowercase();
        let rank = if stem == want {
            0
        } else if stem.ends_with(&format!("_{want}")) {
            1
        } else if stem.contains(&want) {
            2
        } else {
            continue;
        };
        if best.as_ref().is_none_or(|b| (rank, stem.len()) < (b.0, b.1)) {
            best = Some((rank, stem.len(), path));
        }
    }
    best.map(|b| b.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn song_names_match_by_rank() {
        let dir = std::env::temp_dir().join(format!("hp3_music_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for n in ["CUE5_hogwarts_intro.ogg", "CUE5_hogwarts_intro_11_sec.ogg", "Dementor_Action.ogg", "action.ogg"] {
            std::fs::write(dir.join(n), b"").unwrap();
        }
        let name = |s: &str| find_song(&dir, s).map(|p| p.file_name().unwrap().to_string_lossy().into_owned());
        assert_eq!(name("hogwarts_intro").as_deref(), Some("CUE5_hogwarts_intro.ogg"));
        assert_eq!(name("Action").as_deref(), Some("action.ogg"));
        assert_eq!(name("dementor_action").as_deref(), Some("Dementor_Action.ogg"));
        assert_eq!(name("nothing"), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
