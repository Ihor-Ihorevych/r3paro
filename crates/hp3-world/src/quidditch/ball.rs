//! The Quaffle's actor, the scoring hoops, and the goal that comes of putting the one through the other.
//!
//! This touches the VM, like `glue` and `presentation`, but it is called only from `glue`, so the match's own
//! logic stays in one file and the Quaffle's in this one. What it decides comes out of the pure `quaffle` and
//! `goals` modules; what is here is the spawning, the `Location` writes and the log lines.
//!
//! Missing art: HP3 has no Quaffle mesh, so the Quaffle is the same round stand-in mesh a Bludger is made of. The hoops are the
//! retail goal posts' own and get no ring drawn around them (an earlier bead ring was taken out: the rings are the Snitch's).
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
    // The goal hoops are the retail posts' own: no ring is drawn around them (the rings are the Snitch trail's, `rings`).
    let shot = match m.balls.shoot_at {
        Some(t) => format!(", HP3_QUID_SHOOT: a shot through hoop {} at tick {t}", m.balls.shoot_index()),
        None => String::new(),
    };
    note(vm, format!("quidditch: Quaffle at ({:.0}, {:.0}, {:.0}), {} hoops{shot}", at.x, at.y, at.z, m.balls.hoops.len()));
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
