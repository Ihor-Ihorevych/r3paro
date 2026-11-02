//! The running level: actors loaded from a map into the script VM, the begin-play sequence, and the tick that
//! drives events, timers and state code.
//!
//! The world lives in `Vm::ext`; natives reach it with `world(vm)`.

pub mod anim;
pub mod audio;
pub mod debug_move;
pub mod canvas;
pub mod collision;
pub mod config;
pub mod fonts;
pub mod nav;
pub mod gui;
pub mod input;
pub mod movie;
pub mod interactions;
pub mod natives;
pub mod particles;
pub mod physics;
pub mod save;
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
    /// Sounds and music the game asked for, for the platform to play.
    pub audio: audio::Audio,
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
    /// Walking pawns' view offsets while they ease onto a step (drawing only).
    pub step_smooth: std::collections::HashMap<ObjId, f32>,
    /// The physics volumes of the level with their brush planes (built the first time a pawn is checked).
    pub volume_hulls: Option<Vec<VolumeHull>>,
    /// What travelled in with the player (the level keeps it for a restart).
    pub travel_info: TravelInfo,
    actor_slots: HashMap<&'static str, Option<usize>>,
    /// Time per tick phase, when `HP3_TICK_PROFILE` is set.
    pub tick_profile: std::collections::BTreeMap<&'static str, std::time::Duration>,
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

/// The player's pawn and the inventory chain behind it, for the next level.
fn export_player(vm: &mut Vm<'_>, pc: ObjId) -> Option<(String, Vec<TravelActor>)> {
    let pawn = get(vm, pc, "Pawn").as_obj()?;
    let mut actors = Vec::new();
    let mut at = Some(pawn);
    for _ in 0..256 {
        let Some(a) = at else { break };
        actors.extend(export_travel(vm, a));
        at = get(vm, a, "Inventory").as_obj();
    }
    Some((player_name(vm, pc), actors))
}

/// The name the player's replication info holds (empty when it has none).
fn player_name(vm: &mut Vm<'_>, pc: ObjId) -> String {
    match get(vm, pc, "PlayerReplicationInfo").as_obj() {
        Some(pri) => get(vm, pri, "PlayerName").as_str().to_string(),
        None => String::new(),
    }
}

/// Brings the travelling actors of `info` into the new level for the player's pawn: the pawn entry is the
/// pawn itself, others are spawned at the pawn (owned by it); then their `travel` properties are put back, with
/// actor references pointing at the new copies. Returns the actors that came, in the order they were listed.
fn accept_travel(vm: &mut Vm<'_>, pc: ObjId, pawn: ObjId, info: &TravelInfo) -> Vec<ObjId> {
    // The entry under the player's name; failing that (a game with one player) the first one.
    let name = player_name(vm, pc);
    let Some((_, actors)) = info.iter().find(|(n, _)| *n == name).or_else(|| info.first()) else { return Vec::new() };
    let pawn_class = vm.db.find("Pawn");
    let mut accepted: Vec<(String, ObjId, &TravelActor)> = Vec::new();
    for t in actors {
        let is_pawn = pawn_class.is_some_and(|p| vm.is_child_of(t.class, p));
        let actor = if is_pawn {
            Some(pawn)
        } else {
            let class_obj = vm.db.get(t.class).object;
            natives::spawn(vm, Some(pawn), &[Value::Object(Some(class_obj)), Value::Object(Some(pawn))]).ok().and_then(|v| v.as_obj())
        };
        if let Some(a) = actor {
            accepted.push((t.name.clone(), a, t));
        }
    }
    for (_, actor, t) in &accepted {
        for (prop, value) in &t.props {
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
    if std::env::var_os("HP3_TRAVEL_DEBUG").is_some() {
        let items = world(vm).level_info.is_some_and(|li| get(vm, li, "bNextItems").as_bool());
        eprintln!("travel to {url}: restart {restart} items {items}, {} players", info.len());
        for (n, actors) in &info {
            for a in actors {
                eprintln!("  {n}: {} {} ({} values)", vm.db.names.get(vm.db.get(a.class).name), a.name, a.props.len());
            }
        }
    }
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
    canvas::register(vm);
    interactions::register(vm);
    gui::register(vm);
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
        step_smooth: Default::default(),
        volume_hulls: None,
        travel_info: travel.clone().unwrap_or_default(),
        url_options: String::new(),
        world_info: None,
        save_requested: false,
        audio: Default::default(),
        movies: Default::default(),
        particles: Default::default(),
        debug_air_speed: None,
        phys_slots: None,
        controller_classes: Default::default(),
        tick_profile: Default::default(),
        actor_slots: Default::default(),
        config,
    }));
    let collision = build_collision(vm, map, level_model, &actors);
    world(vm).collision = collision;
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
    // The game exists before the level's actors begin play; they read Level.Game.
    if let Some(gc) = game_class.and_then(|g| find_object(vm, g)) {
        let game = natives::spawn(vm, level_info, &[Value::Object(Some(gc))])?.as_obj();
        world(vm).game = game;
        if let (Some(li), Some(g)) = (level_info, game) {
            set(vm, li, "Game", Value::Object(Some(g)));
            let options = take_world_options();
            fire(vm, g, "InitGame", &mut [Value::Str(options.clone()), Value::Str(String::new())]);
            world(vm).url_options = options;
        }
    }
    // Actors that do not belong to the current game state are hidden and stop colliding.
    for &a in &actors {
        fire(vm, a, "FilterForCurrentGameState", &mut []);
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
    // The persistent store has given the level its saved state (none yet): actors apply it.
    for &a in &actors {
        fire(vm, a, "PostPersistentDataRestored", &mut []);
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
    if let Some(g) = world(vm).game {
        let options = world(vm).url_options.clone();
        let portal = NEXT_PORTAL.with(|p| std::mem::take(&mut *p.borrow_mut()));
        let mut args = [Value::Str(portal), Value::Str(options), Value::Str(String::new())];
        let pc = fire(vm, g, "Login", &mut args).as_obj();
        if let Some(pc) = pc {
            world(vm).player = Some(pc);
            // The engine gives the local player's controller its viewport (console, GUI) and input object.
            interactions::init(vm, pc);
            fire(vm, pc, "InitInputSystem", &mut []);
            fire(vm, g, "PostLogin", &mut [Value::Object(Some(pc))]);
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
pub fn tick(vm: &mut Vm<'_>, dt: f32) {
    // The step is scaled by the level's time dilation; while someone has paused the game only the menus run.
    let dilation = world(vm).level_info.map(|li| get(vm, li, "TimeDilation").as_float()).filter(|d| *d > 0.0).unwrap_or(1.0);
    let dt = dt * dilation;
    if world(vm).level_info.is_some_and(|li| get(vm, li, "Pauser").as_obj().is_some()) {
        follow_player(vm);
        interactions::tick(vm, dt);
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
    if let Some(li) = world(vm).level_info {
        let t = world(vm).time;
        set(vm, li, "TimeSeconds", Value::Float(t));
        movie::tick(vm);
        particles::tick(vm, dt);
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
        // A level switch the game asked for happens when its countdown runs out.
        let next = get(vm, li, "NextURL").as_str().to_string();
        if !next.is_empty() {
            let countdown = get(vm, li, "NextSwitchCountdown").as_float() - dt;
            set(vm, li, "NextSwitchCountdown", Value::Float(countdown));
            if countdown <= 0.0 {
                set(vm, li, "NextURL", Value::Str(String::new()));
                world(vm).pending_travel = Some(next);
            }
        }
    }
    if world(vm).frame == 1
        && let Some(pc) = world(vm).player
    {
        // The engine tells the player's controller before the level's first tick.
        fire(vm, pc, "OnEnginePreFirstTick", &mut []);
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
    for &a in &actors {
        if vm.db.objects.get(a).deleted {
            continue;
        }
        if Some(a) == player {
            input::apply(vm, a, dt);
            fire(vm, a, "PlayerTick", &mut [Value::Float(dt)]);
            input::clear_axes(vm, a);
        }
        lap(vm, "actors: input", &mut clock, profile);
        anim::tick_actor(vm, a, dt);
        lap(vm, "actors: anim", &mut clock, profile);
        if let Some(v) = ticked.and_then(|s| vm.db.objects.values_mut(a).and_then(|v| v.get_mut(s))) {
            *v = Value::Bool(parity);
        }
        fire(vm, a, "Tick", &mut [Value::Float(dt)]);
        lap(vm, "actors: Tick event", &mut clock, profile);
        if vm.db.objects.get(a).deleted {
            continue;
        }
        if let Err(e) = vm.tick_state(a, dt) {
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
            && let Some(v) = vm.db.objects.values_mut(a).and_then(|v| v.get_mut(s))
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
        physics::step_actor(vm, a, dt);
        lap(vm, "actors: physics", &mut clock, profile);
    }
    mark(vm, "actors");
    touch::frame_pass(vm);
    mark(vm, "touching");
    anim::update_movement(vm, dt);
    mark(vm, "movement anims");
    anim::update_attachments(vm);
    mark(vm, "animation");
    // A save the game queued is written once the frame's tick is done, as the original does.
    save::run_queued(vm);
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
    let mut offset = vec_of(&get(vm, a, "PrePivot")) + vec_of(&get(vm, a, "SavedPrePivot"));
    let align = get(vm, a, "bAlignBottomAlways").as_bool() || (get(vm, a, "bAlignBottom").as_bool() && get(vm, a, "bCollideWorld").as_bool());
    if align {
        offset.z -= get(vm, a, "CollisionHeight").as_float();
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
    fire(vm, o, "Destroyed", &mut []);
    touch::forget(vm, o);
    vm.db.objects.get_mut(o).deleted = true;
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
        if !sm.collision_triangles.is_empty() {
            for t in &sm.collision_triangles {
                tris.push(t.vertices.map(|i| pos.get(i as usize).copied().unwrap_or(Vec3::ZERO)));
            }
        } else {
            for t in sm.indices.chunks_exact(3) {
                tris.push([t[0], t[1], t[2]].map(|i| pos.get(i as usize).copied().unwrap_or(Vec3::ZERO)));
            }
        }
    } else if get(vm, a, "Brush").as_obj().is_some() {
        tris = brush_triangles(vm, a, s3, scale)?;
    }
    if tris.is_empty() {
        return None;
    }
    let mut shape = collision::MoverShape::new(a, &tris);
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
pub(crate) fn volume_bounds(vm: &mut Vm<'_>, a: ObjId) -> Option<(glam::Vec3, glam::Vec3)> {
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

/// Which physics volume a pawn is in after it has moved: the highest priority one whose brush holds its location, else
/// the level's default. A change is told to the volumes, the pawn and its controller (`NotifyPhysicsVolumeChange`),
/// which is how a sliding volume puts the player into its sliding state.
pub fn update_physics_volume(vm: &mut Vm<'_>, a: ObjId) {
    if world(vm).volume_hulls.is_none() {
        let hulls = build_volume_hulls(vm);
        world(vm).volume_hulls = Some(hulls);
    }
    let p = vec_of(&get(vm, a, "Location"));
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
    let current = get(vm, a, "PhysicsVolume").as_obj();
    let new = match best {
        Some((_, v)) => Some(v),
        None => default_physics_volume(vm).or(current),
    };
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
    let class = vm.db.find("DefaultPhysicsVolume")?;
    let actors = world(vm).actors.clone();
    actors.into_iter().find(|&a| !vm.db.objects.get(a).deleted && vm.db.objects.get(a).class.is_some_and(|c| vm.is_child_of(c, class)))
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
                col.movers.push(shape);
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
        if let Some(mesh) = mesh.filter(|_| blocks) {
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
                eprintln!(
                    "meshstats {} tris={} model={}",
                    vm.db.names.get(vm.db.objects.get(mesh).name),
                    sm.collision_triangles.len(),
                    !sm.collision_model.is_none()
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
        let Some(Asset::Terrain(t)) = native_asset(map, i, &chain) else { continue };
        let (w, h) = (t.heightmap_x as usize, t.heightmap_y as usize);
        let v = |x: usize, y: usize| t.vertices.get(y * w + x).map(|p| Vec3::new(p.x, p.y, p.z));
        for y in 0..h.saturating_sub(1) {
            for x in 0..w.saturating_sub(1) {
                if let (Some(a0), Some(b0), Some(c0), Some(d0)) = (v(x, y), v(x + 1, y), v(x, y + 1), v(x + 1, y + 1)) {
                    col.add_triangle([a0, b0, d0], None);
                    col.add_triangle([a0, d0, c0], None);
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
        .or_else(|| pkg.find_export_path(rest))?;
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
