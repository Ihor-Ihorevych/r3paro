//! A software mixer: decoded sounds played with pitch, volume, 3D attenuation and panning into 48 kHz stereo
//! floats, plus one music track with fades. It knows nothing about SDL; a platform callback calls `mix`.

use std::sync::Arc;

pub const OUTPUT_RATE: u32 = 48_000;

/// Distance (world units) a sound reaches per unit of its radius, as in Unreal.
const RADIUS_SCALE: f32 = 25.0;
/// A sound played without a radius reaches this many radius units.
const DEFAULT_RADIUS: f32 = 64.0;

/// Decoded audio ready to mix.
#[derive(Clone, Debug)]
pub struct Sound {
    pub rate: u32,
    pub channels: usize,
    pub samples: Vec<i16>,
}

impl Sound {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1)
    }
}

impl From<crate::bink::Pcm> for Sound {
    fn from(p: crate::bink::Pcm) -> Sound {
        Sound { rate: p.sample_rate, channels: p.channels, samples: p.samples }
    }
}

/// What kind of sound a voice is, for the volume categories.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Effect,
    Voice,
    Ambient,
}

/// Where a sound comes from.
#[derive(Clone, Copy, Debug)]
pub enum Placement {
    /// Heard equally on both sides (interface sounds and those marked 2D).
    Flat,
    /// At a world position, falling off to nothing at `reach` units.
    World { position: [f32; 3], reach: f32 },
}

pub struct PlayRequest {
    pub sound: Arc<Sound>,
    pub volume: f32,
    pub pitch: f32,
    pub looping: bool,
    pub placement: Placement,
    pub category: Category,
    /// Voices with the same non-zero owner and slot replace each other unless `no_override`.
    pub owner: u64,
    pub slot: u8,
    pub no_override: bool,
    /// Identifies the sound for `stop`.
    pub tag: u64,
}

struct Voice {
    sound: Arc<Sound>,
    /// Position in the sound's own frames.
    position: f64,
    volume: f32,
    pitch: f32,
    looping: bool,
    placement: Placement,
    category: Category,
    owner: u64,
    slot: u8,
    tag: u64,
}

struct MusicTrack {
    sound: Arc<Sound>,
    position: f64,
    looping: bool,
    handle: i32,
    /// Current level and where it is heading, per output frame.
    level: f32,
    target: f32,
    step: f32,
    /// Stop (drop the track) once faded out.
    stopping: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Listener {
    pub position: [f32; 3],
    /// Unit vector to the listener's right on the ground plane.
    pub right: [f32; 2],
}

pub struct Mixer {
    voices: Vec<Voice>,
    music: Option<MusicTrack>,
    pub listener: Listener,
    pub effect_volume: f32,
    pub voice_volume: f32,
    pub ambient_volume: f32,
    pub music_volume: f32,
}

impl Default for Mixer {
    fn default() -> Mixer {
        Mixer {
            voices: Vec::new(),
            music: None,
            listener: Listener { position: [0.0; 3], right: [0.0, 1.0] },
            effect_volume: 1.0,
            voice_volume: 1.0,
            ambient_volume: 1.0,
            music_volume: 0.7,
        }
    }
}

impl Mixer {
    pub fn play(&mut self, r: PlayRequest) {
        if !r.no_override && r.owner != 0 && r.slot != 0 {
            self.voices.retain(|v| !(v.owner == r.owner && v.slot == r.slot));
        }
        // Many voices at once are not worth mixing: drop the oldest.
        if self.voices.len() >= 48 {
            self.voices.remove(0);
        }
        self.voices.push(Voice {
            sound: r.sound,
            position: 0.0,
            volume: r.volume,
            pitch: r.pitch.max(0.05),
            looping: r.looping,
            placement: r.placement,
            category: r.category,
            owner: r.owner,
            slot: r.slot,
            tag: r.tag,
        });
    }

    /// Stops the voices of one sound (`tag`), those of one owner when `owner` is non-zero.
    pub fn stop(&mut self, tag: u64, owner: u64) {
        self.voices.retain(|v| !(v.tag == tag && (owner == 0 || v.owner == owner)));
    }

    pub fn stop_all(&mut self) {
        self.voices.clear();
    }

    pub fn voice_count(&self) -> usize {
        self.voices.len()
    }

    /// Starts `sound` as the music, fading the previous track out over `fade_in` seconds and the new one in.
    pub fn play_music(&mut self, sound: Arc<Sound>, handle: i32, looping: bool, fade_in: f32) {
        let step = if fade_in > 0.0 { 1.0 / (fade_in * OUTPUT_RATE as f32) } else { 1.0 };
        self.music = Some(MusicTrack {
            sound,
            position: 0.0,
            looping,
            handle,
            level: if fade_in > 0.0 { 0.0 } else { 1.0 },
            target: 1.0,
            step,
            stopping: false,
        });
    }

    /// Fades the music out (`handle` 0 stops whichever plays).
    pub fn stop_music(&mut self, handle: i32, fade_out: f32) {
        if let Some(m) = &mut self.music
            && (handle == 0 || m.handle == handle)
        {
            if fade_out > 0.0 {
                m.target = 0.0;
                m.step = 1.0 / (fade_out * OUTPUT_RATE as f32);
                m.stopping = true;
            } else {
                self.music = None;
            }
        }
    }

    pub fn music_playing(&self) -> bool {
        self.music.is_some()
    }

    /// Mixes `out.len() / 2` stereo frames (interleaved left, right), replacing the buffer's contents.
    pub fn mix(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        let frames = out.len() / 2;
        let listener = self.listener;
        let (effect_volume, voice_volume, ambient_volume) = (self.effect_volume, self.voice_volume, self.ambient_volume);
        for v in &mut self.voices {
            let category_volume = match v.category {
                Category::Voice => voice_volume,
                Category::Ambient => ambient_volume,
                Category::Effect => effect_volume,
            };
            let (gain, pan) = match v.placement {
                Placement::Flat => (v.volume * category_volume, 0.0),
                Placement::World { position, reach } => {
                    let d = [position[0] - listener.position[0], position[1] - listener.position[1], position[2] - listener.position[2]];
                    let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    let fall = (1.0 - dist / reach.max(1.0)).clamp(0.0, 1.0);
                    let flat = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1e-3);
                    // Close sounds are heard centred; farther ones lean toward their side.
                    let side = ((d[0] * listener.right[0] + d[1] * listener.right[1]) / flat).clamp(-1.0, 1.0);
                    let lean = (dist / 200.0).clamp(0.0, 1.0);
                    (v.volume * category_volume * fall, side * lean)
                }
            };
            let (gl, gr) = (gain * (1.0 - pan.max(0.0)), gain * (1.0 + pan.min(0.0)));
            let step = v.sound.rate as f64 / OUTPUT_RATE as f64 * v.pitch as f64;
            let total = v.sound.frames();
            let ch = v.sound.channels.max(1);
            if total == 0 {
                v.position = f64::MAX;
                continue;
            }
            for f in 0..frames {
                if v.position >= total as f64 {
                    if v.looping {
                        v.position -= total as f64;
                    } else {
                        break;
                    }
                }
                let i = v.position as usize;
                let frac = (v.position - i as f64) as f32;
                let j = if i + 1 < total { i + 1 } else if v.looping { 0 } else { i };
                let at = |frame: usize, c: usize| v.sound.samples[frame * ch + c.min(ch - 1)] as f32 / 32768.0;
                let (l0, l1) = (at(i, 0), at(j, 0));
                let (r0, r1) = (at(i, 1), at(j, 1));
                let l = l0 + (l1 - l0) * frac;
                let r = r0 + (r1 - r0) * frac;
                out[f * 2] += l * gl;
                out[f * 2 + 1] += r * gr;
                v.position += step;
            }
        }
        self.voices.retain(|v| v.looping || v.position < v.sound.frames() as f64);
        if let Some(m) = &mut self.music {
            let step = m.sound.rate as f64 / OUTPUT_RATE as f64;
            let total = m.sound.frames();
            let ch = m.sound.channels.max(1);
            let mut ended = total == 0;
            for f in 0..frames {
                if m.level != m.target {
                    m.level = if m.level < m.target { (m.level + m.step).min(m.target) } else { (m.level - m.step).max(m.target) };
                }
                if m.position >= total as f64 {
                    if m.looping {
                        m.position -= total as f64;
                    } else {
                        ended = true;
                        break;
                    }
                }
                let i = m.position as usize;
                let frac = (m.position - i as f64) as f32;
                let j = if i + 1 < total { i + 1 } else if m.looping { 0 } else { i };
                let at = |frame: usize, c: usize| m.sound.samples[frame * ch + c.min(ch - 1)] as f32 / 32768.0;
                let g = m.level * self.music_volume;
                out[f * 2] += (at(i, 0) + (at(j, 0) - at(i, 0)) * frac) * g;
                out[f * 2 + 1] += (at(i, 1) + (at(j, 1) - at(i, 1)) * frac) * g;
                m.position += step;
            }
            if ended || (m.stopping && m.level <= 0.0) {
                self.music = None;
            }
        }
        for s in out.iter_mut() {
            // Soft limiting keeps loud mixes from clipping harshly.
            *s = if s.abs() > 0.9 { s.signum() * (0.9 + 0.1 * ((s.abs() - 0.9) * 10.0).tanh()) } else { *s };
        }
    }
}

/// The listener's right-hand unit vector for a yaw in Unreal rotation units (65536 per turn).
pub fn right_from_yaw(yaw: i32) -> [f32; 2] {
    let a = yaw as f32 * std::f32::consts::TAU / 65536.0;
    // Forward is (cos, sin) in the ground plane; right is forward turned a quarter turn clockwise.
    [a.sin(), -a.cos()]
}

/// The fraction of reach a sound has: used for the radius of a request.
pub fn reach_for(radius: f32) -> f32 {
    (if radius > 0.0 { radius } else { DEFAULT_RADIUS }) * RADIUS_SCALE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(frames: usize) -> Arc<Sound> {
        Arc::new(Sound { rate: OUTPUT_RATE, channels: 1, samples: vec![16384; frames] })
    }

    #[test]
    fn flat_voice_plays_and_ends() {
        let mut m = Mixer::default();
        m.play(PlayRequest {
            sound: tone(100),
            volume: 1.0,
            pitch: 1.0,
            looping: false,
            placement: Placement::Flat,
            category: Category::Effect,
            owner: 0,
            slot: 0,
            no_override: false,
            tag: 1,
        });
        let mut out = vec![0.0; 400];
        m.mix(&mut out);
        assert!((out[0] - 0.5).abs() < 1e-3 && (out[1] - 0.5).abs() < 1e-3);
        assert_eq!(out[250], 0.0);
        assert_eq!(m.voice_count(), 0);
    }

    #[test]
    fn distant_sound_is_quieter_and_beyond_reach_silent() {
        let mut level = |x: f32| {
            let mut m = Mixer::default();
            m.play(PlayRequest {
                sound: tone(1000),
                volume: 1.0,
                pitch: 1.0,
                looping: false,
                placement: Placement::World { position: [x, 0.0, 0.0], reach: 1000.0 },
                category: Category::Effect,
                owner: 0,
                slot: 0,
                no_override: false,
                tag: 1,
            });
            let mut out = vec![0.0; 20];
            m.mix(&mut out);
            out[0].abs() + out[1].abs()
        };
        assert!(level(100.0) > level(600.0));
        assert_eq!(level(1500.0), 0.0);
    }

    #[test]
    fn same_slot_replaces_unless_no_override() {
        let mut m = Mixer::default();
        let req = |no_override| PlayRequest {
            sound: tone(1000),
            volume: 1.0,
            pitch: 1.0,
            looping: false,
            placement: Placement::Flat,
            category: Category::Voice,
            owner: 7,
            slot: 5,
            no_override,
            tag: 1,
        };
        m.play(req(false));
        m.play(req(false));
        assert_eq!(m.voice_count(), 1);
        m.play(req(true));
        assert_eq!(m.voice_count(), 2);
    }

    #[test]
    fn music_fades_in_and_out() {
        let mut m = Mixer::default();
        m.music_volume = 1.0;
        m.play_music(tone(OUTPUT_RATE as usize * 10), 1, true, 0.01);
        let mut out = vec![0.0; 2 * 960];
        m.mix(&mut out);
        assert!(out[0] < 0.1 && out[out.len() - 2] > 0.4);
        m.stop_music(1, 0.01);
        m.mix(&mut out);
        m.mix(&mut out);
        assert!(!m.music_playing());
    }
}
