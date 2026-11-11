//! The Snitch's ring trail on screen: the actors that show each flight ring, the sound of flying through one,
//! and the bonus it is worth. What it decides comes out of the pure `trail` module; what is here is the
//! spawning, the `Location` writes, the sounds and the log lines.
//!
//! This touches the VM, like `glue`, `presentation` and `ball`, but it is called only from `glue`.
//!
//! **The Buckbeak ring was tried first and dropped.** `HGame.HippoRing`, the flight ring of the hippogriff
//! ride, carries no mesh: it is the particle emitter `HGame.HippoRingEmitter`, and our particle system does
//! run it (`HP3_COMC`'s 110 rings come out as 36 live sprites each, standing in a circle of about 200 uu by
//! `StartLocationPolarRange`, which the actor's rotation turns -- so it can even be aimed). Spawning ten of
//! them as the trail was tried and rendered: it does not work. The sprites are `HP3_FX.Bats` flappers that fly
//! outward from the circle, so a ring reads as a loose cloud rather than a hoop, and the particles are in
//! world coordinates, so a pool actor moved to its next ring leaves its whole swarm behind (hidden emitters
//! freeze instead of clearing). The frame is in the task's report.
//!
//! So a trail ring is drawn the way the goal hoops are (`quidditch::ball`): a circle of small bright balls
//! around the rim, out of the shared `goals::ring_points`. Eight of them, not twelve, because there are ten
//! rings of them on screen and the phone has 60 fps to keep.
//!
//! The pool is fixed: one set of beads per ring slot (`TrailTuning::max_rings`, ten), moved and hidden rather
//! than spawned and destroyed, so a match of hundreds of rings spawns eighty actors and `glue::stop` destroys
//! eighty.

use glam::Vec3;

use hp3_script::{ObjId, Value, Vm};

use super::glue::QuidditchMatch;
use super::rules::MatchEvent;
use super::trail::{self, RingTrail, TrailEvent};

/// How many beads a trail ring is drawn with and how big each one is (uu). Eight of 48, costing eight actors a
/// ring instead of a goal hoop's twelve, because ten rings are on screen at once and the phone has 60 fps to
/// keep. They stand 168 uu apart on a rim of 220 (`2 * 220 * sin(180/8)`), so the ring is a circle of beads
/// rather than a solid band -- which is also what keeps it from filling the screen when Harry flies through
/// one, and it is read as a ring at the distance it is aimed at. A bead is the goal hoops' own size, so the
/// two kinds of ring are plainly the same family of thing: a trail ring is the wider, sparser one.
const RING_BEADS: usize = 8;
const BEAD_RADIUS: f32 = 32.0;
/// What a bead is wrapped in: the gold of the retail goal posts, the same as the hoop rings' beads -- a plain
/// texture the renderer draws, and the only gold in the stadium's own package.
const RING_SKIN: &str = "HP3_ArchiExterior.Quid_Stadium.Quid_GoalPost";
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
    /// `RING_BEADS` actors per ring slot, laid end to end, reused for ring after ring. All of them are in the
    /// match's `actors`, so `glue::stop` destroys them with the rest.
    pool: Vec<ObjId>,
    /// Which ring each slot is showing, so a slot is only written when the ring in it changes.
    slot: Vec<Option<u64>>,
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

/// Spawns the ring pool: `RING_BEADS` beads for each of the trail's ring slots, born hidden. Called from
/// `glue::start`; every actor goes into the match's `actors`.
pub fn start(vm: &mut Vm<'_>, m: &mut QuidditchMatch) {
    let t = m.rings.trail.tuning();
    m.rings.rng = m.frames ^ 0x7217_f00d_c0de_1151;
    for i in 0..t.max_rings * RING_BEADS {
        // Only the first bead says what it is made of: eighty identical lines are no use to anyone.
        let what = (i == 0).then_some("Snitch trail ring bead");
        let Some(a) = super::presentation::spawn_ball(vm, what, BEAD_RADIUS, Some(RING_SKIN), true) else {
            break;
        };
        // A bead is scenery with nothing to bump into: the disc test in `trail` is the only thing that cares
        // where Harry goes. It is born hidden and stays off `bStatic` (a spawned `bStatic` mesh is drawn by
        // nobody) so it can be moved from ring to ring.
        crate::set(vm, a, "bCollideActors", Value::Bool(false));
        crate::set(vm, a, "bBlockActors", Value::Bool(false));
        crate::set(vm, a, "bBlockPlayers", Value::Bool(false));
        m.actors.push(a);
        m.rings.pool.push(a);
    }
    let slots = m.rings.pool.len() / RING_BEADS;
    m.rings.slot = vec![None; slots];
    note(vm, format!(
        "quidditch: the Snitch's ring trail: {slots} rings of {RING_BEADS} beads at radius {:.0}, one every {:.0} uu, {:.0} s each, {} in a row to catch",
        t.radius, t.spacing, t.life, t.catch_rings,
    ));
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

/// Puts each live ring on its own slot of the pool and hides the slots with no ring in them. A slot's beads
/// are written only when the ring it shows changes, so the ten rings standing in the air cost nothing a frame.
fn place(vm: &mut Vm<'_>, m: &mut QuidditchMatch) {
    let slots = m.rings.slot.len();
    if slots == 0 {
        return;
    }
    let mut want: Vec<Option<(u64, Vec3, Vec3, f32)>> = vec![None; slots];
    for r in m.rings.trail.rings() {
        // At most `max_rings` rings are alive and the ids run on, so the id modulo the number of slots picks
        // each live ring a slot of its own.
        want[(r.id % slots as u64) as usize] = Some((r.id, r.center, r.normal, r.radius));
    }
    for i in 0..slots {
        let beads = &m.rings.pool[i * RING_BEADS..(i + 1) * RING_BEADS];
        match want[i] {
            Some((id, center, normal, radius)) if m.rings.slot[i] != Some(id) => {
                m.rings.slot[i] = Some(id);
                let points = super::goals::ring_points(center, normal, radius, RING_BEADS);
                for (&a, p) in beads.to_vec().iter().zip(points) {
                    if vm.db.objects.get(a).deleted {
                        continue;
                    }
                    crate::set(vm, a, "Location", v3(super::arena::clamp_into(&m.arena, p)));
                    crate::set(vm, a, "bHidden", Value::Bool(false));
                }
            }
            Some(_) => {}
            None if m.rings.slot[i].is_some() => {
                m.rings.slot[i] = None;
                for a in beads.to_vec() {
                    if !vm.db.objects.get(a).deleted {
                        crate::set(vm, a, "bHidden", Value::Bool(true));
                    }
                }
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

    /// A trail ring's beads stand on its rim, in its own plane, whichever way the Snitch was flying -- and
    /// near enough to each other to read as a ring rather than eight dots.
    #[test]
    fn a_trail_rings_beads_stand_on_its_rim_whichever_way_it_faces() {
        let radius = trail::TrailTuning::default().radius;
        for n in [Vec3::X, -Vec3::Y, Vec3::Z, Vec3::new(-2.0, 1.0, 0.5).normalize(), Vec3::new(0.3, -0.4, 0.86).normalize()] {
            let c = Vec3::new(100.0, -200.0, 1500.0);
            let pts = super::super::goals::ring_points(c, n, radius, RING_BEADS);
            assert_eq!(pts.len(), RING_BEADS);
            for p in &pts {
                assert!(((*p - c).length() - radius).abs() < 1e-2, "{p:?} is off the rim for {n:?}");
                assert!((*p - c).dot(n).abs() < 1e-2, "{p:?} is off the plane of {n:?}");
            }
            // Evenly spread, and never further apart than the ring is wide: eight beads of `BEAD_RADIUS` on a
            // rim of 220 are a dotted circle, which is what is wanted, and not a handful of scattered balls.
            let gaps: Vec<f32> = (0..RING_BEADS).map(|i| (pts[(i + 1) % RING_BEADS] - pts[i]).length()).collect();
            let gap = gaps[0];
            assert!(gaps.iter().all(|g| (g - gap).abs() < 1e-2), "the beads are not evenly spread: {gaps:?}");
            assert!(gap <= radius, "the beads are {gap:.0} apart, wider than the ring's own {radius:.0}");
        }
    }
}
