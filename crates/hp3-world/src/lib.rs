//! The running level: actors loaded from a map into the script VM, the begin-play sequence, and the tick that
//! drives events, timers and state code.
//!
//! The world lives in `Vm::ext`; natives reach it with `world(vm)`.

pub mod anim;
pub mod audio;
pub mod debug_move;
pub mod character_switch;
pub mod canvas;
pub mod casttest;
pub mod collision;
pub mod config;
pub mod fonts;
pub mod nav;
pub mod gui;
pub mod input;
pub mod movie;
pub mod interactions;
pub mod interpolation;
pub mod lipsync;
pub mod memo;
pub mod natives;
pub mod particles;
mod perception;
pub mod quidditch;
pub mod physics;
pub mod save;
pub mod shadows;
pub mod settings;
pub mod touch;

use hp3_script::fx::FxHashMap as HashMap;
use std::rc::Rc;

use hp3_assets::Asset;
use hp3_package::object::{self, Body};
use hp3_package::{ObjRef, Package};
use hp3_script::objects::ObjKind;
use hp3_script::{Name, ObjId, StructId, Value, Vm, VmError};

pub struct Timer {
    pub rate: f32,
    pub remaining: f32,
    pub looping: bool,
}

pub struct World {
    pub map: Rc<Package>,
    /// Live actors in level order; destroyed actors are removed.
    pub actors: Vec<ObjId>,
    pub level_info: Option<ObjId>,
    pub game: Option<ObjId>,
    /// The local player's controller.
    pub player: Option<ObjId>,
    pub time: f32,
    pub timers: HashMap<ObjId, Timer>,
    pub actor_class: StructId,
    slots: HashMap<(StructId, Name), Option<usize>>,
    /// Ticks run so far.
    pub frame: u64,
    /// Actors bucketed by position for radius queries, rebuilt when stale.
    pub grid: Option<natives::Grid>,
    /// Script errors raised while running events, with where they happened.
    pub errors: Vec<String>,
    pub spawned: usize,
    pub config: hp3_core::Config,
    pub input: input::Input,
    pub collision: collision::Collision,
    /// Who touches whom, and the index that finds neighbours quickly.
    pub touch: touch::Touch,
    pub anim: anim::Animation,
    pub nav: nav::Navigation,
    /// Opacity set by `SetOpacity` (1 when absent).
    pub opacity: HashMap<ObjId, f32>,
    pub canvas: canvas::CanvasState,
    pub viewport: interactions::Viewport,
    pub native_fonts: fonts::NativeFonts,
    /// Options of the URL the level was loaded with (`?Name=Value...`).
    pub url_options: String,
    /// HP3's `WorldInfo` object (game states, persistent values), kept across levels.
    pub world_info: Option<ObjId>,
    /// A save was asked for last frame (performed, then the level action cleared, this frame).
    pub save_requested: bool,
    /// The level was put back from a saved game (the player's controller hears it before the first tick).
    pub loaded_from_save: bool,
    /// Sounds and music the game asked for, for the platform to play.
    pub audio: audio::Audio,
    /// Mouths following the sounds that are playing (`lipsync`).
    pub lip_syncs: Vec<lipsync::LipSync>,
    pub movies: movie::Movies,
    pub particles: particles::Particles,
    /// The pawn's own `AirSpeed`, kept while flying in the debug modes.
    pub debug_air_speed: Option<f32>,
    /// Property slots the per-tick physics loop reads, found once per frame.
    pub phys_slots: Option<physics::Slots>,
    /// Which classes are controllers (remembered, the per-actor loop asks every tick).
    pub controller_classes: HashMap<hp3_script::StructId, bool>,
    /// A level switch asked for and due (`Level.NextURL`).
    pub pending_travel: Option<String>,
    /// The game asked to leave (`ConsoleCommand("exit")`, the Quit page's Yes): the app ends its main loop.
    pub quit_requested: bool,
    /// Counts changes made in the option menus (`settings`); the app reads the values it needs again when it moves.
    pub settings_version: u32,
    /// Walking pawns' view offsets while they ease onto a step (drawing only).
    pub step_smooth: std::collections::HashMap<ObjId, f32>,
    /// Where each hero's floor was last drawn while it went over stairs (see `hero_stair_offset`).
    pub stair_floor: std::collections::HashMap<ObjId, f32>,
    /// The physics volumes of the level with their brush planes (built the first time a pawn is checked).
    pub volume_hulls: Option<Vec<VolumeHull>>,
    /// What travelled in with the player (the level keeps it for a restart).
    pub travel_info: TravelInfo,
    actor_slots: HashMap<&'static str, Option<usize>>,
    scenery_classes: HashMap<hp3_script::StructId, bool>,
    /// Where each pawn was when its physics volume was last worked out, and the volume it had after.
    volume_checked: hp3_script::fx::FxHashMap<ObjId, ([u32; 3], Option<ObjId>)>,
    /// The level's default physics volume once found (the search walks every actor).
    default_volume: Option<ObjId>,
    ambient_classes: HashMap<hp3_script::StructId, bool>,
    /// Since when each student has been in the trade's camera-setup state (game time), for `vendor_watchdog`.
    vendor_setup_since: HashMap<ObjId, f32>,
    /// Time an NPC ticked less often has not been given yet.
    far_owed: HashMap<ObjId, f32>,
    /// Actors that something has been put on (their `Base` for someone), for `riders_of`: only those can carry anything. Filled
    /// from the level when first asked for, then by the places that set a base; `false` after a load, to be filled again.
    pub ever_base: hp3_script::fx::FxHashSet<ObjId>,
    pub ever_base_valid: bool,
    /// Time per tick phase, when `HP3_TICK_PROFILE` is set.
    pub tick_profile: std::collections::BTreeMap<&'static str, std::time::Duration>,
    /// While profiling: per class, how many `Tick` events ran, how many were non-trivial, and the time they took.
    /// Classes whose `Tick` event does nothing the player can see (answers are remembered per class).
    pub tick_skipped: std::collections::HashMap<hp3_script::class_db::StructId, bool>,
    pub tick_classes: std::collections::HashMap<(&'static str, hp3_script::class_db::StructId), (u32, std::time::Duration)>,
    /// Whether the blocking volumes have been told to the collision.
    pub volumes_marked: bool,
    /// Per-actor answers of the tick loop (scenery skipped), the actor boxes and the baked collision sync.
    pub memo_tick: memo::Memo,
    pub memo_boxes: memo::Memo,
    pub memo_baked: memo::Memo,
    /// Per-actor answer of the draw loop: whether the actor can be drawn at all (1: never).
    pub memo_draw: memo::Memo,
    /// Per-actor answer of the movement-animation loop (1: not a pawn).
    pub memo_pawns: memo::Memo,
    /// Per-actor answer of the shadow collection (1: not a shadow projector).
    pub memo_projectors: memo::Memo,
    /// Per-actor answer of the particle tick (1: not an emitter).
    pub memo_emitters: memo::Memo,
    /// The Quidditch match being played in this level, if any (`quidditch::glue`). None on every other level,
    /// and gone with the level itself.
    pub quidditch: Option<quidditch::glue::QuidditchMatch>,
    /// Holds a `quidditch::glue::start` or `stop` asked for while the match is being ticked until the tick is
    /// over (see `quidditch::glue::Gate`).
    pub quidditch_gate: quidditch::glue::Gate,
    /// Whether a level switch has already been refused for the running match, so it is logged once
    /// (`count_down_travel`). Never set on a level with no match.
    pub quidditch_travel_blocked: bool,
}

/// The world stored in the VM. Panics if no level is loaded.
pub fn world<'a>(vm: &'a mut Vm<'_>) -> &'a mut World {
    vm.ext.as_mut().and_then(|e| e.downcast_mut::<World>()).expect("no world loaded")
}

/// Slot of a property of an object's class, by name.
pub fn slot(vm: &mut Vm<'_>, obj: ObjId, name: &str) -> Option<usize> {
    let class = vm.db.objects.get(obj).class?;
    let n = vm.db.names.intern(name);
    if let Some(w) = vm.ext.as_mut().and_then(|e| e.downcast_mut::<World>())
        && let Some(s) = w.slots.get(&(class, n))
    {
        return *s;
    }
    let s = vm.db.get(class).prop(n).map(|p| p.slot);
    if let Some(w) = vm.ext.as_mut().and_then(|e| e.downcast_mut::<World>()) {
        w.slots.insert((class, n), s);
    }
    s
}

/// Slot of a property declared on `Actor` (the same in every actor class), resolved once.
pub fn actor_slot(vm: &mut Vm<'_>, name: &'static str) -> Option<usize> {
    if let Some(s) = world(vm).actor_slots.get(name) {
        return *s;
    }
    let s = vm.db.find("Actor").and_then(|c| {
        let n = vm.db.names.find(name)?;
        vm.db.get(c).prop(n).map(|p| p.slot)
    });
    world(vm).actor_slots.insert(name, s);
    s
}

static EMPTY: Value = Value::Empty;

/// The value in an actor's slot, without copying it (`Value::Empty` when missing).
pub fn read<'v>(vm: &'v Vm<'_>, a: ObjId, slot: Option<usize>) -> &'v Value {
    slot.and_then(|s| vm.db.objects.values(a).and_then(|v| v.get(s))).unwrap_or(&EMPTY)
}

/// A vector held in an actor's slot.
pub fn read_vec(vm: &Vm<'_>, a: ObjId, slot: Option<usize>) -> glam::Vec3 {
    match read(vm, a, slot) {
        Value::Struct(f) if f.len() >= 3 => glam::Vec3::new(f[0].as_float(), f[1].as_float(), f[2].as_float()),
        _ => glam::Vec3::ZERO,
    }
}

pub fn get(vm: &mut Vm<'_>, obj: ObjId, name: &str) -> Value {
    let Some(s) = slot(vm, obj, name) else { return Value::Empty };
    vm.db.objects.values(obj).and_then(|v| v.get(s).cloned()).unwrap_or_default()
}

/// A property's value looked at in place: like `get`, without copying it (a vector's copy is an allocation), for code that
/// reads many properties of many actors every frame. The slot is found first, then the value borrowed.
pub fn rd<'v>(vm: &'v mut Vm<'_>, obj: ObjId, name: &str) -> &'v Value {
    let s = slot(vm, obj, name);
    read(&*vm, obj, s)
}

/// `rd` of a vector property.
pub fn rd_vec(vm: &mut Vm<'_>, obj: ObjId, name: &str) -> glam::Vec3 {
    vec_of(rd(vm, obj, name))
}

/// `rd` of a rotator property, as its three angles.
pub fn rd_rot(vm: &mut Vm<'_>, obj: ObjId, name: &str) -> [i32; 3] {
    let f = rd(vm, obj, name).fields();
    [f.first().map_or(0, Value::as_int), f.get(1).map_or(0, Value::as_int), f.get(2).map_or(0, Value::as_int)]
}

pub fn set(vm: &mut Vm<'_>, obj: ObjId, name: &str, value: Value) {
    let Some(s) = slot(vm, obj, name) else { return };
    if let Some(v) = vm.db.objects.values_mut(obj).and_then(|v| v.get_mut(s)) {
        *v = value;
    }
}

/// Turns a package export into a live instance in the object table.
fn instantiate(vm: &mut Vm<'_>, pkg: &Rc<Package>, index: usize) -> Option<ObjId> {
    let (class, values) = vm.db.instance_values(pkg, index)?;
    let id = vm.db.obj_for(pkg, index);
    let e = vm.db.objects.get_mut(id);
    e.class = Some(class);
    e.kind = ObjKind::Instance { values, state: None };
    // A map loaded again (a saved game loaded in the same level, a hub visited twice) has the same objects as the last time,
    // which `travel` marked deleted when the level was left: they are alive again.
    e.deleted = false;
    let outer = pkg.exports[index].outer;
    let outer = if outer.is_none() { None } else { vm.db.resolve_obj(pkg, outer) };
    vm.set_intrinsics(id, outer);
    Some(id)
}

/// Runs an event and records script errors instead of stopping the level.
pub fn fire(vm: &mut Vm<'_>, obj: ObjId, event: &str, args: &mut [Value]) -> Value {
    match vm.event(obj, event, args) {
        Ok(v) => v,
        Err(e) => {
            let at = format!("{} {event}: {e}", vm.db.names.get(vm.db.objects.get(obj).name));
            let w = world(vm);
            if w.errors.len() < 1000 {
                w.errors.push(at);
            }
            Value::Empty
        }
    }
}

/// Loads `map` into the VM, spawns the game (`Package.Class`, e.g. `HGame.HGame`), runs the begin-play
/// sequence of its actors and logs in the local player.
pub fn load(vm: &mut Vm<'_>, map: &Rc<Package>, config: hp3_core::Config) -> Result<(), VmError> {
    load_with(vm, map, config, None)
}

/// An actor on its way to the next level: its class, its name and its `travel` property values.
#[derive(Clone)]
pub struct TravelActor {
    class: hp3_script::class_db::StructId,
    name: String,
    props: Vec<(String, TravelValue)>,
}

/// A travelling value: plain, or an actor named (in the same travel set) when the value refers to one.
#[derive(Clone)]
enum TravelValue {
    Plain(Value),
    Actor(String),
}

/// What travels between levels, by player name.
pub type TravelInfo = Vec<(String, Vec<TravelActor>)>;

/// Describes `actor` for travel: nothing unless it is marked `bTravel`, else every `travel` property.
fn export_travel(vm: &mut Vm<'_>, actor: ObjId) -> Option<TravelActor> {
    if !get(vm, actor, "bTravel").as_bool() {
        return None;
    }
    let class = vm.db.objects.get(actor).class?;
    let actor_class = world(vm).actor_class;
    let props: Vec<_> = vm.db.get(class).props.iter().filter(|p| p.flags & CPF_TRAVEL != 0).cloned().collect();
    let mut out = Vec::new();
    for p in props {
        let Some(v) = vm.db.objects.values(actor).and_then(|v| v.get(p.slot)).cloned() else { continue };
        let name = vm.db.names.get(p.name).to_string();
        match v {
            Value::Object(Some(o)) if vm.db.objects.get(o).class.is_some_and(|c| vm.is_child_of(c, actor_class)) => {
                out.push((name, TravelValue::Actor(vm.db.names.get(vm.db.objects.get(o).name).to_string())));
            }
            Value::Object(Some(_)) => {}
            v => out.push((name, TravelValue::Plain(v))),
        }
    }
    Some(TravelActor { class, name: vm.db.names.get(vm.db.objects.get(actor).name).to_string(), props: out })
}

/// The pawns whose inventory goes along: the player's pawn, the level's inventory carrier (`Level.InventoryCarrierPawn`: HP3
/// files every collection, beans, cakes, pasties, on it, and the HUD counts from it) and every pawn of the carrier class (Harry).
/// The carrier can be another pawn than the one played (Ron, Hermione, a creature), and `ServerTravel` points it at the played
/// pawn just before leaving, so all of them are looked at.
fn inventory_holders(vm: &mut Vm<'_>, pc: ObjId) -> Vec<ObjId> {
    let mut out: Vec<ObjId> = Vec::new();
    let mut add = |vm: &mut Vm<'_>, p: Option<ObjId>| {
        if let Some(p) = p
            && !vm.db.objects.get(p).deleted
            && !out.contains(&p)
        {
            out.push(p);
        }
    };
    let pawn = get(vm, pc, "Pawn").as_obj();
    add(vm, pawn);
    if let Some(li) = world(vm).level_info {
        let carrier = get(vm, li, "InventoryCarrierPawn").as_obj();
        add(vm, carrier);
        if let Some(class) = carrier_class(vm, li) {
            for a in world(vm).actors.clone() {
                if vm.db.objects.get(a).class.is_some_and(|c| vm.is_child_of(c, class)) {
                    add(vm, Some(a));
                }
            }
        }
    }
    out
}

/// The class whose pawn carries the player's inventory (`LevelInfo.InventoryCarrierClass`, `HGame.Harry`).
fn carrier_class(vm: &mut Vm<'_>, li: ObjId) -> Option<hp3_script::StructId> {
    let c = get(vm, li, "InventoryCarrierClass").as_obj()?;
    vm.class_of(c)
}

/// The player's pawn and the inventory that goes with it, for the next level: the pawn's own `travel` values first, then each
/// item of every inventory holder once.
fn export_player(vm: &mut Vm<'_>, pc: ObjId) -> Option<(String, Vec<TravelActor>)> {
    let pawn = get(vm, pc, "Pawn").as_obj();
    let mut actors = Vec::new();
    if let Some(p) = pawn {
        actors.extend(export_travel(vm, p));
    }
    let mut seen = std::collections::HashSet::new();
    for holder in inventory_holders(vm, pc) {
        let mut at = get(vm, holder, "Inventory").as_obj();
        for _ in 0..256 {
            let Some(a) = at else { break };
            if !seen.insert(a) {
                break;
            }
            actors.extend(export_travel(vm, a));
            at = get(vm, a, "Inventory").as_obj();
        }
    }
    (pawn.is_some() || !actors.is_empty()).then(|| (player_name(vm, pc), actors))
}

/// The item of `class` already in `pawn`'s inventory chain.
fn held_item(vm: &mut Vm<'_>, pawn: ObjId, class: hp3_script::StructId) -> Option<ObjId> {
    let mut at = get(vm, pawn, "Inventory").as_obj();
    for _ in 0..256 {
        let a = at?;
        if vm.db.objects.get(a).class == Some(class) && !vm.db.objects.get(a).deleted {
            return Some(a);
        }
        at = get(vm, a, "Inventory").as_obj();
    }
    None
}

/// The name the player's replication info holds (empty when it has none).
fn player_name(vm: &mut Vm<'_>, pc: ObjId) -> String {
    match get(vm, pc, "PlayerReplicationInfo").as_obj() {
        Some(pri) => get(vm, pri, "PlayerName").as_str().to_string(),
        None => String::new(),
    }
}

/// Brings the travelling actors of `info` into the new level: the pawn entry is the player's pawn itself; the items are spawned
/// for the level's inventory carrier (`Level.InventoryCarrierPawn`, else the player's pawn) and owned by it. An item of a class
/// the carrier already holds is not made again: a collection (beans, cakes, pasties) adds its count to the one there, anything
/// else (the wand every pawn starts with) is dropped. Then the `travel` values are put back, with actor references pointing at
/// the new copies; the inventory links are left to the items' `TravelPreAccept` (`GiveTo` files each one on its owner).
/// Returns the actors that came, in the order they were listed.
fn accept_travel(vm: &mut Vm<'_>, pc: ObjId, pawn: ObjId, info: &TravelInfo) -> Vec<ObjId> {
    // The entry under the player's name; failing that (a game with one player) the first one.
    let name = player_name(vm, pc);
    let Some((_, actors)) = info.iter().find(|(n, _)| *n == name).or_else(|| info.first()) else { return Vec::new() };
    let pawn_class = vm.db.find("Pawn");
    let collection_class = vm.db.find("KWInventoryCollection");
    let carrier = world(vm).level_info.and_then(|li| get(vm, li, "InventoryCarrierPawn").as_obj()).filter(|&c| !vm.db.objects.get(c).deleted).unwrap_or(pawn);
    let mut accepted: Vec<(String, ObjId, &TravelActor)> = Vec::new();
    let mut made: HashMap<hp3_script::StructId, ObjId> = HashMap::default();
    for t in actors {
        let is_pawn = pawn_class.is_some_and(|p| vm.is_child_of(t.class, p));
        if is_pawn {
            accepted.push((t.name.clone(), pawn, t));
            continue;
        }
        let have = made.get(&t.class).copied().or_else(|| held_item(vm, carrier, t.class));
        if let Some(have) = have {
            if collection_class.is_some_and(|c| vm.is_child_of(t.class, c)) {
                let count = t.props.iter().find(|(p, _)| p.eq_ignore_ascii_case("iNumItems")).map(|(_, v)| match v {
                    TravelValue::Plain(v) => v.as_int(),
                    TravelValue::Actor(_) => 0,
                });
                if let Some(count) = count {
                    let total = get(vm, have, "iNumItems").as_int() + count;
                    set(vm, have, "iNumItems", Value::Int(total));
                }
            }
            continue;
        }
        let class_obj = vm.db.get(t.class).object;
        let actor = natives::spawn(vm, Some(carrier), &[Value::Object(Some(class_obj)), Value::Object(Some(carrier))]).ok().and_then(|v| v.as_obj());
        if let Some(a) = actor {
            made.insert(t.class, a);
            accepted.push((t.name.clone(), a, t));
        }
    }
    for (_, actor, t) in &accepted {
        for (prop, value) in &t.props {
            // The chain is built again by GiveTo; the old links would point items at each other twice.
            if prop.eq_ignore_ascii_case("Inventory") {
                continue;
            }
            let v = match value {
                TravelValue::Plain(v) => v.clone(),
                TravelValue::Actor(target) => match accepted.iter().find(|(n, ..)| n == target) {
                    Some((_, o, _)) => Value::Object(Some(*o)),
                    None => continue,
                },
            };
            set(vm, *actor, prop, v);
        }
    }
    accepted.iter().map(|(_, a, _)| *a).collect()
}

thread_local! {
    /// URL options for the next level load (set by `travel`, read once by the load).
    static NEXT_OPTIONS: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

fn world_options_set(options: &str) {
    NEXT_OPTIONS.with(|o| *o.borrow_mut() = options.to_string());
}

thread_local! {
    /// The portal of the URL being travelled to (`#portal`), read once by the login.
    static NEXT_PORTAL: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

thread_local! {
    static NEXT_WORLD_INFO: std::cell::Cell<Option<ObjId>> = const { std::cell::Cell::new(None) };
}

fn take_world_info() -> Option<ObjId> {
    NEXT_WORLD_INFO.with(|w| w.take())
}

fn take_world_options() -> String {
    NEXT_OPTIONS.with(|o| std::mem::take(&mut *o.borrow_mut()))
}

/// Property flag of values that travel with the player to the next level.
const CPF_TRAVEL: u32 = 0x0001_0000;

/// A level switch the game asked for (`Level.NextURL`), once its countdown has run out.
pub fn take_travel(vm: &mut Vm<'_>) -> Option<String> {
    world(vm).pending_travel.take()
}

/// The map a travel URL names (`HP3_Adv1Express.unr?options#portal` gives `HP3_Adv1Express`).
pub fn url_map(url: &str) -> String {
    let m = url.split(['?', '#']).next().unwrap_or("").trim();
    m.strip_suffix(".unr").or_else(|| m.strip_suffix(".UNR")).unwrap_or(m).to_string()
}

/// Switches to the level of `url` in the same VM: objects made outside the level (persistent game state)
/// survive; the player's pawn takes its `travel` properties along.
pub fn travel(vm: &mut Vm<'_>, url: &str) -> Result<Rc<Package>, VmError> {
    let map_name = url_map(url);
    let map = vm.db.set.get(&map_name).map_err(|e| VmError::Runtime(format!("travel to {map_name}: {e}")))?;
    // What goes with the player: a restart keeps what came in with the level; otherwise, when the level asked
    // for items to travel (`bNextItems`), the player's pawn and its inventory are described.
    let restart = url.trim_start().to_ascii_uppercase().starts_with("?RESTART");
    // Leaving a level the engine has every actor put what it keeps across levels into the game's store first.
    if !restart && world(vm).level_info.is_some_and(|li| get(vm, li, "bNextItems").as_bool()) {
        for a in world(vm).actors.clone() {
            if !vm.db.objects.get(a).deleted {
                fire(vm, a, "SavePersistentData", &mut []);
            }
        }
    }
    let mut info = world(vm).travel_info.clone();
    if !restart {
        let items = world(vm).level_info.is_some_and(|li| get(vm, li, "bNextItems").as_bool());
        if items {
            if let Some(pc) = world(vm).player
                && let Some((name, actors)) = export_player(vm, pc)
            {
                info.retain(|(n, _)| *n != name);
                info.push((name, actors));
            }
        } else {
            info.clear();
        }
    }
    if true {
        let items = world(vm).level_info.is_some_and(|li| get(vm, li, "bNextItems").as_bool());
        eprintln!("travel to {url}: restart {restart} items {items}, {} players", info.len());
        for (n, actors) in &info {
            for a in actors {
                eprintln!("  {n}: {} {} ({} values)", vm.db.names.get(vm.db.get(a.class).name), a.name, a.props.len());
            }
        }
    }
    // The values the scripts saved into the game's store (`SavePersistentData`, when the level asks for items to travel) go
    // to the world info, which outlives the level, and the next level's store takes them from there.
    if !restart
        && world(vm).level_info.is_some_and(|li| get(vm, li, "bNextItems").as_bool())
        && let Some(store) = world(vm).game.and_then(|g| get(vm, g, "PersistentStore").as_obj())
    {
        fire(vm, store, "TravelExporting", &mut []);
    }
    // The chosen profile (New Game slot) belongs to the player, not the level.
    interactions::carry_user_id(vm);
    // The old level goes: its actors are dropped from the VM's per-object state.
    let old = vm.ext.take().and_then(|e| e.downcast::<World>().ok()).ok_or_else(|| VmError::Runtime("no world".into()))?;
    for a in &old.actors {
        vm.latent.remove(a);
        vm.state_pc.remove(a);
        vm.db.objects.get_mut(*a).deleted = true;
    }
    let options = url.find('?').map(|i| url[i..].split('#').next().unwrap_or("").to_string()).unwrap_or_default();
    world_options_set(&options);
    NEXT_PORTAL.with(|p| *p.borrow_mut() = url.split_once('#').map(|(_, portal)| portal.trim().to_string()).unwrap_or_default());
    NEXT_WORLD_INFO.with(|w| w.set(old.world_info));
    load_with(vm, &map, old.config, Some(info))?;
    // Loading a saved game: the map is loaded as for any travel and the saved level put over it.
    if let Some(slot) = save::option_slot(url) {
        save::finish_load(vm, slot);
    }
    Ok(map)
}

/// Like `load`; `travel` is what came with the player from the previous level (`None`: a map opened directly).
pub fn load_with(vm: &mut Vm<'_>, map: &Rc<Package>, config: hp3_core::Config, travel: Option<TravelInfo>) -> Result<(), VmError> {
    let game_class = config.get("Engine.Engine", "DefaultGame").map(str::to_string);
    let game_class = game_class.as_deref();
    let actor_class = vm.db.find("Actor").ok_or_else(|| VmError::Runtime("no Actor class".into()))?;
    natives::register(vm);
    audio::register(vm);
    particles::register(vm);
    movie::register(vm);
    anim::register(vm);
    nav::register(vm);
    perception::register(vm);
    canvas::register(vm);
    interactions::register(vm);
    gui::register(vm);
    save::register(vm);
    memprobe("start");
    let configured = config::apply(vm, &config);
    if std::env::var_os("HP3_CONFIG_DEBUG").is_some() {
        eprintln!("config: {configured} class default values read from the ini files");
    }
    // Every scripted object of the map, so references between them resolve to live instances.
    let mut level_order = Vec::new();
    let mut level_model = None;
    for i in 0..map.exports.len() {
        if map.exports[i].class.is_none() {
            continue;
        }
        let chain = vm.db.set.export_chain(map, i);
        if let Ok(o) = object::read_export(map, i, &chain)
            && matches!(o.body, Body::Deleted { .. })
        {
            continue;
        }
        if map.export_class_name(i).eq_ignore_ascii_case("Level")
            && let Ok(o) = object::read_export(map, i, &chain)
        {
            let base = map.exports[i].serial_offset + map.exports[i].serial_size - o.tail.len();
            let mut r = hp3_package::Reader::with_base(o.tail, base);
            if let Some(Ok(Asset::Level(level))) = hp3_assets::read_native("Level", map, &mut r) {
                level_order = level.actors.clone();
                level_model = Some(level.model);
            }
        }
        instantiate(vm, map, i);
    }
    let mut actors = Vec::new();
    let mut level_info = None;
    let level_info_class = vm.db.find("LevelInfo");
    for r in level_order {
        let Some(e) = r.export() else { continue };
        // Editor-only objects (the viewport cameras a map is saved with) are marked not for the client and not
        // for the server: the game never loads them. Left in, those cameras are player controllers that scripts
        // looking for "the" player controller find first (COMC's countdown went to a camera's HUD, so the ring
        // counter never showed).
        const NOT_FOR_CLIENT: u32 = 0x0010_0000;
        const NOT_FOR_SERVER: u32 = 0x0020_0000;
        if map.exports[e].flags & (NOT_FOR_CLIENT | NOT_FOR_SERVER) == NOT_FOR_CLIENT | NOT_FOR_SERVER {
            continue;
        }
        let id = vm.db.obj_for(map, e);
        let Some(class) = vm.db.objects.get(id).class else { continue };
        if !vm.is_child_of(class, actor_class) {
            continue;
        }
        if level_info.is_none() && level_info_class.is_some_and(|l| vm.is_child_of(class, l)) {
            level_info = Some(id);
        }
        actors.push(id);
    }
    // Scripts writing their own variables (above Actor's) do not change what the per-frame passes look at.
    memprobe("actors instantiated");
    vm.db.objects.quiet_from = vm.db.find("Actor").map_or(0, |c| vm.db.get(c).slots);
    vm.ext = Some(Box::new(World {
        map: map.clone(),
        actors: actors.clone(),
        level_info,
        game: None,
        player: None,
        time: 0.0,
        timers: HashMap::default(),
        actor_class,
        slots: HashMap::default(),
        frame: 0,
        grid: None,
        errors: Vec::new(),
        spawned: 0,
        input: input::Input::from_config(&config),
        collision: collision::Collision::default(),
        touch: Default::default(),
        anim: Default::default(),
        nav: Default::default(),
        opacity: Default::default(),
        canvas: Default::default(),
        viewport: Default::default(),
        native_fonts: Default::default(),
        pending_travel: None,
        quit_requested: false,
        settings_version: 0,
        step_smooth: Default::default(),
        stair_floor: Default::default(),
        volume_hulls: None,
        travel_info: travel.clone().unwrap_or_default(),
        url_options: String::new(),
        world_info: None,
        save_requested: false,
        loaded_from_save: false,
        audio: Default::default(),
        movies: Default::default(),
        particles: Default::default(),
        debug_air_speed: None,
        phys_slots: None,
        controller_classes: Default::default(),
        tick_profile: Default::default(),
        scenery_classes: Default::default(),
        volume_checked: Default::default(),
        default_volume: None,
        ambient_classes: Default::default(),
        vendor_setup_since: Default::default(),
        far_owed: Default::default(),
        lip_syncs: Vec::new(),
        ever_base: Default::default(),
        ever_base_valid: false,
        tick_classes: Default::default(),
        volumes_marked: false,
        memo_tick: Default::default(),
        memo_boxes: Default::default(),
        memo_baked: Default::default(),
        memo_draw: Default::default(),
        memo_pawns: Default::default(),
        memo_projectors: Default::default(),
        memo_emitters: Default::default(),
        tick_skipped: Default::default(),
        actor_slots: Default::default(),
        quidditch: None,
        quidditch_gate: Default::default(),
        quidditch_travel_blocked: false,
        config,
    }));
    memprobe("world made");
    let collision = build_collision(vm, map, level_model, &actors);
    memprobe("collision built");
    world(vm).collision = collision;
    settings::apply_saved(vm);
    // HP3's world info (game states, persistent values) lives across levels: made once, given to each level.
    let world_info = take_world_info().or_else(|| {
        let class = world(vm).config.get("Engine.Engine", "WorldInfo").map(str::to_string)?;
        let c = find_object(vm, &class).and_then(|o| vm.class_of(o))?;
        let n = vm.db.names.intern("WorldInfo");
        Some(vm.new_object(c, n))
    });
    world(vm).world_info = world_info;
    if let (Some(li), Some(wi)) = (level_info, world_info) {
        set(vm, li, "WorldInfo", Value::Object(Some(wi)));
    }
    // Loading a map puts the map's name into an empty LevelEnterText, as the original's LoadMap does. The game reads
    // the level it is leaving from it (KWHeroController.PreClientTravel -> KWPawn.PreviousLevelName), and the next
    // level's SmartStart for that name places the player: left empty, no SmartStart matched and the player arrived
    // at the plain PlayerStart (the inside hub's is in its storage room for actors and props).
    fill_level_enter_text(vm);
    // The game exists before the level's actors begin play; they read Level.Game.
    if let Some(gc) = game_class.and_then(|g| find_object(vm, g)) {
        let game = natives::spawn(vm, level_info, &[Value::Object(Some(gc))])?.as_obj();
        world(vm).game = game;
        if let (Some(li), Some(g)) = (level_info, game) {
            set(vm, li, "Game", Value::Object(Some(g)));
            let options = take_world_options();
            fire(vm, g, "InitGame", &mut [Value::Str(options.clone()), Value::Str(String::new())]);
            world(vm).url_options = options;
            // The game keeps what the player has done (stars, finds, pickups taken) in a store its scripts expect to exist;
            // a level that came by travel gets back the values the last level handed over (`WorldInfo.TempPers*`).
            if get(vm, g, "PersistentStore").as_obj().is_none()
                && let Some(sc) = find_object(vm, "KWGame.PersistentStore")
                && let Some(store) = natives::spawn(vm, level_info, &[Value::Object(Some(sc))])?.as_obj()
            {
                set(vm, g, "PersistentStore", Value::Object(Some(store)));
                if travel.is_some() && world_info.is_some() {
                    fire(vm, store, "TravelPostAccept", &mut []);
                }
            }
        }
    }
    // The level's scripts find actors by `Label` (`GetActorFromLabel`: a crab's patrol points, for one); the editor stored
    // that name in the actor's `Tag`, so an actor with no label of its own is labelled with its tag.
    for &a in &actors {
        let tag = get(vm, a, "Tag").as_name();
        if tag != Name::NONE && get(vm, a, "Label").as_str().is_empty() {
            let text = vm.db.names.get(tag).to_string();
            set(vm, a, "Label", Value::Str(text));
        }
    }
    // The level has a default physics volume before anything begins play (a map may carry it as a deleted actor, so one is
    // made); actors with none, or the deleted one the map placed them in (not one of the level's actors), are moved to it: gravity is read from it when a pawn works out its jump.
    memprobe("game made");
    if let Some(default) = default_physics_volume(vm) {
        let live: std::collections::HashSet<ObjId> = actors.iter().copied().collect();
        for &a in &actors {
            if get(vm, a, "PhysicsVolume").as_obj().is_none_or(|v| !live.contains(&v)) {
                set(vm, a, "PhysicsVolume", Value::Object(Some(default)));
            }
        }
    }
    // `HP3_GAMESTATE=<name>` (debugging): starts the level in that game state, as if the story had got there.
    if let (Ok(state), Some(game)) = (std::env::var("HP3_GAMESTATE"), world(vm).level_info.and_then(|li| get(vm, li, "Game").as_obj())) {
        set(vm, game, "CurrentGameState", Value::Str(state));
    }
    // Actors that do not belong to the current game state are hidden and stop colliding.
    for &a in &actors {
        fire(vm, a, "FilterForCurrentGameState", &mut []);
    }
    // Pawns placed in the level get their controller in `Pawn.PostBeginPlay` only while the level is starting.
    if let Some(li) = world(vm).level_info {
        set(vm, li, "bStartup", Value::Bool(true));
    }
    for &a in &actors {
        fire(vm, a, "PreBeginPlay", &mut []);
    }
    for &a in &actors {
        fire(vm, a, "BeginPlay", &mut []);
    }
    for &a in &actors {
        fire(vm, a, "PostBeginPlay", &mut []);
        // Actors with a mesh now register the script events their animations raise.
        if get(vm, a, "Mesh").as_obj().is_some() {
            fire(vm, a, "AddAnimNotifys", &mut []);
        }
    }
    for &a in &actors {
        fire(vm, a, "SetInitialState", &mut []);
    }
    if let Some(li) = world(vm).level_info {
        set(vm, li, "bStartup", Value::Bool(false));
    }
    // A pawn the map placed a little inside its floor would find none and fall through it.
    for &a in &actors {
        physics::lift_out_of_floor(vm, a);
    }
    // The persistent store has the saved state of the earlier levels: every actor takes what is its own from it, then settles.
    // That is every actor alive now, not only the map's: the game's own managers (`WizardCardManager`, `ChallengeStarManager`,
    // made by `HGame.PreBeginPlay`) hold the cards and shields.
    let live: Vec<ObjId> = world(vm).actors.clone();
    memprobe("before PostBeginPlay");
    for &a in &live {
        if !vm.db.objects.get(a).deleted {
            fire(vm, a, "RestorePersistentData", &mut []);
        }
    }
    for &a in &live {
        if !vm.db.objects.get(a).deleted {
            fire(vm, a, "PostPersistentDataRestored", &mut []);
        }
    }
    // Actors with an `AttachTag` ride on the actor with that tag or name (a knight on its panel).
    let by_name = |vm: &mut Vm<'_>, o: ObjId| vm.db.objects.get(o).name;
    for &a in &actors {
        let tag = get(vm, a, "AttachTag").as_name();
        if tag == hp3_script::Name::NONE {
            continue;
        }
        let mut found = None;
        for &b in &actors {
            if b != a && (get(vm, b, "Tag").as_name() == tag || by_name(vm, b) == tag) {
                found = Some(b);
                break;
            }
        }
        if let Some(b) = found {
            natives::set_base(vm, a, Some(b), false);
        }
    }
    memprobe("begin-play done");
    if let Some(g) = world(vm).game {
        let options = world(vm).url_options.clone();
        let portal = NEXT_PORTAL.with(|p| std::mem::take(&mut *p.borrow_mut()));
        let mut args = [Value::Str(portal), Value::Str(options), Value::Str(String::new())];
        let pc = fire(vm, g, "Login", &mut args).as_obj();
        memprobe("after Login");
        if let Some(pc) = pc {
            world(vm).player = Some(pc);
            // The engine gives the local player's controller its viewport (console, GUI) and input object.
            interactions::init(vm, pc);
            fire(vm, pc, "InitInputSystem", &mut []);
            fire(vm, g, "PostLogin", &mut [Value::Object(Some(pc))]);
            memprobe("after PostLogin");
        }
        follow_player(vm);
        // A loaded map finishes by accepting the player's travel: both are told it is coming, what travelled is
        // put back, the game takes the inventory in, and then they are told it has arrived (last listed first).
        if let Some(pc) = world(vm).player {
            if let Some(pawn) = get(vm, pc, "Pawn").as_obj() {
                if travel.is_none() {
                    arrive_from(vm, pawn);
                }
                fire(vm, pc, "TravelPreAccept", &mut []);
                fire(vm, pawn, "TravelPreAccept", &mut []);
                let info = travel.clone().unwrap_or_default();
                let accepted = accept_travel(vm, pc, pawn, &info);
                for &a in accepted.iter().rev() {
                    fire(vm, a, "TravelPreAccept", &mut []);
                }
                fire(vm, g, "AcceptInventory", &mut [Value::Object(Some(pawn))]);
                for &a in accepted.iter().rev() {
                    fire(vm, a, "TravelPostAccept", &mut []);
                }
            }
            fire(vm, pc, "TravelPostAccept", &mut []);
            init_camera_settings(vm, pc);
            memprobe("after travel accept");
            // The game starts on its local map with the front end (after the intro movies, not played yet);
            // HP3_MENU=<Package.Class> opens a page on any map.
            let local = world(vm).config.get("URL", "LocalMap").map(|m| m.trim_end_matches(".unr").to_ascii_lowercase());
            let page = std::env::var("HP3_MENU").ok().or_else(|| {
                (local.as_deref() == Some(map.name.to_ascii_lowercase().as_str())).then(|| "HGame.HPPageFrontend".to_string())
            });
            if let Some(page) = page {
                interactions::open_menu(vm, &page);
            }
        }
    }
    // `HP3_MATCH=<opponent>` (debugging): the level is loaded and the player is logged in, so a Quidditch
    // match can build its arena and spawn into it. Every other level leaves the match state empty.
    quidditch::glue::start_from_env(vm)?;
    Ok(())
}

/// A map loaded directly has no camera settings carried over from the previous level (or set by the new
/// game's intro): if the player's camera has none, it takes its pawn's, as switching pawns does.
fn init_camera_settings(vm: &mut Vm<'_>, pc: ObjId) {
    let (Some(cam), Some(pawn)) = (get(vm, pc, "Camera").as_obj(), get(vm, pc, "Pawn").as_obj()) else { return };
    let unset = get(vm, cam, "CurrentSet").fields().get(1).is_none_or(|d| d.as_float() == 0.0);
    if unset {
        fire(vm, cam, "InitTarget", &mut [Value::Object(Some(pawn)), Value::Float(0.0), Value::Bool(true)]);
    }
}

/// Makes the player arrive as if travelling from a neighbouring level, so the level's `SmartStart` for it places
/// the player, attaches the companions and starts the game camera. `HP3_FROM` names the previous level;
/// unset, the map's first `SmartStart` is used; `none` arrives from nowhere (a plain start).
fn arrive_from(vm: &mut Vm<'_>, pawn: ObjId) {
    let wanted = std::env::var("HP3_FROM").ok();
    if wanted.as_deref().is_some_and(|w| w.eq_ignore_ascii_case("none")) {
        return;
    }
    let Some(smart) = vm.db.find("SmartStart") else { return };
    let actors = world(vm).actors.clone();
    let mut chosen = None;
    for a in actors {
        if !vm.db.objects.get(a).class.is_some_and(|c| vm.is_child_of(c, smart)) {
            continue;
        }
        let from = get(vm, a, "PreviousLevelName").as_str().to_string();
        if from.is_empty() {
            continue;
        }
        match &wanted {
            Some(w) if from.eq_ignore_ascii_case(w) => {
                chosen = Some(from);
                break;
            }
            None if chosen.is_none() => chosen = Some(from),
            _ => {}
        }
    }
    if let Some(from) = chosen {
        set(vm, pawn, "PreviousLevelName", Value::Str(from));
    }
}

/// The player's view this frame from the game's `PlayerCalcView`: location, rotation (pitch, yaw, roll) and
/// horizontal field of view. `None` when the game gives no view (location at the origin).
pub fn player_view(vm: &mut Vm<'_>) -> Option<(glam::Vec3, [i32; 3], f32)> {
    // A Quidditch match is watched from its own chase camera, not from the game's camera actor (which knows
    // nothing about brooms). Every other level takes the view from the script, as before.
    if let Some(view) = quidditch::glue::view(vm) {
        return Some(view);
    }
    let pc = world(vm).player?;
    // The view is asked for once per drawn frame, with the controller as the view actor and its own location and
    // rotation as the starting values. HP3's script computes a new view only when `bTicked` differs from
    // `bViewUpdated` (once per tick); a view asked for twice in a tick is the one computed the first time.
    let mut fresh = false;
    let (loc, rot) = loop {
        let mut args = [Value::Object(Some(pc)), get(vm, pc, "Location"), get(vm, pc, "Rotation")];
        vm.event(pc, "PlayerCalcView", &mut args).ok()?;
        let (loc, rot) = (args[1].fields().to_vec(), args[2].fields().to_vec());
        let valid = loc.len() == 3 && rot.len() == 3 && !loc.iter().all(|v| v.as_float() == 0.0);
        if valid {
            break (loc, rot);
        }
        // Nothing usable yet (no tick has run since the last view): make the script compute one.
        if fresh {
            return None;
        }
        fresh = true;
        let ticked = get(vm, pc, "bTicked").as_bool();
        set(vm, pc, "bViewUpdated", Value::Bool(!ticked));
    };
    let fov = match get(vm, pc, "FovAngle").as_float() {
        f if f > 1.0 && f < 179.0 => f,
        _ => 90.0,
    };
    Some((
        glam::Vec3::new(loc[0].as_float(), loc[1].as_float(), loc[2].as_float()),
        [rot[0].as_int(), rot[1].as_int(), rot[2].as_int()],
        fov,
    ))
}

thread_local! {
    /// How many times the fight of each Monster Book (by object id) has been started.
    static BOOK_TRIES: std::cell::RefCell<std::collections::HashMap<u32, u32>> = std::cell::RefCell::new(std::collections::HashMap::new());
}

/// A Monster Book that is out of its dormant state without a combat manager has started its fight with nobody to run
/// it: the companions are sent into the fight the way a cutscene's `FightWithHero` command does, which creates the
/// manager that spawns the pages, runs the health bar and ends the fight.
fn start_book_fights(vm: &mut Vm<'_>) {
    let Some(book_class) = vm.db.find("MonsterBook") else { return };
    let Some(pc) = world(vm).player else { return };
    let Some(lead) = get(vm, pc, "Pawn").as_obj() else { return };
    let actors = world(vm).actors.clone();
    let books: Vec<ObjId> = actors
        .into_iter()
        .filter(|&a| !vm.db.objects.get(a).deleted && vm.db.objects.get(a).class.is_some_and(|c| vm.is_child_of(c, book_class)))
        .collect();
    for book in books {
        let awake = vm.state_of(book).is_some_and(|s| !vm.db.names.get(vm.db.get(s).name).eq_ignore_ascii_case("stateNotFighting"));
        if !awake || get(vm, book, "mbcm").as_obj().is_some() || get(vm, book, "bBattleEnded").as_bool() {
            continue;
        }
        let label = get(vm, book, "Label").as_str().to_string();
        if label.is_empty() {
            continue;
        }
        let tries = BOOK_TRIES.with(|t| {
            let mut t = t.borrow_mut();
            let n = t.entry(book.0).or_insert(0);
            *n += 1;
            *n
        });
        if tries > 3 {
            continue;
        }
        let Some(companion) = get(vm, lead, "TrailingChar").as_obj() else { continue };
        let name = get(vm, companion, "Label").as_str().to_string();
        // The command a companion trigger gives: companion, boss label, controller type, manager tag.
        let command = format!("TrailingCharFightWithHero {name} {label} HGame.HPMBookCompanionController None");
        vm.log.push(format!("book fight: starting it with '{command}'"));
        fire(vm, lead, "TrailingCharFightWithHero", &mut [Value::Str(command)]);
        let manager = get(vm, book, "mbcm").as_obj().map(|m| hp3_script::natives::object_path(vm, m));
        vm.log.push(format!("book fight: combat manager after the command: {manager:?}"));
    }
}

/// Advances the level by `dt` seconds: actor ticks, timers, then state code.
/// The level's static meshes and volumes are baked into the collision when it loads; a script can turn an actor's collision
/// off (or on) later, or destroy it. Every tick the baked shapes are made to follow the actors' current flags, which is what the
/// original's live collision hash amounts to: a trace made after a script's change sees it.
fn sync_baked_collision(vm: &mut Vm<'_>) {
    let owners = world(vm).collision.baked_owners().to_vec();
    if !world(vm).volumes_marked {
        world(vm).volumes_marked = true;
        // The ring knights of the Carpe levels are placed with a per-axis stretch on top of their draw scale and tower over the
        // free knight; they are drawn at their draw scale alone.
        // Carpe Retractum B: the invisible volume across the doorway of the knight ring would keep the player out of the chamber where
        // the knights' goodies land; there only the models' own collision is in the way.
        if world(vm).map.name.eq_ignore_ascii_case("HP3_Ch1CarpeRetractumB") {
            let actors = world(vm).actors.clone();
            for a in actors {
                if vm.db.names.get(vm.db.objects.get(a).name).eq_ignore_ascii_case("KWBlockingVolume0") {
                    set(vm, a, "bBlockPlayers", Value::Bool(false));
                }
            }
        }
        // A charmed skeleton with `bEmbedCompanionCastTarget` set shatters at the first Rictusempra hit; cleared, the first hit
        // takes its arm off and the second shatters it, as in the original where they take two hits even from a perfect cast.
        if let Some(skeleton) = vm.db.find("CharmedSkeleton") {
            let actors = world(vm).actors.clone();
            for a in actors {
                if vm.db.objects.get(a).class.is_some_and(|c| vm.is_child_of(c, skeleton)) {
                    set(vm, a, "bEmbedCompanionCastTarget", Value::Bool(false));
                }
            }
        }
        if let Some(knight) = vm.db.find("KnightSpawner") {
            let actors = world(vm).actors.clone();
            for a in actors {
                if vm.db.objects.get(a).class.is_some_and(|c| vm.is_child_of(c, knight)) {
                    set(vm, a, "DrawScale3D", Value::Struct(vec![Value::Float(1.0), Value::Float(1.0), Value::Float(1.0)].into()));
                }
            }
        }
        let volumes: std::collections::HashSet<ObjId> = owners
            .iter()
            .copied()
            .filter(|&a| get(vm, a, "bWorldGeometry").as_bool() && get(vm, a, "StaticMesh").as_obj().is_none())
            .collect();
        world(vm).collision.mark_volumes(&volumes);
    }
    let slots = (actor_slot(vm, "bCollideActors"), actor_slot(vm, "bBlockActors"), actor_slot(vm, "bBlockPlayers"));
    // Only actors written since the last pass can have new flags.
    let mut memo = std::mem::take(&mut world(vm).memo_baked);
    let pass = memo.begin(&mut vm.db.objects);
    for a in owners {
        if memo.get(&pass, &vm.db.objects, a).is_some() {
            continue;
        }
        memo.set(a, 1);
        let (collide, actors, players) = (read(vm, a, slots.0).as_bool(), read(vm, a, slots.1).as_bool(), read(vm, a, slots.2).as_bool());
        let active = !vm.db.objects.get(a).deleted && collide && (actors || players);
        let c = &mut world(vm).collision;
        c.set_blocks(a, actors, players);
        c.set_active(a, active);
    }
    world(vm).memo_baked = memo;
}

/// Makes an actor's baked collision follow its current flags: at once when a script changes them, and in the periodic pass
/// for changes made some other way.
pub fn refresh_baked_collision(vm: &mut Vm<'_>, a: ObjId) {
    let (blocks_actors, blocks_players) = (get(vm, a, "bBlockActors").as_bool(), get(vm, a, "bBlockPlayers").as_bool());
    let active = !vm.db.objects.get(a).deleted && get(vm, a, "bCollideActors").as_bool() && (blocks_actors || blocks_players);
    world(vm).collision.set_blocks(a, blocks_actors, blocks_players);
    world(vm).collision.set_active(a, active);
}

/// Puts the loaded map's name into an empty `LevelEnterText` (see `load_with`); also run after a save is restored,
/// since a save written before the name was filled carries the empty text back.
pub fn fill_level_enter_text(vm: &mut Vm<'_>) {
    let Some(li) = world(vm).level_info else { return };
    if get(vm, li, "LevelEnterText").as_str().is_empty() {
        let name = world(vm).map.name.clone();
        set(vm, li, "LevelEnterText", Value::Str(name));
    }
}

/// A level switch the game asked for (`Level.NextURL`) happens when its countdown runs out.
fn count_down_travel(vm: &mut Vm<'_>, li: ObjId, dt: f32) {
    let next = get(vm, li, "NextURL").as_str().to_string();
    if !next.is_empty() {
        // A Quidditch match is played on a map whose own scripts switch the level when they are done
        // (`quidditch::stage` clears the cutscene that does it, this is the backstop): while a match runs the
        // switch is refused, so the pitch cannot be taken away from under the player. Said once per level.
        if world(vm).quidditch.is_some() {
            set(vm, li, "NextURL", Value::Str(String::new()));
            if !world(vm).quidditch_travel_blocked {
                world(vm).quidditch_travel_blocked = true;
                let line = format!("quidditch: level travel blocked ({next})");
                println!("{line}");
                vm.log.push(line);
            }
            return;
        }
        let countdown = get(vm, li, "NextSwitchCountdown").as_float() - dt;
        set(vm, li, "NextSwitchCountdown", Value::Float(countdown));
        if countdown <= 0.0 {
            set(vm, li, "NextURL", Value::Str(String::new()));
            world(vm).pending_travel = Some(next);
        }
    }
}

pub fn tick(vm: &mut Vm<'_>, dt: f32) {
    // The step is scaled by the level's time dilation; while someone has paused the game only the menus run, and a level
    // switch still happens (a pause page's "leave" asks for one while the game is paused).
    let dilation = world(vm).level_info.map(|li| get(vm, li, "TimeDilation").as_float()).filter(|d| *d > 0.0).unwrap_or(1.0);
    let dt = dt * dilation;
    if let Some(li) = world(vm).level_info
        && get(vm, li, "Pauser").as_obj().is_some()
    {
        follow_player(vm);
        interactions::tick(vm, dt);
        count_down_travel(vm, li, dt);
        return;
    }
    let actors = {
        let w = world(vm);
        w.time += dt;
        w.frame += 1;
        w.actors.clone()
    };
    if world(vm).frame % 30 == 0 {
        start_book_fights(vm);
    }
    let t_pre = std::time::Instant::now();
    perception::tick(vm);
    let t_perception = t_pre.elapsed();
    if std::env::var_os("HP3_NO_BAKED_SYNC").is_none() {
        sync_baked_collision(vm);
    }
    let t_sync = t_pre.elapsed() - t_perception;
    if let Some(li) = world(vm).level_info {
        let t = world(vm).time;
        set(vm, li, "TimeSeconds", Value::Float(t));
        movie::tick(vm);
        let t_particles = std::time::Instant::now();
        lipsync::tick(vm);
        particles::tick(vm, dt);
        if std::env::var_os("HP3_TICK_PROFILE").is_some() {
            let p = &mut world(vm).tick_profile;
            *p.entry("pre: perception").or_default() += t_perception;
            *p.entry("pre: baked collision sync").or_default() += t_sync;
            *p.entry("pre: particles").or_default() += t_particles.elapsed();
        }
        for a in particles::finished_actors(vm) {
            destroy_actor(vm, a);
        }
        // A save the game asked for (LevelAction = saving) happens in the engine's next frame, which then
        // clears the action. Saving itself is not implemented yet.
        const LEVACT_SAVING: u8 = 2;
        if get(vm, li, "LevelAction").as_byte() == LEVACT_SAVING {
            if world(vm).save_requested {
                set(vm, li, "LevelAction", Value::Byte(0));
                world(vm).save_requested = false;
                vm.log.push("engine: save requested (not implemented yet)".into());
            } else {
                world(vm).save_requested = true;
            }
        }
        count_down_travel(vm, li, dt);
    }
    if world(vm).frame == 1
        && let Some(pc) = world(vm).player
    {
        // The engine tells the player's controller before the level's first tick.
        // A level put back from a save says so, which keeps the game from saving over the slot it was loaded from.
        let loaded = world(vm).loaded_from_save;
        fire(vm, pc, "OnEnginePreFirstTick", &mut [Value::Bool(loaded)]);
    }
    let profile = std::env::var_os("HP3_TICK_PROFILE").is_some();
    let mut t = std::time::Instant::now();
    let mut mark = |vm: &mut Vm<'_>, phase: &'static str| {
        if profile {
            let e = t.elapsed();
            *world(vm).tick_profile.entry(phase).or_default() += e;
            t = std::time::Instant::now();
        }
    };
    // Each actor goes through the same sequence, in level order (as the original's level tick does it): its
    // animation advances, its `Tick` event runs, then its state code, its timer, its life span and its physics.
    // The player's controller first reads the input, runs `PlayerTick` and clears the input again.
    follow_player(vm);
    interactions::tick(vm, dt);
    mark(vm, "player");
    physics::begin_frame(vm);
    let parity = world(vm).frame % 2 == 1;
    let ticked = actor_slot(vm, "bTicked");
    let life_span = actor_slot(vm, "LifeSpan");
    let player = world(vm).player;
    let controller_class = vm.db.find("Controller");
    let mut clock = std::time::Instant::now();
    let is_static = actor_slot(vm, "bStatic");
    let skip_static = std::env::var_os("HP3_TICK_STATICS").is_none();
    // Where the player is (none while no pawn is controlled, as in a cutscene: then everyone ticks every frame).
    let player_at = player.and_then(|pc| get(vm, pc, "Pawn").as_obj()).map(|p| physics::vec(&get(vm, p, "Location")));
    let player_at = player_at.filter(|_| std::env::var_os("HP3_NO_FAR_THROTTLE").is_none() && !player_is_scripted(vm));
    // Whether an actor is passed over is worked out again only for actors written since the last frame.
    let mut memo = std::mem::take(&mut world(vm).memo_tick);
    let pass = memo.begin(&mut vm.db.objects);
    for &a in &actors {
        let skip = match memo.get(&pass, &vm.db.objects, a) {
            Some(v) => v == 1,
            None => {
                // A static actor that is only scenery (a mesh, a brush, a light, a marker) is never ticked, as in the original: no
                // event, state code, timer or physics. The ones that do something (a door, a desk) are not in the list.
                let skip = vm.db.objects.get(a).deleted || (skip_static && Some(a) != player && read(vm, a, is_static).as_bool() && is_scenery(vm, a));
                memo.set(a, if skip { 1 } else { 2 });
                skip
            }
        };
        if skip || vm.db.objects.get(a).deleted {
            continue;
        }
        // Ambient NPCs far from the player are ticked every few frames, with the time that passed meanwhile.
        let dt = match far_period(vm, a, player_at, controller_class) {
            1 => dt,
            period if (world(vm).frame + far_key(vm, a) as u64) % period as u64 != 0 => {
                *world(vm).far_owed.entry(a).or_default() += dt;
                continue;
            }
            _ => dt + world(vm).far_owed.remove(&a).unwrap_or(0.0),
        };
        if Some(a) == player {
            input::apply(vm, a, dt);
            fire(vm, a, "PlayerTick", &mut [Value::Float(dt)]);
            input::clear_axes(vm, a);
        }
        lap(vm, "actors: input", &mut clock, profile);
        anim::tick_actor(vm, a, dt);
        lap(vm, "actors: anim", &mut clock, profile);
        if let Some(v) = ticked.and_then(|s| vm.db.objects.values_mut_quiet(a).and_then(|v| v.get_mut(s))) {
            *v = Value::Bool(parity);
        }
        let tick_start = profile.then(std::time::Instant::now);
        if !tick_is_invisible(vm, a) {
            fire(vm, a, "Tick", &mut [Value::Float(dt)]);
        }
        class_time(vm, a, "Tick", tick_start);
        lap(vm, "actors: Tick event", &mut clock, profile);
        if vm.db.objects.get(a).deleted {
            continue;
        }
        let state_start = profile.then(std::time::Instant::now);
        let state_result = vm.tick_state(a, dt);
        class_time(vm, a, "state code", state_start);
        if let Err(e) = state_result {
            let at = format!("{} state code: {e}", vm.db.names.get(vm.db.objects.get(a).name));
            let w = world(vm);
            if w.errors.len() < 1000 {
                w.errors.push(at);
            }
        }
        lap(vm, "actors: state code", &mut clock, profile);
        tick_timer(vm, a, dt);
        lap(vm, "actors: timer", &mut clock, profile);
        if let Some(s) = life_span
            && let Some(v) = vm.db.objects.values_mut_quiet(a).and_then(|v| v.get_mut(s))
            && let Value::Float(life) = v
            && *life > 0.0
        {
            *life -= dt;
            if *life <= 0.0 {
                fire(vm, a, "LifeSpanExpired", &mut []);
                // `LifeSpanExpired` normally destroys the actor; one that does not is destroyed anyway.
                destroy_actor(vm, a);
                continue;
            }
        }
        if is_controller(vm, controller_class, a) {
            nav::tick_controller(vm, a, dt);
            nav::face_focus(vm, a);
        }
        lap(vm, "actors: controller", &mut clock, profile);
        let physics_start = profile.then(std::time::Instant::now);
        physics::step_actor(vm, a, dt);
        class_time(vm, a, "physics", physics_start);
        lap(vm, "actors: physics", &mut clock, profile);
    }
    world(vm).memo_tick = memo;
    mark(vm, "actors");
    // The Quidditch match, after the actors have moved: it reads Harry's pawn and puts the snitch where its
    // own simulation has it. A no-op on every level where no match is running.
    quidditch::glue::tick(vm, dt);
    mark(vm, "quidditch");
    touch::frame_pass(vm);
    mark(vm, "touching");
    anim::update_movement(vm, dt);
    mark(vm, "movement anims");
    anim::update_attachments(vm);
    mark(vm, "animation");
    vendor_watchdog(vm);
    // A save the game queued is written once the frame's tick is done, as the original does.
    save::run_queued(vm);
}

/// How long a student may stay in the camera-setup state of a trade before it goes on to the trade itself (seconds).
const VENDOR_SETUP_LIMIT: f32 = 8.0;

/// A student who is selling something first turns the camera to herself (`SetupForTransaction`) and waits, in 0.3 second steps, until
/// the camera has stopped turning (two samples of its rotation equal) before she starts the trade. How exactly the turn comes to a
/// stop depends on arithmetic and on what else moves the camera, and when it never does the whole trade hangs with Harry unable to
/// move or cast. Past the limit she goes on to the trade, which needs nothing from the camera.
fn vendor_watchdog(vm: &mut Vm<'_>) {
    if world(vm).frame % 20 != 0 {
        return;
    }
    let now = world(vm).time;
    let target = vm.db.names.intern("DoingVendorTransaction");
    let waiting = vm.db.names.intern("SetupForTransaction");
    let actors = world(vm).actors.clone();
    let mut still: Vec<ObjId> = Vec::new();
    for a in actors {
        let Some(state) = vm.state_of(a) else { continue };
        if vm.db.get(state).name != waiting || vm.db.objects.get(a).deleted {
            continue;
        }
        still.push(a);
        let since = *world(vm).vendor_setup_since.entry(a).or_insert(now);
        let limit = crate::envc::var("HP3_VENDOR_SETUP_LIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(VENDOR_SETUP_LIMIT);
        if now - since > limit {
            vm.log.push(format!("trade: {} waited {:.0} s for the camera, going on", vm.db.names.get(vm.db.objects.get(a).name), now - since));
            let _ = vm.goto_state(a, target, hp3_script::Name::NONE);
            world(vm).vendor_setup_since.remove(&a);
        }
    }
    world(vm).vendor_setup_since.retain(|k, _| still.contains(k));
}

/// While profiling: adds the time since `start` to the actor's class under `phase`.
fn class_time(vm: &mut Vm<'_>, a: ObjId, phase: &'static str, start: Option<std::time::Instant>) {
    if let (Some(start), Some(class)) = (start, vm.db.objects.get(a).class) {
        let e = world(vm).tick_classes.entry((phase, class)).or_default();
        e.0 += 1;
        e.1 += start.elapsed();
    }
}

/// Whether an actor's `Tick` event can only update something that is never drawn here: a shadow projector keeps a
/// character's blob shadow under it with traces every tick, and projectors are not rendered, so its tick is skipped.
fn tick_is_invisible(vm: &mut Vm<'_>, a: ObjId) -> bool {
    let Some(class) = vm.db.objects.get(a).class else { return false };
    if let Some(&known) = world(vm).tick_skipped.get(&class) {
        return known;
    }
    let answer = vm.db.find("ShadowProjector").is_some_and(|p| vm.is_child_of(class, p));
    world(vm).tick_skipped.insert(class, answer);
    answer
}

/// Whether an actor is scenery that does nothing when ticked; answers are remembered per class.
fn is_scenery(vm: &mut Vm<'_>, a: ObjId) -> bool {
    let Some(class) = vm.db.objects.get(a).class else { return false };
    if let Some(&known) = world(vm).scenery_classes.get(&class) {
        return known;
    }
    const SCENERY: [&str; 9] = ["StaticMeshActor", "Brush", "Light", "PathNode", "CutKeyPointMark", "CutCamPosition", "CutMark", "AIMarker", "ZoneInfo"];
    let answer = SCENERY.iter().any(|n| vm.db.find(n).is_some_and(|k| vm.is_child_of(class, k)));
    world(vm).scenery_classes.insert(class, answer);
    answer
}

/// Whether something other than the player's own controller has the player's pawn (a cutscene), or it has none.
fn player_is_scripted(vm: &mut Vm<'_>) -> bool {
    let Some(pc) = world(vm).player else { return true };
    let Some(pawn) = get(vm, pc, "Pawn").as_obj() else { return true };
    get(vm, pawn, "Controller").as_obj().is_some_and(|c| c != pc)
}

/// Whether a class is one of the ambient pawns that are ticked less often when far from the player: the hub's students and
/// the spawners and books around them. Answers are remembered per class.
fn is_ambient_pawn(vm: &mut Vm<'_>, class: hp3_script::StructId) -> bool {
    if let Some(&known) = world(vm).ambient_classes.get(&class) {
        return known;
    }
    const AMBIENT: [&str; 6] = ["HPGenericStudent", "HPGenericStudentSpawner", "KnightSpawner", "SaveBook", "MonsterBook", "FlyingBook"];
    let answer = AMBIENT.iter().any(|n| vm.db.find(n).is_some_and(|k| vm.is_child_of(class, k)));
    world(vm).ambient_classes.insert(class, answer);
    answer
}

/// The pawn whose distance decides how often `a` is ticked: an ambient pawn itself, or the controller of one.
fn far_owner(vm: &mut Vm<'_>, a: ObjId, controller_class: Option<hp3_script::StructId>) -> Option<ObjId> {
    let class = vm.db.objects.get(a).class?;
    if is_ambient_pawn(vm, class) {
        return Some(a);
    }
    if is_controller(vm, controller_class, a) {
        let pawn = get(vm, a, "Pawn").as_obj()?;
        let pawn_class = vm.db.objects.get(pawn).class?;
        return is_ambient_pawn(vm, pawn_class).then_some(pawn);
    }
    None
}

/// Every how many frames `a` is ticked: 1 for most; an ambient NPC (or its controller) is ticked every 2nd frame past
/// `FAR_NEAR` from the player and every 4th past `FAR_FAR`.
fn far_period(vm: &mut Vm<'_>, a: ObjId, player_at: Option<glam::Vec3>, controller_class: Option<hp3_script::StructId>) -> u32 {
    const FAR_NEAR: f32 = 900.0;
    const FAR_FAR: f32 = 1800.0;
    let Some(from) = player_at else { return 1 };
    let Some(owner) = far_owner(vm, a, controller_class) else { return 1 };
    let d = (physics::vec(&get(vm, owner, "Location")) - from).length();
    if d > FAR_FAR {
        4
    } else if d > FAR_NEAR {
        2
    } else {
        1
    }
}

/// The id the frame count is offset by for `a`, so an NPC and its controller are ticked in the same frames.
fn far_key(vm: &mut Vm<'_>, a: ObjId) -> u32 {
    let cc = vm.db.find("Controller");
    far_owner(vm, a, cc).map_or(a.0 as u32, |o| o.0 as u32)
}

/// Whether an actor is a controller; answers are remembered per class.
fn is_controller(vm: &mut Vm<'_>, controller_class: Option<hp3_script::StructId>, a: ObjId) -> bool {
    let (Some(cc), Some(class)) = (controller_class, vm.db.objects.get(a).class) else { return false };
    if let Some(&known) = world(vm).controller_classes.get(&class) {
        return known;
    }
    let answer = vm.is_child_of(class, cc);
    world(vm).controller_classes.insert(class, answer);
    answer
}

/// Adds the time since the last lap to a tick-profile entry (when profiling).
fn lap(vm: &mut Vm<'_>, phase: &'static str, clock: &mut std::time::Instant, profile: bool) {
    if profile {
        *world(vm).tick_profile.entry(phase).or_default() += clock.elapsed();
        *clock = std::time::Instant::now();
    }
}

/// Counts an actor's timer down; fires `Timer` when it runs out, and again each period if it loops.
fn tick_timer(vm: &mut Vm<'_>, a: ObjId, dt: f32) {
    let due = match world(vm).timers.get_mut(&a) {
        Some(t) => {
            t.remaining -= dt;
            if t.remaining <= 0.0 {
                if t.looping {
                    t.remaining += t.rate.max(0.0001);
                } else {
                    world(vm).timers.remove(&a);
                }
                true
            } else {
                false
            }
        }
        None => false,
    };
    if due && !vm.db.objects.get(a).deleted {
        fire(vm, a, "Timer", &mut []);
    }
}

/// Keeps `World::player` on the controller that possesses the player's pawn: HP3 hands the pawn to another
/// controller after login, which then needs its input object.
fn follow_player(vm: &mut Vm<'_>) {
    let Some(pc) = world(vm).player else { return };
    let Some(pawn) = get(vm, pc, "Pawn").as_obj() else { return };
    let Some(owner) = get(vm, pawn, "Controller").as_obj() else { return };
    if owner == pc || vm.db.objects.get(owner).deleted {
        return;
    }
    let Some(pcc) = vm.db.find("PlayerController") else { return };
    let is_player = vm.db.objects.get(owner).class.is_some_and(|c| vm.is_child_of(c, pcc));
    if is_player {
        world(vm).player = Some(owner);
        if get(vm, owner, "PlayerInput").as_obj().is_none() {
            fire(vm, owner, "InitInputSystem", &mut []);
        }
    }
}

/// Decodes the native data of an export with `hp3-assets`.
pub(crate) fn native_asset(map: &Package, index: usize, chain: &[String]) -> Option<Asset> {
    let o = object::read_export(map, index, chain).ok()?;
    let base = map.exports[index].serial_offset + map.exports[index].serial_size - o.tail.len();
    let mut r = hp3_package::Reader::with_base(o.tail, base);
    hp3_assets::read_native(o.native_class, map, &mut r)?.ok()
}

/// Where a skeletal mesh's origin (its feet) is drawn relative to the actor's `Location`: `PrePivot` and
/// `SavedPrePivot`, and for actors that align their bottom (`bAlignBottomAlways`, or `bAlignBottom` with
/// `bCollideWorld`) a drop by the collision height, five more units while the actor has physics. Characters
/// that do not align (Harry, Ron, Hermione) get the same drop from their `PrePivot` of -42.
pub fn mesh_offset(vm: &mut Vm<'_>, a: ObjId) -> glam::Vec3 {
    let mut offset = rd_vec(vm, a, "PrePivot") + rd_vec(vm, a, "SavedPrePivot");
    let align = rd(vm, a, "bAlignBottomAlways").as_bool() || (rd(vm, a, "bAlignBottom").as_bool() && rd(vm, a, "bCollideWorld").as_bool());
    if align {
        let scale = match rd(vm, a, "DrawScale").as_float() {
            0.0 => 1.0,
            s => s,
        };
        // As the original: the mesh drops by the collision height and keeps the height of its origin (the vertical part
        // of the mesh origin is not taken off), so a gargoyle modelled with its origin at its middle stands on its shelf
        // and a knight mesh that carries pieces far below its feet is not lifted by them.
        let keep = get(vm, a, "Mesh").as_obj().map_or(0.0, |m| anim::origin_drop(vm, m)) * scale;
        let height = get(vm, a, "CollisionHeight").as_float();
        offset.z -= height - keep;
        if get(vm, a, "Physics").as_byte() != 0 {
            offset.z -= 5.0;
        }
    }
    offset
}

/// Destroys an actor (`Destroy`): its `Destroyed` event runs and it leaves the level. Static and no-delete actors
/// stay; returns whether the actor is gone.
pub fn destroy_actor(vm: &mut Vm<'_>, o: ObjId) -> bool {
    if vm.db.objects.get(o).deleted {
        return true;
    }
    if get(vm, o, "bStatic").as_bool() || get(vm, o, "bNoDelete").as_bool() {
        return false;
    }
    if envc::var_os("HP3_TRACE_DESTROY").is_some() {
        let name = hp3_script::natives::object_path(vm, o);
        let trace = std::backtrace::Backtrace::force_capture().to_string();
        let frames: Vec<&str> = trace.lines().filter(|l| l.contains("hp3_world") || l.contains("hp3_script")).take(14).collect();
        eprintln!("destroy {name}:\n{}", frames.join("\n"));
    }
    // An actor in a state hears `EndState` before `Destroyed`, as in the original's `DestroyActor`: a spell destroyed while it charges
    // fades out the effects its charge made.
    if vm.state_of(o).is_some() && envc::var_os("HP3_NO_DESTROY_ENDSTATE").is_none() {
        fire(vm, o, "EndState", &mut []);
        if vm.db.objects.get(o).deleted {
            return true;
        }
    }
    fire(vm, o, "Destroyed", &mut []);
    touch::forget(vm, o);
    vm.db.objects.get_mut(o).deleted = true;
    world(vm).collision.set_active(o, false);
    let w = world(vm);
    w.actors.retain(|&x| x != o);
    w.timers.remove(&o);
    vm.latent.remove(&o);
    true
}

fn vec_of(v: &Value) -> glam::Vec3 {
    physics::vec(v)
}

/// Collision geometry of a level: its BSP, the triangles of blocking static meshes, and terrain.
/// A mover's collision triangles in its own space (scaled, around its pivot): its static mesh's, or its
/// brush's polygons.
fn mover_shape(vm: &mut Vm<'_>, a: ObjId) -> Option<collision::MoverShape> {
    use glam::Vec3;
    if !get(vm, a, "bCollideActors").as_bool() {
        return None;
    }
    let scale = match get(vm, a, "DrawScale").as_float() {
        0.0 => 1.0,
        s => s,
    };
    let s3 = match get(vm, a, "DrawScale3D") {
        Value::Struct(f) if f.len() == 3 => vec_of(&Value::Struct(f)),
        _ => Vec3::ONE,
    };
    let s3 = if s3 == Vec3::ZERO { Vec3::ONE } else { s3 };
    let mut tris: Vec<[Vec3; 3]> = Vec::new();
    if let Some(mesh) = get(vm, a, "StaticMesh").as_obj() {
        let (pkg, index) = vm.db.objects.get(mesh).key.clone()?;
        let p = vm.db.set.get(&pkg).ok()?;
        let chain = vm.db.set.export_chain(&p, index);
        let Some(Asset::StaticMesh(sm)) = native_asset(&p, index, &chain) else { return None };
        let pos: Vec<Vec3> = sm.vertices.iter().map(|v| Vec3::new(v.position.x, v.position.y, v.position.z) * s3 * scale).collect();
        if std::env::var_os("HP3_COLLISION_STATS").is_some() {
            let model_planes = vm.db.set.resolve(&p, sm.collision_model).and_then(|(cp, ci)| {
                let cchain = vm.db.set.export_chain(&cp, ci);
                match native_asset(&cp, ci, &cchain)? {
                    Asset::Model(m) => Some(m.nodes.len()),
                    _ => None,
                }
            });
            let (boxes, lines) = mesh_simple_flags(&p, index, &chain);
            eprintln!(
                "movermesh {} tris={} model_nodes={:?} simple boxes={boxes} lines={lines}",
                hp3_script::natives::object_path(vm, mesh),
                sm.collision_triangles.len(),
                model_planes
            );
        }
        if std::env::var("HP3_MOVER_MESH").is_ok_and(|n| hp3_script::natives::object_path(vm, a).ends_with(&n)) {
            eprintln!("mover mesh {}: {} vertices, {} render triangles, {} collision triangles, scale {s3} {scale}", hp3_script::natives::object_path(vm, mesh), pos.len(), sm.indices.len() / 3, sm.collision_triangles.len());
            for t in sm.indices.chunks_exact(3) {
                let [a0, b0, c0] = [t[0], t[1], t[2]].map(|i| pos.get(i as usize).copied().unwrap_or(Vec3::ZERO));
                eprintln!("render tri ({:.0},{:.0},{:.0}) ({:.0},{:.0},{:.0}) ({:.0},{:.0},{:.0})", a0.x, a0.y, a0.z, b0.x, b0.y, b0.z, c0.x, c0.y, c0.z);
            }
        }
        if !sm.collision_triangles.is_empty() {
            for t in &sm.collision_triangles {
                tris.push(t.vertices.map(|i| pos.get(i as usize).copied().unwrap_or(Vec3::ZERO)));
            }
        } else {
            for t in sm.indices.chunks_exact(3) {
                tris.push([t[0], t[1], t[2]].map(|i| pos.get(i as usize).copied().unwrap_or(Vec3::ZERO)));
            }
        }
        // The Carpe block's pattern face has a groove along its base (8 high, 6 deep) that is only for the eye: a pawn
        // that climbs the face has to find it solid, so a flat strip closes it for collision.
        if vm.db.objects.get(mesh).key.is_some() && hp3_script::natives::object_path(vm, mesh).to_ascii_lowercase().ends_with(".cr_block_sm") {
            let p = |x: f32, y: f32, z: f32| Vec3::new(x, y, z) * s3 * scale;
            tris.push([p(0.0, -9.0, 8.0), p(0.0, -120.0, 8.0), p(0.0, -9.0, 0.0)]);
            tris.push([p(0.0, -120.0, 8.0), p(0.0, -120.0, 0.0), p(0.0, -9.0, 0.0)]);
        }
    } else if get(vm, a, "Brush").as_obj().is_some() {
        tris = brush_triangles(vm, a, s3, scale)?;
    }
    if tris.is_empty() {
        return None;
    }
    let mut shape = collision::MoverShape::new(a, &tris);
    // A mesh mover answers box traces with the mesh's simple collision model, as a static mesh does.
    if let Some(mesh) = get(vm, a, "StaticMesh").as_obj()
        && let Some((pkg, index)) = vm.db.objects.get(mesh).key.clone()
        && let Ok(p) = vm.db.set.get(&pkg)
    {
        let chain = vm.db.set.export_chain(&p, index);
        if let Some(Asset::StaticMesh(sm)) = native_asset(&p, index, &chain)
            && let Some((cp, ci)) = vm.db.set.resolve(&p, sm.collision_model)
        {
            let (boxes, lines) = mesh_simple_flags(&p, index, &chain);
            let cchain = vm.db.set.export_chain(&cp, ci);
            if let Some(Asset::Model(cm)) = native_asset(&cp, ci, &cchain)
                && let Some(placed) = place_model(*cm, glam::Vec3::ZERO, glam::Mat3::from_diagonal(s3 * scale))
            {
                shape.set_model(placed, boxes, lines);
            }
        }
    }
    shape.place(mover_transform(vm, a));
    Some(shape)
}

/// The triangles of an actor's brush (a mover's, or a volume's) in its own space: scaled, around its pivot.
fn brush_triangles(vm: &mut Vm<'_>, a: ObjId, s3: glam::Vec3, scale: f32) -> Option<Vec<[glam::Vec3; 3]>> {
    use glam::Vec3;
    let brush = get(vm, a, "Brush").as_obj()?;
    let (pkg, index) = vm.db.objects.get(brush).key.clone()?;
    let p = vm.db.set.get(&pkg).ok()?;
    let chain = vm.db.set.export_chain(&p, index);
    let Some(Asset::Model(model)) = native_asset(&p, index, &chain) else { return None };
    let (pp, pi) = vm.db.set.resolve(&p, model.polys)?;
    let chain = vm.db.set.export_chain(&pp, pi);
    let Some(Asset::Polys(polys)) = native_asset(&pp, pi, &chain) else { return None };
    let prepivot = vec_of(&get(vm, a, "PrePivot"));
    let mut tris = Vec::new();
    if std::env::var("HP3_BRUSH_POLYS").is_ok_and(|n| hp3_script::natives::object_path(vm, a).ends_with(&n)) {
        for (i, poly) in polys.polys.iter().enumerate() {
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for v in &poly.vertices {
                let w = Vec3::new(v.x, v.y, v.z);
                lo = lo.min(w);
                hi = hi.max(w);
            }
            eprintln!("brush poly {i}: {} verts normal ({:.2},{:.2},{:.2}) flags {:#x} lo {lo} hi {hi}", poly.vertices.len(), poly.normal.x, poly.normal.y, poly.normal.z, poly.poly_flags);
        }
    }
    for poly in &polys.polys {
        let v: Vec<Vec3> = poly.vertices.iter().map(|v| (Vec3::new(v.x, v.y, v.z) - prepivot) * s3 * scale).collect();
        for k in 1..v.len().saturating_sub(1) {
            tris.push([v[0], v[k], v[k + 1]]);
        }
    }
    Some(tris)
}

/// The box around a volume's brush in the world (None for anything that is not a volume with a brush, and for
/// volumes that move: a mover's shape is handled apart).
pub fn volume_bounds(vm: &mut Vm<'_>, a: ObjId) -> Option<(glam::Vec3, glam::Vec3)> {
    use glam::{Mat3, Vec3};
    let volume = vm.db.find("Volume")?;
    if !vm.db.objects.get(a).class.is_some_and(|c| vm.is_child_of(c, volume)) || !get(vm, a, "bStatic").as_bool() {
        return None;
    }
    let scale = match get(vm, a, "DrawScale").as_float() {
        0.0 => 1.0,
        s => s,
    };
    let s3 = match get(vm, a, "DrawScale3D") {
        Value::Struct(f) if f.len() == 3 => vec_of(&Value::Struct(f)),
        _ => Vec3::ONE,
    };
    let s3 = if s3 == Vec3::ZERO { Vec3::ONE } else { s3 };
    let tris = brush_triangles(vm, a, s3, scale)?;
    if tris.is_empty() {
        return None;
    }
    let loc = vec_of(&get(vm, a, "Location"));
    let r: Vec<i32> = get(vm, a, "Rotation").fields().iter().map(Value::as_int).collect();
    let axes = hp3_script::natives::rot_axes([r.first().copied().unwrap_or(0), r.get(1).copied().unwrap_or(0), r.get(2).copied().unwrap_or(0)]);
    let basis = Mat3::from_cols(Vec3::from(axes[0]), Vec3::from(axes[1]), Vec3::from(axes[2]));
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for t in tris {
        for p in t {
            let w = loc + basis * p;
            lo = lo.min(w);
            hi = hi.max(w);
        }
    }
    Some((lo, hi))
}

/// A physics volume's brush as planes (a point is inside when it is behind all of them).
pub struct VolumeHull {
    pub actor: ObjId,
    pub priority: i32,
    planes: Vec<(glam::Vec3, f32)>,
    lo: glam::Vec3,
    hi: glam::Vec3,
}

fn build_volume_hulls(vm: &mut Vm<'_>) -> Vec<VolumeHull> {
    use glam::{Mat3, Vec3};
    let Some(class) = vm.db.find("PhysicsVolume") else { return Vec::new() };
    let actors = world(vm).actors.clone();
    let mut out = Vec::new();
    for a in actors {
        if vm.db.objects.get(a).deleted || !vm.db.objects.get(a).class.is_some_and(|c| vm.is_child_of(c, class)) {
            continue;
        }
        let scale = match get(vm, a, "DrawScale").as_float() {
            0.0 => 1.0,
            s => s,
        };
        let s3 = match get(vm, a, "DrawScale3D") {
            Value::Struct(f) if f.len() == 3 => vec_of(&Value::Struct(f)),
            _ => Vec3::ONE,
        };
        let s3 = if s3 == Vec3::ZERO { Vec3::ONE } else { s3 };
        let Some(tris) = brush_triangles(vm, a, s3, scale) else { continue };
        if tris.is_empty() {
            continue;
        }
        let loc = vec_of(&get(vm, a, "Location"));
        let r: Vec<i32> = get(vm, a, "Rotation").fields().iter().map(Value::as_int).collect();
        let axes = hp3_script::natives::rot_axes([r.first().copied().unwrap_or(0), r.get(1).copied().unwrap_or(0), r.get(2).copied().unwrap_or(0)]);
        let basis = Mat3::from_cols(Vec3::from(axes[0]), Vec3::from(axes[1]), Vec3::from(axes[2]));
        let world_tris: Vec<[Vec3; 3]> = tris.iter().map(|t| t.map(|p| loc + basis * p)).collect();
        let (mut lo, mut hi, mut sum) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN), Vec3::ZERO);
        for t in &world_tris {
            for p in t {
                lo = lo.min(*p);
                hi = hi.max(*p);
                sum += *p;
            }
        }
        let centre = sum / (world_tris.len() * 3) as f32;
        let mut planes: Vec<(Vec3, f32)> = Vec::new();
        for t in &world_tris {
            let n = (t[1] - t[0]).cross(t[2] - t[0]);
            if n.length_squared() < 1e-6 {
                continue;
            }
            let mut n = n.normalize();
            let mut d = n.dot(t[0]);
            if n.dot(centre) - d > 0.0 {
                n = -n;
                d = -d;
            }
            if !planes.iter().any(|(pn, pd)| pn.dot(n) > 0.9999 && (pd - d).abs() < 0.5) {
                planes.push((n, d));
            }
        }
        out.push(VolumeHull { actor: a, priority: get(vm, a, "Priority").as_int(), planes, lo, hi });
    }
    out
}

/// How far past its brush a slide still holds the player who is on it (units). The ice a hero slides on is wider and longer than
/// the volume laid over it, and at its edges, corners and joins the volume ended under his feet: the slide ended on the spot
/// (back to walking, ground friction, a dead stop, then the restart). The slide ends where the surface does, not where the brush does.
const SLIDE_HOLD_MARGIN: f32 = 192.0;

/// Whether the pawn's controller is in the state called `name`.
fn controller_state_is(vm: &mut Vm<'_>, pawn: ObjId, name: &str) -> bool {
    let Some(c) = get(vm, pawn, "Controller").as_obj() else { return false };
    vm.state_of(c).is_some_and(|s| vm.db.names.get(vm.db.get(s).name).eq_ignore_ascii_case(name))
}

/// Whether `volume` is a sliding volume.
fn is_sliding_volume(vm: &mut Vm<'_>, volume: ObjId) -> bool {
    let Some(class) = vm.db.find("SlidingVolume") else { return false };
    vm.db.objects.get(volume).class.is_some_and(|c| vm.is_child_of(c, class))
}

/// Whether `p` is within `margin` of the brush of `volume`.
fn volume_holds_within(vm: &mut Vm<'_>, volume: ObjId, p: glam::Vec3, margin: f32) -> bool {
    if world(vm).volume_hulls.is_none() {
        let hulls = build_volume_hulls(vm);
        world(vm).volume_hulls = Some(hulls);
    }
    let Some(hulls) = &world(vm).volume_hulls else { return false };
    hulls.iter().filter(|h| h.actor == volume).any(|h| {
        let m = glam::Vec3::splat(margin);
        !(p.cmplt(h.lo - m).any() || p.cmpgt(h.hi + m).any()) && h.planes.iter().all(|(n, d)| n.dot(p) - d <= margin + 0.5)
    })
}

/// Which physics volume a pawn is in after it has moved: the highest priority one whose brush holds its location, else
/// the level's default. A change is told to the volumes, the pawn and its controller (`NotifyPhysicsVolumeChange`),
/// which is how a sliding volume puts the player into its sliding state.
pub fn update_physics_volume(vm: &mut Vm<'_>, a: ObjId) {
    // The volumes do not move: a pawn standing where it was last looked at, in the volume it was then left in, is in the right one.
    let at = vec_of(&get(vm, a, "Location"));
    let current = get(vm, a, "PhysicsVolume").as_obj();
    if world(vm).volume_checked.get(&a) == Some(&(at.to_array().map(f32::to_bits), current)) {
        return;
    }
    update_physics_volume_now(vm, a);
    let after = get(vm, a, "PhysicsVolume").as_obj();
    world(vm).volume_checked.insert(a, (at.to_array().map(f32::to_bits), after));
}

fn update_physics_volume_now(vm: &mut Vm<'_>, a: ObjId) {
    if world(vm).volume_hulls.is_none() {
        let hulls = build_volume_hulls(vm);
        world(vm).volume_hulls = Some(hulls);
    }
    let here = vec_of(&get(vm, a, "Location"));
    let best = volume_holding(vm, here);
    let current = get(vm, a, "PhysicsVolume").as_obj();
    let mut new = match best {
        Some((_, v)) => Some(v),
        None => default_physics_volume(vm).or(current),
    };
    // The player on a slide keeps it a good way past its brush (see SLIDE_HOLD_MARGIN), so its edges never end the slide.
    if physics::is_player_pawn(vm, a)
        && let (Some(cur), Some(n)) = (current, new)
        && n != cur
        && !is_sliding_volume(vm, n)
        && is_sliding_volume(vm, cur)
        && controller_state_is(vm, a, "PlayerSliding")
        && volume_holds_within(vm, cur, here, SLIDE_HOLD_MARGIN)
    {
        new = current;
    }
    // A player in a sliding volume whose controller is walking (a restart put it back on the ice, say) slides: the controller
    // only hears of a volume when it changes.
    if new == current
        && let Some(cur) = current
        && physics::is_player_pawn(vm, a)
        && is_sliding_volume(vm, cur)
        && controller_state_is(vm, a, "PlayerWalking")
        && let Some(c) = get(vm, a, "Controller").as_obj()
    {
        fire(vm, c, "NotifyPhysicsVolumeChange", &mut [Value::Object(Some(cur))]);
    }
    if new == current || new.is_none() {
        return;
    }
    let new = new.unwrap();
    if let Some(old) = current.filter(|&o| !vm.db.objects.get(o).deleted) {
        fire(vm, old, "PawnLeavingVolume", &mut [Value::Object(Some(a))]);
    }
    set(vm, a, "PhysicsVolume", Value::Object(Some(new)));
    if physics::is_player_pawn(vm, a) {
        let name = hp3_script::natives::object_path(vm, new);
        vm.log.push(format!("physics volume: the player entered {name}"));
    }
    fire(vm, new, "PawnEnteredVolume", &mut [Value::Object(Some(a))]);
    fire(vm, a, "PhysicsVolumeChange", &mut [Value::Object(Some(new))]);
    if let Some(c) = get(vm, a, "Controller").as_obj() {
        fire(vm, c, "NotifyPhysicsVolumeChange", &mut [Value::Object(Some(new))]);
    }
}

/// The level's default physics volume (what an actor is in when no brush holds it).
fn default_physics_volume(vm: &mut Vm<'_>) -> Option<ObjId> {
    if let Some(v) = world(vm).default_volume
        && !vm.db.objects.get(v).deleted
    {
        return Some(v);
    }
    let found = find_default_physics_volume(vm);
    world(vm).default_volume = found;
    found
}

fn find_default_physics_volume(vm: &mut Vm<'_>) -> Option<ObjId> {
    let class = vm.db.find("DefaultPhysicsVolume")?;
    let actors = world(vm).actors.clone();
    let found = actors.into_iter().find(|&a| !vm.db.objects.get(a).deleted && vm.db.objects.get(a).class.is_some_and(|c| vm.is_child_of(c, class)));
    if found.is_some() {
        return found;
    }
    // A map can carry its default volume as a deleted actor (InsideHub): the engine makes one itself, with the class's gravity.
    // Without it everything in the level has no gravity for the scripts, and a jump (sqrt(-2 * height * gravity)) has no speed.
    let class_obj = vm.db.get(class).object;
    natives::spawn(vm, None, &[Value::Object(Some(class_obj))]).ok().and_then(|v| v.as_obj())
}

/// A mover's placement: its location and rotation.
pub(crate) fn mover_transform(vm: &mut Vm<'_>, a: ObjId) -> glam::Mat4 {
    let loc = vec_of(&get(vm, a, "Location"));
    let r: Vec<i32> = get(vm, a, "Rotation").fields().iter().map(Value::as_int).collect();
    let rot = hp3_anim::rotator_matrix([r.first().copied().unwrap_or(0), r.get(1).copied().unwrap_or(0), r.get(2).copied().unwrap_or(0)]);
    glam::Mat4::from_translation(loc) * rot
}

/// Half the height of a static mesh actor's bounding box in the world (what it draws, placed, rotated and
/// scaled); None when the actor is not a static mesh actor.
pub(crate) fn static_mesh_half_height(vm: &mut Vm<'_>, a: ObjId) -> Option<f32> {
    use glam::{Mat3, Vec3};
    if !vm.db.find("StaticMeshActor").is_some_and(|c| vm.db.objects.get(a).class.is_some_and(|k| vm.is_child_of(k, c))) {
        return None;
    }
    let mesh = get(vm, a, "StaticMesh").as_obj()?;
    let (pkg, index) = vm.db.objects.get(mesh).key.clone()?;
    let p = vm.db.set.get(&pkg).ok()?;
    let chain = vm.db.set.export_chain(&p, index);
    let Some(Asset::StaticMesh(sm)) = native_asset(&p, index, &chain) else { return None };
    let r: Vec<i32> = get(vm, a, "Rotation").fields().iter().map(Value::as_int).collect();
    let axes = hp3_script::natives::rot_axes([r.first().copied().unwrap_or(0), r.get(1).copied().unwrap_or(0), r.get(2).copied().unwrap_or(0)]);
    let basis = Mat3::from_cols(Vec3::from(axes[0]), Vec3::from(axes[1]), Vec3::from(axes[2]));
    let scale = match get(vm, a, "DrawScale").as_float() {
        0.0 => 1.0,
        s => s,
    };
    let s3 = match get(vm, a, "DrawScale3D") {
        Value::Struct(f) if f.len() == 3 => vec_of(&Value::Struct(f)),
        _ => Vec3::ONE,
    };
    let b = sm.mesh_bounds;
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for corner in 0..8u32 {
        let pick = |bit: u32, min: f32, max: f32| if corner & bit == 0 { min } else { max };
        let local = Vec3::new(pick(1, b.min.x, b.max.x), pick(2, b.min.y, b.max.y), pick(4, b.min.z, b.max.z));
        let z = (basis * (local * s3 * scale)).z;
        lo = lo.min(z);
        hi = hi.max(z);
    }
    Some((hi - lo) * 0.5)
}

/// A static mesh's `UseSimpleBoxCollision` and `UseSimpleLineCollision`: its collision model answers box traces
/// and line traces. A mesh that does not set them has the box one on and the line one off.
fn mesh_simple_flags(pkg: &Package, index: usize, chain: &[String]) -> (bool, bool) {
    let (mut boxes, mut lines) = (true, false);
    if let Ok(o) = object::read_export(pkg, index, chain) {
        for t in o.tags.iter().filter(|t| t.ty == hp3_package::props::TagType::Bool) {
            let name = pkg.name(t.name);
            if name.eq_ignore_ascii_case("UseSimpleBoxCollision") {
                boxes = t.bool_value;
            } else if name.eq_ignore_ascii_case("UseSimpleLineCollision") {
                lines = t.bool_value;
            }
        }
    }
    (boxes, lines)
}

/// A collision model moved into the world: every node plane is carried through the placement (`linear` is the
/// rotation and scale, `loc` the location). None when the scale collapses it.
fn place_model(mut model: hp3_assets::model::Model, loc: glam::Vec3, linear: glam::Mat3) -> Option<hp3_assets::model::Model> {
    use glam::Vec3;
    if linear.determinant().abs() < 1e-9 {
        return None;
    }
    let carry = linear.inverse().transpose();
    for node in &mut model.nodes {
        let old = Vec3::new(node.plane.normal.x, node.plane.normal.y, node.plane.normal.z);
        let moved = carry * old;
        let len = moved.length();
        if len < 1e-9 {
            continue;
        }
        let n = moved / len;
        node.plane.normal.x = n.x;
        node.plane.normal.y = n.y;
        node.plane.normal.z = n.z;
        node.plane.w = (node.plane.w + moved.dot(loc)) / len;
    }
    Some(model)
}

fn build_collision(vm: &mut Vm<'_>, map: &Rc<Package>, model: Option<ObjRef>, actors: &[ObjId]) -> collision::Collision {
    use glam::{Mat3, Vec3};
    let model = model.and_then(|m| vm.db.set.resolve(map, m)).and_then(|(p, i)| {
        let chain = vm.db.set.export_chain(&p, i);
        match native_asset(&p, i, &chain)? {
            Asset::Model(m) => Some(Rc::new(*m)),
            _ => None,
        }
    });
    let mut col = collision::Collision::new(model);
    let mover_class = vm.db.find("Mover");
    let volume_class = vm.db.find("Volume");
    for &a in actors {
        // Movers collide as moving shapes, not as part of the fixed level.
        if mover_class.is_some_and(|m| vm.db.objects.get(a).class.is_some_and(|c| vm.is_child_of(c, m))) {
            let shape = mover_shape(vm, a).filter(|_| std::env::var_os("HP3_NO_MOVER_COLLISION").is_none());
            if std::env::var_os("HP3_COLLISION_STATS").is_some() {
                eprintln!("mover {} collision shape: {}", hp3_script::natives::object_path(vm, a), if shape.is_some() { "built" } else { "NONE" });
            }
            if let Some(shape) = shape {
                col.add_mover(shape);
            }
            continue;
        }
        let mesh = get(vm, a, "StaticMesh").as_obj();
        let blocks = get(vm, a, "bCollideActors").as_bool()
            && (get(vm, a, "bBlockActors").as_bool() || get(vm, a, "bBlockPlayers").as_bool());
        // A volume that blocks (a BlockingVolume, mountable or not) is its brush: invisible walls and ledges.
        if blocks && mesh.is_none() && volume_class.is_some_and(|v| vm.db.objects.get(a).class.is_some_and(|c| vm.is_child_of(c, v))) {
            let loc = vec_of(&get(vm, a, "Location"));
            let r: Vec<i32> = get(vm, a, "Rotation").fields().iter().map(Value::as_int).collect();
            let axes = hp3_script::natives::rot_axes([r.first().copied().unwrap_or(0), r.get(1).copied().unwrap_or(0), r.get(2).copied().unwrap_or(0)]);
            let basis = Mat3::from_cols(Vec3::from(axes[0]), Vec3::from(axes[1]), Vec3::from(axes[2]));
            let scale = match get(vm, a, "DrawScale").as_float() {
                0.0 => 1.0,
                s => s,
            };
            let s3 = match get(vm, a, "DrawScale3D") {
                Value::Struct(f) if f.len() == 3 => vec_of(&Value::Struct(f)),
                _ => Vec3::ONE,
            };
            let s3 = if s3 == Vec3::ZERO { Vec3::ONE } else { s3 };
            if let Some(tris) = brush_triangles(vm, a, s3, scale) {
                if std::env::var_os("HP3_COLLISION_STATS").is_some() {
                    eprintln!("volume {} collision: {} triangles", hp3_script::natives::object_path(vm, a), tris.len());
                }
                for t in tris {
                    col.add_triangle(t.map(|p| loc + basis * p), Some(a));
                }
            }
            continue;
        }
        // A static mesh with `bUseCylinderCollision` collides as its collision cylinder (see `refresh_actor_boxes`), not as its triangles.
        let cylinder = get(vm, a, "bUseCylinderCollision").as_bool();
        if let Some(mesh) = mesh.filter(|_| blocks && !cylinder) {
            let Some((mkey_pkg, mkey_idx)) = vm.db.objects.get(mesh).key.clone() else { continue };
            let Ok(mp) = vm.db.set.get(&mkey_pkg) else { continue };
            let chain = vm.db.set.export_chain(&mp, mkey_idx);
            let Some(Asset::StaticMesh(sm)) = native_asset(&mp, mkey_idx, &chain) else { continue };
            let loc = vec_of(&get(vm, a, "Location"));
            let rot = get(vm, a, "Rotation");
            let r: Vec<i32> = rot.fields().iter().map(Value::as_int).collect();
            let axes = hp3_script::natives::rot_axes([r.first().copied().unwrap_or(0), r.get(1).copied().unwrap_or(0), r.get(2).copied().unwrap_or(0)]);
            let basis = Mat3::from_cols(Vec3::from(axes[0]), Vec3::from(axes[1]), Vec3::from(axes[2]));
            let scale = get(vm, a, "DrawScale").as_float();
            let scale = if scale == 0.0 { 1.0 } else { scale };
            let s3 = match get(vm, a, "DrawScale3D") {
                Value::Struct(f) if f.len() == 3 => vec_of(&Value::Struct(f)),
                _ => Vec3::ONE,
            };
            let to_world = |v: &hp3_assets::Vec3| loc + basis * (Vec3::new(v.x, v.y, v.z) * s3 * scale);
            let pos: Vec<Vec3> = sm.vertices.iter().map(|v| to_world(&v.position)).collect();
            if let Ok(want) = std::env::var("HP3_COLLISION_DEBUG")
                && vm.db.names.get(vm.db.objects.get(a).name).eq_ignore_ascii_case(&want)
            {
                let tri_bounds = |tris: &mut dyn Iterator<Item = [u32; 3]>| {
                    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
                    for t in tris {
                        for i in t {
                            let p = pos.get(i as usize).copied().unwrap_or(Vec3::ZERO);
                            lo = lo.min(p);
                            hi = hi.max(p);
                        }
                    }
                    (lo, hi)
                };
                let col = tri_bounds(&mut sm.collision_triangles.iter().map(|t| t.vertices.map(|i| i as u32)));
                let draw = tri_bounds(&mut sm.indices.chunks_exact(3).map(|t| [t[0] as u32, t[1] as u32, t[2] as u32]));
                eprintln!(
                    "collision debug {want}: {} collision tris bounds {:?}; {} render tris bounds {:?}",
                    sm.collision_triangles.len(),
                    col,
                    sm.indices.len() / 3,
                    draw
                );
            }
            if std::env::var_os("HP3_COLLISION_STATS").is_some() {
                let prims = vm.db.set.resolve(&mp, sm.karma_props).and_then(|(kp, ki)| {
                    let kchain = vm.db.set.export_chain(&kp, ki);
                    match native_asset(&kp, ki, &kchain)? {
                        Asset::MeshProps(m) => Some(format!("boxes {} spheres {} cylinders {} convexes {}", m.boxes.len(), m.spheres.len(), m.cylinders.len(), m.convexes.len())),
                        _ => None,
                    }
                });
                // Planes of the simple collision model that are neither upright nor flat: ramps and slanted faces.
                let slanted = vm.db.set.resolve(&mp, sm.collision_model).and_then(|(cp, ci)| {
                    let cchain = vm.db.set.export_chain(&cp, ci);
                    match native_asset(&cp, ci, &cchain)? {
                        Asset::Model(m) => Some((m.nodes.len(), m.nodes.iter().filter(|n| n.plane.normal.z.abs() > 0.05 && n.plane.normal.z.abs() < 0.95).count())),
                        _ => None,
                    }
                });
                eprintln!(
                    "meshstats {} tris={} model={} planes/slanted={:?} karma={}",
                    vm.db.names.get(vm.db.objects.get(mesh).name),
                    sm.collision_triangles.len(),
                    !sm.collision_model.is_none(),
                    slanted,
                    prims.unwrap_or_else(|| "none".into())
                );
            }
            // A simple collision model stands in for the triangles in the traces its flags allow.
            let (boxes, lines) = mesh_simple_flags(&mp, mkey_idx, &chain);
            if (boxes || lines) && std::env::var_os("HP3_NO_MESH_MODELS").is_none()
                && let Some((cp, ci)) = vm.db.set.resolve(&mp, sm.collision_model)
            {
                let cchain = vm.db.set.export_chain(&cp, ci);
                if let Some(Asset::Model(cm)) = native_asset(&cp, ci, &cchain) {
                    let linear = basis * Mat3::from_diagonal(s3 * scale);
                    if let Some(placed) = place_model(*cm, loc, linear) {
                        let (lo, hi) = pos.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
                        col.add_mesh_model(a, placed, boxes, lines, lo - Vec3::splat(8.0), hi + Vec3::splat(8.0));
                        if std::env::var_os("HP3_COLLISION_STATS").is_some() {
                            eprintln!("meshmodel {} boxes={boxes} lines={lines}", vm.db.names.get(vm.db.objects.get(a).name));
                        }
                    }
                }
            }
            if !sm.collision_triangles.is_empty() {
                for t in &sm.collision_triangles {
                    let [a0, b0, c0] = t.vertices.map(|i| pos.get(i as usize).copied().unwrap_or(Vec3::ZERO));
                    col.add_triangle([a0, b0, c0], Some(a));
                }
            } else {
                for t in sm.indices.chunks_exact(3) {
                    let [a0, b0, c0] = [t[0], t[1], t[2]].map(|i| pos.get(i as usize).copied().unwrap_or(Vec3::ZERO));
                    col.add_triangle([a0, b0, c0], Some(a));
                }
            }
        }
    }
    // Terrain: two triangles per heightmap quad, in world space.
    for i in 0..map.exports.len() {
        if !map.export_class_name(i).eq_ignore_ascii_case("TerrainInfo") {
            continue;
        }
        let chain = vm.db.set.export_chain(map, i);
        // A terrain the map carries but the level does not place (a leftover or deleted one) has no collision: GroundsHub holds a
        // small one over a pit that would leave a player floating above the Spongify pad inside it.
        if !actors.contains(&vm.db.obj_for(map, i)) {
            continue;
        }
        let Some(Asset::Terrain(t)) = native_asset(map, i, &chain) else { continue };
        let (w, h) = (t.heightmap_x as usize, t.heightmap_y as usize);
        let v = |x: usize, y: usize| t.vertices.get(y * w + x).map(|p| Vec3::new(p.x, p.y, p.z));
        // Holes (a tunnel's mouth cut into a hill) collide as little as they are drawn: one bit per heightmap
        // cell, set where the quad is there (an empty bitmap keeps every quad).
        let terrain_actor = vm.db.obj_for(map, i);
        let visible: Vec<u32> = match get(vm, terrain_actor, "QuadVisibilityBitmap") {
            Value::Array(bits) => bits.iter().map(|b| b.as_int() as u32).collect(),
            _ => Vec::new(),
        };
        let turned: Vec<u32> = match get(vm, terrain_actor, "EdgeTurnBitmap") {
            Value::Array(bits) if crate::envc::var_os("HP3_NO_EDGE_TURN").is_none() => bits.iter().map(|b| b.as_int() as u32).collect(),
            _ => Vec::new(),
        };
        if crate::envc::var_os("HP3_TERRAIN_DEBUG").is_some() {
            eprintln!("terrain collision {i}: {} quads with a turned edge", turned.iter().map(|b| b.count_ones()).sum::<u32>());
            if let Ok(spec) = std::env::var("HP3_TERRAIN_AT") {
                let p: Vec<f32> = spec.split(',').filter_map(|s| s.parse().ok()).collect();
                if p.len() == 2 {
                    let best = (0..h)
                        .flat_map(|y| (0..w).map(move |x| (x, y)))
                        .filter_map(|(x, y)| v(x, y).map(|q| (q, (q.x - p[0]).powi(2) + (q.y - p[1]).powi(2))))
                        .min_by(|a, b| a.1.total_cmp(&b.1));
                    eprintln!("terrain {i} nearest vertex to {spec}: {best:?}");
                }
            }
            let hidden: u32 = visible.iter().map(|b| b.count_zeros()).sum();
            eprintln!("terrain collision {i}: {w}x{h}, {} visibility words, {hidden} quads hidden", visible.len());
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for y in 0..h.saturating_sub(1) {
                for x in 0..w.saturating_sub(1) {
                    let q = y * w + x;
                    if !visible.is_empty() && visible.get(q / 32).is_some_and(|b| b & (1 << (q % 32)) == 0) && let Some(p) = v(x, y) {
                        lo = lo.min(p);
                        hi = hi.max(p);
                    }
                }
            }
            eprintln!("  hidden quads span {lo} .. {hi}");
        }
        let quad_there = |x: usize, y: usize| {
            let q = y * w + x;
            visible.is_empty() || visible.get(q / 32).is_some_and(|b| b & (1 << (q % 32)) != 0)
        };
        for y in 0..h.saturating_sub(1) {
            for x in 0..w.saturating_sub(1) {
                if !quad_there(x, y) {
                    continue;
                }
                if let (Some(a0), Some(b0), Some(c0), Some(d0)) = (v(x, y), v(x + 1, y), v(x, y + 1), v(x + 1, y + 1)) {
                    // A quad's diagonal runs from (x, y) to (x + 1, y + 1), or, when the level turned its edge, from (x + 1, y)
                    // to (x, y + 1): a hill's ridge is where the terrain is drawn and walked on only with the right one.
                    let q = y * w + x;
                    if turned.get(q / 32).is_some_and(|b| b & (1 << (q % 32)) != 0) {
                        col.add_triangle([a0, b0, c0], None);
                        col.add_triangle([c0, d0, b0], None);
                    } else {
                        col.add_triangle([a0, b0, d0], None);
                        col.add_triangle([a0, d0, c0], None);
                    }
                }
            }
        }
    }
    col
}

/// Finds an object by its full path (`Package.Group.Name`), loading classes as needed.
pub fn find_object(vm: &mut Vm<'_>, path: &str) -> Option<ObjId> {
    find_object_of(vm, path, None)
}

/// Like `find_object`, preferring an object that is a `class` (by class name, through its hierarchy) when
/// several share the path, as a mesh and its animation set often do.
pub fn find_object_of(vm: &mut Vm<'_>, path: &str, class: Option<&str>) -> Option<ObjId> {
    let (pkg_name, rest) = path.split_once('.')?;
    let pkg = vm.db.set.get(pkg_name).ok()?;
    let set = vm.db.set;
    let index = class
        .and_then(|c| pkg.find_export_path_where(rest, |i| set.export_chain(&pkg, i).iter().any(|n| n.eq_ignore_ascii_case(c))))
        .or_else(|| pkg.find_export_path(rest))
        // A name without its group (`soundeffects.Thunder03` for `soundeffects.Quidditch.thunder03`, as a cutscene's PlaySound
        // gives it): the object of that class with that name in whichever group of the package holds it.
        .or_else(|| {
            let c = class?;
            // Only sounds are named that way; other classes keep the exact path, where a missing object means "none".
            if rest.contains('.') || !c.eq_ignore_ascii_case("Sound") {
                return None;
            }
            (0..pkg.exports.len()).find(|&i| {
                pkg.ref_name(hp3_package::package::ObjRef::from_export(i)).eq_ignore_ascii_case(rest)
                    && set.export_chain(&pkg, i).iter().any(|n| n.eq_ignore_ascii_case(c))
            })
        })?;
    if pkg.exports[index].class.is_none() {
        let s = vm.db.load_struct(&pkg, index)?;
        return Some(vm.db.get(s).object);
    }
    Some(vm.db.obj_for(&pkg, index))
}

/// The object an export reference of the loaded map points to.
pub fn map_object(vm: &mut Vm<'_>, r: ObjRef) -> Option<ObjId> {
    let map = world(vm).map.clone();
    vm.db.resolve_obj(&map, r)
}

/// Environment variables read once: the debug switches are asked for in code that runs for every actor every tick, and a read
/// of the process environment is a scan (a few hundred nanoseconds on a phone).
pub(crate) mod envc {
    use std::cell::RefCell;
    use std::ffi::OsString;

    thread_local! {
        static CACHE: RefCell<hp3_script::fx::FxHashMap<String, Option<OsString>>> = RefCell::new(Default::default());
    }

    pub fn var_os(name: &str) -> Option<OsString> {
        CACHE.with(|c| {
            if let Some(v) = c.borrow().get(name) {
                return v.clone();
            }
            let v = std::env::var_os(name);
            c.borrow_mut().insert(name.to_string(), v.clone());
            v
        })
    }

    pub fn var(name: &str) -> Result<String, std::env::VarError> {
        match var_os(name) {
            Some(v) => v.into_string().map_err(std::env::VarError::NotUnicode),
            None => Err(std::env::VarError::NotPresent),
        }
    }
}

/// The highest priority physics volume whose brush holds the point (none when only the level's default does).
fn volume_holding(vm: &mut Vm<'_>, p: glam::Vec3) -> Option<(i32, ObjId)> {
    if world(vm).volume_hulls.is_none() {
        let hulls = build_volume_hulls(vm);
        world(vm).volume_hulls = Some(hulls);
    }
    let mut best: Option<(i32, ObjId)> = None;
    if let Some(hulls) = &world(vm).volume_hulls {
        for h in hulls {
            if p.cmplt(h.lo).any() || p.cmpgt(h.hi).any() || best.is_some_and(|(pr, _)| pr > h.priority) {
                continue;
            }
            if h.planes.iter().all(|(n, d)| n.dot(p) - d <= 0.5) {
                best = Some((h.priority, h.actor));
            }
        }
    }
    best
}

/// Keeps a pawn's controller in the volume its own place is in (the hero's script reads the controller's volume for the
/// slide's gravity and jumps; the original updates every actor's volume when it moves, silently for a controller).
pub fn update_controller_volume(vm: &mut Vm<'_>, c: ObjId) {
    let mut p = vec_of(&get(vm, c, "Location"));
    // A controller whose place was never set is where its pawn is.
    if p == glam::Vec3::ZERO
        && let Some(pawn) = get(vm, c, "Pawn").as_obj()
    {
        p = vec_of(&get(vm, pawn, "Location"));
    }
    // The volumes do not move: a controller still where it was last looked at, in the volume it was then put in, is done.
    let key = p.to_array().map(f32::to_bits);
    let current = get(vm, c, "PhysicsVolume").as_obj();
    if world(vm).volume_checked.get(&c) == Some(&(key, current)) {
        return;
    }
    let new = match volume_holding(vm, p) {
        Some((_, v)) => Some(v),
        None => default_physics_volume(vm),
    };
    if let Some(v) = new
        && current != Some(v)
    {
        set(vm, c, "PhysicsVolume", Value::Object(Some(v)));
    }
    let after = get(vm, c, "PhysicsVolume").as_obj();
    world(vm).volume_checked.insert(c, (key, after));
}

/// The physics volume (the highest priority one) whose brush holds the point, when one does: the level's default volume is not
/// returned.
pub fn physics_volume_holding(vm: &mut Vm<'_>, p: glam::Vec3) -> Option<ObjId> {
    volume_holding(vm, p).map(|(_, v)| v)
}
#[cfg(windows)]
/// HP3_MEM=1: prints the process's private memory (Windows) at the marked points of loading.
pub fn memprobe(tag: &str) {
    if std::env::var_os("HP3_MEM").is_none() {
        return;
    }
    #[repr(C)]
    struct Pmc { cb: u32, pf: u32, a: usize, b: usize, c: usize, d: usize, e: usize, f: usize, g: usize, h: usize, private: usize }
    unsafe extern "system" { fn K32GetProcessMemoryInfo(h: isize, c: *mut Pmc, cb: u32) -> i32; fn GetCurrentProcess() -> isize; }
    let mut p = Pmc { cb: std::mem::size_of::<Pmc>() as u32, pf: 0, a: 0, b: 0, c: 0, d: 0, e: 0, f: 0, g: 0, h: 0, private: 0 };
    unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut p, p.cb) };
    eprintln!("MEM {tag}: private {} MB", p.private >> 20);
}
#[cfg(not(windows))]
pub fn memprobe(_tag: &str) {}

/// The story state a level picked from a menu should start in: the lowest `GState` the map's actors are grouped under apart
/// from the starting one (a level that is jumped to starts in its first story state, not in `GState000`, which the next level's
/// start spots do not expect and where the way out of the level ends in a staging room). `None` when the map has no such groups.
pub fn menu_start_state(vm: &mut Vm<'_>, map: &str) -> Option<String> {
    // Levels whose story state the actors' groups do not give: the Library is entered from the hub after the Boggart lesson, in
    // `GState070`, and the hub's start cutscene for that state is the one that stands the party outside its door.
    if url_map(map).eq_ignore_ascii_case("HP3_Adv3Library") {
        return Some("GState070".to_string());
    }
    let pkg = vm.db.set.get(&url_map(map)).ok()?;
    let mut best: Option<u32> = None;
    for n in &pkg.names {
        let text = n.name.to_ascii_lowercase();
        for part in text.split(',') {
            if let Some(num) = part.trim().strip_prefix("gstate").and_then(|d| d.parse::<u32>().ok())
                && num > 0
                && best.is_none_or(|b| num < b)
            {
                best = Some(num);
            }
        }
    }
    best.map(|n| format!("GState{n:03}"))
}
