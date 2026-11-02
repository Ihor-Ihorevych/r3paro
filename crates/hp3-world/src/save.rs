//! Saved games. A save is the whole level: every live actor with its values and state, written to a slot file
//! (`Save/Save<slot>.usa`) when the game asks for it, and put back over a freshly loaded copy of the same map when
//! the slot is loaded (`OPEN ?LOAD=<slot>`).
//!
//! What the game does, in order: a script queues a save on the player's controller (`bQueuedToSaveGame`, the
//! slot in `iSaveSlotID`, and optionally a command to run afterwards in `AdditionalSaveGameCommand`, which is how a
//! slot gets copied to another). Once the frame's tick is done the engine tells every actor `PreSaveGame`, writes
//! the slot, then runs the command. A load opens the slot's level; its actors come back as they were, without
//! beginning play again. Slots are named by number only: the load page asks `SaveGameExists` for one number per
//! level, so the numbers mean levels of a profile.
//!
//! The file format is this engine's own (a small binary encoding of values, with references between actors kept
//! by name), not the original's package format.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use hp3_script::{Name, ObjId, Value, Vm};

use crate::{fire, get, set, world};

const MAGIC: &[u8; 8] = b"HP3SAVE1";

/// Where slots live: the `Save` folder next to the game's data.
fn save_dir(vm: &Vm<'_>) -> PathBuf {
    vm.db.set.root().join("Save")
}

fn slot_path(vm: &Vm<'_>, slot: i32) -> PathBuf {
    save_dir(vm).join(format!("Save{slot}.usa"))
}

/// Whether the slot holds a save (a file with something in it).
pub fn exists(vm: &Vm<'_>, slot: i32) -> bool {
    std::fs::metadata(slot_path(vm, slot)).is_ok_and(|m| m.len() > 0)
}

/// Removes a slot's file; false when there was none.
pub fn delete(vm: &Vm<'_>, slot: i32) -> bool {
    std::fs::remove_file(slot_path(vm, slot)).is_ok()
}

/// Copies one slot over another (`CopySavedGame`).
pub fn copy(vm: &Vm<'_>, from: i32, to: i32) -> bool {
    from != to && std::fs::copy(slot_path(vm, from), slot_path(vm, to)).is_ok()
}

// ---------------------------------------------------------------------------------------------------------
// What is written

/// A reference to an object in a saved value.
#[derive(Clone, Debug, PartialEq)]
enum Ref {
    None,
    /// An actor of the level, by name.
    Actor(String),
    /// An object of a package (a texture, a mesh, a class...): the package and the export.
    Package(String, u32),
    /// An object made while running (a menu, an input handler): not saved, the slot keeps the loaded level's own.
    Runtime,
}

#[derive(Clone, Debug)]
enum Saved {
    Empty,
    Byte(u8),
    Int(i32),
    Bool(bool),
    Float(f32),
    Name(String),
    Str(String),
    Object(Ref),
    Struct(Vec<Saved>),
    Array(Vec<Saved>),
    Delegate(Ref, String),
}

#[derive(Clone, Debug, Default)]
struct SavedActor {
    name: String,
    class: String,
    values: Vec<Saved>,
    /// The state it is in and, while its state code waits, where the code goes on (state holding the code, statement).
    state: Option<String>,
    resume: Option<(String, u32)>,
    sleep: Option<f32>,
    /// `SetTimer`: rate, time left, looping.
    timer: Option<(f32, f32, bool)>,
    /// What its main animation channel plays: sequence, frame, rate, looping.
    animation: Option<(String, f32, f32, bool)>,
}

#[derive(Clone, Debug, Default)]
pub struct Slot {
    /// The map the level was (`HP3_Adv1Express`).
    pub map: String,
    time: f32,
    spawned: u32,
    /// The actor that was the local player's controller.
    player: String,
    world_info: Vec<Saved>,
    actors: Vec<SavedActor>,
}

// -- bytes ------------------------------------------------------------------------------------------------

struct Out(Vec<u8>);

impl Out {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn str(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.0.extend_from_slice(s.as_bytes());
    }
    fn reference(&mut self, r: &Ref) {
        match r {
            Ref::None => self.u8(0),
            Ref::Actor(n) => {
                self.u8(1);
                self.str(n);
            }
            Ref::Package(p, i) => {
                self.u8(2);
                self.str(p);
                self.u32(*i);
            }
            Ref::Runtime => self.u8(3),
        }
    }
    fn value(&mut self, v: &Saved) {
        match v {
            Saved::Empty => self.u8(0),
            Saved::Byte(b) => {
                self.u8(1);
                self.u8(*b);
            }
            Saved::Int(i) => {
                self.u8(2);
                self.i32(*i);
            }
            Saved::Bool(b) => {
                self.u8(3);
                self.u8(*b as u8);
            }
            Saved::Float(f) => {
                self.u8(4);
                self.f32(*f);
            }
            Saved::Name(n) => {
                self.u8(5);
                self.str(n);
            }
            Saved::Str(s) => {
                self.u8(6);
                self.str(s);
            }
            Saved::Object(r) => {
                self.u8(7);
                self.reference(r);
            }
            Saved::Struct(f) => {
                self.u8(8);
                self.values(f);
            }
            Saved::Array(a) => {
                self.u8(9);
                self.values(a);
            }
            Saved::Delegate(r, n) => {
                self.u8(10);
                self.reference(r);
                self.str(n);
            }
        }
    }
    fn values(&mut self, v: &[Saved]) {
        self.u32(v.len() as u32);
        for x in v {
            self.value(x);
        }
    }
    fn optional_str(&mut self, s: &Option<String>) {
        match s {
            Some(s) => {
                self.u8(1);
                self.str(s);
            }
            None => self.u8(0),
        }
    }
}

struct In<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> In<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.data.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn i32(&mut self) -> Option<i32> {
        Some(i32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn str(&mut self) -> Option<String> {
        let n = self.u32()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
    fn reference(&mut self) -> Option<Ref> {
        Some(match self.u8()? {
            0 => Ref::None,
            1 => Ref::Actor(self.str()?),
            2 => Ref::Package(self.str()?, self.u32()?),
            3 => Ref::Runtime,
            _ => return None,
        })
    }
    fn value(&mut self) -> Option<Saved> {
        Some(match self.u8()? {
            0 => Saved::Empty,
            1 => Saved::Byte(self.u8()?),
            2 => Saved::Int(self.i32()?),
            3 => Saved::Bool(self.u8()? != 0),
            4 => Saved::Float(self.f32()?),
            5 => Saved::Name(self.str()?),
            6 => Saved::Str(self.str()?),
            7 => Saved::Object(self.reference()?),
            8 => Saved::Struct(self.values()?),
            9 => Saved::Array(self.values()?),
            10 => Saved::Delegate(self.reference()?, self.str()?),
            _ => return None,
        })
    }
    fn values(&mut self) -> Option<Vec<Saved>> {
        let n = self.u32()? as usize;
        (0..n).map(|_| self.value()).collect()
    }
    fn optional_str(&mut self) -> Option<Option<String>> {
        Some(if self.u8()? == 1 { Some(self.str()?) } else { None })
    }
}

fn encode(slot: &Slot) -> Vec<u8> {
    let mut o = Out(Vec::new());
    o.0.extend_from_slice(MAGIC);
    o.str(&slot.map);
    o.f32(slot.time);
    o.u32(slot.spawned);
    o.str(&slot.player);
    o.values(&slot.world_info);
    o.u32(slot.actors.len() as u32);
    for a in &slot.actors {
        o.str(&a.name);
        o.str(&a.class);
        o.values(&a.values);
        o.optional_str(&a.state);
        match &a.resume {
            Some((s, pc)) => {
                o.u8(1);
                o.str(s);
                o.u32(*pc);
            }
            None => o.u8(0),
        }
        match a.sleep {
            Some(t) => {
                o.u8(1);
                o.f32(t);
            }
            None => o.u8(0),
        }
        match a.timer {
            Some((rate, left, looping)) => {
                o.u8(1);
                o.f32(rate);
                o.f32(left);
                o.u8(looping as u8);
            }
            None => o.u8(0),
        }
        match &a.animation {
            Some((seq, frame, rate, looping)) => {
                o.u8(1);
                o.str(seq);
                o.f32(*frame);
                o.f32(*rate);
                o.u8(*looping as u8);
            }
            None => o.u8(0),
        }
    }
    o.0
}

fn decode(data: &[u8]) -> Option<Slot> {
    if data.get(..8)? != MAGIC {
        return None;
    }
    let mut r = In { data, at: 8 };
    let map = r.str()?;
    let time = r.f32()?;
    let spawned = r.u32()?;
    let player = r.str()?;
    let world_info = r.values()?;
    let count = r.u32()? as usize;
    let mut actors = Vec::with_capacity(count);
    for _ in 0..count {
        let name = r.str()?;
        let class = r.str()?;
        let values = r.values()?;
        let state = r.optional_str()?;
        let resume = if r.u8()? == 1 { Some((r.str()?, r.u32()?)) } else { None };
        let sleep = if r.u8()? == 1 { Some(r.f32()?) } else { None };
        let timer = if r.u8()? == 1 { Some((r.f32()?, r.f32()?, r.u8()? != 0)) } else { None };
        let animation = if r.u8()? == 1 { Some((r.str()?, r.f32()?, r.f32()?, r.u8()? != 0)) } else { None };
        actors.push(SavedActor { name, class, values, state, resume, sleep, timer, animation });
    }
    Some(Slot { map, time, spawned, player, world_info, actors })
}

/// The slot's contents, or None when there is no such save (or it is not one of ours).
pub fn read(vm: &Vm<'_>, slot: i32) -> Option<Slot> {
    decode(&std::fs::read(slot_path(vm, slot)).ok()?)
}

// ---------------------------------------------------------------------------------------------------------
// Taking a level apart

fn name_of(vm: &Vm<'_>, a: ObjId) -> String {
    vm.db.names.get(vm.db.objects.get(a).name).to_string()
}

fn reference_of(vm: &Vm<'_>, actors: &HashMap<ObjId, String>, o: Option<ObjId>) -> Ref {
    let Some(o) = o else { return Ref::None };
    if let Some(n) = actors.get(&o) {
        return Ref::Actor(n.clone());
    }
    match &vm.db.objects.get(o).key {
        Some((pkg, index)) => Ref::Package(pkg.to_string(), *index as u32),
        None => Ref::Runtime,
    }
}

fn save_value(vm: &Vm<'_>, actors: &HashMap<ObjId, String>, v: &Value) -> Saved {
    match v {
        Value::Empty => Saved::Empty,
        Value::Byte(b) => Saved::Byte(*b),
        Value::Int(i) => Saved::Int(*i),
        Value::Bool(b) => Saved::Bool(*b),
        Value::Float(f) => Saved::Float(*f),
        Value::Name(n) => Saved::Name(vm.db.names.get(*n).to_string()),
        Value::Str(s) => Saved::Str(s.clone()),
        Value::Object(o) => Saved::Object(reference_of(vm, actors, *o)),
        Value::Struct(f) => Saved::Struct(f.iter().map(|x| save_value(vm, actors, x)).collect()),
        Value::Array(a) => Saved::Array(a.iter().map(|x| save_value(vm, actors, x)).collect()),
        Value::Delegate(o, n) => Saved::Delegate(reference_of(vm, actors, *o), vm.db.names.get(*n).to_string()),
    }
}

/// Everything in the level as it is now. Every actor first hears `PreSaveGame`.
fn capture(vm: &mut Vm<'_>) -> Slot {
    let all = world(vm).actors.clone();
    let live: Vec<ObjId> = all.into_iter().filter(|&a| !vm.db.objects.get(a).deleted).collect();
    for &a in &live {
        fire(vm, a, "PreSaveGame", &mut []);
    }
    let live: Vec<ObjId> = live.into_iter().filter(|&a| !vm.db.objects.get(a).deleted).collect();
    let names: HashMap<ObjId, String> = live.iter().map(|&a| (a, name_of(vm, a))).collect();
    let mut actors = Vec::with_capacity(live.len());
    for &a in &live {
        let Some(class) = vm.db.objects.get(a).class else { continue };
        let values = vm.db.objects.values(a).map(|v| v.iter().map(|x| save_value(vm, &names, x)).collect()).unwrap_or_default();
        let state = vm.state_of(a).map(|s| vm.db.names.get(vm.db.get(s).name).to_string());
        let resume = vm.state_pc.get(&a).map(|&(owner, pc)| (vm.db.names.get(vm.db.get(owner).name).to_string(), pc as u32));
        let sleep = match vm.latent.get(&a) {
            Some(hp3_script::vm::Latent::Sleep(t)) => Some(*t),
            _ => None,
        };
        let timer = world(vm).timers.get(&a).map(|t| (t.rate, t.remaining, t.looping));
        actors.push(SavedActor {
            name: names[&a].clone(),
            class: vm.db.names.get(vm.db.get(class).name).to_string(),
            values,
            state,
            resume,
            sleep,
            timer,
            animation: crate::anim::main_animation(vm, a),
        });
    }
    let world_info = match world(vm).world_info {
        Some(w) => vm.db.objects.values(w).map(|v| v.iter().map(|x| save_value(vm, &names, x)).collect()).unwrap_or_default(),
        None => Vec::new(),
    };
    let player = world(vm).player.and_then(|p| names.get(&p).cloned()).unwrap_or_default();
    let w = world(vm);
    Slot { map: w.map.name.to_string(), time: w.time, spawned: w.spawned as u32, player, world_info, actors }
}

/// Writes the level to a slot.
pub fn write(vm: &mut Vm<'_>, slot: i32) -> bool {
    let saved = capture(vm);
    let dir = save_dir(vm);
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    // Written beside the slot and moved over it, so a failed write never leaves half a save.
    let path = slot_path(vm, slot);
    let part = path.with_extension("tmp");
    let ok = std::fs::write(&part, encode(&saved)).and_then(|_| std::fs::rename(&part, &path)).is_ok();
    if ok {
        vm.log.push(format!("engine: saved game to slot {slot} ({} actors)", saved.actors.len()));
    }
    ok
}

// ---------------------------------------------------------------------------------------------------------
// Putting it back

fn load_ref(vm: &mut Vm<'_>, ids: &HashMap<String, ObjId>, r: &Ref) -> Option<Option<ObjId>> {
    match r {
        Ref::None => Some(None),
        Ref::Actor(n) => Some(ids.get(n).copied()),
        Ref::Package(pkg, index) => {
            let p = vm.db.set.get(pkg).ok()?;
            ((*index as usize) < p.exports.len()).then(|| Some(vm.db.obj_for(&p, *index as usize)))
        }
        Ref::Runtime => None,
    }
}

/// A saved value as a live one; None when it holds something that is not saved.
fn load_value(vm: &mut Vm<'_>, ids: &HashMap<String, ObjId>, v: &Saved) -> Option<Value> {
    Some(match v {
        Saved::Empty => Value::Empty,
        Saved::Byte(b) => Value::Byte(*b),
        Saved::Int(i) => Value::Int(*i),
        Saved::Bool(b) => Value::Bool(*b),
        Saved::Float(f) => Value::Float(*f),
        Saved::Name(n) => Value::Name(vm.db.names.intern(n)),
        Saved::Str(s) => Value::Str(s.clone()),
        Saved::Object(r) => Value::Object(load_ref(vm, ids, r)?),
        Saved::Struct(f) => Value::Struct(f.iter().map(|x| load_value(vm, ids, x)).collect::<Option<Vec<_>>>()?.into()),
        Saved::Array(a) => Value::Array(a.iter().map(|x| load_value(vm, ids, x)).collect::<Option<Vec<_>>>()?),
        Saved::Delegate(r, n) => Value::Delegate(load_ref(vm, ids, r)?, vm.db.names.intern(n)),
    })
}

fn apply_values(vm: &mut Vm<'_>, ids: &HashMap<String, ObjId>, target: ObjId, saved: &[Saved]) {
    for (slot, v) in saved.iter().enumerate() {
        let len = vm.db.objects.values(target).map_or(0, Vec::len);
        if slot >= len {
            break;
        }
        // A value that holds something made while running keeps what the loaded level has.
        if let Some(value) = load_value(vm, ids, v)
            && let Some(vals) = vm.db.objects.values_mut(target)
        {
            vals[slot] = value;
        }
    }
}

/// Removes an actor without any of the events a destroy raises: it was never there.
fn drop_quietly(vm: &mut Vm<'_>, a: ObjId) {
    crate::touch::forget(vm, a);
    vm.db.objects.get_mut(a).deleted = true;
    vm.latent.remove(&a);
    vm.state_pc.remove(&a);
    let w = world(vm);
    w.actors.retain(|&x| x != a);
    w.timers.remove(&a);
}

/// Puts a saved level over the freshly loaded copy of its map: actors that are not in the save go, actors that
/// are only in the save are made (without beginning play), and every actor gets its values, state, timer and
/// animation back. Nothing begins play again.
pub fn restore(vm: &mut Vm<'_>, slot: &Slot) {
    let level_info = world(vm).level_info;
    let mut ids: HashMap<String, ObjId> = HashMap::new();
    for &a in &world(vm).actors.clone() {
        ids.insert(name_of(vm, a), a);
    }
    let kept: HashSet<&str> = slot.actors.iter().map(|a| a.name.as_str()).collect();
    for (name, id) in ids.clone() {
        if !kept.contains(name.as_str()) && Some(id) != level_info {
            drop_quietly(vm, id);
            ids.remove(&name);
        }
    }
    for saved in &slot.actors {
        if ids.contains_key(&saved.name) {
            continue;
        }
        let Some(class) = vm.db.find(&saved.class) else {
            vm.log.push(format!("load: no class {} for {}", saved.class, saved.name));
            continue;
        };
        let name = vm.db.names.intern(&saved.name);
        let obj = vm.new_object_in(class, name, level_info);
        world(vm).actors.push(obj);
        ids.insert(saved.name.clone(), obj);
    }
    // Values first (references between actors resolve through `ids`), then what depends on them.
    for saved in &slot.actors {
        if let Some(&a) = ids.get(&saved.name) {
            apply_values(vm, &ids, a, &saved.values);
        }
    }
    if let Some(wi) = world(vm).world_info {
        apply_values(vm, &ids, wi, &slot.world_info);
    }
    for saved in &slot.actors {
        let Some(&a) = ids.get(&saved.name) else { continue };
        let Some(class) = vm.db.objects.get(a).class else { continue };
        let state = saved.state.as_deref().and_then(|s| {
            let n = vm.db.names.intern(s);
            vm.find_state(class, n)
        });
        if let hp3_script::objects::ObjKind::Instance { state: slot_state, .. } = &mut vm.db.objects.get_mut(a).kind {
            *slot_state = state;
        }
        vm.latent.remove(&a);
        vm.state_pc.remove(&a);
        if let Some((owner, pc)) = &saved.resume {
            let n = vm.db.names.intern(owner);
            if let Some(owner) = vm.find_state(class, n) {
                vm.state_pc.insert(a, (owner, *pc as usize));
            }
        }
        if let Some(t) = saved.sleep {
            vm.latent.insert(a, hp3_script::vm::Latent::Sleep(t));
        }
        match saved.timer {
            Some((rate, remaining, looping)) => {
                world(vm).timers.insert(a, crate::Timer { rate, remaining, looping });
            }
            None => {
                world(vm).timers.remove(&a);
            }
        }
        if let Some(anim) = &saved.animation {
            crate::anim::resume_main_animation(vm, a, anim);
        }
    }
    if std::env::var_os("HP3_SAVE_DEBUG").is_some() {
        let pc = world(vm).player;
        let name = pc.map(|p| name_of(vm, p));
        let alive = pc.is_some_and(|p| !vm.db.objects.get(p).deleted && world(vm).actors.contains(&p));
        eprintln!("load: player {name:?} alive {alive}, {} actors, {} saved", world(vm).actors.len(), slot.actors.len());
    }
    // The game does not log a player in when it opens a saved level: the player's controller is one of the
    // saved actors, and the engine gives it its viewport and input here, as a login would.
    if world(vm).player.is_none() {
        let found = ids.get(&slot.player).copied();
        if let Some(pc) = found {
            world(vm).player = Some(pc);
            crate::interactions::init(vm, pc);
            if get(vm, pc, "PlayerInput").as_obj().is_none() {
                fire(vm, pc, "InitInputSystem", &mut []);
            }
            crate::init_camera_settings(vm, pc);
        }
    }
    let w = world(vm);
    w.time = slot.time;
    w.spawned = w.spawned.max(slot.spawned as usize);
    resync(vm);
}

/// The engine's own tables follow the actors' restored places and links.
fn resync(vm: &mut Vm<'_>) {
    let actors = world(vm).actors.clone();
    world(vm).touch = Default::default();
    world(vm).anim.attachments.clear();
    for &a in &actors {
        let bone = get(vm, a, "AttachmentBone").as_name();
        if bone != Name::NONE
            && let Some(parent) = get(vm, a, "Base").as_obj()
        {
            let bone = vm.db.names.get(bone).to_ascii_lowercase();
            world(vm).anim.attachments.insert(a, (parent, bone));
        }
    }
    // Movers stand where they were saved.
    let movers: Vec<ObjId> = world(vm).collision.movers.iter().map(|m| m.actor).collect();
    for m in movers {
        let t = crate::mover_transform(vm, m);
        if let Some(shape) = world(vm).collision.movers.iter_mut().find(|s| s.actor == m) {
            shape.place(t);
        }
    }
    crate::follow_player(vm);
}

// ---------------------------------------------------------------------------------------------------------
// The game asks

/// After a frame's tick: a save the player's controller queued is written, and the command that came with it runs.
pub fn run_queued(vm: &mut Vm<'_>) {
    let Some(pc) = world(vm).player else { return };
    if !get(vm, pc, "bQueuedToSaveGame").as_bool() {
        return;
    }
    set(vm, pc, "bQueuedToSaveGame", Value::Bool(false));
    let after = get(vm, pc, "AdditionalSaveGameCommand").as_str().to_string();
    set(vm, pc, "AdditionalSaveGameCommand", Value::Str(String::new()));
    let slot = get(vm, pc, "iSaveSlotID").as_int();
    write(vm, slot);
    if !after.is_empty() {
        console(vm, &after);
    }
}

/// The engine's console commands that saves use: `SAVEGAME <slot>`, `COPYSAVEDGAME <from> <to>` and
/// `OPEN ?LOAD=<slot>` (which queues the level switch). Anything else is not for us and gives an empty answer.
pub fn console(vm: &mut Vm<'_>, command: &str) -> String {
    let mut words = command.split_whitespace();
    let Some(verb) = words.next() else { return String::new() };
    let rest: Vec<&str> = words.collect();
    let number = |s: Option<&&str>| s.and_then(|v| v.trim_start_matches(['?', '=']).parse::<i32>().ok());
    match verb.to_ascii_uppercase().as_str() {
        "SAVEGAME" => {
            if let Some(slot) = number(rest.first()) {
                write(vm, slot);
            }
        }
        // The Quit page's Yes: the app leaves its main loop (on the phone the game's activity then ends).
        "EXIT" | "QUIT" => world(vm).quit_requested = true,
        "COPYSAVEDGAME" => {
            if let (Some(from), Some(to)) = (number(rest.first()), number(rest.get(1))) {
                copy(vm, from, to);
            }
        }
        "OPEN" => {
            let url = rest.join(" ");
            if let Some(slot) = option_slot(&url)
                && let Some(saved) = read(vm, slot)
            {
                world(vm).pending_travel = Some(format!("{}?load={slot}", saved.map));
            }
        }
        _ => {}
    }
    String::new()
}

/// The slot a `?load=<n>` option asks for.
pub fn option_slot(url: &str) -> Option<i32> {
    url.split(['?', '#']).find_map(|o| {
        let (k, v) = o.split_once('=')?;
        k.trim().eq_ignore_ascii_case("load").then(|| v.trim().parse().ok())?
    })
}

/// A level just loaded for `?load=<slot>` gets the saved level put over it.
pub fn finish_load(vm: &mut Vm<'_>, slot: i32) {
    match read(vm, slot) {
        Some(saved) => restore(vm, &saved),
        None => vm.log.push(format!("load: slot {slot} cannot be read")),
    }
}
