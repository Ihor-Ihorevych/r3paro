//! The script-VM side of a Quidditch match: builds the arena inside the loaded level, spawns the match's
//! actors, ticks the pure systems and puts their positions back onto the actors.
//!
//! This is the only Quidditch file that touches the VM; everything it drives (`flight`, `snitch`, `bludger`,
//! `rules`, `arena`) is pure Rust, unit-tested on its own. The match state lives in `World::quidditch`, so it
//! goes away with the level and a map that never starts a match pays nothing but one `is_none` check a frame.
//!
//! Flight control, the camera and the input are Task 7; the HUD, sound and the hoop rings come later. What is
//! here is the arena layout and start / stop / tick.

use glam::Vec3;

use hp3_script::{ObjId, Value, Vm, VmError};

use super::arena::{self, ArenaBounds, ArenaPiece};
use super::bludger::Bludgers;
use super::flight::BroomState;
use super::rules::{MatchPhase, Rules};
use super::snitch::Snitch;

/// Harry's broom pawn (`HGame.HarryQuid`) and the Golden Snitch (`HGame.Snitch`): both classes exist in the
/// retail scripts with empty bodies, which is why the engine drives them.
const HARRY_CLASS: &str = "HGame.HarryQuid";
const SNITCH_CLASS: &str = "HGame.Snitch";
/// What the stadium pieces are spawned as.
const MESH_ACTOR_CLASS: &str = "Engine.StaticMeshActor";

/// A match in progress: the pure systems, the arena they play in, and the actors that show them.
pub struct QuidditchMatch {
    /// Whom Gryffindor is playing (`HP3_MATCH=<opponent>`).
    pub opponent: String,
    pub arena: ArenaBounds,
    /// Harry's flight state. Task 7 steers it; until then it follows his pawn's `Location`.
    pub broom: BroomState,
    pub snitch: Snitch,
    pub bludgers: Bludgers,
    pub rules: Rules,
    /// Every actor `start` made, in the order it made them. `stop` destroys all of them.
    pub actors: Vec<ObjId>,
    pub harry: Option<ObjId>,
    pub snitch_actor: Option<ObjId>,
    /// Ticks this match has had.
    frames: u64,
    /// `HP3_MATCH_RESTART=<ticks>` (debugging): start the match over after that many ticks, which takes the
    /// stop-then-start path. Nothing in the game sets it.
    restart_at: Option<u64>,
    /// Whether the end of the match has been reported.
    reported: bool,
}

fn v3(v: Vec3) -> Value {
    Value::Struct(vec![Value::Float(v.x), Value::Float(v.y), Value::Float(v.z)].into())
}

fn rot(r: [i32; 3]) -> Value {
    Value::Struct(vec![Value::Int(r[0]), Value::Int(r[1]), Value::Int(r[2])].into())
}

/// The seed the match's randomness starts from: `HP3_MATCH_SEED`, else the opponent's name, so a given
/// fixture always plays the same way (the headless smoke run needs that).
fn seed_for(opponent: &str) -> u64 {
    if let Some(s) = crate::envc::var("HP3_MATCH_SEED").ok().and_then(|v| v.parse().ok()) {
        return s;
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in opponent.as_bytes() {
        h ^= b.to_ascii_lowercase() as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// Spawns one stadium mesh. The actor is born `bStatic` (as placed scenery is), so it is never ticked;
/// `stop` clears the flag to be able to destroy it again.
fn spawn_piece(vm: &mut Vm<'_>, class: ObjId, p: &ArenaPiece) -> Option<ObjId> {
    let mesh = crate::find_object_of(vm, p.mesh, Some("StaticMesh"));
    if mesh.is_none() {
        vm.log.push(format!("quidditch: static mesh {} not found", p.mesh));
        return None;
    }
    let args = [Value::Object(Some(class)), Value::Object(None), Value::Empty, v3(p.pos), rot(p.rot)];
    let a = crate::natives::spawn(vm, None, &args).ok()?.as_obj()?;
    crate::set(vm, a, "StaticMesh", Value::Object(mesh));
    if p.scale != Vec3::ONE {
        crate::set(vm, a, "DrawScale3D", v3(p.scale));
    }
    Some(a)
}

/// Spawns an actor of `path`'s class at `pos` facing `yaw`.
fn spawn_class(vm: &mut Vm<'_>, path: &str, pos: Vec3, yaw: i32) -> Option<ObjId> {
    let class = crate::find_object(vm, path);
    if class.is_none() {
        vm.log.push(format!("quidditch: class {path} not found"));
        return None;
    }
    let args = [Value::Object(class), Value::Object(None), Value::Empty, v3(pos), rot([0, yaw, 0])];
    crate::natives::spawn(vm, None, &args).ok()?.as_obj()
}

/// Logs where the engine's own rotation maths puts each goal's three hoops, which is what the arena doc's
/// hoop table claims. Only with `HP3_QUID_DEBUG` set: it exists to check the doc, not for the game.
fn log_hoops(pieces: &[ArenaPiece]) {
    if crate::envc::var_os("HP3_QUID_DEBUG").is_none() {
        return;
    }
    for p in pieces.iter().filter(|p| p.mesh == arena::GOAL_POSTS) {
        let axes = hp3_script::natives::rot_axes(p.rot);
        let (y, z) = (Vec3::from(axes[1]), Vec3::from(axes[2]));
        let hoops: Vec<String> = arena::HOOPS_LOCAL
            .iter()
            .map(|&(ly, lz)| {
                let w = p.pos + y * ly + z * lz;
                format!("({:.0}, {:.0}, {:.0})", w.x, w.y, w.z)
            })
            .collect();
        println!("quidditch: goal at ({:.0}, {:.0}, {:.0}) yaw {} hoops {}", p.pos.x, p.pos.y, p.pos.z, p.rot[1], hoops.join(" "));
    }
}

/// Builds the arena in the loaded level and starts a match against `opponent`. Called again while a match is
/// running, it stops that one first, so it is safe to re-enter.
pub fn start(vm: &mut Vm<'_>, opponent: &str) -> Result<(), VmError> {
    let re_entry = crate::world(vm).quidditch.is_some();
    if re_entry {
        stop(vm);
    }
    let bounds = arena::bounds();
    let seed = seed_for(opponent);
    let mut actors: Vec<ObjId> = Vec::new();
    let pieces = arena::arena_spawn_list();
    log_hoops(&pieces);
    match crate::find_object(vm, MESH_ACTOR_CLASS) {
        Some(class) => {
            for p in &pieces {
                if let Some(a) = spawn_piece(vm, class, p) {
                    actors.push(a);
                }
            }
        }
        None => vm.log.push(format!("quidditch: class {MESH_ACTOR_CLASS} not found, no stadium")),
    }
    // Harry's broom pawn starts over his own goal line. It flies (nothing holds a broom up), but nothing
    // steers it yet: Task 7 gives it the flight model and the camera, and possesses it.
    let start_at = arena::harry_start(&bounds);
    let harry = spawn_class(vm, HARRY_CLASS, start_at, 16384);
    if let Some(h) = harry {
        crate::set(vm, h, "Physics", Value::Byte(crate::physics::PHYS_FLYING));
        actors.push(h);
    }
    // The Snitch actor is only a marker the pure snitch drives; `tick` writes its `Location` every frame.
    let snitch = Snitch::new(seed, &bounds);
    let snitch_actor = spawn_class(vm, SNITCH_CLASS, arena::actor_location(&bounds, snitch.pos()), 0);
    if let Some(s) = snitch_actor {
        crate::set(vm, s, "Physics", Value::Byte(crate::physics::PHYS_NONE));
        actors.push(s);
    }
    // A different stream than the snitch's, from the same match seed.
    let mut bludgers = Bludgers::new(seed ^ 0x51ad_9e12_b10c_c3a7);
    bludgers.set_home(arena::bludger_home(&bounds));
    let mut rules = Rules::new();
    rules.start();
    let m = QuidditchMatch {
        opponent: opponent.to_string(),
        arena: bounds,
        broom: BroomState { pos: start_at, vel: Vec3::ZERO, yaw: std::f32::consts::FRAC_PI_2, pitch: 0.0 },
        snitch,
        bludgers,
        rules,
        actors,
        harry,
        snitch_actor,
        frames: 0,
        // The restart is a one-shot: the match it starts does not restart itself again.
        restart_at: (!re_entry).then(|| crate::envc::var("HP3_MATCH_RESTART").ok().and_then(|v| v.parse().ok()).filter(|&n| n > 0)).flatten(),
        reported: false,
    };
    let line = format!(
        "quidditch: match against {} started, phase {:?}, {} actors spawned ({} stadium pieces, Harry {}, Snitch {})",
        m.opponent,
        m.rules.phase(),
        m.actors.len(),
        pieces.len(),
        if m.harry.is_some() { "yes" } else { "MISSING" },
        if m.snitch_actor.is_some() { "yes" } else { "MISSING" },
    );
    println!("{line}");
    vm.log.push(line);
    crate::world(vm).quidditch = Some(m);
    Ok(())
}

/// Ends the match: every actor `start` made is destroyed and the state cleared. Does nothing when no match
/// is running.
pub fn stop(vm: &mut Vm<'_>) {
    let Some(m) = crate::world(vm).quidditch.take() else { return };
    let mut gone = 0;
    for a in &m.actors {
        // Scenery is born `bStatic`, which `destroy_actor` refuses to remove: clear the flag first.
        crate::set(vm, *a, "bStatic", Value::Bool(false));
        crate::set(vm, *a, "bNoDelete", Value::Bool(false));
        if crate::destroy_actor(vm, *a) {
            gone += 1;
        }
    }
    let line = format!(
        "quidditch: match against {} stopped, phase {:?}, score {}-{}, {gone} of {} actors destroyed",
        m.opponent,
        m.rules.phase(),
        m.rules.score().home,
        m.rules.score().away,
        m.actors.len()
    );
    println!("{line}");
    vm.log.push(line);
}

/// One frame of the match. A no-op while none is running, which is every level but a Quidditch one.
pub fn tick(vm: &mut Vm<'_>, dt: f32) {
    if crate::world(vm).quidditch.is_none() {
        return;
    }
    // Taken out and put back: the pure systems are stepped while the world is borrowed elsewhere.
    let Some(mut m) = crate::world(vm).quidditch.take() else { return };
    m.frames += 1;
    if m.restart_at == Some(m.frames) {
        // Debug only: `start` stops the running match first, so this exercises the whole round trip.
        let opponent = m.opponent.clone();
        crate::world(vm).quidditch = Some(m);
        let _ = start(vm, &opponent);
        return;
    }
    // Until Task 7 steers the broom, Harry's flight state follows his pawn.
    if let Some(h) = m.harry.filter(|&h| !vm.db.objects.get(h).deleted) {
        let was = m.broom.pos;
        m.broom.pos = crate::rd_vec(vm, h, "Location");
        if dt > 0.0 {
            m.broom.vel = (m.broom.pos - was) / dt;
        }
    }
    let ended = matches!(m.rules.phase(), MatchPhase::Ended(_));
    if !ended && m.rules.phase() != MatchPhase::Intro {
        for e in m.snitch.step(&m.broom, false, dt) {
            m.rules.on_snitch(e);
        }
        let hits = m.bludgers.step(&m.broom, dt);
        let lost = m.bludgers.control_lost();
        for e in hits {
            m.rules.on_bludger(e, lost);
        }
    }
    if let Some(s) = m.snitch_actor.filter(|&s| !vm.db.objects.get(s).deleted) {
        let at = arena::actor_location(&m.arena, m.snitch.pos());
        crate::set(vm, s, "Location", v3(at));
    }
    // `HP3_QUID_DEBUG`: a sample of the match state now and then, to see that it is running at all.
    if m.frames % 120 == 0 && crate::envc::var_os("HP3_QUID_DEBUG").is_some() {
        let a = m.snitch_actor.map(|s| crate::rd_vec(vm, s, "Location")).unwrap_or(Vec3::ZERO);
        println!(
            "quidditch: tick {} phase {:?} snitch ({:.0}, {:.0}, {:.0}) actor ({:.0}, {:.0}, {:.0}) progress {:.2} bumps {}",
            m.frames,
            m.rules.phase(),
            m.snitch.pos().x, m.snitch.pos().y, m.snitch.pos().z,
            a.x, a.y, a.z,
            m.snitch.progress(),
            m.bludgers.bumps()
        );
    }
    if !m.reported
        && let MatchPhase::Ended(outcome) = m.rules.phase()
    {
        m.reported = true;
        let score = m.rules.score();
        let line = format!("quidditch: match against {} ended {outcome:?}, {}-{}", m.opponent, score.home, score.away);
        println!("{line}");
        vm.log.push(line);
    }
    crate::world(vm).quidditch = Some(m);
}

/// `HP3_MATCH=<opponent>` (debugging): starts a match as soon as a level has finished loading. Called by
/// `load_with` once the player exists.
pub fn start_from_env(vm: &mut Vm<'_>) -> Result<(), VmError> {
    let Ok(opponent) = crate::envc::var("HP3_MATCH") else { return Ok(()) };
    let opponent = opponent.trim();
    if opponent.is_empty() || opponent.eq_ignore_ascii_case("none") {
        return Ok(());
    }
    if crate::world(vm).player.is_none() {
        vm.log.push("quidditch: HP3_MATCH set but the level has no player, no match started".into());
        return Ok(());
    }
    start(vm, opponent)
}
