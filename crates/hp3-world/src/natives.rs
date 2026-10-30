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
        if vm.db.objects.get(o).deleted {
            return Ok(Value::Bool(true));
        }
        if get(vm, o, "bStatic").as_bool() || get(vm, o, "bNoDelete").as_bool() {
            return Ok(Value::Bool(false));
        }
        fire(vm, o, "Destroyed", &mut []);
        vm.db.objects.get_mut(o).deleted = true;
        let w = world(vm);
        w.actors.retain(|&x| x != o);
        w.timers.remove(&o);
        vm.latent.remove(&o);
        Ok(Value::Bool(true))
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
    vm.register("Actor.PlaySound", |_vm, _this, _a| Ok(Value::Empty));
    vm.register("Actor.PlayOwnedSound", |_vm, _this, _a| Ok(Value::Empty));
    vm.register("Actor.PlayMusic", |_vm, _this, _a| Ok(Value::Int(0)));
    // Animation: records the sequence; FinishAnim waits one tick until skeletal animation exists.
    vm.register("Actor.PlayAnim", |vm, this, a| {
        if let Some(o) = this {
            set(vm, o, "AnimSequence", Value::Name(a[0].as_name()));
        }
        Ok(Value::Bool(true))
    });
    vm.register("Actor.LoopAnim", |vm, this, a| {
        if let Some(o) = this {
            set(vm, o, "AnimSequence", Value::Name(a[0].as_name()));
        }
        Ok(Value::Bool(true))
    });
    vm.register("Actor.TweenAnim", |_vm, _this, _a| Ok(Value::Bool(true)));
    vm.register("Actor.FinishAnim", |vm, this, _a| {
        if let Some(o) = this {
            vm.latent.insert(o, Latent::Sleep(0.0));
        }
        Ok(Value::Empty)
    });
    vm.register("Actor.FinishInterpolation", |vm, this, _a| {
        if let Some(o) = this {
            vm.latent.insert(o, Latent::Sleep(0.0));
        }
        Ok(Value::Empty)
    });
    vm.register("Actor.IsAnimating", |_vm, _this, _a| Ok(Value::Bool(false)));
    vm.register("Actor.HasAnim", |_vm, _this, _a| Ok(Value::Bool(true)));
    vm.register("Actor.StopAnimating", |_vm, _this, _a| Ok(Value::Empty));
    vm.register("Actor.GetAnimSequence", |vm, this, _a| Ok(this.map(|o| get(vm, o, "AnimSequence")).unwrap_or(Value::Name(Name::NONE))));
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
    vm.register("Actor.AnimBlendParams", |_vm, _this, _a| Ok(Value::Empty));
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
            let cur = get(vm, o, &prop);
            let v = match cur {
                Value::Int(_) => Value::Int(text.trim().parse().unwrap_or(0)),
                Value::Float(_) => Value::Float(text.trim().parse().unwrap_or(0.0)),
                Value::Byte(_) => Value::Byte(text.trim().parse().unwrap_or(0)),
                Value::Bool(_) => Value::Bool(text.trim().eq_ignore_ascii_case("true") || text.trim() == "1"),
                Value::Name(_) => Value::Name(vm.db.names.intern(text.trim())),
                Value::Str(_) => Value::Str(text),
                Value::Object(_) => Value::Object(crate::find_object(vm, text.trim())),
                _ => return Ok(Value::Empty),
            };
            set(vm, o, &prop, v);
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
            let found = actors_of(vm, obj_arg(a, 0), Name::NONE);
            let mut out = Vec::new();
            for x in found {
                let p = vec3(&get(vm, x, "Location"));
                let d2 = (0..3).map(|i| (p[i] - center[i]).powi(2)).sum::<f32>();
                if d2 <= radius * radius {
                    let mut item = a.to_vec();
                    item[1] = Value::Object(Some(x));
                    out.push(item);
                }
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
    vm.register("LevelInfo.IsDemoBuild", |_vm, _this, _a| Ok(Value::Bool(false)));
    vm.register("Actor.AttachToBone", |vm, this, a| {
        if let Some(att) = obj_arg(a, 0) {
            set(vm, att, "Base", Value::Object(this));
        }
        Ok(Value::Bool(true))
    });
    vm.register("Actor.GetAnimParams", |vm, this, a| {
        if let Some(o) = this
            && a.len() >= 4
        {
            a[1] = get(vm, o, "AnimSequence");
            a[2] = Value::Float(0.0);
            a[3] = Value::Float(1.0);
        }
        Ok(Value::Empty)
    });
    // Skin tint and animation timing: placeholders until rendering and animation exist.
    vm.register("Actor.SetSkinColor", |_vm, _this, _a| Ok(Value::Empty));
    vm.register("Actor.SetAnimTime", |_vm, _this, _a| Ok(Value::Empty));
    vm.register("Actor.AnimBlendToAlpha", |_vm, _this, _a| Ok(Value::Empty));
    vm.register("Object.DynamicLoadObject", |vm, _this, a| {
        let path = a[0].as_str().to_string();
        let found = crate::find_object(vm, &path);
        if found.is_none() && !a.get(2).is_some_and(Value::as_bool) {
            vm.warn(format!("DynamicLoadObject: {path} not found"));
        }
        Ok(Value::Object(found))
    });
}
