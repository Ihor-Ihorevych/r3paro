//! Sound output: the world's sound and music requests turned into mixer voices, played through an SDL3 stream.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use hp3_audio::mixer::{self, Category, Mixer, Placement, PlayRequest, Sound};
use hp3_script::Vm;
use hp3_world::audio::{Event, SoundKey};
use sdl3::audio::{AudioCallback, AudioFormat, AudioSpec, AudioStream, AudioStreamWithCallback};

struct Callback {
    mixer: Arc<Mutex<Mixer>>,
    buffer: Vec<f32>,
    /// `HP3_AUDIO_DEBUG`: a level meter printed once per second of output.
    meter: Option<(f64, usize)>,
}

impl AudioCallback<f32> for Callback {
    fn callback(&mut self, stream: &mut AudioStream, requested: i32) {
        // `requested` counts samples over both channels.
        let n = (requested.max(0) as usize) & !1;
        if n == 0 {
            return;
        }
        self.buffer.resize(n, 0.0);
        let mut voices = 0;
        match self.mixer.lock() {
            Ok(mut m) => {
                m.mix(&mut self.buffer);
                voices = m.voice_count();
            }
            Err(_) => self.buffer.fill(0.0),
        }
        if let Some((sum, frames)) = &mut self.meter {
            *sum += self.buffer.iter().map(|&x| (x as f64).powi(2)).sum::<f64>();
            *frames += n / 2;
            if *frames >= mixer::OUTPUT_RATE as usize {
                eprintln!("audio meter: rms {:.4} ({} voices)", (*sum / (*frames * 2) as f64).sqrt(), voices);
                (*sum, *frames) = (0.0, 0);
            }
        }
        let _ = stream.put_data_f32(&self.buffer);
    }
}

pub struct AudioSystem {
    mixer: Arc<Mutex<Mixer>>,
    _stream: AudioStreamWithCallback<Callback>,
    cache: HashMap<SoundKey, Option<Arc<Sound>>>,
    music_dir: PathBuf,
    /// Ambient sounds playing: the actor's owner id and the sound's tag.
    ambient: HashMap<u64, u64>,
    frame: u32,
    /// The game's `AmbientVolume` and `SoundVolume` settings, which scale every ambient sound.
    ambient_setting: f32,
    sound_setting: f32,
    /// `logs/audio.log`: every sound asked for and stopped, ambient sounds and music, and a count of the voices.
    log: Option<std::io::BufWriter<std::fs::File>>,
    started: std::time::Instant,
    /// Plays in the last second, by sound and asker, to point at a sound asked for over and over.
    repeats: HashMap<String, u32>,
}

/// Slots of `PlaySound` (`ESoundSlot`) that matter here.
const SLOT_TALK: u8 = 5;
const SLOT_INTERFACE: u8 = 6;
const SLOT_AMBIENT: u8 = 4;

fn tag_of(key: &SoundKey) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    h.finish()
}

impl AudioSystem {
    /// Opens the default output; `None` when there is no audio device (the game then runs silent).
    pub fn new(sdl: &sdl3::Sdl, root: &Path) -> Option<AudioSystem> {
        let audio = sdl.audio().ok()?;
        let spec = AudioSpec { freq: Some(mixer::OUTPUT_RATE as i32), channels: Some(2), format: Some(AudioFormat::f32_sys()) };
        // Volumes from the game's own settings (`[ALAudio.ALAudioSubsystem]`).
        let config = hp3_core::Config::load(root);
        let setting = |key: &str, default: f32| {
            config.get("ALAudio.ALAudioSubsystem", key).and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(default)
        };
        let mixer = Arc::new(Mutex::new(Mixer::default()));
        if let Ok(mut m) = mixer.lock() {
            m.music_volume = setting("MusicVolume", 0.5);
            m.effect_volume = setting("SoundVolume", 1.0);
            m.voice_volume = setting("VoiceVolume", 1.0);
            // Ambient sounds carry the `AmbientVolume` and `SoundVolume` settings in their own volume.
            m.ambient_volume = 1.0;
            m.rolloff = setting("Rolloff", 1.0).max(0.0);
        }
        let stream = audio.open_playback_stream(&spec, Callback { mixer: mixer.clone(), buffer: Vec::new(), meter: std::env::var_os("HP3_AUDIO_DEBUG").map(|_| (0.0, 0)) }).ok()?;
        stream.resume().ok()?;
        let music_dir = ["Music", "music"].iter().map(|d| root.join(d)).find(|p| p.is_dir()).unwrap_or_else(|| root.join("Music"));
        Some(AudioSystem { mixer, _stream: stream, cache: HashMap::new(), music_dir, ambient: HashMap::new(),
            frame: 0,
            ambient_setting: setting("AmbientVolume", 1.0),
            sound_setting: setting("SoundVolume", 1.0),
            log: std::fs::create_dir_all(root.join("logs"))
                .ok()
                .and_then(|_| std::fs::File::create(root.join("logs").join("audio.log")).ok())
                .map(std::io::BufWriter::new),
            started: std::time::Instant::now(),
            repeats: HashMap::new(),
        })
    }

    fn sound(&mut self, vm: &mut Vm<'_>, key: &SoundKey) -> Option<Arc<Sound>> {
        if let Some(s) = self.cache.get(key) {
            return s.clone();
        }
        let decoded = hp3_world::audio::sound_file(vm, key).and_then(|(ty, data)| match ty.as_str() {
            "bik" => hp3_audio::bink::decode(&data).ok().map(Sound::from),
            "wav" => hp3_audio::wav::decode(&data).map(Sound::from),
            _ => None,
        });
        let decoded = decoded.map(Arc::new);
        self.cache.insert(key.clone(), decoded.clone());
        decoded
    }

    fn music(&mut self, song: &str) -> Option<Arc<Sound>> {
        let path = hp3_audio::music::find_song(&self.music_dir, song)?;
        let data = std::fs::read(&path).ok()?;
        match hp3_audio::music::decode_ogg(&data) {
            Ok(s) => Some(Arc::new(s)),
            Err(e) => {
                eprintln!("music {}: {e}", path.display());
                None
            }
        }
    }

    fn note(&mut self, line: String) {
        use std::io::Write;
        let t = self.started.elapsed().as_secs_f32();
        if let Some(log) = &mut self.log {
            let _ = writeln!(log, "[{t:8.2}] {line}");
        }
    }

    /// Applies what the world asked to hear since the last call, and places the listener.
    pub fn update(&mut self, vm: &mut Vm<'_>, position: glam::Vec3, yaw: i32) {
        let events = hp3_world::audio::drain(vm);
        for e in events {
            match e {
                Event::Play { sound, emitter, location, slot, volume, pitch, radius, no_override, attenuate, no_3d, who } => {
                    let name = hp3_world::audio::sound_name(vm, &sound);
                    let loaded = self.sound(vm, &sound);
                    let length = match &loaded {
                        Some(s) => format!(" ({:.2}s)", s.frames() as f32 / s.rate as f32),
                        None => " (not decoded)".to_string(),
                    };
                    self.note(format!(
                        "PLAY {name} by {who} slot {slot} volume {volume:.2} radius {radius:.0} pitch {pitch:.2} at {location} no_override {no_override} attenuate {attenuate} no3d {no_3d}{length}"
                    ));
                    *self.repeats.entry(format!("{name} by {who}")).or_default() += 1;
                    let Some(s) = loaded else { continue };
                    let flat = no_3d || slot == SLOT_INTERFACE;
                    let placement = if flat {
                        Placement::Flat
                    } else {
                        Placement::World { position: location.to_array(), radius: mixer::radius_for(radius) }
                    };
                    let req = PlayRequest {
                        sound: s,
                        volume,
                        pitch,
                        looping: false,
                        placement,
                        category: if slot == SLOT_TALK { Category::Voice } else { Category::Effect },
                        owner: emitter.map_or(0, |o| o.0 as u64 + 1),
                        slot,
                        no_override,
                        tag: tag_of(&sound),
                    };
                    if let Ok(mut m) = self.mixer.lock() {
                        m.play(req);
                    }
                }
                Event::Stop { sound, emitter } => {
                    let name = hp3_world::audio::sound_name(vm, &sound);
                    self.note(format!("STOP {name} emitter {emitter:?}"));
                    if let Ok(mut m) = self.mixer.lock() {
                        m.stop(tag_of(&sound), emitter.map_or(0, |o| o.0 as u64 + 1));
                    }
                }
                Event::Music { song, handle, fade_in, looping, stab } => {
                    self.note(format!("MUSIC {song} handle {handle} fade {fade_in} loop {looping} stab {stab}"));
                    if let Some(track) = self.music(&song)
                        && let Ok(mut m) = self.mixer.lock()
                    {
                        m.play_music(track, handle, looping, fade_in, stab);
                    }
                }
                Event::StopMusic { handle, fade_out } => {
                    self.note(format!("STOPMUSIC handle {handle} fade {fade_out}"));
                    if let Ok(mut m) = self.mixer.lock() {
                        m.stop_music(handle, fade_out);
                    }
                }
                Event::StopAllMusic { fade_out } => {
                    self.note(format!("STOPALLMUSIC fade {fade_out}"));
                    if let Ok(mut m) = self.mixer.lock() {
                        m.stop_music(0, fade_out);
                    }
                }
            }
        }
        if let Ok(mut m) = self.mixer.lock() {
            m.listener = mixer::Listener { position: position.to_array(), right: mixer::right_from_yaw(yaw) };
        }
        self.frame += 1;
        if self.frame % 15 == 1 {
            self.sync_ambient(vm, position);
        }
        // Once a second: how many voices play, and any sound asked for more than a few times in that second.
        if self.frame % 60 == 0 {
            let voices = self.mixer.lock().map(|m| m.voice_count()).unwrap_or(0);
            let busy: Vec<String> = self.repeats.iter().filter(|(_, n)| **n >= 4).map(|(k, n)| format!("{k} x{n}")).collect();
            let repeated = if busy.is_empty() { String::new() } else { format!(" REPEATED: {}", busy.join("; ")) };
            self.note(format!("VOICES {voices}{repeated}"));
            self.repeats.clear();
            if let Some(log) = &mut self.log {
                use std::io::Write;
                let _ = log.flush();
            }
        }
    }

    /// Starts the ambient sounds whose actors the listener is now within reach of, and stops those left behind.
    fn sync_ambient(&mut self, vm: &mut Vm<'_>, listener: glam::Vec3) {
        let mut wanted: HashMap<u64, u64> = HashMap::new();
        for a in hp3_world::audio::ambient_sounds(vm) {
            if a.location.distance(listener) >= mixer::audible_distance(a.radius) {
                continue;
            }
            let owner = a.actor.0 as u64 + 1;
            let tag = tag_of(&a.sound);
            wanted.insert(owner, tag);
            if self.ambient.get(&owner) == Some(&tag) {
                continue;
            }
            let Some(s) = self.sound(vm, &a.sound) else { continue };
            let line = format!(
                "AMBIENT start {} on {} radius {} volume {:.2} at {}",
                hp3_world::audio::sound_name(vm, &a.sound),
                hp3_script::natives::object_path(vm, a.actor),
                a.radius,
                a.volume,
                a.location
            );
            self.note(line);
            if let Ok(mut m) = self.mixer.lock() {
                if let Some(old) = self.ambient.get(&owner) {
                    m.stop(*old, owner);
                }
                m.play(PlayRequest {
                    sound: s,
                    volume: a.volume * if a.full_volume { 1.0 } else { self.ambient_setting } * self.sound_setting,
                    pitch: a.pitch,
                    looping: true,
                    placement: Placement::World { position: a.location.to_array(), radius: mixer::radius_for(a.radius) },
                    category: Category::Ambient,
                    owner,
                    slot: SLOT_AMBIENT,
                    no_override: false,
                    tag,
                });
            }
        }
        if let Ok(mut m) = self.mixer.lock() {
            for (owner, tag) in &self.ambient {
                if wanted.get(owner) != Some(tag) {
                    m.stop(*tag, *owner);
                }
            }
        }
        self.ambient = wanted;
    }

    /// Silences everything, music included (on a level change).
    pub fn stop_all(&mut self) {
        self.ambient.clear();
        if let Ok(mut m) = self.mixer.lock() {
            m.stop_everything();
        }
    }
}
