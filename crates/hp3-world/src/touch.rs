//! Who touches whom.
//!
//! Behaviour re-expressed from the reference engine: touches are not a per-frame sweep over "moving" actors. An
//! actor's overlaps are re-checked at the moment it moves (`SetLocation`, `Move`, a physics step), is resized,
//! changes its collision flag, or appears; actors that have not changed are only looked at as neighbours of one
//! that has. The first of a pair to be re-checked gets `Touch` first, and the second only gets it if the pair
//! is still touching afterwards. Ending a touch (`UnTouch`) works the same way, and a destroyed actor ends all
//! of its touches at once.
//!
//! A pair touches when both collide with actors, their shapes overlap, at least one of them blocks
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
    /// A volume is its brush: half the size of the box around it (`location` is then that box's middle).
    half: Option<Vec3>,
    /// A `CollideType` box: half sizes along its own axes (radius, width, height) and those axes in the world.
    obox: Option<(Vec3, [Vec3; 3])>,
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
    collide_type: Option<usize>,
    width: Option<usize>,
    rotation: Option<usize>,
}

/// `Actor.CollideType` of a box turned with the actor (0 is the upright cylinder).
const CT_BOX: u8 = 1;

#[derive(Default)]
pub struct Touch {
    slots: Option<Slots>,
    /// The index has every actor of the level (built the first time anything asks).
    primed: bool,
    /// Moving actors have been checked once against everything (the level's first frame).
    first_pass_done: bool,
    /// The object-table pass number the last `frame_pass` started (actors written since are re-checked).
    seen: u32,
    shapes: HashMap<ObjId, Shape>,
    /// The current shape of an actor and the object-table epoch it is good for (`None`: the actor is gone), so a neighbour nobody
    /// wrote since is not read property by property again for each actor that moves near it.
    fresh: HashMap<ObjId, (u32, Option<Shape>)>,
    /// The world box of each actor's shape by object id, good until the actor is next written (epoch 0: unknown), for the quick
    /// rejection of actors far from a trace.
    dense: Vec<DenseBox>,
    cells: HashMap<(i32, i32), Vec<ObjId>>,
    cell_of: HashMap<ObjId, (i32, i32)>,
    big: Vec<ObjId>,
    max_radius: f32,
    /// The box around each volume's brush in the world (None: not a volume, or no brush).
    bounds: HashMap<ObjId, Option<(Vec3, Vec3)>>,
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
        collide_type: actor_slot(vm, "CollideType"),
        width: actor_slot(vm, "CollisionWidth"),
        rotation: actor_slot(vm, "Rotation"),
    };
    world(vm).touch.slots = Some(s);
    s
}

#[derive(Clone, Copy, Default)]
struct DenseBox {
    epoch: u32,
    lo: Vec3,
    hi: Vec3,
}

fn remember_box(vm: &mut Vm<'_>, b: ObjId, shape: Option<&Shape>, epoch: u32) {
    let t = &mut world(vm).touch;
    let i = b.0 as usize;
    if t.dense.len() <= i {
        t.dense.resize(i + 1, DenseBox::default());
    }
    t.dense[i] = match shape {
        Some(s) => {
            let half = s.half.unwrap_or_else(|| extent_of(s));
            DenseBox { epoch, lo: s.location - half, hi: s.location + half }
        }
        None => DenseBox::default(),
    };
}

/// Whether `b` surely lies clear of the box swept by a trace (the segment's box widened by `pad`), judged from its remembered shape;
/// false when it may be in the way or its shape is not known to be current.
pub fn clear_of_trace(vm: &mut Vm<'_>, b: ObjId, lo: Vec3, hi: Vec3, pad: f32) -> bool {
    let Some(d) = world(vm).touch.dense.get(b.0 as usize).copied() else { return false };
    if d.epoch == 0 || vm.db.objects.changed_since(b, d.epoch) {
        return false;
    }
    let pad = Vec3::splat(pad + 1.0);
    let apart = (d.hi + pad).cmplt(lo).any() || (d.lo - pad).cmpgt(hi).any();
    // A mover is met by its geometry, which its cylinder does not bound.
    apart && world(vm).collision.mover(b).is_none()
}

/// `shape_of` for an actor other than the one being settled, remembered until the actor is next written.
fn shape_cached(vm: &mut Vm<'_>, b: ObjId, s: &Slots) -> Option<Shape> {
    if let Some(&(epoch, shape)) = world(vm).touch.fresh.get(&b)
        && !vm.db.objects.changed_since(b, epoch)
    {
        return shape;
    }
    let shape = if vm.db.objects.get(b).deleted { None } else { Some(shape_of(vm, b, s)) };
    // A write from now on is stamped with the new epoch, which is what this entry is good until.
    let epoch = vm.db.objects.begin_pass();
    world(vm).touch.fresh.insert(b, (epoch, shape));
    remember_box(vm, b, shape.as_ref(), epoch);
    shape
}

fn shape_of(vm: &mut Vm<'_>, a: ObjId, s: &Slots) -> Shape {
    let bounds = match world(vm).touch.bounds.get(&a) {
        Some(b) => *b,
        None => {
            let b = crate::volume_bounds(vm, a);
            world(vm).touch.bounds.insert(a, b);
            b
        }
    };
    let collide = read(vm, a, s.collide).as_bool();
    if let Some((lo, hi)) = bounds {
        let half = (hi - lo) * 0.5;
        return Shape { collide, location: (lo + hi) * 0.5, radius: half.truncate().length(), height: half.z, half: Some(half), obox: None };
    }
    let (radius, height) = (read(vm, a, s.radius).as_float(), read(vm, a, s.height).as_float());
    let location = read_vec(vm, a, s.location);
    if read(vm, a, s.collide_type).as_byte() == CT_BOX {
        let width = read(vm, a, s.width).as_float();
        let half = Vec3::new(radius, if width > 0.0 { width } else { radius }, height);
        let r: Vec<i32> = read(vm, a, s.rotation).fields().iter().map(Value::as_int).collect();
        let ax = hp3_script::natives::rot_axes([r.first().copied().unwrap_or(0), r.get(1).copied().unwrap_or(0), r.get(2).copied().unwrap_or(0)]);
        let axes = [Vec3::from(ax[0]), Vec3::from(ax[1]), Vec3::from(ax[2])];
        // For the spatial index the box counts as wide as its corners reach.
        return Shape { collide, location, radius: half.truncate().length(), height: world_half(half, &axes).z, half: None, obox: Some((half, axes)) };
    }
    Shape { collide, location, radius, height, half: None, obox: None }
}

/// Half size of the world-aligned box around a turned box.
fn world_half(half: Vec3, axes: &[Vec3; 3]) -> Vec3 {
    axes[0].abs() * half.x + axes[1].abs() * half.y + axes[2].abs() * half.z
}

/// A turned box against an upright cylinder: the cylinder's middle in the box's frame, its nearest point of the box
/// across the ground less than the radius away, and the heights overlapping.
fn box_cylinder(location: Vec3, half: Vec3, axes: &[Vec3; 3], c: &Shape) -> bool {
    let d = c.location - location;
    let local = Vec3::new(d.dot(axes[0]), d.dot(axes[1]), d.dot(axes[2]));
    let near = local.clamp(-half, half);
    let (dx, dy) = (local.x - near.x, local.y - near.y);
    dx * dx + dy * dy < c.radius * c.radius && local.z.abs() < half.z + c.height
}

/// Two turned boxes: no separating axis among the ground axes of either, and their world heights overlap.
fn box_box(a: &Shape, ah: Vec3, aa: &[Vec3; 3], b: &Shape, bh: Vec3, ba: &[Vec3; 3]) -> bool {
    let d = b.location - a.location;
    if d.z.abs() >= world_half(ah, aa).z + world_half(bh, ba).z {
        return false;
    }
    let flat = |v: Vec3| Vec3::new(v.x, v.y, 0.0);
    let reach = |h: Vec3, ax: &[Vec3; 3], n: Vec3| h.x * flat(ax[0]).dot(n).abs() + h.y * flat(ax[1]).dot(n).abs();
    for n in [aa[0], aa[1], ba[0], ba[1]].map(flat) {
        if n.length_squared() < 1e-6 {
            continue;
        }
        let n = n.normalize();
        if flat(d).dot(n).abs() >= reach(ah, aa, n) + reach(bh, ba, n) {
            return false;
        }
    }
    true
}

fn cell(p: Vec3) -> (i32, i32) {
    ((p.x / CELL).floor() as i32, (p.y / CELL).floor() as i32)
}

fn overlaps(a: &Shape, b: &Shape) -> bool {
    let d = a.location - b.location;
    // A volume's brush box against a cylinder or a turned box (each taken as its box) or another brush box.
    if a.half.is_some() || b.half.is_some() {
        let as_box = |s: &Shape| s.half.or(s.obox.map(|(h, ax)| world_half(h, &ax))).unwrap_or(Vec3::new(s.radius, s.radius, s.height));
        let (ha, hb) = (as_box(a), as_box(b));
        return d.x.abs() < ha.x + hb.x && d.y.abs() < ha.y + hb.y && d.z.abs() < ha.z + hb.z;
    }
    match (a.obox, b.obox) {
        (Some((ah, aa)), Some((bh, ba))) => box_box(a, ah, &aa, b, bh, &ba),
        (Some((h, ax)), None) => box_cylinder(a.location, h, &ax, b),
        (None, Some((h, ax))) => box_cylinder(b.location, h, &ax, a),
        (None, None) => (d.x * d.x + d.y * d.y).sqrt() < a.radius + b.radius && d.z.abs() < a.height + b.height,
    }
}

/// Whether two actors overlap. A mover is its geometry rather than its cylinder, and two movers never overlap.
fn overlap_between(col: &crate::collision::Collision, a: (ObjId, &Shape), b: (ObjId, &Shape)) -> bool {
    match (col.mover(a.0), col.mover(b.0)) {
        (Some(_), Some(_)) => false,
        (Some(m), None) => m.overlaps(b.1.location, extent_of(b.1)),
        (None, Some(m)) => m.overlaps(a.1.location, extent_of(a.1)),
        (None, None) => overlaps(a.1, b.1),
    }
}

/// Half size of the world-aligned box an actor fills (a mover is tested against it).
fn extent_of(s: &Shape) -> Vec3 {
    s.obox.map(|(h, ax)| world_half(h, &ax)).unwrap_or(Vec3::new(s.radius, s.radius, s.height))
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
            let Some(other) = shape_cached(vm, b, &s) else { continue };
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
    if crate::envc::var_os("HP3_SLIDE_DEBUG").is_some() && (crate::physics::is_player_pawn(vm, a) || crate::physics::is_player_pawn(vm, b)) {
        let (pa, pb) = (hp3_script::natives::object_path(vm, a), hp3_script::natives::object_path(vm, b));
        eprintln!("slide: frame {} touch {pa} and {pb}", world(vm).frame);
    }
    link(vm, a, b);
    for x in [a, b] {
        if vm.db.objects.get(x).class.is_some_and(|c| vm.db.names.get(vm.db.get(c).name) == "WizardCracker") {
            let on_ground = get(vm, x, "bOnGround").as_bool();
            let (pa, pb) = (hp3_script::natives::object_path(vm, a), hp3_script::natives::object_path(vm, b));
            vm.log.push(format!("touch: {pa} and {pb} (cracker bOnGround {on_ground})"));
            // What PickupActor looks at, for the player's pawn touching it.
            let other = if x == a { b } else { a };
            if is_player(vm, other) {
                let main = get(vm, other, "bIsMainPlayer").as_bool();
                let can_be = get(vm, x, "bCanBePickedUp").as_bool();
                let phys = get(vm, other, "Physics").as_byte();
                let ctl = get(vm, other, "Controller").as_obj();
                let ctl_state = ctl.and_then(|c| vm.state_of(c)).map(|s| vm.db.names.get(vm.db.get(s).name).to_string()).unwrap_or_default();
                let holding = get(vm, other, "aHolding").as_obj().map(|o| (hp3_script::natives::object_path(vm, o), vm.db.objects.get(o).deleted));
                let can_do = fire(vm, other, "CanDoPickupActor", &mut []).as_bool();
                vm.log.push(format!("pickup check: bIsMainPlayer {main} bCanBePickedUp {can_be} physics {phys} controller state '{ctl_state}' aHolding {holding:?} CanDoPickupActor {can_do}"));
            }
        }
    }
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
    // Its shape is gone from the index: the next pass must look at it again.
    vm.db.objects.get_mut(a);
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
    let seen = std::mem::replace(&mut world(vm).touch.seen, 0);
    let next = vm.db.objects.begin_pass();
    world(vm).touch.seen = next;
    for a in actors {
        if vm.db.objects.get(a).deleted {
            continue;
        }
        // A shape is made of the actor's own properties: one nobody wrote since the last pass has not changed.
        if !first && !vm.db.objects.changed_since(a, seen) {
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

/// Colliding actors whose cylinders could be reached by a segment from `start` to `end` swept with a box of
/// half size `pad` (a superset; callers test each one).
pub fn along(vm: &mut Vm<'_>, start: Vec3, end: Vec3, pad: f32) -> Vec<ObjId> {
    if !world(vm).touch.primed {
        prime(vm);
    }
    let t = &world(vm).touch;
    let reach = pad + t.max_radius;
    let lo = start.min(end) - Vec3::splat(reach);
    let hi = start.max(end) + Vec3::splat(reach);
    let (x0, y0) = cell(lo);
    let (x1, y1) = cell(hi);
    let mut out: Vec<ObjId> = t.big.clone();
    if ((x1 - x0 + 1) as i64) * ((y1 - y0 + 1) as i64) > t.cells.len() as i64 {
        for ((cx, cy), list) in &t.cells {
            if (x0..=x1).contains(cx) && (y0..=y1).contains(cy) {
                out.extend_from_slice(list);
            }
        }
    } else {
        for cx in x0..=x1 {
            for cy in y0..=y1 {
                if let Some(list) = t.cells.get(&(cx, cy)) {
                    out.extend_from_slice(list);
                }
            }
        }
    }
    out
}

fn is_player(vm: &mut Vm<'_>, a: ObjId) -> bool {
    world(vm).player.is_some_and(|pc| get(vm, pc, "Pawn").as_obj() == Some(a))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cylinder(at: Vec3, radius: f32, height: f32) -> Shape {
        Shape { collide: true, location: at, radius, height, half: None, obox: None }
    }

    fn turned_box(at: Vec3, half: Vec3, yaw: i32) -> Shape {
        let ax = hp3_script::natives::rot_axes([0, yaw, 0]);
        let axes = [Vec3::from(ax[0]), Vec3::from(ax[1]), Vec3::from(ax[2])];
        Shape { collide: true, location: at, radius: half.truncate().length(), height: world_half(half, &axes).z, half: None, obox: Some((half, axes)) }
    }

    #[test]
    fn thin_box_across_a_doorway_is_touched_along_its_whole_width() {
        // A 10 deep, 200 wide line trigger (the crab room's cutscene line) and a pawn of radius 15.
        let line = turned_box(Vec3::ZERO, Vec3::new(10.0, 200.0, 100.0), 0);
        assert!(overlaps(&line, &cylinder(Vec3::new(0.0, 150.0, 0.0), 15.0, 38.0)));
        assert!(overlaps(&cylinder(Vec3::new(20.0, -190.0, 0.0), 15.0, 38.0), &line));
        assert!(!overlaps(&line, &cylinder(Vec3::new(30.0, 0.0, 0.0), 15.0, 38.0)));
        assert!(!overlaps(&line, &cylinder(Vec3::new(0.0, 220.0, 0.0), 15.0, 38.0)));
        assert!(!overlaps(&line, &cylinder(Vec3::new(0.0, 0.0, 140.0), 15.0, 38.0)));
    }

    #[test]
    fn a_turned_box_is_tested_in_its_own_frame() {
        // Turned a quarter: its width now runs along x.
        let line = turned_box(Vec3::ZERO, Vec3::new(10.0, 200.0, 100.0), 16384);
        assert!(overlaps(&line, &cylinder(Vec3::new(150.0, 0.0, 0.0), 15.0, 38.0)));
        assert!(!overlaps(&line, &cylinder(Vec3::new(0.0, 150.0, 0.0), 15.0, 38.0)));
    }

    #[test]
    fn two_boxes_meet_only_without_a_gap() {
        let a = turned_box(Vec3::ZERO, Vec3::new(10.0, 100.0, 50.0), 0);
        let b = turned_box(Vec3::new(50.0, 0.0, 0.0), Vec3::new(10.0, 100.0, 50.0), 8192);
        assert!(overlaps(&a, &b));
        let c = turned_box(Vec3::new(200.0, 0.0, 0.0), Vec3::new(10.0, 100.0, 50.0), 0);
        assert!(!overlaps(&a, &c));
    }
}
