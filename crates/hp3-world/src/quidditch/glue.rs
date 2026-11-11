//! The script-VM side of a Quidditch match: builds the arena inside the loaded level, spawns the match's
//! actors, ticks the pure systems and puts their positions back onto the actors.
//!
//! This is the only Quidditch file that touches the VM; everything it drives (`flight`, `snitch`, `bludger`,
//! `rules`, `arena`) is pure Rust, unit-tested on its own. The match state lives in `World::quidditch`, so it
//! goes away with the level and a map that never starts a match pays nothing but one `take()` of a `None` a
//! frame. `World::quidditch_gate` keeps `start` and `stop` honest when they are called from inside `tick`.
//!
//! The HUD, sound and the hoop rings come later. What is here is the arena layout, start / stop / tick, the
//! flight of Harry's broom (steered by `flight` from the keys `crate::input::apply` hands over) and the chase
//! camera `crate::player_view` shows it from.

use glam::Vec3;

use hp3_script::{ObjId, Value, Vm, VmError};

use super::arena::{self, ArenaBounds, ArenaPiece};
use super::bludger::Bludgers;
use super::camera::{self, CamState};
use super::flight::{self, BroomState, BroomTuning};
use super::input::{desktop_input, KeyState};
use super::presentation::{self, Presentation};
use super::rules::{MatchEvent, MatchPhase, Rules};
use super::snitch::{Snitch, SnitchTuning};

/// Harry's broom pawn (`HGame.HarryQuid`) and the Golden Snitch (`HGame.Snitch`): both classes exist in the
/// retail scripts with empty bodies, which is why the engine drives them.
const HARRY_CLASS: &str = "HGame.HarryQuid";
const SNITCH_CLASS: &str = "HGame.Snitch";
/// What the stadium pieces and the broom are spawned as.
const MESH_ACTOR_CLASS: &str = "Engine.StaticMeshActor";
/// The broom Harry rides. Its group inside the package is not documented anywhere (unlike the `Quid_Stadium`
/// meshes), so it is looked up by name; `find_static_mesh` logs the full path it resolved to. The plan called
/// the mesh `QuidBroom_SM`, but no static mesh of that name exists in the retail package (only the string, in
/// its name table): the mesh is `miscDetails.QuiddichBroom_SM`. Both spellings are tried, in that order.
const BROOM_PACKAGE: &str = "HP3_InteriorAccents_SM";
const BROOM_MESHES: [&str; 2] = ["QuidBroom_SM", "QuiddichBroom_SM"];
/// Bones of the `HarryQuid` skeleton the broom may hang from, best first. The first one the mesh has wins;
/// with none of them the broom is based on the pawn instead (it still rides along, just not off a bone).
/// `body_broom` is the real one: the retail `HarryQuid` skeleton carries a bone for exactly this (its last,
/// after the hand and finger bones). The rest are fallbacks in case another mesh is used for the rider.
const BROOM_BONES: [&str; 5] = ["body_broom", "broom", "body_rhand", "body_lhand", "body_root"];
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
    /// The boost toggle: a tap of the boost key or button flips it (HP1's broom works that way). It is the
    /// match's, not the input module's, so a pause of the tick cannot reset it.
    pub(super) boost: bool,
    /// What this match may spend on a frame, as the app's frame times ask for (`phone::feed_frame_ms`,
    /// `quidditch::lod`). Nothing steers team players yet (Task 13); what keeps to it today is the
    /// presentation, which leaves distant Bludgers to their proxy.
    pub budget: super::lod::Budget,
    /// The 120-frame count behind `budget`.
    pub(super) gov: super::lod::BudgetGovernor,
    /// The pawn the player controller had before the match, given back by `stop`.
    was_playing: Option<ObjId>,
    /// Ticks this match has had.
    frames: u64,
    /// `HP3_MATCH_RESTART=<ticks>` (debugging): start the match over after that many ticks, which takes the
    /// stop-then-start path. Nothing in the game sets it.
    restart_at: Option<u64>,
    /// Whether the end of the match has been reported.
    reported: bool,
    /// `HP3_QUID_EASY=1` (debugging, off by default): the snitch is caught the moment Harry is anywhere near
    /// it and the catch key is held for him, so a headless smoke run reaches a result in a few hundred ticks.
    /// Nothing in the game sets it.
    easy: bool,
}

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

/// The first of `names` that is a static mesh of `package`, whatever group it sits in, with the full path it
/// was found at. `find_object_of` needs the whole `Package.Group.Name`, which is not known for the broom.
fn find_static_mesh(vm: &mut Vm<'_>, package: &str, names: &[&str]) -> Option<(ObjId, String)> {
    let pkg = vm.db.set.get(package).ok()?;
    let meshes: Vec<(usize, String)> = (0..pkg.exports.len())
        .filter(|&i| pkg.export_class_name(i).eq_ignore_ascii_case("StaticMesh"))
        .map(|i| (i, pkg.ref_name(hp3_package::ObjRef::from_export(i)).to_string()))
        .collect();
    for want in names {
        if let Some(&(i, _)) = meshes.iter().find(|(_, n)| n.eq_ignore_ascii_case(want)) {
            let path = pkg.ref_path(hp3_package::ObjRef::from_export(i));
            return Some((vm.db.obj_for(&pkg, i), path));
        }
    }
    if crate::envc::var_os("HP3_QUID_DEBUG").is_some() {
        let like: Vec<&String> = meshes.iter().map(|(_, n)| n).filter(|n| n.to_ascii_lowercase().contains("broom")).collect();
        println!("quidditch: no {names:?} in {package}; its broom-like meshes are {like:?}");
    }
    None
}

/// Gives `harry` his broom: a static mesh actor hung off a bone of his skeleton, so it follows every pose the
/// animation puts him in. Returns the broom actor, which the match owns like any other.
fn attach_broom(vm: &mut Vm<'_>, harry: ObjId) -> Option<ObjId> {
    let Some((mesh, path)) = find_static_mesh(vm, BROOM_PACKAGE, &BROOM_MESHES) else {
        vm.log.push(format!("quidditch: no broom mesh {BROOM_MESHES:?} in {BROOM_PACKAGE}, Harry rides nothing"));
        return None;
    };
    let class = crate::find_object(vm, MESH_ACTOR_CLASS)?;
    let (at, facing) = (crate::rd_vec(vm, harry, "Location"), crate::rd_rot(vm, harry, "Rotation"));
    let args = [Value::Object(Some(class)), Value::Object(Some(harry)), Value::Empty, v3(at), rot(facing)];
    let broom = crate::natives::spawn(vm, None, &args).ok()?.as_obj()?;
    crate::set(vm, broom, "StaticMesh", Value::Object(Some(mesh)));
    // A static mesh actor is born `bStatic`, which would stop it following the pawn: the broom moves.
    crate::set(vm, broom, "bStatic", Value::Bool(false));
    crate::set(vm, broom, "bNoDelete", Value::Bool(false));
    // The bones of Harry's skeletal mesh, to pick the one the broom hangs from.
    let bones = crate::get(vm, harry, "Mesh")
        .as_obj()
        .and_then(|m| crate::anim::skeleton_of(vm, m))
        .map(|s| s.skeleton.names.clone())
        .unwrap_or_default();
    let bone = BROOM_BONES.iter().find(|b| bones.iter().any(|n| n == *b)).copied();
    match bone {
        // Exactly what `Actor.AttachToBone` does: base, bone name, and the engine's attachment table, which
        // puts the child on the bone every frame in `anim::update_attachments`.
        Some(b) => {
            let name = vm.db.names.intern(b);
            crate::set(vm, broom, "Base", Value::Object(Some(harry)));
            crate::set(vm, broom, "AttachmentBone", Value::Name(name));
            crate::world(vm).ever_base.insert(harry);
            crate::world(vm).anim.attachments.insert(broom, (harry, b.to_string()));
            crate::anim::update_attachments(vm);
            vm.log.push(format!("quidditch: broom {path} on bone {b}"));
        }
        // No bone of ours is in the skeleton: the broom is based on the pawn, so it still moves with him.
        // The bone list is logged so the right name can be added to `BROOM_BONES`.
        None => {
            crate::natives::set_base(vm, broom, Some(harry), false);
            vm.log.push(format!("quidditch: broom {path} based on the pawn, no bone of {BROOM_BONES:?} in its skeleton"));
            if crate::envc::var_os("HP3_QUID_DEBUG").is_some() {
                println!("quidditch: HarryQuid bones: {bones:?}");
            }
        }
    }
    Some(broom)
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
    let start_at = arena::harry_start(&bounds);
    let harry = spawn_class(vm, HARRY_CLASS, start_at, 16384);
    let mut broom = None;
    let mut was_playing = None;
    if let Some(h) = harry {
        crate::set(vm, h, "Physics", Value::Byte(crate::physics::PHYS_FLYING));
        actors.push(h);
        broom = attach_broom(vm, h);
        if let Some(b) = broom {
            actors.push(b);
        }
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
    let broom_state = BroomState { pos: start_at, vel: Vec3::ZERO, yaw: std::f32::consts::FRAC_PI_2, pitch: 0.0 };
    let mut m = QuidditchMatch {
        opponent: opponent.to_string(),
        arena: bounds,
        broom: broom_state,
        tuning: BroomTuning::default(),
        cam: camera::behind(&broom_state),
        snitch,
        bludgers,
        rules,
        actors,
        harry,
        snitch_actor,
        show: Presentation::default(),
        pending_input: None,
        pending_touch: None,
        boost: false,
        budget: super::lod::Budget::default(),
        gov: super::lod::BudgetGovernor::default(),
        was_playing,
        frames: 0,
        // The restart is a one-shot: the match it starts does not restart itself again.
        restart_at: (!re_entry).then(|| crate::envc::var("HP3_MATCH_RESTART").ok().and_then(|v| v.parse().ok()).filter(|&n| n > 0)).flatten(),
        reported: false,
        easy,
    };
    // Meshes, sounds and music. It may add actors of its own, so it runs before the count below is printed.
    presentation::start(vm, &mut m, seed);
    let yes_no = |o: Option<ObjId>| if o.is_some() { "yes" } else { "MISSING" };
    let line = format!(
        "quidditch: match against {} started, phase {:?}, {} actors spawned ({} stadium pieces, Harry {}, broom {}, Snitch {}){}",
        m.opponent,
        m.rules.phase(),
        m.actors.len(),
        pieces.len(),
        yes_no(m.harry),
        yes_no(broom),
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
    // The music stops with the match; the ambient loops go with the actors that carry them.
    presentation::stop(vm, &m);
    // The pawn goes back to the player before the broom pawn is destroyed: a controller must never be left
    // holding a deleted pawn.
    unpossess(vm, m.was_playing, m.harry);
    let mut gone = 0;
    // Backwards, so a child goes before what it rides on (the broom before Harry).
    for a in m.actors.iter().rev() {
        // Scenery is born `bStatic`, which `destroy_actor` refuses to remove: clear the flag first.
        crate::set(vm, *a, "bStatic", Value::Bool(false));
        crate::set(vm, *a, "bNoDelete", Value::Bool(false));
        if crate::destroy_actor(vm, *a) {
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
    // steering, with the same boost toggle); the keys and the mouse are the desktop's.
    let mut i = match m.pending_touch.take() {
        Some(touched) => touched,
        None => desktop_input(&keys, (mouse.0 * mouse_speed, mouse.1 * mouse_speed), invert_pitch, &mut m.boost),
    };
    let soft = inset(&m.arena, SOFT_MARGIN);
    let turn = flight::boundary_turn(m.broom.pos, m.broom.yaw, &soft);
    i.yaw = (i.yaw + turn * BOUNDARY_WEIGHT).clamp(-1.0, 1.0);
    let flown = flight::step(&m.broom, &i, &m.tuning, dt);
    m.broom = flight::confine(&flown, &m.arena);
    m.cam = camera::chase_camera(&m.broom, &m.cam, dt);
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
        // The catch key (`Space`/`Enter`), or every frame under `HP3_QUID_EASY`.
        let wants_catch = m.easy || keys.catch;
        for e in m.snitch.step(&m.broom, wants_catch, dt) {
            m.rules.on_snitch(e);
            events.push(MatchEvent::Snitch(e));
        }
        let hits = m.bludgers.step(&m.broom, dt);
        let lost = m.bludgers.control_lost();
        for e in hits {
            m.rules.on_bludger(e, lost);
            events.push(MatchEvent::Bludger(e));
        }
        if let MatchPhase::Ended(outcome) = m.rules.phase() {
            events.push(MatchEvent::Ended(outcome));
        }
    }
    if let Some(s) = m.snitch_actor.filter(|&s| !vm.db.objects.get(s).deleted) {
        let at = arena::clamp_into(&m.arena, m.snitch.pos());
        crate::set(vm, s, "Location", v3(at));
    }
    // The Bludger balls, the sounds that follow the match and the commentary line this frame's events earn.
    presentation::kickoff(vm, &mut m);
    presentation::tick(vm, &mut m, dt, &events);
    // `HP3_QUID_DEBUG`: a sample of the match state now and then, to see that it is running at all, and where
    // the broom has got to (with the pawn's own `Location`, to see that it followed).
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
        // What the HUD is showing at this moment (the headless run has no screen to look at), and where the
        // Bludgers in flight have got to (a headless run has no screen to see the balls on either).
        println!("{}", presentation::hud_line(&super::hud::hud_items(&presentation::hud_input(&m))));
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
            "quidditch: tick {} broom ({:.0}, {:.0}, {:.0}) pawn ({:.0}, {:.0}, {:.0}) speed {:.0} yaw {:.2} pitch {:.2} boost {} camera ({:.0}, {:.0}, {:.0}) flown ({:.0}, {:.0}, {:.0}) room ({:.0}, {:.0}, {:.0})",
            m.frames,
            m.broom.pos.x, m.broom.pos.y, m.broom.pos.z,
            h.x, h.y, h.z,
            m.broom.vel.length(),
            m.broom.yaw,
            m.broom.pitch,
            m.boost,
            m.cam.pos.x, m.cam.pos.y, m.cam.pos.z,
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
        let result = presentation::result_of(&m).unwrap_or_default();
        let line = format!("quidditch: match against {} ended {outcome:?}, {}-{} -- {result}", m.opponent, score.home, score.away);
        println!("{line}");
        vm.log.push(line);
    }
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
