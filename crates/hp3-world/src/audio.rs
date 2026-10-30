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
    },
    Stop { sound: SoundKey, emitter: Option<ObjId> },
    Music { song: String, handle: i32, fade_in: f32, looping: bool, stab: bool },
    StopMusic { handle: i32, fade_out: f32 },
    StopAllMusic { fade_out: f32 },
}

#[derive(Default)]
pub struct Audio {
    pub events: Vec<Event>,
    /// The song last asked for (`FindCurrentMusic`).
    pub current_music: String,
    next_handle: i32,
    durations: HashMap<SoundKey, f32>,
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

fn play(vm: &mut Vm<'_>, this: Option<ObjId>, a: &[Value]) {
    let Some(sound) = a.first().and_then(|v| key_of(vm, v)) else { return };
    let location = this.map(|o| crate::physics::vec(&get(vm, o, "Location"))).unwrap_or(Vec3::ZERO);
    let event = Event::Play {
        sound,
        emitter: this,
        location,
        slot: a.get(1).filter(|v| !matches!(v, Value::Empty)).map(Value::as_byte).unwrap_or(0),
        volume: float_or(a, 2, 1.0),
        no_override: bool_or(a, 3, false),
        radius: float_or(a, 4, 0.0),
        pitch: float_or(a, 5, 1.0),
        attenuate: bool_or(a, 6, true),
        no_3d: bool_or(a, 7, false),
    };
    world(vm).audio.events.push(event);
}

pub fn register(vm: &mut Vm<'_>) {
    // PlaySound / PlayOwnedSound(Sound, slot, Volume, bNoOverride, Radius, Pitch, Attenuate, No3D)
    vm.register("Actor.PlaySound", |vm, this, a| {
        play(vm, this, a);
        Ok(Value::Empty)
    });
    vm.register("Actor.PlayOwnedSound", |vm, this, a| {
        play(vm, this, a);
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
