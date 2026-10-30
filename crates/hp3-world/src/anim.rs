//! Actors' animation channels: which sequence plays at which frame and rate, advanced every tick, with the
//! `AnimEnd` event and `FinishAnim` waits. Rendering samples the same state.
//!
//! Written from the script declarations and standard Unreal animation behaviour; NOT yet verified against
//! HP3's Engine.dll (blending, tweening and notifies are approximations or missing).

use hp3_script::fx::FxHashMap as HashMap;
use std::rc::Rc;

use hp3_anim::AnimSet;
use hp3_assets::Asset;
use hp3_script::vm::Latent;
use hp3_script::{Name, ObjId, Value, Vm};

use crate::{fire, get, set, world};

#[derive(Clone, Debug, Default)]
pub struct Channel {
    pub sequence: Option<usize>,
    /// Current frame, from 0 to the sequence's frame count.
    pub frame: f32,
    /// Frames per second.
    pub rate: f32,
    pub looping: bool,
    pub playing: bool,
    /// Blend weight set by `AnimBlendParams`/`AnimBlendToAlpha` (channel 0 is always full).
    pub alpha: f32,
}

#[derive(Default)]
pub struct ActorAnim {
    pub set: Option<Rc<AnimSet>>,
    pub channels: Vec<Channel>,
}

/// A skeletal mesh's skeleton and its mesh-to-actor transform.
pub struct MeshSkeleton {
    pub skeleton: hp3_anim::Skeleton,
    pub to_actor: glam::Mat4,
}

#[derive(Default)]
pub struct Animation {
    pub actors: HashMap<ObjId, ActorAnim>,
    sets: HashMap<ObjId, Option<Rc<AnimSet>>>,
    skeletons: HashMap<ObjId, Option<Rc<MeshSkeleton>>>,
    bone_maps: HashMap<(ObjId, usize), Rc<Vec<Option<usize>>>>,
    /// Actors attached to another actor's bone (lower-case bone name).
    pub attachments: HashMap<ObjId, (ObjId, String)>,
}

/// Loads an animation set object (`MeshAnimation`).
fn load_set(vm: &mut Vm<'_>, anim: ObjId) -> Option<Rc<AnimSet>> {
    if let Some(s) = world(vm).anim.sets.get(&anim) {
        return s.clone();
    }
    let (pkg, index) = vm.db.objects.get(anim).key.clone()?;
    let p = vm.db.set.get(&pkg).ok()?;
    let chain = vm.db.set.export_chain(&p, index);
    let set = match crate::native_asset(&p, index, &chain) {
        Some(Asset::MeshAnimation(a)) => Some(Rc::new(AnimSet::new(*a, &p))),
        _ => None,
    };
    world(vm).anim.sets.insert(anim, set.clone());
    set
}

/// The animation set an actor plays from: one linked by script, else its skeletal mesh's default.
pub fn set_of(vm: &mut Vm<'_>, actor: ObjId) -> Option<Rc<AnimSet>> {
    if let Some(a) = world(vm).anim.actors.get(&actor)
        && a.set.is_some()
    {
        return a.set.clone();
    }
    let mesh = get(vm, actor, "Mesh").as_obj()?;
    let (pkg, index) = vm.db.objects.get(mesh).key.clone()?;
    let p = vm.db.set.get(&pkg).ok()?;
    let chain = vm.db.set.export_chain(&p, index);
    let Some(Asset::SkeletalMesh(m)) = crate::native_asset(&p, index, &chain) else { return None };
    let anim = vm.db.resolve_obj(&p, m.default_anim)?;
    let s = load_set(vm, anim);
    world(vm).anim.actors.entry(actor).or_default().set = s.clone();
    s
}

/// The skeleton of a skeletal mesh object.
pub fn skeleton_of(vm: &mut Vm<'_>, mesh: ObjId) -> Option<Rc<MeshSkeleton>> {
    if let Some(s) = world(vm).anim.skeletons.get(&mesh) {
        return s.clone();
    }
    let s = (|| {
        let (pkg, index) = vm.db.objects.get(mesh).key.clone()?;
        let p = vm.db.set.get(&pkg).ok()?;
        let chain = vm.db.set.export_chain(&p, index);
        let Some(Asset::SkeletalMesh(m)) = crate::native_asset(&p, index, &chain) else { return None };
        Some(Rc::new(MeshSkeleton { skeleton: hp3_anim::Skeleton::from_mesh(&m, &p), to_actor: hp3_anim::mesh_to_actor(&m) }))
    })();
    world(vm).anim.skeletons.insert(mesh, s.clone());
    s
}

fn rotator_of(v: &Value) -> [i32; 3] {
    let r: Vec<i32> = v.fields().iter().map(Value::as_int).collect();
    [r.first().copied().unwrap_or(0), r.get(1).copied().unwrap_or(0), r.get(2).copied().unwrap_or(0)]
}

/// An actor's transform: actor space to world.
pub fn actor_matrix(vm: &mut Vm<'_>, a: ObjId) -> glam::Mat4 {
    let loc = crate::physics::vec(&get(vm, a, "Location"));
    let rot = rotator_of(&get(vm, a, "Rotation"));
    let scale = match get(vm, a, "DrawScale").as_float() {
        0.0 => 1.0,
        s => s,
    };
    let s3 = match get(vm, a, "DrawScale3D") {
        v @ Value::Struct(_) => crate::physics::vec(&v),
        _ => glam::Vec3::ONE,
    };
    let s3 = if s3 == glam::Vec3::ZERO { glam::Vec3::ONE } else { s3 };
    let prepivot = crate::physics::vec(&get(vm, a, "PrePivot"));
    glam::Mat4::from_translation(loc + prepivot) * hp3_anim::rotator_matrix(rot) * glam::Mat4::from_scale(s3 * scale)
}

/// A bone's current transform in world space (scale included), from the actor's animation channel 0.
pub fn bone_matrix(vm: &mut Vm<'_>, a: ObjId, bone: &str) -> Option<glam::Mat4> {
    let mesh = get(vm, a, "Mesh").as_obj()?;
    let skel = skeleton_of(vm, mesh)?;
    let index = skel.skeleton.find(bone)?;
    let set = set_of(vm, a);
    let ch = world(vm).anim.actors.get(&a).and_then(|x| x.channels.first().cloned()).unwrap_or_default();
    let bones = match &set {
        Some(set) => {
            let key = (mesh, Rc::as_ptr(set) as usize);
            let map = match world(vm).anim.bone_maps.get(&key) {
                Some(m) => m.clone(),
                None => {
                    let m = Rc::new(set.bone_map(&skel.skeleton));
                    world(vm).anim.bone_maps.insert(key, m.clone());
                    m
                }
            };
            hp3_anim::bones(&skel.skeleton, Some((set, &map)), ch.sequence, ch.frame)
        }
        None => hp3_anim::bones(&skel.skeleton, None, None, 0.0),
    };
    Some(actor_matrix(vm, a) * skel.to_actor * *bones.get(index)?)
}

/// `m` without scale: unit axes, same origin.
fn unscaled(m: glam::Mat4) -> glam::Mat4 {
    glam::Mat4::from_cols(
        m.x_axis.truncate().normalize_or_zero().extend(0.0),
        m.y_axis.truncate().normalize_or_zero().extend(0.0),
        m.z_axis.truncate().normalize_or_zero().extend(0.0),
        m.w_axis,
    )
}

fn rotator_value(r: [i32; 3]) -> Value {
    Value::Struct(r.iter().map(|&v| Value::Int(v)).collect::<Vec<_>>().into())
}

/// Moves actors attached to bones onto their bones, offset by their relative location and rotation.
pub fn update_attachments(vm: &mut Vm<'_>) {
    let list: Vec<(ObjId, (ObjId, String))> = world(vm).anim.attachments.iter().map(|(k, v)| (*k, v.clone())).collect();
    for (child, (parent, bone)) in list {
        if vm.db.objects.get(child).deleted || vm.db.objects.get(parent).deleted {
            world(vm).anim.attachments.remove(&child);
            continue;
        }
        let Some(m) = bone_matrix(vm, parent, &bone) else { continue };
        let rel_loc = crate::physics::vec(&get(vm, child, "RelativeLocation"));
        let rel_rot = rotator_of(&get(vm, child, "RelativeRotation"));
        let placed = unscaled(m) * glam::Mat4::from_translation(rel_loc) * hp3_anim::rotator_matrix(rel_rot);
        set(vm, child, "Location", crate::physics::value(placed.w_axis.truncate()));
        set(vm, child, "Rotation", rotator_value(hp3_anim::matrix_rotator(&placed)));
    }
}

fn channel_mut<'a>(vm: &'a mut Vm<'_>, actor: ObjId, index: usize) -> &'a mut Channel {
    let a = world(vm).anim.actors.entry(actor).or_default();
    while a.channels.len() <= index {
        a.channels.push(Channel { alpha: if a.channels.is_empty() { 1.0 } else { 0.0 }, ..Default::default() });
    }
    &mut a.channels[index]
}

/// Starts a sequence on a channel. Returns false when the actor has no such sequence.
fn play(vm: &mut Vm<'_>, actor: ObjId, seq: Name, rate: f32, channel: usize, looping: bool) -> bool {
    let Some(set) = set_of(vm, actor) else {
        if std::env::var("HP3_ANIM_TRACE").is_ok() {
            eprintln!("anim: {actor:?} has no animation set for {}", vm.db.names.get(seq));
        }
        return false;
    };
    let name = vm.db.names.get(seq).to_string();
    let Some(si) = set.find(&name) else {
        if std::env::var("HP3_ANIM_TRACE").is_ok() {
            eprintln!("anim: {actor:?} has no sequence {name}");
        }
        return false;
    };
    let base = set.sequences[si].rate;
    let ch = channel_mut(vm, actor, channel);
    // Looping the sequence that already loops keeps its place.
    if looping && ch.looping && ch.playing && ch.sequence == Some(si) {
        ch.rate = base * rate;
        return true;
    }
    *ch = Channel { sequence: Some(si), frame: 0.0, rate: base * rate, looping, playing: true, alpha: ch.alpha };
    if channel == 0 {
        set_script_view(vm, actor, seq, 0.0, base * rate);
    }
    true
}

fn set_script_view(vm: &mut Vm<'_>, actor: ObjId, seq: Name, frame: f32, rate: f32) {
    set(vm, actor, "AnimSequence", Value::Name(seq));
    set(vm, actor, "AnimFrame", Value::Float(frame));
    set(vm, actor, "AnimRate", Value::Float(rate));
}

/// Advances every channel; fires `AnimEnd` and ends `FinishAnim` waits when non-looping sequences finish.
pub fn tick(vm: &mut Vm<'_>, dt: f32) {
    let ids: Vec<ObjId> = world(vm).anim.actors.keys().copied().collect();
    for a in ids {
        let Some(anims) = world(vm).anim.actors.get(&a).and_then(|x| x.set.clone()) else { continue };
        let mut ended = Vec::new();
        let mut looped = false;
        if let Some(actor) = world(vm).anim.actors.get_mut(&a) {
            for (ci, ch) in actor.channels.iter_mut().enumerate() {
                let Some(si) = ch.sequence else { continue };
                if !ch.playing {
                    continue;
                }
                let frames = anims.sequences[si].frames as f32;
                ch.frame += ch.rate * dt;
                if ch.frame >= frames {
                    if ch.looping {
                        ch.frame %= frames.max(1.0);
                        looped |= ci == 0;
                    } else {
                        ch.frame = frames;
                        ch.playing = false;
                        ended.push(ci);
                    }
                }
            }
        }
        if let Some(ch0) = world(vm).anim.actors.get(&a).and_then(|x| x.channels.first().cloned())
            && let Some(si) = ch0.sequence
        {
            let frames = anims.sequences[si].frames.max(1) as f32;
            set(vm, a, "AnimFrame", Value::Float(ch0.frame / frames));
        }
        if (ended.contains(&0) || looped) && matches!(vm.latent.get(&a), Some(Latent::Other("anim"))) {
            vm.finish_latent(a);
        }
        for ci in ended {
            fire(vm, a, "AnimEnd", &mut [Value::Int(ci as i32)]);
        }
    }
}

/// Pawns with `bPhysicsAnimUpdate`: the movement animation follows velocity (forward, back, left or right of
/// `MovementAnims`, at speed / `BaseMovementRate`); standing still hands over to the script's `PlayWaiting`,
/// falling to `PlayFalling`.
pub fn update_movement(vm: &mut Vm<'_>) {
    let Some(pawn_class) = vm.db.find("Pawn") else { return };
    let actors = world(vm).actors.clone();
    for a in actors {
        let Some(class) = vm.db.objects.get(a).class else { continue };
        if vm.db.objects.get(a).deleted || !vm.is_child_of(class, pawn_class) || !get(vm, a, "bPhysicsAnimUpdate").as_bool() {
            continue;
        }
        let Some(anims) = set_of(vm, a) else { continue };
        let physics = get(vm, a, "Physics").as_byte();
        let vel = crate::physics::vec(&get(vm, a, "Velocity"));
        let current = world(vm).anim.actors.get(&a).and_then(|x| x.channels.first().cloned()).unwrap_or_default();
        let movement: Vec<Name> = (0..4).map(|i| get_indexed(vm, a, "MovementAnims", i).as_name()).collect();
        let current_name = current.sequence.map(|s| anims.sequences[s].name.clone());
        let in_movement = current_name.as_ref().is_some_and(|n| movement.iter().any(|m| vm.db.names.get(*m).eq_ignore_ascii_case(n)));
        match physics {
            crate::physics::PHYS_WALKING => {
                let speed = (vel.x * vel.x + vel.y * vel.y).sqrt();
                if speed > 10.0 {
                    let yaw = get(vm, a, "Rotation").fields().get(1).map(Value::as_int).unwrap_or(0) as f32;
                    let k = std::f32::consts::TAU / 65536.0;
                    let fwd = glam::Vec2::new((yaw * k).cos(), (yaw * k).sin());
                    let dir = glam::Vec2::new(vel.x, vel.y) / speed;
                    let (f, r) = (dir.dot(fwd), fwd.perp_dot(dir));
                    let index = if f.abs() >= r.abs() { if f >= 0.0 { 0 } else { 1 } } else if r > 0.0 { 2 } else { 3 };
                    let seq = movement[index];
                    let base = get(vm, a, "BaseMovementRate").as_float().max(1.0);
                    if seq != Name::NONE {
                        play(vm, a, seq, speed / base, 0, true);
                    }
                } else if in_movement || current.sequence.is_none() {
                    fire(vm, a, "PlayWaiting", &mut []);
                }
            }
            crate::physics::PHYS_FALLING if in_movement || current.sequence.is_none() => {
                fire(vm, a, "PlayFalling", &mut []);
            }
            // Pawns parked without physics (inactive NPCs) still stand idle.
            _ if current.sequence.is_none() => {
                fire(vm, a, "PlayWaiting", &mut []);
            }
            _ => {}
        }
    }
}

/// Element `index` of a static array property.
fn get_indexed(vm: &mut Vm<'_>, a: ObjId, name: &str, index: usize) -> Value {
    let Some(slot) = crate::slot(vm, a, name) else { return Value::Empty };
    vm.db.objects.values(a).and_then(|v| v.get(slot + index).cloned()).unwrap_or_default()
}

fn channel_arg(a: &[Value], i: usize) -> usize {
    a.get(i).map(|v| v.as_int().max(0) as usize).unwrap_or(0)
}

fn rate_arg(a: &[Value], i: usize) -> f32 {
    match a.get(i) {
        None | Some(Value::Empty) => 1.0,
        Some(v) => v.as_float(),
    }
}

pub fn register(vm: &mut Vm<'_>) {
    // PlayAnim(Sequence, optional Rate, optional TweenTime, optional Channel)
    vm.register("Actor.PlayAnim", |vm, this, a| {
        let Some(o) = this else { return Ok(Value::Bool(false)) };
        Ok(Value::Bool(play(vm, o, a[0].as_name(), rate_arg(a, 1), channel_arg(a, 3), false)))
    });
    vm.register("Actor.LoopAnim", |vm, this, a| {
        let Some(o) = this else { return Ok(Value::Bool(false)) };
        Ok(Value::Bool(play(vm, o, a[0].as_name(), rate_arg(a, 1), channel_arg(a, 3), true)))
    });
    // TweenAnim(Sequence, Time, optional Channel): holds the first frame for the tween time.
    vm.register("Actor.TweenAnim", |vm, this, a| {
        let Some(o) = this else { return Ok(Value::Bool(false)) };
        let ok = play(vm, o, a[0].as_name(), 1.0, channel_arg(a, 2), false);
        if ok {
            let time = a.get(1).map(Value::as_float).unwrap_or(0.0);
            let ch = channel_mut(vm, o, channel_arg(a, 2));
            ch.rate = 0.0;
            ch.playing = time > 0.0;
            if time <= 0.0 {
                ch.playing = false;
            }
        }
        Ok(Value::Bool(ok))
    });
    vm.register("Actor.FinishAnim", |vm, this, _a| {
        if let Some(o) = this {
            let playing = world(vm).anim.actors.get(&o).and_then(|x| x.channels.first()).is_some_and(|c| c.playing);
            if playing {
                vm.latent.insert(o, Latent::Other("anim"));
            }
        }
        Ok(Value::Empty)
    });
    vm.register("Actor.IsAnimating", |vm, this, a| {
        let ci = channel_arg(a, 0);
        let playing = this
            .and_then(|o| world(vm).anim.actors.get(&o))
            .and_then(|x| x.channels.get(ci))
            .is_some_and(|c| c.playing);
        Ok(Value::Bool(playing))
    });
    vm.register("Actor.StopAnimating", |vm, this, _a| {
        if let Some(o) = this
            && let Some(x) = world(vm).anim.actors.get_mut(&o)
        {
            for c in &mut x.channels {
                c.playing = false;
            }
        }
        Ok(Value::Empty)
    });
    vm.register("Actor.HasAnim", |vm, this, a| {
        let Some(o) = this else { return Ok(Value::Bool(false)) };
        let name = vm.db.names.get(a[0].as_name()).to_string();
        Ok(Value::Bool(set_of(vm, o).is_some_and(|s| s.find(&name).is_some())))
    });
    // GetAnimParams(Channel, out OutSeqName, out OutAnimFrame, out OutAnimRate)
    vm.register("Actor.GetAnimParams", |vm, this, a| {
        let Some(o) = this else { return Ok(Value::Empty) };
        let ci = channel_arg(a, 0);
        let Some(set) = set_of(vm, o) else { return Ok(Value::Empty) };
        let ch = world(vm).anim.actors.get(&o).and_then(|x| x.channels.get(ci).cloned()).unwrap_or_default();
        if let Some(si) = ch.sequence
            && a.len() >= 4
        {
            let s = &set.sequences[si];
            a[1] = Value::Name(vm.db.names.intern(&s.name));
            a[2] = Value::Float(ch.frame / s.frames.max(1) as f32);
            a[3] = Value::Float(ch.rate / s.frames.max(1) as f32);
        }
        Ok(Value::Empty)
    });
    vm.register("Actor.GetAnimSequence", |vm, this, _a| Ok(this.map(|o| get(vm, o, "AnimSequence")).unwrap_or(Value::Name(Name::NONE))));
    // SetAnimTime / SetAnimRate (HP3): set a channel's position or speed.
    vm.register("Actor.SetAnimTime", |vm, this, a| {
        if let Some(o) = this {
            let t = a.first().map(Value::as_float).unwrap_or(0.0);
            let ci = channel_arg(a, 1);
            let frames = set_of(vm, o).and_then(|s| {
                let si = world(vm).anim.actors.get(&o)?.channels.get(ci)?.sequence?;
                Some(s.sequences[si].frames as f32)
            });
            if let Some(f) = frames {
                channel_mut(vm, o, ci).frame = t * f;
            }
        }
        Ok(Value::Empty)
    });
    vm.register("Actor.SetAnimRate", |vm, this, a| {
        if let Some(o) = this {
            let r = a.first().map(Value::as_float).unwrap_or(1.0);
            let ci = channel_arg(a, 1);
            let base = set_of(vm, o).and_then(|s| {
                let si = world(vm).anim.actors.get(&o)?.channels.get(ci)?.sequence?;
                Some(s.sequences[si].rate)
            });
            if let Some(b) = base {
                channel_mut(vm, o, ci).rate = b * r;
            }
        }
        Ok(Value::Empty)
    });
    vm.register("Actor.AnimBlendParams", |vm, this, a| {
        if let Some(o) = this {
            let ci = channel_arg(a, 0);
            channel_mut(vm, o, ci).alpha = a.get(1).map(Value::as_float).unwrap_or(0.0);
        }
        Ok(Value::Empty)
    });
    vm.register("Actor.AnimBlendToAlpha", |vm, this, a| {
        if let Some(o) = this {
            let ci = channel_arg(a, 0);
            channel_mut(vm, o, ci).alpha = a.get(1).map(Value::as_float).unwrap_or(0.0);
        }
        Ok(Value::Empty)
    });
    // AttachToBone(Attachment, BoneName)
    vm.register("Actor.AttachToBone", |vm, this, a| {
        let (Some(o), Some(child)) = (this, a.first().and_then(Value::as_obj)) else { return Ok(Value::Bool(false)) };
        let bone = vm.db.names.get(a[1].as_name()).to_ascii_lowercase();
        let found = get(vm, o, "Mesh").as_obj().and_then(|m| skeleton_of(vm, m)).is_some_and(|s| s.skeleton.find(&bone).is_some());
        if !found {
            return Ok(Value::Bool(false));
        }
        set(vm, child, "Base", Value::Object(Some(o)));
        set(vm, child, "AttachmentBone", Value::Name(a[1].as_name()));
        world(vm).anim.attachments.insert(child, (o, bone));
        update_attachments(vm);
        Ok(Value::Bool(true))
    });
    vm.register("Actor.DetachFromBone", |vm, _this, a| {
        let Some(child) = a.first().and_then(Value::as_obj) else { return Ok(Value::Bool(false)) };
        let was = world(vm).anim.attachments.remove(&child).is_some();
        if was {
            set(vm, child, "Base", Value::Object(None));
            set(vm, child, "AttachmentBone", Value::Name(Name::NONE));
        }
        Ok(Value::Bool(was))
    });
    // GetBoneCoords(BoneName): Coords { Origin, XAxis, YAxis, ZAxis } in world space.
    vm.register("Actor.GetBoneCoords", |vm, this, a| {
        let bone = vm.db.names.get(a[0].as_name()).to_string();
        let m = match this {
            Some(o) => bone_matrix(vm, o, &bone).unwrap_or_else(|| actor_matrix(vm, o)),
            None => glam::Mat4::IDENTITY,
        };
        let m = unscaled(m);
        Ok(Value::Struct(vec![
            crate::physics::value(m.w_axis.truncate()),
            crate::physics::value(m.x_axis.truncate()),
            crate::physics::value(m.y_axis.truncate()),
            crate::physics::value(m.z_axis.truncate()),
        ].into()))
    });
    vm.register("Actor.GetBoneRotation", |vm, this, a| {
        let bone = vm.db.names.get(a[0].as_name()).to_string();
        let r = this.and_then(|o| bone_matrix(vm, o, &bone)).map(|m| hp3_anim::matrix_rotator(&m)).unwrap_or([0; 3]);
        Ok(rotator_value(r))
    });
    // LinkMesh(NewMesh, optional bKeepAnim)
    vm.register("Actor.LinkMesh", |vm, this, a| {
        if let Some(o) = this {
            if std::env::var("HP3_ANIM_TRACE").is_ok() {
                let m = a.first().and_then(Value::as_obj).map(|m| hp3_script::natives::object_path(vm, m));
                eprintln!("anim: LinkMesh {o:?} -> {m:?}");
            }
            set(vm, o, "Mesh", Value::Object(a.first().and_then(Value::as_obj)));
            if !a.get(1).is_some_and(Value::as_bool)
                && let Some(x) = world(vm).anim.actors.get_mut(&o)
            {
                x.set = None;
                x.channels.clear();
            }
        }
        Ok(Value::Empty)
    });
    vm.register("Actor.LinkSkelAnim", |vm, this, a| {
        if let (Some(o), Some(anim)) = (this, a.first().and_then(Value::as_obj)) {
            let s = load_set(vm, anim);
            world(vm).anim.actors.entry(o).or_default().set = s;
        }
        Ok(Value::Empty)
    });
}
