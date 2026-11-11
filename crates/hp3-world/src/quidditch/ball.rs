//! The Quaffle's actor, the hoop rings, and the goal that comes of putting the one through the other.
//!
//! This touches the VM, like `glue` and `presentation`, but it is called only from `glue`, so the match's own
//! logic stays in one file and the Quaffle's in this one. What it decides comes out of the pure `quaffle` and
//! `goals` modules; what is here is the spawning, the `Location` writes and the log lines.
//!
//! Missing art: HP3 has no Quaffle mesh, so the Quaffle is the same round stand-in mesh a Bludger is made of.
//! It has **no ring mesh at all**, and the retail data was searched for one (`docs/formats/quidditch-arena.md`):
//! the Buckbeak flight rings, the obvious thing to borrow, are a particle emitter and no geometry
//! (`HGame.HippoRing` carries no mesh; its `ringFXClass` is `HGame.HippoRingEmitter`, sprites of the
//! `HP_FX.Particles.Ring` texture), and the only mesh in the game with a hole through it, the Dementor's
//! spawn ring, is a capless cylinder band that all but vanishes when it is looked at down its own axis --
//! which is exactly how a goal hoop is seen. So a ring is drawn the way the old games' rings were: a circle
//! of twelve small balls around the hoop's rim, wrapped in the goal posts' own gold and lit by themselves,
//! so a hoop is a gold ring from across the pitch. The retail quads stay where they are.
//!
//! Nothing throws the Quaffle yet: the Chasers are Task 13. Until then it lies on the pitch, and the debug
//! `HP3_QUID_SHOOT=<tick>` shoots it through a hoop to show that a goal is scored and counted.

use glam::Vec3;

use hp3_script::{ObjId, Value, Vm};

use super::arena::{self, ArenaBounds};
use super::glue::QuidditchMatch;
use super::goals::Hoop;
use super::presentation;
use super::quaffle::{self, Quaffle};

/// How many beads a hoop's ring is drawn with, and how big each one is (uu). Twelve beads of 32 around a
/// radius of 120 touch each other (`120 * sin(180/12) = 31`), so the ring reads as a ring, and a bead is a
/// ball from every side -- unlike a flat ring, it never vanishes when the hoop is looked at head on.
const RING_BEADS: usize = 12;
const BEAD_RADIUS: f32 = 32.0;
/// What a ring bead is wrapped in: the gold of the retail goal posts under it
/// (`HP3_ArchiExterior.Quid_Stadium.Quid_GoalPost`), which is a plain texture the renderer draws. The glow
/// the Bludgers wear is no use here -- `HP_FX.General.Bludger_Halo` is a `Fire.IceTexture`, a procedural
/// material nothing generates, so a bead skinned with it comes out black.
const RING_SKIN: &str = "HP3_ArchiExterior.Quid_Stadium.Quid_GoalPost";

/// The Quaffle and the hoops, with the actors that show them.
pub struct Balls {
    /// The pure ball: where it is, where it is going, who has it.
    pub quaffle: Quaffle,
    /// The six scoring hoops (`arena::hoops`).
    pub hoops: Vec<Hoop>,
    /// The ball's actor, hidden while there is none.
    actor: Option<ObjId>,
    /// `HP3_QUID_SHOOT=<tick>` (debugging, off by default): the tick at which the Quaffle is shot from the
    /// centre of the pitch through `shoot_hoop`. Nothing in the game sets it.
    shoot_at: Option<u64>,
    /// `HP3_QUID_SHOOT_HOOP=<0..5>` (debugging): which hoop that shot goes through -- 0..2 are the north
    /// goal's (a home goal), 3..5 the south goal's (the opponent's). Default 0.
    shoot_hoop: usize,
}

impl Balls {
    /// The Quaffle at the middle of the pitch and the six hoops of the arena, with the debug shot read off
    /// the environment.
    pub fn new(b: &ArenaBounds) -> Balls {
        let shoot_at = crate::envc::var("HP3_QUID_SHOOT").ok().and_then(|v| v.trim().parse::<u64>().ok()).filter(|&n| n > 0);
        let shoot_hoop = crate::envc::var("HP3_QUID_SHOOT_HOOP").ok().and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0);
        Balls {
            quaffle: Quaffle::new(arena::bludger_home(b)),
            hoops: arena::hoops(),
            actor: None,
            shoot_at,
            shoot_hoop,
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

/// Two unit vectors that span the disc of a hoop whose axis is `n`.
fn disc_axes(n: Vec3) -> (Vec3, Vec3) {
    let n = n.normalize_or_zero();
    if n == Vec3::ZERO {
        return (Vec3::X, Vec3::Z);
    }
    // Anything not along the axis will do to start from; the hoops' axes lie along y, so z is the one.
    let seed = if n.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
    let u = n.cross(seed).normalize_or_zero();
    if u == Vec3::ZERO {
        return (Vec3::X, Vec3::Z);
    }
    (u, n.cross(u))
}

/// Where the beads of `h`'s ring go: `RING_BEADS` points evenly around its rim, in its own plane.
fn ring_points(h: &Hoop) -> Vec<Vec3> {
    let (u, v) = disc_axes(h.normal);
    (0..RING_BEADS)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / RING_BEADS as f32;
            h.center + (u * a.cos() + v * a.sin()) * h.radius
        })
        .collect()
}

/// Spawns the Quaffle's ball and a bead ring at every hoop. Called at the end of `glue::start`; every actor
/// it makes goes into the match's `actors`, so `glue::stop` destroys them with the rest.
pub fn start(vm: &mut Vm<'_>, m: &mut QuidditchMatch) {
    let at = m.balls.quaffle.pos;
    if let Some(a) = presentation::spawn_ball(vm, Some("Quaffle"), quaffle::RADIUS, None, false) {
        crate::set(vm, a, "Location", v3(at));
        crate::set(vm, a, "bHidden", Value::Bool(false));
        m.actors.push(a);
        m.balls.actor = Some(a);
    }
    // The rings. A bead never moves and nothing may bounce off it -- a hoop is scored through, and the disc
    // test in `goals` is the only thing that cares where the ball goes. It is left off `bStatic` all the
    // same: a spawned `bStatic` static mesh is drawn by nobody (`presentation::spawn_ball`).
    let mut beads = 0;
    for h in m.balls.hoops.clone() {
        for p in ring_points(&h) {
            // Only the first bead says what it is made of: seventy-two identical lines are no use to anyone.
            let what = (beads == 0).then_some("hoop ring bead");
            let Some(a) = presentation::spawn_ball(vm, what, BEAD_RADIUS, Some(RING_SKIN), true) else {
                break;
            };
            crate::set(vm, a, "Location", v3(p));
            crate::set(vm, a, "bCollideActors", Value::Bool(false));
            crate::set(vm, a, "bBlockActors", Value::Bool(false));
            crate::set(vm, a, "bBlockPlayers", Value::Bool(false));
            crate::set(vm, a, "bHidden", Value::Bool(false));
            m.actors.push(a);
            beads += 1;
        }
    }
    let shot = match m.balls.shoot_at {
        Some(t) => format!(", HP3_QUID_SHOOT: a shot through hoop {} at tick {t}", m.balls.shoot_index()),
        None => String::new(),
    };
    note(vm, format!("quidditch: Quaffle at ({:.0}, {:.0}, {:.0}), {} hoop rings of {beads} beads{shot}", at.x, at.y, at.z, m.balls.hoops.len()));
}

impl Balls {
    /// The hoop the debug shot aims at, kept inside the table however the environment was set.
    fn shoot_index(&self) -> usize {
        if self.hoops.is_empty() { 0 } else { self.shoot_hoop.min(self.hoops.len() - 1) }
    }
}

/// One frame of the Quaffle: the debug shot if this is its tick, the ball's own flight, the actor that shows
/// it, and the goal it may have scored. `Some(home)` is a goal for that side, which `glue` hands to
/// `Rules::add_goal` and to the commentary.
pub fn tick(vm: &mut Vm<'_>, m: &mut QuidditchMatch, dt: f32) -> Option<bool> {
    if m.balls.shoot_at == Some(m.frames) && !m.balls.hoops.is_empty() {
        let h = m.balls.hoops[m.balls.shoot_index()];
        let from = arena::bludger_home(&m.arena);
        m.balls.quaffle.shoot(from, h.center, quaffle::SHOOT_SPEED);
        note(vm, format!(
            "quidditch: HP3_QUID_SHOOT at tick {}: Quaffle from ({:.0}, {:.0}, {:.0}) at the {} hoop ({:.0}, {:.0}, {:.0})",
            m.frames, from.x, from.y, from.z,
            if h.home { "north (home)" } else { "south (away)" },
            h.center.x, h.center.y, h.center.z,
        ));
    }
    let scored = m.balls.quaffle.step_and_score(dt, &m.balls.hoops);
    // Nobody is playing with it, so it comes to rest on the pitch floor instead of falling out of the world.
    m.balls.quaffle.confine(&m.arena);
    // `confine` has already made it finite and put it inside the arena, turf included, so it goes onto the
    // actor as it is (`clamp_into` would lift a resting ball to the flight volume's floor).
    if let Some(a) = m.balls.actor.filter(|&a| !vm.db.objects.get(a).deleted) {
        crate::set(vm, a, "Location", v3(m.balls.quaffle.pos));
    }
    let h = m.balls.hoops[scored?];
    note(vm, format!(
        "quidditch: GOAL for {} through the hoop at ({:.0}, {:.0}, {:.0}) on tick {}",
        if h.home { "Gryffindor" } else { &m.opponent },
        h.center.x, h.center.y, h.center.z, m.frames,
    ));
    // Back to the middle of the pitch for the next play, as the real game restarts after a goal.
    m.balls.quaffle = Quaffle::new(arena::bludger_home(&m.arena));
    Some(h.home)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ring_lies_in_its_hoops_plane_at_its_radius() {
        for h in arena::hoops() {
            let pts = ring_points(&h);
            assert_eq!(pts.len(), RING_BEADS);
            for p in &pts {
                assert!(((*p - h.center).length() - h.radius).abs() < 1e-3, "{p:?} is not on the rim of {h:?}");
                // In the plane of the disc: no bead sticks out in front of or behind the hoop.
                assert!((*p - h.center).dot(h.normal).abs() < 1e-3, "{p:?} is off the plane of {h:?}");
            }
            // The beads are spread around the rim, not bunched on one side.
            let mid = pts.iter().copied().fold(Vec3::ZERO, |a, b| a + b) / RING_BEADS as f32;
            assert!((mid - h.center).length() < 1e-2, "the ring of {h:?} is lopsided: {mid:?}");
        }
    }

    #[test]
    fn the_beads_of_a_ring_touch_each_other() {
        // The gap between two neighbours must be no wider than a bead, or the ring reads as loose dots.
        for h in arena::hoops() {
            let pts = ring_points(&h);
            let gap = (pts[1] - pts[0]).length();
            assert!(gap <= 2.0 * BEAD_RADIUS, "the beads of {h:?} are {gap:.1} apart, wider than {BEAD_RADIUS}");
        }
    }
}
