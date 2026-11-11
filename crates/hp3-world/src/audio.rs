//! Sound natives: what the game asks to hear is queued as events for the platform's mixer (headless runs
//! just drop them), and sound lengths come from the sounds' own headers.

use std::collections::HashMap;

use glam::Vec3;
use hp3_script::{ObjId, Value, Vm};

use crate::{get, world};

/// A sound object: its package and export index.
pub type SoundKey = (String, usize);

#[derive(Clone, Debug)]
pub enum Event {
    Play {
        sound: SoundKey,
        emitter: Option<ObjId>,
        location: Vec3,
        slot: u8,
        volume: f32,
        pitch: f32,
        radius: f32,
        no_override: bool,
        attenuate: bool,
        no_3d: bool,
        /// Who asked, for the sound log: the actor, its class and its state.
        who: String,
    },
    Stop { sound: SoundKey, emitter: Option<ObjId> },
    Music { song: String, handle: i32, fade_in: f32, looping: bool, stab: bool },
    StopMusic { handle: i32, fade_out: f32 },
    StopAllMusic { fade_out: f32 },
    /// The `stopsounds` console command (a skipped cutscene runs it): every sound effect stops, music goes on.
    StopAllSounds,
}

#[derive(Default)]
pub struct Audio {
    pub events: Vec<Event>,
    /// The song last asked for (`FindCurrentMusic`).
    pub current_music: String,
    next_handle: i32,
    durations: HashMap<SoundKey, f32>,
}

/// A sound's name for logs: its package path.
pub fn sound_name(vm: &Vm<'_>, key: &SoundKey) -> String {
    match vm.db.set.get(&key.0) {
        Ok(p) => p.ref_path(hp3_package::ObjRef::from_export(key.1)),
        Err(_) => format!("{}#{}", key.0, key.1),
    }
}

/// Takes the events queued since the last call.
pub fn drain(vm: &mut Vm<'_>) -> Vec<Event> {
    std::mem::take(&mut world(vm).audio.events)
}

fn key_of(vm: &Vm<'_>, v: &Value) -> Option<SoundKey> {
    let o = v.as_obj()?;
    vm.db.objects.get(o).key.as_ref().map(|(p, i)| (p.to_string(), *i))
}

/// A sound's embedded file type (`bik` or `wav`) and bytes.
pub fn sound_file(vm: &mut Vm<'_>, key: &SoundKey) -> Option<(String, Vec<u8>)> {
    let pkg = vm.db.set.get(&key.0).ok()?;
    let chain = vm.db.set.export_chain(&pkg, key.1);
    let hp3_assets::Asset::Sound(s) = crate::native_asset(&pkg, key.1, &chain)? else { return None };
    Some((s.file_type.clone(), pkg.data.get(s.data_offset..s.data_offset + s.data_len)?.to_vec()))
}

/// A float argument given by the caller, else `default`.
fn float_or(a: &[Value], i: usize, default: f32) -> f32 {
    match a.get(i) {
        None | Some(Value::Empty) => default,
        Some(v) => v.as_float(),
    }
}

fn bool_or(a: &[Value], i: usize, default: bool) -> bool {
    match a.get(i) {
        None | Some(Value::Empty) => default,
        Some(v) => v.as_bool(),
    }
}

/// The slot a sound takes when the caller names none (`SLOT_Misc`).
const SLOT_MISC: u8 = 1;

/// A sound asked for by `PlaySound` or `PlayOwnedSound`: `(Sound, Slot, Volume, bNoOverride, Radius, Pitch,
/// Attenuate, No3D)`. What the caller leaves out comes from the emitter: its `TransientSoundVolume` and
/// `TransientSoundRadius`; the sound is quieter by the instigator's `SoundDampening` when the emitter is the
/// instigator or its owner. `PlaySound` attenuates by default, `PlayOwnedSound` does not.
fn play(vm: &mut Vm<'_>, this: Option<ObjId>, a: &[Value], attenuate_by_default: bool) {
    let Some(sound) = a.first().and_then(|v| key_of(vm, v)) else { return };
    let Some(emitter) = this else { return };
    let location = crate::physics::vec(&get(vm, emitter, "Location"));
    let mut volume = float_or(a, 2, get(vm, emitter, "TransientSoundVolume").as_float());
    let mut radius = float_or(a, 4, get(vm, emitter, "TransientSoundRadius").as_float());
    if radius <= 0.0 {
        // A radius of 0 asks for the sound's own; HP3's sounds do not carry one we can read, so the actor's default.
        radius = get(vm, emitter, "TransientSoundRadius").as_float();
    }
    if let Some(instigator) = get(vm, emitter, "Instigator").as_obj()
        && (instigator == emitter || get(vm, emitter, "Owner").as_obj() == Some(instigator))
    {
        let damp = get(vm, instigator, "SoundDampening").as_float();
        if damp > 0.0 {
            volume *= damp;
        }
    }
    let who = {
        let path = hp3_script::natives::object_path(vm, emitter);
        let class = vm.db.objects.get(emitter).class.map(|c| vm.db.names.get(vm.db.get(c).name).to_string()).unwrap_or_default();
        let state = vm.state_of(emitter).map(|s| vm.db.names.get(vm.db.get(s).name).to_string()).unwrap_or_default();
        format!("{path} ({class}, state {state})")
    };
    // Volumes arrive in a 0..2 range and are halved before they are played. An unattenuated sound plays flat
    // only when its maker belongs to what the player is viewing (or it is marked No3D); any other still has a
    // place in the world and fades with distance.
    let attenuate = bool_or(a, 6, attenuate_by_default);
    let mut no_3d = bool_or(a, 7, false);
    if !attenuate && !no_3d {
        let view = world(vm).player.and_then(|pc| get(vm, pc, "ViewTarget").as_obj().or_else(|| get(vm, pc, "Pawn").as_obj()));
        let mut at = Some(emitter);
        for _ in 0..8 {
            match at {
                Some(o) if Some(o) == view => {
                    no_3d = true;
                    break;
                }
                Some(o) => at = get(vm, o, "Owner").as_obj(),
                None => break,
            }
        }
    }
    let volume = volume * 0.5;
    let event = Event::Play {
        who,
        sound,
        emitter: Some(emitter),
        location,
        slot: a.get(1).filter(|v| !matches!(v, Value::Empty)).map(Value::as_byte).unwrap_or(SLOT_MISC),
        volume,
        no_override: bool_or(a, 3, false),
        radius,
        pitch: float_or(a, 5, 1.0),
        attenuate,
        no_3d,
    };
    world(vm).audio.events.push(event);
    // A face envelope on the sound moves the speaker's mouth.
    if let Some(key) = a.first().and_then(|v| key_of(vm, v)) {
        crate::lipsync::start(vm, emitter, &key);
    }
}

/// Starts a song from the engine's own side (`Actor.PlayMusic` is the script's way in); the handle stops it
/// again. Used by the Quidditch match, which has no script to ask for its music.
pub fn play_music(vm: &mut Vm<'_>, song: &str, fade_in: f32, looping: bool) -> i32 {
    let w = world(vm);
    w.audio.next_handle += 1;
    let handle = w.audio.next_handle;
    w.audio.current_music = song.to_string();
    w.audio.events.push(Event::Music { song: song.to_string(), handle, fade_in, looping, stab: false });
    handle
}

/// Stops a song started by `play_music`.
pub fn stop_music(vm: &mut Vm<'_>, handle: i32, fade_out: f32) {
    let w = world(vm);
    w.audio.current_music.clear();
    w.audio.events.push(Event::StopMusic { handle, fade_out });
}

pub fn register(vm: &mut Vm<'_>) {
    // PlaySound / PlayOwnedSound(Sound, slot, Volume, bNoOverride, Radius, Pitch, Attenuate, No3D)
    vm.register("Actor.PlaySound", |vm, this, a| {
        play(vm, this, a, true);
        Ok(Value::Empty)
    });
    vm.register("Actor.PlayOwnedSound", |vm, this, a| {
        play(vm, this, a, false);
        Ok(Value::Empty)
    });
    vm.register("Actor.StopSound", |vm, this, a| {
        if let Some(sound) = a.first().and_then(|v| key_of(vm, v)) {
            world(vm).audio.events.push(Event::Stop { sound, emitter: this });
        }
        Ok(Value::Empty)
    });
    // GetSoundDuration(Sound): seconds.
    vm.register("Actor.GetSoundDuration", |vm, _this, a| {
        let Some(key) = a.first().and_then(|v| key_of(vm, v)) else { return Ok(Value::Float(0.0)) };
        if let Some(d) = world(vm).audio.durations.get(&key) {
            return Ok(Value::Float(*d));
        }
        let seconds = sound_file(vm, &key)
            .map(|(ty, data)| match ty.as_str() {
                "bik" => hp3_audio::bink::duration(&data).unwrap_or(0.0),
                "wav" => hp3_audio::wav::duration(&data).unwrap_or(0.0),
                _ => 0.0,
            })
            .unwrap_or(0.0);
        world(vm).audio.durations.insert(key, seconds);
        Ok(Value::Float(seconds))
    });
    // PlayMusic(string Song, float FadeInTime, bool bLoop, bool bStab): a handle for StopMusic.
    vm.register("Actor.PlayMusic", |vm, _this, a| {
        let song = match a.first() {
            Some(Value::Str(s)) => s.clone(),
            _ => String::new(),
        };
        let fade_in = float_or(a, 1, 0.0);
        let looping = bool_or(a, 2, true);
        let stab = bool_or(a, 3, false);
        let w = world(vm);
        w.audio.next_handle += 1;
        let handle = w.audio.next_handle;
        w.audio.current_music = song.clone();
        w.audio.events.push(Event::Music { song, handle, fade_in, looping, stab });
        Ok(Value::Int(handle))
    });
    // StopMusic(int SongHandle, float FadeOutTime)
    vm.register("Actor.StopMusic", |vm, _this, a| {
        let handle = a.first().map(Value::as_int).unwrap_or(0);
        let fade_out = float_or(a, 1, 0.0);
        world(vm).audio.events.push(Event::StopMusic { handle, fade_out });
        Ok(Value::Empty)
    });
    vm.register("Actor.StopAllMusic", |vm, _this, a| {
        let fade_out = float_or(a, 0, 0.0);
        let w = world(vm);
        w.audio.current_music.clear();
        w.audio.events.push(Event::StopAllMusic { fade_out });
        Ok(Value::Empty)
    });
    // FindCurrentMusic(out string CurrentMusic)
    vm.register("Actor.FindCurrentMusic", |vm, _this, a| {
        let song = world(vm).audio.current_music.clone();
        if let Some(slot) = a.get_mut(0) {
            *slot = Value::Str(song);
        }
        Ok(Value::Empty)
    });
}

/// An actor's looping ambient sound.
#[derive(Clone, Debug)]
pub struct Ambient {
    pub actor: ObjId,
    pub sound: SoundKey,
    pub location: Vec3,
    /// The actor's own loudness (`SoundVolume` as a fraction of the 0..2 range, scaled by its light's brightness
    /// when it has one), before the game's `AmbientVolume` and `SoundVolume` settings.
    pub volume: f32,
    /// `bFullVolume`: the `AmbientVolume` setting does not apply.
    pub full_volume: bool,
    pub pitch: f32,
    /// In the actor's `SoundRadius` units.
    pub radius: f32,
}

/// Every live actor that has an `AmbientSound`.
pub fn ambient_sounds(vm: &mut Vm<'_>) -> Vec<Ambient> {
    let actors = world(vm).actors.clone();
    let mut out = Vec::new();
    for a in actors {
        if vm.db.objects.get(a).deleted {
            continue;
        }
        let v = get(vm, a, "AmbientSound");
        let Some(sound) = key_of(vm, &v) else { continue };
        let mut volume = get(vm, a, "SoundVolume").as_float() / 255.0 / 2.0;
        if get(vm, a, "LightType").as_byte() != 0 {
            volume *= get(vm, a, "LightBrightness").as_float() / 255.0;
        }
        let pitch = (get(vm, a, "SoundPitch").as_float() / 64.0).clamp(0.5, 2.0);
        out.push(Ambient {
            actor: a,
            sound,
            location: crate::physics::vec(&get(vm, a, "Location")),
            volume,
            full_volume: get(vm, a, "bFullVolume").as_bool(),
            pitch: if get(vm, a, "SoundPitch").as_float() > 0.0 { pitch } else { 1.0 },
            radius: get(vm, a, "SoundRadius").as_float(),
        });
    }
    out
}
