//! Distance fog as the original's renderer picks it each frame: a physics volume with fog at the view point wins; otherwise
//! the fog of the zone the view is in, blended from the last zone's over its `DistanceFogBlendTime`.

use std::collections::HashMap;

use glam::Vec3;
use hp3_render::scene::Fog;
use hp3_script::{ObjId, Value, Vm};
use hp3_world::{get, world};


/// The far clipping plane when there is no fog (the original's `FAR_CLIPPING_PLANE`).
const FAR: f32 = 65536.0;

/// What the fog blend remembers from frame to frame.
pub struct FogState {
    last_zone: Option<usize>,
    since: f32,
    start: f32,
    end: f32,
    color: Vec3,
    /// Zone actors by their export index in the map.
    zone_actors: HashMap<usize, Option<ObjId>>,
}

impl Default for FogState {
    fn default() -> FogState {
        FogState { last_zone: None, since: 1.0e6, start: FAR, end: FAR, color: Vec3::ZERO, zone_actors: HashMap::new() }
    }
}

/// A `Color` struct value as 0..1 channels.
fn color_of(v: &Value) -> Vec3 {
    let f = v.fields();
    // The script struct lists its bytes as B, G, R, A.
    let c = |i: usize| f.get(i).map_or(0.0, |x| x.as_byte() as f32 / 255.0);
    Vec3::new(c(2), c(1), c(0))
}

struct ZoneFog {
    on: bool,
    start: f32,
    end: f32,
    color: Vec3,
    blend_time: f32,
    clear: bool,
}

fn read_fog(vm: &mut Vm<'_>, a: ObjId) -> ZoneFog {
    ZoneFog {
        on: get(vm, a, "bDistanceFog").as_bool(),
        start: get(vm, a, "DistanceFogStart").as_float(),
        end: get(vm, a, "DistanceFogEnd").as_float(),
        color: color_of(&get(vm, a, "DistanceFogColor")),
        blend_time: get(vm, a, "DistanceFogBlendTime").as_float(),
        clear: get(vm, a, "bClearToFogColor").as_bool(),
    }
}

/// The fog at the view point for this frame (`dt` seconds since the last), `None` when there is none.
pub fn view_fog(vm: &mut Vm<'_>, state: &mut FogState, map_name: &str, at: Vec3, dt: f32) -> Option<Fog> {
    if let Some(v) = hp3_world::physics_volume_holding(vm, at) {
        let f = read_fog(vm, v);
        if f.on {
            return Some(Fog { color: f.color, start: f.start, end: f.end.max(f.start + 1.0), clear: f.clear });
        }
    }
    let zone = world(vm).collision.zone_at(at)?;
    let export = world(vm).collision.zone_actor_export(zone);
    let actor = match export {
        Some(e) => *state.zone_actors.entry(e).or_insert_with(|| {
            let name = map_name.to_ascii_lowercase();
            world(vm).actors.clone().into_iter().find(|&a| vm.db.objects.get(a).key.as_ref().is_some_and(|(p, i)| p.as_ref() == name && *i == e))
        }),
        None => None,
    }?;
    let zf = read_fog(vm, actor);
    state.since += dt;
    if state.last_zone != Some(zone) {
        // Coming out of a zone without fog: the blend starts from this zone's color.
        if zf.on && state.start == FAR {
            state.color = zf.color;
        }
        // No blend out of zone 0.
        state.since = if state.last_zone == Some(0) { 1.0e6 } else { 0.0 };
        state.last_zone = Some(zone);
    }
    let mut alpha = if zf.blend_time > 0.0 { state.since / zf.blend_time } else { 1.0 };
    let (w_start, w_end, w_color) = if zf.on { (zf.start, zf.end, zf.color) } else { (FAR, FAR, state.color) };
    if alpha > 1.0 || zone == 0 {
        state.start = w_start;
        state.end = w_end;
        state.color = w_color;
        state.since = 1.0e6;
        alpha = 1.0;
    }
    let lerp = |a: f32, b: f32| a + (b - a) * alpha;
    let (start, end) = (lerp(state.start, w_start), lerp(state.end, w_end));
    let color = state.color.lerp(w_color, alpha);
    (zf.on || alpha < 1.0).then_some(Fog { color, start, end: end.max(start + 1.0), clear: zf.clear })
}

/// The fog a still view at `at` has (no blending), for the tools.
pub fn still_fog(vm: &mut Vm<'_>, map_name: &str, at: Vec3) -> Option<Fog> {
    let mut state = FogState::default();
    // The first look starts a blend from no fog; the second, long after, has finished it.
    view_fog(vm, &mut state, map_name, at, 0.0);
    view_fog(vm, &mut state, map_name, at, 100.0)
}


/// Prints the zone the point is in and its fog values (HP3_FOG_DEBUG).
pub fn debug_zone(vm: &mut Vm<'_>, map_name: &str, at: Vec3) {
    let Some(zone) = world(vm).collision.zone_at(at) else { return eprintln!("fog: no zone at {at}") };
    let export = world(vm).collision.zone_actor_export(zone);
    let name = map_name.to_ascii_lowercase();
    let actor = world(vm).actors.clone().into_iter().find(|&a| vm.db.objects.get(a).key.as_ref().is_some_and(|(p, i)| p.as_ref() == name && Some(*i) == export));
    match actor {
        Some(a) => {
            let f = read_fog(vm, a);
            let raw = get(vm, a, "DistanceFogColor");
            eprintln!("fog: zone {zone} actor {} on {} start {} end {} color {} (raw {raw:?}) blend {} clear {}", hp3_script::natives::object_path(vm, a), f.on, f.start, f.end, f.color, f.blend_time, f.clear);
        }
        None => eprintln!("fog: zone {zone} export {export:?} has no actor"),
    }
}
