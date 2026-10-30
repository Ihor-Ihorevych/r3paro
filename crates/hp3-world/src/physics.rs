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
        boxes.push(ActorBox { actor: a, center: crate::read_vec(vm, a, s.location), extent: Vec3::new(r, r, h), blocks_actors, blocks_players });
    }
    world(vm).actors = actors;
    world(vm).collision.actors = boxes;
}

/// Slots of the Actor properties the per-tick loops read.
#[derive(Clone, Copy)]
struct Slots {
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
    let ext = extent(vm, a);
    let player = is_player_pawn(vm, a);
    let hit = world(vm).collision.trace_as(player, loc, loc + delta, ext, Some(a));
    let time = hit.map(|h| h.time).unwrap_or(1.0);
    set(vm, a, "Location", value(loc + delta * time));
    hit
}

/// Moves along `delta`, sliding along what it hits (at most twice).
fn move_and_slide(vm: &mut Vm<'_>, a: ObjId, delta: Vec3) -> Option<Hit> {
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
fn is_player_pawn(vm: &mut Vm<'_>, a: ObjId) -> bool {
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

fn walk(vm: &mut Vm<'_>, a: ObjId, dt: f32) {
    let accel = vec(&get(vm, a, "Acceleration"));
    let mut vel = vec(&get(vm, a, "Velocity"));
    // A walker at rest stays put without probing its floor (a floor that moves away is not noticed until
    // the walker moves again).
    if accel.length_squared() <= 1e-4 && vel.length_squared() <= 1e-4 {
        return;
    }
    vel.z = 0.0;
    let max_speed = get(vm, a, "GroundSpeed").as_float().max(1.0);
    let friction = 8.0;
    if accel.length_squared() > 1e-4 {
        let dir = accel.normalize();
        vel -= (vel - dir * vel.length()) * (friction * dt).min(1.0);
        vel += accel * dt;
        if vel.length() > max_speed {
            vel = vel.normalize() * max_speed;
        }
    } else {
        let speed = vel.length();
        let drop = (speed * friction * dt).min(speed);
        vel = if speed > 0.0 { vel * ((speed - drop) / speed) } else { Vec3::ZERO };
    }
    set(vm, a, "Velocity", value(vel));
    let delta = vel * dt;
    if let Some(hit) = move_actor(vm, a, delta)
        && hit.normal.z < WALKABLE_Z
    {
        // A wall: try stepping up over it, else slide along it.
        if std::env::var_os("HP3_WALK_DEBUG").is_some() {
            let name = hit.actor.map(|h| vm.db.names.get(vm.db.objects.get(h).name).to_string());
            eprintln!("walk: hit {name:?} at {} normal {} time {}", hit.location, hit.normal, hit.time);
        }
        let remaining = delta * (1.0 - hit.time);
        let before = vec(&get(vm, a, "Location"));
        let up = move_actor(vm, a, Vec3::Z * MAX_STEP_HEIGHT);
        let climbed = vec(&get(vm, a, "Location")).z - before.z;
        let blocked = move_actor(vm, a, remaining).is_some_and(|h| h.normal.z < WALKABLE_Z);
        if up.is_some() || blocked || climbed <= 0.0 {
            set(vm, a, "Location", value(before));
            let slide = remaining - hit.normal * remaining.dot(hit.normal);
            move_and_slide(vm, a, slide);
            fire(vm, a, "HitWall", &mut [value(hit.normal), Value::Object(hit.actor)]);
        } else {
            move_actor(vm, a, -Vec3::Z * (climbed + 2.0));
        }
    }
    match floor_below(vm, a, MAX_STEP_HEIGHT + 2.0) {
        Some(floor) => {
            let loc = vec(&get(vm, a, "Location"));
            let drop = (MAX_STEP_HEIGHT + 2.0) * floor.time;
            if drop > 0.5 {
                set(vm, a, "Location", value(loc - Vec3::Z * (drop - 0.5)));
            }
        }
        None => {
            set(vm, a, "Physics", Value::Byte(PHYS_FALLING));
            fire(vm, a, "Falling", &mut []);
        }
    }
}

fn fall(vm: &mut Vm<'_>, a: ObjId, dt: f32) {
    let accel = vec(&get(vm, a, "Acceleration"));
    let mut vel = vec(&get(vm, a, "Velocity"));
    let air = get(vm, a, "AirControl").as_float();
    vel += Vec3::new(accel.x, accel.y, 0.0) * air * dt;
    vel.z += gravity(vm, a) * dt;
    let terminal = 4000.0;
    vel.z = vel.z.max(-terminal);
    set(vm, a, "Velocity", value(vel));
    if let Some(hit) = move_and_slide(vm, a, vel * dt) {
        if hit.normal.z >= WALKABLE_Z {
            let mut v = vec(&get(vm, a, "Velocity"));
            fire(vm, a, "Landed", &mut [value(hit.normal)]);
            if get(vm, a, "Physics").as_byte() == PHYS_FALLING {
                set(vm, a, "Physics", Value::Byte(PHYS_WALKING));
            }
            v.z = 0.0;
            set(vm, a, "Velocity", value(v));
        } else {
            fire(vm, a, "HitWall", &mut [value(hit.normal), Value::Object(hit.actor)]);
        }
    }
}

fn fly(vm: &mut Vm<'_>, a: ObjId, dt: f32) {
    let accel = vec(&get(vm, a, "Acceleration"));
    let mut vel = vec(&get(vm, a, "Velocity"));
    let max = get(vm, a, "AirSpeed").as_float().max(1.0);
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
        match hit.actor {
            Some(other) => {
                fire(vm, a, "Touch", &mut [Value::Object(Some(other))]);
            }
            None => {
                fire(vm, a, "HitWall", &mut [value(hit.normal), Value::Object(None)]);
            }
        }
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

/// Collision cylinders overlap: horizontal distance under the radii's sum and vertical under the heights'.
fn cylinders_overlap(a: (Vec3, f32, f32), b: (Vec3, f32, f32)) -> bool {
    let d = a.0 - b.0;
    (d.x * d.x + d.y * d.y).sqrt() < a.1 + b.1 && d.z.abs() < a.2 + b.2
}

/// Updates who touches whom for non-static colliding actors, firing `Touch` and `UnTouch` on both sides and
/// keeping each actor's `Touching` array current.
pub fn update_touching(vm: &mut Vm<'_>) {
    let actors = std::mem::take(&mut world(vm).actors);
    let s = Slots::get(vm);
    let mut colliders = Vec::new();
    for &a in &actors {
        if vm.db.objects.get(a).deleted || !crate::read(vm, a, s.collide).as_bool() {
            continue;
        }
        let (r, h) = (crate::read(vm, a, s.radius).as_float(), crate::read(vm, a, s.height).as_float());
        if r <= 0.0 && h <= 0.0 {
            continue;
        }
        let moving = crate::read(vm, a, s.physics).as_byte() != PHYS_NONE && !crate::read(vm, a, s.is_static).as_bool();
        colliders.push((a, crate::read_vec(vm, a, s.location), r, h, moving));
    }
    world(vm).actors = actors;
    // Colliders bucketed by position; each moving one is tested against its neighbourhood only.
    const CELL: f32 = 512.0;
    let cell = |p: Vec3| ((p.x / CELL).floor() as i32, (p.y / CELL).floor() as i32);
    let mut grid: hp3_script::fx::FxHashMap<(i32, i32), Vec<usize>> = Default::default();
    let mut max_radius: f32 = 0.0;
    for (i, c) in colliders.iter().enumerate() {
        grid.entry(cell(c.1)).or_default().push(i);
        max_radius = max_radius.max(c.2);
    }
    let mut now: hp3_script::fx::FxHashSet<(ObjId, ObjId)> = Default::default();
    for (i, m) in colliders.iter().enumerate() {
        if !m.4 {
            continue;
        }
        let reach = m.2 + max_radius;
        let (x0, y0) = cell(m.1 - Vec3::splat(reach));
        let (x1, y1) = cell(m.1 + Vec3::splat(reach));
        for cx in x0..=x1 {
            for cy in y0..=y1 {
                let Some(list) = grid.get(&(cx, cy)) else { continue };
                for &j in list {
                    if i == j {
                        continue;
                    }
                    let o = &colliders[j];
                    if cylinders_overlap((m.1, m.2, m.3), (o.1, o.2, o.3)) {
                        let pair = if m.0 < o.0 { (m.0, o.0) } else { (o.0, m.0) };
                        now.insert(pair);
                    }
                }
            }
        }
    }
    let before = std::mem::take(&mut world(vm).touching);
    for &(a, b) in now.difference(&before) {
        add_touching(vm, a, b);
        add_touching(vm, b, a);
        fire(vm, a, "Touch", &mut [Value::Object(Some(b))]);
        fire(vm, b, "Touch", &mut [Value::Object(Some(a))]);
    }
    for &(a, b) in before.difference(&now) {
        remove_touching(vm, a, b);
        remove_touching(vm, b, a);
        fire(vm, a, "UnTouch", &mut [Value::Object(Some(b))]);
        fire(vm, b, "UnTouch", &mut [Value::Object(Some(a))]);
    }
    world(vm).touching = now;
}

fn add_touching(vm: &mut Vm<'_>, a: ObjId, other: ObjId) {
    let mut list = match get(vm, a, "Touching") {
        Value::Array(v) => v,
        _ => Vec::new(),
    };
    if !list.iter().any(|v| v.as_obj() == Some(other)) {
        list.push(Value::Object(Some(other)));
        set(vm, a, "Touching", Value::Array(list));
    }
}

fn remove_touching(vm: &mut Vm<'_>, a: ObjId, other: ObjId) {
    if let Value::Array(mut list) = get(vm, a, "Touching") {
        list.retain(|v| v.as_obj() != Some(other));
        set(vm, a, "Touching", Value::Array(list));
    }
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
fn moving_brush(vm: &mut Vm<'_>, a: ObjId, dt: f32) {
    if !get(vm, a, "bInterpolating").as_bool() {
        return;
    }
    let rate = get(vm, a, "PhysRate").as_float();
    let alpha = (get(vm, a, "PhysAlpha").as_float() + rate * dt).min(1.0);
    set(vm, a, "PhysAlpha", Value::Float(alpha));
    let f = if get(vm, a, "MoverGlideType").as_byte() == 1 { alpha * alpha * (3.0 - 2.0 * alpha) } else { alpha };
    let (prev, key) = (get(vm, a, "PrevKeyNum").as_byte() as usize, get(vm, a, "KeyNum").as_byte() as usize);
    let base = vec(&get(vm, a, "BasePos"));
    let (p0, p1) = (indexed_vec(vm, a, "KeyPos", prev), indexed_vec(vm, a, "KeyPos", key));
    let old = vec(&get(vm, a, "Location"));
    let pos = base + p0.lerp(p1, f);
    let br: Vec<i32> = get(vm, a, "BaseRot").fields().iter().map(Value::as_int).collect();
    let (r0, r1) = (indexed_rot(vm, a, "KeyRot", prev), indexed_rot(vm, a, "KeyRot", key));
    let rot: Vec<Value> = (0..3)
        .map(|i| Value::Int(br.get(i).copied().unwrap_or(0) + r0[i] + ((r1[i] - r0[i]) as f32 * f).round() as i32))
        .collect();
    set(vm, a, "Location", value(pos));
    set(vm, a, "Rotation", Value::Struct(rot.into()));
    // Actors standing on the mover ride along.
    let delta = pos - old;
    if delta != Vec3::ZERO {
        let riders: Vec<ObjId> = world(vm).actors.iter().copied().filter(|&r| r != a).collect();
        for r in riders {
            if get(vm, r, "Base").as_obj() == Some(a) {
                let p = vec(&get(vm, r, "Location"));
                set(vm, r, "Location", value(p + delta));
            }
        }
    }
    let t = crate::mover_transform(vm, a);
    if let Some(m) = world(vm).collision.movers.iter_mut().find(|m| m.actor == a) {
        m.place(t);
    }
    if alpha >= 1.0 {
        set(vm, a, "bInterpolating", Value::Bool(false));
        fire(vm, a, "InterpolateEnd", &mut [Value::Object(None)]);
    }
}

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
