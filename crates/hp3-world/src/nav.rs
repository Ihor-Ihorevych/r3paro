//! Navigation: the path network of the map's navigation points, reachability tests, route finding and the
//! latent AI moves (`MoveTo`, `MoveToward`) that steer a pawn along a route.
//!
//! Written from the script declarations and standard Unreal behaviour; NOT yet verified against HP3's
//! Engine.dll. Reachability is a simplified line test (no jump or fall analysis) and route costs are the
//! reach specs' distances plus the nodes' `cost` and `ExtraCost`.

use std::collections::BinaryHeap;

use hp3_script::fx::FxHashMap as HashMap;

use glam::Vec3;
use hp3_script::vm::Latent;
use hp3_script::{ObjId, Value, Vm};

use crate::physics::{vec, value};
use crate::{get, set, slot, world};

struct Edge {
    to: usize,
    distance: f32,
}

#[derive(Default)]
pub struct Graph {
    pub nodes: Vec<ObjId>,
    positions: Vec<Vec3>,
    edges: Vec<Vec<Edge>>,
}

/// A latent move in progress for a controller.
#[derive(Clone, Copy, Debug)]
pub struct Move {
    pub target: Option<ObjId>,
    pub destination: Vec3,
    /// Seconds left before giving up.
    pub timer: f32,
}

#[derive(Default)]
pub struct Navigation {
    graph: Option<std::rc::Rc<Graph>>,
    pub moves: HashMap<ObjId, Move>,
}

/// Height the reachability trace is lifted by, so that steps and small bumps do not block it.
const STEP: f32 = 24.0;
/// Nodes farther than this from a point are not considered as its entry to the network.
const ANCHOR_RANGE: f32 = 1500.0;

/// Number of navigation points and links in the map's path network.
pub fn network_size(vm: &mut Vm<'_>) -> (usize, usize) {
    let g = graph(vm);
    (g.nodes.len(), g.edges.iter().map(Vec::len).sum())
}

fn graph(vm: &mut Vm<'_>) -> std::rc::Rc<Graph> {
    if let Some(g) = &world(vm).nav.graph {
        return g.clone();
    }
    let mut g = Graph::default();
    let actors = world(vm).actors.clone();
    let mut index = HashMap::default();
    if let Some(nav) = vm.db.find("NavigationPoint") {
        for a in actors {
            let e = vm.db.objects.get(a);
            if !e.deleted && e.class.is_some_and(|c| vm.is_child_of(c, nav)) {
                index.insert(a, g.nodes.len());
                g.nodes.push(a);
                g.positions.push(vec(&get(vm, a, "Location")));
            }
        }
    }
    for (i, &n) in g.nodes.clone().iter().enumerate() {
        let mut edges = Vec::new();
        if let Value::Array(specs) = get(vm, n, "PathList") {
            for spec in specs.iter().filter_map(Value::as_obj) {
                if !vm.ensure_loaded(spec) {
                    continue;
                }
                let Some(end) = get(vm, spec, "End").as_obj() else { continue };
                let Some(&to) = index.get(&end) else { continue };
                let distance = get(vm, spec, "Distance").as_int() as f32;
                let distance = if distance > 0.0 { distance } else { g.positions[i].distance(g.positions[to]) };
                edges.push(Edge { to, distance });
            }
        }
        g.edges.push(edges);
    }
    let g = std::rc::Rc::new(g);
    world(vm).nav.graph = Some(g.clone());
    g
}

/// Whether a pawn could walk in a straight line from `from` to `to`: nothing solid between them (a hit on
/// `goal` itself counts as reaching it).
pub fn reachable(vm: &mut Vm<'_>, pawn: ObjId, from: Vec3, to: Vec3, goal: Option<ObjId>) -> bool {
    let lift = Vec3::Z * STEP;
    match world(vm).collision.trace(from + lift, to + lift, Vec3::ZERO, Some(pawn)) {
        None => true,
        Some(h) => goal.is_some() && h.actor == goal,
    }
}

/// Network nodes a point can reach directly (or be reached from), nearest first, with their distances.
fn anchors(vm: &mut Vm<'_>, g: &Graph, pawn: ObjId, at: Vec3, goal: Option<ObjId>) -> Vec<(usize, f32)> {
    let mut near: Vec<(usize, f32)> =
        g.positions.iter().enumerate().map(|(i, p)| (i, p.distance(at))).filter(|(_, d)| *d < ANCHOR_RANGE).collect();
    near.sort_by(|a, b| a.1.total_cmp(&b.1));
    near.into_iter().filter(|&(i, _)| reachable(vm, pawn, at, g.positions[i], goal.or(Some(g.nodes[i])))).take(4).collect()
}

#[derive(PartialEq)]
struct Open(f32, usize);
impl Eq for Open {}
impl PartialOrd for Open {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Open {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        o.0.total_cmp(&self.0)
    }
}

/// The cheapest route from the pawn to `goal_at`: the nodes to visit in order and its length.
fn route(vm: &mut Vm<'_>, pawn: ObjId, goal_at: Vec3, goal: Option<ObjId>) -> Option<(Vec<ObjId>, f32)> {
    let g = graph(vm);
    if g.nodes.is_empty() {
        return None;
    }
    let from = vec(&get(vm, pawn, "Location"));
    let starts = anchors(vm, &g, pawn, from, None);
    let ends: HashMap<usize, f32> = anchors(vm, &g, pawn, goal_at, goal).into_iter().collect();
    if starts.is_empty() || ends.is_empty() {
        return None;
    }
    let node_cost: Vec<f32> = g
        .nodes
        .iter()
        .map(|&n| {
            if get(vm, n, "bBlocked").as_bool() {
                f32::INFINITY
            } else {
                (get(vm, n, "cost").as_int() + get(vm, n, "ExtraCost").as_int()) as f32
            }
        })
        .collect();
    let mut best = vec![f32::INFINITY; g.nodes.len()];
    let mut prev = vec![usize::MAX; g.nodes.len()];
    let mut open = BinaryHeap::new();
    for &(s, d) in &starts {
        if d < best[s] {
            best[s] = d;
            open.push(Open(d, s));
        }
    }
    let mut found: Option<(usize, f32)> = None;
    while let Some(Open(d, n)) = open.pop() {
        if d > best[n] {
            continue;
        }
        if let Some(&e) = ends.get(&n) {
            let total = d + e;
            if found.is_none_or(|f| total < f.1) {
                found = Some((n, total));
            }
        }
        if found.is_some_and(|f| d >= f.1) {
            break;
        }
        for edge in &g.edges[n] {
            let nd = d + edge.distance + node_cost[edge.to];
            if nd < best[edge.to] {
                best[edge.to] = nd;
                prev[edge.to] = n;
                open.push(Open(nd, edge.to));
            }
        }
    }
    let (mut n, total) = found?;
    let mut path = vec![g.nodes[n]];
    while prev[n] != usize::MAX {
        n = prev[n];
        path.push(g.nodes[n]);
    }
    path.reverse();
    Some((path, total))
}

fn set_indexed(vm: &mut Vm<'_>, o: ObjId, name: &str, index: usize, v: Value) {
    let Some(s) = slot(vm, o, name) else { return };
    if let Some(x) = vm.db.objects.values_mut(o).and_then(|vals| vals.get_mut(s + index)) {
        *x = v;
    }
}

/// Finds a route for a controller's pawn and records it; returns the next point to move toward.
fn find_path(vm: &mut Vm<'_>, controller: ObjId, goal_at: Vec3, goal: Option<ObjId>) -> Option<ObjId> {
    let pawn = get(vm, controller, "Pawn").as_obj()?;
    let from = vec(&get(vm, pawn, "Location"));
    if reachable(vm, pawn, from, goal_at, goal) {
        if let Some(g) = goal {
            set_indexed(vm, controller, "RouteCache", 0, Value::Object(Some(g)));
            return Some(g);
        }
    }
    let (path, dist) = route(vm, pawn, goal_at, goal)?;
    for i in 0..16 {
        set_indexed(vm, controller, "RouteCache", i, Value::Object(path.get(i).copied()));
    }
    set(vm, controller, "RouteGoal", Value::Object(goal.or(path.last().copied())));
    set(vm, controller, "RouteDist", Value::Float(dist));
    // A node the pawn already stands on is skipped.
    let radius = get(vm, pawn, "CollisionRadius").as_float().max(16.0);
    let first = path[0];
    let at_first = vec(&get(vm, first, "Location")).truncate().distance(from.truncate()) < radius;
    Some(if at_first && path.len() > 1 { path[1] } else { first })
}

fn start_move(vm: &mut Vm<'_>, controller: ObjId, target: Option<ObjId>, destination: Vec3) {
    let Some(pawn) = get(vm, controller, "Pawn").as_obj() else { return };
    let from = vec(&get(vm, pawn, "Location"));
    let speed = get(vm, pawn, "GroundSpeed").as_float().max(50.0);
    let timer = 1.0 + 2.0 * from.distance(destination) / speed;
    set(vm, controller, "MoveTarget", Value::Object(target));
    set(vm, controller, "Destination", value(destination));
    set(vm, controller, "MoveTimer", Value::Float(timer));
    world(vm).nav.moves.insert(controller, Move { target, destination, timer });
    vm.latent.insert(controller, Latent::Other("move"));
}

/// Steers pawns of controllers with a move in progress; ends moves that arrived or ran out of time.
pub fn tick(vm: &mut Vm<'_>, dt: f32) {
    // WaitForLanding ends when the pawn is no longer falling.
    let waiting: Vec<ObjId> = vm.latent.iter().filter(|(_, l)| matches!(l, Latent::Other("landing"))).map(|(o, _)| *o).collect();
    for c in waiting {
        let falling = get(vm, c, "Pawn").as_obj().is_some_and(|p| get(vm, p, "Physics").as_byte() == crate::physics::PHYS_FALLING);
        if !falling {
            vm.finish_latent(c);
        }
    }
    let moves: Vec<(ObjId, Move)> = world(vm).nav.moves.iter().map(|(k, v)| (*k, *v)).collect();
    for (c, mut m) in moves {
        let pawn = get(vm, c, "Pawn").as_obj().filter(|&p| !vm.db.objects.get(p).deleted);
        let dest = match m.target {
            Some(t) if !vm.db.objects.get(t).deleted => vec(&get(vm, t, "Location")),
            _ => m.destination,
        };
        m.timer -= dt;
        let done = match pawn {
            None => true,
            Some(p) => {
                let at = vec(&get(vm, p, "Location"));
                let flying = get(vm, p, "Physics").as_byte() == crate::physics::PHYS_FLYING;
                let mut delta = dest - at;
                if !flying {
                    delta.z = 0.0;
                }
                let target_radius = m.target.map(|t| get(vm, t, "CollisionRadius").as_float()).unwrap_or(0.0);
                let reach = get(vm, p, "CollisionRadius").as_float().max(8.0) * 0.5 + target_radius;
                if delta.length() <= reach.max(8.0) || m.timer <= 0.0 {
                    set(vm, p, "Acceleration", value(Vec3::ZERO));
                    true
                } else {
                    let dir = delta.normalize();
                    let accel = get(vm, p, "AccelRate").as_float().max(100.0);
                    set(vm, p, "Acceleration", value(dir * accel));
                    let yaw = (dir.y.atan2(dir.x) * 65536.0 / std::f32::consts::TAU).round() as i32;
                    let mut rot: Vec<Value> = get(vm, p, "Rotation").fields().to_vec();
                    if rot.len() == 3 {
                        rot[1] = Value::Int(yaw);
                        set(vm, p, "Rotation", Value::Struct(rot.into()));
                    }
                    false
                }
            }
        };
        set(vm, c, "MoveTimer", Value::Float(m.timer));
        if done {
            world(vm).nav.moves.remove(&c);
            if matches!(vm.latent.get(&c), Some(Latent::Other("move"))) {
                vm.finish_latent(c);
            }
        } else {
            world(vm).nav.moves.insert(c, m);
        }
    }
}

pub fn register(vm: &mut Vm<'_>) {
    // FindPathToward(Actor anActor, optional bool bWeightDetours, optional bool bClearPaths)
    vm.register("Controller.FindPathToward", |vm, this, a| {
        let (Some(c), Some(goal)) = (this, a.first().and_then(Value::as_obj)) else { return Ok(Value::Object(None)) };
        let at = vec(&get(vm, goal, "Location"));
        Ok(Value::Object(find_path(vm, c, at, Some(goal))))
    });
    // FindPathTo(vector aPoint, ...)
    vm.register("Controller.FindPathTo", |vm, this, a| {
        let Some(c) = this else { return Ok(Value::Object(None)) };
        let at = vec(&a[0]);
        Ok(Value::Object(find_path(vm, c, at, None)))
    });
    vm.register("Controller.actorReachable", |vm, this, a| {
        let (Some(c), Some(goal)) = (this, a.first().and_then(Value::as_obj)) else { return Ok(Value::Bool(false)) };
        let Some(pawn) = get(vm, c, "Pawn").as_obj() else { return Ok(Value::Bool(false)) };
        let (from, to) = (vec(&get(vm, pawn, "Location")), vec(&get(vm, goal, "Location")));
        Ok(Value::Bool(reachable(vm, pawn, from, to, Some(goal))))
    });
    vm.register("Controller.pointReachable", |vm, this, a| {
        let Some(pawn) = this.and_then(|c| get(vm, c, "Pawn").as_obj()) else { return Ok(Value::Bool(false)) };
        let from = vec(&get(vm, pawn, "Location"));
        Ok(Value::Bool(reachable(vm, pawn, from, vec(&a[0]), None)))
    });
    // MoveTo(vector NewDestination, optional Actor ViewFocus, optional bool bShouldWalk)
    vm.register("Controller.MoveTo", |vm, this, a| {
        if let Some(c) = this {
            start_move(vm, c, None, vec(&a[0]));
        }
        Ok(Value::Empty)
    });
    // MoveToward(Actor NewTarget, optional Actor ViewFocus, ...)
    vm.register("Controller.MoveToward", |vm, this, a| {
        if let (Some(c), Some(t)) = (this, a.first().and_then(Value::as_obj)) {
            let at = vec(&get(vm, t, "Location"));
            start_move(vm, c, Some(t), at);
        }
        Ok(Value::Empty)
    });
    vm.register("Controller.WaitForLanding", |vm, this, _a| {
        if let Some(c) = this {
            vm.latent.insert(c, Latent::Other("landing"));
        }
        Ok(Value::Empty)
    });
    // Turning is immediate for now: the pawn takes the controller's desired rotation.
    vm.register("Controller.FinishRotation", |vm, this, _a| {
        if let Some(c) = this
            && let Some(p) = get(vm, c, "Pawn").as_obj()
        {
            let r = get(vm, p, "DesiredRotation");
            if matches!(r, Value::Struct(_)) {
                set(vm, p, "Rotation", r);
            }
        }
        Ok(Value::Empty)
    });
    vm.register("Controller.StopMovement", |vm, this, _a| {
        if let Some(c) = this {
            world(vm).nav.moves.remove(&c);
            if let Some(p) = get(vm, c, "Pawn").as_obj() {
                set(vm, p, "Acceleration", value(Vec3::ZERO));
            }
        }
        Ok(Value::Empty)
    });
}
