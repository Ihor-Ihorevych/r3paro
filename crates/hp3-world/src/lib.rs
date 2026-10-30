//! The running level: actors loaded from a map into the script VM, the begin-play sequence, and the tick that
//! drives events, timers and state code.
//!
//! The world lives in `Vm::ext`; natives reach it with `world(vm)`.

pub mod natives;

use std::collections::HashMap;
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
    /// Script errors raised while running events, with where they happened.
    pub errors: Vec<String>,
    pub spawned: usize,
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
pub fn load(vm: &mut Vm<'_>, map: &Rc<Package>, game_class: Option<&str>) -> Result<(), VmError> {
    let actor_class = vm.db.find("Actor").ok_or_else(|| VmError::Runtime("no Actor class".into()))?;
    natives::register(vm);
    // Every scripted object of the map, so references between them resolve to live instances.
    let mut level_order = Vec::new();
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
        timers: HashMap::new(),
        actor_class,
        slots: HashMap::new(),
        errors: Vec::new(),
        spawned: 0,
    }));
    // The game exists before the level's actors begin play; they read Level.Game.
    if let Some(gc) = game_class.and_then(|g| find_object(vm, g)) {
        let game = natives::spawn(vm, level_info, &[Value::Object(Some(gc))])?.as_obj();
        world(vm).game = game;
        if let (Some(li), Some(g)) = (level_info, game) {
            set(vm, li, "Game", Value::Object(Some(g)));
            fire(vm, g, "InitGame", &mut [Value::Str(String::new()), Value::Str(String::new())]);
        }
    }
    for &a in &actors {
        fire(vm, a, "PreBeginPlay", &mut []);
    }
    for &a in &actors {
        fire(vm, a, "BeginPlay", &mut []);
    }
    for &a in &actors {
        fire(vm, a, "PostBeginPlay", &mut []);
    }
    for &a in &actors {
        fire(vm, a, "SetInitialState", &mut []);
    }
    if let Some(g) = world(vm).game {
        let mut args = [Value::Str(String::new()), Value::Str(String::new()), Value::Str(String::new())];
        let pc = fire(vm, g, "Login", &mut args).as_obj();
        if let Some(pc) = pc {
            world(vm).player = Some(pc);
            fire(vm, g, "PostLogin", &mut [Value::Object(Some(pc))]);
        }
    }
    Ok(())
}

/// Advances the level by `dt` seconds: actor ticks, timers, then state code.
pub fn tick(vm: &mut Vm<'_>, dt: f32) {
    let actors = {
        let w = world(vm);
        w.time += dt;
        w.actors.clone()
    };
    if let Some(li) = world(vm).level_info {
        let t = world(vm).time;
        set(vm, li, "TimeSeconds", Value::Float(t));
    }
    for &a in &actors {
        if vm.db.objects.get(a).deleted {
            continue;
        }
        fire(vm, a, "Tick", &mut [Value::Float(dt)]);
    }
    let mut due = Vec::new();
    for (&a, t) in world(vm).timers.iter_mut() {
        t.remaining -= dt;
        if t.remaining <= 0.0 {
            due.push(a);
            if t.looping {
                t.remaining += t.rate.max(0.0001);
            }
        }
    }
    world(vm).timers.retain(|_, t| t.remaining > 0.0 || t.looping);
    for a in due {
        if !vm.db.objects.get(a).deleted {
            fire(vm, a, "Timer", &mut []);
        }
    }
    for &a in &actors {
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
    }
}

/// Finds an object by its full path (`Package.Group.Name`), loading classes as needed.
pub fn find_object(vm: &mut Vm<'_>, path: &str) -> Option<ObjId> {
    let (pkg_name, rest) = path.split_once('.')?;
    let pkg = vm.db.set.get(pkg_name).ok()?;
    let index = pkg.find_export_path(rest)?;
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
