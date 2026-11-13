//! Actor physics: walking, falling, flying, projectiles and rotation, moving through `Collision`.
//!
//! Written from standard Unreal movement rules (ground friction, step-up, sliding, landing) and HP3's step
//! height of 24; NOT yet verified against HP3's Engine.dll. Values will be tuned against movement traces
//! recorded from the retail game.

use glam::Vec3;
use hp3_script::{ObjId, Value, Vm};

use crate::collision::{ActorBox, Hit};
use crate::{fire, get, set, world};

pub const PHYS_NONE: u8 = 0;
pub const PHYS_WALKING: u8 = 1;
pub const PHYS_FALLING: u8 = 2;
pub const PHYS_SWIMMING: u8 = 3;
pub const PHYS_FLYING: u8 = 4;
pub const PHYS_ROTATING: u8 = 5;
pub const PHYS_PROJECTILE: u8 = 6;
pub const PHYS_TRAILER: u8 = 10;
pub const PHYS_INTERPOLATING: u8 = 7;

/// HP3's maximum step height.
pub const MAX_STEP_HEIGHT: f32 = 24.0;
/// Floors steeper than this (normal Z below it) are walls.
pub const WALKABLE_Z: f32 = 0.7;
const DEFAULT_GRAVITY: f32 = -950.0;

pub fn vec(v: &Value) -> Vec3 {
    let f = v.fields();
    Vec3::new(
        f.first().map(Value::as_float).unwrap_or(0.0),
        f.get(1).map(Value::as_float).unwrap_or(0.0),
        f.get(2).map(Value::as_float).unwrap_or(0.0),
    )
}

pub fn value(v: Vec3) -> Value {
    Value::Struct(Box::new([Value::Float(v.x), Value::Float(v.y), Value::Float(v.z)]))
}

fn extent(vm: &mut Vm<'_>, a: ObjId) -> Vec3 {
    let r = get(vm, a, "CollisionRadius").as_float();
    let h = get(vm, a, "CollisionHeight").as_float();
    Vec3::new(r, r, h)
}

/// The widest a digable object blocks: the rabbit (radius 10) then stands at 40, inside the 45 (60 when facing it) that its `CanDigMe` allows.
const DIG_BLOCK_RADIUS: f32 = 30.0;

fn is_digable(vm: &mut Vm<'_>, a: ObjId) -> bool {
    let class = vm.db.objects.get(a).class;
    vm.db.find("LapiforDigableObject").zip(class).is_some_and(|(k, c)| vm.is_child_of(c, k))
}

/// `Actor.CollideType` value of a box (`CT_Box`; 0 is `CT_Cylinder`).
const CT_BOX: u8 = 1;

/// Rebuilds the boxes of actors that block movement (static meshes collide by their triangles instead).
fn refresh_actor_boxes(vm: &mut Vm<'_>) {
    let actors = std::mem::take(&mut world(vm).actors);
    let s = Slots::get(vm);
    let (collide_type, collision_width, rotation) = (crate::actor_slot(vm, "CollideType"), crate::actor_slot(vm, "CollisionWidth"), crate::actor_slot(vm, "Rotation"));
    let use_cylinder = crate::actor_slot(vm, "bUseCylinderCollision");
    let mut boxes = Vec::new();
    // Which actors have a box at all is worked out again only for actors written since the last tick.
    let mut memo = std::mem::take(&mut crate::world(vm).memo_boxes);
    let pass = memo.begin(&mut vm.db.objects);
    for &a in &actors {
        if memo.get(&pass, &vm.db.objects, a) == Some(1) {
            continue;
        }
        let blocks_actors = crate::read(vm, a, s.block).as_bool();
        let blocks_players = crate::read(vm, a, s.block_players).as_bool();
        let (r, h) = (crate::read(vm, a, s.radius).as_float(), crate::read(vm, a, s.height).as_float());
        // As the original picks an actor's collision: a `CollideType` box first, whatever it draws; then a static mesh by its
        // triangles (unless it asks for its cylinder); anything else is its cylinder.
        let ct_box = crate::read(vm, a, collide_type).as_byte() == CT_BOX && crate::collision_rules_new();
        let has_box = crate::read(vm, a, s.collide).as_bool()
            && (blocks_actors || blocks_players)
            && (ct_box || crate::read(vm, a, s.static_mesh).as_obj().is_none() || crate::read(vm, a, use_cylinder).as_bool())
            && r > 0.0
            && h > 0.0;
        memo.set(a, if has_box { 2 } else { 1 });
        if !has_box {
            continue;
        }
        // `CollideType` box (1): a box turned with the actor, half sizes radius (x), width (y, the radius when 0) and height (z): the
        // invisible walls and ramps of the levels. Anything else is an upright cylinder.
        let is_box = crate::read(vm, a, collide_type).as_byte() == CT_BOX;
        if is_box {
            let width = crate::read(vm, a, collision_width).as_float();
            let rot: Vec<i32> = crate::read(vm, a, rotation).fields().iter().map(Value::as_int).collect();
            let ax = hp3_script::natives::rot_axes([rot.first().copied().unwrap_or(0), rot.get(1).copied().unwrap_or(0), rot.get(2).copied().unwrap_or(0)]);
            let axes = [Vec3::from(ax[0]), Vec3::from(ax[1]), Vec3::from(ax[2])];
            let y = if width > 0.0 { width } else { r };
            boxes.push(ActorBox { actor: a, center: crate::read_vec(vm, a, s.location), extent: Vec3::new(r, y, h), blocks_actors, blocks_players, is_pawn: is_pawn(vm, a), owner: crate::read(vm, a, s.owner).as_obj(), axes: Some(axes) });
            continue;
        }
        // A mound the rabbit digs is dug from up to 60 away, so its wall must let the rabbit within that (radius 72 would keep it at 82).
        let r = if r > DIG_BLOCK_RADIUS && is_digable(vm, a) { DIG_BLOCK_RADIUS } else { r };
        boxes.push(ActorBox { actor: a, center: crate::read_vec(vm, a, s.location), extent: Vec3::new(r, r, h), blocks_actors, blocks_players, is_pawn: is_pawn(vm, a), owner: crate::read(vm, a, s.owner).as_obj(), axes: None });
    }
    crate::world(vm).memo_boxes = memo;
    world(vm).actors = actors;
    world(vm).collision.actors = boxes;
}

/// Slots of the Actor properties the per-tick loops read.
#[derive(Clone, Copy)]
pub struct Slots {
    collide: Option<usize>,
    block: Option<usize>,
    block_players: Option<usize>,
    static_mesh: Option<usize>,
    radius: Option<usize>,
    height: Option<usize>,
    location: Option<usize>,
    physics: Option<usize>,
    is_static: Option<usize>,
    owner: Option<usize>,
    velocity: Option<usize>,
}

impl Slots {
    fn get(vm: &mut Vm<'_>) -> Slots {
        Slots {
            collide: crate::actor_slot(vm, "bCollideActors"),
            block: crate::actor_slot(vm, "bBlockActors"),
            block_players: crate::actor_slot(vm, "bBlockPlayers"),
            static_mesh: crate::actor_slot(vm, "StaticMesh"),
            radius: crate::actor_slot(vm, "CollisionRadius"),
            height: crate::actor_slot(vm, "CollisionHeight"),
            location: crate::actor_slot(vm, "Location"),
            physics: crate::actor_slot(vm, "Physics"),
            is_static: crate::actor_slot(vm, "bStatic"),
            owner: crate::actor_slot(vm, "Owner"),
            velocity: crate::actor_slot(vm, "Velocity"),
        }
    }
}

/// Moves `a` by `delta`, stopping at the first hit. Returns the hit.
pub fn move_actor(vm: &mut Vm<'_>, a: ObjId, delta: Vec3) -> Option<Hit> {
    if delta.length_squared() < 1e-8 {
        return None;
    }
    let loc = vec(&get(vm, a, "Location"));
    world(vm).collision.mover_owner = get(vm, a, "Owner").as_obj();
    // A ghost (the debug mode) has no collision at all.
    if is_player_pawn(vm, a) && !get(vm, a, "bCollideWorld").as_bool() && !get(vm, a, "bCollideActors").as_bool() {
        set(vm, a, "Location", value(loc + delta));
        return None;
    }
    let ext = extent(vm, a);
    let player = blocked_as_player(vm, a);
    // The level stops an actor only with `bCollideWorld`, and other actors only with `bCollideActors` and
    // `bBlockActors` (a spell flies through the creatures it touches, and a pawn in the middle of a climb has all
    // of them off so it can pass through what it climbs).
    let how = crate::collision::Mover {
        player,
        blocks_actors: get(vm, a, "bBlockActors").as_bool() && get(vm, a, "bCollideActors").as_bool(),
        collide_world: get(vm, a, "bCollideWorld").as_bool(),
    };
    let mut loc = loc;
    let whole = delta.length();
    let mut delta = delta;
    // How much of the move is already made (moving off a surface the box starts inside).
    let mut made = 0.0f32;
    let mut hit = world(vm).collision.trace_for(how, loc, loc + delta, ext, Some(a));
    // A box that only touches a surface and moves along it (up the face of a riser) is not stopped by it. Swept
    // again with a box a hair smaller, the touch is gone and only what really lies in the way remains.
    if let Some(h) = hit
        && h.time * whole <= 0.02
        && delta.dot(h.normal) >= -1e-3 * whole
    {
        // A wall the collision keeps a margin around may need a bigger hair.
        for shrink in [0.2f32, 1.0, 3.0] {
            let smaller = Vec3::new((ext.x - shrink).max(0.5), (ext.y - shrink).max(0.5), (ext.z - shrink).max(0.5));
            hit = world(vm).collision.trace_for(how, loc, loc + delta, smaller, Some(a));
            if hit.is_none_or(|h| h.time * whole > 0.02) {
                break;
            }
        }
    }
    // Blocked before moving at all: the box overlaps what it touches by a hair (a sloped hull, a rounding of the
    // last step). Nudge it out along the contact's normal, as far as it takes to be clear (a few tries), and
    // sweep again, so a pawn does not stay glued to a surface it only grazes.
    for _ in 0..10 {
        let reach = delta.length();
        let Some(h) = hit.filter(|h| h.time * reach <= 0.02 && h.normal.length_squared() > 0.5) else { break };
        let moving_away = delta.dot(h.normal) > 0.0;
        if moving_away {
            // Starting inside a surface it is leaving (a tread a hair above the feet, as it goes up): it goes on,
            // half a unit at a time, until it is out, and the rest of the move is swept from there.
            let step = delta.normalize_or_zero() * reach.min(0.5);
            loc += step;
            made += step.length();
            delta -= step;
            set(vm, a, "Location", value(loc));
            if delta.length_squared() < 1e-8 {
                hit = None;
                break;
            }
            hit = world(vm).collision.trace_for(how, loc, loc + delta, ext, Some(a));
            continue;
        }
        // Only a box that is really inside something is pushed out; one that merely touches the surface is blocked
        // and stays (a pawn pushed back from a wall every tick never builds up speed to climb it). A slightly
        // smaller box tells the two apart.
        let smaller = Vec3::new((ext.x - 0.2).max(0.5), (ext.y - 0.2).max(0.5), (ext.z - 0.2).max(0.5));
        let inside = world(vm).collision.trace_for(how, loc + Vec3::Z * 0.01, loc, smaller, Some(a)).is_some_and(|c| c.time <= 0.0);
        if player && delta.z < -20.0 {
            vm.log.push(format!("stuckfall: delta {delta} hit normal {} inside {inside} at {loc}", h.normal));
        }
        if !inside {
            break;
        }
        // A pawn sunk further into a surface needs a bigger push: try 0.4, then 1, 2, 4 and 8 units.
        let mut freed = None;
        for push in [0.4f32, 1.0, 2.0, 4.0, 8.0] {
            let out = loc + h.normal * push;
            if world(vm).collision.trace_for(how, out + Vec3::Z * 0.01, out, ext, Some(a)).is_none_or(|c| c.time > 0.0) {
                freed = Some(out);
                break;
            }
        }
        // Wedged where the contact's normal leads only deeper into geometry: any other way out will do.
        if freed.is_none() {
            'search: for push in [2.0f32, 4.0, 8.0, 16.0, 32.0] {
                for dir in [Vec3::Z, Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y] {
                    let out = loc + dir * push;
                    if world(vm).collision.trace_for(how, out + Vec3::Z * 0.01, out, ext, Some(a)).is_none_or(|c| c.time > 0.0) {
                        freed = Some(out);
                        break 'search;
                    }
                }
            }
        }
        let Some(out) = freed else { break };
        if player {
            vm.log.push(format!("nudge: pushed out of a surface with normal ({:.2},{:.2},{:.2}) by {:.2}", h.normal.x, h.normal.y, h.normal.z, (out - loc).length()));
        }
        loc = out;
        set(vm, a, "Location", value(loc));
        hit = world(vm).collision.trace_for(how, loc, loc + delta, ext, Some(a));
    }
    let time = hit.map(|h| h.time).unwrap_or(1.0);
    set(vm, a, "Location", value(loc + delta * time));
    // The hit's time is a share of the whole move asked for, not of what was left of it.
    if made > 0.0
        && let Some(h) = hit.as_mut()
        && whole > 0.0
    {
        h.time = (made + h.time * delta.length()) / whole;
    }
    // Running into an actor (not the level's geometry, not what it stands on) bumps both of them.
    if let Some(other) = hit.and_then(|h| h.actor)
        && !get(vm, other, "bWorldGeometry").as_bool()
        && !stands_on(vm, a, other)
    {
        notify_bump(vm, other, a);
        notify_bump(vm, a, other);
    }
    hit
}

/// Whether `a` is based (directly or through other bases) on `other`.
fn stands_on(vm: &mut Vm<'_>, a: ObjId, other: ObjId) -> bool {
    let mut at = get(vm, a, "Base").as_obj();
    for _ in 0..16 {
        match at {
            Some(b) if b == other => return true,
            Some(b) => at = get(vm, b, "Base").as_obj(),
            None => return false,
        }
    }
    false
}

/// `who` was run into by `other`: a pawn's controller hears `NotifyBump` first and may take it; otherwise `Bump`.
fn notify_bump(vm: &mut Vm<'_>, who: ObjId, other: ObjId) {
    if vm.db.objects.get(who).deleted || vm.db.objects.get(other).deleted {
        return;
    }
    if is_pawn(vm, who)
        && let Some(c) = get(vm, who, "Controller").as_obj()
    {
        let taken = fire(vm, c, "NotifyBump", &mut [Value::Object(Some(other))]).as_bool();
        // HP3_BUMP_LOG: every bump between two pawns, with the bumped pawn's controller and whether it took the bump.
        if crate::envc::var_os("HP3_BUMP_LOG").is_some() && is_pawn(vm, other) {
            let state = vm.state_of(c).map(|s| vm.db.names.get(vm.db.get(s).name).to_string()).unwrap_or_default();
            vm.log.push(format!("bump: {} bumped by {}: controller {} state '{state}' taken {taken}", hp3_script::natives::object_path(vm, who), hp3_script::natives::object_path(vm, other), hp3_script::natives::object_path(vm, c)));
        }
        if taken {
            return;
        }
    } else if crate::envc::var_os("HP3_BUMP_LOG").is_some() && is_pawn(vm, who) && is_pawn(vm, other) {
        vm.log.push(format!("bump: {} bumped by {}: no controller", hp3_script::natives::object_path(vm, who), hp3_script::natives::object_path(vm, other)));
    }
    fire(vm, who, "Bump", &mut [Value::Object(Some(other))]);
}

/// Moves along `delta`, sliding along what it hits (at most twice).
pub(crate) fn move_and_slide(vm: &mut Vm<'_>, a: ObjId, delta: Vec3) -> Option<Hit> {
    let mut remaining = delta;
    let mut first = None;
    for _ in 0..3 {
        match move_actor(vm, a, remaining) {
            None => break,
            Some(h) => {
                if crate::envc::var_os("HP3_FALL_DEBUG").is_some() {
                    eprintln!("slide: remaining {remaining} hit t {} n {} actor {:?} at {}", h.time, h.normal, h.actor.is_some(), vec(&get(vm, a, "Location")));
                }
                first.get_or_insert(h);
                let left = remaining * (1.0 - h.time);
                remaining = left - h.normal * left.dot(h.normal);
                if remaining.length_squared() < 1.0e-6 {
                    break;
                }
            }
        }
    }
    first
}

/// For the game log: what a pawn that pushes but does not move has around it (what its box meets ahead, above and
/// below, and the surface under its middle).
pub fn debug_blocked(vm: &mut Vm<'_>, a: ObjId) -> String {
    let loc = vec(&get(vm, a, "Location"));
    let accel = vec(&get(vm, a, "Acceleration"));
    let ahead = Vec3::new(accel.x, accel.y, 0.0).normalize_or_zero() * 24.0;
    let ext = extent(vm, a);
    let player = blocked_as_player(vm, a);
    let describe = |vm: &mut Vm<'_>, from: Vec3, to: Vec3, e: Vec3| -> String {
        match world(vm).collision.trace_as(player, from, to, e, Some(a)) {
            Some(h) => {
                let who = h.actor.map(|o| hp3_script::natives::object_path(vm, o));
                format!("hit t={:.3} normal ({:.2},{:.2},{:.2}) {who:?}", h.time, h.normal.x, h.normal.y, h.normal.z)
            }
            None => "free".to_string(),
        }
    };
    let forward = describe(vm, loc, loc + ahead, ext);
    let up = describe(vm, loc, loc + Vec3::Z * (MAX_STEP_HEIGHT + 2.0), ext);
    let over = describe(vm, loc + Vec3::Z * (MAX_STEP_HEIGHT + 2.0), loc + Vec3::Z * (MAX_STEP_HEIGHT + 2.0) + ahead, ext);
    let down = describe(vm, loc, loc - Vec3::Z * (MAX_STEP_HEIGHT + 2.0), ext);
    let middle = describe(vm, loc, loc - Vec3::Z * (MAX_STEP_HEIGHT + 2.0), Vec3::new(2.0, 2.0, ext.z));
    format!("box {ext} | ahead: {forward} | up: {up} | over: {over} | down: {down} | under middle: {middle}")
}

/// For the game log: what is under a pawn and around it (after a ledge climb), to see why it stands or falls.
pub fn debug_floor(vm: &mut Vm<'_>, a: ObjId) -> String {
    let loc = vec(&get(vm, a, "Location"));
    let ext = extent(vm, a);
    let phys = get(vm, a, "Physics").as_byte();
    let base = get(vm, a, "Base").as_obj().map(|b| hp3_script::natives::object_path(vm, b));
    let floor = floor_below(vm, a, MAX_STEP_HEIGHT + 2.0).map(|f| (f.time, f.normal.z, f.actor.map(|o| hp3_script::natives::object_path(vm, o))));
    let raw = world(vm).collision.trace_as(true, loc, loc - Vec3::Z * 60.0, ext, Some(a)).map(|h| (h.time, h.normal, h.actor.map(|o| hp3_script::natives::object_path(vm, o))));
    let inside = world(vm).collision.box_overlaps_triangles(loc, ext - Vec3::splat(0.5), Some(a));
    let touching: Vec<String> = match get(vm, a, "Touching") {
        Value::Array(v) => v.iter().filter_map(Value::as_obj).map(|o| hp3_script::natives::object_path(vm, o)).collect(),
        _ => Vec::new(),
    };
    let flags = (get(vm, a, "bCollideWorld").as_bool(), get(vm, a, "bCollideActors").as_bool(), get(vm, a, "bBlockActors").as_bool());
    let near: Vec<String> = world(vm)
        .collision
        .mover_boxes()
        .into_iter()
        .filter(|(_, lo, hi)| loc.x > lo.x - 120.0 && loc.x < hi.x + 120.0 && loc.y > lo.y - 120.0 && loc.y < hi.y + 120.0 && loc.z > lo.z - 200.0 && loc.z < hi.z + 200.0)
        .map(|(o, lo, hi)| format!("{} box [{:.0},{:.0},{:.0}]-[{:.0},{:.0},{:.0}]", hp3_script::natives::object_path(vm, o), lo.x, lo.y, lo.z, hi.x, hi.y, hi.z))
        .collect();
    let heading = rotator_direction(&get(vm, a, "Rotation"));
    let ahead = world(vm).collision.trace_as(true, loc, loc + Vec3::new(heading.x, heading.y, 0.0).normalize_or_zero() * 24.0, ext, Some(a)).map(|h| (h.time, h.normal, h.actor.map(|o| hp3_script::natives::object_path(vm, o))));
    format!("ahead {ahead:?} movers {near:?} at {loc} phys {phys} base {base:?} floor {floor:?} raw {raw:?} inside-mesh {inside} collide(world,actors,block) {flags:?} touching {touching:?}")
}

/// Why `floor_below` found no floor (debugging): the sweeps it makes and what each met.
fn floor_report(vm: &mut Vm<'_>, a: ObjId) -> String {
    let loc = vec(&get(vm, a, "Location"));
    let ext = extent(vm, a);
    let player = blocked_as_player(vm, a);
    let depth = MAX_STEP_HEIGHT + 2.0;
    let mut out = format!("at {loc} ext {ext}");
    for (name, e) in [("box", ext), ("narrow2", Vec3::new(ext.x - 2.0, ext.y - 2.0, ext.z)), ("narrow5", Vec3::new(ext.x - 5.0, ext.y - 5.0, ext.z)), ("thin", Vec3::new(2.0, 2.0, ext.z))] {
        let h = world(vm).collision.trace_as(player, loc, loc - Vec3::Z * depth, e, Some(a));
        out += &format!(" {name} {:?}", h.map(|h| (h.time, h.normal)));
    }
    let deep = world(vm).collision.trace_as(player, loc, loc - Vec3::Z * 200.0, Vec3::new(2.0, 2.0, ext.z), Some(a));
    out += &format!(" deep-thin {:?}", deep.map(|h| (h.time * 200.0, h.normal)));
    out
}

/// The least `normal.z` a floor has to have for a pawn to stand on it. HP3's heroes set `fMinFloorZ` to 0.01 while they
/// slide (`StartSliding`), so the whole track holds them however steep it is, and put it back after (`EndSliding`).
pub(crate) fn min_floor_z(vm: &mut Vm<'_>, a: ObjId) -> f32 {
    if get(vm, a, "bIsSliding").as_bool() {
        let z = get(vm, a, "fMinFloorZ").as_float();
        if z > 0.0 && z < WALKABLE_Z {
            return z;
        }
    }
    WALKABLE_Z
}

fn floor_below(vm: &mut Vm<'_>, a: ObjId, depth: f32) -> Option<Hit> {
    let wz = min_floor_z(vm, a);
    let loc = vec(&get(vm, a, "Location"));
    let ext = extent(vm, a);
    let player = blocked_as_player(vm, a);
    // A box pressed into a wall by a hair has the wall at the very start of its downward sweep, and finds no floor.
    // Narrower boxes are tried until one is clear of the wall.
    let mut hit = None;
    for shrink in [0.0, 2.0, 5.0] {
        let narrow = Vec3::new((ext.x - shrink).max(1.0), (ext.y - shrink).max(1.0), ext.z);
        let found = world(vm).collision.trace_as(player, loc, loc - Vec3::Z * depth, narrow, Some(a))?;
        let embedded_in_wall = found.time <= 0.0 && found.normal.z < wz;
        hit = Some(found);
        if !embedded_in_wall {
            break;
        }
    }
    // A pawn on a tread, with its box just over a riser (the corner of a stair), can still be embedded in that riser's face
    // however narrow the box: it then looks for the floor from a hair higher, which clears the face, and the result is turned
    // back into time along the original sweep.
    if hit.is_some_and(|h| h.time <= 0.0 && h.normal.z < wz) {
        // (The sweep pushes the planes out by 1.1 times the box, so at a corner the box's bottom can sit about 4 units in.)
        for lift in [1.0, 2.0, 4.0, 5.0] {
            let found = world(vm).collision.trace_as(player, loc + Vec3::Z * lift, loc - Vec3::Z * depth, ext, Some(a));
            if crate::envc::var("HP3_FLOOR_DEBUG").is_ok_and(|n| hp3_script::natives::object_path(vm, a).ends_with(&n)) {
                eprintln!("floor retry: frame {} at {loc} lift {lift} -> {:?}", world(vm).frame, found.map(|f| (f.time, f.normal, f.location)));
            }
            let Some(found) = found else { continue };
            if found.time <= 0.0 && found.normal.z < wz {
                continue;
            }
            let drop = found.time * (depth + lift) - lift;
            // A box coming straight down cannot meet a riser's side: landing with an upright normal is the corner of a tread, and
            // the tread is what it came down on.
            let normal = if found.normal.z < wz && found.normal.z.abs() < 0.1 { Vec3::Z } else { found.normal };
            hit = Some(Hit { time: (drop / depth).max(0.0), normal, ..found });
            break;
        }
    }
    let hit = hit?;
    // Going down a stair, the pawn's middle passes the edge of the tread it stands on while its box still rests on it: the sweep
    // down meets the tread's corner half a unit below and reports the riser's upright normal (the thin box under the middle too).
    // A box moving straight down cannot run into an upright face, so this is the tread's top, as the original's round foot finds
    // it: the pawn stays on the tread until its box is past the edge, then drops a whole step onto the next one in one tick
    // (which the stair smoothing turns into a ramp). Taken for a wall, it walked off every tread and fell (and grunted) on landing.
    let hit = if hit.time > 0.0 && hit.normal.z.abs() < 0.1 { Hit { normal: Vec3::Z, ..hit } } else { hit };
    let normal = surface_normal(vm, a, &hit, loc, depth);
    if crate::envc::var("HP3_FLOOR_DEBUG").is_ok_and(|n| hp3_script::natives::object_path(vm, a).ends_with(&n)) {
        eprintln!("floor result: frame {} hit time {} normal {} -> surface normal {normal}", world(vm).frame, hit.time, hit.normal);
    }
    (normal.z >= wz).then_some(Hit { normal, ..hit })
}

/// Puts a walking pawn that sits a little inside the floor (a saved game can hold it so) back on top of it: with no floor
/// under it, but one within a step's height above, it is lifted onto that floor. Without this its first look at the floor
/// finds none and it falls out of the level.
pub fn lift_out_of_floor(vm: &mut Vm<'_>, a: ObjId) {
    if get(vm, a, "Physics").as_byte() != PHYS_WALKING || get(vm, a, "Controller").as_obj().is_none() || floor_below(vm, a, 50.0).is_some() {
        return;
    }
    let start = get(vm, a, "Location");
    let loc = vec(&start);
    for lift in [4.0, 8.0, 16.0, MAX_STEP_HEIGHT] {
        set(vm, a, "Location", value(loc + Vec3::Z * lift));
        if let Some(floor) = floor_below(vm, a, lift + 2.0) {
            set(vm, a, "Location", value(floor.location));
            return;
        }
    }
    set(vm, a, "Location", start);
}

/// The normal of the surface a pawn's box came down on. The box meets the edge or corner of a step or a bevelled
/// tread as often as its flat top, and then reports a slanted normal; the surface it rests on is what lies under
/// its middle, so a thin box dropped from `from` over the same depth tells it.
fn surface_normal(vm: &mut Vm<'_>, a: ObjId, hit: &Hit, from: Vec3, depth: f32) -> Vec3 {
    let wz = min_floor_z(vm, a);
    if hit.normal.z >= wz {
        return hit.normal;
    }
    let ext = extent(vm, a);
    let thin = Vec3::new(2.0, 2.0, ext.z);
    let player = blocked_as_player(vm, a);
    if let Some(under) = world(vm).collision.trace_as(player, from, from - Vec3::Z * depth, thin, Some(a))
        && under.normal.z >= wz
    {
        return under.normal;
    }
    // Standing on a tread with its middle right at the riser's edge, even the thin box is inside the riser's plane region: from a
    // little higher it is clear of it and finds the tread.
    for lift in [1.0, 2.0, 4.0, 5.0] {
        if let Some(under) = world(vm).collision.trace_as(player, from + Vec3::Z * lift, from - Vec3::Z * depth, thin, Some(a))
            && under.normal.z >= wz
        {
            return under.normal;
        }
    }
    hit.normal
}

/// Whether `a` is the pawn the local player controls (what `bBlockPlayers` actors block).
pub(crate) fn is_player_pawn(vm: &mut Vm<'_>, a: ObjId) -> bool {
    let Some(pc) = world(vm).player else { return false };
    get(vm, pc, "Pawn").as_obj() == Some(a)
}

/// Whether what `bBlockPlayers` stops stops `a`: the pawn the player controls, except the rabbit and the dragon the
/// player has turned into, which go where the heroes may not.
fn blocked_as_player(vm: &mut Vm<'_>, a: ObjId) -> bool {
    if !is_player_pawn(vm, a) {
        return false;
    }
    let class = vm.db.objects.get(a).class;
    let animal = ["LapiforPawn", "Draconisfors"].iter().any(|n| vm.db.find(n).zip(class).is_some_and(|(k, c)| vm.is_child_of(c, k)));
    !animal
}

fn gravity(vm: &mut Vm<'_>, a: ObjId) -> f32 {
    let volume = get(vm, a, "PhysicsVolume").as_obj();
    match volume {
        Some(v) => {
            let g = vec(&get(vm, v, "Gravity"));
            if g.z != 0.0 { g.z } else { DEFAULT_GRAVITY }
        }
        None => DEFAULT_GRAVITY,
    }
}

/// Ground friction of the volume a pawn stands in (8 where nothing says otherwise).
fn ground_friction(vm: &mut Vm<'_>, a: ObjId) -> f32 {
    match get(vm, a, "PhysicsVolume").as_obj() {
        Some(v) => match get(vm, v, "GroundFriction").as_float() {
            f if f > 0.0 => f,
            _ => 8.0,
        },
        None => 8.0,
    }
}

/// The velocity of a walking pawn after `dt` seconds of its acceleration.
///
/// With nothing pushing it brakes (twice the friction, and it stops dead when slow) instead of drifting to a
/// stop. With acceleration, the part of its velocity not along the push bleeds off, the push is added (capped by
/// `AccelRate`, less when the pawn walks or crouches), and the result is limited to its top speed (scaled by
/// `DesiredSpeed` for computer-controlled pawns and by `WalkingPct` or `CrouchedPct`).
fn ground_velocity(vm: &mut Vm<'_>, a: ObjId, accel: Vec3, mut vel: Vec3, dt: f32, human: bool) -> (Vec3, Vec3) {
    let friction = ground_friction(vm, a);
    let slow = get(vm, a, "bIsWalking").as_bool() || get(vm, a, "bIsCrouched").as_bool();
    let crouched = get(vm, a, "bIsCrouched").as_bool();
    let share = if crouched { get(vm, a, "CrouchedPct").as_float() } else { get(vm, a, "WalkingPct").as_float() };
    let share = if share > 0.0 { share } else { 1.0 };
    let accel_rate = get(vm, a, "AccelRate").as_float().max(1.0);
    let mut accel = accel;
    if accel.length_squared() <= 1e-6 {
        // Braking, in steps of at most 0.03 s so it does not depend much on the frame rate.
        let before = vel;
        let mut left = dt;
        let mut kept = Vec3::ZERO;
        while left > 0.0 {
            let step = left.min(0.03);
            vel -= 2.0 * vel * step * friction;
            if vel.dot(before) > 0.0 {
                kept += vel * (step / dt);
            }
            left -= step;
        }
        vel = if kept.dot(before) < 0.0 || kept.length_squared() < 100.0 { Vec3::ZERO } else { kept };
    } else {
        let dir = accel.normalize();
        let limit = if slow { accel_rate * share } else { accel_rate };
        if accel.length() > limit {
            accel = dir * limit;
        }
        let speed = vel.length();
        vel -= (vel - dir * speed) * (dt * friction).min(1.0);
        vel += accel * dt;
    }
    let mut top = get(vm, a, "GroundSpeed").as_float().max(1.0);
    if !human {
        let desired = get(vm, a, "DesiredSpeed").as_float();
        top *= if desired > 0.0 { desired } else { 1.0 };
    }
    if slow {
        top *= share;
    }
    if vel.length() > top {
        vel = vel.normalize() * top;
    }
    (vel, accel)
}

/// A walking pawn for one tick.
///
/// A pawn without a controller stands still. Otherwise its velocity is worked out (`ground_velocity`) and the
/// move is made in sub-steps of at most 0.05 s. Each step goes forward; a barrier is climbed if it is low enough
/// (else the pawn slides along it); the pawn is put back on the floor and takes what it stands on as its
/// `Base`. With no floor under it, a pawn that must not walk off ledges has the step undone, and any other one
/// starts to fall. At the end the velocity is what the pawn actually moved.
/// Walks, then gives the pawn the stair smoothing of the original game (`smooth_steps`).
fn walk_smoothed(vm: &mut Vm<'_>, a: ObjId, dt: f32) {
    let old = vec(&get(vm, a, "Location"));
    walk(vm, a, dt);
    smooth_steps(vm, a, old);
}

/// Stair smoothing as the original's pawns have it. The pawn itself climbs a step in one tick; what it keeps in
/// `SavedPrePivot.z` is how far its mesh (and the script's camera target) lags behind, so they rise over a stair as over a ramp.
///
/// After a tick on foot, `d` is the horizontal distance moved times `StepHeight / StepWidth` (16 / 32: the slope of a
/// stair), at most `StepHeight`. What is kept moves towards nothing by `d`, never past it. Then a tick that took the
/// pawn down by `StepHeight` (within 1) keeps `StepHeight - d`, and one that took it up keeps `-(StepHeight - d)`: the
/// mesh starts a step behind and gains on the pawn at the stair's own slope while it walks. Only walking gives it back.
/// Written from the behaviour checked in the original's pawn physics (the flipendo notes, engine.md "Stair smoothing").
fn smooth_steps(vm: &mut Vm<'_>, a: ObjId, old: Vec3) {
    if get(vm, a, "Physics").as_byte() != PHYS_WALKING {
        set_saved_pre_pivot(vm, a, 0.0);
        return;
    }
    let (step_height, step_width) = (get(vm, a, "StepHeight").as_float(), get(vm, a, "StepWidth").as_float());
    if step_height <= 0.0 || step_width <= 0.0 {
        return;
    }
    let new = vec(&get(vm, a, "Location"));
    let d = ((new - old).truncate().length() * step_height / step_width).min(step_height);
    let mut kept = saved_pre_pivot(vm, a);
    if kept > 0.0 {
        kept = (kept - d).max(0.0);
    } else if kept < 0.0 {
        kept = (kept + d).min(0.0);
    }
    let rise = new.z - old.z;
    if (rise + step_height).abs() <= 1.0 {
        kept = step_height - d;
    } else if (rise - step_height).abs() <= 1.0 {
        kept = -(step_height - d);
    }
    set_saved_pre_pivot(vm, a, kept);
}

fn saved_pre_pivot(vm: &mut Vm<'_>, a: ObjId) -> f32 {
    vec(&get(vm, a, "SavedPrePivot")).z
}

fn set_saved_pre_pivot(vm: &mut Vm<'_>, a: ObjId, z: f32) {
    let now = vec(&get(vm, a, "SavedPrePivot"));
    if (now.z - z).abs() > 1e-4 {
        set(vm, a, "SavedPrePivot", value(Vec3::new(now.x, now.y, z)));
    }
}

/// A pawn that is not walking keeps no stair offset (other actors have none to clear).
fn drop_step_offset(vm: &mut Vm<'_>, a: ObjId) {
    if is_pawn(vm, a) && saved_pre_pivot(vm, a) != 0.0 {
        set_saved_pre_pivot(vm, a, 0.0);
    }
}

/// The mesh offset the stair smoothing keeps now lives in `SavedPrePivot` (drawn through `mesh_offset`, read by the camera
/// script), so there is nothing extra to add to where an actor is drawn.
pub fn view_offset(_vm: &mut Vm<'_>, _a: ObjId) -> f32 {
    0.0
}

fn walk(vm: &mut Vm<'_>, a: ObjId, dt: f32) {
    let Some(controller) = get(vm, a, "Controller").as_obj() else {
        set(vm, a, "Acceleration", value(Vec3::ZERO));
        set(vm, a, "Velocity", value(Vec3::ZERO));
        return;
    };
    let human = world(vm).player == Some(controller);
    let mut accel = vec(&get(vm, a, "Acceleration"));
    accel.z = 0.0;
    let mut vel = vec(&get(vm, a, "Velocity"));
    vel.z = 0.0;
    // A pawn at rest only looks at its floor now and then (staggered between pawns, every 32nd tick); moving
    // ones always do.
    let resting = accel.length_squared() <= 1e-6 && vel.length_squared() <= 1e-4;
    if resting && (world(vm).frame + a.0 as u64) % 32 != 0 {
        return;
    }
    let new_vel = if resting {
        Vec3::ZERO
    } else {
        let (v, capped) = ground_velocity(vm, a, accel, vel, dt, human);
        set(vm, a, "Acceleration", value(capped));
        set(vm, a, "Velocity", value(v));
        // The pawn's script may change the new speed (a sliding pawn turned backwards is pushed on down the slide).
        fire(vm, a, "ModifyVelocity", &mut [Value::Float(dt), value(vel)]);
        if human && crate::envc::var_os("HP3_SLIDE_DEBUG").is_some() {
            eprintln!("walkvel: in {vel} accel {accel} ground {v} after-script {}", vec(&get(vm, a, "Velocity")));
        }
        if vm.db.objects.get(a).deleted || get(vm, a, "Physics").as_byte() != PHYS_WALKING {
            return;
        }
        let mut v = vec(&get(vm, a, "Velocity"));
        v.z = 0.0;
        v
    };
    let start = vec(&get(vm, a, "Location"));
    let old_base = get(vm, a, "Base").as_obj();
    let can_jump = get(vm, a, "bCanJump").as_bool();
    let wz = min_floor_z(vm, a);
    let sliding = get(vm, a, "bIsSliding").as_bool();
    let walks_off = get(vm, a, "bCanWalkOffLedges").as_bool() || !(get(vm, a, "bIsWalking").as_bool() || get(vm, a, "bIsCrouched").as_bool());
    let mut left = dt;
    let mut asked_may_fall = false;
    let mut steps = 0;
    while left > 0.0 && steps < 8 {
        steps += 1;
        let tick = if left > 0.05 { 0.05f32.min(left * 0.5) } else { left };
        left -= tick;
        let delta = new_vel * tick;
        let before = vec(&get(vm, a, "Location"));
        if delta.length_squared() > 1e-8 {
            let hit = move_actor(vm, a, delta);
            if crate::envc::var_os("HP3_WALK_DEBUG").is_some_and(|v| human || v == "all") {
                eprintln!("walk: {} step {delta} from {before} hit {:?}", hp3_script::natives::object_path(vm, a), hit.map(|h| (h.time, h.normal, h.actor.map(|o| o.0))));
            }
            if let Some(hit) = hit
                && hit.normal.z < wz
                && climb_or_slide(vm, a, delta, hit)
            {
                // The script took the pawn up a ledge: the rest of the step is its business.
                return;
            }
            // Met a slope it may walk on (a ramp, or the sloped foot of a block): the rest of the step goes
            // along the slope, up or down it, instead of stopping dead against it.
            if let Some(hit) = hit
                && hit.normal.z >= wz
                && hit.time < 1.0
            {
                let remaining = delta * (1.0 - hit.time);
                // Blocked on the spot by a surface that faces up: the next tread of a stair, a hair above the
                // feet. It is stepped onto like any other step (up, over, down); only when that gets nowhere does
                // the move go along the surface instead.
                let mut stepped = false;
                // (A ramp is not a tread: climbing its face as a step picked a spot off to one side, and the pawn went sideways.)
                // (A hero on a slide is the exception: its track is faceted, and going along each facet bled its speed away at every
                // crease until it stood wedged on the slope; the step-up, as in the original, keeps the way it was going.)
                let ramp = hit.normal.z < 0.998 && !sliding && crate::envc::var_os("HP3_RAMP_STEPUP").is_none();
                if hit.time * delta.length() <= 1.0 && !ramp {
                    let before = vec(&get(vm, a, "Location"));
                    step_up(vm, a, remaining, hit, 0);
                    stepped = (vec(&get(vm, a, "Location")) - before).truncate().length_squared() > 0.0025;
                    if crate::envc::var_os("HP3_WALK_DEBUG").is_some_and(|v| human || v == "all") {
                        eprintln!("walk: step_up from {before} to {} stepped {stepped}", vec(&get(vm, a, "Location")));
                    }
                }
                if !stepped {
                    // Along the surface; where that meets a second one (the crease between two slopes, a valley in a
                    // slide track) the rest goes along the crease, so the pawn is not left wedged in it.
                    let mut along = remaining - hit.normal * remaining.dot(hit.normal);
                    // Going up a slope a walker may stand on keeps its heading and its walking speed: it moves the way it was going
                    // and rises with the slope. (Projecting onto the slope kept only cos(slope) of the horizontal distance, which the
                    // next tick's velocity, taken from the distance moved, compounded into a crawl; and on a slope that also tilts
                    // sideways it turned the pawn off its heading.)
                    if !sliding && hit.normal.z < 0.998 && hit.normal.z > 0.1 {
                        let n = hit.normal;
                        along = Vec3::new(remaining.x, remaining.y, -(n.x * remaining.x + n.y * remaining.y) / n.z);
                    }
                    let mut plane = hit.normal;
                    for _ in 0..3 {
                        if along.length_squared() <= 1e-8 {
                            break;
                        }
                        let along_hit = move_actor(vm, a, along);
                        if crate::envc::var_os("HP3_WALK_DEBUG").is_some_and(|v| human || v == "all") {
                            eprintln!("walk: along {along} now at {} hit {:?}", vec(&get(vm, a, "Location")), along_hit.map(|h| (h.time, h.normal, h.actor.map(|o| o.0))));
                        }
                        let Some(next) = along_hit.filter(|h| h.time < 1.0 && h.normal.length_squared() > 0.5) else { break };
                        let left = along * (1.0 - next.time);
                        let crease = plane.cross(next.normal);
                        along = if crease.length_squared() > 1e-4 {
                            let c = crease.normalize();
                            c * c.dot(left)
                        } else {
                            left - next.normal * left.dot(next.normal)
                        };
                        plane = next.normal;
                    }
                }
            }
        }
        let floor_found = floor_below(vm, a, MAX_STEP_HEIGHT + 2.0);
        if crate::envc::var("HP3_FLOOR_DEBUG").is_ok_and(|n| hp3_script::natives::object_path(vm, a).ends_with(&n)) {
            let at = vec(&get(vm, a, "Location"));
            let ext = extent(vm, a);
            let raw = world(vm).collision.trace_as(false, at, at - Vec3::Z * (MAX_STEP_HEIGHT + 2.0), ext, Some(a));
            eprintln!("floor: frame {} at {at} ext {ext} floor {:?} raw {:?}", world(vm).frame, floor_found.map(|f| (f.time, f.normal)), raw.map(|f| (f.time, f.normal)));
        }
        match floor_found {
            Some(floor) => {
                let loc = vec(&get(vm, a, "Location"));
                set(vm, a, "Floor", value(floor.normal));
                if sliding && is_player_pawn(vm, a) {
                    let z = vec(&get(vm, a, "Location")).z;
                    SLIDE_GROUND_Z.with(|g| g.set(z));
                }
                let drop = (MAX_STEP_HEIGHT + 2.0) * floor.time;
                if drop > 0.5 {
                    set(vm, a, "Location", value(loc - Vec3::Z * (drop - 0.5)));
                }
                let before = get(vm, a, "Base").as_obj();
                if before != floor.actor {
                    // Standing on (or stepping off) another pawn is told to both (a Bundimun hears `PawnLandedOnMe`).
                    let pawn_involved = floor.actor.is_some_and(|b| is_pawn(vm, b)) || before.is_some_and(|b| is_pawn(vm, b));
                    crate::natives::set_base(vm, a, floor.actor, pawn_involved);
                }
            }
            None => {
                if crate::envc::var_os("HP3_FALL_LOG").is_some() {
                    let (frame, report) = (world(vm).frame, floor_report(vm, a));
                    eprintln!("walk off: frame {frame} {report}");
                }
                // Nothing to stand on. The controller gets a chance to stop the pawn once.
                if !asked_may_fall && can_jump {
                    asked_may_fall = true;
                    fire(vm, controller, "MayFall", &mut []);
                }
                if (!can_jump && !sliding) || !walks_off {
                    // This pawn must not fall: undo the step and stop it.
                    set(vm, a, "Location", value(before));
                    set(vm, a, "Velocity", value(Vec3::ZERO));
                    set(vm, a, "Acceleration", value(Vec3::ZERO));
                    set(vm, controller, "MoveTimer", Value::Float(-1.0));
                    return;
                }
                if get(vm, a, "Physics").as_byte() == PHYS_WALKING {
                    fire(vm, a, "Falling", &mut []);
                }
                if get(vm, a, "Physics").as_byte() == PHYS_WALKING {
                    set(vm, a, "Physics", Value::Byte(PHYS_FALLING));
                }
                set(vm, a, "Base", Value::Object(None));
                // The rest of the tick is spent falling.
                set(vm, a, "Velocity", value(Vec3::new(new_vel.x, new_vel.y, 0.0)));
                if left > 0.0 {
                    fall(vm, a, left);
                }
                return;
            }
        }
    }
    let moved = vec(&get(vm, a, "Location")) - start;
    let mut actual = moved / dt;
    actual.z = 0.0;
    set(vm, a, "Velocity", value(actual));
    let _ = old_base;
}

/// The least a walker is pushed forward when it steps up onto a tread (units).
const MIN_STEP_PUSH: f32 = 6.0;

/// A normal whose z is below this is the side of a step (a riser); steeper than that is not climbed.
const MAX_STEP_SIDE_Z: f32 = 0.08;

/// A walker met a barrier: a ledge it can mount is handed to the script (true); else it steps up the barrier
/// or slides along it (and lets the script know with `HitWall`).
fn climb_or_slide(vm: &mut Vm<'_>, a: ObjId, delta: Vec3, hit: Hit) -> bool {
    if try_mount(vm, a, Vec3::Z, &hit) {
        return true;
    }
    step_up(vm, a, delta * (1.0 - hit.time), hit, 0);
    false
}

/// With a `hit.flag` file next to the game's data (or `HP3_HIT_LOG` set), says once a second what blocks a mover.
fn log_block(vm: &mut Vm<'_>, a: ObjId, hit: &Hit) {
    if world(vm).frame % 30 != 0 {
        return;
    }
    let wanted = crate::envc::var_os("HP3_HIT_LOG").is_some() || std::path::Path::new("/sdcard/FlipendoHP3/hit.flag").exists();
    if !wanted {
        return;
    }
    let who = hp3_script::natives::object_path(vm, a);
    let by = hit.actor.map(|o| hp3_script::natives::object_path(vm, o)).unwrap_or_else(|| "the level".into());
    let at = vec(&get(vm, a, "Location"));
    eprintln!("blocked: {who} at {at} by {by} normal {}", hit.normal);
}

/// Tells the pawn's script it ran into a wall: not for another pawn, and not when it was not pushing.
fn notify_hit_wall(vm: &mut Vm<'_>, a: ObjId, hit: &Hit) {
    log_block(vm, a, hit);
    let human = world(vm).player.is_some_and(|pc| get(vm, pc, "Pawn").as_obj() == Some(a));
    if hit.actor.is_some_and(|o| is_pawn(vm, o)) {
        return;
    }
    let wall = hit.actor.or(world(vm).level_info);
    let mut normal = hit.normal;
    // A pawn a controller steers hears about a wall only when it pushes into it head on, and the controller may take
    // the event for itself first (the fire crab's flip, an idle wanderer turning round). The player's pawn always
    // hears it. A pawn nobody controls, or one that wants every hit (bDirectHitWall), hears it directly.
    if !human
        && !get(vm, a, "bDirectHitWall").as_bool()
        && let Some(c) = get(vm, a, "Controller").as_obj()
    {
        if vec(&get(vm, a, "Acceleration")).length_squared() < 1e-6 {
            return;
        }
        let at = vec(&get(vm, a, "Location"));
        let mut dir = (vec(&get(vm, c, "Destination")) - at).normalize_or_zero();
        let walking = get(vm, a, "Physics").as_byte() == PHYS_WALKING;
        if walking {
            normal.z = 0.0;
            dir.z = 0.0;
            normal = normal.normalize_or_zero();
            dir = dir.normalize_or_zero();
        }
        if get(vm, c, "MinHitWall").as_float() < dir.dot(normal) {
            return;
        }
        if fire(vm, c, "NotifyHitWall", &mut [value(normal), Value::Object(wall)]).as_bool() {
            return;
        }
        if get(vm, a, "Physics").as_byte() != PHYS_FALLING {
            // Back down from any step the move had taken.
            move_actor(vm, a, Vec3::new(0.0, 0.0, -MAX_STEP_HEIGHT));
        }
    }
    fire(vm, a, "HitWall", &mut [value(normal), Value::Object(wall)]);
}

/// Takes a walker past the barrier it met: raised by a step's height (and 2), the rest of the move is made at
/// that height and, when it ends against yet another riser, raised again (stairs); then the pawn is put back down
/// onto whatever is under it. A barrier that is no step and no floor is slid along instead, once the script has
/// been told. Behaviour of the reference engine's pawn step-up (UnPhysic.cpp), written afresh.
fn step_up(vm: &mut Vm<'_>, a: ObjId, delta: Vec3, hit: Hit, depth: u32) {
    let lift = MAX_STEP_HEIGHT + 2.0;
    let mut step_down = true;
    let mut steppable = hit.normal.z.abs() < MAX_STEP_SIDE_Z || hit.normal.z >= min_floor_z(vm, a);
    // The edge of a step, met by a box instead of a cylinder, reads as a slanted face. When there is room to go up
    // by a step and on over the top, it is a step all the same.
    if !steppable && hit.normal.z >= MAX_STEP_SIDE_Z && delta.length_squared() > 1e-8 && get(vm, a, "Physics").as_byte() == PHYS_WALKING {
        let loc = vec(&get(vm, a, "Location"));
        let ext = extent(vm, a);
        let player = blocked_as_player(vm, a);
        let ahead = delta.normalize_or_zero() * (ext.x * 2.0 + 4.0);
        let up_clear = world(vm).collision.trace_as(player, loc, loc + Vec3::Z * lift, ext, Some(a)).is_none();
        let over_clear = world(vm).collision.trace_as(player, loc + Vec3::Z * lift, loc + Vec3::Z * lift + ahead, ext, Some(a)).is_none();
        steppable = up_clear && over_clear;
    }
    // A walker pressed against a riser has almost no speed and what it has may drift sideways along the face; the
    // step goes where it is pushed (the player's heading), by at least a few units, so that it comes down on the
    // tread and not back where it stood.
    let mut delta = delta;
    // Any walker that is being driven does this, not only the player: a companion or a student led up a stair stood
    // against the first riser, its drift along the face never carrying it over.
    if depth == 0 && steppable && get(vm, a, "Physics").as_byte() == PHYS_WALKING {
        let push = vec(&get(vm, a, "Acceleration"));
        let push = Vec3::new(push.x, push.y, 0.0).normalize_or_zero();
        if push != Vec3::ZERO {
            // At least a few units, so that it comes down on the tread and not back where it stood. (Tried less for a pawn walking at
            // speed, to stop the small lunge at each riser: 3 units or none left it bouncing back off the riser for many frames.)
            delta = push * delta.length().max(MIN_STEP_PUSH);
        }
    }
    let mut now = Some(hit);
    let start = vec(&get(vm, a, "Location"));
    let mut lifted = 0.0;
    let mut across = Vec3::ZERO;
    if steppable {
        move_actor(vm, a, Vec3::Z * lift);
        lifted = vec(&get(vm, a, "Location")).z - start.z;
        let before = vec(&get(vm, a, "Location"));
        now = move_actor(vm, a, delta);
        across = vec(&get(vm, a, "Location")) - before;
        // A riser taller than a step: the lifted box then ends with its feet below the tread's top, inside a surface that
        // only blocks from above (the round Carpe platform's rim), and the pawn walks on through it and falls. The tread must
        // lie under the box, not across it: where a box dropped from above meets a floor on the way down, the step is no step.
        if get(vm, a, "Physics").as_byte() == PHYS_WALKING {
            let end = vec(&get(vm, a, "Location"));
            let ext = extent(vm, a);
            let player = blocked_as_player(vm, a);
            // Only a floor that cuts through the body counts (its top between the feet and the head): one far overhead, the
            // next flight of a stair, is no burial.
            let floor_z = min_floor_z(vm, a);
            let buried = crate::envc::var_os("HP3_NO_BURIED").is_none()
                && world(vm).collision.trace_as(player, end + Vec3::Z * (ext.z * 2.0), end, ext, Some(a)).is_some_and(|h| {
                    let rise = h.location.z - end.z;
                    h.time < 1.0 && h.normal.z >= floor_z && rise > 1.0 && rise < ext.z * 2.0 - 2.0
                });
            if buried {
                set(vm, a, "Location", value(start));
                step_down = false;
                lifted = 0.0;
                across = Vec3::ZERO;
                now = Some(hit);
            }
        }
    } else if get(vm, a, "Physics").as_byte() != PHYS_WALKING {
        // Not walking: slide up the slope.
        now = move_actor(vm, a, delta + Vec3::Z * (delta.length() * hit.normal.z));
        step_down = false;
    }
    if let Some(h) = now {
        // Another riser close ahead: step down onto this tread and go up again.
        if h.normal.z.abs() < MAX_STEP_SIDE_Z && h.time * delta.length_squared() > 144.0 && depth < 8 {
            if step_down {
                move_actor(vm, a, -Vec3::Z * lift);
            }
            step_up(vm, a, delta * (1.0 - h.time), h, depth + 1);
            return;
        }
        notify_hit_wall(vm, a, &h);
        if get(vm, a, "Physics").as_byte() == PHYS_FALLING {
            return;
        }
        // Slide along the barrier, treated as upright.
        let mut n = h.normal;
        n.z = 0.0;
        let n = n.normalize_or_zero();
        let along = (delta - n * delta.dot(n)) * (1.0 - h.time);
        if along.dot(delta) >= 0.0
            && let Some(second) = move_actor(vm, a, along)
        {
            notify_hit_wall(vm, a, &second);
        }
    }
    if step_down {
        move_actor(vm, a, -Vec3::Z * lift);
    }
    if depth == 0 && is_player_pawn(vm, a) {
        let end = vec(&get(vm, a, "Location"));
        if (end - start).truncate().length() < 1.0 && (end.z - start.z).abs() < 1.0 {
            let (velocity, acceleration) = (vec(&get(vm, a, "Velocity")), vec(&get(vm, a, "Acceleration")));
            vm.log.push(format!(
                "stepup failed at {start}: velocity {} accel {} face ({:.2},{:.2},{:.2}) time {:.3} steppable {steppable} wanted {delta} lifted {lifted:.1} across {across} then {}",
                velocity,
                acceleration,
                hit.normal.x,
                hit.normal.y,
                hit.normal.z,
                hit.time,
                match now {
                    Some(h) => format!("hit ({:.2},{:.2},{:.2}) at {:.3}", h.normal.x, h.normal.y, h.normal.z, h.time),
                    None => "free".to_string(),
                }
            ));
        }
    }
}

/// Whether a pawn that ran (or fell) into a wall can climb onto a ledge there, and if so hands the move to its
/// script (`Mount(delta, actor)`, which plays the climb).
///
/// The wall must stand nearly upright and face the pawn, and the pawn must have a `MaxMountHeight`. A box the size
/// of the pawn is dropped just past the wall from that height above it (or, with `bUseStaticMeshSizeForMount`, from
/// the height of the static mesh it ran into). The top it finds has to be at least a step up (any height while
/// falling), and the pawn must be able to go straight up from where it is and then across to the top. The level's
/// geometry and movers are tried first, then other pawns.
fn try_mount(vm: &mut Vm<'_>, a: ObjId, up: Vec3, hit: &Hit) -> bool {
    let Some(destination) = mount_destination(vm, a, up, hit) else { return false };
    let loc = vec(&get(vm, a, "Location"));
    let wall = hit.actor.or(world(vm).level_info);
    let taken = fire(vm, a, "Mount", &mut [value(destination - loc), Value::Object(wall)]).as_bool();
    if taken && is_player_pawn(vm, a) {
        MOUNT_GOAL.with(|g| g.set(Some((a.0, destination, false))));
    }
    if is_player_pawn(vm, a) {
        let name = wall.map(|w| hp3_script::natives::object_path(vm, w)).unwrap_or_default();
        let mountable = wall.map(|w| get(vm, w, "bIsMountable").as_bool()).unwrap_or(false);
        let found = LAST_LEDGE.with(|c| c.get());
        let how = found.map(|(at, n, o, t, start, pass)| format!("drop from {start} landed at {at} time {t:.3} normal ({:.2},{:.2},{:.2}) on {:?} pass {pass}", n.x, n.y, n.z, o.map(|o| hp3_script::natives::object_path(vm, o)))).unwrap_or_default();
        mount_note(vm, format!("mount: ledge {:.0} up on {name} (bIsMountable {mountable}) from {loc} -> script {} [{how}]", destination.z - loc.z, if taken { "took it" } else { "refused" }));
    }
    taken
}

thread_local! {
    /// Whether the last tick of the player was logged as sliding (HP3_SLIDE_DEBUG), to log the tick it leaves the slide.
    static SLIDE_LOGGED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

thread_local! {
    /// The player's climb in progress: the pawn, where the ledge search said it would end up, and whether the script's
    /// climb states have been seen yet.
    static MOUNT_GOAL: std::cell::Cell<Option<(u32, Vec3, bool)>> = const { std::cell::Cell::new(None) };
}

thread_local! {
    /// A pawn easing onto the top of the ledge it climbed: the pawn, the spot and the seconds left.
    static MOUNT_GLIDE: std::cell::Cell<Option<(u32, Vec3, f32)>> = const { std::cell::Cell::new(None) };
}

/// One step of that glide: the pawn moves a share of the way to the spot (and stands still otherwise). True while it
/// goes on.
fn glide_to_ledge(vm: &mut Vm<'_>, a: ObjId, dt: f32) -> bool {
    let Some((who, goal, left)) = MOUNT_GLIDE.with(|g| g.get()) else { return false };
    if who != a.0 {
        return false;
    }
    let loc = vec(&get(vm, a, "Location"));
    let share = (dt * 14.0).min(1.0);
    let next = loc + (goal - loc) * share;
    let done = (goal - next).length() < 0.5 || left - dt <= 0.0;
    set(vm, a, "Location", value(if done { goal } else { next }));
    set(vm, a, "Velocity", value(Vec3::ZERO));
    set(vm, a, "Acceleration", value(Vec3::ZERO));
    MOUNT_GLIDE.with(|g| g.set(if done { None } else { Some((who, goal, left - dt)) }));
    crate::touch::settle(vm, a);
    !done
}

/// When the player's climb ends away from the ledge top the search found (the climb animation's own motion can stop
/// short against the wall and leave the pawn overlapping the block it climbs), the pawn is put on the ledge top.
fn finish_mount(vm: &mut Vm<'_>, a: ObjId) {
    let Some((who, goal, seen)) = MOUNT_GOAL.with(|g| g.get()) else { return };
    if who != a.0 {
        return;
    }
    let climbing = vm.state_of(a).is_some_and(|s| vm.db.names.get(vm.db.get(s).name).to_ascii_lowercase().contains("mount"));
    if climbing {
        MOUNT_GOAL.with(|g| g.set(Some((who, goal, true))));
        return;
    }
    if !seen {
        return;
    }
    MOUNT_GOAL.with(|g| g.set(None));
    let loc = vec(&get(vm, a, "Location"));
    let off = goal - loc;
    if off.truncate().length() < 10.0 && off.z.abs() < 12.0 {
        return;
    }
    if off.length() > 160.0 {
        return;
    }
    // The ledge top has to be free for the pawn's box.
    let ext = extent(vm, a) - Vec3::splat(0.5);
    let free = world(vm).collision.trace_as(true, goal + Vec3::Z * 0.1, goal, ext, Some(a)).is_none();
    if free {
        // He glides there over a third of a second (see `glide_to_ledge`) instead of jumping.
        MOUNT_GLIDE.with(|g| g.set(Some((a.0, goal, 0.35))));
        vm.log.push(format!("mount: the climb ended at {loc}; the pawn glides onto the ledge top at {goal}"));
    } else {
        vm.log.push(format!("mount: the climb ended at {loc}, away from the ledge top {goal}, which is not free"));
    }
}

/// Writes a line about a mount attempt to the log once (the same line again and again is dropped).
fn mount_note(vm: &mut Vm<'_>, line: String) {
    thread_local! {
        static LAST: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    }
    let fresh = LAST.with(|l| {
        let mut l = l.borrow_mut();
        if *l == line {
            false
        } else {
            *l = line.clone();
            true
        }
    });
    if fresh {
        vm.log.push(line);
    }
}

/// Where a pawn that met a wall at `hit` would climb to, if the wall is a ledge it can mount (see `try_mount`).
pub fn mount_destination(vm: &mut Vm<'_>, a: ObjId, up: Vec3, hit: &Hit) -> Option<Vec3> {
    let max_height = get(vm, a, "MaxMountHeight").as_float();
    let debug = crate::envc::var_os("HP3_MOUNT_DEBUG").is_some() && is_player_pawn(vm, a);
    let player = blocked_as_player(vm, a);
    if max_height <= 0.0 || hit.normal.z <= -0.1 || hit.normal.z >= 0.17 {
        if debug {
            eprintln!("mount: not a wall (normal {}, max height {max_height})", hit.normal);
        }
        if player && hit.normal.z.abs() < 0.9 {
            mount_note(vm, format!("mount: not an upright wall (normal {:.2},{:.2},{:.2}, MaxMountHeight {max_height})", hit.normal.x, hit.normal.y, hit.normal.z));
        }
        return None;
    }
    if rotator_direction(&get(vm, a, "Rotation")).dot(hit.normal) >= 0.0 {
        if debug {
            eprintln!("mount: wall {} does not face the pawn", hit.normal);
        }
        if player {
            mount_note(vm, format!("mount: wall ({:.2},{:.2},{:.2}) does not face the pawn", hit.normal.x, hit.normal.y, hit.normal.z));
        }
        return None;
    }
    let (radius, half_height) = (get(vm, a, "CollisionRadius").as_float(), get(vm, a, "CollisionHeight").as_float());
    let ext = Vec3::new(radius, radius, half_height);
    let mut reach = max_height;
    if get(vm, a, "bUseStaticMeshSizeForMount").as_bool()
        && let Some(wall) = hit.actor
        && let Some(half) = crate::static_mesh_half_height(vm, wall)
    {
        reach = half + half_height;
    }
    let loc = vec(&get(vm, a, "Location"));
    let min_rise = if get(vm, a, "Physics").as_byte() == PHYS_FALLING {
        0.0
    } else {
        match get(vm, a, "MaxStepHeight").as_float() {
            h if h > 0.0 => h,
            _ => MAX_STEP_HEIGHT,
        }
    };
    let ledge = Ledge { loc, up, normal: hit.normal, radius, reach, min_rise, ext, ignore: a };
    let found = find_ledge(&world(vm).collision, &ledge);
    if found.is_none() && player {
        let name = hit.actor.map(|w| hp3_script::natives::object_path(vm, w)).unwrap_or_else(|| "the level".to_string());
        {
            let into = Vec3::new(-hit.normal.x, -hit.normal.y, 0.0).normalize_or_zero();
            let start = loc + up * reach + into * (2.0 * radius + reach * hit.normal.z);
            let end = loc + into * (2.0 * radius);
            let col = &world(vm).collision;
            let d = col.trace_static(start, end, ext);
            let dp = col.trace_pawns(start, end, ext, Some(a));
            let dest = d.map(|h| Vec3::new(h.location.x, h.location.y, h.location.z + 2.0));
            let steps = dest.map(|dst| { let lift = Vec3::new(loc.x + up.x * reach, loc.y + up.y * reach, dst.z); (lift, col.trace_static(loc, lift, ext).map(|h| (h.location, h.time, h.normal, h.actor)), col.trace_static(lift, dst, ext).map(|h| (h.location, h.time, h.normal))) });
            vm.log.push(format!("mount debug: steps {steps:?}"));
            let msg = format!("mount debug: drop {start} -> {end} ext {ext}: static {:?} pawns {:?}", d.map(|h| (h.location, h.time, h.normal)), dp.map(|h| h.location));
            vm.log.push(msg);
        }
        let flags = hit.actor.map(|w| (get(vm, w, "bBlockActors").as_bool(), get(vm, w, "bBlockPlayers").as_bool())); mount_note(vm, format!("mount: no ledge found on {name} (reach {reach:.0}, min rise {min_rise:.0}, loc {loc}, normal {}, blockActors/blockPlayers {flags:?})", hit.normal));
    }
    found
}

/// What `find_ledge` needs to know about the pawn and the wall it met.
struct Ledge {
    loc: Vec3,
    up: Vec3,
    normal: Vec3,
    radius: f32,
    /// How far above the pawn to look for the top of the wall.
    reach: f32,
    /// The least a ledge must be above the pawn's centre.
    min_rise: f32,
    ext: Vec3,
    ignore: ObjId,
}

/// The place on top of a wall a pawn can climb to: a box the size of the pawn is dropped just past the wall, from
/// `reach` above the pawn, and must find a top at least `min_rise` up; the pawn must then be able to go straight
/// up from where it is, and across to that spot, without hitting anything. The level's geometry and movers are
/// tried first, then other pawns.
thread_local! {
    /// What the last ledge search settled on: where the drop landed, the surface's normal and actor, the hit time, the
    /// drop's start and which pass found it (for the game log).
    static LAST_LEDGE: std::cell::Cell<Option<(Vec3, Vec3, Option<ObjId>, f32, Vec3, i32)>> = const { std::cell::Cell::new(None) };
}

fn find_ledge(col: &crate::collision::Collision, l: &Ledge) -> Option<Vec3> {
    LAST_LEDGE.with(|c| c.set(None));
    let into = Vec3::new(-l.normal.x, -l.normal.y, 0.0).normalize_or_zero();
    let top = l.loc + l.up * l.reach;
    // What the original does is one drop two radii in from the wall: its answer is used as it comes. A stone
    // block's collision hull can slope there, so when that first answer is not a flat top, drops further in are
    // tried and a flat top found there is preferred; the first answer stays as the fallback.
    let mut first: Option<Vec3> = None;
    let mut flat: Option<Vec3> = None;
    for deeper in [0.0, 1.0, 2.0, 3.0, 4.0] {
        let shift = into * (deeper * l.radius);
        let start = top + into * (2.0 * l.radius + l.reach * l.normal.z) + shift;
        let end = l.loc + into * (2.0 * l.radius) + shift;
        for pass in 0..2 {
            let trace = |from: Vec3, to: Vec3| {
                if pass == 0 { col.trace_static(from, to, l.ext) } else { col.trace_pawns(from, to, l.ext, Some(l.ignore)) }
            };
            let Some(down) = trace(start, end) else { continue };
            if down.location.z - l.loc.z < l.min_rise {
                continue;
            }
            let destination = Vec3::new(down.location.x, down.location.y, down.location.z + 2.0);
            let lift = Vec3::new(top.x, top.y, destination.z);
            // The way up and across is swept with a box a little narrower (6 units): a wall that leans a degree or two over the pawn
            // (Carpe Retractum's Mover34 block) would otherwise catch the box the moment it rises.
            let slim = Vec3::new((l.ext.x - 6.0).max(1.0), (l.ext.y - 6.0).max(1.0), l.ext.z);
            let sweep = |from: Vec3, to: Vec3| if pass == 0 { col.trace_static(from, to, slim) } else { col.trace_pawns(from, to, slim, Some(l.ignore)) };
            if sweep(l.loc, lift).is_some() || sweep(lift, destination).is_some() {
                continue;
            }
            if first.is_none() {
                first = Some(destination);
                LAST_LEDGE.with(|c| c.set(Some((down.location, down.normal, down.actor, down.time, start, pass))));
            }
            if down.normal.z >= WALKABLE_Z && flat.is_none_or(|b| destination.z > b.z + 1.0) {
                flat = Some(destination);
            }
            break;
        }
        if deeper == 0.0 && first.is_some() && flat.is_some() {
            break;
        }
    }
    flat.or(first)
}

/// A walking or falling pawn whose point lies in the level's zone 0 (outside the BSP) has left the world: the original's
/// `physWalking` and `physFalling` raise `FellOutOfWorld` there (the script respawns the player or kills the pawn). Flying
/// and swimming pawns never do. Returns true when the event was raised, so the caller skips this tick's move.
/// `HP3_NO_FELL_CHECK` turns it off.
fn fell_out_of_world(vm: &mut Vm<'_>, a: ObjId) -> bool {
    if !is_pawn(vm, a) || crate::envc::var_os("HP3_NO_FELL_CHECK").is_some() {
        return false;
    }
    let loc = vec(&get(vm, a, "Location"));
    if world(vm).collision.zone_at(loc) != Some(0) {
        return false;
    }
    vm.log.push(format!("{} fell out of the world at {loc}", hp3_script::natives::object_path(vm, a)));
    fire(vm, a, "FellOutOfWorld", &mut [Value::Byte(0)]);
    true
}

thread_local! {
    /// The height a sliding player last stood on the track at.
    static SLIDE_GROUND_Z: std::cell::Cell<f32> = const { std::cell::Cell::new(f32::NEG_INFINITY) };
}

/// A sliding hero that falls far below the track and is not in a jump the level made for it has left the track: the game
/// marks that (`bFlewOffTrack`, with the jump time restarted) and its slide state, seeing the mark last longer than
/// `SlideLeaveTrackDieTimeout`, sends the player back to the last telepit. The game sets the mark itself when the surface
/// next to the player is of the off-track kind (a material read by a trace), which the engine's traces do not give.
fn left_the_track(vm: &mut Vm<'_>, a: ObjId) {
    if !get(vm, a, "bIsSliding").as_bool() || !is_player_pawn(vm, a) {
        SLIDE_GROUND_Z.with(|z| z.set(f32::NEG_INFINITY));
        return;
    }
    let z = vec(&get(vm, a, "Location")).z;
    let ground = SLIDE_GROUND_Z.with(std::cell::Cell::get);
    if z > ground - 400.0 {
        // Not far below the track yet (a jump the level planned stays within this).
        return;
    }
    let Some(pc) = get(vm, a, "Controller").as_obj() else { return };
    let now = world(vm).level_info.map_or(0.0, |li| get(vm, li, "TimeSeconds").as_float());
    let last_jump = get(vm, a, "LastSlideJumpTime").as_float();
    if now - last_jump > 1.0 && !get(vm, pc, "bFlewOffTrack").as_bool() {
        set(vm, pc, "bFlewOffTrack", Value::Bool(true));
        set(vm, a, "LastSlideJumpTime", Value::Float(now));
    }
}

fn fall(vm: &mut Vm<'_>, a: ObjId, dt: f32) {
    left_the_track(vm, a);
    // A pawn nobody controls that is placed with its box sunk into a static mesh (a statue a few units into its
    // pedestal) stays where the level put it: sweeping out of it would drop it through.
    if !is_player_pawn(vm, a) && get(vm, a, "Controller").as_obj().is_none() {
        let loc = vec(&get(vm, a, "Location"));
        let ext = extent(vm, a) - Vec3::splat(0.5);
        let vel = vec(&get(vm, a, "Velocity"));
        if vel.truncate().length_squared() < 1.0 && world(vm).collision.box_overlaps_triangles(loc, ext, Some(a)) {
            set(vm, a, "Velocity", value(Vec3::ZERO));
            return;
        }
    }
    // What the pawn can steer by in the air: a part of its ground acceleration, none when the way ahead is a
    // wall (so air control cannot be used to climb it), and only enough to keep its sideways speed once that
    // is at the ground speed.
    let mut accel = vec(&get(vm, a, "Acceleration"));
    accel.z = 0.0;
    let vel0 = vec(&get(vm, a, "Velocity"));
    let (accel_rate, ground_speed) = (get(vm, a, "AccelRate").as_float(), get(vm, a, "GroundSpeed").as_float());
    let mut air = get(vm, a, "AirControl").as_float();
    if air > 0.05 && dt > 0.0 {
        let mut ahead = (accel.normalize_or_zero() * air * accel_rate + vel0) * dt;
        ahead.z = 0.0;
        let (loc, ext) = (vec(&get(vm, a, "Location")), extent(vm, a));
        if world(vm).collision.trace_static(loc, loc + ahead, ext).is_some() {
            air = 0.0;
        }
    }
    let mut max_accel = accel_rate * air;
    let speed2d = vel0.truncate().length();
    let mut bound = 0.0;
    if speed2d < 10.0 && air > 0.0 && dt > 0.0 {
        max_accel += (10.0 - speed2d) / dt;
    } else if speed2d >= ground_speed {
        if air <= 0.05 {
            max_accel = 1.0;
        } else {
            bound = speed2d;
        }
    }
    if accel.length_squared() > max_accel * max_accel {
        accel = accel.normalize_or_zero() * max_accel;
    }
    // A pawn can be pushed by a steady acceleration of its own (Carpe Retractum pulls with it) on top of gravity.
    let pawn = is_pawn(vm, a);
    let push = if pawn { vec(&get(vm, a, "ConstantAcceleration")) } else { Vec3::ZERO };
    let g = Vec3::new(0.0, 0.0, gravity(vm, a));
    let volume = get(vm, a, "PhysicsVolume").as_obj();
    let (terminal, drift) = match volume {
        Some(v) => (get(vm, v, "TerminalVelocity").as_float(), vec(&get(vm, v, "ZoneVelocity"))),
        None => (0.0, Vec3::ZERO),
    };
    // Slices of at most 0.05 s, so a long frame lands and bounces off walls like a short one. Each slice moves
    // at the speed halfway through it (half the slice's acceleration added), then takes the speed it really
    // moved at and carries it on to the slice's end (twice the average less the start), as the original's falling
    // does; a pawn's script may change the speed in between (ModifyVelocity: the hippogriff holds its course
    // with it), and a script that puts the pawn elsewhere (bJustTeleported) keeps the speed it set.
    let mut left = dt;
    for _ in 0..8 {
        if left <= 0.0 {
            break;
        }
        let tick = if left > 0.05 { 0.05f32.min(left * 0.5) } else { left };
        left -= tick;
        let from = vec(&get(vm, a, "Location"));
        set(vm, a, "bJustTeleported", Value::Bool(false));
        let mut start_vel = vec(&get(vm, a, "Velocity"));
        let mut vel = start_vel + (accel + g + push) * (0.5 * tick);
        set(vm, a, "Velocity", value(vel));
        if pawn {
            fire(vm, a, "ModifyVelocity", &mut [Value::Float(tick), value(start_vel)]);
            if vm.db.objects.get(a).deleted || get(vm, a, "Physics").as_byte() != PHYS_FALLING {
                return;
            }
            vel = vec(&get(vm, a, "Velocity"));
            // The top of a jump the controller asked to hear about (NotifyJumpApex: the hero starts its falling
            // animation there): said once, as the speed turns downward.
            if let Some(c) = get(vm, a, "Controller").as_obj()
                && vel.z <= 0.0
                && get(vm, c, "bNotifyApex").as_bool()
            {
                set(vm, a, "bJustTeleported", Value::Bool(true));
                set(vm, c, "bNotifyApex", Value::Bool(false));
                fire(vm, c, "NotifyJumpApex", &mut []);
                set(vm, c, "bJumpOverWall", Value::Bool(false));
                vel = vec(&get(vm, a, "Velocity"));
            }
        }
        if bound != 0.0 && vel.truncate().length_squared() > bound * bound {
            let flat = vel.truncate().normalize_or_zero() * bound;
            vel = Vec3::new(flat.x, flat.y, vel.z);
        }
        set(vm, a, "Velocity", value(vel));
        let hit = move_and_slide(vm, a, (vel + drift) * tick);
        if vm.db.objects.get(a).deleted {
            return;
        }
        let refine = |vm: &mut Vm<'_>, start_vel: Vec3| {
            if get(vm, a, "bJustTeleported").as_bool() {
                return;
            }
            let mut v = (vec(&get(vm, a, "Location")) - from) / tick - drift;
            if v.z < start_vel.z || start_vel.z >= 0.0 {
                v = 2.0 * v - start_vel;
            }
            if terminal > 0.0 && v.length_squared() > terminal * terminal {
                v = v.normalize() * terminal;
            }
            set(vm, a, "Velocity", value(v));
        };
        let Some(hit) = hit else {
            if pawn {
                set(vm, a, "Floor", value(Vec3::Z));
            }
            refine(vm, start_vel);
            continue;
        };
        if pawn {
            set(vm, a, "Floor", value(hit.normal));
        }
        if crate::envc::var_os("HP3_FALL_DEBUG").is_some() {
            eprintln!("fall: {} vel {vel} step {} hit time {} normal {} actor {:?}", hp3_script::natives::object_path(vm, a), vel * tick, hit.time, hit.normal, hit.actor.map(|o| hp3_script::natives::object_path(vm, o)));
        }
        // Coming down on the edge of a step counts as landing on the step when its top lies under the middle.
        let landing = surface_normal(vm, a, &hit, hit.location, 24.0);
        if crate::envc::var_os("HP3_FALL_DEBUG").is_some() {
            eprintln!("fall: landing normal {landing} at {}", vec(&get(vm, a, "Location")));
        }
        // A bouncing actor (a bean tossed out of a chest) does not land: it hears `HitWall`, turns its velocity round
        // there and keeps falling, until its script settles it.
        if get(vm, a, "bBounce").as_bool() {
            fire(vm, a, "HitWall", &mut [value(landing), Value::Object(hit.actor)]);
            continue;
        }
        if landing.z >= min_floor_z(vm, a) {
            let hit = Hit { normal: landing, ..hit };
            // Lands with the speed it really came down at (when it moved far enough to tell).
            if !get(vm, a, "bJustTeleported").as_bool() && hit.time > 0.1 && hit.time * tick > 0.003 {
                let v = (vec(&get(vm, a, "Location")) - from) / (tick * hit.time);
                set(vm, a, "Velocity", value(v));
            }
            if pawn {
                set(vm, a, "Floor", value(hit.normal));
            }
            let mut v = vec(&get(vm, a, "Velocity"));
            if vm.db.objects.get(a).class.is_some_and(|c| vm.db.names.get(vm.db.get(c).name) == "WizardCracker") {
                let at = vec(&get(vm, a, "Location"));
                vm.log.push(format!("cracker landed at {at}"));
            }
            // What the landing spot overlaps is known before the landing is announced (the original's move updates its touches
            // itself): a spongify pad under the feet is in `Touching` when the character's `Landed` looks for it, so a jump
            // or step down onto it bounces instead of only a fast fall.
            if pawn {
                crate::touch::settle(vm, a);
            }
            // The controller hears the landing first and can keep it from the pawn (the hero's slide takes the
            // falling speed there, the fire crab's flip counts its bounces).
            let held = pawn
                && get(vm, a, "Controller").as_obj().is_some_and(|c| fire(vm, c, "NotifyLanded", &mut [value(hit.normal)]).as_bool());
            if !held {
                fire(vm, a, "Landed", &mut [value(hit.normal)]);
            }
            if get(vm, a, "Physics").as_byte() == PHYS_FALLING {
                set(vm, a, "Physics", Value::Byte(PHYS_WALKING));
            }
            v.z = 0.0;
            set(vm, a, "Velocity", value(v));
            return;
        } else if try_mount(vm, a, Vec3::Z, &hit) {
            return;
        } else {
            if pawn {
                notify_hit_wall(vm, a, &hit);
            } else {
                log_block(vm, a, &hit);
                fire(vm, a, "HitWall", &mut [value(hit.normal), Value::Object(hit.actor)]);
            }
            if vm.db.objects.get(a).deleted || get(vm, a, "Physics").as_byte() != PHYS_FALLING {
                return;
            }
            // Against a wall the start speed is the one it slid along at (keeping its rise or fall), so the
            // carry-on does not push it back into the wall.
            if pawn && (vec(&get(vm, a, "Location")) - from).length_squared() < 1.0e-4 {
                // Wedged on a slope the box cannot slide down (a corner of a chute: the slope under it, a wall
                // beside it and one ahead). The original's cylinder rolls out of such a corner; the box steps
                // sideways along the free way that leads most downhill, and the slope takes it from there.
                let (loc0, ext0) = (vec(&get(vm, a, "Location")), extent(vm, a));
                let under = world(vm).collision.trace_as(true, loc0, loc0 - Vec3::Z * 8.0, ext0, Some(a)).map_or(landing, |h| h.normal);
                let down = Vec3::new(under.x, under.y, 0.0).normalize_or_zero();
                let mut dirs: Vec<Vec3> = (0..8).map(|i| {
                    let t = i as f32 * std::f32::consts::FRAC_PI_4;
                    Vec3::new(t.cos(), t.sin(), 0.0)
                }).collect();
                dirs.sort_by(|p, q| q.dot(down).total_cmp(&p.dot(down)));
                for d in dirs {
                    let at = vec(&get(vm, a, "Location"));
                    if move_actor(vm, a, d * 2.0).is_none() {
                        if crate::envc::var_os("HP3_FALL_DEBUG").is_some() {
                            eprintln!("fall: unwedged by {d} at {at}");
                        }
                        break;
                    }
                }
            }
            let slid = (vec(&get(vm, a, "Location")) - from) / tick;
            start_vel = Vec3::new(slid.x, slid.y, start_vel.z);
            refine(vm, start_vel);
        }
    }
}

/// A pawn in the air under its own power, as the original flies it: the speed drifts toward the way it is pushed with
/// the volume's fluid friction (half of it), caps at `AirSpeed` (scaled by `DesiredSpeed` when no player steers),
/// lets the script change it (`ModifyVelocity`), slides along what it meets and says `HitWall`, and ends with the
/// speed it really moved at.
fn fly_pawn(vm: &mut Vm<'_>, a: ObjId, dt: f32) {
    if dt <= 0.0 {
        return;
    }
    let human = world(vm).player.is_some_and(|pc| get(vm, pc, "Pawn").as_obj() == Some(a));
    let accel_rate = get(vm, a, "AccelRate").as_float();
    let mut accel = vec(&get(vm, a, "Acceleration"));
    let dir = accel.normalize_or_zero();
    if accel.length_squared() > accel_rate * accel_rate {
        accel = dir * accel_rate;
        set(vm, a, "Acceleration", value(accel));
    }
    let volume = get(vm, a, "PhysicsVolume").as_obj();
    let friction = 0.5 * volume.map_or(0.0, |v| get(vm, v, "FluidFriction").as_float());
    // The fly and ghost cheats: a ghost leaves the zones it passes through behind (their fluid friction can be nothing), so the debug
    // flight brakes by itself, to a stop in about a fifth of a second once no key is held.
    let friction = if human && world(vm).debug_air_speed.is_some() { friction.max(5.0) } else { friction };
    // Speed settles where the push balances that friction (push / friction), and the script's push is capped by AccelRate
    // (walking-sized), so left alone the debug flight tops out near walking speed whatever AirSpeed says: push hard
    // enough to reach AirSpeed (the cap below still holds it).
    if human && world(vm).debug_air_speed.is_some() && accel.length_squared() > 0.0 {
        accel = dir * get(vm, a, "AirSpeed").as_float() * friction;
    }
    let drift = match volume {
        Some(v) => vec(&get(vm, v, "ZoneVelocity")),
        None => Vec3::ZERO,
    };
    let old = vec(&get(vm, a, "Velocity"));
    let speed = old.length();
    let mut vel = old - (old - dir * speed) * dt * friction;
    vel = vel * (1.0 - friction * dt) + accel * dt;
    set(vm, a, "Velocity", value(vel));
    fire(vm, a, "ModifyVelocity", &mut [Value::Float(dt), value(old)]);
    if vm.db.objects.get(a).deleted || get(vm, a, "Physics").as_byte() != PHYS_FLYING {
        return;
    }
    let mut max = get(vm, a, "AirSpeed").as_float();
    if !human {
        let desired = get(vm, a, "DesiredSpeed").as_float();
        if desired > 0.0 {
            max *= desired;
        }
    }
    vel = vec(&get(vm, a, "Velocity"));
    if max > 0.0 && vel.length_squared() > max * max {
        vel = vel.normalize() * max;
    }
    set(vm, a, "Velocity", value(vel));
    let from = vec(&get(vm, a, "Location"));
    set(vm, a, "bJustTeleported", Value::Bool(false));
    let zone = if human || drift.length_squared() > 90000.0 { drift } else { Vec3::ZERO };
    if let Some(hit) = move_and_slide(vm, a, (vel + zone) * dt) {
        notify_hit_wall(vm, a, &hit);
    }
    if vm.db.objects.get(a).deleted {
        return;
    }
    if !get(vm, a, "bJustTeleported").as_bool() && !get(vm, a, "bNoVelocityUpdate").as_bool() {
        let moved = (vec(&get(vm, a, "Location")) - from) / dt;
        set(vm, a, "Velocity", value(moved));
    }
    set(vm, a, "bNoVelocityUpdate", Value::Bool(false));
}

fn fly(vm: &mut Vm<'_>, a: ObjId, dt: f32) {
    if is_pawn(vm, a) && get(vm, a, "Physics").as_byte() == PHYS_FLYING {
        fly_pawn(vm, a, dt);
        return;
    }
    let accel = vec(&get(vm, a, "Acceleration"));
    let mut vel = vec(&get(vm, a, "Velocity"));
    let max = get(vm, a, "AirSpeed").as_float().max(1.0);
    // Braking: speed bleeds off by 5 per second, whether or not something pushes.
    vel *= (1.0 - 5.0 * dt).max(0.0);
    vel += accel * dt;
    if vel.length() > max {
        vel = vel.normalize() * max;
    }
    set(vm, a, "Velocity", value(vel));
    move_and_slide(vm, a, vel * dt);
}

fn projectile(vm: &mut Vm<'_>, a: ObjId, dt: f32) {
    let vel = vec(&get(vm, a, "Velocity")) + vec(&get(vm, a, "Acceleration")) * dt;
    set(vm, a, "Velocity", value(vel));
    if let Some(hit) = move_actor(vm, a, vel * dt) {
        // Stopped by the level or by an actor that blocks it: the script hears `HitWall` with what stopped it
        // (the level itself for its geometry). Actors it only overlaps are touched, not hit.
        let wall = hit.actor.or(world(vm).level_info);
        fire(vm, a, "HitWall", &mut [value(hit.normal), Value::Object(wall)]);
    }
}

/// One angle of a rotating actor moved by `step` this tick: toward `want` when it rotates to its desired rotation
/// (the short way, or always forward in the step's direction with a fixed direction), round and round by the step
/// with only a fixed direction, as the original's rotating physics turns each angle.
fn rotating_turn(cur: i32, want: i32, step: i32, fixed_dir: bool, to_desired: bool) -> i32 {
    let (cur, want) = (cur & 0xFFFF, want & 0xFFFF);
    match (fixed_dir, to_desired) {
        (true, true) if step > 0 => (cur + step.min((want - cur).rem_euclid(65536))) & 0xFFFF,
        (true, true) => (cur - (-step).min((cur - want).rem_euclid(65536))) & 0xFFFF,
        (true, false) => (cur + step) & 0xFFFF,
        (false, true) => fixed_turn(cur, want, step),
        (false, false) => cur,
    }
}

fn rotate(vm: &mut Vm<'_>, a: ObjId, dt: f32) {
    // A pawn someone controls is turned by its own rules (pawn_rotation), not by the rotating physics as well: the
    // hippogriff in COMC's win cutscene spun round its pitch at its 80000 rate when both turned it.
    if is_pawn(vm, a) && get(vm, a, "Controller").as_obj().is_some() {
        return;
    }
    let (to_desired, fixed_dir) = (get(vm, a, "bRotateToDesired").as_bool(), get(vm, a, "bFixedRotationDir").as_bool());
    if !to_desired && !fixed_dir {
        return;
    }
    let rate = get(vm, a, "RotationRate");
    let rot = get(vm, a, "Rotation");
    let (r, c) = (rate.fields(), rot.fields());
    if r.len() == 3 && c.len() == 3 {
        let old_rot = rotator_ints(&rot);
        let want = rotator_ints(&get(vm, a, "DesiredRotation"));
        let same = |x: [i32; 3], y: [i32; 3]| (0..3).all(|i| x[i] & 0xFFFF == y[i] & 0xFFFF);
        if to_desired && same(old_rot, want) {
            return;
        }
        let mut new_rot = old_rot;
        for i in 0..3 {
            let step = (r[i].as_int() as f32 * dt) as i32;
            if step != 0 && (!to_desired || old_rot[i] & 0xFFFF != want[i] & 0xFFFF) {
                new_rot[i] = rotating_turn(old_rot[i], want[i], step, fixed_dir, to_desired);
            }
        }
        if same(old_rot, new_rot) {
            return;
        }
        let v: Vec<Value> = new_rot.iter().map(|&x| Value::Int(x & 0xFFFF)).collect();
        set(vm, a, "Rotation", Value::Struct(v.into()));
        if to_desired && same(new_rot, want) {
            fire(vm, a, "EndedRotation", &mut []);
        }
        // A spinning platform (a mover) takes its collision and what stands on it round with it.
        if world(vm).collision.mover(a).is_some() {
            place_mover_shape(vm, a);
            let at = vec(&get(vm, a, "Location"));
            carry_riders(vm, a, at, Vec3::ZERO, old_rot, new_rot);
        }
    }
}

/// Turns a pawn toward its desired rotation (yaw) at its rotation rate.
fn turn_to_desired(vm: &mut Vm<'_>, a: ObjId, dt: f32) {
    if !get(vm, a, "bRotateToDesired").as_bool() {
        return;
    }
    let desired = get(vm, a, "DesiredRotation");
    let rot = get(vm, a, "Rotation");
    let rate = get(vm, a, "RotationRate");
    let (d, c, r) = (desired.fields(), rot.fields(), rate.fields());
    if d.len() != 3 || c.len() != 3 || r.len() != 3 {
        return;
    }
    let mut out: Vec<Value> = c.to_vec();
    let cur = c[1].as_int() & 0xFFFF;
    let want = d[1].as_int() & 0xFFFF;
    let mut diff = (want - cur) & 0xFFFF;
    if diff > 32768 {
        diff -= 65536;
    }
    let step = (r[1].as_int().abs() as f32 * dt) as i32;
    let turn = diff.clamp(-step.max(1), step.max(1));
    out[1] = Value::Int((cur + turn) & 0xFFFF);
    set(vm, a, "Rotation", Value::Struct(out.into()));
}

/// Runs one physics step for every moving actor.
pub const PHYS_MOVING_BRUSH: u8 = 8;

fn indexed_vec(vm: &mut Vm<'_>, a: ObjId, name: &str, i: usize) -> Vec3 {
    let Some(s) = crate::slot(vm, a, name) else { return Vec3::ZERO };
    vm.db.objects.values(a).and_then(|v| v.get(s + i)).map(vec).unwrap_or(Vec3::ZERO)
}

fn indexed_rot(vm: &mut Vm<'_>, a: ObjId, name: &str, i: usize) -> [i32; 3] {
    let Some(s) = crate::slot(vm, a, name) else { return [0; 3] };
    let r: Vec<i32> = vm.db.objects.values(a).and_then(|v| v.get(s + i)).map(|v| v.fields().iter().map(Value::as_int).collect()).unwrap_or_default();
    [r.first().copied().unwrap_or(0), r.get(1).copied().unwrap_or(0), r.get(2).copied().unwrap_or(0)]
}

/// Movers between keyframes (`InterpolateTo`): from key `PrevKeyNum` to `KeyNum` as `PhysAlpha` goes from 0
/// to 1 at `PhysRate`, gliding (eased) or moving evenly by `MoverGlideType`; `InterpolateEnd` at the end.
fn rotator_ints(v: &Value) -> [i32; 3] {
    let f = v.fields();
    [0, 1, 2].map(|i| f.get(i).map(Value::as_int).unwrap_or(0))
}

fn rotator_value(r: [i32; 3]) -> Value {
    Value::Struct(Box::new(r.map(Value::Int)))
}

/// Puts a mover's collision shape where the actor now is.
pub fn place_mover_shape(vm: &mut Vm<'_>, a: ObjId) {
    let t = crate::mover_transform(vm, a);
    if let Some(m) = world(vm).collision.mover_mut(a) {
        m.place(t);
    }
}

/// The rotation a mover has at fraction `f` of its way from where it started (`OldRot`) to its key's rotation.
/// With `bUseShortestRotation` each angle takes the short way round, and a leftover under 10 units is ignored.
fn mover_rotation(vm: &mut Vm<'_>, a: ObjId, f: f32, done: bool) -> [i32; 3] {
    let old = rotator_ints(&get(vm, a, "OldRot"));
    let key = get(vm, a, "KeyNum").as_byte() as usize;
    let (base, k) = (rotator_ints(&get(vm, a, "BaseRot")), indexed_rot(vm, a, "KeyRot", key));
    let shortest = get(vm, a, "bUseShortestRotation").as_bool();
    // At the end the mover is exactly at its key's turn from where it was placed (`BaseRot`).
    if shortest && done {
        return [0, 1, 2].map(|i| (base[i] & 0xFFFF) + k[i]);
    }
    [0, 1, 2].map(|i| {
        let goal = if shortest { (base[i] & 0xFFFF) + k[i] } else { base[i] + k[i] };
        let mut d = goal - old[i];
        if shortest {
            d = (d + 32768).rem_euclid(65536) - 32768;
            if d.abs() < 10 {
                d = 0;
            }
        }
        old[i] + (d as f32 * f) as i32
    })
}

/// Whether `other` is something a mover has to deal with when it moves into it: it blocks the mover's kind of
/// actor, is not part of the level's fixed decoration and is not riding the mover (or carrying it).
fn can_be_encroached(vm: &mut Vm<'_>, mover: ObjId, other: ObjId) -> bool {
    if other == mover || vm.db.objects.get(other).deleted || Some(other) == world(vm).level_info {
        return false;
    }
    if !get(vm, other, "bCollideActors").as_bool() || !get(vm, other, "bBlockActors").as_bool() {
        return false;
    }
    // A mover passes through the world's own actors (the original's `ActorEncroachmentCheck` skips `bWorldGeometry` ones for an
    // encroacher): the invisible `BlockingVolume`s that keep pawns out do not stop a door sliding open.
    if get(vm, other, "bWorldGeometry").as_bool() {
        return false;
    }
    if stands_on(vm, other, mover) || stands_on(vm, mover, other) || world(vm).collision.mover(other).is_some() {
        return false;
    }
    // What is hard-attached to a mover (a knight fixed to its alcove) goes where that mover goes and is not in the way of the other
    // movers that pass it: the alcoves of the knight ring turn through each other's places.
    if get(vm, other, "bHardAttach").as_bool() && get(vm, other, "Base").as_obj().is_some_and(|b| world(vm).collision.mover(b).is_some()) {
        return false;
    }
    let class = vm.db.objects.get(other).class;
    !vm.db.find("StaticMeshActor").is_some_and(|sm| class.is_some_and(|c| vm.is_child_of(c, sm)))
}

/// Actors that overlap the mover where it now is, among those it has to deal with.
fn encroached_by(vm: &mut Vm<'_>, a: ObjId) -> Vec<ObjId> {
    encroached_by_margin(vm, a, 0.0)
}

/// Like `encroached_by`, with every actor's box made `margin` smaller on each side, so only a real overlap counts and an actor
/// that merely touches the mover (standing on it) does not.
fn encroached_by_margin(vm: &mut Vm<'_>, a: ObjId, margin: f32) -> Vec<ObjId> {
    // Only blocking actors near the mover can be in its way (their boxes are kept for the frame).
    let at = vec(&get(vm, a, "Location"));
    let reach = world(vm).collision.mover(a).map_or(0.0, |m| m.reach());
    let near: Vec<ObjId> = world(vm)
        .collision
        .actors
        .iter()
        .filter(|b| b.center.distance(at) <= reach + b.extent.length())
        .map(|b| b.actor)
        .collect();
    let mut found = Vec::new();
    for o in near {
        if !can_be_encroached(vm, a, o) {
            continue;
        }
        let (at, ext) = (vec(&get(vm, o, "Location")), extent(vm, o));
        if world(vm).collision.mover(a).is_some_and(|m| m.overlaps(at, (ext - Vec3::splat(margin)).max(Vec3::splat(0.5)))) {
            found.push(o);
        }
    }
    found
}

/// How far a pawn may rest inside a mover's triangles without being in its way: `move_actor` stops a box against a surface
/// with a hair of up to 3 units to spare (its smaller-box sweeps), so the pawn that bumped a door open stands that far into
/// the door's skin. Only a deeper overlap is pushed, refused or told `EncroachedBy`.
const ENCROACH_SKIN: f32 = 3.0;

/// How deep a step must put a pawn that was already leaning on the mover before it counts as pushed into.
const ENCROACH_DEEP: f32 = 8.0;

/// Whether the mover may take the step to `to`/`rot`: every actor in its way is first pushed along with it, and
/// if one is still in the way the mover's own `EncroachingOn` decides (true: the step is refused). Actors it
/// does end up overlapping are told with `EncroachedBy`.
fn clear_way(vm: &mut Vm<'_>, a: ObjId, from: Vec3, to: Vec3, rot: [i32; 3]) -> bool {
    let old_rot = rotator_ints(&get(vm, a, "Rotation"));
    // What the mover already overlaps where it stands was placed that way (a gargoyle in the niche behind a portrait): the
    // step neither pushes nor crushes it, the mover slides through.
    let mut overlapping_now = slide_through(vm, a);
    // A pawn that leans on the mover (the player pressing into a door) already overlaps its triangles a little where it stands. A
    // step along that face does not push into it, however far it sinks in: it only counts as in the way when the step puts it deep
    // inside the mover.
    let leaning = encroached_by_margin(vm, a, 0.0);
    set(vm, a, "Location", value(to));
    set(vm, a, "Rotation", rotator_value(rot));
    place_mover_shape(vm, a);
    let deep = encroached_by_margin(vm, a, ENCROACH_DEEP);
    let grazed: Vec<ObjId> = leaning.into_iter().filter(|o| !deep.contains(o) && !overlapping_now.contains(o)).collect();
    overlapping_now.extend(grazed);
    let in_way: Vec<ObjId> = encroached_by_margin(vm, a, ENCROACH_SKIN).into_iter().filter(|o| !overlapping_now.contains(o)).collect();
    if crate::envc::var_os("HP3_TRACE_MOVER").is_some() {
        let names: Vec<String> = in_way.iter().map(|&o| hp3_script::natives::object_path(vm, o)).collect();
        let skipped: Vec<String> = overlapping_now.iter().map(|&o| hp3_script::natives::object_path(vm, o)).collect();
        eprintln!("mover {} step to {to}: in the way {names:?}, already overlapping {skipped:?}", hp3_script::natives::object_path(vm, a));
    }
    // Pushing happens with the mover where it still is.
    set(vm, a, "Location", value(from));
    set(vm, a, "Rotation", rotator_value(old_rot));
    place_mover_shape(vm, a);
    for &o in &in_way {
        move_and_slide(vm, o, to - from);
    }
    set(vm, a, "Location", value(to));
    set(vm, a, "Rotation", rotator_value(rot));
    place_mover_shape(vm, a);
    let mut refused = false;
    for &o in &in_way {
        if !encroached_by_margin(vm, a, ENCROACH_SKIN).contains(&o) {
            continue;
        }
        if fire(vm, a, "EncroachingOn", &mut [Value::Object(Some(o))]).as_bool() {
            refused = true;
            break;
        }
    }
    if refused {
        set(vm, a, "Location", value(from));
        set(vm, a, "Rotation", rotator_value(old_rot));
        place_mover_shape(vm, a);
        return false;
    }
    for o in encroached_by_margin(vm, a, ENCROACH_SKIN).into_iter().filter(|o| !overlapping_now.contains(o)) {
        fire(vm, o, "EncroachedBy", &mut [Value::Object(Some(a))]);
    }
    true
}

/// Carries what rides on the mover along its step: the same shift, plus the swing a rotation gives a rider at
/// some distance from the pivot, and the turn itself for the rider's heading.
/// The actors standing on (based on) `a`: found by reading the `Base` slot of every actor directly, not by name.
fn riders_of(vm: &mut Vm<'_>, a: ObjId) -> Vec<ObjId> {
    let base = crate::actor_slot(vm, "Base");
    if !world(vm).ever_base_valid {
        let mut found = hp3_script::fx::FxHashSet::default();
        let all = world(vm).actors.clone();
        for r in all {
            if let Some(b) = crate::read(vm, r, base).as_obj() {
                found.insert(b);
            }
        }
        let w = world(vm);
        w.ever_base.extend(found);
        w.ever_base_valid = true;
    }
    if !world(vm).ever_base.contains(&a) {
        return Vec::new();
    }
    let actors = std::mem::take(&mut world(vm).actors);
    let riders = actors.iter().copied().filter(|&r| r != a && crate::read(vm, r, base).as_obj() == Some(a)).collect();
    world(vm).actors = actors;
    riders
}

fn carry_riders(vm: &mut Vm<'_>, a: ObjId, from: Vec3, delta: Vec3, old_rot: [i32; 3], new_rot: [i32; 3]) {
    use hp3_script::natives::rot_axes;
    let riders = riders_of(vm, a);
    let (o, n) = (rot_axes(old_rot), rot_axes(new_rot));
    let turn = [0, 1, 2].map(|i| ((new_rot[i] & 0xFFFF) - (old_rot[i] & 0xFFFF) + 32768).rem_euclid(65536) - 32768);
    for r in riders {
        if vm.db.objects.get(r).deleted {
            continue;
        }
        let rel = vec(&get(vm, r, "Location")) - from;
        let mut shift = delta;
        if old_rot != new_rot {
            let along = |axis: [f32; 3]| Vec3::from(axis);
            let swung = along(n[0]) * along(o[0]).dot(rel) + along(n[1]) * along(o[1]).dot(rel) + along(n[2]) * along(o[2]).dot(rel);
            shift += swung - rel;
            let mut heading = rotator_ints(&get(vm, r, "Rotation"));
            let pawn = is_pawn(vm, r);
            for i in 0..3 {
                if !(pawn && i == 2) {
                    heading[i] += turn[i];
                }
            }
            set(vm, r, "Rotation", rotator_value(heading));
            if pawn && let Some(c) = get(vm, r, "Controller").as_obj() {
                let mut cr = rotator_ints(&get(vm, c, "Rotation"));
                cr[0] += turn[0];
                cr[1] += turn[1];
                set(vm, c, "Rotation", rotator_value(cr));
            }
        }
        if get(vm, r, "bHardAttach").as_bool() {
            // Hard-attached to the mover: it goes exactly where the mover goes, whatever is in the way.
            let there = vec(&get(vm, r, "Location")) + shift;
            set(vm, r, "Location", value(there));
            crate::touch::settle(vm, r);
        } else {
            move_and_slide(vm, r, shift);
        }
    }
}

/// Movers between keyframes (`InterpolateTo` sets `OldPos`/`OldRot`, `PhysAlpha` 0, `PhysRate`): they go from
/// where they started to the key's place as `PhysAlpha` runs to 1, gliding (eased) or moving evenly by
/// `MoverGlideType`. What is in the way is pushed or may refuse the step (`EncroachingOn`); what rides the mover
/// is carried; arriving clears `bInterpolating` and raises `KeyFrameReached`.
fn moving_brush(vm: &mut Vm<'_>, a: ObjId, dt: f32) {
    if !get(vm, a, "bInterpolating").as_bool() {
        return;
    }
    let started = vec(&get(vm, a, "Location"));
    let mut first_step = get(vm, a, "PhysAlpha").as_float() <= 0.0;
    let mut left = dt;
    while get(vm, a, "bInterpolating").as_bool() && left > 0.0 {
        let rate = get(vm, a, "PhysRate").as_float();
        let from_alpha = get(vm, a, "PhysAlpha").as_float();
        let mut alpha = from_alpha + left * rate;
        if alpha > 1.0 {
            // The step's remainder carries on after the arrival; the time used is only up to it.
            left *= (alpha - 1.0) / (alpha - from_alpha);
            alpha = 1.0;
        } else {
            left = 0.0;
        }
        let f = if get(vm, a, "MoverGlideType").as_byte() == 1 { alpha * alpha * (3.0 - 2.0 * alpha) } else { alpha };
        let key = get(vm, a, "KeyNum").as_byte() as usize;
        let (old_pos, goal) = (vec(&get(vm, a, "OldPos")), vec(&get(vm, a, "BasePos")) + indexed_vec(vm, a, "KeyPos", key));
        let to = old_pos + (goal - old_pos) * f;
        let rot = mover_rotation(vm, a, f, alpha >= 1.0);
        let here = vec(&get(vm, a, "Location"));
        let here_rot = rotator_ints(&get(vm, a, "Rotation"));
        if !clear_way(vm, a, here, to, rot) {
            // Turned back by what is in the way: the alpha stays and it tries again next time.
            break;
        }
        carry_riders(vm, a, here, to - here, here_rot, rot);
        if first_step && to != here {
            first_step = false;
            if crate::envc::var_os("HP3_MOVER_LOG").is_some() {
                let riders = riders_of(vm, a).len();
                let name = hp3_script::natives::object_path(vm, a);
                vm.log.push(format!("mover {name} moves with {riders} riders"));
            }
        }
        set(vm, a, "PhysAlpha", Value::Float(alpha));
        if alpha >= 1.0 {
            set(vm, a, "bInterpolating", Value::Bool(false));
            vm.finish_latent(a);
            let mut v = vec(&get(vm, a, "Velocity"));
            v.z = v.z.max(rate * (goal.z - old_pos.z));
            set(vm, a, "Velocity", value(v));
            fire(vm, a, "KeyFrameReached", &mut []);
        }
    }
    let moved = vec(&get(vm, a, "Location")) - started;
    if dt > 0.0 {
        set(vm, a, "Velocity", value(moved / dt));
    }
}

/// Prepares a frame's physics: the blocking actors' boxes.
pub fn begin_frame(vm: &mut Vm<'_>) {
    refresh_actor_boxes(vm);
    let s = Slots::get(vm);
    world(vm).phys_slots = Some(s);
}

/// Runs one actor's physics for `dt` seconds (nothing for static actors and those without physics).
pub fn step_actor(vm: &mut Vm<'_>, a: ObjId, dt: f32) {
    if vm.db.objects.get(a).deleted {
        return;
    }
    let s = match world(vm).phys_slots {
        Some(s) => s,
        None => Slots::get(vm),
    };
    if crate::read(vm, a, s.physics).as_byte() == PHYS_NONE || crate::read(vm, a, s.is_static).as_bool() {
        return;
    }
    if crate::envc::var("HP3_STEP_DEBUG").is_ok_and(|n| hp3_script::natives::object_path(vm, a).ends_with(&n)) {
        let at = vec(&get(vm, a, "Location"));
        let ext = extent(vm, a);
        let down = world(vm).collision.trace_as(false, at, at - Vec3::Z * 30.0, ext, Some(a));
        let thin = world(vm).collision.trace_as(false, at, at - Vec3::Z * 30.0, Vec3::new(1.0, 1.0, ext.z), Some(a));
        eprintln!("step: {} phys {} at {} ext {ext} vel {} down {:?} thin {:?}", hp3_script::natives::object_path(vm, a), crate::read(vm, a, s.physics).as_byte(), at, vec(&get(vm, a, "Velocity")), down.map(|h| (h.time, h.normal)), thin.map(|h| (h.time, h.normal)));
    }
    // HP3_SLIDE_DEBUG: the player's pawn, every tick it is in the slide or has just left it (state, place, speed, floor).
    if crate::envc::var_os("HP3_SLIDE_DEBUG").is_some() && is_player_pawn(vm, a) {
        let ctl = get(vm, a, "Controller").as_obj();
        let state = ctl.and_then(|c| vm.state_of(c)).map(|s| vm.db.names.get(vm.db.get(s).name).to_string()).unwrap_or_default();
        let sliding = state == "PlayerSliding";
        if sliding || SLIDE_LOGGED.with(|l| l.replace(sliding)) {
            SLIDE_LOGGED.with(|l| l.set(sliding));
            let volume = get(vm, a, "PhysicsVolume").as_obj().map(|v| hp3_script::natives::object_path(vm, v)).unwrap_or_default();
            let frame = world(vm).frame;
            let phys = crate::read(vm, a, s.physics).as_byte();
            let (at, vel, floor) = (vec(&get(vm, a, "Location")), vec(&get(vm, a, "Velocity")), vec(&get(vm, a, "Floor")));
            let (r, h) = (get(vm, a, "CollisionRadius").as_float(), get(vm, a, "CollisionHeight").as_float());
            let crouched = get(vm, a, "bIsCrouched").as_bool();
            eprintln!("slide: frame {frame} state {state} phys {phys} at {at} vel {vel} floor {floor} vol {volume} size {r}x{h} crouched {crouched}");
        }
    }
    if MOUNT_GOAL.with(|g| g.get().is_some()) {
        finish_mount(vm, a);
    }
    if MOUNT_GLIDE.with(|g| g.get().is_some()) && glide_to_ledge(vm, a, dt) {
        return;
    }
    // The speed before this step: a pawn leans into the change (pawn_rotation).
    let old_vel = crate::read_vec(vm, a, s.velocity);
    match crate::read(vm, a, s.physics).as_byte() {
        PHYS_MOVING_BRUSH => {
            moving_brush(vm, a, dt);
            return;
        }
        PHYS_WALKING | PHYS_FALLING if fell_out_of_world(vm, a) => return,
        PHYS_WALKING => walk_smoothed(vm, a, dt),
        PHYS_FALLING => {
            drop_step_offset(vm, a);
            fall(vm, a, dt)
        }
        PHYS_FLYING | PHYS_SWIMMING => {
            drop_step_offset(vm, a);
            fly(vm, a, dt)
        }
        PHYS_ROTATING => rotate(vm, a, dt),
        PHYS_PROJECTILE => projectile(vm, a, dt),
        PHYS_TRAILER => {
            trailer(vm, a);
            return;
        }
        // An interpolation manager moves its owner along a path of points.
        PHYS_INTERPOLATING => {
            if is_interpolation_manager(vm, a) {
                crate::interpolation::step(vm, a, dt);
            }
            return;
        }
        _ => return,
    }
    if is_pawn(vm, a) {
        pawn_rotation(vm, a, dt, old_vel);
        crate::update_physics_volume(vm, a);
        if let Some(c) = get(vm, a, "Controller").as_obj() {
            crate::update_controller_volume(vm, c);
        }
    } else {
        turn_to_desired(vm, a, dt);
    }
}

/// The direction a rotator (pitch, yaw in 65536ths of a turn) points.
fn rotator_direction(r: &Value) -> Vec3 {
    let f = r.fields();
    let k = std::f32::consts::TAU / 65536.0;
    let (pitch, yaw) = (f.first().map_or(0, Value::as_int) as f32 * k, f.get(1).map_or(0, Value::as_int) as f32 * k);
    Vec3::new(pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), pitch.sin())
}

/// The rotator (roll 0) that points along `v`.
pub(crate) fn direction_rotator(v: Vec3) -> Value {
    let k = 65536.0 / std::f32::consts::TAU;
    let yaw = (v.y.atan2(v.x) * k).round() as i32 & 0xFFFF;
    let pitch = (v.z.atan2(v.truncate().length()) * k).round() as i32 & 0xFFFF;
    Value::Struct(vec![Value::Int(pitch), Value::Int(yaw), Value::Int(0)].into())
}

/// `PHYS_Trailer`: the actor stays with its owner (effects, proxy triggers). Nothing happens without an owner or
/// while the actor is attached to a base. A sprite may keep the owner's pivot offset or trail behind it
/// (`bTrailerPrePivot`, `bTrailerSameRotation` with `Mass` as the distance). Others take the owner's place and
/// face against its movement (pointing up when it stands still), or its own rotation with `bTrailerSameRotation`;
/// `bTrailerAllowRotation` leaves their rotation alone.
fn trailer(vm: &mut Vm<'_>, a: ObjId) {
    let Some(owner) = get(vm, a, "Owner").as_obj() else { return };
    if get(vm, a, "Base").as_obj().is_some() {
        return;
    }
    let mut at = vec(&get(vm, owner, "Location"));
    let same_rotation = get(vm, a, "bTrailerSameRotation").as_bool();
    if get(vm, a, "DrawType").as_byte() == 1 {
        if get(vm, a, "bTrailerPrePivot").as_bool() {
            at += vec(&get(vm, a, "PrePivot"));
        } else if same_rotation {
            at -= get(vm, a, "Mass").as_float() * rotator_direction(&get(vm, owner, "Rotation"));
        }
        set(vm, a, "Location", value(at));
        return;
    }
    set(vm, a, "Location", value(at));
    if get(vm, a, "bTrailerAllowRotation").as_bool() {
        return;
    }
    let rotation = if same_rotation {
        get(vm, owner, "Rotation")
    } else {
        let velocity = vec(&get(vm, owner, "Velocity"));
        if velocity.length_squared() < 1e-4 {
            Value::Struct(vec![Value::Int(16384), Value::Int(0), Value::Int(0)].into())
        } else {
            direction_rotator(-velocity)
        }
    };
    set(vm, a, "Rotation", rotation);
}

fn is_interpolation_manager(vm: &mut Vm<'_>, a: ObjId) -> bool {
    let Some(manager) = vm.db.find("InterpolationManager") else { return false };
    vm.db.objects.get(a).class.is_some_and(|c| vm.is_child_of(c, manager))
}

fn is_pawn(vm: &mut Vm<'_>, a: ObjId) -> bool {
    let Some(pawn) = vm.db.find("Pawn") else { return false };
    vm.db.objects.get(a).class.is_some_and(|c| vm.is_child_of(c, pawn))
}

/// Moves the angle `cur` toward `want` (both in Unreal's 65536 per turn) by at most `delta`, the short way round.
pub fn fixed_turn(cur: i32, want: i32, delta: i32) -> i32 {
    let (cur, want) = (cur & 0xFFFF, want & 0xFFFF);
    let mut diff = (want - cur) & 0xFFFF;
    if diff > 32768 {
        diff -= 65536;
    }
    (cur + diff.clamp(-delta.abs(), delta.abs())) & 0xFFFF
}

/// How a pawn turns toward its desired rotation each tick.
///
/// A pawn with no controller stays as it is. The player's pawn turns only when its controller asks
/// (`bRotateToDesired`), toward the controller's `DesiredRotation`, at twice `EnemyTurnSpeed`. Any other pawn
/// always turns toward its own `DesiredRotation` at `RotationRate.Yaw`, up to twice that when the angle left is
/// large. Pitch settles to level while on the floor, and roll to none.
/// How far a pawn leans (roll) after this tick when it does not roll to a desired rotation: as the original's pawns
/// bank into their acceleration. Slow walkers settle level; otherwise the sideways part of the real acceleration
/// (the speed change over the step) sets the roll the pawn eases toward, within `RotationRate.Roll`. `rot` is the
/// pawn's rotation after turning.
fn lean_roll(vm: &mut Vm<'_>, a: ObjId, dt: f32, old_vel: Vec3, rot: [i32; 3], max_lean: i32) -> i32 {
    let roll = rot[2] & 0xFFFF;
    let level = |roll: i32, smooth: f32| -> i32 {
        if roll < 32768 { (roll as f32 * (1.0 - smooth)) as i32 } else { (roll as f32 + (65536 - roll) as f32 * smooth) as i32 }
    };
    // The roll stays within the lean the pawn may have.
    let roll = if roll < 32768 { roll.min(max_lean) } else if roll < 65536 - max_lean { 65536 - max_lean } else { roll };
    let vel = vec(&get(vm, a, "Velocity"));
    let walking = get(vm, a, "Physics").as_byte() == PHYS_WALKING;
    if walking && vel.length_squared() < 40000.0 {
        return level(roll, (8.0 * dt).min(1.0));
    }
    let real = (vel - old_vel) / dt.max(1e-4);
    if real.length_squared() <= 10000.0 {
        return level(roll, (8.0 * dt).min(1.0));
    }
    let max_roll = if walking { 4096.0 } else { 28000.0 };
    let right = hp3_anim::rotator_matrix([rot[0], rot[1], 0]).y_axis.truncate();
    let side = real.dot(right);
    let accel_rate = get(vm, a, "AccelRate").as_float().max(1.0);
    let target = if side > 0.0 { max_lean.min((side * max_roll / accel_rate) as i32) } else { (65536 - max_lean).max((65536.0 + side * max_roll / accel_rate) as i32) };
    // Ease from the roll it has (on the same side of the turn as the target) to the target.
    let mut cur = roll;
    if target > 32768 {
        if cur < 32768 {
            cur += 65536;
        }
    } else if cur > 32768 {
        cur -= 65536;
    }
    let smooth = (5.0 * dt).min(1.0);
    (target as f32 * smooth + cur as f32 * (1.0 - smooth)) as i32 & 0xFFFF
}

fn pawn_rotation(vm: &mut Vm<'_>, a: ObjId, dt: f32, old_vel: Vec3) {
    let Some(c) = get(vm, a, "Controller").as_obj() else { return };
    let ints = |v: Value| -> [i32; 3] {
        let f = v.fields();
        [f.first().map_or(0, Value::as_int), f.get(1).map_or(0, Value::as_int), f.get(2).map_or(0, Value::as_int)]
    };
    let rot = ints(get(vm, a, "Rotation"));
    let human = world(vm).player == Some(c);
    let max_lean = ints(get(vm, a, "RotationRate"))[2];
    // A player's pawn that no input turns still leans into its movement.
    if human && !get(vm, c, "bRotateToDesired").as_bool() {
        if max_lean > 0 && !get(vm, a, "bRollToDesired").as_bool() {
            let roll = lean_roll(vm, a, dt, old_vel, rot, max_lean);
            if roll != rot[2] & 0xFFFF {
                set(vm, a, "Rotation", Value::Struct(vec![Value::Int(rot[0]), Value::Int(rot[1]), Value::Int(roll)].into()));
            }
        }
        return;
    }
    let (desired, delta) = if human {
        let mut d = ints(get(vm, c, "DesiredRotation"));
        // The pawn keeps its own desired roll (a pawn that rolls to its desired rotation, such as one hanging from a
        // Carpe Retractum, is leaned by the script).
        d[2] = ints(get(vm, a, "DesiredRotation"))[2];
        set(vm, a, "DesiredRotation", Value::Struct(vec![Value::Int(d[0]), Value::Int(d[1]), Value::Int(d[2])].into()));
        (d, (2.0 * get(vm, c, "EnemyTurnSpeed").as_float() * dt).round() as i32)
    } else {
        let d = ints(get(vm, a, "DesiredRotation"));
        let rate = ints(get(vm, a, "RotationRate"))[1].max(1);
        let mut left = ((d[1] - rot[1]) & 0xFFFF).abs();
        if left > 32768 {
            left = 65536 - left;
        }
        (d, ((2 * left).clamp(rate, 2 * rate) as f32 * dt).round() as i32)
    };
    let on_floor = matches!(get(vm, a, "Physics").as_byte(), PHYS_WALKING);
    // On the floor or falling the pawn stands upright, unless it rolls to its desired rotation.
    let roll_to_desired = get(vm, a, "bRollToDesired").as_bool();
    let want_pitch = if !roll_to_desired && (on_floor || get(vm, a, "Physics").as_byte() == PHYS_FALLING) { 0 } else { desired[0] };
    let yaw = fixed_turn(rot[1], desired[1], delta);
    let pitch = fixed_turn(rot[0], want_pitch, delta);
    let roll = if roll_to_desired {
        fixed_turn(rot[2], desired[2], delta)
    } else if max_lean > 0 {
        lean_roll(vm, a, dt, old_vel, [pitch, yaw, rot[2]], max_lean)
    } else {
        0
    };
    if yaw != rot[1] & 0xFFFF || pitch != rot[0] & 0xFFFF || roll != rot[2] & 0xFFFF {
        set(vm, a, "Rotation", Value::Struct(vec![Value::Int(pitch), Value::Int(yaw), Value::Int(roll)].into()));
    }
    if human {
        // The controller's heading follows the pawn's while it is turning the pawn.
        let mut cr = ints(get(vm, c, "Rotation"));
        cr[1] = yaw;
        cr[0] = fixed_turn(cr[0], ints(get(vm, c, "DesiredRotation"))[0], delta);
        set(vm, c, "Rotation", Value::Struct(cr.iter().map(|&v| Value::Int(v)).collect::<Vec<_>>().into()));
    }
}

/// Every actor's physics in level order (the old whole-level step, used by tools).
pub fn step(vm: &mut Vm<'_>, dt: f32) {
    refresh_actor_boxes(vm);
    let s = Slots::get(vm);
    // Only actors with physics move; most of a level has none.
    let actors = std::mem::take(&mut world(vm).actors);
    let movers: Vec<ObjId> = actors
        .iter()
        .copied()
        .filter(|&a| crate::read(vm, a, s.physics).as_byte() != PHYS_NONE && !crate::read(vm, a, s.is_static).as_bool())
        .collect();
    world(vm).actors = actors;
    if crate::envc::var_os("HP3_MOVERS").is_some() && world(vm).frame == 100 {
        let mut by: std::collections::BTreeMap<(u8, String), usize> = Default::default();
        for &a in &movers {
            let class = vm.db.objects.get(a).class.map(|c| vm.db.names.get(vm.db.get(c).name).to_string()).unwrap_or_default();
            *by.entry((crate::read(vm, a, s.physics).as_byte(), class)).or_default() += 1;
        }
        for ((p, c), n) in by {
            eprintln!("movers: physics {p} {c} x{n}");
        }
    }
    for a in movers {
        if vm.db.objects.get(a).deleted {
            continue;
        }
        match crate::read(vm, a, s.physics).as_byte() {
            PHYS_MOVING_BRUSH => {
                moving_brush(vm, a, dt);
                continue;
            }
            PHYS_WALKING | PHYS_FALLING if fell_out_of_world(vm, a) => continue,
            PHYS_WALKING => walk_smoothed(vm, a, dt),
            PHYS_FALLING => {
                drop_step_offset(vm, a);
                fall(vm, a, dt)
            }
            PHYS_FLYING | PHYS_SWIMMING => {
                drop_step_offset(vm, a);
                fly(vm, a, dt)
            }
            PHYS_ROTATING => rotate(vm, a, dt),
            PHYS_PROJECTILE => projectile(vm, a, dt),
            _ => continue,
        }
        turn_to_desired(vm, a, dt);
    }
}

#[cfg(test)]
mod mount_tests {
    use super::*;
    use crate::collision::Collision;

    /// A floor at z=0 and a wall at x=100 facing -x, 90 high, with a flat top reaching to x=300 (a ledge).
    fn ledge_world(wall_height: f32) -> Collision {
        let mut c = Collision::new(None);
        let quad = |c: &mut Collision, a: Vec3, b: Vec3, d: Vec3, e: Vec3| {
            c.add_triangle([a, b, d], None);
            c.add_triangle([a, d, e], None);
        };
        // The floor, the face of the wall, the top.
        quad(&mut c, Vec3::new(-500.0, -500.0, 0.0), Vec3::new(100.0, -500.0, 0.0), Vec3::new(100.0, 500.0, 0.0), Vec3::new(-500.0, 500.0, 0.0));
        quad(&mut c, Vec3::new(100.0, -500.0, 0.0), Vec3::new(100.0, -500.0, wall_height), Vec3::new(100.0, 500.0, wall_height), Vec3::new(100.0, 500.0, 0.0));
        quad(&mut c, Vec3::new(100.0, -500.0, wall_height), Vec3::new(300.0, -500.0, wall_height), Vec3::new(300.0, 500.0, wall_height), Vec3::new(100.0, 500.0, wall_height));
        c
    }

    fn pawn_at(x: f32, c: &Collision, reach: f32) -> Option<Vec3> {
        let ext = Vec3::new(15.0, 15.0, 38.0);
        let l = Ledge {
            loc: Vec3::new(x, 0.0, 38.5),
            up: Vec3::Z,
            normal: Vec3::new(-1.0, 0.0, 0.0),
            radius: 15.0,
            reach,
            min_rise: MAX_STEP_HEIGHT,
            ext,
            ignore: ObjId(0),
        };
        find_ledge(c, &l)
    }

    #[test]
    fn climbs_a_ledge_within_reach() {
        let c = ledge_world(70.0);
        let to = pawn_at(70.0, &c, 96.5).expect("a 70 high ledge can be mounted");
        // The destination is on top of the wall (the pawn's centre half its height above 70, plus the 2 margin).
        assert!((to.z - (70.0 + 38.0 + 2.0)).abs() < 1.5, "top at {}", to.z);
        assert!((to.x - 100.0).abs() < 1.5, "two radii past the pawn, at the wall's face: {}", to.x);
    }

    /// Carpe Retractum Mover34: a 96-high block (top 816) met mid-jump at the top of the arc.
    #[test]
    fn debug_mover34_block() {
        let mut c = Collision::new(None);
        let quad = |c: &mut Collision, a: Vec3, b: Vec3, d: Vec3, e: Vec3| {
            c.add_triangle([a, b, d], None);
            c.add_triangle([a, d, e], None);
        };
        quad(&mut c, Vec3::new(-3000.0, 4000.0, 720.0), Vec3::new(-2500.0, 4000.0, 720.0), Vec3::new(-2500.0, 4940.0, 720.0), Vec3::new(-3000.0, 4940.0, 720.0));
        // Leaning face: the top (z 816) is 2.3 closer to the pawn than the bottom.
        quad(&mut c, Vec3::new(-2816.0, 4940.0, 720.0), Vec3::new(-2688.0, 4940.0, 720.0), Vec3::new(-2688.0, 4942.3, 816.0), Vec3::new(-2816.0, 4942.3, 816.0));
        quad(&mut c, Vec3::new(-2816.0, 4812.0, 816.0), Vec3::new(-2688.0, 4812.0, 816.0), Vec3::new(-2688.0, 4942.3, 816.0), Vec3::new(-2816.0, 4942.3, 816.0));
        let ext = Vec3::new(15.0, 15.0, 38.0);
        for z in [758.0f32, 780.0, 800.0, 822.0] {
            let l = Ledge { loc: Vec3::new(-2756.0, 4955.5, z), up: Vec3::Z, normal: Vec3::new(0.0, 1.0, 0.0), radius: 15.0, reach: 96.5, min_rise: 0.0, ext, ignore: ObjId(0) };
            let top = l.loc + l.up * l.reach;
            let start = top + Vec3::new(0.0, -30.0, 0.0);
            let end = l.loc + Vec3::new(0.0, -30.0, 0.0);
            let down = c.trace_static(start, end, ext);
            eprintln!("z {z}: down {:?} -> {:?}", down.map(|d| (d.location, d.time)), find_ledge(&c, &l));
            let slim = Vec3::new(9.0, 9.0, 38.0);
            let lift = Vec3::new(l.loc.x, l.loc.y, 856.03125);
            eprintln!("  up {:?} across {:?} up-full {:?}", c.trace_static(l.loc, lift, slim).map(|h| (h.location, h.time, h.normal)), c.trace_static(lift, Vec3::new(lift.x, 4925.5, 856.03125), slim).map(|h| (h.location, h.time, h.normal)), c.trace_static(l.loc, lift, ext).map(|h| h.time));
        }
    }

    #[test]
    fn a_wall_too_high_is_left_alone() {
        let c = ledge_world(300.0);
        assert!(pawn_at(70.0, &c, 96.5).is_none());
    }

    #[test]
    fn a_low_step_is_not_a_mount() {
        // 10 units is below the least rise (a step, walked up without the script).
        let c = ledge_world(10.0);
        assert!(pawn_at(70.0, &c, 96.5).is_none());
    }
}

/// Whether a box at `at` overlaps the level (its geometry and movers; actors do not count).
fn overlaps_world(col: &crate::collision::Collision, at: Vec3, extent: Vec3) -> bool {
    // A box that has sunk into a surface is behind its face, where a sweep finds nothing: the triangles are tested for
    // overlap as well.
    col.trace_static(at + Vec3::Z * 0.1, at, extent).is_some_and(|h| h.time <= 0.0) || (crate::envc::var_os("HP3_NO_OVERLAP_TRI").is_none() && col.box_overlaps_triangles(at, extent, None))
}

/// A place close to `want` where a box of `extent` fits in the level: `want` itself, or a nudge out of the floor,
/// the ceiling or a wall it sinks into; None when no such place is found. A box with no size always fits.
///
/// Behaviour: reference `ULevel::FindSpot`/`CheckSlice` (a thin slice at the box's middle decides whether the
/// trouble is above or below, then the box is moved clear of it), re-expressed.
pub fn find_spot(col: &crate::collision::Collision, extent: Vec3, want: Vec3) -> Option<Vec3> {
    // A spot inside a solid volume is first pushed out through its nearest face, as the original's point check reports it.
    let want = col.push_out_of_hulls(want, extent);
    if !overlaps_world(col, want, extent) {
        return Some(want);
    }
    if extent == Vec3::ZERO {
        return None;
    }
    let slice = Vec3::new(extent.x, extent.y, 1.0);
    let down = Vec3::new(0.0, 0.0, extent.z);
    // A thin slice at the middle that does not touch anything means the floor or the ceiling is in the way.
    if !overlaps_world(col, want, slice) {
        let floor = col.trace_static(want, want - down * 2.0, slice);
        // Away from a wall by the box's radius, along the floor's slope.
        let clear_of_wall = |at: Vec3, normal: Vec3| {
            let at = at + Vec3::new(normal.x, normal.y, 0.0) * extent.x;
            (!overlaps_world(col, at, extent)).then_some(at)
        };
        return match floor {
            // Nothing below: the ceiling was the problem, so move down by the half height and settle on it.
            None => {
                let at = want - down;
                if !overlaps_world(col, at, extent) {
                    Some(col.trace_static(at, at + down, extent).map_or(at, |h| h.location))
                } else {
                    None
                }
            }
            // The floor is far: same, by how far it is.
            Some(h) if h.time > 0.5 => {
                let at = want - down * (2.0 * h.time - 1.0) + Vec3::Z;
                if !overlaps_world(col, at, extent) {
                    Some(col.trace_static(at, at + down, extent).map_or(at, |h| h.location))
                } else {
                    clear_of_wall(at, h.normal)
                }
            }
            // The floor is close: lift out of it.
            Some(h) => {
                let at = want + down * (2.0 * (0.5 - h.time)) + Vec3::Z;
                if !overlaps_world(col, at, extent) { Some(at) } else { clear_of_wall(at, h.normal) }
            }
        };
    }
    // Half-size slices at the corners: those that fit say which way is free.
    let half = Vec3::new(0.5 * extent.x, 0.5 * extent.y, 1.0);
    let mut moved = want;
    let mut fits = 0;
    for i in [-1.0f32, 1.0] {
        for j in [-1.0f32, 1.0] {
            let offset = Vec3::new(0.55 * extent.x * i, 0.55 * extent.y * j, 0.0);
            if fits < 2 && !overlaps_world(col, want + offset, half) {
                fits += 1;
                moved += offset;
            }
        }
    }
    if fits == 0 {
        return None;
    }
    if fits == 1 {
        moved = moved * 2.0 - want;
    }
    if col.trace_static(want, moved, Vec3::ZERO).is_some() || overlaps_world(col, moved, extent) {
        return None;
    }
    // Back toward where it was wanted, as far as it stays clear.
    let back = want + (want - moved) * 0.2;
    Some(col.trace_static(moved, back, extent).map_or(moved, |h| h.location))
}

/// Puts an actor somewhere else without sweeping (`SetLocation`, `FarMoveActor`): an actor that collides with the
/// world is first fitted to a nearby spot (and refused when there is none); then it lets go of its base, carries
/// what is attached to it and settles. With `test` only the answer is given. Returns whether it was placed.
pub fn far_move(vm: &mut Vm<'_>, a: ObjId, to: Vec3, test: bool, attached_move: bool) -> bool {
    if get(vm, a, "bStatic").as_bool() {
        return false;
    }
    let from = vec(&get(vm, a, "Location"));
    if test && from == to {
        return true;
    }
    let mut at = to;
    if crate::envc::var_os("HP3_FAR_DEBUG").is_some() {
        let ext = extent(vm, a);
        eprintln!("far_move {} to {to} collideworld {} ext {ext} overlaps {}", hp3_script::natives::object_path(vm, a), get(vm, a, "bCollideWorld").as_bool(), overlaps_world(&world(vm).collision, to, ext));
    }
    if get(vm, a, "bCollideWorld").as_bool() {
        let ext = extent(vm, a);
        let spot = find_spot(&world(vm).collision, ext, to);
        if crate::envc::var_os("HP3_FAR_DEBUG").is_some() {
            let pushed = world(vm).collision.push_out_of_hulls(to, ext);
            let c = &world(vm).collision;
            let (sweep, tris) = (c.trace_static(to + Vec3::Z * 0.1, to, ext).map(|h| (h.time, h.normal, h.actor)), c.box_overlaps_triangles(to, ext, None));
            let owners: Vec<String> = c.debug_overlaps(to, ext).into_iter().map(|o| o.map_or("terrain".to_string(), |o| hp3_script::natives::object_path(vm, o))).collect();
            eprintln!("far_move {}: pushed out of volumes to {pushed}, spot {spot:?}; at the spot: sweep {sweep:?} triangles {tris} {owners:?}", hp3_script::natives::object_path(vm, a));
        }
        match spot {
            Some(spot) => at = spot,
            None => return false,
        }
    }
    if test {
        return true;
    }
    set(vm, a, "bJustTeleported", Value::Bool(true));
    if !attached_move {
        crate::natives::set_base(vm, a, None, true);
    }
    // What rides on it (not on a bone) goes along, unchecked.
    let carried: Vec<ObjId> = riders_of(vm, a).into_iter().filter(|&o| get(vm, o, "AttachmentBone").as_name() == hp3_script::Name::NONE).collect();
    for o in carried {
        let there = vec(&get(vm, o, "Location")) + (at - from);
        set(vm, o, "Location", value(there));
        crate::touch::settle(vm, o);
    }
    set(vm, a, "Location", value(at));
    crate::touch::settle(vm, a);
    true
}

/// A snapshot of the player's pawn and what is around it, for a bug report (the app's F9 key writes it to the log): the
/// place, turn, speed, physics, controller state, floor, volume, collision size, what it touches and every actor within 400
/// units with its class, place and collision, then the floor sweeps.
/// What the camera looks at, for a screenshot: the first actor whose baked shape the view ray crosses, then the actors with a
/// static mesh whose origin lies close to the ray (an object without collision has no baked shape to hit).
pub fn aim(vm: &mut Vm<'_>, origin: Vec3, rot: [i32; 3]) -> Vec<String> {
    let k = std::f32::consts::TAU / 65536.0;
    let (pitch, yaw) = (rot[0] as f32 * k, rot[1] as f32 * k);
    let dir = Vec3::new(pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), pitch.sin());
    let mut out = Vec::new();
    let describe = |vm: &mut Vm<'_>, o: ObjId| -> String {
        let class = vm.db.objects.get(o).class.map(|c| vm.db.names.get(vm.db.get(c).name).to_string()).unwrap_or_default();
        let mesh = get(vm, o, "StaticMesh").as_obj().map(|m| hp3_script::natives::object_path(vm, m)).unwrap_or_default();
        let p = vec(&get(vm, o, "Location"));
        let name = hp3_script::natives::object_path(vm, o);
        let name = name.rsplit_once('.').map_or(name.clone(), |(_, n)| n.to_string());
        {
            let rot = rotator_ints(&get(vm, o, "Rotation"));
            let base = get(vm, o, "Base").as_obj().map(|b| hp3_script::natives::object_path(vm, b)).unwrap_or_default();
            let (phys, scale) = (get(vm, o, "Physics").as_byte(), get(vm, o, "DrawScale").as_float());
            let (tag, group) = (get(vm, o, "Tag"), get(vm, o, "Group"));
            format!("{name} {} Z={:.0} X={:.0} Y={:.0} | ROT {rot:?} SCALE {scale} PHYS {phys} BASE {base} TAG {tag:?} GROUP {group:?} MESH {mesh}", mesh.rsplit_once('.').map_or(mesh.as_str(), |(_, m)| m), p.z, p.x, p.y)
        }
    };
    if let Some((o, d)) = world(vm).collision.pick_triangle(origin, dir, 20000.0) {
        out.push(format!("AIM {} DIST {d:.0}", describe(vm, o)));
    }
    let mut near: Vec<(f32, ObjId)> = Vec::new();
    for o in world(vm).actors.clone() {
        if vm.db.objects.get(o).deleted || get(vm, o, "StaticMesh").as_obj().is_none() {
            continue;
        }
        let rel = vec(&get(vm, o, "Location")) - origin;
        let t = rel.dot(dir);
        if t > 0.0 && t < 4000.0 {
            let off = (rel - dir * t).length();
            if off < 260.0 {
                near.push((off, o));
            }
        }
    }
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (off, o) in near.into_iter().take(8) {
        out.push(format!("RAY+{off:.0} {}", describe(vm, o)));
    }
    out
}

/// The short lines stamped on a screenshot (F4): where the player is, so a report can say exactly what was on screen.
pub fn caption(vm: &mut Vm<'_>, cam: Vec3, cam_rot: [i32; 3]) -> Vec<String> {
    let map = world(vm).level_info.map(|l| hp3_script::natives::object_path(vm, l)).unwrap_or_default();
    let frame = world(vm).frame;
    let Some(a) = world(vm).player.and_then(|pc| get(vm, pc, "Pawn").as_obj()) else {
        let mut lines = vec![format!("MAP {map}  FRAME {frame}"), "NO PAWN".into()];
        lines.extend(aim(vm, cam, cam_rot));
        return lines;
    };
    let at = vec(&get(vm, a, "Location"));
    let (x, y, z) = (at.x, at.y, at.z);
    let rot = rotator_ints(&get(vm, a, "Rotation"));
    let phys = get(vm, a, "Physics").as_byte();
    let who = hp3_script::natives::object_path(vm, a);
    let mut lines = vec![
        format!("MAP {map}  FRAME {frame}"),
        format!("POS X={x:.0} Y={y:.0} Z={z:.0}"),
        format!("ROT PITCH={} YAW={} ROLL={}  PHYS {phys}", rot[0], rot[1], rot[2]),
        format!("PAWN {}", who.rsplit_once('.').map_or(who.as_str(), |(_, n)| n)),
    ];
    lines.extend(aim(vm, cam, cam_rot));
    lines
}

pub fn mark(vm: &mut Vm<'_>) -> String {
    let Some(pc) = world(vm).player else { return "mark: no player".into() };
    let Some(a) = get(vm, pc, "Pawn").as_obj() else { return "mark: no pawn".into() };
    let state = vm.state_of(pc).map(|s| vm.db.names.get(vm.db.get(s).name).to_string()).unwrap_or_default();
    let li = world(vm).level_info;
    let map = li.map(|l| hp3_script::natives::object_path(vm, l)).unwrap_or_default();
    let at = vec(&get(vm, a, "Location"));
    let phys = get(vm, a, "Physics").as_byte();
    let vel = vec(&get(vm, a, "Velocity"));
    let floor = vec(&get(vm, a, "Floor"));
    let rot = rotator_ints(&get(vm, a, "Rotation"));
    let (r, h) = (get(vm, a, "CollisionRadius").as_float(), get(vm, a, "CollisionHeight").as_float());
    let volume = get(vm, a, "PhysicsVolume").as_obj().map(|v| hp3_script::natives::object_path(vm, v)).unwrap_or_default();
    let base = get(vm, a, "Base").as_obj().map(|v| hp3_script::natives::object_path(vm, v)).unwrap_or_default();
    let crouched = get(vm, a, "bIsCrouched").as_bool();
    let (frame, who) = (world(vm).frame, hp3_script::natives::object_path(vm, a));
    let mut out = format!(
        "MARK map {map} frame {}\n  pawn {} at {at} rot {rot:?} vel {vel} phys {phys} controller state {state} floor {floor} base {base} volume {volume} size {r}x{h} crouched {crouched}\n",
        frame, who
    );
    if let Value::Array(touching) = get(vm, a, "Touching") {
        let names: Vec<String> = touching.iter().filter_map(Value::as_obj).map(|o| hp3_script::natives::object_path(vm, o)).collect();
        out += &format!("  touching {names:?}\n");
    }
    for o in world(vm).actors.clone() {
        if o == a || vm.db.objects.get(o).deleted {
            continue;
        }
        let p = vec(&get(vm, o, "Location"));
        if (p - at).length() > 400.0 {
            continue;
        }
        let class = vm.db.objects.get(o).class.map(|c| vm.db.names.get(vm.db.get(c).name).to_string()).unwrap_or_default();
        let (cr, ch) = (get(vm, o, "CollisionRadius").as_float(), get(vm, o, "CollisionHeight").as_float());
        let (ca, cb) = (get(vm, o, "bCollideActors").as_bool(), get(vm, o, "bBlockActors").as_bool());
        out += &format!("  near {} [{class}] at {p} dist {:.0} coll {cr}x{ch} collideActors {ca} block {cb}\n", hp3_script::natives::object_path(vm, o), (p - at).length());
    }
    // Carpe spells and their lines wherever they are (a line left behind after a cast).
    for o in world(vm).actors.clone() {
        if vm.db.objects.get(o).deleted {
            continue;
        }
        let class = vm.db.objects.get(o).class.map(|c| vm.db.names.get(vm.db.get(c).name).to_string()).unwrap_or_default();
        if !class.contains("Retractum") && !class.contains("CarpeFly") {
            continue;
        }
        let state = vm.state_of(o).map(|s| vm.db.names.get(vm.db.get(s).name).to_string()).unwrap_or_default();
        let (p, hidden) = (vec(&get(vm, o, "Location")), get(vm, o, "bHidden").as_bool());
        let name = |vm: &mut Vm<'_>, v: Value| v.as_obj().map(|x| hp3_script::natives::object_path(vm, x)).unwrap_or_else(|| "-".into());
        let (target, caster, owner) = (get(vm, o, "TargetActor"), get(vm, o, "spellCaster"), get(vm, o, "Owner"));
        let (target, caster, owner) = (name(vm, target), name(vm, caster), name(vm, owner));
        let (soft, tohit) = (get(vm, o, "fTimeSoFar").as_float(), get(vm, o, "fTimeToHit").as_float());
        out += &format!("  carpe {} [{class}] state {state} at {p} hidden {hidden} target {target} caster {caster} owner {owner} timeSoFar {soft} toHit {tohit}\n", hp3_script::natives::object_path(vm, o));
    }
    out += &format!("  floor sweeps: {}\n", debug_floor(vm, a));
    out
}

thread_local! {
    /// For each mover, the actors it was placed overlapping and has not yet cleared.
    static SLIDE_THROUGH: std::cell::RefCell<hp3_script::fx::FxHashMap<ObjId, Vec<ObjId>>> = Default::default();
}

/// The actors a mover slides through instead of pushing or crushing: those that overlapped it deeply when it began to move
/// (a gargoyle in the niche behind a portrait), until the mover has cleared them. An actor that only touches the mover
/// (standing on it) never counts.
fn slide_through(vm: &mut Vm<'_>, a: ObjId) -> Vec<ObjId> {
    if crate::envc::var_os("HP3_NO_SLIDE_THROUGH").is_some() {
        return Vec::new();
    }
    let mut kept = SLIDE_THROUGH.with(|s| s.borrow().get(&a).cloned().unwrap_or_default());
    let touching = encroached_by(vm, a);
    kept.retain(|o| touching.contains(o));
    if kept.is_empty() {
        kept = encroached_by_margin(vm, a, 6.0);
    }
    SLIDE_THROUGH.with(|s| s.borrow_mut().insert(a, kept.clone()));
    kept
}
