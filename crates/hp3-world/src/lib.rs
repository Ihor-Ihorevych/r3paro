//! The running level: actors loaded from a map into the script VM, the begin-play sequence, and the tick that
//! drives events, timers and state code.
//!
//! The world lives in `Vm::ext`; natives reach it with `world(vm)`.

pub mod anim;
pub mod canvas;
pub mod collision;
pub mod config;
pub mod nav;
pub mod input;
pub mod natives;
pub mod physics;

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
    /// Pairs of actors currently touching (lower id first).
    pub touching: hp3_script::fx::FxHashSet<(ObjId, ObjId)>,
    pub anim: anim::Animation,
    pub nav: nav::Navigation,
    /// Opacity set by `SetOpacity` (1 when absent).
    pub opacity: HashMap<ObjId, f32>,
    pub canvas: canvas::CanvasState,
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
    let game_class = config.get("Engine.Engine", "DefaultGame").map(str::to_string);
    let game_class = game_class.as_deref();
    let actor_class = vm.db.find("Actor").ok_or_else(|| VmError::Runtime("no Actor class".into()))?;
    natives::register(vm);
    anim::register(vm);
    nav::register(vm);
    canvas::register(vm);
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
        touching: Default::default(),
        anim: Default::default(),
        nav: Default::default(),
        opacity: Default::default(),
        canvas: Default::default(),
        config,
    }));
    let collision = build_collision(vm, map, level_model, &actors);
    world(vm).collision = collision;
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
            // The engine gives the local player's controller its input object.
            fire(vm, pc, "InitInputSystem", &mut []);
            fire(vm, g, "PostLogin", &mut [Value::Object(Some(pc))]);
        }
        follow_player(vm);
        // A loaded map finishes by accepting the player's travel: pawn first, then its controller.
        if let Some(pc) = world(vm).player {
            if let Some(pawn) = get(vm, pc, "Pawn").as_obj() {
                arrive_from(vm, pawn);
                fire(vm, pawn, "TravelPostAccept", &mut []);
            }
            fire(vm, pc, "TravelPostAccept", &mut []);
            init_camera_settings(vm, pc);
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

/// Advances the level by `dt` seconds: actor ticks, timers, then state code.
pub fn tick(vm: &mut Vm<'_>, dt: f32) {
    let actors = {
        let w = world(vm);
        w.time += dt;
        w.frame += 1;
        w.actors.clone()
    };
    if let Some(li) = world(vm).level_info {
        let t = world(vm).time;
        set(vm, li, "TimeSeconds", Value::Float(t));
    }
    // The player's controller reads input and moves first, as the local player does.
    follow_player(vm);
    if let Some(pc) = world(vm).player {
        input::apply(vm, pc);
        fire(vm, pc, "PlayerTick", &mut [Value::Float(dt)]);
    }
    nav::tick(vm, dt);
    physics::step(vm, dt);
    physics::update_touching(vm);
    anim::update_movement(vm);
    anim::tick(vm, dt);
    anim::update_attachments(vm);
    // Ticked actors carry the frame's parity in `bTicked`; scripts compare it to see a new frame (HP3's
    // camera updates its view once per frame this way).
    let parity = world(vm).frame % 2 == 1;
    for &a in &actors {
        if vm.db.objects.get(a).deleted {
            continue;
        }
        set(vm, a, "bTicked", Value::Bool(parity));
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

fn vec_of(v: &Value) -> glam::Vec3 {
    physics::vec(v)
}

/// Collision geometry of a level: its BSP, the triangles of blocking static meshes, and terrain.
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
    for &a in actors {
        let mesh = get(vm, a, "StaticMesh").as_obj();
        let blocks = get(vm, a, "bCollideActors").as_bool()
            && (get(vm, a, "bBlockActors").as_bool() || get(vm, a, "bBlockPlayers").as_bool());
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
