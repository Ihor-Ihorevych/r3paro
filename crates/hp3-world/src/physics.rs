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

/// Rebuilds the boxes of actors that block movement (static meshes collide by their triangles instead).
fn refresh_actor_boxes(vm: &mut Vm<'_>) {
    let actors = std::mem::take(&mut world(vm).actors);
    let s = Slots::get(vm);
    let mut boxes = Vec::new();
    for &a in &actors {
        let blocks_actors = crate::read(vm, a, s.block).as_bool();
        let blocks_players = crate::read(vm, a, s.block_players).as_bool();
        if !crate::read(vm, a, s.collide).as_bool() || !(blocks_actors || blocks_players) {
            continue;
        }
        if crate::read(vm, a, s.static_mesh).as_obj().is_some() {
            continue;
        }
        let (r, h) = (crate::read(vm, a, s.radius).as_float(), crate::read(vm, a, s.height).as_float());
        if r <= 0.0 || h <= 0.0 {
            continue;
        }
        boxes.push(ActorBox { actor: a, center: crate::read_vec(vm, a, s.location), extent: Vec3::new(r, r, h), blocks_actors, blocks_players, is_pawn: is_pawn(vm, a) });
    }
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
        }
    }
}

/// Moves `a` by `delta`, stopping at the first hit. Returns the hit.
pub fn move_actor(vm: &mut Vm<'_>, a: ObjId, delta: Vec3) -> Option<Hit> {
    if delta.length_squared() < 1e-8 {
        return None;
    }
    let loc = vec(&get(vm, a, "Location"));
    // A ghost (the debug mode) has no collision at all.
    if is_player_pawn(vm, a) && !get(vm, a, "bCollideWorld").as_bool() && !get(vm, a, "bCollideActors").as_bool() {
        set(vm, a, "Location", value(loc + delta));
        return None;
    }
    let ext = extent(vm, a);
    let player = is_player_pawn(vm, a);
    // The level stops an actor only with `bCollideWorld`, and other actors only with `bCollideActors` and
    // `bBlockActors` (a spell flies through the creatures it touches, and a pawn in the middle of a climb has all
    // of them off so it can pass through what it climbs).
    let how = crate::collision::Mover {
        player,
        blocks_actors: get(vm, a, "bBlockActors").as_bool() && get(vm, a, "bCollideActors").as_bool(),
        collide_world: get(vm, a, "bCollideWorld").as_bool(),
    };
    let hit = world(vm).collision.trace_for(how, loc, loc + delta, ext, Some(a));
    let time = hit.map(|h| h.time).unwrap_or(1.0);
    set(vm, a, "Location", value(loc + delta * time));
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
        && fire(vm, c, "NotifyBump", &mut [Value::Object(Some(other))]).as_bool()
    {
        return;
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
                first.get_or_insert(h);
                let left = remaining * (1.0 - h.time);
                remaining = left - h.normal * left.dot(h.normal);
                if remaining.length_squared() < 0.01 {
                    break;
                }
            }
        }
    }
    first
}

fn floor_below(vm: &mut Vm<'_>, a: ObjId, depth: f32) -> Option<Hit> {
    let loc = vec(&get(vm, a, "Location"));
    let ext = extent(vm, a);
    let player = is_player_pawn(vm, a);
    world(vm).collision.trace_as(player, loc, loc - Vec3::Z * depth, ext, Some(a)).filter(|h| h.normal.z >= WALKABLE_Z)
}

/// Whether `a` is the pawn the local player controls (what `bBlockPlayers` actors block).
pub(crate) fn is_player_pawn(vm: &mut Vm<'_>, a: ObjId) -> bool {
    let Some(pc) = world(vm).player else { return false };
    get(vm, pc, "Pawn").as_obj() == Some(a)
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
        v
    };
    let start = vec(&get(vm, a, "Location"));
    let old_base = get(vm, a, "Base").as_obj();
    let can_jump = get(vm, a, "bCanJump").as_bool();
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
            if let Some(hit) = move_actor(vm, a, delta)
                && hit.normal.z < WALKABLE_Z
                && climb_or_slide(vm, a, delta, hit)
            {
                // The script took the pawn up a ledge: the rest of the step is its business.
                return;
            }
        }
        match floor_below(vm, a, MAX_STEP_HEIGHT + 2.0) {
            Some(floor) => {
                let loc = vec(&get(vm, a, "Location"));
                let drop = (MAX_STEP_HEIGHT + 2.0) * floor.time;
                if drop > 0.5 {
                    set(vm, a, "Location", value(loc - Vec3::Z * (drop - 0.5)));
                }
                if get(vm, a, "Base").as_obj() != floor.actor {
                    set(vm, a, "Base", Value::Object(floor.actor));
                }
            }
            None => {
                // Nothing to stand on. The controller gets a chance to stop the pawn once.
                if !asked_may_fall && can_jump {
                    asked_may_fall = true;
                    fire(vm, controller, "MayFall", &mut []);
                }
                if !can_jump || !walks_off {
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

/// A walker met a barrier: a ledge it can mount is handed to the script (true); else it climbs the barrier when the
/// step up is possible, or slides along it (and lets the script know with `HitWall`).
fn climb_or_slide(vm: &mut Vm<'_>, a: ObjId, delta: Vec3, hit: Hit) -> bool {
    if try_mount(vm, a, Vec3::Z, &hit) {
        return true;
    }
    let human = world(vm).player.is_some_and(|pc| get(vm, pc, "Pawn").as_obj() == Some(a));
    let remaining = delta * (1.0 - hit.time);
    let before = vec(&get(vm, a, "Location"));
    let up = move_actor(vm, a, Vec3::Z * MAX_STEP_HEIGHT);
    let climbed = vec(&get(vm, a, "Location")).z - before.z;
    let blocked = move_actor(vm, a, remaining).is_some_and(|h| h.normal.z < WALKABLE_Z);
    if up.is_some() || blocked || climbed <= 0.0 {
        set(vm, a, "Location", value(before));
        let slide = remaining - hit.normal * remaining.dot(hit.normal);
        move_and_slide(vm, a, slide);
        // Hitting another pawn is not hitting a wall; a pawn that is not pushing makes no wall event.
        let hit_pawn = hit.actor.is_some_and(|o| is_pawn(vm, o));
        let pushing = vec(&get(vm, a, "Acceleration")).length_squared() > 1e-6 || human;
        if !hit_pawn && pushing {
            fire(vm, a, "HitWall", &mut [value(hit.normal), Value::Object(hit.actor)]);
        }
    } else {
        move_actor(vm, a, -Vec3::Z * (climbed + 2.0));
    }
    false
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
    fire(vm, a, "Mount", &mut [value(destination - loc), Value::Object(wall)]).as_bool()
}

/// Where a pawn that met a wall at `hit` would climb to, if the wall is a ledge it can mount (see `try_mount`).
pub fn mount_destination(vm: &mut Vm<'_>, a: ObjId, up: Vec3, hit: &Hit) -> Option<Vec3> {
    let max_height = get(vm, a, "MaxMountHeight").as_float();
    let debug = std::env::var_os("HP3_MOUNT_DEBUG").is_some() && is_player_pawn(vm, a);
    if max_height <= 0.0 || hit.normal.z <= -0.1 || hit.normal.z >= 0.17 {
        if debug {
            eprintln!("mount: not a wall (normal {}, max height {max_height})", hit.normal);
        }
        return None;
    }
    if rotator_direction(&get(vm, a, "Rotation")).dot(hit.normal) >= 0.0 {
        if debug {
            eprintln!("mount: wall {} does not face the pawn", hit.normal);
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
    find_ledge(&world(vm).collision, &ledge)
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
fn find_ledge(col: &crate::collision::Collision, l: &Ledge) -> Option<Vec3> {
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
            if trace(l.loc, lift).is_some() || trace(lift, destination).is_some() {
                continue;
            }
            if first.is_none() {
                first = Some(destination);
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

fn fall(vm: &mut Vm<'_>, a: ObjId, dt: f32) {
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
    let push = vec(&get(vm, a, "ConstantAcceleration"));
    let g = Vec3::new(0.0, 0.0, gravity(vm, a));
    let terminal = 4000.0;
    // Slices of at most 0.05 s, so a long frame lands and bounces off walls like a short one.
    let mut left = dt;
    for _ in 0..8 {
        if left <= 0.0 {
            break;
        }
        let tick = if left > 0.05 { 0.05f32.min(left * 0.5) } else { left };
        left -= tick;
        let mut vel = vec(&get(vm, a, "Velocity")) + (accel + g + push) * tick;
        vel.z = vel.z.max(-terminal);
        if bound != 0.0 && vel.truncate().length_squared() > bound * bound {
            let flat = vel.truncate().normalize_or_zero() * bound;
            vel = Vec3::new(flat.x, flat.y, vel.z);
        }
        set(vm, a, "Velocity", value(vel));
        let Some(hit) = move_and_slide(vm, a, vel * tick) else { continue };
        if hit.normal.z >= WALKABLE_Z {
            let mut v = vec(&get(vm, a, "Velocity"));
            fire(vm, a, "Landed", &mut [value(hit.normal)]);
            if get(vm, a, "Physics").as_byte() == PHYS_FALLING {
                set(vm, a, "Physics", Value::Byte(PHYS_WALKING));
            }
            v.z = 0.0;
            set(vm, a, "Velocity", value(v));
            return;
        } else if try_mount(vm, a, Vec3::Z, &hit) {
            return;
        } else {
            fire(vm, a, "HitWall", &mut [value(hit.normal), Value::Object(hit.actor)]);
        }
    }
}

fn fly(vm: &mut Vm<'_>, a: ObjId, dt: f32) {
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

fn rotate(vm: &mut Vm<'_>, a: ObjId, dt: f32) {
    let rate = get(vm, a, "RotationRate");
    let rot = get(vm, a, "Rotation");
    let (r, c) = (rate.fields(), rot.fields());
    if r.len() == 3 && c.len() == 3 {
        let v: Vec<Value> = (0..3).map(|i| Value::Int(c[i].as_int().wrapping_add((r[i].as_int() as f32 * dt) as i32) & 0xFFFF)).collect();
        set(vm, a, "Rotation", Value::Struct(v.into()));
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
fn place_mover_shape(vm: &mut Vm<'_>, a: ObjId) {
    let t = crate::mover_transform(vm, a);
    if let Some(m) = world(vm).collision.movers.iter_mut().find(|m| m.actor == a) {
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
    if shortest && done {
        return k;
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
    if stands_on(vm, other, mover) || stands_on(vm, mover, other) || world(vm).collision.mover(other).is_some() {
        return false;
    }
    let class = vm.db.objects.get(other).class;
    !vm.db.find("StaticMeshActor").is_some_and(|sm| class.is_some_and(|c| vm.is_child_of(c, sm)))
}

/// Actors that overlap the mover where it now is, among those it has to deal with.
fn encroached_by(vm: &mut Vm<'_>, a: ObjId) -> Vec<ObjId> {
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
        if world(vm).collision.mover(a).is_some_and(|m| m.overlaps(at, ext)) {
            found.push(o);
        }
    }
    found
}

/// Whether the mover may take the step to `to`/`rot`: every actor in its way is first pushed along with it, and
/// if one is still in the way the mover's own `EncroachingOn` decides (true: the step is refused). Actors it
/// does end up overlapping are told with `EncroachedBy`.
fn clear_way(vm: &mut Vm<'_>, a: ObjId, from: Vec3, to: Vec3, rot: [i32; 3]) -> bool {
    let old_rot = rotator_ints(&get(vm, a, "Rotation"));
    set(vm, a, "Location", value(to));
    set(vm, a, "Rotation", rotator_value(rot));
    place_mover_shape(vm, a);
    let in_way = encroached_by(vm, a);
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
        if !encroached_by(vm, a).contains(&o) {
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
    for o in encroached_by(vm, a) {
        fire(vm, o, "EncroachedBy", &mut [Value::Object(Some(a))]);
    }
    true
}

/// Carries what rides on the mover along its step: the same shift, plus the swing a rotation gives a rider at
/// some distance from the pivot, and the turn itself for the rider's heading.
fn carry_riders(vm: &mut Vm<'_>, a: ObjId, from: Vec3, delta: Vec3, old_rot: [i32; 3], new_rot: [i32; 3]) {
    use hp3_script::natives::rot_axes;
    let riders: Vec<ObjId> = world(vm).actors.iter().copied().filter(|&r| r != a).collect();
    let (o, n) = (rot_axes(old_rot), rot_axes(new_rot));
    let turn = [0, 1, 2].map(|i| ((new_rot[i] & 0xFFFF) - (old_rot[i] & 0xFFFF) + 32768).rem_euclid(65536) - 32768);
    for r in riders {
        if get(vm, r, "Base").as_obj() != Some(a) || vm.db.objects.get(r).deleted {
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
        move_and_slide(vm, r, shift);
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
    match crate::read(vm, a, s.physics).as_byte() {
        PHYS_MOVING_BRUSH => {
            moving_brush(vm, a, dt);
            return;
        }
        PHYS_WALKING => walk(vm, a, dt),
        PHYS_FALLING => fall(vm, a, dt),
        PHYS_FLYING | PHYS_SWIMMING => fly(vm, a, dt),
        PHYS_ROTATING => rotate(vm, a, dt),
        PHYS_PROJECTILE => projectile(vm, a, dt),
        PHYS_TRAILER => {
            trailer(vm, a);
            return;
        }
        _ => return,
    }
    if is_pawn(vm, a) {
        pawn_rotation(vm, a, dt);
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
fn direction_rotator(v: Vec3) -> Value {
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
fn pawn_rotation(vm: &mut Vm<'_>, a: ObjId, dt: f32) {
    let Some(c) = get(vm, a, "Controller").as_obj() else { return };
    let ints = |v: Value| -> [i32; 3] {
        let f = v.fields();
        [f.first().map_or(0, Value::as_int), f.get(1).map_or(0, Value::as_int), f.get(2).map_or(0, Value::as_int)]
    };
    let rot = ints(get(vm, a, "Rotation"));
    let human = world(vm).player == Some(c);
    let (desired, delta) = if human {
        if !get(vm, c, "bRotateToDesired").as_bool() {
            return;
        }
        let d = ints(get(vm, c, "DesiredRotation"));
        set(vm, a, "DesiredRotation", Value::Struct(vec![Value::Int(d[0]), Value::Int(d[1]), Value::Int(0)].into()));
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
    let want_pitch = if on_floor || get(vm, a, "Physics").as_byte() == PHYS_FALLING { 0 } else { desired[0] };
    let yaw = fixed_turn(rot[1], desired[1], delta);
    let pitch = fixed_turn(rot[0], want_pitch, delta);
    if yaw != rot[1] & 0xFFFF || pitch != rot[0] & 0xFFFF || rot[2] != 0 {
        set(vm, a, "Rotation", Value::Struct(vec![Value::Int(pitch), Value::Int(yaw), Value::Int(0)].into()));
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
    if std::env::var_os("HP3_MOVERS").is_some() && world(vm).frame == 100 {
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
            PHYS_WALKING => walk(vm, a, dt),
            PHYS_FALLING => fall(vm, a, dt),
            PHYS_FLYING | PHYS_SWIMMING => fly(vm, a, dt),
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
    col.trace_static(at + Vec3::Z * 0.1, at, extent).is_some_and(|h| h.time <= 0.0)
}

/// A place close to `want` where a box of `extent` fits in the level: `want` itself, or a nudge out of the floor,
/// the ceiling or a wall it sinks into; None when no such place is found. A box with no size always fits.
///
/// Behaviour: reference `ULevel::FindSpot`/`CheckSlice` (a thin slice at the box's middle decides whether the
/// trouble is above or below, then the box is moved clear of it), re-expressed.
pub fn find_spot(col: &crate::collision::Collision, extent: Vec3, want: Vec3) -> Option<Vec3> {
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
    if get(vm, a, "bCollideWorld").as_bool() {
        let ext = extent(vm, a);
        match find_spot(&world(vm).collision, ext, to) {
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
    let everyone: Vec<ObjId> = world(vm).actors.clone();
    let carried: Vec<ObjId> = everyone
        .into_iter()
        .filter(|&o| o != a && get(vm, o, "Base").as_obj() == Some(a) && get(vm, o, "AttachmentBone").as_name() == hp3_script::Name::NONE)
        .collect();
    for o in carried {
        let there = vec(&get(vm, o, "Location")) + (at - from);
        set(vm, o, "Location", value(there));
        crate::touch::settle(vm, o);
    }
    set(vm, a, "Location", value(at));
    crate::touch::settle(vm, a);
    true
}
