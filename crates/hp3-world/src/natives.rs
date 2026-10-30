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
    set(vm, obj, "Tag", Value::Name(if given(a, 2) { a[2].as_name() } else { class_name }));
    set(vm, obj, "Level", Value::Object(level));
    let instigator = spawner.map(|s| get(vm, s, "Instigator")).unwrap_or_default();
    set(vm, obj, "Instigator", instigator);
    world(vm).actors.push(obj);
    if let Some(owner) = obj_arg(a, 1) {
        fire(vm, owner, "GainedChild", &mut [Value::Object(Some(obj))]);
    }
    for e in ["Spawned", "PreBeginPlay", "BeginPlay", "PostBeginPlay", "SetInitialState"] {
        fire(vm, obj, e, &mut []);
    }
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
        let w = world(vm);
        if rate <= 0.0 {
            w.timers.remove(&o);
        } else {
            w.timers.insert(o, Timer { rate, remaining: rate, looping });
        }
        Ok(Value::Empty)
    });
    vm.register("Actor.SetLocation", |vm, this, a| {
        if let Some(o) = this {
            set(vm, o, "Location", a[0].clone());
        }
        Ok(Value::Bool(true))
    });
    vm.register("Actor.SetRotation", |vm, this, a| {
        if let Some(o) = this {
            set(vm, o, "Rotation", a[0].clone());
        }
        Ok(Value::Bool(true))
    });
    vm.register("Actor.SetPhysics", |vm, this, a| {
        if let Some(o) = this {
            // The movement-animation driver notices a change to falling and tells the script.
            set(vm, o, "Physics", Value::Byte(a[0].as_byte()));
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
        }
        Ok(Value::Empty)
    });
    vm.register("Actor.SetCollisionSize", |vm, this, a| {
        if let Some(o) = this {
            set(vm, o, "CollisionRadius", Value::Float(a[0].as_float()));
            set(vm, o, "CollisionHeight", Value::Float(a[1].as_float()));
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
    vm.register("Actor.SetOwner", |vm, this, a| {
        if let Some(o) = this {
            set(vm, o, "Owner", Value::Object(obj_arg(a, 0)));
        }
        Ok(Value::Empty)
    });
    vm.register("Actor.SetBase", |vm, this, a| {
        if let Some(o) = this {
            set(vm, o, "Base", Value::Object(obj_arg(a, 0)));
        }
        Ok(Value::Empty)
    });
    // Sound and music: silent until the audio crate exists.
    // Animation natives live in anim.rs.
    vm.register("Actor.FinishInterpolation", |vm, this, _a| {
        if let Some(o) = this {
            vm.latent.insert(o, Latent::Sleep(0.0));
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
    // Moves without collision until the collision code exists.
    vm.register("Actor.Move", |vm, this, a| {
        if let Some(o) = this {
            let loc = get(vm, o, "Location");
            let (l, d) = (loc.fields(), a[0].fields());
            if l.len() == 3 && d.len() == 3 {
                let v: Vec<Value> = (0..3).map(|i| Value::Float(l[i].as_float() + d[i].as_float())).collect();
                set(vm, o, "Location", Value::Struct(v.into()));
            }
        }
        Ok(Value::Bool(true))
    });
    vm.register("Actor.SetRelativeLocation", |vm, this, a| {
        if let Some(o) = this {
            set(vm, o, "RelativeLocation", a[0].clone());
        }
        Ok(Value::Bool(true))
    });
    vm.register("Actor.SetRelativeRotation", |vm, this, a| {
        if let Some(o) = this {
            set(vm, o, "RelativeRotation", a[0].clone());
        }
        Ok(Value::Bool(true))
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
    vm.register("Actor.ConsoleCommand", |_vm, _this, _a| Ok(Value::Str(String::new())));
    vm.register("PlayerController.ConsoleCommand", |_vm, _this, _a| Ok(Value::Str(String::new())));
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
    // Spatial iterators: (BaseClass, out Actor, Radius, optional Loc). Visibility needs traces, which do not
    // exist yet, so the visible variants match everything in range.
    for name in ["Actor.RadiusActors", "Actor.VisibleActors", "Actor.VisibleCollidingActors", "Actor.CollidingActors"] {
        vm.register_iterator(name, |vm, this, a| {
            let Some(o) = this else { return Ok(Vec::new()) };
            let radius = a.get(2).map(Value::as_float).unwrap_or(0.0);
            let center = if given(a, 3) { vec3(&a[3]) } else { vec3(&get(vm, o, "Location")) };
            // Distance first, straight from the location slot every actor shares; class checks only in range.
            let class = obj_arg(a, 0).and_then(|c| vm.class_of(c));
            let Some(loc_slot) = location_slot(vm, o) else { return Ok(Vec::new()) };
            let r2 = radius * radius;
            let center = glam::Vec3::from(center);
            let mut actors = actors_near(vm, center, radius, loc_slot);
            // Creation order, as a full scan of the actor list gives.
            actors.sort_unstable();
            if std::env::var("HP3_GRID_CHECK").is_ok() {
                let all = world(vm).actors.clone();
                let brute = all.iter().filter(|&&x| location_at(vm, x, loc_slot).is_some_and(|p| p.distance_squared(center) <= r2)).count();
                let grid = actors.iter().filter(|&&x| location_at(vm, x, loc_slot).is_some_and(|p| p.distance_squared(center) <= r2)).count();
                if brute != grid {
                    eprintln!("grid mismatch: brute {brute} grid {grid} radius {radius}");
                }
            }
            let mut out = Vec::new();
            for &x in &actors {
                let e = vm.db.objects.get(x);
                if e.deleted {
                    continue;
                }
                let Some(p) = location_at(vm, x, loc_slot) else { continue };
                if p.distance_squared(center) > r2 {
                    continue;
                }
                if let (Some(want), Some(have)) = (class, e.class)
                    && !vm.is_child_of(have, want)
                {
                    continue;
                }
                let mut item = a.to_vec();
                item[1] = Value::Object(Some(x));
                out.push(item);
            }
            Ok(out)
        });
    }
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
    vm.register("Actor.Trace", |vm, this, a| {
        let Some(o) = this else { return Ok(Value::Object(None)) };
        let end = crate::physics::vec(&a[2]);
        let start = if given(a, 3) { crate::physics::vec(&a[3]) } else { crate::physics::vec(&get(vm, o, "Location")) };
        let trace_actors = a.get(4).is_some_and(Value::as_bool);
        let extent = if given(a, 5) { crate::physics::vec(&a[5]) } else { glam::Vec3::ZERO };
        let w = world(vm);
        let saved = if trace_actors { None } else { Some(std::mem::take(&mut w.collision.actors)) };
        let hit = w.collision.trace(start, end, extent, Some(o));
        if let Some(saved) = saved {
            w.collision.actors = saved;
        }
        let level = w.level_info;
        match hit {
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
        let w = world(vm);
        let saved = std::mem::take(&mut w.collision.actors);
        let clear = w.collision.trace(start, end, glam::Vec3::ZERO, Some(o)).is_none();
        w.collision.actors = saved;
        Ok(Value::Bool(clear))
    });
    // TraceActors(BaseClass, out Actor, out HitLoc, out HitNorm, End, optional Start, optional Extent):
    // every blocking actor the segment passes through, nearest first.
    vm.register_iterator("Actor.TraceActors", |vm, this, a| {
        let Some(o) = this else { return Ok(Vec::new()) };
        let end = crate::physics::vec(&a[4]);
        let start = if given(a, 5) { crate::physics::vec(&a[5]) } else { crate::physics::vec(&get(vm, o, "Location")) };
        let extent = if given(a, 6) { crate::physics::vec(&a[6]) } else { glam::Vec3::ZERO };
        let class = obj_arg(a, 0).and_then(|c| vm.class_of(c));
        let boxes = world(vm).collision.actors.clone();
        let mut hits = Vec::new();
        for b in boxes {
            if Some(b.actor) == this {
                continue;
            }
            if let (Some(want), Some(have)) = (class, vm.db.objects.get(b.actor).class)
                && !vm.is_child_of(have, want)
            {
                continue;
            }
            let mut single = crate::collision::Collision::default();
            single.actors.push(b);
            if let Some(h) = single.trace(start, end, extent, None) {
                hits.push(h);
            }
        }
        // The level's geometry stops the segment: its first hit (Level for BSP and terrain, the actor for a
        // static mesh) ends the list. HP3's camera keeps out of walls with this.
        let level = world(vm).level_info;
        if let Some(mut h) = world(vm).collision.trace_static(start, end, extent) {
            h.actor = h.actor.or(level);
            let wanted = match (class, h.actor.and_then(|x| vm.db.objects.get(x).class)) {
                (Some(want), Some(have)) => vm.is_child_of(have, want),
                _ => true,
            };
            hits.retain(|x| x.time <= h.time);
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
    // Emitters are not simulated yet; triggering one does nothing.
    // SetOpacity(float): kept for the renderer.
    vm.register("Actor.SetOpacity", |vm, this, a| {
        if let Some(o) = this {
            let alpha = a.first().map(Value::as_float).unwrap_or(1.0);
            world(vm).opacity.insert(o, alpha);
        }
        Ok(Value::Empty)
    });
    // AI hearing and music: nothing to do until those systems exist.
    vm.register("Actor.MakeNoise", |_vm, _this, _a| Ok(Value::Empty));
    vm.register("Actor.StopAllMusic", |_vm, _this, _a| Ok(Value::Empty));
    vm.register("LevelInfo.IsDemoBuild", |_vm, _this, _a| Ok(Value::Bool(false)));
    // Skin tint: a placeholder until rendering uses it.
    vm.register("Actor.SetSkinColor", |_vm, _this, _a| Ok(Value::Empty));
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
