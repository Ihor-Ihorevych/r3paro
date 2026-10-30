//! Minimal WAV output.

use crate::bink::Pcm;

/// A 16-bit PCM WAV file.
pub fn to_wav(pcm: &Pcm) -> Vec<u8> {
    let data_len = pcm.samples.len() * 2;
    let mut out = Vec::with_capacity(44 + data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(pcm.channels as u16).to_le_bytes());
    out.extend_from_slice(&pcm.sample_rate.to_le_bytes());
    out.extend_from_slice(&(pcm.sample_rate * pcm.channels as u32 * 2).to_le_bytes());
    out.extend_from_slice(&((pcm.channels * 2) as u16).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    for s in &pcm.samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Length in seconds of a PCM WAV file (from its `fmt ` and `data` chunks).
pub fn duration(file: &[u8]) -> Option<f32> {
    if file.len() < 12 || &file[..4] != b"RIFF" || &file[8..12] != b"WAVE" {
        return None;
    }
    let (mut byte_rate, mut at) = (0u32, 12);
    while at + 8 <= file.len() {
        let size = u32::from_le_bytes(file[at + 4..at + 8].try_into().ok()?) as usize;
        match &file[at..at + 4] {
            b"fmt " => byte_rate = u32::from_le_bytes(file.get(at + 16..at + 20)?.try_into().ok()?),
            b"data" if byte_rate > 0 => return Some(size as f32 / byte_rate as f32),
            _ => {}
        }
        at += 8 + size + (size & 1);
    }
    None
}

/// Decodes a 16-bit (or 8-bit) PCM WAV file; other encodings give `None`.
pub fn decode(file: &[u8]) -> Option<Pcm> {
    if file.len() < 12 || &file[..4] != b"RIFF" || &file[8..12] != b"WAVE" {
        return None;
    }
    let (mut at, mut fmt) = (12, None);
    while at + 8 <= file.len() {
        let size = u32::from_le_bytes(file[at + 4..at + 8].try_into().ok()?) as usize;
        let body = file.get(at + 8..(at + 8 + size).min(file.len()))?;
        match &file[at..at + 4] {
            b"fmt " if body.len() >= 16 => {
                let tag = u16::from_le_bytes(body[0..2].try_into().ok()?);
                let channels = u16::from_le_bytes(body[2..4].try_into().ok()?) as usize;
                let rate = u32::from_le_bytes(body[4..8].try_into().ok()?);
                let bits = u16::from_le_bytes(body[14..16].try_into().ok()?);
                fmt = Some((tag, channels, rate, bits));
            }
            b"data" => {
                let (tag, channels, rate, bits) = fmt?;
                if tag != 1 || channels == 0 {
                    return None;
                }
                let samples = match bits {
                    16 => body.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect(),
                    8 => body.iter().map(|&b| ((b as i16) - 128) * 256).collect(),
                    _ => return None,
                };
                return Some(Pcm { sample_rate: rate, channels, samples });
            }
            _ => {}
        }
        at += 8 + size + (size & 1);
    }
    None
}
