//! Collision queries against the level: the BSP, static meshes and terrain (as triangles), and blocking
//! actors (as boxes). Traces sweep an axis-aligned box (a point when the extent is zero) from start to end.
//!
//! Written from the decoded data and standard swept-box techniques; NOT yet verified against HP3's Engine.dll,
//! whose collision rules (cylinders, hulls, per-poly flags) will be matched against traces from the retail game.

use hp3_script::fx::FxHashMap as HashMap;

use glam::Vec3;
use hp3_assets::model::Model;
use hp3_script::ObjId;

/// Poly flags that make a BSP surface pass-through: not solid, or a zone portal.
const PF_NOT_SOLID: u32 = 0x0000_0008;
const PF_PORTAL: u32 = 0x0400_0000;
const EPSILON: f32 = 0.03125;
/// Grid cell size for triangles, in world units.
const CELL: f32 = 64.0;

#[derive(Clone, Copy, Debug)]
pub struct Hit {
    /// Fraction of the move done before contact.
    pub time: f32,
    pub location: Vec3,
    pub normal: Vec3,
    pub actor: Option<ObjId>,
}

#[derive(Clone, Copy)]
struct Tri {
    v: [Vec3; 3],
    normal: Vec3,
    actor: Option<ObjId>,
}

/// A blocking actor's box: center and half extent.
#[derive(Clone, Copy)]
pub struct ActorBox {
    pub actor: ObjId,
    pub center: Vec3,
    pub extent: Vec3,
    /// Blocks other actors (`bBlockActors`) and player-controlled ones (`bBlockPlayers`).
    pub blocks_actors: bool,
    pub blocks_players: bool,
    pub is_pawn: bool,
    /// The actor's `Owner`: an owner and what it owns do not stop each other (a thrown pebble passes its thrower).
    pub owner: Option<ObjId>,
    /// `CollideType` box: the box's own axes in the world (it turns with the actor). None: an upright cylinder.
    pub axes: Option<[Vec3; 3]>,
}

#[derive(Default)]
pub struct Collision {
    model: Option<std::rc::Rc<Model>>,
    tris: Vec<Tri>,
    grid: HashMap<(i32, i32), Vec<u32>>,
    pub actors: Vec<ActorBox>,
    /// The `Owner` of the actor being moved.
    pub mover_owner: Option<ObjId>,
    /// Whether the trace being made is for the player's pawn (which `bBlockPlayers` blocks) or for any other
    /// actor (which `bBlockActors` blocks).
    player_mover: bool,
    /// For the trace being made: the thing moving is not stopped by blocking actors (it does not have
    /// `bBlockActors`) or by the level's geometry (it does not have `bCollideWorld`).
    passes_actors: bool,
    passes_world: bool,
    /// Movers: their triangles in their own space, placed by a transform that follows them.
    pub movers: Vec<MoverShape>,
    /// Static meshes that have a simple collision model, already placed in the world, and the cells they cover.
    mesh_models: Vec<MeshModel>,
    modeled: HashMap<ObjId, (bool, bool)>,
    /// Actors whose static triangles, volumes or models are baked into the level but whose collision the game has
    /// turned off since (`SetCollision`, a destroyed actor): they are not traced against until it is on again.
    inactive: std::collections::HashSet<ObjId>,
    /// Who the baked shapes of an actor stop: (other actors, players). An actor not listed stops both.
    blocks: HashMap<ObjId, (bool, bool)>,
    model_grid: HashMap<(i32, i32), Vec<u32>>,
    /// World boxes of the volumes (blocking volumes): a mover that starts inside one is not held by it, so it can leave.
    volume_bounds: HashMap<ObjId, (Vec3, Vec3)>,
    /// `baked_owners`, kept until a triangle or model is added.
    owners_cache: Option<Vec<ObjId>>,
}

/// A static mesh's collision model placed in the world. It stands in for the mesh's triangles for the kinds of
/// trace its flags allow: boxes (`UseSimpleBoxCollision`) and lines (`UseSimpleLineCollision`).
struct MeshModel {
    actor: ObjId,
    model: std::rc::Rc<Model>,
}

/// How the thing being moved collides with others.
#[derive(Clone, Copy)]
pub struct Mover {
    /// The player's pawn (which `bBlockPlayers` actors block).
    pub player: bool,
    /// It has `bBlockActors`: blocking actors stop it.
    pub blocks_actors: bool,
    /// It has `bCollideWorld`: the level's geometry stops it.
    pub collide_world: bool,
}

/// A mover's collision: triangles in mover space and the current mover-to-world placement.
pub struct MoverShape {
    pub actor: ObjId,
    tris: Vec<Tri>,
    pub to_world: glam::Mat4,
    to_local: glam::Mat4,
    /// Radius of the triangles around the mover's origin, to skip far traces.
    radius: f32,
    /// `world_bounds` for the current placement, kept up to date by `place`.
    bounds: (Vec3, Vec3),
    /// The mesh's simple collision model in mover space, and its `UseSimpleBoxCollision` / `UseSimpleLineCollision`: it answers
    /// the traces those allow in place of the triangles, as it does for a static mesh actor that stands still.
    model: Option<(std::rc::Rc<Model>, bool, bool)>,
}

impl MoverShape {
    /// Gives the shape its mesh's collision model (planes already scaled into mover space).
    pub fn set_model(&mut self, model: Model, boxes: bool, lines: bool) {
        if !model.nodes.is_empty() && (boxes || lines) {
            self.model = Some((std::rc::Rc::new(model), boxes, lines));
        }
    }

    pub fn new(actor: ObjId, triangles: &[[Vec3; 3]]) -> MoverShape {
        let mut tris = Vec::new();
        let mut radius: f32 = 0.0;
        for v in triangles {
            let normal = (v[1] - v[0]).cross(v[2] - v[0]);
            if normal.length_squared() < 1e-8 {
                continue;
            }
            radius = radius.max(v[0].length()).max(v[1].length()).max(v[2].length());
            tris.push(Tri { v: *v, normal: normal.normalize(), actor: Some(actor) });
        }
        let mut shape = MoverShape { actor, tris, to_world: glam::Mat4::IDENTITY, to_local: glam::Mat4::IDENTITY, radius, bounds: (Vec3::ZERO, Vec3::ZERO), model: None };
        shape.bounds = shape.world_bounds();
        shape
    }

    /// The box around the mover's triangles in the world as it is now.
    pub fn world_bounds(&self) -> (Vec3, Vec3) {
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for t in &self.tris {
            for v in t.v {
                let w = self.to_world.transform_point3(v);
                lo = lo.min(w);
                hi = hi.max(w);
            }
        }
        (lo, hi)
    }

    /// The triangles in the world as the mover stands now (for the probe tools).
    pub fn world_triangles(&self) -> Vec<[Vec3; 3]> {
        self.tris.iter().map(|t| t.v.map(|v| self.to_world.transform_point3(v))).collect()
    }

    pub fn place(&mut self, to_world: glam::Mat4) {
        self.to_world = to_world;
        self.to_local = to_world.inverse();
        self.bounds = self.world_bounds();
    }

    /// Where a box swept from `start` to `end` first meets the mover's triangles.
    pub fn hit(&self, start: Vec3, end: Vec3, extent: Vec3) -> Option<Hit> {
        self.trace(start, end, extent)
    }

    /// How far the mover's triangles reach from its origin.
    pub fn reach(&self) -> f32 {
        self.radius
    }

    /// Whether a box centered at `at` overlaps the mover's triangles.
    pub fn overlaps(&self, at: Vec3, extent: Vec3) -> bool {
        self.trace(at + Vec3::Z * 0.1, at, extent).is_some_and(|h| h.time <= 0.0)
    }

    fn trace(&self, start: Vec3, end: Vec3, extent: Vec3) -> Option<Hit> {
        let origin = self.to_world.w_axis.truncate();
        let reach = self.radius + extent.length() + start.distance(end);
        if origin.distance(start) > reach + 1.0 {
            return None;
        }
        // The box around the move against the mover's box as it stands (a big, thin mover is mostly empty sphere).
        let pad = extent.length() + 1.0;
        if (start.min(end) - pad).cmpgt(self.bounds.1).any() || (start.max(end) + pad).cmplt(self.bounds.0).any() {
            return None;
        }
        let (ls, le) = (self.to_local.transform_point3(start), self.to_local.transform_point3(end));
        // The simple collision model, when this kind of trace is one it answers.
        static NO_MODELS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if let Some((model, boxes, lines)) = &self.model
            && !*NO_MODELS.get_or_init(|| crate::envc::var_os("HP3_NO_MODELS").is_some() || crate::envc::var_os("HP3_NO_MOVER_MODELS").is_some())
            && if extent == Vec3::ZERO { *lines } else { *boxes }
        {
            let mut out = None;
            bsp_trace(model, 0, 0.0, 1.0, ls, le, ls, le, extent, Vec3::Z, &mut out);
            return out.map(|h| Hit {
                time: h.time,
                location: start + (end - start) * h.time,
                normal: self.to_world.transform_vector3(h.normal).normalize_or_zero(),
                actor: Some(self.actor),
            });
        }
        let mut best: Option<Hit> = None;
        for t in &self.tris {
            if let Some(h) = tri_trace(t, ls, le, extent)
                && best.is_none_or(|b| h.time < b.time)
            {
                best = Some(h);
            }
        }
        best.map(|h| Hit {
            time: h.time,
            location: start + (end - start) * h.time,
            normal: self.to_world.transform_vector3(h.normal).normalize_or_zero(),
            actor: Some(self.actor),
        })
    }
}

/// Where a box of half size `extent` swept from `start` to `end` first meets an actor's collision cylinder
/// (`radius`, `half_height`, centered on `center`), or `None`. A sweep that starts inside the cylinder only
/// counts when it moves toward the axis (it is then stopped at once); moving outward or sideways it is let
/// through.
pub fn cylinder_trace(center: Vec3, radius: f32, half_height: f32, start: Vec3, end: Vec3, extent: Vec3) -> Option<Hit> {
    let reach = radius + extent.x;
    let half = half_height + extent.z;
    let (top, bottom) = (center.z + half, center.z - half);
    let d = end - start;
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    let mut normal = Vec3::ZERO;
    // Time spent between the cylinder's top and bottom planes.
    if d.z.abs() > 1e-9 {
        let (ta, tb) = ((top - start.z) / d.z, (bottom - start.z) / d.z);
        let (enter, leave) = (ta.min(tb), ta.max(tb));
        if enter > t0 {
            t0 = enter;
            normal = if d.z < 0.0 { Vec3::Z } else { -Vec3::Z };
        }
        t1 = t1.min(leave);
    } else if start.z > top || start.z < bottom {
        return None;
    }
    if t0 >= t1 {
        return None;
    }
    let k = Vec3::new(start.x - center.x, start.y - center.y, 0.0);
    let c = k.length_squared() - reach * reach;
    if c < 1.0 && start.z > bottom && start.z < top {
        let inward = d.x * k.x + d.y * k.y;
        return (inward < -0.1).then(|| Hit { time: 0.0, location: start, normal: k.normalize_or_zero(), actor: None });
    }
    let a = d.x * d.x + d.y * d.y;
    let b = 2.0 * (k.x * d.x + k.y * d.y);
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return None;
    }
    if a < 1e-8 {
        if c > 0.0 {
            return None;
        }
    } else {
        let root = disc.sqrt();
        t1 = t1.min((root - b) / (2.0 * a));
        let t = -(root + b) / (2.0 * a);
        if t > t0 {
            t0 = t;
            let at = start + d * t0 - center;
            normal = Vec3::new(at.x, at.y, 0.0).normalize_or_zero();
        }
    }
    if t0 >= t1 {
        return None;
    }
    Some(Hit { time: t0, location: start + d * t0, normal, actor: None })
}

thread_local! {
    /// Per triangle, the stamp of the last trace that tried it, and the last stamp given out.
    static TRI_STAMPS: std::cell::Cell<(Vec<u32>, u32)> = const { std::cell::Cell::new((Vec::new(), 0)) };
}

fn cell(v: f32) -> i32 {
    (v / CELL).floor() as i32
}

/// The grid cells a box of `extent` swept from `start` to `end` can touch: for each column of cells, only the
/// rows the segment crosses within that column (widened by the box), so a long diagonal does not visit the whole
/// square its ends span.
fn cells_swept(start: Vec3, end: Vec3, extent: Vec3) -> Vec<(i32, i32)> {
    let (lo, hi) = (start.min(end) - extent, start.max(end) + extent);
    let mut out = Vec::new();
    let d = end - start;
    for x in cell(lo.x)..=cell(hi.x) {
        // The part of the segment inside this column, widened by the box in x.
        let (x0, x1) = (x as f32 * CELL - extent.x, (x + 1) as f32 * CELL + extent.x);
        let (mut t0, mut t1) = (0.0f32, 1.0f32);
        if d.x.abs() > 1e-6 {
            let (a, b) = ((x0 - start.x) / d.x, (x1 - start.x) / d.x);
            t0 = t0.max(a.min(b));
            t1 = t1.min(a.max(b));
        } else if start.x < x0 || start.x > x1 {
            continue;
        }
        if t0 > t1 {
            continue;
        }
        let (ya, yb) = (start.y + d.y * t0, start.y + d.y * t1);
        let (y_lo, y_hi) = (ya.min(yb) - extent.y, ya.max(yb) + extent.y);
        for y in cell(y_lo)..=cell(y_hi) {
            out.push((x, y));
        }
    }
    out
}

impl Collision {
    pub fn new(model: Option<std::rc::Rc<Model>>) -> Collision {
        Collision { model, ..Default::default() }
    }

    pub fn triangle_count(&self) -> usize {
        self.tris.len()
    }

    /// Registers a static mesh actor's collision model (planes already in world space). `boxes` and `lines` are
    /// the mesh's `UseSimpleBoxCollision` and `UseSimpleLineCollision`; `lo`/`hi` bound it in the world.
    pub fn add_mesh_model(&mut self, actor: ObjId, model: Model, boxes: bool, lines: bool, lo: Vec3, hi: Vec3) {
        if model.nodes.is_empty() || !(boxes || lines) {
            return;
        }
        let index = self.mesh_models.len() as u32;
        self.owners_cache = None;
        self.mesh_models.push(MeshModel { actor, model: std::rc::Rc::new(model) });
        self.modeled.insert(actor, (boxes, lines));
        for x in cell(lo.x)..=cell(hi.x) {
            for y in cell(lo.y)..=cell(hi.y) {
                self.model_grid.entry((x, y)).or_default().push(index);
            }
        }
    }

    /// Whether a trace with this extent uses `actor`'s collision model instead of its triangles: boxes when the
    /// extent is not zero, lines when it is.
    fn uses_model(&self, actor: ObjId, extent: Vec3) -> bool {
        // Read once: this runs for many triangles in every trace.
        static NO_MODELS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *NO_MODELS.get_or_init(|| crate::envc::var_os("HP3_NO_MODELS").is_some()) {
            return false;
        }
        self.modeled.get(&actor).is_some_and(|&(boxes, lines)| if extent == Vec3::ZERO { lines } else { boxes })
    }

    /// Adds a world-space triangle; degenerate ones are skipped.
    pub fn add_triangle(&mut self, v: [Vec3; 3], actor: Option<ObjId>) {
        let normal = (v[1] - v[0]).cross(v[2] - v[0]);
        if normal.length_squared() < 1e-8 {
            return;
        }
        let index = self.tris.len() as u32;
        self.owners_cache = None;
        self.tris.push(Tri { v, normal: normal.normalize(), actor });
        let min = v[0].min(v[1]).min(v[2]);
        let max = v[0].max(v[1]).max(v[2]);
        for x in cell(min.x)..=cell(max.x) {
            for y in cell(min.y)..=cell(max.y) {
                self.grid.entry((x, y)).or_default().push(index);
            }
        }
    }

    /// The first thing a box of half size `extent` hits moving from `start` to `end`, ignoring `ignore`.
    pub fn trace(&self, start: Vec3, end: Vec3, extent: Vec3, ignore: Option<ObjId>) -> Option<Hit> {
        self.trace_with(start, end, extent, ignore, true)
    }

    /// Like `trace`, for a mover that is (or is not) the player's pawn.
    pub fn trace_as(&mut self, player: bool, start: Vec3, end: Vec3, extent: Vec3, ignore: Option<ObjId>) -> Option<Hit> {
        self.player_mover = player;
        let hit = self.trace_with(start, end, extent, ignore, true);
        self.player_mover = false;
        hit
    }

    /// Like `trace_as`, for a thing that may pass through actors or through the level.
    pub fn trace_for(&mut self, how: Mover, start: Vec3, end: Vec3, extent: Vec3, ignore: Option<ObjId>) -> Option<Hit> {
        self.player_mover = how.player;
        self.passes_actors = !how.blocks_actors;
        self.passes_world = !how.collide_world;
        let hit = self.trace_with(start, end, extent, ignore, true);
        self.player_mover = false;
        self.passes_actors = false;
        self.passes_world = false;
        hit
    }

    /// The level's geometry only: BSP, meshes and terrain, not actors.
    pub fn trace_static(&self, start: Vec3, end: Vec3, extent: Vec3) -> Option<Hit> {
        self.trace_with(start, end, extent, None, false)
    }

    fn trace_with(&self, start: Vec3, end: Vec3, extent: Vec3, ignore: Option<ObjId>, actors: bool) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        let mut keep = |h: Hit| {
            if best.is_none_or(|b| h.time < b.time) {
                best = Some(h);
            }
        };
        if let Some(model) = &self.model
            && !self.passes_world
            && !model.nodes.is_empty()
        {
            let mut out = None;
            bsp_trace(model, 0, 0.0, 1.0, start, end, start, end, extent, Vec3::Z, &mut out);
            if let Some(h) = out {
                keep(h);
            }
        }
        // Triangles in the cells the swept box covers.
        // A triangle in several cells is tried once: marked with this trace's stamp in a table kept between traces.
        let mut stamps = TRI_STAMPS.take();
        if stamps.0.len() < self.tris.len() {
            stamps.0.resize(self.tris.len(), 0);
        }
        stamps.1 = stamps.1.wrapping_add(1);
        if stamps.1 == 0 {
            stamps.0.fill(0);
            stamps.1 = 1;
        }
        let stamp = stamps.1;
        let swept = cells_swept(start, end, extent);
        for key in &swept {
            let Some(list) = self.grid.get(key) else { continue };
            for &i in list {
                if self.passes_world {
                    break;
                }
                if std::mem::replace(&mut stamps.0[i as usize], stamp) == stamp {
                    continue;
                }
                let t = &self.tris[i as usize];
                if t.actor.is_some() && t.actor == ignore {
                    continue;
                }
                // The triangle's own test first: most candidates are nowhere near the move, and the per-actor lookups
                // below only ever skip a triangle that would be hit.
                let Some(h) = tri_trace(t, start, end, extent) else { continue };
                if t.actor.is_some_and(|a| self.inactive.contains(&a) || !self.blocks_mover(a) || self.uses_model(a, extent) || self.starts_in_volume(a, start)) {
                    continue;
                }
                keep(h);
            }
        }
        TRI_STAMPS.set(stamps);
        // Static meshes traced through their collision models.
        let mut seen_models = hp3_script::fx::FxHashSet::default();
        for key in &swept {
            let Some(list) = self.model_grid.get(key) else { continue };
            for &i in list {
                if self.passes_world || !seen_models.insert(i) {
                    continue;
                }
                let m = &self.mesh_models[i as usize];
                if Some(m.actor) == ignore || self.inactive.contains(&m.actor) || !self.blocks_mover(m.actor) || !self.uses_model(m.actor, extent) {
                    continue;
                }
                let mut out = None;
                bsp_trace(&m.model, 0, 0.0, 1.0, start, end, start, end, extent, Vec3::Z, &mut out);
                if let Some(h) = out {
                    keep(Hit { actor: Some(m.actor), ..h });
                }
            }
        }
        for m in &self.movers {
            if Some(m.actor) == ignore || self.passes_actors {
                continue;
            }
            if let Some(h) = m.trace(start, end, extent) {
                keep(h);
            }
        }
        for b in self.actors.iter().filter(|_| actors && !self.passes_actors) {
            if Some(b.actor) == ignore || (b.owner.is_some() && b.owner == ignore) || (self.mover_owner.is_some() && self.mover_owner == Some(b.actor)) || !(if self.player_mover { b.blocks_players } else { b.blocks_actors }) {
                continue;
            }
            if let Some(h) = box_trace(b, start, end, extent) {
                keep(h);
            }
        }
        best
    }

    /// The first blocking pawn a box meets moving from `start` to `end` (the level and everything else is ignored).
    pub fn trace_pawns(&self, start: Vec3, end: Vec3, extent: Vec3, ignore: Option<ObjId>) -> Option<Hit> {
        self.actors
            .iter()
            .filter(|b| b.is_pawn && Some(b.actor) != ignore)
            .filter_map(|b| box_trace(b, start, end, extent))
            .min_by(|a, b| a.time.total_cmp(&b.time))
    }

    /// Each mover's name id and world box (for the game log).
    pub fn mover_boxes(&self) -> Vec<(ObjId, Vec3, Vec3)> {
        self.movers.iter().map(|m| { let (lo, hi) = m.world_bounds(); (m.actor, lo, hi) }).collect()
    }

    /// The mover of `actor`, when that actor is one.
    pub fn mover(&self, actor: ObjId) -> Option<&MoverShape> {
        self.movers.iter().find(|m| m.actor == actor)
    }

    /// Marks these baked actors as volumes, whose boxes are worked out from their triangles.
    pub fn mark_volumes(&mut self, volumes: &std::collections::HashSet<ObjId>) {
        let mut bounds: HashMap<ObjId, (Vec3, Vec3)> = HashMap::default();
        for t in &self.tris {
            let Some(a) = t.actor.filter(|a| volumes.contains(a)) else { continue };
            let e = bounds.entry(a).or_insert((t.v[0], t.v[0]));
            for v in t.v {
                *e = (e.0.min(v), e.1.max(v));
            }
        }
        self.volume_bounds = bounds;
    }

    fn starts_in_volume(&self, actor: ObjId, start: Vec3) -> bool {
        self.volume_bounds.get(&actor).is_some_and(|(lo, hi)| start.cmpgt(*lo).all() && start.cmplt(*hi).all())
    }

    /// The world box of the baked triangles of `actor`.
    pub fn baked_bounds(&self, actor: ObjId) -> Option<(Vec3, Vec3)> {
        let mut it = self.tris.iter().filter(|t| t.actor == Some(actor)).flat_map(|t| t.v);
        let first = it.next()?;
        Some(it.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v))))
    }

    /// The actors that own baked triangles or models.
    pub fn baked_owners(&mut self) -> &[ObjId] {
        if self.owners_cache.is_none() {
            let mut seen = hp3_script::fx::FxHashSet::default();
            let tris = self.tris.iter().filter_map(|t| t.actor);
            self.owners_cache = Some(tris.chain(self.mesh_models.iter().map(|m| m.actor)).filter(|a| seen.insert(*a)).collect());
        }
        self.owners_cache.as_deref().unwrap_or_default()
    }

    /// Whether the baked shapes of `actor` stop the mover being traced: a volume that blocks players only lets creatures
    /// through, and one that blocks creatures only lets the player through.
    fn blocks_mover(&self, actor: ObjId) -> bool {
        self.blocks.get(&actor).is_none_or(|&(actors, players)| if self.player_mover { players } else { actors })
    }

    /// Sets which kinds of mover the baked shapes of an actor stop.
    pub fn set_blocks(&mut self, actor: ObjId, actors: bool, players: bool) {
        self.blocks.insert(actor, (actors, players));
    }

    /// Turns the baked collision of an actor off or on.
    pub fn set_active(&mut self, actor: ObjId, active: bool) {
        if active {
            self.inactive.remove(&actor);
        } else {
            self.inactive.insert(actor);
        }
    }

    /// Whether a box (center, half size) overlaps a triangle of the static meshes (not the level's BSP).
    pub fn box_overlaps_triangles(&self, center: Vec3, half: Vec3, ignore: Option<ObjId>) -> bool {
        let swept = cells_swept(center, center, half);
        let mut seen = hp3_script::fx::FxHashSet::default();
        for key in &swept {
            let Some(list) = self.grid.get(key) else { continue };
            for &i in list {
                if !seen.insert(i) {
                    continue;
                }
                let t = &self.tris[i as usize];
                if t.actor.is_some() && t.actor == ignore || t.actor.is_some_and(|a| self.inactive.contains(&a)) {
                    continue;
                }
                if tri_box_overlap(&t.v, t.normal, center, half) {
                    return true;
                }
            }
        }
        false
    }

    /// For debugging: every BSP polygon of the level model whose bounds touch the box `lo`..`hi`, as text lines (flags, brush
    /// actor, plane, bounds), to see what the level holds in a place whatever the collision does with it.
    pub fn debug_polygons_in(&self, lo: Vec3, hi: Vec3) -> Vec<String> {
        let Some(model) = &self.model else { return Vec::new() };
        let mut out = Vec::new();
        for (i, node) in model.nodes.iter().enumerate() {
            if node.surf < 0 || node.num_vertices == 0 {
                continue;
            }
            let (mut blo, mut bhi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for k in 0..node.num_vertices as i32 {
                let Some(v) = model.verts.get((node.vert_pool + k) as usize) else { continue };
                let Some(p) = model.points.get(v.vertex as usize) else { continue };
                let p = Vec3::new(p.x, p.y, p.z);
                blo = blo.min(p);
                bhi = bhi.max(p);
            }
            if blo.cmpgt(hi).any() || bhi.cmplt(lo).any() {
                continue;
            }
            let flags = model.surfs.get(node.surf as usize).map(|s| s.poly_flags).unwrap_or(0);
            let actor = model.surfs.get(node.surf as usize).map(|s| s.actor.0).unwrap_or(0);
            out.push(format!(
                "node {i} flags {flags:#010x} actor ref {actor} normal ({:.2},{:.2},{:.2}) bounds ({:.0},{:.0},{:.0})..({:.0},{:.0},{:.0})",
                node.plane.normal.x, node.plane.normal.y, node.plane.normal.z, blo.x, blo.y, blo.z, bhi.x, bhi.y, bhi.z
            ));
        }
        out
    }

    /// Whether a box at `at` overlaps solid geometry.
    pub fn encroaches(&self, at: Vec3, extent: Vec3, ignore: Option<ObjId>) -> bool {
        self.trace(at + Vec3::Z * 0.1, at, extent, ignore).is_some_and(|h| h.time <= 0.0)
    }
}

/// Signed distance of `p` from a node's plane, and the plane's normal.
fn plane_of(model: &Model, node: usize) -> (Vec3, f32) {
    let p = model.nodes[node].plane;
    (Vec3::new(p.normal.x, p.normal.y, p.normal.z), p.w)
}

fn node_passes_through(model: &Model, node: usize) -> bool {
    // HP3_NOT_SOLID_SOLID=1 (a debugging switch) lets the not-solid surfaces block too, to see what they would add.
    static NOT_SOLID_BLOCKS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let passes = if *NOT_SOLID_BLOCKS.get_or_init(|| std::env::var_os("HP3_NOT_SOLID_SOLID").is_some()) { PF_PORTAL } else { PF_NOT_SOLID | PF_PORTAL };
    let surf = model.nodes[node].surf;
    surf >= 0 && model.surfs.get(surf as usize).is_some_and(|s| s.poly_flags & passes != 0)
}

/// Whether a point (in the model's space) is inside its solid: walk the BSP to a leaf; ending behind a
/// plane with no child there is solid.
pub fn point_inside(model: &Model, p: Vec3) -> bool {
    if model.nodes.is_empty() {
        return false;
    }
    let mut node = 0usize;
    for _ in 0..model.nodes.len() {
        let (n, w) = plane_of(model, node);
        let nd = &model.nodes[node];
        let child = if n.dot(p) - w >= 0.0 { nd.front } else { nd.back };
        if child < 0 {
            return n.dot(p) - w < 0.0;
        }
        node = child as usize;
    }
    false
}

/// Quake-style recursive hull trace: each plane is pushed out by the box's support distance.
#[allow(clippy::too_many_arguments)]
fn bsp_trace(
    model: &Model,
    node: usize,
    f0: f32,
    f1: f32,
    p0: Vec3,
    p1: Vec3,
    start: Vec3,
    end: Vec3,
    extent: Vec3,
    entry_normal: Vec3,
    out: &mut Option<Hit>,
) {
    if out.is_some_and(|h| h.time <= f0) {
        return;
    }
    let (n, w) = plane_of(model, node);
    let offset = plane_offset(n, extent);
    let d0 = n.dot(p0) - w;
    let d1 = n.dot(p1) - w;
    let nd = &model.nodes[node];
    let pass = node_passes_through(model, node);
    let front = nd.front;
    let back = nd.back;
    let visit = |child: i32, is_back: bool, f0: f32, f1: f32, p0: Vec3, p1: Vec3, normal: Vec3, out: &mut Option<Hit>| {
        if child >= 0 {
            bsp_trace(model, child as usize, f0, f1, p0, p1, start, end, extent, normal, out);
        } else if is_back && !pass {
            // A solid leaf: the move stops where it entered it.
            let time = (f0 - EPSILON / (end - start).length().max(EPSILON)).max(0.0);
            if out.is_none_or(|h| time < h.time) {
                *out = Some(Hit { time, location: start + (end - start) * time, normal, actor: None });
            }
        }
    };
    if d0 >= offset && d1 >= offset {
        visit(front, false, f0, f1, p0, p1, entry_normal, out);
        return;
    }
    if d0 < -offset && d1 < -offset {
        visit(back, true, f0, f1, p0, p1, entry_normal, out);
        return;
    }
    // The segment crosses the expanded plane: near side first, then the far side.
    // The near side's segment runs until the box has fully crossed the plane (support distance past it);
    // the far side's starts where the box first touches it.
    let (near_back, frac_near, frac_far) = if d0 < d1 {
        let inv = 1.0 / (d0 - d1);
        (true, ((d0 - offset + EPSILON) * inv).clamp(0.0, 1.0), ((d0 + offset + EPSILON) * inv).clamp(0.0, 1.0))
    } else if d0 > d1 {
        let inv = 1.0 / (d0 - d1);
        (false, ((d0 + offset + EPSILON) * inv).clamp(0.0, 1.0), ((d0 - offset - EPSILON) * inv).clamp(0.0, 1.0))
    } else {
        (false, 1.0, 0.0)
    };
    let mid_f = f0 + (f1 - f0) * frac_near;
    let mid = p0 + (p1 - p0) * frac_near;
    let (near, far) = if near_back { (back, front) } else { (front, back) };
    visit(near, near_back, f0, mid_f, p0, mid, entry_normal, out);
    let mid_f = f0 + (f1 - f0) * frac_far;
    let mid = p0 + (p1 - p0) * frac_far;
    // Crossing from front to back enters the solid side through this plane.
    let normal = if near_back { -n } else { n };
    visit(far, !near_back, mid_f, f1, mid, p1, normal, out);
}

/// Swept box against one triangle, approximated as the triangle's plane pushed out by the box, limited to the
/// triangle grown by the box's horizontal radius.
/// How far the moving thing reaches toward a plane with normal `n`: it is an upright cylinder (its larger half width is
/// the radius), so a wall at an angle is no closer to it than a wall square on.
fn plane_offset(n: Vec3, extent: Vec3) -> f32 {
    (n.x * n.x + n.y * n.y).sqrt() * extent.x.max(extent.y) + n.z.abs() * extent.z
}

fn tri_trace(t: &Tri, start: Vec3, end: Vec3, extent: Vec3) -> Option<Hit> {
    let mut n = t.normal;
    let mut d0 = n.dot(start - t.v[0]);
    // Two-sided: the side the move starts on is the front.
    if d0 < 0.0 {
        n = -n;
        d0 = -d0;
    }
    let d1 = n.dot(end - t.v[0]);
    let offset = plane_offset(n, extent);
    if d1 >= offset || d0 < offset - 1.0 || d0 <= d1 {
        return None;
    }
    let time = ((d0 - offset - EPSILON) / (d0 - d1)).clamp(0.0, 1.0);
    let at = start + (end - start) * time;
    let contact = at - n * (n.dot(at - t.v[0]));
    // The box's footprint is approximated by a circle of its larger half width: the contact point must lie in
    // the triangle or within that distance of one of its edges.
    let grow = extent.x.max(extent.y);
    let inside = (0..3).all(|i| {
        let (a, b) = (t.v[i], t.v[(i + 1) % 3]);
        let edge_normal = (b - a).cross(t.normal);
        let toward_inside = edge_normal.dot(t.v[(i + 2) % 3] - a).signum();
        edge_normal.dot(contact - a) * toward_inside >= 0.0
    });
    if !inside && (0..3).all(|i| segment_distance(contact, t.v[i], t.v[(i + 1) % 3]) > grow) {
        return None;
    }
    Some(Hit { time, location: at, normal: n, actor: t.actor })
}

/// Distance from `p` to the segment `a`-`b`.
fn segment_distance(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let len2 = ab.length_squared();
    let t = if len2 > 1e-12 { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) } else { 0.0 };
    (p - (a + ab * t)).length()
}

/// A moving box swept against an actor whose collision is a box turned with it (`CollideType` box: the invisible walls and
/// ramps along stairs). Everything is done in the blocker's own frame, where it is axis aligned: the mover's box (upright
/// in the world) becomes the box around its turned corners, the blocker is grown by it, and the segment is clipped by the
/// slabs. A mover that already overlaps is only stopped when its end is deeper in than its start, so it can walk out.
fn oriented_box_trace(b: &ActorBox, axes: [Vec3; 3], start: Vec3, end: Vec3, extent: Vec3) -> Option<Hit> {
    let local = |p: Vec3| {
        let d = p - b.center;
        Vec3::new(d.dot(axes[0]), d.dot(axes[1]), d.dot(axes[2]))
    };
    // Half size of the turned mover box along each blocker axis.
    let reach = Vec3::new(
        axes[0].x.abs() * extent.x + axes[0].y.abs() * extent.y + axes[0].z.abs() * extent.z,
        axes[1].x.abs() * extent.x + axes[1].y.abs() * extent.y + axes[1].z.abs() * extent.z,
        axes[2].x.abs() * extent.x + axes[2].y.abs() * extent.y + axes[2].z.abs() * extent.z,
    );
    let grown = b.extent + reach;
    let (ls, le) = (local(start), local(end));
    let d = le - ls;
    let (mut t_enter, mut t_exit) = (f32::MIN, 1.0f32);
    let mut normal_local = Vec3::ZERO;
    for i in 0..3 {
        let (s, v) = (ls[i], d[i]);
        if v.abs() < 1e-9 {
            if s.abs() >= grown[i] {
                return None;
            }
            continue;
        }
        let (mut t0, mut t1) = ((-grown[i] - s) / v, (grown[i] - s) / v);
        if t0 > t1 {
            std::mem::swap(&mut t0, &mut t1);
        }
        if t0 > t_enter {
            t_enter = t0;
            normal_local = Vec3::ZERO;
            normal_local[i] = -v.signum();
        }
        t_exit = t_exit.min(t1);
    }
    let to_world = |n: Vec3| (axes[0] * n.x + axes[1] * n.y + axes[2] * n.z).normalize_or_zero();
    // How far inside the grown box a point is (negative: outside).
    let depth = |p: Vec3| (0..3).map(|i| grown[i] - p[i].abs()).fold(f32::MAX, f32::min);
    if t_enter < 0.0 {
        // The start is inside the grown box.
        if depth(ls) <= 0.0 || depth(le) <= depth(ls) {
            return None;
        }
        // Blocked on the spot, pushed back out the way that is shallowest.
        let axis = (0..3).min_by(|&i, &j| (grown[i] - ls[i].abs()).total_cmp(&(grown[j] - ls[j].abs()))).unwrap_or(0);
        let mut n = Vec3::ZERO;
        n[axis] = if ls[axis] >= 0.0 { 1.0 } else { -1.0 };
        return Some(Hit { time: 0.0, location: start, normal: to_world(n), actor: Some(b.actor) });
    }
    if t_enter > t_exit || t_enter > 1.0 || normal_local == Vec3::ZERO {
        return None;
    }
    let len = (end - start).length().max(EPSILON);
    let time = (t_enter - EPSILON / len).max(0.0);
    Some(Hit { time, location: start + (end - start) * time, normal: to_world(normal_local), actor: Some(b.actor) })
}

/// Swept cylinder against an actor's cylinder: an actor collides as an upright cylinder (radius, half height), so the
/// moving thing's circle is added to the actor's in the plane and its half height to the actor's in height.
fn box_trace(b: &ActorBox, start: Vec3, end: Vec3, extent: Vec3) -> Option<Hit> {
    if let Some(axes) = b.axes {
        return oriented_box_trace(b, axes, start, end, extent);
    }
    let radius = b.extent.x.max(b.extent.y) + extent.x.max(extent.y);
    let half_height = b.extent.z + extent.z;
    let dir = end - start;
    let mut t_enter = 0.0f32;
    let mut t_exit = 1.0f32;
    let mut normal = Vec3::ZERO;
    // Height: the same slab test as a box's.
    let (s, d) = (start.z - b.center.z, dir.z);
    if d.abs() < 1e-6 {
        if s <= -half_height || s >= half_height {
            return None;
        }
    } else {
        let (mut t0, mut t1) = ((-half_height - s) / d, (half_height - s) / d);
        if t0 > t1 {
            std::mem::swap(&mut t0, &mut t1);
        }
        if t0 > t_enter {
            t_enter = t0;
            normal = Vec3::new(0.0, 0.0, -d.signum());
        }
        t_exit = t_exit.min(t1);
    }
    // Plane: the segment against the circle.
    let p = Vec3::new(start.x - b.center.x, start.y - b.center.y, 0.0);
    let v = Vec3::new(dir.x, dir.y, 0.0);
    let a = v.dot(v);
    let c = p.dot(p) - radius * radius;
    if a < 1e-9 {
        if c >= 0.0 {
            return None;
        }
    } else {
        let half_b = p.dot(v);
        let disc = half_b * half_b - a * c;
        if disc <= 0.0 {
            return None;
        }
        let root = disc.sqrt();
        let (t0, t1) = ((-half_b - root) / a, (-half_b + root) / a);
        if t0 > t_enter {
            t_enter = t0;
            normal = (p + v * t0).normalize_or_zero();
        }
        t_exit = t_exit.min(t1);
    }
    if t_enter > t_exit || normal == Vec3::ZERO || t_enter > 1.0 {
        return None;
    }
    let time = (t_enter - EPSILON / dir.length().max(EPSILON)).max(0.0);
    Some(Hit { time, location: start + dir * time, normal, actor: Some(b.actor) })
}

/// Whether a triangle and a box (center, half size) overlap: the separating axis test.
fn tri_box_overlap(v: &[Vec3; 3], normal: Vec3, center: Vec3, half: Vec3) -> bool {
    let p = [v[0] - center, v[1] - center, v[2] - center];
    // The box's own axes.
    for axis in 0..3 {
        let (lo, hi) = (p[0][axis].min(p[1][axis]).min(p[2][axis]), p[0][axis].max(p[1][axis]).max(p[2][axis]));
        if lo > half[axis] || hi < -half[axis] {
            return false;
        }
    }
    // The triangle's plane.
    let reach = half.x * normal.x.abs() + half.y * normal.y.abs() + half.z * normal.z.abs();
    if normal.dot(p[0]).abs() > reach {
        return false;
    }
    // Edge crossed with each box axis.
    let edges = [p[1] - p[0], p[2] - p[1], p[0] - p[2]];
    for e in edges {
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            let a = e.cross(axis);
            if a.length_squared() < 1e-9 {
                continue;
            }
            let (d0, d1, d2) = (a.dot(p[0]), a.dot(p[1]), a.dot(p[2]));
            let r = half.x * a.x.abs() + half.y * a.y.abs() + half.z * a.z.abs();
            if d0.min(d1).min(d2) > r || d0.max(d1).max(d2) < -r {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod oriented_box_tests {
    use super::*;

    fn wall(extent: Vec3, yaw_deg: f32) -> ActorBox {
        let (s, c) = yaw_deg.to_radians().sin_cos();
        ActorBox {
            actor: ObjId(1),
            center: Vec3::ZERO,
            extent,
            blocks_actors: true,
            blocks_players: true,
            is_pawn: false,
            owner: None,
            axes: Some([Vec3::new(c, s, 0.0), Vec3::new(-s, c, 0.0), Vec3::Z]),
        }
    }

    /// A thin wall (half sizes 100 x 5 x 50) along x is stopped by from the side, and a mover running along its length is not.
    #[test]
    fn thin_wall_blocks_across_but_not_along() {
        let b = wall(Vec3::new(100.0, 5.0, 50.0), 0.0);
        let pawn = Vec3::new(10.0, 10.0, 20.0);
        let hit = box_trace(&b, Vec3::new(0.0, -200.0, 0.0), Vec3::new(0.0, 200.0, 0.0), pawn).expect("blocked across the wall");
        assert!(hit.normal.y < -0.99, "stopped on the near face: {:?}", hit.normal);
        assert!(hit.time > 0.4 && hit.time < 0.5, "stops about 15 + 5 short of the wall: {}", hit.time);
        assert!(box_trace(&b, Vec3::new(-300.0, -100.0, 0.0), Vec3::new(300.0, -100.0, 0.0), pawn).is_none(), "passes alongside it");
    }

    /// Turned 45 degrees, the same wall stops a mover going along x, with a slanted face normal.
    #[test]
    fn turned_wall_has_a_slanted_face() {
        let b = wall(Vec3::new(100.0, 5.0, 50.0), 45.0);
        let hit = box_trace(&b, Vec3::new(-300.0, 0.0, 0.0), Vec3::new(300.0, 0.0, 0.0), Vec3::new(10.0, 10.0, 20.0)).expect("blocked");
        assert!((hit.normal.x.abs() - 0.7071).abs() < 0.01 && (hit.normal.y.abs() - 0.7071).abs() < 0.01, "slanted normal {:?}", hit.normal);
    }

    /// A mover already inside can walk out, not deeper.
    #[test]
    fn mover_inside_walks_out() {
        let b = wall(Vec3::new(100.0, 5.0, 50.0), 0.0);
        let pawn = Vec3::new(10.0, 10.0, 20.0);
        assert!(box_trace(&b, Vec3::new(0.0, 8.0, 0.0), Vec3::new(0.0, 40.0, 0.0), pawn).is_none());
        assert!(box_trace(&b, Vec3::new(0.0, 8.0, 0.0), Vec3::new(0.0, 0.0, 0.0), pawn).is_some());
    }
}
