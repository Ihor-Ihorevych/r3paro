//! Actor natives the level needs to run. Written from the script declarations and standard UnrealScript
//! semantics; NOT yet verified against HP3's Engine.dll, and the animation, sound and collision ones are
//! placeholders until those systems exist.

use hp3_script::vm::Latent;
use hp3_script::{Name, ObjId, Value, Vm, VmError};

use crate::{fire, get, set, world, Timer};

fn obj_arg(a: &[Value], i: usize) -> Option<ObjId> {
    a.get(i).and_then(Value::as_obj)
}

fn vec3(v: &Value) -> [f32; 3] {
    let f = v.fields();
    [0, 1, 2].map(|i| f.get(i).map(Value::as_float).unwrap_or(0.0))
}

fn given(a: &[Value], i: usize) -> bool {
    !matches!(a.get(i), None | Some(Value::Empty))
}

/// Gives an actor a new base (what it stands on or hangs from), or none. The old base hears `Detach` and the new
/// one `Attach` (unless it is world geometry), and the actor itself `BaseChange` when `notify` is set. A base that
/// would sit on its own child is refused. A bone attachment ends when the base changes.
pub fn set_base(vm: &mut Vm<'_>, o: ObjId, new_base: Option<ObjId>, notify: bool) {
    let mut up = new_base;
    for _ in 0..64 {
        match up {
            Some(x) if x == o => return,
            Some(x) => up = get(vm, x, "Base").as_obj(),
            None => break,
        }
    }
    let old = get(vm, o, "Base").as_obj();
    if new_base != old && get(vm, o, "AttachTag").as_name() != Name::NONE {
        // Attached actors (a knight on its panel): every change of what they ride on is written to the game log.
        let (from, to) = (old.map(|b| hp3_script::natives::object_path(vm, b)), new_base.map(|b| hp3_script::natives::object_path(vm, b)));
        let who = hp3_script::natives::object_path(vm, o);
        let state = vm.state_of(o).map(|s| vm.db.names.get(vm.db.get(s).name).to_string()).unwrap_or_default();
        vm.log.push(format!("base: {who} (state {state}) {from:?} -> {to:?}"));
    }
    if new_base != old {
        if let Some(b) = old
            && !get(vm, b, "bWorldGeometry").as_bool()
        {
            fire(vm, b, "Detach", &mut [Value::Object(Some(o))]);
        }
        set(vm, o, "Base", Value::Object(new_base));
        if let Some(b) = new_base {
            world(vm).ever_base.insert(b);
        }
        if world(vm).anim.attachments.get(&o).map(|(parent, _)| Some(*parent)) != Some(new_base) {
            world(vm).anim.attachments.remove(&o);
        }
        if let Some(b) = new_base
            && !get(vm, b, "bWorldGeometry").as_bool()
        {
            fire(vm, b, "Attach", &mut [Value::Object(Some(o))]);
        }
        if notify {
            fire(vm, o, "BaseChange", &mut []);
        }
    }
    let on_world = new_base.is_none_or(|b| get(vm, b, "bWorldGeometry").as_bool());
    if on_world {
        set(vm, o, "AttachmentBone", Value::Name(Name::NONE));
    }
}

/// What an actor iterator asks for beyond being in range.
struct Spatial {
    /// A line from the centre must reach the actor without meeting the world.
    visible: bool,
    /// Only actors that collide with actors.
    colliding: bool,
}

fn spatial(vm: &mut Vm<'_>, this: Option<ObjId>, a: &[Value], what: Spatial) -> Result<Vec<Vec<Value>>, VmError> {
    let Some(o) = this else { return Ok(Vec::new()) };
    let radius = a.get(2).map(Value::as_float).unwrap_or(0.0);
    let center = if given(a, 3) { crate::physics::vec(&a[3]) } else { crate::physics::vec(&get(vm, o, "Location")) };
    // The visible-actors form has no collision radius in its range test, and 0 means no limit.
    let plain_visible = what.visible && !what.colliding;
    let unlimited = plain_visible && radius == 0.0;
    let ignore_hidden = plain_visible || (what.visible && given(a, 4) && a[4].as_bool());
    let class = obj_arg(a, 0).and_then(|c| vm.class_of(c));
    let Some(loc_slot) = location_slot(vm, o) else { return Ok(Vec::new()) };
    let mut actors = if unlimited { world(vm).actors.clone() } else { actors_near(vm, center, radius, loc_slot) };
    // Creation order, as a full scan of the actor list gives.
    actors.sort_unstable();
    let mut out = Vec::new();
    for x in actors {
        if vm.db.objects.get(x).deleted {
            continue;
        }
        let Some(p) = location_at(vm, x, loc_slot) else { continue };
        let reach = if plain_visible { radius } else { radius + get(vm, x, "CollisionRadius").as_float() };
        if !unlimited && p.distance_squared(center) >= reach * reach {
            continue;
        }
        if let (Some(want), Some(have)) = (class, vm.db.objects.get(x).class)
            && !vm.is_child_of(have, want)
        {
            continue;
        }
        if what.colliding && !get(vm, x, "bCollideActors").as_bool() {
            continue;
        }
        if ignore_hidden && get(vm, x, "bHidden").as_bool() {
            continue;
        }
        if what.visible {
            let blocked = world(vm).collision.trace_static(center, p, glam::Vec3::ZERO).is_some_and(|h| h.actor != Some(x) && h.actor != Some(o));
            if blocked {
                continue;
            }
        }
        let mut item = a.to_vec();
        item[1] = Value::Object(Some(x));
        out.push(item);
    }
    Ok(out)
}

/// Which colliding actors a line check looks at.
struct LineFilter {
    /// Only actors of this class (or a subclass).
    class: Option<hp3_script::StructId>,
    /// Only actors that can be aimed at or that block (`bProjTarget` / `bBlockActors`).
    projectile_targets: bool,
    /// Movers are met as well (a trace of the world already includes them).
    movers: bool,
}

/// Every collidable actor a box swept from `start` to `end` meets, with where. Actors must collide, and block the
/// kind of trace made (`bBlockZeroExtentTraces` for a line, `bBlockNonZeroExtentTraces` with an extent); volumes
/// and static meshes that are part of the level are left to the level's own check, and what `source` owns (or its
/// pawn) is skipped. Actors are cylinders; movers are their geometry.
fn line_hits(vm: &mut Vm<'_>, source: ObjId, start: glam::Vec3, end: glam::Vec3, extent: glam::Vec3, filter: &LineFilter) -> Vec<crate::collision::Hit> {
    let volume = vm.db.find("Volume");
    let own_pawn = get(vm, source, "Pawn").as_obj();
    let mut hits = Vec::new();
    // Hundreds of actors are near a line in a busy level: their properties are read by slot, the cheap flags first.
    let (collide, block_flag, owner) = (crate::actor_slot(vm, "bCollideActors"), crate::actor_slot(vm, if extent == glam::Vec3::ZERO { "bBlockZeroExtentTraces" } else { "bBlockNonZeroExtentTraces" }), crate::actor_slot(vm, "Owner"));
    let (proj_target, block_actors, static_mesh, world_geometry) = (crate::actor_slot(vm, "bProjTarget"), crate::actor_slot(vm, "bBlockActors"), crate::actor_slot(vm, "StaticMesh"), crate::actor_slot(vm, "bWorldGeometry"));
    let (radius, height, location) = (crate::actor_slot(vm, "CollisionRadius"), crate::actor_slot(vm, "CollisionHeight"), crate::actor_slot(vm, "Location"));
    let (collide_type, collision_width, rotation) = (crate::actor_slot(vm, "CollideType"), crate::actor_slot(vm, "CollisionWidth"), crate::actor_slot(vm, "Rotation"));
    let (seg_lo, seg_hi, pad) = (start.min(end), start.max(end), extent.max_element());
    for b in crate::touch::along(vm, start, end, pad) {
        if b == source || Some(b) == own_pawn || crate::touch::clear_of_trace(vm, b, seg_lo, seg_hi, pad) || vm.db.objects.get(b).deleted {
            continue;
        }
        if !crate::read(vm, b, collide).as_bool() || !crate::read(vm, b, block_flag).as_bool() || crate::read(vm, b, owner).as_obj() == Some(source) {
            continue;
        }
        let Some(have) = vm.db.objects.get(b).class else { continue };
        if filter.class.is_some_and(|want| !vm.is_child_of(have, want)) || volume.is_some_and(|v| vm.is_child_of(have, v)) {
            continue;
        }
        if filter.projectile_targets && !crate::read(vm, b, proj_target).as_bool() && !crate::read(vm, b, block_actors).as_bool() {
            continue;
        }
        let hit = if let Some(m) = world(vm).collision.mover(b) {
            if filter.movers { m.hit(start, end, extent) } else { None }
        } else if crate::read(vm, b, static_mesh).as_obj().is_some() && crate::read(vm, b, world_geometry).as_bool() {
            None
        } else {
            let (r, h) = (crate::read(vm, b, radius).as_float(), crate::read(vm, b, height).as_float());
            let center = crate::read_vec(vm, b, location);
            // A `CollideType` box (the big ambient sound triggers over the grounds, the walls along stairs) is met as its turned
            // box: a line starting inside it only hits when it goes deeper. As a cylinder of its long radius, every trace from
            // a camera inside one heading towards its middle stopped at once, so the spell cursor could not lock on there.
            if crate::read(vm, b, collide_type).as_byte() == 1 {
                let width = crate::read(vm, b, collision_width).as_float();
                let rot: Vec<i32> = crate::read(vm, b, rotation).fields().iter().map(Value::as_int).collect();
                let ax = hp3_script::natives::rot_axes([rot.first().copied().unwrap_or(0), rot.get(1).copied().unwrap_or(0), rot.get(2).copied().unwrap_or(0)]);
                let axes = [glam::Vec3::from(ax[0]), glam::Vec3::from(ax[1]), glam::Vec3::from(ax[2])];
                let half = glam::Vec3::new(r, if width > 0.0 { width } else { r }, h);
                crate::collision::turned_box_trace(b, center, half, axes, start, end, extent)
            } else {
                crate::collision::cylinder_trace(center, r, h, start, end, extent)
            }
        };
        if let Some(mut h) = hit {
            h.actor = Some(b);
            hits.push(h);
        }
    }
    hits
}

/// Debugging: what `TraceActors` from `source` would meet between two points (actors, then the level), nearest first.
pub fn debug_trace_actors(vm: &mut Vm<'_>, source: ObjId, start: glam::Vec3, end: glam::Vec3, extent: glam::Vec3) -> Vec<(String, f32, glam::Vec3)> {
    let filter = LineFilter { class: None, projectile_targets: false, movers: true };
    let mut hits = line_hits(vm, source, start, end, extent, &filter);
    let ghost = crate::collision::Mover { player: false, blocks_actors: false, collide_world: true };
    if let Some(h) = world(vm).collision.trace_for(ghost, start, end, extent, None) {
        hits.push(h);
    }
    hits.sort_by(|x, y| x.time.total_cmp(&y.time));
    hits.into_iter().map(|h| (h.actor.map_or("level".to_string(), |a| hp3_script::natives::object_path(vm, a)), h.time, h.location)).collect()
}

/// Creates an actor of `class`: defaults, owner, tag, location, rotation, then the begin-play events.
pub fn spawn(vm: &mut Vm<'_>, spawner: Option<ObjId>, a: &[Value]) -> Result<Value, VmError> {
    let Some(class_obj) = obj_arg(a, 0) else { return Ok(Value::Object(None)) };
    let Some(class) = vm.class_of(class_obj) else { return Ok(Value::Object(None)) };
    let actor_class = world(vm).actor_class;
    if !vm.is_child_of(class, actor_class) {
        return Ok(Value::Object(None));
    }
    let n = world(vm).spawned;
    world(vm).spawned += 1;
    let class_name = vm.db.get(class).name;
    let name = format!("{}{}", vm.db.names.get(class_name), 1000 + n);
    let name = vm.db.names.intern(&name);
    let level = world(vm).level_info;
    let obj = vm.new_object_in(class, name, level);
    let (loc, rot) = match spawner {
        Some(s) => (get(vm, s, "Location"), get(vm, s, "Rotation")),
        None => (Value::Empty, Value::Empty),
    };
    set(vm, obj, "Location", if given(a, 3) { a[3].clone() } else { loc });
    set(vm, obj, "Rotation", if given(a, 4) { a[4].clone() } else { rot });
    set(vm, obj, "Owner", Value::Object(obj_arg(a, 1)));
    // A tag that is none (a script's `FancySpawn` passes its own optional tag on) means the class's name, as when it is left out:
    // cutscenes find the dragon and the rabbit by that tag.
    let tag = if given(a, 2) { a[2].as_name() } else { hp3_script::Name::NONE };
    set(vm, obj, "Tag", Value::Name(if tag == hp3_script::Name::NONE { class_name } else { tag }));
    set(vm, obj, "Level", Value::Object(level));
    let instigator = spawner.map(|s| get(vm, s, "Instigator")).unwrap_or_default();
    set(vm, obj, "Instigator", instigator);
    // An actor that collides with the world is fitted to a free place near where it was asked for, as the original does when
    // it spawns one: a creature born with its feet in the floor is lifted out of it, not left to fall through.
    if get(vm, obj, "bCollideWorld").as_bool() {
        let at = crate::physics::vec(&get(vm, obj, "Location"));
        crate::physics::far_move(vm, obj, at, false, false);
    }
    // Before the game and its mutators exist (the game spawns its managers in its own PreBeginPlay) there is no one to ask whether
    // an actor is relevant, and `Actor.PreBeginPlay` would take the missing answer for "no" and destroy it: the vendor, wizard card
    // and bump line managers died at birth, and trading never ran. Such an actor is taken as game-relevant, which skips the question.
    let game_ready = level.and_then(|li| get(vm, li, "Game").as_obj()).is_some_and(|g| get(vm, g, "BaseMutator").as_obj().is_some());
    if !game_ready {
        set(vm, obj, "bGameRelevant", Value::Bool(true));
    }
    world(vm).actors.push(obj);
    {
        let class_text = vm.db.names.get(class_name).to_string();
        if class_text.contains("CombatManager") || class_text.starts_with("MonsterBook") {
            let who = spawner.map(|s| hp3_script::natives::object_path(vm, s)).unwrap_or_default();
            vm.log.push(format!("spawned {class_text} as {} by {who}", vm.db.names.get(name)));
        }
    }
    if vm.db.find("Emitter").is_some_and(|e| vm.is_child_of(class, e)) {
        crate::particles::own_emitters(vm, obj);
    }
    if let Some(owner) = obj_arg(a, 1) {
        fire(vm, owner, "GainedChild", &mut [Value::Object(Some(obj))]);
    }
    for e in ["Spawned", "PreBeginPlay", "BeginPlay", "PostBeginPlay", "AddAnimNotifys", "SetInitialState"] {
        if e == "AddAnimNotifys" && (vm.db.objects.get(obj).deleted || get(vm, obj, "Mesh").as_obj().is_none()) {
            continue;
        }
        fire(vm, obj, e, &mut []);
    }
    // Born inside something: it touches it at once.
    crate::touch::settle(vm, obj);
    Ok(Value::Object(Some(obj)))
}

/// Actors bucketed into vertical columns of `GRID_CELL` units by where they were when it was built.
use hp3_script::fx::FxHashMap as HashMap;

pub struct Grid {
    frame: u64,
    actors: usize,
    cells: HashMap<(i32, i32), Vec<ObjId>>,
}

const GRID_CELL: f32 = 1024.0;
/// How far an actor may have moved since the grid was built and still be found.
const GRID_SLACK: f32 = 512.0;

fn location_slot(vm: &mut Vm<'_>, a: ObjId) -> Option<usize> {
    crate::slot(vm, a, "Location")
}

fn location_at(vm: &Vm<'_>, a: ObjId, slot: usize) -> Option<glam::Vec3> {
    match vm.db.objects.values(a)?.get(slot)? {
        Value::Struct(f) if f.len() >= 3 => Some(glam::Vec3::new(f[0].as_float(), f[1].as_float(), f[2].as_float())),
        _ => None,
    }
}

/// Actors that may be within `radius` of `center` (a superset; callers check the distance).
fn actors_near(vm: &mut Vm<'_>, center: glam::Vec3, radius: f32, loc_slot: usize) -> Vec<ObjId> {
    let (frame, count) = {
        let w = world(vm);
        (w.frame, w.actors.len())
    };
    let stale = world(vm).grid.as_ref().is_none_or(|g| g.frame != frame || g.actors != count);
    if stale {
        let actors = world(vm).actors.clone();
        let mut cells: HashMap<(i32, i32), Vec<ObjId>> = HashMap::default();
        for a in actors {
            if let Some(p) = location_at(vm, a, loc_slot) {
                cells.entry(((p.x / GRID_CELL).floor() as i32, (p.y / GRID_CELL).floor() as i32)).or_default().push(a);
            }
        }
        world(vm).grid = Some(Grid { frame, actors: count, cells });
    }
    let r = radius + GRID_SLACK;
    let (x0, x1) = (((center.x - r) / GRID_CELL).floor() as i32, ((center.x + r) / GRID_CELL).floor() as i32);
    let (y0, y1) = (((center.y - r) / GRID_CELL).floor() as i32, ((center.y + r) / GRID_CELL).floor() as i32);
    let g = world(vm).grid.as_ref().expect("grid built above");
    let mut out = Vec::new();
    // Very large radii cover most cells: walk the cells that exist instead.
    if ((x1 - x0 + 1) as i64) * ((y1 - y0 + 1) as i64) > g.cells.len() as i64 {
        for ((cx, cy), v) in &g.cells {
            if (x0..=x1).contains(cx) && (y0..=y1).contains(cy) {
                out.extend_from_slice(v);
            }
        }
    } else {
        for cx in x0..=x1 {
            for cy in y0..=y1 {
                if let Some(v) = g.cells.get(&(cx, cy)) {
                    out.extend_from_slice(v);
                }
            }
        }
    }
    out
}

fn actors_of(vm: &mut Vm<'_>, class: Option<ObjId>, tag: Name) -> Vec<ObjId> {
    let class = class.and_then(|c| vm.class_of(c));
    let actors = world(vm).actors.clone();
    let mut out = Vec::new();
    for a in actors {
        let e = vm.db.objects.get(a);
        if e.deleted {
            continue;
        }
        if let (Some(want), Some(have)) = (class, e.class)
            && !vm.is_child_of(have, want)
        {
            continue;
        }
        if tag != Name::NONE && get(vm, a, "Tag").as_name() != tag {
            continue;
        }
        out.push(a);
    }
    out
}

fn link(vm: &mut Vm<'_>, this: Option<ObjId>, list: &str, next: &str) {
    let (Some(o), Some(li)) = (this, world(vm).level_info) else { return };
    let head = get(vm, li, list);
    set(vm, o, next, head);
    set(vm, li, list, Value::Object(Some(o)));
}

fn unlink(vm: &mut Vm<'_>, this: Option<ObjId>, list: &str, next: &str) {
    let (Some(o), Some(li)) = (this, world(vm).level_info) else { return };
    let after = get(vm, o, next);
    if get(vm, li, list).as_obj() == Some(o) {
        set(vm, li, list, after);
        return;
    }
    let mut cur = get(vm, li, list).as_obj();
    let mut guard = 0;
    while let Some(c) = cur {
        guard += 1;
        if guard > 100_000 {
            return;
        }
        let n = get(vm, c, next).as_obj();
        if n == Some(o) {
            set(vm, c, next, after);
            return;
        }
        cur = n;
    }
}

pub fn register(vm: &mut Vm<'_>) {
    vm.lenient = true;
    vm.register("Actor.Spawn", |vm, this, a| spawn(vm, this, a));
    vm.register("Actor.Destroy", |vm, this, _a| {
        let Some(o) = this else { return Ok(Value::Bool(false)) };
        Ok(Value::Bool(crate::destroy_actor(vm, o)))
    });
    vm.register_iterator("Actor.AllActors", |vm, _this, a| {
        let tag = a.get(2).map(Value::as_name).unwrap_or(Name::NONE);
        let found = actors_of(vm, obj_arg(a, 0), tag);
        Ok(found.into_iter().map(|x| vec![a[0].clone(), Value::Object(Some(x)), Value::Name(tag)]).collect())
    });
    vm.register_iterator("Actor.DynamicActors", |vm, _this, a| {
        let tag = a.get(2).map(Value::as_name).unwrap_or(Name::NONE);
        let found: Vec<ObjId> = actors_of(vm, obj_arg(a, 0), tag).into_iter().filter(|&x| !get(vm, x, "bStatic").as_bool()).collect();
        Ok(found.into_iter().map(|x| vec![a[0].clone(), Value::Object(Some(x)), Value::Name(tag)]).collect())
    });
    vm.register_iterator("Actor.ChildActors", |vm, this, a| {
        let found: Vec<ObjId> = actors_of(vm, obj_arg(a, 0), Name::NONE).into_iter().filter(|&x| get(vm, x, "Owner").as_obj() == this).collect();
        Ok(found.into_iter().map(|x| vec![a[0].clone(), Value::Object(Some(x))]).collect())
    });
    vm.register("Actor.SetTimer", |vm, this, a| {
        let Some(o) = this else { return Ok(Value::Empty) };
        let rate = a.first().map(Value::as_float).unwrap_or(0.0);
        let looping = a.get(1).is_some_and(Value::as_bool);
        {
            let class_text = vm.db.objects.get(o).class.map(|c| vm.db.names.get(vm.db.get(c).name).to_string()).unwrap_or_default();
            if class_text.contains("CombatManager") || class_text.starts_with("MonsterBook") {
                vm.log.push(format!("SetTimer {rate} looping {looping} on {class_text}"));
            }
        }
        let w = world(vm);
        if rate <= 0.0 {
            w.timers.remove(&o);
        } else {
            w.timers.insert(o, Timer { rate, remaining: rate, looping });
        }
        Ok(Value::Empty)
    });
    vm.register("Actor.SetLocation", |vm, this, a| {
        let Some(o) = this else { return Ok(Value::Bool(false)) };
        let to = crate::physics::vec(&a[0]);
        Ok(Value::Bool(crate::physics::far_move(vm, o, to, false, false)))
    });
    // A teleport: where the actor ends up is what touches, with nothing swept on the way.
    vm.register("Actor.FarMoveActor", |vm, this, a| {
        let Some(o) = this else { return Ok(Value::Bool(false)) };
        let to = crate::physics::vec(&a[0]);
        let flag = |i: usize| a.get(i).is_some_and(Value::as_bool);
        // (Dest, optional test, optional no-check, optional attached move)
        if flag(2) {
            if flag(1) {
                return Ok(Value::Bool(true));
            }
            set(vm, o, "Location", a[0].clone());
            crate::touch::settle(vm, o);
            return Ok(Value::Bool(true));
        }
        Ok(Value::Bool(crate::physics::far_move(vm, o, to, flag(1), flag(3))))
    });
    vm.register("Actor.SetRotation", |vm, this, a| {
        if let Some(o) = this {
            set(vm, o, "Rotation", a[0].clone());
        }
        Ok(Value::Bool(true))
    });
    // A change of physics: modes that do not rest on something (falling, flying, projectile, trailer, movers...)
    // let go of the actor's base, and a stopped or only rotating actor loses its velocity. The movement-animation
    // driver notices a change to falling and tells the script.
    vm.register("Actor.SetPhysics", |vm, this, a| {
        if let Some(o) = this {
            let new = a[0].as_byte();
            if get(vm, o, "Physics").as_byte() != new {
                if get(vm, o, "AttachTag").as_name() != Name::NONE {
                    let who = hp3_script::natives::object_path(vm, o);
                    let old_physics = get(vm, o, "Physics").as_byte();
                    vm.log.push(format!("physics: {who} {old_physics} -> {new}"));
                }
                set(vm, o, "Physics", Value::Byte(new));
                // A pickup thrown by a vendor (a student's wizard card, `ThrownPickup`) falls onto the floor. The pickup that spawned it had
                // switched its world collision off when it left its bounce-into-place state, so without this a card that missed the player fell
                // through the floor for good and the sale was lost.
                if new == 2 && vm.state_of(o).is_some_and(|s| vm.db.names.get(vm.db.get(s).name) == "ThrownPickup") {
                    set(vm, o, "bCollideWorld", Value::Bool(true));
                }
                if matches!(new, 2 | 3 | 4 | 6 | 7 | 8 | 10 | 11 | 13 | 14) && get(vm, o, "Base").as_obj().is_some() {
                    set_base(vm, o, None, false);
                }
                if matches!(new, 0 | 5) {
                    set(vm, o, "Velocity", crate::physics::value(glam::Vec3::ZERO));
                    set(vm, o, "Acceleration", crate::physics::value(glam::Vec3::ZERO));
                }
            }
        }
        Ok(Value::Empty)
    });
    vm.register("Actor.SetCollision", |vm, this, a| {
        if let Some(o) = this {
            for (i, p) in ["bCollideActors", "bBlockActors", "bBlockPlayers"].iter().enumerate() {
                if given(a, i) {
                    set(vm, o, p, Value::Bool(a[i].as_bool()));
                }
            }
            crate::refresh_baked_collision(vm, o);
            crate::touch::settle(vm, o);
        }
        Ok(Value::Empty)
    });
    vm.register("Actor.SetCollisionSize", |vm, this, a| {
        if let Some(o) = this {
            set(vm, o, "CollisionRadius", Value::Float(a[0].as_float()));
            set(vm, o, "CollisionHeight", Value::Float(a[1].as_float()));
            crate::touch::settle(vm, o);
        }
        Ok(Value::Bool(true))
    });
    vm.register("Actor.SetDrawScale", |vm, this, a| {
        if let Some(o) = this {
            set(vm, o, "DrawScale", Value::Float(a[0].as_float()));
        }
        Ok(Value::Empty)
    });
    vm.register("Actor.SetDrawType", |vm, this, a| {
        if let Some(o) = this {
            set(vm, o, "DrawType", Value::Byte(a[0].as_byte()));
        }
        Ok(Value::Empty)
    });
    // SetStaticMesh(mesh): the actor shows another static mesh (the debris generator picks one for each piece).
    vm.register("Actor.SetStaticMesh", |vm, this, a| {
        if let Some(o) = this {
            set(vm, o, "StaticMesh", Value::Object(obj_arg(a, 0)));
        }
        Ok(Value::Empty)
    });
    vm.register("Actor.SetOwner", |vm, this, a| {
        if let Some(o) = this {
            set(vm, o, "Owner", Value::Object(obj_arg(a, 0)));
        }
        Ok(Value::Empty)
    });
    vm.register("Actor.SetBase", |vm, this, a| {
        if let Some(o) = this {
            let notify = given(a, 2) && a[2].as_bool();
            set_base(vm, o, obj_arg(a, 0), notify);
        }
        Ok(Value::Empty)
    });
    // Sound and music: silent until the audio crate exists.
    // Animation natives live in anim.rs.
    vm.register("Actor.FinishInterpolation", |vm, this, _a| {
        // Waits until the mover has finished its move (`bInterpolating` goes off); returns at once when none runs.
        if let Some(o) = this {
            if get(vm, o, "bInterpolating").as_bool() {
                vm.latent.insert(o, Latent::Other("interpolation"));
            } else {
                vm.latent.insert(o, Latent::Sleep(0.0));
            }
        }
        Ok(Value::Empty)
    });
    // Level-wide linked lists of pawns and controllers.
    vm.register("Pawn.AddPawn", |vm, this, _a| {
        link(vm, this, "PawnList", "nextPawn");
        Ok(Value::Empty)
    });
    vm.register("Pawn.RemovePawn", |vm, this, _a| {
        unlink(vm, this, "PawnList", "nextPawn");
        Ok(Value::Empty)
    });
    vm.register("Pawn.AddSpecialPawn", |vm, this, _a| {
        link(vm, this, "SpecialPawnList", "nextSpecialPawn");
        Ok(Value::Empty)
    });
    vm.register("Pawn.RemoveSpecialPawn", |vm, this, _a| {
        unlink(vm, this, "SpecialPawnList", "nextSpecialPawn");
        Ok(Value::Empty)
    });
    vm.register("Controller.AddController", |vm, this, _a| {
        link(vm, this, "ControllerList", "nextController");
        Ok(Value::Empty)
    });
    vm.register("Controller.RemoveController", |vm, this, _a| {
        unlink(vm, this, "ControllerList", "nextController");
        Ok(Value::Empty)
    });
    vm.register("Actor.OnlyAffectPawns", |vm, this, a| {
        if let Some(o) = this {
            set(vm, o, "bOnlyAffectPawns", Value::Bool(a[0].as_bool()));
        }
        Ok(Value::Empty)
    });
    // `Move`: straight by the delta. An actor that collides with the world is stopped by it (the result says
    // whether it got all the way); every other actor goes through. `MoveSmooth` slides along what stops it.
    vm.register("Actor.Move", |vm, this, a| {
        let mut moved_fully = true;
        if let Some(o) = this {
            let delta = crate::physics::vec(&a[0]);
            if get(vm, o, "bCollideWorld").as_bool() {
                moved_fully = crate::physics::move_actor(vm, o, delta).is_none();
            } else {
                let loc = crate::physics::vec(&get(vm, o, "Location"));
                set(vm, o, "Location", crate::physics::value(loc + delta));
            }
            crate::touch::settle(vm, o);
        }
        Ok(Value::Bool(moved_fully))
    });
    vm.register("Actor.MoveSmooth", |vm, this, a| {
        let mut moved = true;
        if let Some(o) = this {
            let delta = crate::physics::vec(&a[0]);
            let before = crate::physics::vec(&get(vm, o, "Location"));
            if get(vm, o, "bCollideWorld").as_bool() {
                crate::physics::move_and_slide(vm, o, delta);
            } else {
                set(vm, o, "Location", crate::physics::value(before + delta));
            }
            moved = crate::physics::vec(&get(vm, o, "Location")).distance_squared(before) > 1e-6 || delta.length_squared() < 1e-8;
            crate::touch::settle(vm, o);
        }
        Ok(Value::Bool(moved))
    });
    // SetRelativeLocation(vector): on a bone the offset is only stored; on a base the actor moves to that offset in
    // the base's frame (a spell placed at a creature's mouth) and keeps its offset from the base; otherwise nothing.
    vm.register("Actor.SetRelativeLocation", |vm, this, a| {
        let Some(o) = this else { return Ok(Value::Bool(false)) };
        if get(vm, o, "AttachmentBone").as_name() != hp3_script::Name::NONE {
            set(vm, o, "RelativeLocation", a[0].clone());
            return Ok(Value::Bool(true));
        }
        let Some(base) = get(vm, o, "Base").as_obj() else { return Ok(Value::Bool(false)) };
        let offset = crate::physics::vec(&a[0]);
        let base_at = crate::physics::vec(&get(vm, base, "Location"));
        let r: Vec<i32> = get(vm, base, "Rotation").fields().iter().map(Value::as_int).collect();
        let axes = hp3_script::natives::rot_axes([r.first().copied().unwrap_or(0), r.get(1).copied().unwrap_or(0), r.get(2).copied().unwrap_or(0)]);
        let to = base_at + glam::Vec3::from(axes[0]) * offset.x + glam::Vec3::from(axes[1]) * offset.y + glam::Vec3::from(axes[2]) * offset.z;
        let moved = crate::physics::far_move(vm, o, to, false, true);
        if get(vm, o, "Base").as_obj().is_some() {
            let now = crate::physics::vec(&get(vm, o, "Location"));
            set(vm, o, "RelativeLocation", crate::physics::value(now - base_at));
        }
        Ok(Value::Bool(moved))
    });
    vm.register("Actor.SetRelativeRotation", |vm, this, a| {
        if let Some(o) = this {
            set(vm, o, "RelativeRotation", a[0].clone());
        }
        Ok(Value::Bool(true))
    });
    // SetSkinColor(optional byte R, G, B, A): tints the actor's skin (`SkinColorModifier`, a `Color` declared B, G, R, A, 128 being
    // no change in a channel); a channel left out keeps its value. The fade delegates call it every tick.
    vm.register("Actor.SetSkinColor", |vm, this, a| {
        let Some(o) = this else { return Ok(Value::Empty) };
        let now = get(vm, o, "SkinColorModifier");
        let old = |i: usize| now.fields().get(i).map_or(128, Value::as_int) as u8;
        let pick = |arg: usize, field: usize| match a.get(arg) {
            Some(Value::Empty) | None => old(field),
            Some(v) => v.as_byte(),
        };
        let (r, g, b, alpha) = (pick(0, 2), pick(1, 1), pick(2, 0), pick(3, 3));
        set(vm, o, "SkinColorModifier", Value::Struct(vec![Value::Byte(b), Value::Byte(g), Value::Byte(r), Value::Byte(alpha)].into()));
        set(vm, o, "bUseSkinColorMod", Value::Bool(true));
        Ok(Value::Empty)
    });
    vm.register("Actor.SetDrawScale3D", |vm, this, a| {
        if let Some(o) = this {
            set(vm, o, "DrawScale3D", a[0].clone());
        }
        Ok(Value::Empty)
    });
    vm.register("Actor.GetRenderBoundingSphere", |vm, this, _a| {
        let Some(o) = this else { return Ok(Value::Empty) };
        let loc = get(vm, o, "Location");
        let r = get(vm, o, "CollisionRadius").as_float();
        let mut f: Vec<Value> = loc.fields().to_vec();
        f.push(Value::Float(r));
        Ok(Value::Struct(f.into()))
    });
    vm.register("PlayerController.SetViewTarget", |vm, this, a| {
        if let Some(o) = this {
            set(vm, o, "ViewTarget", Value::Object(obj_arg(a, 0)));
        }
        Ok(Value::Empty)
    });
    // ConsoleCommand(Command): the commands saved games use are carried out; the answer is empty.
    for name in ["Actor.ConsoleCommand", "PlayerController.ConsoleCommand"] {
        vm.register(name, |vm, _this, a| {
            let command = a.first().map(|v| v.as_str().to_string()).unwrap_or_default();
            Ok(Value::Str(crate::save::console(vm, &command)))
        });
    }
    vm.register("LevelInfo.IsSoftwareRendering", |_vm, _this, _a| Ok(Value::Bool(false)));
    // Rendering-side effects with nothing to do headless.
    vm.register("Actor.StopFeedbackEffect", |_vm, _this, _a| Ok(Value::Empty));
    vm.register("Projector.AttachProjector", |_vm, _this, _a| Ok(Value::Empty));
    vm.register("Projector.DetachProjector", |_vm, _this, _a| Ok(Value::Empty));
    vm.register("SceneManager.GetTotalSceneTime", |_vm, _this, _a| Ok(Value::Float(0.0)));
    vm.register("Object.Localize", |vm, _this, a| {
        let (section, key, package) = (a[0].as_str().to_string(), a[1].as_str().to_string(), a[2].as_str().to_string());
        let found = world(vm).config.localize(&package, &section, &key);
        Ok(Value::Str(found.unwrap_or_else(|| format!("<?{}?{package}.{section}.{key}?>", world(vm).config.language))))
    });
    vm.register_iterator("Actor.TouchingActors", |vm, this, a| {
        let Some(o) = this else { return Ok(Vec::new()) };
        let touching = get(vm, o, "Touching");
        let class = obj_arg(a, 0).and_then(|c| vm.class_of(c));
        let mut out = Vec::new();
        let list: Vec<Value> = match touching {
            Value::Array(v) => v,
            other => vec![other],
        };
        for t in list.iter().filter_map(Value::as_obj) {
            let ok = match (class, vm.db.objects.get(t).class) {
                (Some(want), Some(have)) => vm.is_child_of(have, want),
                _ => true,
            };
            if ok && !vm.db.objects.get(t).deleted {
                out.push(vec![a[0].clone(), Value::Object(Some(t))]);
            }
        }
        Ok(out)
    });
    vm.register("Object.SetPropertyText", |vm, this, a| {
        if let Some(o) = this {
            let (prop, text) = (a[0].as_str().to_string(), a[1].as_str().to_string());
            let ty = vm.db.objects.get(o).class.and_then(|c| {
                let n = vm.db.names.find(&prop)?;
                vm.db.get(c).prop(n).map(|p| p.ty.clone())
            });
            if let Some(v) = ty.and_then(|ty| crate::config::parse(vm, &ty, &text)) {
                set(vm, o, &prop, v);
            }
        }
        Ok(Value::Empty)
    });
    vm.register("Object.GetPropertyText", |vm, this, a| {
        let Some(o) = this else { return Ok(Value::Str(String::new())) };
        let v = get(vm, o, a[0].as_str());
        Ok(Value::Str(match v {
            Value::Int(i) => i.to_string(),
            Value::Float(f) => hp3_script::natives::float_to_string(f),
            Value::Byte(b) => b.to_string(),
            Value::Bool(b) => if b { "True".into() } else { "False".into() },
            Value::Name(n) => vm.db.names.get(n).to_string(),
            Value::Str(s) => s,
            Value::Object(Some(x)) => hp3_script::natives::object_path(vm, x),
            _ => String::new(),
        }))
    });
    // Spatial iterators, each as in the reference:
    // - RadiusActors(Class, out A, Radius, optional Loc): every actor within Radius plus its own collision radius.
    // - VisibleActors(Class, out A, optional Radius, optional Loc): visible (not hidden) actors within Radius
    //   (0 is no limit) that a line from Loc reaches without meeting the world first.
    // - CollidingActors(Class, out A, Radius, optional Loc): the same as RadiusActors, for actors that collide.
    // - VisibleCollidingActors(Class, out A, Radius, optional Loc, optional bIgnoreHidden): colliding actors that a
    //   line from Loc reaches.
    vm.register_iterator("Actor.RadiusActors", |vm, this, a| spatial(vm, this, a, Spatial { visible: false, colliding: false }));
    vm.register_iterator("Actor.VisibleActors", |vm, this, a| spatial(vm, this, a, Spatial { visible: true, colliding: false }));
    vm.register_iterator("Actor.CollidingActors", |vm, this, a| spatial(vm, this, a, Spatial { visible: false, colliding: true }));
    vm.register_iterator("Actor.VisibleCollidingActors", |vm, this, a| spatial(vm, this, a, Spatial { visible: true, colliding: true }));
    vm.register_iterator("Actor.BasedActors", |vm, this, a| {
        let found: Vec<ObjId> = actors_of(vm, obj_arg(a, 0), Name::NONE).into_iter().filter(|&x| get(vm, x, "Base").as_obj() == this).collect();
        Ok(found.into_iter().map(|x| vec![a[0].clone(), Value::Object(Some(x))]).collect())
    });
    // HP3: a duplicate of this object.
    vm.register("Object.Copy", |vm, this, _a| {
        let Some(o) = this else { return Ok(Value::Object(None)) };
        let e = vm.db.objects.get(o).clone();
        let (Some(class), Some(values)) = (e.class, vm.db.objects.values(o).cloned()) else { return Ok(Value::Object(None)) };
        let copy = vm.db.objects.add(e.name, class, values);
        Ok(Value::Object(Some(copy)))
    });
    // Trace(out HitLocation, out HitNormal, TraceEnd, optional TraceStart, optional bTraceActors,
    //       optional Extent, optional out Material): the actor hit, Level for world geometry, or None.
    // Trace(out HitLoc, out HitNorm, End, optional Start, optional bTraceActors, optional Extent): the first thing
    // met. Without `bTraceActors` (which defaults to the tracer's own `bCollideActors`) only the world counts: the
    // level and the movers; with it, also actors that can be aimed at or that block. A controller traces as its pawn.
    vm.register("Actor.Trace", |vm, this, a| {
        let Some(o) = this else { return Ok(Value::Object(None)) };
        let end = crate::physics::vec(&a[2]);
        let start = if given(a, 3) { crate::physics::vec(&a[3]) } else { crate::physics::vec(&get(vm, o, "Location")) };
        let trace_actors = if given(a, 4) { a[4].as_bool() } else { get(vm, o, "bCollideActors").as_bool() };
        let extent = if given(a, 5) { crate::physics::vec(&a[5]) } else { glam::Vec3::ZERO };
        let source = get(vm, o, "Pawn").as_obj().unwrap_or(o);
        let mut best = world(vm).collision.trace_static(start, end, extent).filter(|h| h.actor != Some(source));
        if trace_actors {
            let filter = LineFilter { class: None, projectile_targets: true, movers: false };
            for h in line_hits(vm, source, start, end, extent, &filter) {
                if best.is_none_or(|b| h.time < b.time) {
                    best = Some(h);
                }
            }
        }
        let level = world(vm).level_info;
        if crate::physics::is_player_pawn(vm, source) && (end - start).truncate().length_squared() < 1.0 && (start.z - end.z) > 50.0 {
            let who = best.and_then(|h| h.actor).map(|o| hp3_script::natives::object_path(vm, o));
            vm.log.push(format!("trace down from {start} (actors {trace_actors}, extent {extent}): hit {:?} at {:?} normal {:?}", who, best.map(|h| h.location), best.map(|h| h.normal)));
        }
        match best {
            Some(h) => {
                a[0] = crate::physics::value(h.location);
                a[1] = crate::physics::value(h.normal);
                Ok(Value::Object(h.actor.or(level)))
            }
            None => {
                a[0] = crate::physics::value(end);
                a[1] = crate::physics::value(glam::Vec3::ZERO);
                Ok(Value::Object(None))
            }
        }
    });
    vm.register("Actor.FastTrace", |vm, this, a| {
        let Some(o) = this else { return Ok(Value::Bool(true)) };
        let end = crate::physics::vec(&a[0]);
        let start = if given(a, 1) { crate::physics::vec(&a[1]) } else { crate::physics::vec(&get(vm, o, "Location")) };
        Ok(Value::Bool(world(vm).collision.trace_static(start, end, glam::Vec3::ZERO).is_none_or(|h| h.actor == Some(o))))
    });
    // HPCharacter.SwitchToFightStanceAnims, the script's steps written out, with one difference: a character lying collapsed
    // (the dementor encounter's `DementorEncounterCollapse` state) is left as he is, and only the characters trailing him
    // are switched. The encounter starts while Harry, handed back to the hero controller at the end of his cutscene part,
    // is still on the ground; the engine's movement animation would stand him up in the new idle once he is given to the
    // companion controller, and he stayed down in the original. Called again later, he is switched then.
    vm.register_override("HPCharacter.SwitchToFightStanceAnims", |vm, this, _a| {
        let Some(o) = this else { return Ok(Value::Empty) };
        if get(vm, o, "bUsingFightStanceAnims").as_bool() {
            return Ok(Value::Empty);
        }
        let collapsed = vm.state_of(o).is_some_and(|s| vm.db.names.get(vm.db.get(s).name).eq_ignore_ascii_case("DementorEncounterCollapse"));
        if !collapsed {
            set(vm, o, "bUsingFightStanceAnims", Value::Bool(true));
            for (from, to) in [("FightStanceCastHoldAnim", "castHoldAnim"), ("FightStanceCastReleaseAnim", "castReleaseAnim")] {
                let v = get(vm, o, from);
                if v.as_name() != Name::NONE {
                    set(vm, o, to, v);
                }
            }
            for (from, to) in [("FightStanceTurnLeftAnim", "AlternateTurnLeftAnim"), ("FightStanceTurnRightAnim", "AlternateTurnRightAnim")] {
                let v = get(vm, o, from);
                set(vm, o, to, v);
            }
            let stance = get(vm, o, "FightStanceAnim");
            if stance.as_name() != Name::NONE {
                set(vm, o, "IdleAnimName", stance);
            }
            fire(vm, o, "ChangeAnimation", &mut []);
            fire(vm, o, "AnimateStanding", &mut []);
        }
        for i in 0..2 {
            if let Some(c) = crate::anim::get_indexed(vm, o, "TrailingChar", i).as_obj() {
                fire(vm, c, "SwitchToFightStanceAnims", &mut []);
            }
        }
        Ok(Value::Empty)
    });
    // KWAIController.FindClosestNavPoint, written out natively (the companions call it every tick and the script loop,
    // a trace per nav point, was the biggest cost of a busy hub). The same steps as the script: the nav point nearest
    // the pawn within 1500 (twice the distance for those behind the way to the hero) is the nearest one, but one that
    // a line from the pawn to it (a box of 0.3 of the pawn's size, level geometry only) reaches is preferred.
    if crate::envc::var_os("HP3_NO_NAV_OVERRIDE").is_none() {
    vm.register_override("KWAIController.FindClosestNavPoint", |vm, this, _a| {
        let Some(c) = this else { return Ok(Value::Object(None)) };
        let Some(pawn) = get(vm, c, "Pawn").as_obj() else { return Ok(Value::Object(None)) };
        let Some(class) = vm.db.find("NavigationPoint") else { return Ok(Value::Object(None)) };
        let at = |vm: &mut Vm<'_>, o: ObjId| crate::physics::vec(&get(vm, o, "Location"));
        let from = at(vm, pawn);
        let hero = world(vm).level_info.and_then(|li| get(vm, li, "PlayerHeroActor").as_obj());
        let to_hero = hero.map_or(glam::Vec3::ZERO, |h| at(vm, h) - from).normalize_or_zero();
        let (radius, height) = (get(vm, pawn, "CollisionRadius").as_float(), get(vm, pawn, "CollisionHeight").as_float());
        let extent = glam::Vec3::new(radius * 0.3, radius * 0.3, height * 0.3);
        let mut closest = 1500.0f32;
        let mut closest_clear = closest;
        let (mut best, mut best_clear): (Option<ObjId>, Option<ObjId>) = (None, None);
        for a in world(vm).actors.clone() {
            let e = vm.db.objects.get(a);
            if e.deleted || !e.class.is_some_and(|k| vm.is_child_of(k, class)) {
                continue;
            }
            let p = at(vm, a);
            let mut d = (p - from).length();
            if (p - from).dot(to_hero) < 0.0 {
                d *= 2.0;
            }
            if d < closest {
                closest = d;
                best = Some(a);
            }
            if d < closest_clear {
                let hit = world(vm).collision.trace_static(from, p, extent).filter(|h| h.actor != Some(pawn));
                if hit.is_none_or(|h| h.actor == Some(a)) {
                    closest_clear = d;
                    best_clear = Some(a);
                }
            }
        }
        Ok(Value::Object(best_clear.or(best)))
    });
    }
    // TraceActors(BaseClass, out Actor, out HitLoc, out HitNorm, End, optional Start, optional Extent): every
    // colliding actor the swept segment crosses, nearest first, and the first thing of the level's geometry
    // (reported as the level itself for BSP and terrain). A trace without extent only meets actors that block
    // zero-extent traces, one with extent those that block the other kind; movers are met by their geometry,
    // volumes not at all, and what the tracer owns (or its pawn) is skipped.
    vm.register_iterator("Actor.TraceActors", |vm, this, a| {
        use crate::collision::Mover;
        let Some(o) = this else { return Ok(Vec::new()) };
        let end = crate::physics::vec(&a[4]);
        let start = if given(a, 5) { crate::physics::vec(&a[5]) } else { crate::physics::vec(&get(vm, o, "Location")) };
        let extent = if given(a, 6) { crate::physics::vec(&a[6]) } else { glam::Vec3::ZERO };
        let class = obj_arg(a, 0).and_then(|c| vm.class_of(c));
        let filter = LineFilter { class, projectile_targets: false, movers: true };
        let mut hits = line_hits(vm, o, start, end, extent, &filter);
        let level = world(vm).level_info;
        let ghost = Mover { player: false, blocks_actors: false, collide_world: true };
        if let Some(mut h) = world(vm).collision.trace_for(ghost, start, end, extent, None) {
            h.actor = h.actor.or(level);
            let wanted = match (class, h.actor.and_then(|x| vm.db.objects.get(x).class)) {
                (Some(want), Some(have)) => vm.is_child_of(have, want),
                _ => true,
            };
            if wanted {
                hits.push(h);
            }
        }
        hits.sort_by(|x, y| x.time.total_cmp(&y.time));
        Ok(hits
            .into_iter()
            .map(|h| {
                let mut item = a.to_vec();
                item[1] = Value::Object(h.actor);
                item[2] = crate::physics::value(h.location);
                item[3] = crate::physics::value(h.normal);
                item
            })
            .collect())
    });
    // Encompasses(Actor Other): whether Other's location is inside this volume's brush.
    vm.register("Volume.Encompasses", |vm, this, a| {
        let (Some(v), Some(other)) = (this, obj_arg(a, 0)) else { return Ok(Value::Bool(false)) };
        let Some(brush) = get(vm, v, "Brush").as_obj() else { return Ok(Value::Bool(false)) };
        let Some((pkg, index)) = vm.db.objects.get(brush).key.clone() else { return Ok(Value::Bool(false)) };
        let Ok(p) = vm.db.set.get(&pkg) else { return Ok(Value::Bool(false)) };
        let chain = vm.db.set.export_chain(&p, index);
        let Some(hp3_assets::Asset::Model(model)) = crate::native_asset(&p, index, &chain) else { return Ok(Value::Bool(false)) };
        // Brush space to world: translate(Location) * rotate * scale * translate(-PrePivot).
        let loc = crate::physics::vec(&get(vm, v, "Location"));
        let prepivot = crate::physics::vec(&get(vm, v, "PrePivot"));
        let r: Vec<i32> = get(vm, v, "Rotation").fields().iter().map(Value::as_int).collect();
        let rot = hp3_anim::rotator_matrix([r.first().copied().unwrap_or(0), r.get(1).copied().unwrap_or(0), r.get(2).copied().unwrap_or(0)]);
        let scale = match get(vm, v, "DrawScale").as_float() {
            0.0 => 1.0,
            s => s,
        };
        let to_world = glam::Mat4::from_translation(loc) * rot * glam::Mat4::from_scale(glam::Vec3::splat(scale)) * glam::Mat4::from_translation(-prepivot);
        let at = to_world.inverse().transform_point3(crate::physics::vec(&get(vm, other, "Location")));
        Ok(Value::Bool(crate::collision::point_inside(&model, at)))
    });
    // SetOpacity(float): kept for the renderer.
    vm.register("Actor.SetOpacity", |vm, this, a| {
        if let Some(o) = this {
            let alpha = a.first().map(Value::as_float).unwrap_or(1.0);
            world(vm).opacity.insert(o, alpha);
        }
        Ok(Value::Empty)
    });
    // AI hearing: no game script listens (no HearNoise handler anywhere), so a noise is not heard.
    vm.register("Actor.MakeNoise", |_vm, _this, _a| Ok(Value::Empty));
    vm.register("LevelInfo.IsDemoBuild", |_vm, _this, _a| Ok(Value::Bool(false)));
    vm.register("Object.DynamicLoadObject", |vm, _this, a| {
        let path = a[0].as_str().to_string();
        // The class argument is a class object; its name picks among objects sharing the path.
        let class = a.get(1).and_then(Value::as_obj).and_then(|c| match vm.db.objects.get(c).kind {
            hp3_script::objects::ObjKind::Struct(s) => Some(vm.db.names.get(vm.db.structs[s.0 as usize].name).to_string()),
            _ => None,
        });
        let found = crate::find_object_of(vm, &path, class.as_deref());
        if found.is_none() && !a.get(2).is_some_and(Value::as_bool) {
            vm.warn(format!("DynamicLoadObject: {path} not found"));
        }
        Ok(Value::Object(found))
    });
}
