//! The script-VM side of a Quidditch match: builds the arena inside the loaded level, spawns the match's
//! actors, ticks the pure systems and puts their positions back onto the actors.
//!
//! This is the only Quidditch file that touches the VM; everything it drives (`flight`, `snitch`, `bludger`,
//! `rules`, `arena`) is pure Rust, unit-tested on its own. The match state lives in `World::quidditch`, so it
//! goes away with the level and a map that never starts a match pays nothing but one `take()` of a `None` a
//! frame. `World::quidditch_gate` keeps `start` and `stop` honest when they are called from inside `tick`.
//!
//! What is here is the arena layout, start / stop / tick, the flight of Harry's broom (steered by `flight`
//! from the keys `crate::input::apply` hands over) and the chase camera `crate::player_view` shows it from.
//! The HUD and the sounds are `presentation`'s, the Quaffle and the hoops `ball`'s.

use glam::Vec3;

use hp3_script::{ObjId, Value, Vm, VmError};

use super::arena::{self, ArenaBounds, ArenaPiece};
use super::ball::{self, Balls};
use super::bludger::Bludgers;
use super::camera::{self, CamState};
use super::flight::{self, BroomInput, BroomState, BroomTuning};
use super::input::{desktop_input, KeyState};
use super::momentum::Momentum;
use super::presentation::{self, Presentation};
use super::rings::{self, TrailRings};
use super::rules::{MatchEvent, MatchPhase, Rules};
use super::snitch::{Snitch, SnitchEvent, SnitchTuning};
use super::team::{self, Teams};

/// Harry's broom pawn (`HGame.HarryQuid`) and the Golden Snitch (`HGame.Snitch`): both classes exist in the
/// retail scripts with empty bodies, which is why the engine drives them.
const HARRY_CLASS: &str = "HGame.HarryQuid";
const SNITCH_CLASS: &str = "HGame.Snitch";
/// What the stadium pieces are spawned as.
const MESH_ACTOR_CLASS: &str = "Engine.StaticMeshActor";
//
// **Harry rides no separate broom actor.** The retail `HarryQuid` skeletal mesh already carries its own
// broom, modelled into the mesh and posed by the `hover` animation. An earlier pass also hung the static
// mesh `HP3_InteriorAccents_SM.miscDetails.QuiddichBroom_SM` off the skeleton's `body_broom` bone, which gave
// him two: the modelled one under him and the static one sticking out sideways (the bone's rest orientation
// is not the broom's). Attaching nothing is the fix; see `docs/formats/quidditch-arena.md`.
/// How far inside the arena walls the broom starts steering itself back in (uu): the soft boundary. The hard
/// one is `flight::confine`, which the broom reaches only by flying at a wall through this band.
const SOFT_MARGIN: f32 = 900.0;
/// How much of the player's full stick deflection the soft boundary may add (it is added to his own input, so
/// he can still fight it, but not win while he faces the wall).
const BOUNDARY_WEIGHT: f32 = 0.8;
/// Animations the rider is put in while he flies, best first: the first one his animation set has wins. The
/// retail `HarryQuid` set holds `broommount`, `hover`, `idle`, `flytostop`, `lookaround`, `reach`,
/// `reachloop`, `lookdown`, `fall`, `falllook` and `stand`; `hover` is the one that sits on a broom, and the
/// rest are fallbacks in case another mesh is used for the rider. A set with none of them plays nothing.
const FLY_ANIMS: [&str; 5] = ["Hover", "QuidFly", "Fly", "Idle", "Stand"];
/// How long the broom spirals out of the player's hands once the Bludgers have hit him often enough
/// (`BludgerTuning::bumps_to_lose_control`), in seconds. A placeholder, tuned by feel like the rest.
const SPIN_TIME: f32 = 2.0;

/// Whether Harry reaches for the Snitch by himself while the catch window is open. Off: the catch is the
/// player's (Space on the desktop, the JUMP button on the phone). `HP3_QUID_AUTOCATCH` turns it on, for
/// headless runs.
fn auto_catch() -> bool {
    crate::envc::var_os("HP3_QUID_AUTOCATCH").is_some()
}

/// A match in progress: the pure systems, the arena they play in, and the actors that show them.
pub struct QuidditchMatch {
    /// Whom Gryffindor is playing (`HP3_MATCH=<opponent>`).
    pub opponent: String,
    pub arena: ArenaBounds,
    /// Harry's flight state: what the player steers, and what his pawn's `Location` is written from.
    pub broom: BroomState,
    /// How the broom flies (tuned by feel in a later task).
    pub tuning: BroomTuning,
    /// The chase camera `crate::player_view` hands the renderer while the match runs.
    pub cam: CamState,
    pub snitch: Snitch,
    pub bludgers: Bludgers,
    /// The Quaffle and the six scoring hoops (`quidditch::ball`). The Chasers of `teams` throw it; the debug
    /// `HP3_QUID_SHOOT` still does too.
    pub balls: Balls,
    /// The flight rings the Snitch leaves behind it and the actors that show them (`quidditch::trail`,
    /// `quidditch::rings`): Harry's road to the Snitch, and the run of rings his catch needs.
    pub rings: TrailRings,
    /// Both sides' AI flyers and the Quaffle play between them (`quidditch::ai`, `quidditch::team`).
    pub teams: Teams,
    pub rules: Rules,
    /// Every actor `start` made, in the order it made them. `stop` destroys all of them.
    pub actors: Vec<ObjId>,
    pub harry: Option<ObjId>,
    pub snitch_actor: Option<ObjId>,
    /// What the match looks, sounds and reads like (`quidditch::presentation`): the Bludger balls, the
    /// sounds, the commentary subtitle and the HUD.
    pub show: Presentation,
    /// This frame's broom keys and mouse motion, as `crate::input::apply` read them (`feed_input`). Taken by
    /// the tick, so a frame whose input never arrived steers nothing instead of repeating the last one.
    pending_input: Option<(KeyState, (f32, f32))>,
    /// This frame's steering from the phone's touch layout (`phone::feed_touch`), already a `BroomInput`
    /// because the touch controls are a scheme of their own. Set, it is flown instead of the keys and mouse,
    /// and it is taken by the tick for the same reason as `pending_input`.
    pub(super) pending_touch: Option<flight::BroomInput>,
    /// Harry's speed: there is no boost or brake, a run of flight rings charges it up and it drains away on
    /// its own (`quidditch::momentum`).
    pub(super) momentum: Momentum,
    /// A tap of the phone's JUMP button no frame has taken yet (`phone::feed_touch`).
    pub(super) pending_reach: bool,
    /// Seconds left of the reach a tap opened: the catch is wanted while it lasts.
    reach_left: f32,
    /// 1 right after a ring was taken, fading to 0: the HUD's pulse.
    pub(super) charge_glow: f32,
    /// Set once the Snitch has been caught (and by whom the match is won is the score's business).
    pub(super) caught: bool,
    /// Whether a touch layout has ever steered this match (the HUD then names the JUMP button, not Space).
    pub(super) touch_seen: bool,
    /// The finish: the slow camera orbit round Harry once the match is over.
    pub(super) finale: Option<Finale>,
    /// What this match may spend on a frame, as the app's frame times ask for (`phone::feed_frame_ms`,
    /// `quidditch::lod`). Nothing steers team players yet (Task 13); what keeps to it today is the
    /// presentation, which leaves distant Bludgers to their proxy.
    pub budget: super::lod::Budget,
    /// The 120-frame count behind `budget`.
    pub(super) gov: super::lod::BudgetGovernor,
    /// The pawn the player controller had before the match, given back by `stop`.
    was_playing: Option<ObjId>,
    /// Ticks this match has had.
    pub(super) frames: u64,
    /// `HP3_MATCH_RESTART=<ticks>` (debugging): start the match over after that many ticks, which takes the
    /// stop-then-start path. Nothing in the game sets it.
    restart_at: Option<u64>,
    /// Whether the end of the match has been reported.
    reported: bool,
    /// Seconds left of the spiral the Bludgers knocked the broom into (`SPIN_TIME`); 0 while he has control.
    spin: f32,
    /// Whether the match was started from the front end (`menu::request`) rather than by hand
    /// (`HP3_MATCH=<opponent>`). Only such a match writes its result into the league and takes the player back
    /// to the menu when it is over; a debug match is left running, as it always was.
    pub(super) from_menu: bool,
    /// The tick a finished match hands the player back to the front end on: the result line stays up until
    /// then (`menu::LINGER_TICKS`). Only ever set for a `from_menu` match.
    leave_at: Option<u64>,
    /// `HP3_QUID_EASY=1` (debugging, off by default): the snitch is caught the moment Harry is anywhere near
    /// it and the catch key is held for him, so a headless smoke run reaches a result in a few hundred ticks.
    /// Nothing in the game sets it.
    easy: bool,
}

/// The end of a match: Harry glides to a hover and the camera orbits him. `t` is seconds since it began.
#[derive(Debug, Clone, Copy)]
pub(super) struct Finale {
    pub t: f32,
    /// The chase camera as it was at the end, which the orbit grows out of.
    pub from: CamState,
}

/// How long a tap of reach keeps the catch wanted (s).
const REACH_TIME: f32 = 0.4;
/// The slow motion of the catch: the broom's time runs at this at first and eases back to 1 over `SLOWMO_TIME`.
const SLOWMO: f32 = 0.35;
const SLOWMO_TIME: f32 = 0.8;

/// The snitch of an `HP3_QUID_EASY` run: the bar fills from anywhere on the pitch and the catch window is
/// wide open, so the match ends on its own without anyone at the keyboard. The gain rate is deliberately slow
/// enough (about 12 seconds) that the Bludgers, the commentary and the HUD all get a turn first. Only
/// reachable through the environment variable.
fn easy_snitch_tuning() -> SnitchTuning {
    SnitchTuning {
        max_gain_radius: 20000.0,
        neutral_radius: 20000.0,
        max_loss_radius: 40000.0,
        gain_rate: 0.08,
        catch_radius: 20000.0,
        ..SnitchTuning::default()
    }
}

/// A `start` or `stop` that was asked for while a tick was in progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Start(String),
    Stop,
}

/// Keeps `start` and `stop` out of the window in which `tick` holds the match in a local and
/// `World::quidditch` is therefore `None`. Called in that window, a `stop` would destroy nothing (the actors
/// would leak and the match would never end) and a `start` would install a match the tick's put-back then
/// overwrites, orphaning its actors. So a request made inside the window is recorded instead, and `tick`
/// applies it once the match is back in the world.
///
/// One slot, last request of the tick wins: that is not a loss, because a `start` stops whatever is running
/// anyway, and a `stop` after a `start` means the caller ended up wanting no match.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Gate {
    ticking: bool,
    pending: Option<Request>,
}

impl Gate {
    /// Whether a tick is holding the match out of the world right now.
    pub fn ticking(&self) -> bool {
        self.ticking
    }

    /// Records `r` and answers whether it has to wait for the end of the tick.
    pub fn defer(&mut self, r: Request) -> bool {
        if !self.ticking {
            return false;
        }
        self.pending = Some(r);
        true
    }

    /// Opens the window.
    pub fn begin(&mut self) {
        self.ticking = true;
    }

    /// Closes the window and hands back the request to run now, if one was made.
    pub fn end(&mut self) -> Option<Request> {
        self.ticking = false;
        self.pending.take()
    }
}

fn v3(v: Vec3) -> Value {
    Value::Struct(vec![Value::Float(v.x), Value::Float(v.y), Value::Float(v.z)].into())
}

fn rot(r: [i32; 3]) -> Value {
    Value::Struct(vec![Value::Int(r[0]), Value::Int(r[1]), Value::Int(r[2])].into())
}

thread_local! {
    /// A seed the headless smoke driver (`hp3-run quid-smoke`) picks per match; the environment is read once per
    /// thread, so it cannot vary the seed between matches. Never set by the game.
    static SEED_OVERRIDE: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

thread_local! {
    /// Whether `HP3_MATCH` has already started its match in this run (`start_from_env`).
    static ENV_MATCH_DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Pins (or, with `None`, releases) the seed of the next matches started on this thread. Debug driver only.
pub fn set_seed_override(seed: Option<u64>) {
    SEED_OVERRIDE.with(|c| c.set(seed));
}

/// Flies this frame's broom with `input` and holds the catch key if `catch` (a scripted pilot, for the headless
/// smoke run). The same slot the phone's touch layout uses; nothing in the game calls it.
pub fn feed_scripted(vm: &mut Vm<'_>, input: flight::BroomInput, catch: bool) {
    if let Some(m) = crate::world(vm).quidditch.as_mut() {
        m.pending_touch = Some(input);
        m.pending_input = Some((KeyState { catch, ..KeyState::default() }, (0.0, 0.0)));
    }
}

/// The seed the match's randomness starts from: `HP3_MATCH_SEED`, else the opponent's name, so a given
/// fixture always plays the same way (the headless smoke run needs that).
fn seed_for(opponent: &str) -> u64 {
    if let Some(s) = SEED_OVERRIDE.with(|c| c.get()) {
        return s;
    }
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

/// Hands the player controller the broom pawn, so the player is Harry on his broom and the pawn that walked
/// into the level stops answering to him. Returns the pawn the controller had, which `stop` gives back.
///
/// This is the engine's own half of a possession (the two pointers that make a controller and a pawn a pair);
/// the game's `SwitchControlToPawn` is not used because it trades the controllers of two pawns that are both
/// in the level's cast, and the broom pawn is one the match spawned, with no controller of its own.
fn possess(vm: &mut Vm<'_>, harry: ObjId) -> Option<ObjId> {
    let pc = crate::world(vm).player?;
    let had = crate::get(vm, pc, "Pawn").as_obj();
    if had == Some(harry) {
        return had;
    }
    // The broom is flown by the match, not by the pawn physics: it collides with nothing (the stadium is
    // spawned after the level's collision was baked anyway) and `flight::confine` keeps it in the arena, so
    // no fall and no out-of-world check can ever reach it.
    crate::set(vm, harry, "bCollideWorld", Value::Bool(false));
    crate::set(vm, harry, "bIgnoreOutOfWorld", Value::Bool(true));
    crate::set(vm, harry, "bHidden", Value::Bool(false));
    // A controller and its pawn point at each other, so the pawn that was being played lets go of it first:
    // two pawns claiming the player would make every `Controller.Pawn` answer about the broom instead.
    if let Some(p) = had {
        crate::set(vm, p, "Controller", Value::Object(None));
    }
    crate::set(vm, harry, "Controller", Value::Object(Some(pc)));
    crate::set(vm, pc, "Pawn", Value::Object(Some(harry)));
    had
}

/// What the controller is to play once the match lets go of the broom pawn, as a decision on its own because
/// it is the one thing that must never be got wrong: `stop` destroys the broom pawn straight afterwards and
/// nothing in the engine clears references to a destroyed actor, so the controller must not be left holding
/// it. `None` means "leave the controller alone" (something else took the player over meanwhile);
/// `Some(pawn)` is what to play, and `Some(None)` is no pawn at all, which is what a controller that had
/// none before the match goes back to.
fn give_back(playing: Option<ObjId>, broom: Option<ObjId>, had: Option<ObjId>, had_alive: bool) -> Option<Option<ObjId>> {
    if broom.is_some() && playing != broom {
        return None;
    }
    Some(had.filter(|p| had_alive && Some(*p) != broom))
}

/// Gives the controller back the pawn it was playing before the match and takes the broom pawn off it, so
/// nothing points at the pawn `stop` is about to destroy. `harry` is the broom pawn, `had` the pawn the match
/// took the controller from.
fn unpossess(vm: &mut Vm<'_>, had: Option<ObjId>, harry: Option<ObjId>) {
    let Some(pc) = crate::world(vm).player else { return };
    let playing = crate::get(vm, pc, "Pawn").as_obj();
    let alive = had.is_some_and(|p| !vm.db.objects.get(p).deleted);
    // The broom pawn lets go of the controller whoever is playing now.
    if let Some(h) = harry.filter(|&h| crate::get(vm, h, "Controller").as_obj() == Some(pc)) {
        crate::set(vm, h, "Controller", Value::Object(None));
    }
    let Some(back) = give_back(playing, harry, had, alive) else {
        vm.log.push("quidditch: the player was taken over during the match, the broom pawn was only let go".into());
        return;
    };
    crate::set(vm, pc, "Pawn", Value::Object(back));
    if let Some(p) = back {
        crate::set(vm, p, "Controller", Value::Object(Some(pc)));
    }
}

/// `HP3_QUID_DEBUG`: who the player plays now the match is over and its actors are gone. It is printed after
/// the broom pawn has been destroyed, because that is when the answer matters: the controller must hold the
/// pawn it held before the match (or none), never the deleted one.
fn log_handover(vm: &mut Vm<'_>, harry: Option<ObjId>) {
    if crate::envc::var_os("HP3_QUID_DEBUG").is_none() {
        return;
    }
    let Some(pc) = crate::world(vm).player else { return };
    let pawn = crate::get(vm, pc, "Pawn").as_obj();
    let its_controller = pawn.and_then(|p| crate::get(vm, p, "Controller").as_obj());
    let gone = pawn.is_some_and(|p| vm.db.objects.get(p).deleted);
    let broom_gone = harry.is_some_and(|h| vm.db.objects.get(h).deleted);
    let name = |o: Option<ObjId>| o.map(|o| hp3_script::natives::object_path(vm, o)).unwrap_or_else(|| "none".into());
    println!(
        "quidditch: after the stop the player plays {} (deleted {gone}), whose controller is {} (the player controller is {}); the broom pawn {} is deleted {broom_gone}",
        name(pawn),
        name(its_controller),
        name(Some(pc)),
        name(harry),
    );
}

/// Puts the rider in a flying pose, if his animation set has one of `FLY_ANIMS`. Looping and on the main
/// channel, so the engine's own movement animations (made for running) do not fight it.
fn play_flying(vm: &mut Vm<'_>, harry: ObjId) {
    let Some(set) = crate::anim::set_of(vm, harry) else {
        vm.log.push("quidditch: Harry has no animation set, he rides in his rest pose".into());
        return;
    };
    let found = FLY_ANIMS.iter().find(|a| set.find(a).is_some()).copied();
    if crate::envc::var_os("HP3_QUID_DEBUG").is_some() {
        // Which of the rider's animations might be a better flying pose than the one picked, to add to
        // `FLY_ANIMS`. It exists to read the retail set, not for the game.
        let names: Vec<&str> = set.sequences.iter().map(|s| s.name.as_str()).take(40).collect();
        println!("quidditch: Harry rides in {found:?}; his animation set holds {names:?}");
    }
    match found {
        Some(a) => {
            let name = vm.db.names.intern(a);
            crate::anim::play(vm, harry, name, 1.0, 0.2, 0, true);
            vm.log.push(format!("quidditch: Harry flies in {a}"));
        }
        None => vm.log.push(format!("quidditch: no animation of {FLY_ANIMS:?} for the rider")),
    }
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

/// `HP3_QUID_START=x,y,z` (debugging, off by default): where Harry's broom starts instead of his own goal
/// line, brought inside the flight volume. It is how a headless frame is taken from somewhere other than the
/// kickoff -- in front of a goal, say. Nothing in the game sets it.
fn start_override(b: &ArenaBounds) -> Option<Vec3> {
    let v = crate::envc::var("HP3_QUID_START").ok()?;
    let n: Vec<f32> = v.split(',').filter_map(|p| p.trim().parse::<f32>().ok()).filter(|f| f.is_finite()).collect();
    let p = (n.len() == 3).then(|| Vec3::new(n[0], n[1], n[2]))?;
    Some(arena::clamp_into(b, p))
}

/// Builds the arena in the loaded level and starts a match against `opponent`. Called again while a match is
/// running, it stops that one first, so it is safe to re-enter. Called from inside `tick` (where the match is
/// out of the world) it is recorded and run at the end of that tick instead.
pub fn start(vm: &mut Vm<'_>, opponent: &str) -> Result<(), VmError> {
    if crate::world(vm).quidditch_gate.defer(Request::Start(opponent.to_string())) {
        vm.log.push(format!("quidditch: start against {opponent} asked for during the tick, deferred to its end"));
        return Ok(());
    }
    let re_entry = crate::world(vm).quidditch.is_some();
    if re_entry {
        stop(vm);
    }
    // The level's own gameplay first: the retail Quidditch map is the Dementor cutscene's stage, and that
    // cutscene would take the camera and end the match by switching the level (`stage`).
    let cleared = super::stage::clear(vm);
    println!("{cleared}");
    vm.log.push(cleared);
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
    // Harry's broom pawn starts over his own goal line, flying (nothing holds a broom up), steered by
    // `flight` from this frame's input, and played by the player: the controller possesses it.
    let start_at = match start_override(&bounds) {
        Some(p) => {
            vm.log.push(format!("quidditch: HP3_QUID_START: Harry starts at ({:.0}, {:.0}, {:.0})", p.x, p.y, p.z));
            p
        }
        None => arena::harry_start(&bounds),
    };
    let harry = spawn_class(vm, HARRY_CLASS, start_at, 16384);
    let mut was_playing = None;
    if let Some(h) = harry {
        crate::set(vm, h, "Physics", Value::Byte(crate::physics::PHYS_FLYING));
        actors.push(h);
        // No broom actor: the `HarryQuid` mesh has one of its own (see the note by `MESH_ACTOR_CLASS`).
        was_playing = possess(vm, h);
        play_flying(vm, h);
    }
    // The Snitch actor is driven by the pure snitch; `tick` writes its `Location` every frame and
    // `presentation::start` gives it its mesh and its loop.
    let easy = crate::envc::var_os("HP3_QUID_EASY").is_some();
    let snitch = if easy { Snitch::with_tuning(seed, &bounds, easy_snitch_tuning()) } else { Snitch::new(seed, &bounds) };
    let snitch_actor = spawn_class(vm, SNITCH_CLASS, arena::clamp_into(&bounds, snitch.pos()), 0);
    if let Some(s) = snitch_actor {
        crate::set(vm, s, "Physics", Value::Byte(crate::physics::PHYS_NONE));
        actors.push(s);
    }
    // A different stream than the snitch's, from the same match seed.
    let mut bludgers = Bludgers::new(seed ^ 0x51ad_9e12_b10c_c3a7);
    bludgers.set_home(arena::bludger_home(&bounds));
    let mut rules = Rules::new();
    rules.start();
    // He starts facing up the pitch (`yaw` of a quarter turn is +y, the far goal), as his pawn's spawn
    // rotation of 16384 does.
    let broom_state = BroomState::new(start_at, Vec3::ZERO, std::f32::consts::FRAC_PI_2, 0.0);
    let mut m = QuidditchMatch {
        opponent: opponent.to_string(),
        arena: bounds,
        broom: broom_state,
        tuning: BroomTuning::default(),
        cam: camera::behind(&broom_state),
        snitch,
        bludgers,
        balls: Balls::new(&bounds),
        rings: TrailRings::default(),
        // A third stream off the same match seed, so a given fixture always plays the same way.
        teams: Teams::new(seed ^ 0x2e11_a1be_9c04_77d1),
        rules,
        actors,
        harry,
        snitch_actor,
        show: Presentation::default(),
        pending_input: None,
        pending_touch: None,
        momentum: Momentum::default(),
        pending_reach: false,
        reach_left: 0.0,
        charge_glow: 0.0,
        caught: false,
        touch_seen: false,
        finale: None,
        budget: super::lod::Budget::default(),
        gov: super::lod::BudgetGovernor::default(),
        was_playing,
        frames: 0,
        // The restart is a one-shot: the match it starts does not restart itself again.
        restart_at: (!re_entry).then(|| crate::envc::var("HP3_MATCH_RESTART").ok().and_then(|v| v.parse().ok()).filter(|&n| n > 0)).flatten(),
        reported: false,
        spin: 0.0,
        // Set by `start_from_env` for a match the front end asked for; a `start` from anywhere else is a
        // debug match and changes no league.
        from_menu: false,
        leave_at: None,
        easy,
    };
    // Meshes, sounds and music, then the Quaffle and the hoop rings, then the Snitch's ring trail, then the
    // teams (whose kickoff hands the Quaffle to a Chaser, so it has to exist first). All of them add actors of
    // their own, so they run before the count below is printed.
    presentation::start(vm, &mut m, seed);
    ball::start(vm, &mut m);
    rings::start(vm, &mut m);
    team::start(vm, &mut m);
    let yes_no = |o: Option<ObjId>| if o.is_some() { "yes" } else { "MISSING" };
    let line = format!(
        "quidditch: match against {} started, phase {:?}, {} actors spawned ({} stadium pieces, Harry {} (broom in his own mesh), Snitch {}){}",
        m.opponent,
        m.rules.phase(),
        m.actors.len(),
        pieces.len(),
        yes_no(m.harry),
        yes_no(m.snitch_actor),
        if m.easy { ", HP3_QUID_EASY: the snitch catches itself" } else { "" },
    );
    println!("{line}");
    vm.log.push(line);
    crate::world(vm).quidditch = Some(m);
    Ok(())
}

/// Ends the match: every actor `start` made is destroyed and the state cleared. Does nothing when no match
/// is running. Called from inside `tick` it is recorded and run at the end of that tick instead.
pub fn stop(vm: &mut Vm<'_>) {
    if crate::world(vm).quidditch_gate.defer(Request::Stop) {
        vm.log.push("quidditch: stop asked for during the tick, deferred to its end".into());
        return;
    }
    let Some(m) = crate::world(vm).quidditch.take() else { return };
    // The music stops with the match; the ambient loops go with the actors that carry them. The level's own
    // players go back on the turf before anything is destroyed: they are the level's, not the match's.
    presentation::stop(vm, &m);
    team::stop(vm, &m);
    // The flight rings' emitters are spawned by the rings' own script and are not in `actors`.
    rings::stop(vm, &m);
    // The pawn goes back to the player before the broom pawn is destroyed: a controller must never be left
    // holding a deleted pawn.
    unpossess(vm, m.was_playing, m.harry);
    let mut gone = 0;
    // Backwards, so a child goes before whatever it rides on.
    for a in m.actors.iter().rev() {
        // Scenery is born `bStatic`, which `destroy_actor` refuses to remove: clear the flag first.
        crate::set(vm, *a, "bStatic", Value::Bool(false));
        crate::set(vm, *a, "bNoDelete", Value::Bool(false));
        // `destroy_actor` also says yes to an actor that was already gone (a level script got there first):
        // only the ones this stop removed are counted.
        let was_there = !vm.db.objects.get(*a).deleted;
        if crate::destroy_actor(vm, *a) && was_there {
            gone += 1;
        }
    }
    log_handover(vm, m.harry);
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
    // Taken out and put back: the pure systems are stepped while the world is borrowed elsewhere. The gate is
    // open for exactly that window, so a `start` or `stop` made inside it waits for the put-back.
    let Some(mut m) = crate::world(vm).quidditch.take() else { return };
    crate::world(vm).quidditch_gate.begin();
    // The frame itself is a function of its own, so nothing in it (an early return today or one added
    // tomorrow) can skip the put-back and the closing of the gate below.
    tick_match(vm, &mut m, dt);
    crate::world(vm).quidditch = Some(m);
    // The match is back in the world, so whatever was asked for meanwhile can run for real.
    match crate::world(vm).quidditch_gate.end() {
        Some(Request::Start(opponent)) => {
            let _ = start(vm, &opponent);
        }
        Some(Request::Stop) => stop(vm),
        None => {}
    }
}

/// The frame `tick` runs between taking the match out of the world and putting it back.
fn tick_match(vm: &mut Vm<'_>, m: &mut QuidditchMatch, dt: f32) {
    m.frames += 1;
    if m.restart_at == Some(m.frames) {
        // Debug only, and deliberately through the public entry points while the match is out of the world:
        // this is exactly the call that used to destroy nothing and leak every actor. Both must be deferred,
        // and the later one must win (the `start` stops the running match itself).
        let opponent = m.opponent.clone();
        stop(vm);
        let _ = start(vm, &opponent);
        debug_assert!(crate::world(vm).quidditch.is_none(), "the match must still be held by this tick");
    }
    // The broom: this frame's keys and mouse, the soft boundary that turns it back from the walls, the flight
    // model, and the hard boundary that keeps it in the volume whatever it was told to do.
    let (keys, mouse) = m.pending_input.take().unwrap_or_default();
    let (mouse_speed, invert_pitch) = match crate::world(vm).player {
        Some(pc) => crate::settings::mouse_look(vm, pc),
        None => (1.0, false),
    };
    // The phone's touch layout flies the broom itself (`phone::feed_touch` has already turned it into
    // steering); the keys and the mouse are the desktop's.
    let mut i = match m.pending_touch.take() {
        Some(touched) => touched,
        None => desktop_input(&keys, (mouse.0 * mouse_speed, mouse.1 * mouse_speed), invert_pitch),
    };
    // A tap of reach (a key, or the phone's JUMP) wants the catch for a moment.
    let tapped = keys.reach || std::mem::take(&mut m.pending_reach);
    if tapped {
        m.reach_left = REACH_TIME;
        // The same tap is the boost: one second of speed, then a cooldown (`momentum`), so the lunge for the
        // Snitch is a thing you have to time and not a thing you hold.
        m.momentum.try_boost();
    }
    let real_dt = if dt.is_finite() { dt.clamp(0.0, 0.25) } else { 0.0 };
    m.reach_left = (m.reach_left - if tapped { 0.0 } else { real_dt }).max(0.0);
    m.charge_glow = (m.charge_glow - real_dt * 1.5).max(0.0);
    // Knocked off course by the Bludgers, the broom flies itself for a moment: a diving turn, whatever the
    // player does, and the charge is gone with his grip on the broom.
    if m.spin > 0.0 {
        m.spin -= real_dt;
        i = BroomInput { yaw: 1.0, pitch: -0.12, ..BroomInput::default() };
        m.momentum.cut(0.0);
    }
    // `HP3_QUID_RING_AUTOFLY` (debugging): the broom flies the Snitch's ring trail by itself, which is how a
    // headless run shows a ring being flown through. It is off unless the variable is set. It has no boost to
    // reach for: it earns its speed from the rings like anyone else.
    if let Some(target) = rings::autofly_target(m) {
        i = flight::look_at(&m.broom, target, false);
    }
    // The match is over: nobody steers any more. The broom glides to a hover, levelling out, in the slow
    // motion of the catch, and the camera orbits him instead of chasing.
    let mut sim_dt = dt;
    let mut target_speed = m.momentum.target_speed();
    if let Some(f) = m.finale.as_mut() {
        f.t += real_dt;
        i = BroomInput::default();
        target_speed = 0.0;
        sim_dt = dt * (SLOWMO + (1.0 - SLOWMO) * (f.t / SLOWMO_TIME).clamp(0.0, 1.0));
        m.broom.want_pitch *= (1.0 - 2.0 * real_dt).max(0.0);
    } else {
        m.momentum.step(dt);
    }
    let soft = inset(&m.arena, SOFT_MARGIN);
    // The soft boundary steers the rider's *look* (which is what the broom chases now), not the broom's own
    // heading: a correction measured against the heading would keep being undone by the look the player left
    // pointing at the wall.
    let turn = flight::boundary_turn(m.broom.pos, m.broom.want_yaw, &soft);
    i.yaw = (i.yaw + turn * BOUNDARY_WEIGHT).clamp(-1.0, 1.0);
    let flown = flight::step_to(&m.broom, &i, &m.tuning, target_speed, sim_dt);
    m.broom = flight::confine(&flown, &m.arena);
    m.cam = match m.finale {
        Some(f) => camera::orbit_camera(m.broom.pos, &f.from, f.t),
        None => camera::chase_camera(&m.broom, &m.cam, dt),
    };
    // Harry's pawn shows where the broom is. Its `Location` is the clamped one, so nothing the match does can
    // put the player outside the arena, let alone out of the world.
    if let Some(h) = m.harry.filter(|&h| !vm.db.objects.get(h).deleted) {
        crate::set(vm, h, "Location", v3(arena::clamp_into(&m.arena, m.broom.pos)));
        crate::set(vm, h, "Rotation", rot(camera::rotator(m.broom.pitch, m.broom.yaw)));
        crate::set(vm, h, "Velocity", v3(m.broom.vel));
        crate::set(vm, h, "Acceleration", v3(Vec3::ZERO));
        // A broom never walks and never falls, which is also what keeps the out-of-world check (raised only
        // by walking and falling pawns) away from the player for the whole match.
        crate::set(vm, h, "Physics", Value::Byte(crate::physics::PHYS_FLYING));
    }
    let ended = matches!(m.rules.phase(), MatchPhase::Ended(_));
    // Everything that happened this frame, for the commentary and the sounds.
    let mut events: Vec<MatchEvent> = Vec::new();
    if !ended && m.rules.phase() != MatchPhase::Intro {
        // The catch key (`Space`/`Enter`), or every frame under `HP3_QUID_EASY` and where there is no catch key
        // to hold (`auto_catch`).
        let wants_catch = m.easy || keys.catch || m.reach_left > 0.0 || auto_catch();
        for e in m.snitch.step(&m.broom, wants_catch, dt) {
            match e {
                SnitchEvent::Caught => m.caught = true,
                // A catch missed costs half the speed he had built.
                SnitchEvent::Missed => m.momentum.cut(0.5),
                _ => {}
            }
            m.rules.on_snitch(e);
            events.push(MatchEvent::Snitch(e));
        }
        // The Snitch's ring trail: a ring is dropped behind it, Harry's step is tested against the rings he is
        // among, and a ring flown through pays into the catch bar. It also tells the Snitch whether the run of
        // rings the catch needs is in (`trail::TrailTuning::catch_rings`), so it runs right after its step.
        rings::tick(vm, m, dt, &mut events);
        let mut hits = m.bludgers.step(&m.broom, dt);
        // A Bludger that has left the stadium is gone (a miss): its ball would otherwise hang on the wall
        // where `clamp_into` holds it while the real one flies on outside.
        hits.extend(m.bludgers.expire_outside(&m.arena));
        // `HP3_QUID_BUMP=<tick>` (debugging, off by default): from that tick on Harry takes a hit a frame until
        // the Bludgers have his broom, to see the spiral without waiting for three balls to find him.
        if crate::envc::var("HP3_QUID_BUMP").ok().and_then(|v| v.trim().parse::<u64>().ok()).is_some_and(|t| m.frames >= t && m.frames < t + m.bludgers.bumps_to_lose() as u64) {
            m.bludgers.bump();
        }
        let lost = m.bludgers.control_lost();
        for e in hits {
            if e == super::bludger::BludgerEvent::Hit {
                m.momentum.hit();
            }
            m.rules.on_bludger(e, lost);
            events.push(MatchEvent::Bludger(e));
        }
        // Enough hits and the broom is out of his hands for a moment: it spirals (`SPIN_TIME`, flown at the
        // top of the next frames), and the count starts again.
        if lost {
            m.spin = SPIN_TIME;
            m.bludgers.recover();
            let line = format!("quidditch: tick {}: the Bludgers knocked Harry off course, the broom spirals for {SPIN_TIME} s", m.frames);
            println!("{line}");
            vm.log.push(line);
        }
    }
    // The catch (or the last miss) ends the match there and then: no goal is scored, logged or commented on
    // in the frame the Snitch was caught.
    if !ended && !matches!(m.rules.phase(), MatchPhase::Intro | MatchPhase::Ended(_)) {
        // Both teams: the Chasers take the Quaffle up the pitch and shoot, the Keepers guard their posts and
        // the Beaters swat the Bludgers. It runs before the ball, so a shot taken this frame flies this frame.
        team::tick(vm, m, dt);
        // The Quaffle flies, and a hoop it goes through the right way is ten points for that side. The side
        // that conceded takes the restart with it.
        if let Some(home) = ball::tick(vm, m, dt) {
            m.rules.add_goal(home);
            team::restart(m, home);
            events.push(MatchEvent::Goal { home });
        }
    }
    if !ended && let MatchPhase::Ended(outcome) = m.rules.phase() {
        events.push(MatchEvent::Ended(outcome));
        // The finish begins: the orbit grows out of the chase camera as it stands.
        m.finale = Some(Finale { t: 0.0, from: m.cam });
    }
    if let Some(s) = m.snitch_actor.filter(|&s| !vm.db.objects.get(s).deleted) {
        let mut at = arena::clamp_into(&m.arena, m.snitch.pos());
        if m.finale.is_some() {
            if m.caught {
                // Caught: it sits in Harry's hand (a little to his right and over the broom), wings still.
                let fwd = Vec3::new(m.broom.yaw.cos(), m.broom.yaw.sin(), 0.0);
                at = m.broom.pos + fwd.cross(Vec3::Z) * 25.0 + Vec3::Z * 30.0;
            } else {
                // Never caught: the match ended on the last miss, and it flies off the screen with the chase.
                crate::set(vm, s, "bHidden", Value::Bool(true));
            }
        }
        crate::set(vm, s, "Location", v3(at));
    }
    // The Bludger balls, the sounds that follow the match and the commentary line this frame's events earn.
    presentation::kickoff(vm, m);
    presentation::tick(vm, m, dt, &events);
    // `HP3_QUID_DEBUG`: a sample of the match state now and then, to see that it is running at all, and where
    // the broom has got to (with the pawn's own `Location`, to see that it followed).
    // `HP3_QUID_DEBUG=<n>` samples every n ticks instead of every 120, which is how a headless run watches the
    // steering frame by frame.
    let every = crate::envc::var("HP3_QUID_DEBUG").ok().and_then(|v| v.trim().parse::<u64>().ok()).filter(|&n| n > 0).unwrap_or(120);
    if m.frames % every == 0 && crate::envc::var_os("HP3_QUID_DEBUG").is_some() {
        let a = m.snitch_actor.map(|s| crate::rd_vec(vm, s, "Location")).unwrap_or(Vec3::ZERO);
        // The loop the Snitch is flying around its path, so the helix can be read straight off the trace.
        let loop_off = m.snitch.pos() - m.snitch.path_pos();
        println!(
            "quidditch: tick {} phase {:?} snitch ({:.0}, {:.0}, {:.0}) actor ({:.0}, {:.0}, {:.0}) path ({:.0}, {:.0}, {:.0}) loop ({:.0}, {:.0}, {:.0}) r {:.0} progress {:.2} bumps {}",
            m.frames,
            m.rules.phase(),
            m.snitch.pos().x, m.snitch.pos().y, m.snitch.pos().z,
            a.x, a.y, a.z,
            m.snitch.path_pos().x, m.snitch.path_pos().y, m.snitch.path_pos().z,
            loop_off.x, loop_off.y, loop_off.z,
            loop_off.length(),
            m.snitch.progress(),
            m.bludgers.bumps()
        );
        // What the HUD is showing at this moment (the headless run has no screen to look at), and where the
        // Bludgers in flight have got to (a headless run has no screen to see the balls on either).
        println!("{}", presentation::hud_line(&super::hud::hud_items(&presentation::hud_input(m))));
        println!("{}", rings::debug_line(m));
        let balls: Vec<String> = m.bludgers.positions().iter().map(|p| format!("({:.0}, {:.0}, {:.0})", p.x, p.y, p.z)).collect();
        println!("quidditch: bludgers in flight [{}]", balls.join(" "));
        let h = m.harry.filter(|&h| !vm.db.objects.get(h).deleted).map(|h| crate::rd_vec(vm, h, "Location")).unwrap_or(Vec3::ZERO);
        // `flown` is where the flight model wanted to be before `confine` had its say, and `room` is how far
        // that was from each face of the volume: negative on an axis means the broom pushed through it there
        // and was brought back, which is the only evidence that the boundary is doing anything.
        let room = Vec3::new(
            m.arena.half.x - (flown.pos.x - m.arena.center.x).abs(),
            m.arena.half.y - (flown.pos.y - m.arena.center.y).abs(),
            (m.arena.ceiling - flown.pos.z).min(flown.pos.z - (m.arena.center.z - m.arena.half.z)),
        );
        println!(
            "quidditch: tick {} broom ({:.0}, {:.0}, {:.0}) pawn ({:.0}, {:.0}, {:.0}) speed {:.0} yaw {:.2} pitch {:.2} look ({:.2}, {:.2}) charge {:.2} camera ({:.0}, {:.0}, {:.0}) arm {:.0} fov {:.1} flown ({:.0}, {:.0}, {:.0}) room ({:.0}, {:.0}, {:.0})",
            m.frames,
            m.broom.pos.x, m.broom.pos.y, m.broom.pos.z,
            h.x, h.y, h.z,
            m.broom.vel.length(),
            m.broom.yaw,
            m.broom.pitch,
            m.broom.want_yaw,
            m.broom.want_pitch,
            m.momentum.charge(),
            m.cam.pos.x, m.cam.pos.y, m.cam.pos.z,
            m.cam.arm,
            m.cam.fov,
            flown.pos.x, flown.pos.y, flown.pos.z,
            room.x, room.y, room.z,
        );
    }
    if !m.reported
        && let MatchPhase::Ended(outcome) = m.rules.phase()
    {
        m.reported = true;
        let score = m.rules.score();
        // The same words the HUD shows, so the log and the screen agree.
        let result = presentation::result_of(m).unwrap_or_default();
        let line = format!("quidditch: match against {} ended {outcome:?}, {}-{} -- {result}", m.opponent, score.home, score.away);
        println!("{line}");
        vm.log.push(line);
        // A match the front end started goes into the league and then hands the player back to the menu; the
        // result line stays up in between. A debug match (`HP3_MATCH`) does neither.
        if m.from_menu {
            super::menu::record_result(vm, &m.opponent, outcome, score);
            m.leave_at = Some(m.frames + super::menu::LINGER_TICKS);
        }
    }
    // The result has been read: the match is stopped (at the end of this tick, through the gate) and the
    // player travels back to the front end, which is only allowed once no match is running.
    if m.leave_at == Some(m.frames) {
        let map = super::menu::leave(vm);
        crate::world(vm).pending_travel = Some(map);
    }
}

/// `b` with its walls brought in by `margin` (never past its own centre): the band in which the broom steers
/// itself back in.
fn inset(b: &ArenaBounds, margin: f32) -> ArenaBounds {
    let half = Vec3::new(
        (b.half.x - margin).max(1.0),
        (b.half.y - margin).max(1.0),
        (b.half.z - margin).max(1.0),
    );
    ArenaBounds { center: b.center, half, ceiling: (b.ceiling - margin).max(b.center.z - half.z + 1.0) }
}

/// This frame's broom keys and mouse motion, from `crate::input::apply` (the only place that has them: the
/// world's input is cleared at the end of every frame). Nothing happens when no match is running, which is
/// why the normal input path is untouched on every other level.
pub fn feed_input(vm: &mut Vm<'_>, keys: KeyState, mouse: (f32, f32)) {
    if let Some(m) = crate::world(vm).quidditch.as_mut() {
        m.pending_input = Some((keys, mouse));
    }
}

/// The view a match shows, for `crate::player_view`: the chase camera behind Harry's broom. `None` when no
/// match is running, and the game's own `PlayerCalcView` is then used as before.
pub fn view(vm: &mut Vm<'_>) -> Option<(Vec3, [i32; 3], f32)> {
    let m = crate::world(vm).quidditch.as_ref()?;
    Some((m.cam.pos, m.cam.rot, m.cam.fov))
}

/// Starts the match this level was loaded for, as soon as it has finished loading: the one the front end asked
/// for (`menu::request`, the `quidditch` command) or `HP3_MATCH=<opponent>` (debugging). Called by `load_with`
/// once the player exists; every other level leaves the match state empty.
pub fn start_from_env(vm: &mut Vm<'_>) -> Result<(), VmError> {
    // The front end's request first: it is a real match, it goes into the league, and it ends by taking the
    // player back to the menu.
    if let Some(opponent) = super::menu::take_request() {
        let here = crate::world(vm).map.name.clone();
        if !here.eq_ignore_ascii_case(super::menu::MATCH_MAP) {
            let line = format!("quidditch: a match against {opponent} was asked for but {here} is not {}, dropped", super::menu::MATCH_MAP);
            println!("{line}");
            vm.log.push(line);
        } else if crate::world(vm).player.is_none() {
            vm.log.push(format!("quidditch: a match against {opponent} was asked for but the level has no player, no match started"));
        } else {
            start(vm, &opponent)?;
            if let Some(m) = crate::world(vm).quidditch.as_mut() {
                m.from_menu = true;
            }
            return Ok(());
        }
    }
    let Ok(opponent) = crate::envc::var("HP3_MATCH") else { return Ok(()) };
    let opponent = opponent.trim();
    if opponent.is_empty() || opponent.eq_ignore_ascii_case("none") {
        return Ok(());
    }
    // Once a run: the variable starts a match on the level the run opens with, not again on every level the
    // player travels to afterwards (leaving the match would otherwise start another one on the front end).
    if ENV_MATCH_DONE.with(|c| c.replace(true)) {
        return Ok(());
    }
    // A house the league knows, in its own spelling (`sly` is Slytherin); anything else is not played.
    let Some(house) = super::league::Opponent::parse(opponent) else {
        let line = format!("quidditch: HP3_MATCH={opponent} is no house (Slytherin, Ravenclaw, Hufflepuff), no match started");
        println!("{line}");
        vm.log.push(line);
        return Ok(());
    };
    if crate::world(vm).player.is_none() {
        vm.log.push("quidditch: HP3_MATCH set but the level has no player, no match started".into());
        return Ok(());
    }
    start(vm, house.name())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Outside a tick nothing is deferred and nothing is left behind.
    #[test]
    fn gate_lets_requests_straight_through_outside_a_tick() {
        let mut g = Gate::default();
        assert!(!g.ticking());
        assert!(!g.defer(Request::Stop));
        assert!(!g.defer(Request::Start("Slytherin".into())));
        assert_eq!(g, Gate::default());
    }

    /// Inside the tick window a request waits, and the end of the tick hands it back exactly once.
    #[test]
    fn gate_defers_a_request_and_hands_it_back_at_the_end_of_the_tick() {
        let mut g = Gate::default();
        g.begin();
        assert!(g.ticking());
        assert!(g.defer(Request::Stop));
        assert_eq!(g.end(), Some(Request::Stop));
        assert!(!g.ticking());
        assert_eq!(g.end(), None);
        assert_eq!(g, Gate::default());
    }

    /// Several requests in one tick: the last one is what the caller ended up wanting, and it is the only one
    /// applied. A `start` stops whatever runs anyway, so nothing is lost either way round.
    #[test]
    fn gate_keeps_the_last_request_of_a_tick() {
        let mut g = Gate::default();
        g.begin();
        assert!(g.defer(Request::Start("Ravenclaw".into())));
        assert!(g.defer(Request::Stop));
        assert!(g.defer(Request::Start("Slytherin".into())));
        assert_eq!(g.end(), Some(Request::Start("Slytherin".into())));

        let mut g = Gate::default();
        g.begin();
        assert!(g.defer(Request::Start("Slytherin".into())));
        assert!(g.defer(Request::Stop));
        assert_eq!(g.end(), Some(Request::Stop));
    }

    /// Letting the broom pawn go: the controller must come out of a match playing the pawn it played before
    /// (or nothing), and never the broom pawn, which `stop` destroys right afterwards.
    #[test]
    fn give_back_never_leaves_the_controller_on_the_broom_pawn() {
        let (broom, walking) = (ObjId(10), ObjId(11));
        // The usual way round: the match has the player, the old pawn is still there.
        assert_eq!(give_back(Some(broom), Some(broom), Some(walking), true), Some(Some(walking)));
        // Nothing was being played when the match started: nothing is played after it.
        assert_eq!(give_back(Some(broom), Some(broom), None, false), Some(None));
        // The old pawn was destroyed while the match ran: no pawn rather than a deleted one.
        assert_eq!(give_back(Some(broom), Some(broom), Some(walking), false), Some(None));
        // Something else took the player over (a cutscene): it is left alone, and only the broom pawn is let
        // go. Whatever the answer, it is never the broom pawn.
        assert_eq!(give_back(Some(ObjId(12)), Some(broom), Some(walking), true), None);
        for playing in [None, Some(broom), Some(ObjId(12))] {
            for had in [None, Some(walking), Some(broom)] {
                for alive in [false, true] {
                    assert_ne!(give_back(playing, Some(broom), had, alive), Some(Some(broom)), "{playing:?} {had:?} {alive}");
                }
            }
        }
        // A match that never got a broom pawn still hands the controller back what it had.
        assert_eq!(give_back(Some(walking), None, Some(walking), true), Some(Some(walking)));
    }

    /// The soft boundary's band lies inside the arena, is never inverted, and leaves the arena centre where
    /// it is (so `boundary_turn` steers towards the pitch and not somewhere else).
    #[test]
    fn inset_stays_inside_the_arena_and_never_turns_inside_out() {
        let b = arena::bounds();
        for margin in [0.0, SOFT_MARGIN, 1e6] {
            let s = inset(&b, margin);
            assert_eq!(s.center, b.center);
            assert!(s.half.x > 0.0 && s.half.y > 0.0 && s.half.z > 0.0, "{s:?}");
            assert!(s.half.x <= b.half.x && s.half.y <= b.half.y && s.half.z <= b.half.z, "{s:?}");
            assert!(s.ceiling <= b.ceiling && s.ceiling > s.center.z - s.half.z, "{s:?}");
        }
        // The band really is a band: a point just inside the real wall is outside the soft one.
        let s = inset(&b, SOFT_MARGIN);
        let near_wall = Vec3::new(b.center.x + b.half.x - 100.0, b.center.y, b.center.z);
        assert_ne!(super::flight::boundary_turn(near_wall, 0.0, &s), 0.0, "the soft boundary must bite before the wall");
        assert_eq!(super::flight::boundary_turn(near_wall, 0.0, &b), 0.0, "and the hard one only at it");
    }

    /// A tick nobody asked anything of leaves no request, and the window really closes.
    #[test]
    fn gate_is_empty_after_a_quiet_tick_and_reopens() {
        let mut g = Gate::default();
        g.begin();
        assert_eq!(g.end(), None);
        assert!(!g.defer(Request::Stop));
        g.begin();
        assert!(g.defer(Request::Stop));
        assert_eq!(g.end(), Some(Request::Stop));
    }
}
