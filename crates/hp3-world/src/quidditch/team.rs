//! The team's actors: what the pure `quidditch::ai` looks like on the pitch.
//!
//! This touches the VM, like `glue`, `presentation` and `ball`, but it is called only from `glue`, so the
//! match's own logic stays in one file and the teams' in this one. Every decision comes out of the pure `ai`
//! module; what is here is finding the riders, writing their `Location` every frame, and putting the level's
//! own back as they were.
//!
//! **The riders are the level's own placed players.** `HP3_Quidditch` stands thirteen of them on the turf --
//! seven Gryffindor (`QuidPlayerGriffMale`, one `QuidPlayerGriffFem`) and six Slytherin
//! (`QuidPlayerSlyMale`) -- and `stage::clear` deliberately leaves them where they are. A match borrows them:
//! each one is put on a broom, flown by an agent for as long as the match runs, and set back down where it
//! stood when the match stops. There are thirteen agents and thirteen placed players, but not in the same mix
//! (the opponent fields seven and Gryffindor six), so a side that runs short has the rest spawned, and those
//! belong to the match like any other actor it made.

use glam::Vec3;

use hp3_script::{ObjId, Value, Vm};

use super::ai::{self, Team, TeamEvent, TeamPlay};
use super::arena;
use super::camera;
use super::glue::QuidditchMatch;

/// The pawn classes each side's riders are, best first: the class a short side's extra rider is spawned as,
/// and the classes a placed player is recognised by.
///
/// The retail data has riders for Gryffindor and Slytherin only (`QuidPlayer*Griff*`, `QuidPlayer*Sly*`),
/// which is all the game's one Quidditch match ever needed. A league round against Ravenclaw or Hufflepuff
/// therefore **flies the Slytherin riders**: the house is named in the HUD and the commentary, but the kit on
/// the brooms is green. Recolouring them would mean new textures or a material override on the skeletal mesh,
/// which is a job of its own and not done here.
const HOME_CLASSES: [&str; 2] = ["HGame.QuidPlayerGriffMale", "HGame.QuidPlayerGriffFem"];
const AWAY_CLASSES: [&str; 2] = ["HGame.QuidPlayerSlyMale", "HGame.QuidPlayerSlyFem"];

/// Animations a rider is put in while he flies, best first: the first one his animation set has wins, as it
/// does for Harry. A set with none of them plays nothing and the rider flies in his rest pose.
///
/// `fly_forward` is the real one: the retail `QuidPlayer*_KW` sets are made for exactly this match and hold
/// `fly_forward`, `brake`, `boost`, `turn_left`, `turn_right`, `dive`, `pull_up`, `bump`, `throw_*` and
/// `catch_*` (the rest are fallbacks in case another mesh is used for a rider). Only the one flying pose is
/// used: picking the turn and the dive from the agent's own steering is a polish pass of its own.
const FLY_ANIMS: [&str; 6] = ["fly_forward", "Hover", "QuidFly", "Fly", "Idle", "Stand"];

/// What the match took from the level, and what to put back when it ends.
struct Borrowed {
    actor: ObjId,
    location: Vec3,
    rotation: [i32; 3],
    physics: u8,
    hidden: bool,
}

/// Both teams: the pure play and the actors that show it.
pub struct Teams {
    /// The agents and the Quaffle play between them (`quidditch::ai`).
    pub play: TeamPlay,
    /// One rider per agent, in the agents' own order; `None` for an agent nothing could be found for.
    pawns: Vec<Option<ObjId>>,
    /// The level's own players this match is flying, with what they were.
    borrowed: Vec<Borrowed>,
    /// The animation the riders fly in, for the log.
    anim: Option<&'static str>,
    /// `HP3_QUID_DEBUG` only: how long the team AI and the riders' `Location` writes have taken in all, and
    /// over how many ticks, so the mean cost of a frame of team play can be read off a headless run and
    /// judged against the phone's 16.6 ms. Nothing measures itself without the variable set.
    cost: std::time::Duration,
    measured: u64,
}

impl Teams {
    pub fn new(seed: u64) -> Teams {
        Teams {
            play: TeamPlay::new(seed),
            pawns: Vec::new(),
            borrowed: Vec::new(),
            anim: None,
            cost: std::time::Duration::ZERO,
            measured: 0,
        }
    }

    /// How many riders a match has on the pitch, for the log.
    pub fn riders(&self) -> usize {
        self.pawns.iter().filter(|p| p.is_some()).count()
    }
}

fn v3(v: Vec3) -> Value {
    Value::Struct(vec![Value::Float(v.x), Value::Float(v.y), Value::Float(v.z)].into())
}

fn rot(r: [i32; 3]) -> Value {
    Value::Struct(vec![Value::Int(r[0]), Value::Int(r[1]), Value::Int(r[2])].into())
}

/// The classes a side's riders are made of.
fn classes_of(team: Team) -> &'static [&'static str] {
    if team.is_home() { &HOME_CLASSES } else { &AWAY_CLASSES }
}

/// The level's own placed players of `team`, in level order: the pool a match borrows its riders from. The
/// player's own pawn is never one of them.
fn placed(vm: &mut Vm<'_>, team: Team) -> Vec<ObjId> {
    let wanted: Vec<_> = classes_of(team).iter().filter_map(|p| p.rsplit('.').next()).filter_map(|n| vm.db.find(n)).collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    let actors = crate::world(vm).actors.clone();
    let player = crate::world(vm).player;
    let player_pawn = player.and_then(|pc| crate::get(vm, pc, "Pawn").as_obj());
    let pool: Vec<ObjId> = actors
        .into_iter()
        .filter(|&a| !vm.db.objects.get(a).deleted && Some(a) != player_pawn)
        .filter(|&a| vm.db.objects.get(a).class.is_some_and(|c| wanted.iter().any(|&w| vm.is_child_of(c, w))))
        .collect();
    if crate::envc::var_os("HP3_QUID_DEBUG").is_some() {
        let names: Vec<String> = pool.iter().map(|&a| hp3_script::natives::object_path(vm, a)).collect();
        println!("quidditch: {team:?} riders on the turf: {names:?}");
    }
    pool
}

/// Spawns one rider of `team` at `pos`. Only a side the level is short of needs it.
fn spawn_rider(vm: &mut Vm<'_>, team: Team, pos: Vec3) -> Option<ObjId> {
    for path in classes_of(team) {
        let Some(class) = crate::find_object(vm, path) else { continue };
        let args = [Value::Object(Some(class)), Value::Object(None), Value::Empty, v3(pos), rot([0, 0, 0])];
        if let Some(a) = crate::natives::spawn(vm, None, &args).ok().and_then(|v| v.as_obj()) {
            return Some(a);
        }
    }
    vm.log.push(format!("quidditch: no rider class of {:?} could be spawned", classes_of(team)));
    None
}

/// Puts one rider in the air: the match flies him, so he collides with nothing, never falls and is never
/// checked for being out of the world (`flight::confine` is all the boundary an agent has).
fn mount(vm: &mut Vm<'_>, a: ObjId) {
    crate::set(vm, a, "bCollideWorld", Value::Bool(false));
    crate::set(vm, a, "bIgnoreOutOfWorld", Value::Bool(true));
    crate::set(vm, a, "bHidden", Value::Bool(false));
    crate::set(vm, a, "Physics", Value::Byte(crate::physics::PHYS_FLYING));
    crate::set(vm, a, "Acceleration", v3(Vec3::ZERO));
}

/// Puts the rider in a flying pose, and says which animation it was. Looping and on the main channel, so the
/// engine's own movement animations (made for running) do not fight it.
fn play_flying(vm: &mut Vm<'_>, a: ObjId, verbose: bool) -> Option<&'static str> {
    let set = crate::anim::set_of(vm, a)?;
    let found = FLY_ANIMS.iter().find(|n| set.find(n).is_some()).copied();
    if verbose && crate::envc::var_os("HP3_QUID_DEBUG").is_some() {
        // Which of a rider's animations might be a better flying pose than the one picked, to add to
        // `FLY_ANIMS`. It exists to read the retail sets, not for the game.
        let names: Vec<&str> = set.sequences.iter().map(|s| s.name.as_str()).collect();
        println!("quidditch: a rider flies in {found:?}; his animation set holds {names:?}");
    }
    let a_name = vm.db.names.intern(found?);
    crate::anim::play(vm, a, a_name, 1.0, 0.2, 0, true);
    found
}

/// Gives every agent a rider and puts the ball in play. Called at the end of `glue::start`, after `ball::start`
/// (the kickoff hands the Quaffle to a Chaser, so the ball has to exist).
pub fn start(vm: &mut Vm<'_>, m: &mut QuidditchMatch) {
    let mut pools = [placed(vm, Team::Home), placed(vm, Team::Away)];
    let (mut borrowed, mut spawned, mut anim) = (0usize, 0usize, None);
    let agents = m.teams.play.agents.clone();
    m.teams.pawns = Vec::with_capacity(agents.len());
    for a in &agents {
        let pool = &mut pools[usize::from(!a.team.is_home())];
        let pawn = match pool.pop() {
            Some(p) => {
                m.teams.borrowed.push(Borrowed {
                    actor: p,
                    location: crate::rd_vec(vm, p, "Location"),
                    rotation: crate::rd_rot(vm, p, "Rotation"),
                    physics: crate::get(vm, p, "Physics").as_byte(),
                    hidden: crate::get(vm, p, "bHidden").as_bool(),
                });
                borrowed += 1;
                Some(p)
            }
            None => {
                let s = spawn_rider(vm, a.team, a.body.pos);
                if let Some(s) = s {
                    // Spawned for the match, so the match owns it: `glue::stop` destroys it with the rest.
                    m.actors.push(s);
                    spawned += 1;
                }
                s
            }
        };
        if let Some(p) = pawn {
            mount(vm, p);
            crate::set(vm, p, "Location", v3(arena::clamp_into(&m.arena, a.body.pos)));
            crate::set(vm, p, "Rotation", rot(camera::rotator(a.body.pitch, a.body.yaw)));
            let first = anim.is_none();
            anim = anim.or(play_flying(vm, p, first));
        }
        m.teams.pawns.push(pawn);
    }
    // Whoever is left over on the turf is put out of the way for the match and set back by `stop`.
    let mut benched = 0;
    for p in pools.into_iter().flatten() {
        m.teams.borrowed.push(Borrowed {
            actor: p,
            location: crate::rd_vec(vm, p, "Location"),
            rotation: crate::rd_rot(vm, p, "Rotation"),
            physics: crate::get(vm, p, "Physics").as_byte(),
            hidden: crate::get(vm, p, "bHidden").as_bool(),
        });
        crate::set(vm, p, "bHidden", Value::Bool(true));
        benched += 1;
    }
    m.teams.anim = anim;
    let kick = m.teams.play.kickoff(&mut m.balls.quaffle, arena::bludger_home(&m.arena));
    let line = format!(
        "quidditch: {} AI agents on {} riders ({borrowed} of the level's own, {spawned} spawned, {benched} left out), flying in {:?}; {:?} kick off with the Quaffle",
        agents.len(),
        m.teams.riders(),
        m.teams.anim,
        kick,
    );
    println!("{line}");
    vm.log.push(line);
}

/// One frame of both teams: the pure play, the Bludgers its Beaters swatted, and the riders' `Location`.
pub fn tick(vm: &mut Vm<'_>, m: &mut QuidditchMatch, dt: f32) {
    let debug = crate::envc::var_os("HP3_QUID_DEBUG").is_some();
    let started = debug.then(std::time::Instant::now);
    let bludgers = m.bludgers.flying();
    let (harry, snitch) = (m.broom.pos, m.snitch.pos());
    let events = m.teams.play.step(
        &m.arena,
        &m.balls.hoops,
        &mut m.balls.quaffle,
        &bludgers,
        harry,
        snitch,
        &m.budget,
        dt,
    );
    for e in &events {
        if let TeamEvent::Beat { index, dir } = e {
            m.bludgers.redirect(*index, *dir, ai::BEAT_SPEED);
        }
    }
    // The riders. Each `Location` is the clamped one, so nothing an agent does can put a pawn outside the
    // arena, let alone out of the world.
    for (i, a) in m.teams.play.agents.iter().enumerate() {
        let Some(p) = m.teams.pawns.get(i).copied().flatten().filter(|&p| !vm.db.objects.get(p).deleted) else { continue };
        crate::set(vm, p, "Location", v3(arena::clamp_into(&m.arena, a.body.pos)));
        crate::set(vm, p, "Rotation", rot(camera::rotator(a.body.pitch, a.body.yaw)));
        crate::set(vm, p, "Velocity", v3(a.body.vel));
        crate::set(vm, p, "Acceleration", v3(Vec3::ZERO));
        crate::set(vm, p, "Physics", Value::Byte(crate::physics::PHYS_FLYING));
    }
    if let Some(started) = started {
        // The elapsed time is taken before anything is printed, or the measurement would be of the printing.
        m.teams.cost += started.elapsed();
        m.teams.measured += 1;
        for e in &events {
            println!("quidditch: tick {} team play {e:?}", m.frames);
        }
        if m.teams.measured % 120 == 0 {
            let mean = m.teams.cost.as_secs_f64() * 1000.0 / m.teams.measured as f64;
            let active = ai::active_agents(&m.teams.play.agents, &m.budget).len();
            println!(
                "quidditch: team play {:.3} ms a tick over {} ticks ({} agents, {active} of them fully steered, budget {})",
                mean,
                m.teams.measured,
                m.teams.play.agents.len(),
                m.budget.max_active_ai,
            );
        }
    }
}

/// A goal has just gone in: the Quaffle is back at the middle of the pitch, and the side that conceded takes
/// the restart with it, as the real game does. Without this the pitch is symmetric, every loose ball goes to
/// whichever side happens to be a hair nearer, and one team can take every one of them all match.
pub fn restart(m: &mut QuidditchMatch, home_scored: bool) {
    let at = arena::bludger_home(&m.arena);
    let team = if home_scored { Team::Away } else { Team::Home };
    m.teams.play.hand_to(&mut m.balls.quaffle, team, at);
}

/// Sets the level's own players back down where they stood. Called from `glue::stop` before the match's own
/// actors are destroyed; the riders the match spawned are in its `actors` and go with them.
pub fn stop(vm: &mut Vm<'_>, m: &QuidditchMatch) {
    let mut back = 0;
    for b in &m.teams.borrowed {
        if vm.db.objects.get(b.actor).deleted {
            continue;
        }
        crate::set(vm, b.actor, "Location", v3(b.location));
        crate::set(vm, b.actor, "Rotation", rot(b.rotation));
        crate::set(vm, b.actor, "Velocity", v3(Vec3::ZERO));
        crate::set(vm, b.actor, "Physics", Value::Byte(b.physics));
        crate::set(vm, b.actor, "bHidden", Value::Bool(b.hidden));
        crate::set(vm, b.actor, "bCollideWorld", Value::Bool(true));
        crate::set(vm, b.actor, "bIgnoreOutOfWorld", Value::Bool(false));
        back += 1;
    }
    if back > 0 {
        let line = format!("quidditch: {back} of the level's own players set back on the turf");
        println!("{line}");
        vm.log.push(line);
    }
}
