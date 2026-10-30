//! Who touches whom.
//!
//! Behaviour re-expressed from the reference engine: touches are not a per-frame sweep over "moving" actors. An
//! actor's overlaps are re-checked at the moment it moves (`SetLocation`, `Move`, a physics step), is resized,
//! changes its collision flag, or appears; actors that have not changed are only looked at as neighbours of one
//! that has. The first of a pair to be re-checked gets `Touch` first, and the second only gets it if the pair
//! is still touching afterwards. Ending a touch (`UnTouch`) works the same way, and a destroyed actor ends all
//! of its touches at once.
//!
//! A pair touches when both collide with actors, their cylinders overlap, at least one of them blocks
//! non-zero-extent traces (so two triggers never touch each other), and neither stands on the other.

use glam::Vec3;
use hp3_script::fx::FxHashMap as HashMap;
use hp3_script::{ObjId, Value, Vm};

use crate::{actor_slot, fire, get, read, read_vec, set, world};

/// Width of a cell of the spatial index.
const CELL: f32 = 512.0;
/// Actors wider than this are kept in a short list that every query checks, so they do not stretch the search.
const BIG_RADIUS: f32 = 640.0;
/// How deep touch events may nest (a `Touch` handler that moves an actor which touches another...).
const MAX_DEPTH: u32 = 6;

/// What an actor looked like when its touches were last worked out.
#[derive(Clone, Copy, PartialEq)]
struct Shape {
    collide: bool,
    location: Vec3,
    radius: f32,
    height: f32,
}

/// Property slots read for every actor, resolved once.
#[derive(Clone, Copy)]
struct Slots {
    collide: Option<usize>,
    location: Option<usize>,
    radius: Option<usize>,
    height: Option<usize>,
    physics: Option<usize>,
    is_static: Option<usize>,
    base: Option<usize>,
    traceable: Option<usize>,
}

#[derive(Default)]
pub struct Touch {
    slots: Option<Slots>,
    /// The index has every actor of the level (built the first time anything asks).
    primed: bool,
    /// Moving actors have been checked once against everything (the level's first frame).
    first_pass_done: bool,
    shapes: HashMap<ObjId, Shape>,
    cells: HashMap<(i32, i32), Vec<ObjId>>,
    cell_of: HashMap<ObjId, (i32, i32)>,
    big: Vec<ObjId>,
    max_radius: f32,
    /// Everything each actor currently touches.
    partners: HashMap<ObjId, Vec<ObjId>>,
    depth: u32,
}

fn slots(vm: &mut Vm<'_>) -> Slots {
    if let Some(s) = world(vm).touch.slots {
        return s;
    }
    let s = Slots {
        collide: actor_slot(vm, "bCollideActors"),
        location: actor_slot(vm, "Location"),
        radius: actor_slot(vm, "CollisionRadius"),
        height: actor_slot(vm, "CollisionHeight"),
        physics: actor_slot(vm, "Physics"),
        is_static: actor_slot(vm, "bStatic"),
        base: actor_slot(vm, "Base"),
        traceable: actor_slot(vm, "bBlockNonZeroExtentTraces"),
    };
    world(vm).touch.slots = Some(s);
    s
}

fn shape_of(vm: &Vm<'_>, a: ObjId, s: &Slots) -> Shape {
    Shape {
        collide: read(vm, a, s.collide).as_bool(),
        location: read_vec(vm, a, s.location),
        radius: read(vm, a, s.radius).as_float(),
        height: read(vm, a, s.height).as_float(),
    }
}

fn cell(p: Vec3) -> (i32, i32) {
    ((p.x / CELL).floor() as i32, (p.y / CELL).floor() as i32)
}

fn overlaps(a: &Shape, b: &Shape) -> bool {
    let d = a.location - b.location;
    (d.x * d.x + d.y * d.y).sqrt() < a.radius + b.radius && d.z.abs() < a.height + b.height
}

/// Whether two actors overlap. A mover is its geometry rather than its cylinder, and two movers never overlap.
fn overlap_between(col: &crate::collision::Collision, a: (ObjId, &Shape), b: (ObjId, &Shape)) -> bool {
    match (col.mover(a.0), col.mover(b.0)) {
        (Some(_), Some(_)) => false,
        (Some(m), None) => m.overlaps(b.1.location, Vec3::new(b.1.radius, b.1.radius, b.1.height)),
        (None, Some(m)) => m.overlaps(a.1.location, Vec3::new(a.1.radius, a.1.radius, a.1.height)),
        (None, None) => overlaps(a.1, b.1),
    }
}

/// Enters (or moves) an actor in the spatial index.
fn index(t: &mut Touch, a: ObjId, shape: &Shape, always: bool) {
    let wanted = (shape.collide && (shape.radius > 0.0 || shape.height > 0.0)).then(|| cell(shape.location));
    let big = always || shape.radius > BIG_RADIUS;
    if wanted.is_some() && !big {
        t.max_radius = t.max_radius.max(shape.radius);
    }
    if let Some(old) = t.cell_of.get(&a).copied() {
        if wanted == Some(old) && !big {
            return;
        }
        if let Some(list) = t.cells.get_mut(&old) {
            list.retain(|&x| x != a);
        }
        t.cell_of.remove(&a);
    }
    t.big.retain(|&x| x != a);
    match wanted {
        Some(_) if big => t.big.push(a),
        Some(c) => {
            t.cells.entry(c).or_default().push(a);
            t.cell_of.insert(a, c);
            t.max_radius = t.max_radius.max(shape.radius);
        }
        None => {}
    }
}

/// Builds the index from the actors of the level without raising any event.
fn prime(vm: &mut Vm<'_>) {
    let s = slots(vm);
    let actors = world(vm).actors.clone();
    for a in actors {
        if vm.db.objects.get(a).deleted {
            continue;
        }
        let shape = shape_of(vm, a, &s);
        let always = world(vm).collision.mover(a).is_some();
        let t = &mut world(vm).touch;
        index(t, a, &shape, always);
        t.shapes.insert(a, shape);
    }
    world(vm).touch.primed = true;
}

/// Actors whose cylinders might reach `shape`, nearest first.
fn neighbours(t: &Touch, me: ObjId, shape: &Shape, own_reach: f32) -> Vec<ObjId> {
    let reach = shape.radius.max(own_reach) + t.max_radius;
    let (x0, y0) = cell(shape.location - Vec3::splat(reach));
    let (x1, y1) = cell(shape.location + Vec3::splat(reach));
    let mut out: Vec<ObjId> = t.big.iter().copied().filter(|&b| b != me).collect();
    for cx in x0..=x1 {
        for cy in y0..=y1 {
            if let Some(list) = t.cells.get(&(cx, cy)) {
                out.extend(list.iter().copied().filter(|&b| b != me));
            }
        }
    }
    out
}

fn blocks_traces(vm: &Vm<'_>, a: ObjId, s: &Slots) -> bool {
    // A class without the flag counts as having it set (its default).
    s.traceable.is_none() || read(vm, a, s.traceable).as_bool()
}

fn standing_on(vm: &Vm<'_>, a: ObjId, b: ObjId, s: &Slots) -> bool {
    read(vm, a, s.base).as_obj() == Some(b) || read(vm, b, s.base).as_obj() == Some(a)
}

/// Re-checks one actor's overlaps right now, raising `Touch` / `UnTouch` for what changed.
pub fn settle(vm: &mut Vm<'_>, a: ObjId) {
    if vm.ext.is_none() || vm.db.objects.get(a).deleted {
        return;
    }
    if !world(vm).touch.primed {
        prime(vm);
    }
    if world(vm).touch.depth >= MAX_DEPTH {
        return;
    }
    let s = slots(vm);
    let shape = shape_of(vm, a, &s);
    let mut now: Vec<(f32, ObjId)> = Vec::new();
    {
        let own_reach = world(vm).collision.mover(a).map(|m| m.reach());
        let candidates = {
            let t = &mut world(vm).touch;
            index(t, a, &shape, own_reach.is_some());
            t.shapes.insert(a, shape);
            if !shape.collide || (shape.radius <= 0.0 && shape.height <= 0.0) { Vec::new() } else { neighbours(t, a, &shape, own_reach.unwrap_or(0.0)) }
        };
        let me_traces = blocks_traces(vm, a, &s);
        for b in candidates {
            if vm.db.objects.get(b).deleted {
                continue;
            }
            let other = shape_of(vm, b, &s);
            if !other.collide || !overlap_between(&world(vm).collision, (a, &shape), (b, &other)) {
                continue;
            }
            if !me_traces && !blocks_traces(vm, b, &s) {
                continue;
            }
            if standing_on(vm, a, b, &s) {
                continue;
            }
            now.push(((shape.location - other.location).length_squared(), b));
        }
    }
    now.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.0.cmp(&y.1.0)));
    let now: Vec<ObjId> = now.into_iter().map(|(_, b)| b).collect();
    let before = world(vm).touch.partners.get(&a).cloned().unwrap_or_default();
    world(vm).touch.depth += 1;
    for &b in before.iter().filter(|b| !now.contains(b)) {
        end(vm, a, b);
    }
    for &b in now.iter().filter(|b| !before.contains(b)) {
        begin(vm, a, b);
    }
    world(vm).touch.depth -= 1;
}

fn begin(vm: &mut Vm<'_>, a: ObjId, b: ObjId) {
    if vm.db.objects.get(a).deleted || vm.db.objects.get(b).deleted {
        return;
    }
    link(vm, a, b);
    fire(vm, a, "Touch", &mut [Value::Object(Some(b))]);
    // The first handler may have destroyed one of them or ended the touch.
    let still = world(vm).touch.partners.get(&a).is_some_and(|p| p.contains(&b));
    if still && !vm.db.objects.get(b).deleted {
        fire(vm, b, "Touch", &mut [Value::Object(Some(a))]);
    }
}

fn end(vm: &mut Vm<'_>, a: ObjId, b: ObjId) {
    if !unlink(vm, a, b) {
        return;
    }
    if !vm.db.objects.get(a).deleted {
        fire(vm, a, "UnTouch", &mut [Value::Object(Some(b))]);
    }
    if !vm.db.objects.get(b).deleted {
        fire(vm, b, "UnTouch", &mut [Value::Object(Some(a))]);
    }
}

fn link(vm: &mut Vm<'_>, a: ObjId, b: ObjId) {
    for (x, y) in [(a, b), (b, a)] {
        let list = world(vm).touch.partners.entry(x).or_default();
        if !list.contains(&y) {
            list.push(y);
        }
        let mut touching = match get(vm, x, "Touching") {
            Value::Array(v) => v,
            _ => Vec::new(),
        };
        if !touching.iter().any(|v| v.as_obj() == Some(y)) {
            touching.push(Value::Object(Some(y)));
            set(vm, x, "Touching", Value::Array(touching));
        }
    }
}

/// Removes a pair; false when it was not touching.
fn unlink(vm: &mut Vm<'_>, a: ObjId, b: ObjId) -> bool {
    let was = world(vm).touch.partners.get(&a).is_some_and(|p| p.contains(&b));
    for (x, y) in [(a, b), (b, a)] {
        if let Some(list) = world(vm).touch.partners.get_mut(&x) {
            list.retain(|&v| v != y);
        }
        if let Value::Array(mut touching) = get(vm, x, "Touching") {
            touching.retain(|v| v.as_obj() != Some(y));
            set(vm, x, "Touching", Value::Array(touching));
        }
    }
    was
}

/// An actor is leaving the level: everything it touched hears `UnTouch`, and it leaves the index.
pub fn forget(vm: &mut Vm<'_>, a: ObjId) {
    if vm.ext.is_none() {
        return;
    }
    let partners = world(vm).touch.partners.remove(&a).unwrap_or_default();
    for b in partners {
        if let Some(list) = world(vm).touch.partners.get_mut(&b) {
            list.retain(|&v| v != a);
        }
        if let Value::Array(mut touching) = get(vm, b, "Touching") {
            touching.retain(|v| v.as_obj() != Some(a));
            set(vm, b, "Touching", Value::Array(touching));
        }
        if !vm.db.objects.get(b).deleted {
            fire(vm, b, "UnTouch", &mut [Value::Object(Some(a))]);
        }
    }
    let t = &mut world(vm).touch;
    if let Some(c) = t.cell_of.remove(&a)
        && let Some(list) = t.cells.get_mut(&c)
    {
        list.retain(|&x| x != a);
    }
    t.big.retain(|&x| x != a);
    t.shapes.remove(&a);
}

/// The end-of-tick pass: re-checks every actor whose shape changed since it was last looked at. On the level's
/// first frame the actors that move by physics are checked against everything once (the rest of the level
/// starts with nothing touching).
pub fn frame_pass(vm: &mut Vm<'_>) {
    if !world(vm).touch.primed {
        prime(vm);
    }
    let s = slots(vm);
    let first = !world(vm).touch.first_pass_done;
    world(vm).touch.first_pass_done = true;
    let actors = world(vm).actors.clone();
    for a in actors {
        if vm.db.objects.get(a).deleted {
            continue;
        }
        let shape = shape_of(vm, a, &s);
        let changed = world(vm).touch.shapes.get(&a) != Some(&shape);
        let moving = first && read(vm, a, s.physics).as_byte() != 0 && !read(vm, a, s.is_static).as_bool();
        if changed || moving {
            settle(vm, a);
        }
    }
}
