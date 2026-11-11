//! The Snitch's ring trail on screen: the actors that show each flight ring, the sound of flying through one,
//! and the bonus it is worth. What it decides comes out of the pure `trail` module; what is here is the
//! spawning, the `Location` writes, the sounds and the log lines.
//!
//! This touches the VM, like `glue`, `presentation` and `ball`, but it is called only from `glue`.
//!
//! **These are the real Buckbeak flight rings.** `HGame.HippoRing` is the ring the hippogriff ride is flown
//! through (about 110 of them in `HP3_COMC`, more in `HP3_Adv6Paddock`). It carries no mesh at all: the whole
//! ring is particles. Read out of the package and off a render of `HP3_COMC`:
//!
//! * the actor's `TurnOn(path)` spawns its `ringFXClass`, which is `HGame.HippoRingEmitter`, at its own
//!   `Location`, then hands the emitter `SetRadius(CollisionWidth)` and `SetColor(ringColorStart,
//!   ringColorEnd)` and goes to `StateActive`. Nothing is drawn until `TurnOn` has run.
//! * `HippoRingEmitter` is three `SpriteEmitter`s of 32 particles each (96 in all), textured
//!   `HP3_FX.Bats.BatFlap1_A00` / `2` / `3` -- the three flap frames of the bat sprite, which is what makes a
//!   ring shimmer rather than sit still. Alpha-blended, 18..28 uu a sprite, 2 s each and respawned, drifting
//!   only +-8 uu/s, opacity 0.9, `CoordinateSystem` **relative** (so the sprites ride the actor).
//! * the circle comes from `StartLocationShape` = polar with `StartLocationPolarRange`, whose **radius is 0
//!   in the defaults**: `SetRadius` is what opens it out, and the actor's rotation turns the circle so it
//!   stands across the flight. The retail rings are 192 uu (`CollisionWidth`) and white (255, 255, 255).
//!
//! **Why the first attempt got a swarm of bats.** It spawned a bare `HGame.HippoRingEmitter` and left it
//! alone. With no `HippoRing` to call `SetRadius`, the polar radius stays 0, so all 96 sprites are born on
//! top of each other at the actor and the only thing that moves them is the +-8 uu/s drift: a cloud, not a
//! ring. The bats are not the mistake -- the retail ring really is made of bat flappers -- the missing
//! `SetRadius` (and a rotation) was.
//!
//! So a trail ring here is a real `HGame.HippoRing`, spawned by us and turned on by its own script, one per
//! ring slot (`TrailTuning::max_rings`, six). Re-aiming a slot is a `Location` + `Rotation` write and another
//! `TurnOn`, which destroys the slot's old emitter and spawns a fresh one in the new place: about one spawn
//! a second over the whole trail, which is what the retail ride does when a path is switched. `stop` kills
//! the emitters the rings are holding and `glue::stop` destroys the rings themselves.
//!
//! **What it costs**: six rings x 96 sprites = 576 particles at most, all of them one-quad sprites. That is
//! the number to cut first if the phone's frame goes over (fewer rings, or a smaller `max_rings`).

use glam::Vec3;

use hp3_script::{Name, ObjId, Value, Vm};

use super::glue::QuidditchMatch;
use super::rules::MatchEvent;
use super::trail::{self, RingTrail, TrailEvent};

/// The retail Buckbeak flight ring. Its own script builds the sprite ring; we only place it, size it and
/// switch it on.
const RING_CLASS: &str = "HGame.HippoRing";
/// The sound of going through one: HP1 plays one of fifteen `Q_Through_Hoop` samples, which HP3 does not ship
/// (`soundeffects.Quidditch` has no hoop sound at all), so these are its broom swishes and whooshes, picked at
/// random the same way.
const PASS_SOUNDS: [&str; 7] = [
    "soundeffects.Quidditch.Q_BRM_Swish_04",
    "soundeffects.Quidditch.Q_BRM_Swish_06",
    "soundeffects.Quidditch.broomwhoosh01",
    "soundeffects.Quidditch.broomwhoosh02",
    "soundeffects.Quidditch.broomwhoosh03",
    "soundeffects.Quidditch.broomwhoosh04",
    "soundeffects.Quidditch.broomwhoosh05",
];

/// The trail and the actors that show it.
pub struct TrailRings {
    /// The pure trail: where the rings are and what Harry has done with them.
    pub trail: RingTrail,
    /// One `HGame.HippoRing` per ring slot, reused for ring after ring. All of them are in the match's
    /// `actors`, so `glue::stop` destroys them with the rest.
    pool: Vec<ObjId>,
    /// Which ring each slot is showing, so a slot is only turned on again when the ring in it changes.
    slot: Vec<Option<u64>>,
    /// The emitter each slot's ring is holding (`HippoRing.ringFX`), so `stop` can destroy it: the ring
    /// spawns it itself and it is not in the match's `actors`.
    fx: Vec<Option<ObjId>>,
    /// Where Harry was at the end of the last frame: the start of the segment tested against the rings.
    last_harry: Option<Vec3>,
    /// `HP3_QUID_RING_AUTOFLY=1` (debugging, off by default): the broom flies itself from ring to ring, which
    /// is how a headless run proves a pass-through without anyone at the keyboard. Nothing in the game sets it.
    autofly: bool,
    /// The trail's own stream for picking a pass sound.
    rng: u64,
}

impl Default for TrailRings {
    fn default() -> TrailRings {
        TrailRings {
            trail: RingTrail::with_tuning(trail::TrailTuning::default()),
            pool: Vec::new(),
            slot: Vec::new(),
            fx: Vec::new(),
            last_harry: None,
            autofly: crate::envc::var("HP3_QUID_RING_AUTOFLY").is_ok_and(|v| v.trim() != "0"),
            rng: 0,
        }
    }
}

fn v3(v: Vec3) -> Value {
    Value::Struct(vec![Value::Float(v.x), Value::Float(v.y), Value::Float(v.z)].into())
}

/// A line that goes to the console and into the level's log.
fn note(vm: &mut Vm<'_>, line: String) {
    println!("{line}");
    vm.log.push(line);
}

/// SplitMix64: the trail's own stream, so picking a sound never moves the match's own randomness.
fn split(state: &mut u64) -> u32 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    ((z ^ (z >> 31)) >> 32) as u32
}

/// Spawns the ring pool: one `HGame.HippoRing` per ring slot, born hidden and switched off (nothing is drawn
/// until `TurnOn`). Called from `glue::start`; every ring goes into the match's `actors`.
pub fn start(vm: &mut Vm<'_>, m: &mut QuidditchMatch) {
    let t = m.rings.trail.tuning();
    m.rings.rng = m.frames ^ 0x7217_f00d_c0de_1151;
    let class = crate::find_object(vm, RING_CLASS);
    if class.is_none() {
        note(vm, format!("quidditch: no {RING_CLASS}, the Snitch's ring trail is invisible (it still counts)"));
    }
    for _ in 0..t.max_rings {
        let Some(class) = class else { break };
        let args = [Value::Object(Some(class)), Value::Object(None), Value::Empty, v3(Vec3::ZERO), Value::Struct(vec![Value::Int(0); 3].into())];
        let Some(a) = crate::natives::spawn(vm, None, &args).ok().and_then(|v| v.as_obj()) else {
            break;
        };
        // A ring is a guide, not an obstacle: the disc test in `trail` is the only thing that cares where
        // Harry goes, and the retail ring's own `Touch` wants a hippogriff rider to hand a speed boost to.
        // (`TurnOn` calls `SetCollision(true, ...)`, so this is set again after every one.)
        crate::set(vm, a, "bCollideActors", Value::Bool(false));
        crate::set(vm, a, "bBlockActors", Value::Bool(false));
        crate::set(vm, a, "bBlockPlayers", Value::Bool(false));
        crate::set(vm, a, "bStatic", Value::Bool(false));
        crate::set(vm, a, "bNoDelete", Value::Bool(false));
        crate::set(vm, a, "bHidden", Value::Bool(true));
        // What `TurnOn` hands the emitter's `SetRadius`: without it the polar radius is 0 and all 96 sprites
        // are born on top of each other. `fRingLength` 0 keeps the ring a flat circle, as the retail ones are.
        crate::set(vm, a, "CollisionWidth", Value::Float(t.radius));
        crate::set(vm, a, "CollisionHeight", Value::Float(t.radius));
        crate::set(vm, a, "fRingLength", Value::Float(0.0));
        m.actors.push(a);
        m.rings.pool.push(a);
    }
    let slots = m.rings.pool.len();
    m.rings.slot = vec![None; slots];
    m.rings.fx = vec![None; slots];
    note(vm, format!(
        "quidditch: the Snitch's ring trail: {slots} Buckbeak flight rings ({RING_CLASS}) at radius {:.0}, one every {:.0} uu, {:.0} s each, {} in a row to catch",
        t.radius, t.spacing, t.life, t.catch_rings,
    ));
}

/// Destroys the emitters the rings are holding. The rings themselves are in the match's `actors` and go with
/// the rest; their emitters are spawned by the ring's own script and are nobody else's, so they are killed
/// here. Called from `glue::stop`.
pub fn stop(vm: &mut Vm<'_>, m: &QuidditchMatch) {
    for (i, a) in m.rings.pool.iter().enumerate() {
        if !vm.db.objects.get(*a).deleted {
            crate::fire(vm, *a, "TurnOff", &mut []);
        }
        // `TurnOff` only `Kill`s the emitter, which stops it spawning; `AutoDestroy` is off, so the actor
        // itself has to go here or it would outlive the match.
        if let Some(fx) = m.rings.fx.get(i).copied().flatten() {
            crate::set(vm, fx, "bNoDelete", Value::Bool(false));
            crate::destroy_actor(vm, fx);
        }
    }
}

/// Where the debug autofly is steering, if it is on: the next ring of the trail, else the Snitch itself.
/// `glue` flies the broom at it instead of reading the keys.
pub fn autofly_target(m: &QuidditchMatch) -> Option<Vec3> {
    if !m.rings.autofly {
        return None;
    }
    Some(m.rings.trail.next_ring().map(|r| r.center).unwrap_or_else(|| m.snitch.pos()))
}

/// One frame of the trail: the pure step, the bonus a ring is worth, its sound, and the actors that show the
/// rings. Called from `glue::tick_match` while the match is being played.
pub fn tick(vm: &mut Vm<'_>, m: &mut QuidditchMatch, dt: f32, events: &mut Vec<MatchEvent>) {
    let now = m.broom.pos;
    let prev = m.rings.last_harry.unwrap_or(now);
    m.rings.last_harry = Some(now);
    let t = m.rings.trail.tuning();
    // On the Snitch's path, not at the Snitch itself: see the note at the top of `trail`.
    let got = m.rings.trail.step(m.snitch.path_pos(), m.snitch.travel_dir(), prev, now, dt);
    for e in &got {
        match *e {
            TrailEvent::Through { index, streak } => {
                let bonus = trail::bonus_for(&t, e);
                m.snitch.bonus_progress(bonus);
                let ready = streak >= t.catch_rings;
                events.push(MatchEvent::Rings { streak, ready });
                let mut state = m.rings.rng;
                let pick = split(&mut state) as usize % PASS_SOUNDS.len();
                m.rings.rng = state;
                if let Some(snd) = crate::find_object_of(vm, PASS_SOUNDS[pick], Some("Sound")) {
                    super::presentation::play_once(vm, snd, now, 0.8, 1.0, 3000.0);
                }
                note(vm, format!(
                    "quidditch: tick {}: through ring {index} ({streak} in a row), +{bonus:.2} to the bar (now {:.2}), the catch {}",
                    m.frames,
                    m.snitch.progress(),
                    if ready { "is open to him" } else { "still wants more rings" },
                ));
            }
            TrailEvent::Missed { streak_lost } if streak_lost > 0 => {
                note(vm, format!("quidditch: tick {}: a ring gone past, the run of {streak_lost} is lost", m.frames));
            }
            TrailEvent::Missed { .. } => {}
        }
    }
    // The owner's rule: no catch without the run of rings. Told to the Snitch every frame, so a run lost to a
    // missed ring shuts the window's door again straight away.
    m.snitch.set_rings_ready(m.rings.trail.catch_ready());
    place(vm, m);
}

/// An Unreal rotator whose forward axis is `n`, which is the way a flight ring faces: the Buckbeak rings are
/// flat circles standing across the flight, and the emitter's circle is turned by the actor's own rotation.
fn facing(n: Vec3) -> Value {
    let n = n.normalize_or_zero();
    let n = if n == Vec3::ZERO { Vec3::X } else { n };
    let r = super::camera::rotator(n.z.clamp(-1.0, 1.0).asin(), n.y.atan2(n.x));
    Value::Struct(vec![Value::Int(r[0]), Value::Int(r[1]), Value::Int(r[2])].into())
}

/// Puts each live ring on its own slot of the pool and switches off the slots with no ring in them. A slot is
/// only re-aimed when the ring it shows changes, so the six rings standing in the air cost nothing a frame.
fn place(vm: &mut Vm<'_>, m: &mut QuidditchMatch) {
    let slots = m.rings.slot.len();
    if slots == 0 {
        return;
    }
    let mut want: Vec<Option<(u64, Vec3, Vec3)>> = vec![None; slots];
    for r in m.rings.trail.rings() {
        // At most `max_rings` rings are alive and the ids run on, so the id modulo the number of slots picks
        // each live ring a slot of its own.
        want[(r.id % slots as u64) as usize] = Some((r.id, r.center, r.normal));
    }
    for i in 0..slots {
        let a = m.rings.pool[i];
        if vm.db.objects.get(a).deleted {
            continue;
        }
        match want[i] {
            Some((id, center, normal)) if m.rings.slot[i] != Some(id) => {
                m.rings.slot[i] = Some(id);
                crate::set(vm, a, "Location", v3(super::arena::clamp_into(&m.arena, center)));
                crate::set(vm, a, "Rotation", facing(normal));
                crate::set(vm, a, "bHidden", Value::Bool(false));
                // The ring's own axes are read off its rotation, and then its own script builds the sprite
                // ring: `TurnOn` destroys the slot's old emitter, spawns a fresh `HippoRingEmitter` here and
                // gives it the radius and the colour. It also switches the collision on, which this match
                // does not want.
                crate::fire(vm, a, "Setup", &mut []);
                crate::fire(vm, a, "TurnOn", &mut [Value::Name(Name::NONE)]);
                crate::set(vm, a, "bCollideActors", Value::Bool(false));
                m.rings.fx[i] = crate::get(vm, a, "ringFX").as_obj();
            }
            Some(_) => {}
            None if m.rings.slot[i].is_some() => {
                m.rings.slot[i] = None;
                crate::fire(vm, a, "TurnOff", &mut []);
                if let Some(fx) = m.rings.fx[i].take() {
                    crate::set(vm, fx, "bNoDelete", Value::Bool(false));
                    crate::destroy_actor(vm, fx);
                }
                crate::set(vm, a, "bHidden", Value::Bool(true));
            }
            None => {}
        }
    }
}

/// The rings and the run, for the headless trace.
pub fn debug_line(m: &QuidditchMatch) -> String {
    let rings: Vec<String> = m.rings.trail.rings().iter().map(|r| format!("#{}({:.0}, {:.0}, {:.0})", r.id, r.center.x, r.center.y, r.center.z)).collect();
    format!(
        "quidditch: ring trail {} of {} rings, streak {} of {} (catch {}): [{}]",
        rings.len(),
        m.rings.trail.tuning().max_rings,
        m.rings.trail.streak(),
        m.rings.trail.tuning().catch_rings,
        if m.snitch.rings_ready() { "open" } else { "shut" },
        rings.join(" "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Buckbeak ring is a flat circle standing across the flight, and the emitter's circle is turned by the
    /// ring actor's own rotation: so the rotation `place` writes must point its forward axis straight down
    /// the ring's normal, whichever way the Snitch was flying.
    #[test]
    fn a_trail_ring_faces_the_way_the_snitch_was_flying() {
        let k = std::f32::consts::TAU / 65536.0;
        for n in [Vec3::X, -Vec3::Y, Vec3::Z, -Vec3::Z, Vec3::new(-2.0, 1.0, 0.5).normalize(), Vec3::new(0.3, -0.4, 0.86).normalize()] {
            let f = facing(n);
            let r = f.fields();
            assert_eq!(r.len(), 3, "{f:?} is not a rotator");
            let (pitch, yaw) = (r[0].as_int() as f32 * k, r[1].as_int() as f32 * k);
            let fwd = Vec3::new(pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), pitch.sin());
            assert!((fwd - n).length() < 1e-3, "{fwd:?} is not {n:?}");
            assert_eq!(r[2].as_int(), 0, "a flight ring has no roll");
        }
        // Nothing usable in still gives a usable rotator rather than a NaN one.
        for n in [Vec3::ZERO, Vec3::splat(f32::NAN)] {
            let r = facing(n);
            assert!(r.fields().iter().all(|v| matches!(v, Value::Int(_))), "{r:?}");
        }
    }
}
