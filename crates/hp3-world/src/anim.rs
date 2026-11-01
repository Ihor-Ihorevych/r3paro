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
    /// Where `alpha` is heading and how fast (per second); 0 means it jumps there.
    pub target_alpha: f32,
    pub blend_rate: f32,
    /// The bone (lower case) the channel's animation is limited to, with everything below it.
    pub bone: Option<String>,
    /// The script set this channel's blending (`AnimBlendParams`/`AnimBlendToAlpha`); otherwise it acts at full
    /// strength over the whole body while its animation plays.
    pub scripted_blend: bool,
    /// While a new sequence fades in: what played before (sequence, frame, looping), how long the fade takes
    /// and how long it has run.
    pub from: Option<(Option<usize>, f32, bool)>,
    pub tween_total: f32,
    pub tween_t: f32,
    /// A `TweenAnim`: only the fade to the sequence's first pose; it ends with `AnimEnd`.
    pub tween_only: bool,
    /// `EnableChannelNotify(channel, false)`: the channel raises neither sequence notifies nor `AnimEnd`.
    pub ignore_notify: bool,
}

impl Channel {
    /// How far the fade-in has got, 0 (still the old pose) to 1 (done).
    pub fn tween_weight(&self) -> f32 {
        if self.tween_total <= 0.0 { 1.0 } else { (self.tween_t / self.tween_total).clamp(0.0, 1.0) }
    }
}

#[derive(Default)]
pub struct ActorAnim {
    pub set: Option<Rc<AnimSet>>,
    pub channels: Vec<Channel>,
    /// Bone controllers (`SetBoneRotation`): lower-case bone name to (pitch, yaw, roll) and weight.
    pub bone_turns: Vec<(String, [i32; 3], f32)>,
    /// State of the movement-animation driver for this pawn.
    pub movement: MovementState,
    /// The channel whose notify was raised last (`GetNotifyChannel`).
    pub last_notify_channel: usize,
}

/// An actor's bone controller rotations as (bone index, matrix) for a skeleton.
pub fn turns_for(skel: &hp3_anim::Skeleton, turns: &[(String, [i32; 3], f32)]) -> Vec<(usize, glam::Mat4)> {
    turns
        .iter()
        .filter_map(|(bone, rot, alpha)| {
            let i = skel.find(bone)?;
            let full = glam::Quat::from_mat4(&hp3_anim::rotator_matrix(*rot));
            let q = glam::Quat::IDENTITY.slerp(full, alpha.clamp(0.0, 1.0));
            Some((i, glam::Mat4::from_quat(q)))
        })
        .collect()
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
    /// `FinishAnim` waits: (who waits, whose channel, which channel).
    pub waiters: Vec<(ObjId, ObjId, usize)>,
    /// Script events a sequence raises on its actor as it plays: per animation set (by identity) and sequence,
    /// the place in the sequence (0 to 1) and the event name. Every actor playing from the set raises them.
    notifies: HashMap<(usize, usize), Vec<(f32, Name)>>,
}

/// Identity of an animation set, for the notify table.
fn set_key(set: &Rc<AnimSet>) -> usize {
    Rc::as_ptr(set) as *const () as usize
}

/// Frames a channel passes while it moves from `old` by `step` in a sequence of `frames`: the stretches
/// (from, to] where its notifies fire. A looping sequence that wraps passes the end and then the start.
fn passed_stretches(old: f32, step: f32, frames: f32, looping: bool) -> [(f32, f32); 2] {
    let new = old + step;
    if new < frames || frames <= 0.0 {
        [(old, new), (0.0, -1.0)]
    } else if looping {
        // The second stretch includes frame 0 itself, hence the small step below zero.
        [(old, frames), (-0.001, new % frames)]
    } else {
        [(old, frames), (0.0, -1.0)]
    }
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
    // Bones, and what is attached to them (a student's hair on a head), follow the placement the mesh is drawn with:
    // its saved pivot and the drop of a bottom-aligned mesh, not the bare PrePivot.
    let prepivot = crate::mesh_offset(vm, a);
    glam::Mat4::from_translation(loc + prepivot) * hp3_anim::rotator_matrix(rot) * glam::Mat4::from_scale(s3 * scale)
}

/// The root bone, and the head and feet when the skeleton has them, for the game log: where each is against the
/// pawn and which way its up axis points (to see how a pose leans).
pub fn pose_debug(vm: &mut Vm<'_>, a: ObjId) -> String {
    let Some(mesh) = get(vm, a, "Mesh").as_obj() else { return String::new() };
    let Some(skel) = skeleton_of(vm, mesh) else { return String::new() };
    let origin = crate::physics::vec(&get(vm, a, "Location"));
    let mut names: Vec<String> = skel.skeleton.names.iter().take(1).cloned().collect();
    for want in ["bip01 head", "head", "bip01 l foot", "bip01 r foot", "bip01 spine", "bip01 spine1"] {
        if skel.skeleton.find(want).is_some() {
            names.push(want.to_string());
        }
    }
    let mut out = String::new();
    for n in names {
        if let Some(m) = bone_matrix(vm, a, &n) {
            let at = m.w_axis.truncate() - origin;
            let up = m.z_axis.truncate().normalize_or_zero();
            let fwd = m.x_axis.truncate().normalize_or_zero();
            out += &format!(" [{n} at ({:.0},{:.0},{:.0}) x ({:.2},{:.2},{:.2}) z ({:.2},{:.2},{:.2})]", at.x, at.y, at.z, fwd.x, fwd.y, fwd.z, up.x, up.y, up.z);
        }
    }
    out
}

/// A bone's current transform in world space (scale included), from the actor's animation channel 0.
pub fn bone_matrix(vm: &mut Vm<'_>, a: ObjId, bone: &str) -> Option<glam::Mat4> {
    // The hang correction turns the whole actor, so the bones go with it.
    let corrected = hang_correction(vm, a) * actor_matrix(vm, a);
    let extra = strafe_turns(vm, a);
    bone_matrix_extra(vm, a, bone, corrected, &extra)
}

/// The controlled pawn's run animations swing the root bone and the upper body from side to side (the sideways and
/// backward ones twist it about 45 degrees and the upper body and head keep turning the same way). The root is turned back as a whole (`hang_correction`); these are the turns
/// that bring the spine, neck and head round to face the way the pawn does too. Empty at any other time.
pub fn strafe_turns(vm: &mut Vm<'_>, a: ObjId) -> Vec<(usize, glam::Mat4)> {
    if get(vm, a, "Physics").as_byte() != crate::physics::PHYS_WALKING || !crate::physics::is_player_pawn(vm, a) {
        return Vec::new();
    }
    let Some(set) = set_of(vm, a) else { return Vec::new() };
    let sideways = world(vm).anim.actors.get(&a).is_some_and(|x| {
        x.channels.iter().any(|c| {
            c.alpha > 0.3
                && c.sequence.is_some_and(|s| {
                    let n = set.sequences[s].name.to_ascii_lowercase();
                    n.starts_with("run") || n.contains("walk")
                })
        })
    });
    if !sideways {
        return Vec::new();
    }
    let Some(mesh) = get(vm, a, "Mesh").as_obj() else { return Vec::new() };
    let Some(skel) = skeleton_of(vm, mesh) else { return Vec::new() };
    let plain = actor_matrix(vm, a);
    let drawn = hang_correction(vm, a) * plain;
    let axes = |m: glam::Mat4| glam::Mat3::from_cols(m.x_axis.truncate().normalize_or_zero(), m.y_axis.truncate().normalize_or_zero(), m.z_axis.truncate().normalize_or_zero());
    let rest_all = hp3_anim::bones(&skel.skeleton, None, None, 0.0);
    let mut turns: Vec<(usize, glam::Mat4)> = Vec::new();
    for name in ["body_lowerspine", "body_midspine", "body_topspine", "body_neckbase", "body_neck"] {
        let Some(i) = skel.skeleton.find(name) else { continue };
        let Some(m) = bone_matrix_extra(vm, a, name, drawn, &turns) else { continue };
        let rest = axes(plain * skel.to_actor * rest_all[i]);
        let current = axes(m);
        let f = current * (rest.inverse() * glam::Vec3::X);
        let yaw = f.y.atan2(f.x);
        // The sideways lean the stride gives this part of the spine, after the turn: its up axis loses the part that
        // points to the pawn's side.
        let back = glam::Mat3::from_rotation_z(-yaw);
        let heading = plain.x_axis.truncate();
        let heading = glam::Vec3::new(heading.x, heading.y, 0.0).normalize_or_zero();
        let side = glam::Vec3::new(heading.y, -heading.x, 0.0);
        let up2 = (back * current * (rest.inverse() * glam::Vec3::Z)).normalize_or_zero();
        let flat = (up2 - side * up2.dot(side)).normalize_or_zero();
        let lean = if up2 == glam::Vec3::ZERO || flat == glam::Vec3::ZERO { glam::Quat::IDENTITY } else { glam::Quat::from_rotation_arc(up2, flat) };
        if yaw.abs() < 0.01 && lean.angle_between(glam::Quat::IDENTITY) < 0.01 {
            continue;
        }
        // About the bone's own place: its world rotation turned back and uprighted, in its own axes.
        let local = current.inverse() * glam::Mat3::from_quat(lean) * back * current;
        turns.push((i, glam::Mat4::from_mat3(local)));
    }
    turns
}

/// While a pawn hangs from a Carpe Retractum line (or the controlled pawn is in the air), the animations tilt its whole body (the root bone swings to one
/// side and pitches as it is pulled), so it is turned back upright about its own place: a rotation in world space,
/// the identity at any other time.
pub fn hang_correction(vm: &mut Vm<'_>, a: ObjId) -> glam::Mat4 {
    let hanging = vm.state_of(a).is_some_and(|s| vm.db.names.get(vm.db.get(s).name).eq_ignore_ascii_case("StateRetractum"));
    // The jump and fall animations of the controlled pawn lean the body the same way.
    let airborne = !hanging && get(vm, a, "Physics").as_byte() == crate::physics::PHYS_FALLING && crate::physics::is_player_pawn(vm, a);
    // On the ground the sideways and backward runs twist the root bone about 45 degrees; the body is turned back
    // to face the way the pawn does.
    let grounded = !hanging && !airborne && get(vm, a, "Physics").as_byte() == crate::physics::PHYS_WALKING && crate::physics::is_player_pawn(vm, a);
    if !hanging && !airborne && !grounded {
        return glam::Mat4::IDENTITY;
    }
    let Some(mesh) = get(vm, a, "Mesh").as_obj() else { return glam::Mat4::IDENTITY };
    let Some(skel) = skeleton_of(vm, mesh) else { return glam::Mat4::IDENTITY };
    let Some(root_name) = skel.skeleton.names.first().cloned() else { return glam::Mat4::IDENTITY };
    let plain = actor_matrix(vm, a);
    let Some(current) = bone_matrix_in(vm, a, &root_name, plain) else { return glam::Mat4::IDENTITY };
    let rest = plain * skel.to_actor * hp3_anim::bones(&skel.skeleton, None, None, 0.0)[0];
    let axes = |m: glam::Mat4| glam::Mat3::from_cols(m.x_axis.truncate().normalize_or_zero(), m.y_axis.truncate().normalize_or_zero(), m.z_axis.truncate().normalize_or_zero());
    let (current, rest) = (axes(current), axes(rest));
    // Which way the body's up points now, given that it pointed straight up at rest.
    let up_in_bone = rest.inverse() * glam::Vec3::Z;
    let up_now = (current * up_in_bone).normalize_or_zero();
    if grounded {
        // The run swings the whole body from side to side: it leans about its own heading with each stride. That lean
        // is taken out (the lean forward stays), and then the root's turn.
        let heading = plain.x_axis.truncate();
        let heading = glam::Vec3::new(heading.x, heading.y, 0.0).normalize_or_zero();
        let side = glam::Vec3::new(heading.y, -heading.x, 0.0);
        let upright = (up_now - side * up_now.dot(side)).normalize_or_zero();
        let lean = if upright == glam::Vec3::ZERO || up_now == glam::Vec3::ZERO { glam::Quat::IDENTITY } else { glam::Quat::from_rotation_arc(up_now, upright) };
        let fwd = lean * (current * (rest.inverse() * glam::Vec3::X));
        let yaw = fwd.y.atan2(fwd.x);
        if yaw.abs() < 0.02 && lean.angle_between(glam::Quat::IDENTITY) < 0.01 {
            return glam::Mat4::IDENTITY;
        }
        let at = crate::physics::vec(&get(vm, a, "Location"));
        return glam::Mat4::from_translation(at) * glam::Mat4::from_rotation_z(-yaw) * glam::Mat4::from_quat(lean) * glam::Mat4::from_translation(-at);
    }
    if up_now == glam::Vec3::ZERO || up_now.dot(glam::Vec3::Z) > 0.9999 {
        return glam::Mat4::IDENTITY;
    }
    let turn = glam::Quat::from_rotation_arc(up_now, glam::Vec3::Z);
    let at = crate::physics::vec(&get(vm, a, "Location"));
    glam::Mat4::from_translation(at) * glam::Mat4::from_quat(turn) * glam::Mat4::from_translation(-at)
}

/// For the game log: how far the root bone's heading and up have turned from the rest pose, in degrees
/// (heading about the vertical, tilt of up), and the first bones' places, to see what an animation does to the body.
pub fn root_twist(vm: &mut Vm<'_>, a: ObjId) -> Option<(f32, f32)> {
    let mesh = get(vm, a, "Mesh").as_obj()?;
    let skel = skeleton_of(vm, mesh)?;
    let root_name = skel.skeleton.names.first().cloned()?;
    let plain = actor_matrix(vm, a);
    let current = bone_matrix_in(vm, a, &root_name, plain)?;
    let rest = plain * skel.to_actor * hp3_anim::bones(&skel.skeleton, None, None, 0.0)[0];
    let axes = |m: glam::Mat4| glam::Mat3::from_cols(m.x_axis.truncate().normalize_or_zero(), m.y_axis.truncate().normalize_or_zero(), m.z_axis.truncate().normalize_or_zero());
    let (c, r) = (axes(current), axes(rest));
    // The body's own forward at rest is the bone's axis that points along the actor's x; follow it.
    let fwd_in_bone = r.inverse() * glam::Vec3::X;
    let up_in_bone = r.inverse() * glam::Vec3::Z;
    let fwd = c * fwd_in_bone;
    let up = (c * up_in_bone).normalize_or_zero();
    let yaw = fwd.y.atan2(fwd.x).to_degrees();
    let tilt = up.dot(glam::Vec3::Z).clamp(-1.0, 1.0).acos().to_degrees();
    Some((yaw, tilt))
}

/// For the game log: the heading (degrees about the vertical, from the rest pose) of several bones, as animated and
/// as drawn (with the body correction), so a twist can be told apart from the root's.
pub fn bone_headings(vm: &mut Vm<'_>, a: ObjId) -> String {
    let Some(mesh) = get(vm, a, "Mesh").as_obj() else { return String::new() };
    let Some(skel) = skeleton_of(vm, mesh) else { return String::new() };
    let plain = actor_matrix(vm, a);
    let drawn = hang_correction(vm, a) * plain;
    let names: Vec<String> = skel.skeleton.names.clone();
    let axes = |m: glam::Mat4| glam::Mat3::from_cols(m.x_axis.truncate().normalize_or_zero(), m.y_axis.truncate().normalize_or_zero(), m.z_axis.truncate().normalize_or_zero());
    let rest_all = hp3_anim::bones(&skel.skeleton, None, None, 0.0);
    let extra = strafe_turns(vm, a);
    let mut out = format!(" extra turns {}:", extra.len());
    for n in names {
        let Some(i) = skel.skeleton.find(&n) else { continue };
        let rest = axes(plain * skel.to_actor * rest_all[i]);
        let heading = |m: glam::Mat4| {
            let f = axes(m) * (rest.inverse() * glam::Vec3::X);
            f.y.atan2(f.x).to_degrees()
        };
        let (Some(raw), Some(shown)) = (bone_matrix_in(vm, a, &n, plain), bone_matrix_extra(vm, a, &n, drawn, &extra)) else { continue };
        let (r, d) = (heading(raw), heading(shown));
        if r.abs() > 8.0 || d.abs() > 8.0 {
            out += &format!(" [{n} {r:.0}/{d:.0}]");
        }
    }
    out
}

fn bone_matrix_in(vm: &mut Vm<'_>, a: ObjId, bone: &str, actor: glam::Mat4) -> Option<glam::Mat4> {
    bone_matrix_extra(vm, a, bone, actor, &[])
}

fn bone_matrix_extra(vm: &mut Vm<'_>, a: ObjId, bone: &str, actor: glam::Mat4, extra: &[(usize, glam::Mat4)]) -> Option<glam::Mat4> {
    let mesh = get(vm, a, "Mesh").as_obj()?;
    let skel = skeleton_of(vm, mesh)?;
    let index = skel.skeleton.find(bone)?;
    let set = set_of(vm, a);
    let channels = world(vm).anim.actors.get(&a).map(|x| x.channels.clone()).unwrap_or_default();
    let base = channels.first().map(|c| hp3_anim::Base {
        sequence: c.sequence,
        frame: c.frame,
        looping: c.looping,
        from: c.from.filter(|_| c.tween_t < c.tween_total),
        weight: c.tween_weight(),
    });
    // The other channels blended over the base, limited to their bone: the same pose the renderer draws.
    let layers: Vec<hp3_anim::Layer> = channels
        .iter()
        .skip(1)
        .filter(|c| c.sequence.is_some() && c.alpha > 0.001)
        .map(|c| hp3_anim::Layer { sequence: c.sequence, frame: c.frame, alpha: c.alpha * c.tween_weight(), root: c.bone.as_deref().and_then(|b| skel.skeleton.find(b)), looping: c.looping })
        .collect();
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
            let mut turns = world(vm).anim.actors.get(&a).map(|x| turns_for(&skel.skeleton, &x.bone_turns)).unwrap_or_default();
            turns.extend_from_slice(extra);
            hp3_anim::bones_blended(&skel.skeleton, Some((set, &map)), &base.unwrap_or_default(), &layers, &turns)
        }
        None => hp3_anim::bones(&skel.skeleton, None, None, 0.0),
    };
    Some(actor * skel.to_actor * *bones.get(index)?)
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
/// What an actor's main animation channel plays, for a save: sequence name, frame, rate and whether it loops.
pub fn main_animation(vm: &mut Vm<'_>, actor: ObjId) -> Option<(String, f32, f32, bool)> {
    let ch = world(vm).anim.actors.get(&actor)?.channels.first()?.clone();
    if !ch.playing && !ch.looping {
        return None;
    }
    let set = set_of(vm, actor)?;
    Some((set.sequences.get(ch.sequence?)?.name.clone(), ch.frame, ch.rate, ch.looping))
}

/// Starts the main animation a save recorded where it was: the same sequence at the same frame and rate.
pub fn resume_main_animation(vm: &mut Vm<'_>, actor: ObjId, saved: &(String, f32, f32, bool)) {
    let name = vm.db.names.intern(&saved.0);
    let Some(set) = set_of(vm, actor) else { return };
    let Some(index) = set.find(&saved.0) else { return };
    let base = set.sequences[index].rate.max(0.001);
    if !play(vm, actor, name, saved.2 / base, 0.0, 0, saved.3) {
        return;
    }
    let ch = channel_mut(vm, actor, 0);
    ch.frame = saved.1;
}

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
        a.channels.push(Channel { alpha: 1.0, target_alpha: 1.0, ..Default::default() });
    }
    &mut a.channels[index]
}

/// Starts a sequence on a channel. Returns false when the actor has no such sequence.
fn play(vm: &mut Vm<'_>, actor: ObjId, seq: Name, rate: f32, tween: f32, channel: usize, looping: bool) -> bool {
    let Some(set) = set_of(vm, actor) else {
        if std::env::var("HP3_ANIM_TRACE").is_ok() {
            eprintln!("anim: {actor:?} has no animation set for {}", vm.db.names.get(seq));
        }
        return false;
    };
    let name = vm.db.names.get(seq).to_string();
    if std::env::var("HP3_ANIM_TRACE").is_ok() {
        eprintln!("anim: play {actor:?} {name} channel {channel} looping {looping}");
    }
    let Some(si) = set.find(&name) else {
        if std::env::var("HP3_ANIM_TRACE").is_ok() {
            eprintln!("anim: {actor:?} has no sequence {name}");
        }
        return false;
    };
    let base = set.sequences[si].rate;
    if name.to_ascii_lowercase().contains("pickup") {
        let key = set_key(&set);
        let list: Vec<(f32, Name)> = world(vm).anim.notifies.get(&(key, si)).cloned().unwrap_or_default();
        let registered: Vec<String> = list.iter().map(|(t, e)| format!("{:.1}->{}", t * set.sequences[si].frames as f32, vm.db.names.get(*e))).collect();
        vm.log.push(format!("anim: play {name} rate {rate} channel {channel} frames {} notifies {registered:?}", set.sequences[si].frames));
    }
    let ch = channel_mut(vm, actor, channel);
    // Looping the sequence that already loops keeps its place.
    if looping && ch.looping && ch.playing && ch.sequence == Some(si) {
        ch.rate = base * rate;
        return true;
    }
    // How long the fade into the new sequence takes. A one-shot played on an empty channel starts at once; a
    // tween time of -1 asks for as long as one frame of what was playing (a fortieth of a second when nothing
    // was), as the original does.
    let mut tween = tween;
    if !looping && ch.sequence.is_none() {
        tween = 0.0;
    } else if tween == -1.0 {
        let old_frames = ch.sequence.and_then(|s| set.sequences.get(s)).map_or(0.0, |s| s.frames as f32);
        let new_frames = set.sequences[si].frames.max(1) as f32;
        tween = if ch.rate > 0.0 && old_frames > 0.0 { old_frames / new_frames / ch.rate } else { 0.025 };
    }
    // The pose it leaves is faded away from over the tween time.
    let from = (tween > 0.0).then_some((ch.sequence, ch.frame, ch.looping));
    *ch = Channel {
        sequence: Some(si),
        frame: 0.0,
        rate: base * rate,
        looping,
        playing: true,
        from,
        tween_total: tween.max(0.0),
        tween_t: 0.0,
        tween_only: false,
        ..ch.clone()
    };
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
        tick_actor(vm, a, dt);
    }
}

/// Advances one actor's animation channels, sending `AnimEnd` where a sequence ends.
pub fn tick_actor(vm: &mut Vm<'_>, actor: ObjId, dt: f32) {
    let ids: Vec<ObjId> = if world(vm).anim.actors.contains_key(&actor) { vec![actor] } else { Vec::new() };
    for a in ids {
        let Some(anims) = world(vm).anim.actors.get(&a).and_then(|x| x.set.clone()) else { continue };
        let mut ended = Vec::new();
        let mut looped = false;
        let mut raised: Vec<(Name, usize)> = Vec::new();
        let key = set_key(&anims);
        let Animation { actors, notifies, .. } = &mut world(vm).anim;
        if let Some(actor) = actors.get_mut(&a) {
            for (ci, ch) in actor.channels.iter_mut().enumerate() {
                if ci > 0 && ch.alpha != ch.target_alpha {
                    let step = if ch.blend_rate > 0.0 { ch.blend_rate * dt } else { f32::MAX };
                    ch.alpha = if ch.alpha < ch.target_alpha { (ch.alpha + step).min(ch.target_alpha) } else { (ch.alpha - step).max(ch.target_alpha) };
                }
                // While a new sequence fades in it waits on its first frame: time goes to the fade, and only what is
                // left over after it plays the sequence.
                let mut dt = dt;
                if ch.tween_t < ch.tween_total {
                    let left = ch.tween_total - ch.tween_t;
                    if dt <= left {
                        ch.tween_t += dt;
                        dt = 0.0;
                    } else {
                        ch.tween_t = ch.tween_total;
                        dt -= left;
                    }
                }
                if ch.tween_only {
                    if ch.tween_t >= ch.tween_total {
                        ch.tween_only = false;
                        ch.playing = false;
                        ended.push(ci);
                    }
                    continue;
                }
                let Some(si) = ch.sequence else { continue };
                if !ch.playing {
                    continue;
                }
                let frames = anims.sequences[si].frames as f32;
                if let Some(list) = notifies.get(&(key, si)).filter(|_| !ch.ignore_notify) {
                    for (from, to) in passed_stretches(ch.frame, ch.rate * dt, frames, ch.looping) {
                        for &(at, event) in list {
                            let f = at * frames;
                            if f > from && f <= to {
                                if std::env::var_os("HP3_NOTIFY_DEBUG").is_some() {
                                    eprintln!("notify: seq #{si} {:?} frames {frames} channel {ci} from {from:.2} to {to:.2} at {f:.2}", anims.sequences[si].name);
                                }
                                raised.push((event, ci));
                            }
                        }
                    }
                }
                ch.frame += ch.rate * dt;
                if ch.frame >= frames {
                    if ch.looping {
                        ch.frame %= frames.max(1.0);
                        looped |= ci == 0;
                    } else {
                        ch.frame = frames;
                        ch.playing = false;
                        if ci > 0 && !ch.scripted_blend {
                            ch.alpha = 0.0;
                            ch.target_alpha = 0.0;
                        }
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
        for (event, channel) in raised {
            let name = vm.db.names.get(event).to_string();
            if name.to_ascii_lowercase().contains("pickup") {
                vm.log.push(format!("anim: notify {name} raised on channel {channel}"));
            }
            world(vm).anim.actors.entry(a).or_default().last_notify_channel = channel;
            if std::env::var("HP3_ANIM_TRACE").is_ok() {
                eprintln!("anim: notify {name} on {a:?} channel {channel}");
            }
            fire(vm, a, &name, &mut []);
        }
        for ci in ended {
            if std::env::var("HP3_ANIM_TRACE").is_ok() {
                let c = get(vm, a, "Controller").as_obj();
                eprintln!("anim: AnimEnd {a:?} channel {ci} controller {c:?}");
            }
            let silenced = world(vm).anim.actors.get(&a).and_then(|x| x.channels.get(ci)).is_some_and(|c| c.ignore_notify);
            if silenced {
                wake_anim_waiters(vm, a, ci);
                continue;
            }
            // A pawn's controller gets the event when it handles it; otherwise the pawn does, never both.
            match get(vm, a, "Controller").as_obj() {
                Some(c) if handles_anim_end(vm, c) => {
                    fire(vm, c, "AnimEnd", &mut [Value::Int(ci as i32)]);
                }
                _ => {
                    fire(vm, a, "AnimEnd", &mut [Value::Int(ci as i32)]);
                }
            }
            wake_anim_waiters(vm, a, ci);
        }
    }
}

/// Animation channels the movement driver owns (the script's own animations use channel 0 and 12 upward).
const TURN_RIGHT_CH: usize = 2;
const TURN_LEFT_CH: usize = 3;
const MOVE_FIRST_CH: usize = 4;
const MOVE_LAST_CH: usize = 7;

/// What the movement driver remembers about a pawn between ticks.
#[derive(Default)]
pub struct MovementState {
    started: bool,
    old_physics: u8,
    was_on_ground: bool,
    was_crouched: bool,
    was_walking: bool,
    was_moving: bool,
    old_yaw: f32,
}

fn fade_channels(vm: &mut Vm<'_>, a: ObjId, from: usize, to_inclusive: usize, target: f32, per_second: f32) {
    for ci in from..=to_inclusive {
        let ch = channel_mut(vm, a, ci);
        ch.target_alpha = target;
        ch.blend_rate = per_second;
    }
}

/// Fades a group of movement channels to nothing; once every one of them is already at nothing the group is
/// stopped (no rate, back to its first frame), so an idle pawn's hidden stride raises no notifies and no ends.
/// Behaviour of the reference engine's `ZeroMovementAlpha`.
fn zero_movement(vm: &mut Vm<'_>, a: ObjId, from: usize, to_inclusive: usize, per_second: f32) {
    let mut all_zero = true;
    for ci in from..=to_inclusive {
        let ch = channel_mut(vm, a, ci);
        if ch.alpha > 0.0 {
            all_zero = false;
        }
        ch.target_alpha = 0.0;
        ch.blend_rate = per_second;
    }
    if all_zero {
        for ci in from..=to_inclusive {
            let ch = channel_mut(vm, a, ci);
            ch.rate = 0.0;
            ch.frame = 0.0;
        }
    }
}

fn natural_rate(anims: &AnimSet, vm: &mut Vm<'_>, a: ObjId, ci: usize) -> f32 {
    let seq = world(vm).anim.actors.get(&a).and_then(|x| x.channels.get(ci)).and_then(|c| c.sequence);
    seq.and_then(|s| anims.sequences.get(s)).map_or(30.0, |s| s.rate.max(1.0))
}

/// Pawns whose movement animation the engine drives (`bPhysicsAnimUpdate`): the walk/run animations of the four
/// directions and the turn-in-place animations are kept playing on their own channels and faded in and out
/// by how the pawn is moving; the script's own animation (channel 0) is left alone. The script is told of
/// the changes it should react to (`ChangeAnimation`, `PlayJump`, `PlayFalling`, `PlayLandingAnimation`).
pub fn update_movement(vm: &mut Vm<'_>, dt: f32) {
    let Some(pawn_class) = vm.db.find("Pawn") else { return };
    let actors = world(vm).actors.clone();
    for a in actors {
        let Some(class) = vm.db.objects.get(a).class else { continue };
        if vm.db.objects.get(a).deleted || !vm.is_child_of(class, pawn_class) || !get(vm, a, "bPhysicsAnimUpdate").as_bool() {
            continue;
        }
        if get(vm, a, "bTearOff").as_bool() {
            continue;
        }
        drive_movement(vm, a, dt.max(1.0 / 240.0));
    }
}

fn drive_movement(vm: &mut Vm<'_>, a: ObjId, dt: f32) {
    let Some(anims) = set_of(vm, a) else { return };
    let blend_time = get(vm, a, "BlendChangeTime").as_float().max(0.02);
    let per_second = 1.0 / blend_time;
    let movement: Vec<Name> = (0..4).map(|i| get_indexed(vm, a, "MovementAnims", i).as_name()).collect();
    let turn_right = get(vm, a, "TurnRightAnim").as_name();
    let turn_left = get(vm, a, "TurnLeftAnim").as_name();

    // First time: get the movement channels playing, invisible until something fades them in.
    if !world(vm).anim.actors.get(&a).is_some_and(|x| x.movement.started) {
        for (ci, name) in [(TURN_RIGHT_CH, turn_right), (TURN_LEFT_CH, turn_left)].into_iter().chain((0..4).map(|i| (MOVE_FIRST_CH + i, movement[i]))) {
            if name != Name::NONE {
                play(vm, a, name, 1.0, 0.1, ci, true);
            }
            let ch = channel_mut(vm, a, ci);
            ch.alpha = 0.0;
            ch.target_alpha = 0.0;
            ch.scripted_blend = true;
            // They all play the same stride, so only the first movement channel raises its sequence's notifies
            // (a footstep once, not once per channel).
            ch.ignore_notify = ci != MOVE_FIRST_CH;
        }
        let yaw = get(vm, a, "Rotation").fields().get(1).map_or(0, Value::as_int) as f32;
        let physics = get_physics(vm, a);
        let m = &mut world(vm).anim.actors.entry(a).or_default().movement;
        m.started = true;
        m.old_physics = physics;
        m.was_on_ground = physics == crate::physics::PHYS_WALKING;
        m.old_yaw = yaw;
    }

    // Changes of state the script should know about.
    let physics = get_physics(vm, a);
    let crouched = get(vm, a, "bIsCrouched").as_bool();
    let walking = get(vm, a, "bIsWalking").as_bool();
    let vel = crate::physics::vec(&get(vm, a, "Velocity"));
    let (old_physics, was_on_ground, was_crouched, was_walking) = {
        let m = &world(vm).anim.actors.entry(a).or_default().movement;
        (m.old_physics, m.was_on_ground, m.was_crouched, m.was_walking)
    };
    let falling = physics == crate::physics::PHYS_FALLING;
    if crouched != was_crouched || walking != was_walking {
        fire(vm, a, "ChangeAnimation", &mut []);
    }
    if (old_physics == crate::physics::PHYS_FALLING) != falling {
        if falling {
            if was_on_ground && vel.z > 0.0 {
                fire(vm, a, "PlayJump", &mut []);
            } else {
                fire(vm, a, "PlayFalling", &mut []);
            }
        } else if physics == crate::physics::PHYS_WALKING {
            fire(vm, a, "PlayLandingAnimation", &mut [Value::Float(vel.z)]);
        }
    }
    if physics != old_physics && !falling {
        fire(vm, a, "ChangeAnimation", &mut []);
    }
    {
        let m = &mut world(vm).anim.actors.entry(a).or_default().movement;
        m.old_physics = physics;
        m.was_on_ground = physics == crate::physics::PHYS_WALKING;
        m.was_crouched = crouched;
        m.was_walking = walking;
    }

    // Nothing of this while falling, or just after the script asked for a pause in blending.
    let now = world(vm).time;
    if falling || now < get(vm, a, "MovementBlendStartTime").as_float() {
        zero_movement(vm, a, TURN_RIGHT_CH, MOVE_LAST_CH, per_second);
        return;
    }

    let accel = crate::physics::vec(&get(vm, a, "Acceleration")) + crate::physics::vec(&get(vm, a, "ConstantAcceleration"));
    let flat_speed2 = vel.x * vel.x + vel.y * vel.y;
    if accel.length_squared() > 1e-6 && flat_speed2 >= 25.0 {
        zero_movement(vm, a, TURN_RIGHT_CH, TURN_LEFT_CH, per_second);
        world(vm).anim.actors.entry(a).or_default().movement.was_moving = true;
        // Where the pawn is accelerating, relative to where it faces.
        let yaw = get(vm, a, "Rotation").fields().get(1).map_or(0, Value::as_int) as f32 * std::f32::consts::TAU / 65536.0;
        let look = glam::Vec3::new(yaw.cos(), yaw.sin(), 0.0);
        let left = look.cross(glam::Vec3::Z).normalize_or_zero();
        let dir = accel.normalize_or_zero();
        let (forward_pct, left_pct) = (look.dot(dir), left.dot(dir));
        let mut forward_sq: f32 = if forward_pct != 0.0 { 1.0 } else { 0.0 };
        let mut left_sq = left_pct * left_pct;
        if forward_sq > 0.0 && left_sq > 0.1 {
            let bias = get(vm, a, if forward_pct > 0.0 { "ForwardStrafeBias" } else { "BackwardStrafeBias" }).as_float();
            left_sq += bias * (1.0 - left_sq);
        }
        forward_sq = forward_sq.min(1.0);
        left_sq = left_sq.min(1.0);

        // The channels in use, and their sequences kept current (a change of the script's movement set fades over).
        let (fwd_ch, other_fwd) = if forward_pct >= 0.0 { (MOVE_FIRST_CH, MOVE_FIRST_CH + 1) } else { (MOVE_FIRST_CH + 1, MOVE_FIRST_CH) };
        let (side_ch, other_side) = if left_pct >= 0.0 { (MOVE_FIRST_CH + 2, MOVE_FIRST_CH + 3) } else { (MOVE_FIRST_CH + 3, MOVE_FIRST_CH + 2) };
        for i in 0..4 {
            let ci = MOVE_FIRST_CH + i;
            let current = world(vm).anim.actors.get(&a).and_then(|x| x.channels.get(ci)).and_then(|c| c.sequence);
            let want = anims.find(vm.db.names.get(movement[i]));
            if movement[i] != Name::NONE && want.is_some() && want != current {
                play(vm, a, movement[i], 1.0, blend_time, ci, true);
            }
        }
        // They all play at one pace: the pawn's top speed against the rate the animations were made for, at the
        // natural frame rate of the animations in use.
        let ground = get(vm, a, "GroundSpeed").as_float();
        let top = if walking { ground * get(vm, a, "WalkingPct").as_float().max(0.05) } else { ground };
        let base_rate = get(vm, a, "BaseMovementRate").as_float().max(1.0);
        let mut frame_rate = 0.0;
        let mut used = 0.0;
        if forward_pct != 0.0 {
            frame_rate += natural_rate(&anims, vm, a, fwd_ch);
            used += 1.0;
        }
        if left_pct != 0.0 {
            frame_rate += natural_rate(&anims, vm, a, side_ch);
            used += 1.0;
        }
        let pace = if used > 0.0 { top / base_rate * frame_rate / used } else { 0.0 };
        fade_channels(vm, a, fwd_ch, fwd_ch, forward_sq, per_second);
        fade_channels(vm, a, other_fwd, other_fwd, 0.0, per_second);
        fade_channels(vm, a, side_ch, side_ch, left_sq, per_second);
        fade_channels(vm, a, other_side, other_side, 0.0, per_second);
        // Keep them all on the same point of the cycle as the first.
        let lead = world(vm).anim.actors.get(&a).and_then(|x| x.channels.get(MOVE_FIRST_CH)).map(|c| (c.frame, c.sequence)).unwrap_or((0.0, None));
        let lead_fraction = lead.1.and_then(|s| anims.sequences.get(s)).map_or(0.0, |s| lead.0 / s.frames.max(1) as f32);
        for ci in MOVE_FIRST_CH..=MOVE_LAST_CH {
            let frames = world(vm).anim.actors.get(&a).and_then(|x| x.channels.get(ci)).and_then(|c| c.sequence).and_then(|s| anims.sequences.get(s)).map(|s| s.frames);
            let ch = channel_mut(vm, a, ci);
            ch.rate = pace;
            ch.playing = true;
            if ci != MOVE_FIRST_CH
                && let Some(n) = frames
            {
                ch.frame = lead_fraction * n.max(1) as f32;
            }
        }
        return;
    }

    // Standing still: the script's animation shows through; the pawn may turn on the spot.
    if std::mem::take(&mut world(vm).anim.actors.entry(a).or_default().movement.was_moving) && !falling {
        fire(vm, a, "ChangeAnimation", &mut []);
    }
    zero_movement(vm, a, MOVE_FIRST_CH, MOVE_LAST_CH, per_second);
    let yaw = (get(vm, a, "Rotation").fields().get(1).map_or(0, Value::as_int) & 0xFFFF) as f32;
    let old_yaw = world(vm).anim.actors.entry(a).or_default().movement.old_yaw;
    let mut turned = yaw - old_yaw;
    if turned > 32768.0 {
        turned -= 65536.0;
    } else if turned < -32768.0 {
        turned += 65536.0;
    }
    world(vm).anim.actors.entry(a).or_default().movement.old_yaw = yaw;
    if physics == crate::physics::PHYS_WALKING || physics == crate::physics::PHYS_SWIMMING {
        let turn_rate = turned / dt;
        for (ci, name, active) in [(TURN_RIGHT_CH, turn_right, turn_rate > 1000.0), (TURN_LEFT_CH, turn_left, turn_rate < -1000.0)] {
            if !active || name == Name::NONE {
                zero_movement(vm, a, ci, ci, per_second);
                continue;
            }
            let current = world(vm).anim.actors.get(&a).and_then(|x| x.channels.get(ci)).and_then(|c| c.sequence);
            if anims.find(vm.db.names.get(name)) != current {
                play(vm, a, name, 1.0, 0.0, ci, true);
            }
            let natural = natural_rate(&anims, vm, a, ci);
            fade_channels(vm, a, ci, ci, 1.0, per_second);
            let ch = channel_mut(vm, a, ci);
            ch.rate = natural * turn_rate.abs() / 16384.0;
            ch.playing = true;
        }
    }
}

fn get_physics(vm: &mut Vm<'_>, a: ObjId) -> u8 {
    get(vm, a, "Physics").as_byte()
}

/// Element `index` of a static array property.
fn get_indexed(vm: &mut Vm<'_>, a: ObjId, name: &str, index: usize) -> Value {
    let Some(slot) = crate::slot(vm, a, name) else { return Value::Empty };
    vm.db.objects.values(a).and_then(|v| v.get(slot + index).cloned()).unwrap_or_default()
}

fn tween_arg(a: &[Value], i: usize) -> f32 {
    match a.get(i) {
        None | Some(Value::Empty) => 0.0,
        Some(v) => v.as_float().max(0.0),
    }
}

/// Whether a controller handles `AnimEnd` itself: its current state or class defines the event beyond the
/// engine's empty one.
fn handles_anim_end(vm: &mut Vm<'_>, c: ObjId) -> bool {
    let Some(n) = vm.db.names.find("AnimEnd") else { return false };
    let Some(f) = vm.find_function(Some(c), None, n) else { return false };
    // A controller that defines nothing of its own gets the empty event of Actor.
    !vm.function_path(f).eq_ignore_ascii_case("Actor.AnimEnd")
}

/// Ends the waits of `FinishAnim` calls on this actor's channel.
fn wake_anim_waiters(vm: &mut Vm<'_>, target: ObjId, channel: usize) {
    let waiting: Vec<ObjId> = world(vm).anim.waiters.iter().filter(|w| w.1 == target && w.2 == channel).map(|w| w.0).collect();
    world(vm).anim.waiters.retain(|w| !(w.1 == target && w.2 == channel));
    for w in waiting {
        if matches!(vm.latent.get(&w), Some(Latent::Other("anim"))) {
            vm.finish_latent(w);
        }
    }
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
        Ok(Value::Bool(play(vm, o, a[0].as_name(), rate_arg(a, 1), tween_arg(a, 2), channel_arg(a, 3), false)))
    });
    vm.register("Actor.LoopAnim", |vm, this, a| {
        let Some(o) = this else { return Ok(Value::Bool(false)) };
        Ok(Value::Bool(play(vm, o, a[0].as_name(), rate_arg(a, 1), tween_arg(a, 2), channel_arg(a, 3), true)))
    });
    // TweenAnim(Sequence, Time, optional Channel): fade from the current pose to the sequence's first pose over
    // Time, then `AnimEnd`.
    vm.register("Actor.TweenAnim", |vm, this, a| {
        let Some(o) = this else { return Ok(Value::Bool(false)) };
        let time = a.get(1).map(Value::as_float).unwrap_or(0.0).max(0.0);
        let ci = channel_arg(a, 2);
        let ok = play(vm, o, a[0].as_name(), 1.0, time, ci, false);
        if ok {
            let ch = channel_mut(vm, o, ci);
            ch.rate = 0.0;
            ch.frame = 0.0;
            ch.tween_only = true;
            ch.playing = true;
            if ch.tween_total <= 0.0 {
                // No fade asked for: it is over at once.
                ch.tween_total = 0.0001;
            }
        }
        Ok(Value::Bool(ok))
    });
    // FinishAnim(optional Channel): wait for the channel's animation to end. Called on a controller it waits on
    // its pawn; a loop is stopped so it ends with its current cycle.
    vm.register("Actor.FinishAnim", |vm, this, a| {
        let Some(o) = this else { return Ok(Value::Empty) };
        let ci = channel_arg(a, 0);
        let target = if world(vm).anim.actors.contains_key(&o) { Some(o) } else { get(vm, o, "Pawn").as_obj() };
        let Some(t) = target else { return Ok(Value::Empty) };
        let playing = match world(vm).anim.actors.get_mut(&t).and_then(|x| x.channels.get_mut(ci)) {
            Some(ch) if ch.playing && ch.sequence.is_some() => {
                ch.looping = false;
                true
            }
            _ => false,
        };
        if playing {
            world(vm).anim.waiters.push((o, t, ci));
            vm.latent.insert(o, Latent::Other("anim"));
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
        let has = set_of(vm, o).is_some_and(|s| s.find(&name).is_some());
        if std::env::var("HP3_ANIM_TRACE").is_ok() {
            let mesh = get(vm, o, "Mesh").as_obj().map(|m| hp3_script::natives::object_path(vm, m));
            eprintln!("anim: HasAnim {o:?} {name} -> {has} (mesh {mesh:?})");
        }
        Ok(Value::Bool(has))
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
    // GetAnimSequence / GetAnimRate / GetAnimTime / GetAnimFrame (optional Channel): what a channel plays, its
    // speed as a multiple of the sequence's own, how far along it is (0 to 1) and at which frame.
    vm.register("Actor.GetAnimSequence", |vm, this, a| {
        let Some(o) = this else { return Ok(Value::Name(Name::NONE)) };
        let ci = channel_arg(a, 0);
        let (Some(set), Some(si)) = (set_of(vm, o), world(vm).anim.actors.get(&o).and_then(|x| x.channels.get(ci)).and_then(|c| c.sequence)) else {
            return Ok(Value::Name(Name::NONE));
        };
        Ok(Value::Name(vm.db.names.intern(&set.sequences[si].name)))
    });
    vm.register("Actor.GetAnimRate", |vm, this, a| {
        let Some(o) = this else { return Ok(Value::Float(0.0)) };
        let ci = channel_arg(a, 0);
        let (Some(set), Some(c)) = (set_of(vm, o), world(vm).anim.actors.get(&o).and_then(|x| x.channels.get(ci)).cloned()) else {
            return Ok(Value::Float(0.0));
        };
        let base = c.sequence.map(|si| set.sequences[si].rate).filter(|r| *r > 0.0).unwrap_or(1.0);
        Ok(Value::Float(if c.playing { c.rate / base } else { 0.0 }))
    });
    vm.register("Actor.GetAnimTime", |vm, this, a| {
        let Some(o) = this else { return Ok(Value::Float(0.0)) };
        let ci = channel_arg(a, 0);
        let (Some(set), Some(c)) = (set_of(vm, o), world(vm).anim.actors.get(&o).and_then(|x| x.channels.get(ci)).cloned()) else {
            return Ok(Value::Float(0.0));
        };
        let frames = c.sequence.map(|si| set.sequences[si].frames.max(1) as f32).unwrap_or(1.0);
        Ok(Value::Float((c.frame / frames).clamp(0.0, 1.0)))
    });
    vm.register("Actor.GetAnimFrame", |vm, this, a| {
        let Some(o) = this else { return Ok(Value::Float(0.0)) };
        let ci = channel_arg(a, 0);
        Ok(Value::Float(world(vm).anim.actors.get(&o).and_then(|x| x.channels.get(ci)).map(|c| c.frame).unwrap_or(0.0)))
    });
    vm.register("Actor.EnableChannelNotify", |vm, this, a| {
        if let Some(o) = this {
            let ci = channel_arg(a, 0);
            let on = a.get(1).is_none_or(|v| matches!(v, Value::Empty) || v.as_bool() || v.as_int() != 0);
            channel_mut(vm, o, ci).ignore_notify = !on;
        }
        Ok(Value::Empty)
    });
    vm.register("Actor.GetNotifyChannel", |vm, this, _a| {
        let last = this.and_then(|o| world(vm).anim.actors.get(&o)).map(|x| x.last_notify_channel).unwrap_or(0);
        Ok(Value::Int(last as i32))
    });
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
    // AnimBlendParams(Channel, DesiredStrength, InTime, OutTime, BoneName, bGlobalPose): limits the channel to a
    // bone and its children and fades it to the strength over InTime.
    vm.register("Actor.AnimBlendParams", |vm, this, a| {
        if let Some(o) = this {
            let ci = channel_arg(a, 0);
            let strength = a.get(1).map(Value::as_float).unwrap_or(0.0);
            let in_time = a.get(2).map(Value::as_float).unwrap_or(0.0);
            let bone = match a.get(4) {
                Some(Value::Name(n)) if *n != Name::NONE => Some(vm.db.names.get(*n).to_ascii_lowercase()),
                _ => None,
            };
            let ch = channel_mut(vm, o, ci);
            ch.scripted_blend = true;
            ch.bone = bone;
            ch.target_alpha = strength;
            ch.blend_rate = if in_time > 0.0 { 1.0 / in_time } else { 0.0 };
            if in_time <= 0.0 {
                ch.alpha = strength;
            }
        }
        Ok(Value::Empty)
    });
    // AnimBlendToAlpha(Channel, Alpha, TimeToBlend)
    vm.register("Actor.AnimBlendToAlpha", |vm, this, a| {
        if let Some(o) = this {
            let ci = channel_arg(a, 0);
            let alpha = a.get(1).map(Value::as_float).unwrap_or(0.0);
            let time = a.get(2).map(Value::as_float).unwrap_or(0.0);
            let ch = channel_mut(vm, o, ci);
            ch.scripted_blend = true;
            ch.target_alpha = alpha;
            ch.blend_rate = if time > 0.0 { 1.0 / time } else { 0.0 };
            if time <= 0.0 {
                ch.alpha = alpha;
            }
        }
        Ok(Value::Empty)
    });
    // AttachToBone(Attachment, BoneName)
    vm.register("Actor.AttachToBone", |vm, this, a| {
        let (Some(o), Some(child)) = (this, a.first().and_then(Value::as_obj)) else { return Ok(Value::Bool(false)) };
        let bone = vm.db.names.get(a[1].as_name()).to_ascii_lowercase();
        let found = get(vm, o, "Mesh").as_obj().and_then(|m| skeleton_of(vm, m)).is_some_and(|s| s.skeleton.find(&bone).is_some());
        if !found {
            if std::env::var("HP3_ANIM_TRACE").is_ok() {
                let mesh = get(vm, o, "Mesh").as_obj().map(|m| hp3_script::natives::object_path(vm, m));
                eprintln!("anim: AttachToBone {bone} failed on {o:?} with mesh {mesh:?}");
            }
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
    // SetBoneRotation(BoneName, rotator BoneTurn, optional int Space, optional float Alpha): a rotation added
    // to the bone in its own space (the space argument is not told apart yet).
    vm.register("Actor.SetBoneRotation", |vm, this, a| {
        let Some(o) = this else { return Ok(Value::Empty) };
        let bone = vm.db.names.get(a[0].as_name()).to_ascii_lowercase();
        let r: Vec<i32> = a.get(1).map(|v| v.fields().iter().map(Value::as_int).collect()).unwrap_or_default();
        let rot = [r.first().copied().unwrap_or(0), r.get(1).copied().unwrap_or(0), r.get(2).copied().unwrap_or(0)];
        let alpha = match a.get(3) {
            None | Some(Value::Empty) => 1.0,
            Some(v) => v.as_float(),
        };
        let x = world(vm).anim.actors.entry(o).or_default();
        x.bone_turns.retain(|t| t.0 != bone);
        if alpha > 0.0 && rot != [0, 0, 0] {
            x.bone_turns.push((bone, rot, alpha));
        }
        Ok(Value::Empty)
    });
    // SetAnimFrame(Time, optional Channel, optional UnitFlag): a channel's position, as a fraction of the
    // sequence (or in frames with UnitFlag 1).
    vm.register("Actor.SetAnimFrame", |vm, this, a| {
        if let Some(o) = this {
            let t = a.first().map(Value::as_float).unwrap_or(0.0);
            let ci = channel_arg(a, 1);
            let in_frames = a.get(2).is_some_and(|v| v.as_int() == 1);
            let frames = set_of(vm, o).and_then(|s| {
                let si = world(vm).anim.actors.get(&o)?.channels.get(ci)?.sequence?;
                Some(s.sequences[si].frames as f32)
            });
            if let Some(f) = frames {
                // HP3's own scripts pass a frame number (the pickup's start frame of 1) without the unit flag; a time
                // of one whole or more cannot be a share of the sequence, so it is read as frames.
                channel_mut(vm, o, ci).frame = if in_frames || t >= 1.0 { t.min(f) } else { t * f };
            }
        }
        Ok(Value::Empty)
    });
    // GetAnimNumFrames(Sequence): its frame count (0 when the actor has no such sequence).
    vm.register("Actor.GetAnimNumFrames", |vm, this, a| {
        let Some(o) = this else { return Ok(Value::Int(0)) };
        let name = vm.db.names.get(a[0].as_name()).to_string();
        let frames = set_of(vm, o).and_then(|s| s.find(&name).map(|i| s.sequences[i].frames)).unwrap_or(0);
        Ok(Value::Int(frames))
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
    // `AddNotify(AnimSet, Sequence, Frame, EventName)`: the sequence raises the event on its actor at that frame.
    // Adding the same one twice changes nothing.
    vm.register("Actor.AddNotify", |vm, _this, a| {
        let (Some(anim), Some(seq)) = (a.first().and_then(Value::as_obj), a.get(1)) else { return Ok(Value::Empty) };
        let Some(set) = load_set(vm, anim) else { return Ok(Value::Empty) };
        let name = vm.db.names.get(seq.as_name()).to_string();
        let Some(si) = set.find(&name) else { return Ok(Value::Empty) };
        let frames = set.sequences[si].frames as f32;
        if frames <= 0.0 {
            return Ok(Value::Empty);
        }
        let at = (a.get(2).map(Value::as_float).unwrap_or(0.0) / frames).clamp(0.0, 1.0);
        let event = a.get(3).map(Value::as_name).unwrap_or(Name::NONE);
        let list = world(vm).anim.notifies.entry((set_key(&set), si)).or_default();
        if !list.iter().any(|&(t, e)| e == event && (t - at).abs() < 0.0001) {
            list.push((at, event));
            if std::env::var("HP3_ANIM_TRACE").is_ok() {
                eprintln!("anim: notify {name} at {at:.3} -> {} (set {:#x})", vm.db.names.get(event), set_key(&set));
            }
        }
        Ok(Value::Empty)
    });
    vm.register("Actor.AddNotifyObj", |_vm, _this, _a| Ok(Value::Empty));
    // The animation set an actor plays a named sequence from.
    vm.register("Actor.GetAnimObjectByName", |vm, this, a| {
        let Some(o) = this else { return Ok(Value::Object(None)) };
        let Some(set) = set_of(vm, o) else { return Ok(Value::Object(None)) };
        let name = vm.db.names.get(a.first().map(Value::as_name).unwrap_or(Name::NONE)).to_string();
        if set.find(&name).is_none() {
            return Ok(Value::Object(None));
        }
        let object = world(vm).anim.sets.iter().find(|(_, s)| s.as_ref().is_some_and(|s| Rc::ptr_eq(s, &set))).map(|(&k, _)| k);
        Ok(Value::Object(object))
    });
    vm.register("Actor.LinkSkelAnim", |vm, this, a| {
        if let (Some(o), Some(anim)) = (this, a.first().and_then(Value::as_obj)) {
            let s = load_set(vm, anim);
            world(vm).anim.actors.entry(o).or_default().set = s;
        }
        Ok(Value::Empty)
    });
}
