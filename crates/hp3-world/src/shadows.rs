//! Blob shadows under characters. The original gives each pawn a `ShadowProjector` that decals a soft dark disc onto the floor
//! under it. The projector scripts are not run here (their tick is skipped); this reads the projector's `ShadowActor` and
//! `bShadowActive` instead, traces down to the floor, and hands the renderer one disc per shadow.

use glam::Vec3;
use hp3_script::Vm;

use crate::physics::vec;
use crate::{get, world};

/// How far below an actor the floor is looked for (the original's `SetupDynamics` traces 8192 down).
const TRACE_DEPTH: f32 = 8192.0;
/// A shadow is not drawn farther from the camera than the projector's default `CullDistance`.
const CULL_DISTANCE: f32 = 1200.0;
/// The disc is this much wider than the actor's collision radius.
const RADIUS_SCALE: f32 = 1.7;
/// The disc floats this far off the floor, against depth-buffer fighting.
const LIFT: f32 = 0.6;
/// Higher than this above the floor the shadow has faded out entirely.
const FADE_HEIGHT: f32 = 160.0;
/// How strongly the decal darkens the floor directly under an actor standing on it (the texture is a modulate map: grey 0.5 changes nothing).
const BASE_ALPHA: f32 = 0.75;

/// One disc on the floor.
#[derive(Clone, Copy, Debug)]
pub struct Blob {
    pub center: Vec3,
    /// The floor's upward normal.
    pub normal: Vec3,
    pub radius: f32,
    pub alpha: f32,
}

/// The frame's shadows, seen from `camera`.
pub fn collect(vm: &mut Vm<'_>, camera: Vec3) -> Vec<Blob> {
    let Some(projector) = vm.db.find("ShadowProjector") else { return Vec::new() };
    let actors = world(vm).actors.clone();
    let mut out = Vec::new();
    let debug = crate::envc::var("HP3_SHADOW_DEBUG").is_ok() && world(vm).frame % 120 == 0;
    let mut seen = 0;
    // Which actors are projectors at all is worked out again only for actors written since the last frame.
    let mut memo = std::mem::take(&mut world(vm).memo_projectors);
    let pass = memo.begin(&mut vm.db.objects);
    for p in actors {
        if memo.get(&pass, &vm.db.objects, p) == Some(1) {
            continue;
        }
        let o = vm.db.objects.get(p);
        if o.deleted {
            memo.set(p, 1);
            continue;
        }
        let Some(class) = o.class else { continue };
        if !vm.is_child_of(class, projector) {
            memo.set(p, 1);
            continue;
        }
        seen += 1;
        if debug {
            let (act, hid) = (get(vm, p, "bShadowActive").as_bool(), get(vm, p, "bHidden").as_bool());
            let target = get(vm, p, "ShadowActor").as_obj();
            eprintln!("shadow: projector {} active {act} hidden {hid} actor {:?}", hp3_script::natives::object_path(vm, p), target.map(|t| hp3_script::natives::object_path(vm, t)));
        }
        if !get(vm, p, "bShadowActive").as_bool() {
            continue;
        }
        let Some(a) = get(vm, p, "ShadowActor").as_obj() else { continue };
        if vm.db.objects.get(a).deleted || get(vm, a, "bHidden").as_bool() {
            continue;
        }
        let loc = vec(&get(vm, a, "Location"));
        let cull = match get(vm, p, "CullDistance").as_float() {
            d if d > 0.0 => d,
            _ => CULL_DISTANCE,
        };
        let distance = loc.distance(camera);
        if distance > cull {
            continue;
        }
        let radius = get(vm, a, "CollisionRadius").as_float();
        let height = get(vm, a, "CollisionHeight").as_float();
        if radius <= 0.0 {
            continue;
        }
        // A thin box from the actor's middle straight down finds the floor under it, not the edge it may be standing beside.
        let traced = world(vm).collision.trace_static(loc, loc - Vec3::Z * TRACE_DEPTH, Vec3::new(2.0, 2.0, 1.0));
        if debug {
            eprintln!("shadow: trace for actor at {loc} r {radius} h {height} dist {distance} -> {:?}", traced.map(|h| (h.time, h.location, h.normal)));
        }
        let Some(hit) = traced else { continue };
        if hit.normal.z < 0.5 {
            continue;
        }
        // The floor's top is where the thin box's bottom stops; the actor's feet are `height` below its middle.
        let floor = hit.location;
        let above = (loc.z - height - floor.z).max(0.0);
        let near = (1.0 - above / FADE_HEIGHT).clamp(0.0, 1.0);
        let far = (1.0 - (distance / cull).powi(2)).clamp(0.0, 1.0);
        let alpha = BASE_ALPHA * near * far;
        if alpha < 0.01 {
            continue;
        }
        if debug {
            eprintln!("shadow: blob actor at {loc} floor {floor} normal {} radius {radius} alpha {alpha} dist {distance}", hit.normal);
        }
        out.push(Blob { center: floor + hit.normal * LIFT, normal: hit.normal, radius: radius * RADIUS_SCALE * (1.0 + 0.35 * (1.0 - near)), alpha });
    }
    world(vm).memo_projectors = memo;
    if debug {
        let (frame, total) = (world(vm).frame, world(vm).actors.len());
        eprintln!("shadow: frame {frame} projectors {seen} blobs {} (actors {total})", out.len());
    }
    out
}
