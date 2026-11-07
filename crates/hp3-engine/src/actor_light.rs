//! Which lights shine on an actor and how they are handed to the shader: chosen and fitted the way the original's renderer
//! does it (`GetRelevantLights` and the Direct3D light setup), so characters and props are shaded like the original's.
//!
//! This runs for every drawn actor every frame, so it reads properties through slots found once, keeps the world's lights
//! in a list refreshed a few times a second (moving lights are followed every frame) and spreads the line-of-sight traces
//! out over frames.

use std::collections::HashMap;
use std::sync::OnceLock;

use glam::Vec3;
use hp3_render::dynamic::{ActorLight, ShaderLight};
use hp3_render::gpu::{MovingLight, MAX_MOVING_LIGHTS};
use hp3_render::scene::{rotation_matrix, LightSource, Lighting, LE_QUADRATIC_NON_INCIDENCE, LE_SPOTLIGHT, LE_STATIC_SPOT};
use hp3_script::{ObjId, Value, Vm};
use hp3_world::{get, read, world};

use crate::{light_color, vec3, LE_SUNLIGHT};

/// A light of the running world as the actor light selector sees it.
pub(crate) struct LiveLight {
    id: ObjId,
    source: LightSource,
    /// The light's color before its `LightType` wave scales it.
    base_color: Vec3,
    kind: u8,
    period: f32,
    phase: f32,
    /// `LightBrightness` (0..255): the weight of the light when choosing which reach an actor.
    brightness: f32,
    is_static: bool,
    /// `bDynamicLight`: never baked into the level's lighting; the original adds it over the surfaces it reaches each frame.
    dynamic: bool,
}

/// Slots of the actor properties used here, found once.
#[derive(Clone, Copy)]
struct Slots {
    light_type: Option<usize>,
    brightness: Option<usize>,
    is_static: Option<usize>,
    location: Option<usize>,
    unlit: Option<usize>,
    glow: Option<usize>,
    dramatic: Option<usize>,
    max_lights: Option<usize>,
    sees: Option<usize>,
    coll_radius: Option<usize>,
    coll_height: Option<usize>,
}

/// What the actor lighting remembers between frames.
#[derive(Default)]
pub struct LightCache {
    slots: Option<Slots>,
    lights: Vec<LiveLight>,
    /// The tick the light list was last rebuilt on.
    built: Option<u64>,
    /// The fixed lights by the ground squares their reach covers (indices into `lights`), rebuilt with the list.
    grid: HashMap<(i32, i32), Vec<u32>>,
    /// Lights looked at for every actor: suns and lights that move.
    always: Vec<u32>,
    /// The zone ambient light of each actor and the place it was worked out for (it is worked out again when the actor moves).
    zone_ambient: HashMap<ObjId, ([u32; 3], Vec3)>,
    /// What each actor last saw of each light: (visible now, visible before, when it looked).
    vis: HashMap<(ObjId, ObjId), (bool, bool, f32)>,
    /// Line-of-sight traces left to spend this frame.
    budget: u32,
}

/// The seconds of the running level.
pub(crate) fn level_time(vm: &mut Vm<'_>) -> f32 {
    match world(vm).level_info {
        Some(li) => get(vm, li, "TimeSeconds").as_float(),
        None => 0.0,
    }
}

/// How bright a light is right now for its `LightType` (the original's `FDynamicLight::Update`).
fn light_intensity(kind: u8, time: f32, period: f32, phase: f32) -> f32 {
    let wave = |t: f32| (std::f32::consts::TAU * (t * 35.0 / period.max(1.0) + phase / 256.0)).sin();
    match kind {
        1 | 5 => 1.0,
        2 => 0.6 + 0.39 * wave(time),
        3 => {
            if (((time * 35.0 / (period + 1.0)) + phase) as i64) & 1 == 1 { 0.0 } else { 1.0 }
        }
        // The original draws a flicker afresh only when the actor changes: its middle is used.
        4 => 0.75,
        7 => 0.9 + 0.09 * wave(time),
        10 => 1.0,
        _ => 0.0,
    }
}

impl LightCache {
    fn slots(&mut self, vm: &mut Vm<'_>) -> Slots {
        *self.slots.get_or_insert_with(|| Slots {
            light_type: hp3_world::actor_slot(vm, "LightType"),
            brightness: hp3_world::actor_slot(vm, "LightBrightness"),
            is_static: hp3_world::actor_slot(vm, "bStatic"),
            location: hp3_world::actor_slot(vm, "Location"),
            unlit: hp3_world::actor_slot(vm, "bUnlit"),
            glow: hp3_world::actor_slot(vm, "AmbientGlow"),
            dramatic: hp3_world::actor_slot(vm, "bDramaticLighting"),
            max_lights: hp3_world::actor_slot(vm, "MaxLights"),
            sees: hp3_world::actor_slot(vm, "bLightingVisibility"),
            coll_radius: hp3_world::actor_slot(vm, "CollisionRadius"),
            coll_height: hp3_world::actor_slot(vm, "CollisionHeight"),
        })
    }

    /// Starts a frame: the light list is rebuilt every few ticks (lights switch on and off by script) and the lights that
    /// move are followed every frame.
    pub(crate) fn begin_frame(&mut self, vm: &mut Vm<'_>) {
        let s = self.slots(vm);
        let tick = world(vm).frame;
        self.budget = 12;
        if self.built.is_none_or(|b| tick < b || tick - b >= 5) {
            self.built = Some(tick);
            self.lights = rebuild(vm, &s);
            self.grid.clear();
            self.always.clear();
            for (i, l) in self.lights.iter().enumerate() {
                if !l.is_static || l.source.direction.is_some() {
                    self.always.push(i as u32);
                    continue;
                }
                let (p, r) = (l.source.position, l.source.radius);
                for x in grid_cell(p.x - r)..=grid_cell(p.x + r) {
                    for y in grid_cell(p.y - r)..=grid_cell(p.y + r) {
                        self.grid.entry((x, y)).or_default().push(i as u32);
                    }
                }
            }
        } else {
            for l in &mut self.lights {
                if !l.is_static && !vm.db.objects.get(l.id).deleted && l.source.direction.is_none() {
                    l.source.position = hp3_world::read_vec(vm, l.id, s.location);
                }
            }
        }
    }

    /// The dynamic lights (`bDynamicLight`, a spell's or a wand's glow) the level's surfaces take this frame: those reaching
    /// nearest the eye, with their color as their `LightType` makes it now.
    pub(crate) fn moving_lights(&self, vm: &Vm<'_>, eye: Vec3, time: f32) -> Vec<MovingLight> {
        let mut out: Vec<(f32, MovingLight)> = self
            .lights
            .iter()
            .filter(|l| l.dynamic && l.source.direction.is_none() && !vm.db.objects.get(l.id).deleted)
            .filter_map(|l| {
                let color = l.base_color * light_intensity(l.kind, time, l.period, l.phase);
                (color.max_element() > 0.0).then(|| {
                    let reach = (l.source.position.distance(eye) - l.source.radius).max(0.0);
                    let quadratic = l.source.effect == LE_QUADRATIC_NON_INCIDENCE;
                    (reach, MovingLight { position: l.source.position, radius: l.source.radius, color, quadratic })
                })
            })
            .collect();
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        out.into_iter().take(MAX_MOVING_LIGHTS).map(|(_, l)| l).collect()
    }

    /// Where the actor's picture sphere is: its radius for lighting.
    pub(crate) fn sphere_of(&mut self, vm: &mut Vm<'_>, a: ObjId) -> f32 {
        let s = self.slots(vm);
        read(vm, a, s.coll_radius).as_float().max(read(vm, a, s.coll_height).as_float()).max(16.0) * 1.1
    }
}

/// Every lit actor of the world that can light something.
fn rebuild(vm: &mut Vm<'_>, s: &Slots) -> Vec<LiveLight> {
    let mut out = Vec::new();
    let level_scale = crate::level_brightness(vm);
    for a in world(vm).actors.clone() {
        if vm.db.objects.get(a).deleted {
            continue;
        }
        let kind = read(vm, a, s.light_type).as_byte();
        if kind == 0 {
            continue;
        }
        let brightness = read(vm, a, s.brightness).as_float();
        if brightness <= 0.0 {
            continue;
        }
        let period = get(vm, a, "LightPeriod").as_float();
        let phase = get(vm, a, "LightPhase").as_float();
        let base_color = light_color(get(vm, a, "LightHue").as_byte(), get(vm, a, "LightSaturation").as_byte()) * (brightness / 255.0) * level_scale;
        let position = hp3_world::read_vec(vm, a, s.location);
        let radius = 25.0 * (get(vm, a, "LightRadius").as_float() + 1.0);
        let effect = get(vm, a, "LightEffect").as_byte();
        let r: Vec<i32> = get(vm, a, "Rotation").fields().iter().map(Value::as_int).collect();
        let aim = rotation_matrix([r.first().copied().unwrap_or(0), r.get(1).copied().unwrap_or(0), 0]).transform_vector3(Vec3::X);
        let direction = (effect == LE_SUNLIGHT).then_some(aim);
        let cone = get(vm, a, "LightCone").as_float();
        out.push(LiveLight {
            id: a,
            source: LightSource { position, color: base_color, radius, direction, effect, cone, aim },
            base_color,
            kind,
            period,
            phase,
            brightness,
            is_static: read(vm, a, s.is_static).as_bool(),
            dynamic: get(vm, a, "bDynamicLight").as_bool(),
        });
    }
    let _ = vec3;
    out
}

const LIGHT_UPDATE_TIME: f32 = 0.35;

/// The factors the level's baked lighting is scaled by (ambient, lights), the same as for the static meshes and the BSP, so
/// characters match the surfaces they stand on. `HP3_MESH_AMB_MUL` and `HP3_MESH_MUL` change them.
fn light_multipliers() -> (f32, f32) {
    static M: OnceLock<(f32, f32)> = OnceLock::new();
    *M.get_or_init(|| {
        let get = |n: &str, d: f32| std::env::var(n).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
        (get("HP3_MESH_AMB_MUL", 0.25), get("HP3_MESH_MUL", 0.6))
    })
}

/// The ambient light an actor stands in: the strongest, per channel, of the zones its bounding sphere reaches into (the
/// original takes it over the zones its box touches), or the map's average where no zone is found.
fn zone_ambient_of(vm: &mut Vm<'_>, cache: &mut HashMap<ObjId, ([u32; 3], Vec3)>, lighting: &Lighting, actor: ObjId, centre: Vec3, sphere: f32) -> Vec3 {
    let key = centre.to_array().map(f32::to_bits);
    if let Some((at, ambient)) = cache.get(&actor)
        && *at == key
    {
        return *ambient;
    }
    let mut best: Option<Vec3> = None;
    for d in [Vec3::ZERO, Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z] {
        let collision = &world(vm).collision;
        let Some(zone) = collision.zone_at(centre + d * sphere) else { continue };
        let Some(ambient) = collision.zone_actor_export(zone).and_then(|e| lighting.zone_ambient.get(&e)) else { continue };
        best = Some(best.map_or(*ambient, |b| b.max(*ambient)));
    }
    let ambient = best.unwrap_or(lighting.ambient);
    cache.insert(actor, (key, ambient));
    ambient
}

/// Side of a square of the light grid (world units).
const GRID_CELL: f32 = 512.0;

fn grid_cell(v: f32) -> i32 {
    (v / GRID_CELL).floor() as i32
}

/// Direct3D's attenuation fitted to Unreal's curve over the distances an actor's bounding sphere spans, as the original's
/// renderer sets it (it can only use factors of a falloff 1 / (a0 + a1 d + a2 d^2)). Returns the factors and the range.
fn fitted_attenuation(light: &LightSource, centre_distance: f32, sphere: f32) -> ([f32; 3], f32) {
    let r = light.radius;
    if light.effect == LE_QUADRATIC_NON_INCIDENCE {
        return ([0.0, 0.0, 8.0 / (r * r)], r);
    }
    let unreal = |d: f32| {
        if d <= r {
            let a = d / r;
            2.0 * (2.0 * a * a * a - 3.0 * a * a + 1.0)
        } else {
            0.0
        }
    };
    let centre = centre_distance.max(0.1);
    let max_d = (centre + sphere).clamp(r * 0.05, r * 0.9);
    let min_d = (centre - sphere).clamp(r * 0.1, r * 0.95);
    let (min_a, max_a) = (1.0 / unreal(min_d), 1.0 / unreal(max_d));
    let (a0, a1) = if (min_a - max_a).abs() < 1e-6 {
        (min_a, 0.0)
    } else {
        let a0 = (min_a - (max_a - min_a) / (max_d - min_d) * min_d).max(0.01);
        (a0, ((min_a - a0) / min_d).max(0.0))
    };
    let range = ((256.0 - a0) / a1.max(0.01)).clamp(0.0, 1.0e18);
    ([a0.max(0.0), a1, 0.0], range)
}

/// What lights an actor: the lights that reach its bounding sphere strongest (eight, four for dramatically lit actors) that
/// it can see, each faded in and out over a third of a second and set up the way Direct3D wants them, plus ambient light.
pub(crate) fn actor_light_for(
    vm: &mut Vm<'_>,
    lc: &mut LightCache,
    lighting: &Lighting,
    actor: ObjId,
    centre: Vec3,
    sphere: f32,
    time: f32,
) -> ActorLight {
    let s = lc.slots(vm);
    let unlit = read(vm, actor, s.unlit).as_bool();
    let glow = match read(vm, actor, s.glow).as_byte() {
        255 => 0.25 + 0.2 * (8.0 * time).sin(),
        g => g as f32 / 255.0,
    };
    let (amb_mul, light_mul) = light_multipliers();
    let ambient = if unlit { Vec3::ONE } else { zone_ambient_of(vm, &mut lc.zone_ambient, lighting, actor, centre, sphere) * amb_mul + Vec3::splat(glow) };
    // HP3_ACTOR_LIGHTS=0: ambient light only (to measure what the lights cost).
    static NO_LIGHTS: OnceLock<bool> = OnceLock::new();
    if unlit || *NO_LIGHTS.get_or_init(|| std::env::var("HP3_ACTOR_LIGHTS").is_ok_and(|v| v == "0")) {
        return ActorLight { ambient, lights: Vec::new(), fade: 0.0 };
    }
    let dramatic = read(vm, actor, s.dramatic).as_bool();
    let max_lights = {
        let m = read(vm, actor, s.max_lights).as_int();
        let cap = if dramatic { 4 } else { 8 };
        if m > 0 { cap.min(m as usize) } else { cap }
    };
    let sees = read(vm, actor, s.sees).as_bool();
    let LightCache { lights, vis, budget, grid, always, .. } = lc;
    // Only the lights whose reach covers a square the actor's sphere touches can light it.
    let mut near: Vec<u32> = always.clone();
    for x in grid_cell(centre.x - sphere)..=grid_cell(centre.x + sphere) {
        for y in grid_cell(centre.y - sphere)..=grid_cell(centre.y + sphere) {
            if let Some(list) = grid.get(&(x, y)) {
                near.extend_from_slice(list);
            }
        }
    }
    near.sort_unstable();
    near.dedup();
    // Candidates, strongest first (the original's sort key).
    let mut ranked: Vec<(f32, &LiveLight, f32)> = Vec::new();
    for l in near.iter().map(|&i| &lights[i as usize]) {
        let src = &l.source;
        let intensity = light_intensity(l.kind, time, l.period, l.phase);
        if intensity <= 0.0 {
            continue;
        }
        let key = if src.direction.is_some() {
            f32::MAX
        } else {
            let d2 = (src.position - centre).length_squared();
            let reach = src.radius + sphere;
            if d2 >= reach * reach {
                continue;
            }
            let k = (1.0 - d2 / (reach * reach)) * l.brightness * 1024.0;
            if src.effect == LE_STATIC_SPOT || src.effect == LE_SPOTLIGHT {
                let to = (centre - src.position).normalize_or_zero();
                let cone = (1.0 - src.cone / 256.0).powi(2);
                let dot = to.dot(src.aim);
                if dot > 0.0 && dot > cone { k } else { 0.0 }
            } else {
                k
            }
        };
        if key > 0.0 {
            ranked.push((key, l, intensity));
        }
    }
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut out = Vec::new();
    for (_, l, intensity) in ranked.into_iter().take(16) {
        if out.len() >= max_lights {
            break;
        }
        // Whether the actor can see the light, looked at again every third of a second (a few traces a frame at most,
        // and a light not yet looked at counts as seen); a change fades over that time.
        let known = vis.get(&(actor, l.id)).copied();
        let due = known.is_none_or(|(_, _, traced)| traced <= time - LIGHT_UPDATE_TIME || traced > time);
        if due && sees && l.is_static && *budget > 0 {
            *budget -= 1;
            let end = match l.source.direction {
                Some(d) => centre - d * 65536.0,
                None => l.source.position,
            };
            // The actor's own collision never blocks its lights: a hit on it (a block lit from inside its bounds) is passed through.
            let mut from = centre;
            let mut visible = true;
            for _ in 0..4 {
                match world(vm).collision.trace_static(from, end, Vec3::ZERO) {
                    None => break,
                    Some(h) if h.actor == Some(actor) => from = h.location + (end - from).normalize_or_zero() * 1.0,
                    Some(_) => {
                        visible = false;
                        break;
                    }
                }
            }
            let last = known.filter(|&(_, _, traced)| traced <= time).map_or(visible, |(v, _, _)| v);
            vis.insert((actor, l.id), (visible, last, time));
        } else if known.is_none() {
            vis.insert((actor, l.id), (true, true, time));
        }
        let (visible, last, traced) = vis[&(actor, l.id)];
        let (from, to) = (if last { 1.0f32 } else { 0.0 }, if visible { 1.0f32 } else { 0.0 });
        let alpha = from + (to - from) * ((time - traced) / LIGHT_UPDATE_TIME).clamp(0.0, 1.0);
        if !(visible || last) || alpha <= 0.0 {
            continue;
        }
        // Dramatically lit actors get the strongest lights boosted and the weakest dimmed.
        let fudge = if dramatic { [2.0f32, 1.5, 1.0, 0.5][out.len().min(3)] } else { 1.0 };
        let src = &l.source;
        let color = l.base_color * intensity * alpha * fudge * if l.dynamic { 1.0 } else { light_mul };
        let shader = if let Some(dir) = src.direction {
            ShaderLight { kind: 1, direction: dir, color: color * 1.75, ..Default::default() }
        } else {
            let fitted = LightSource { radius: src.radius * fudge, ..*src };
            let (atten, range) = fitted_attenuation(&fitted, (src.position - centre).length(), sphere);
            let spot = src.effect == LE_STATIC_SPOT || src.effect == LE_SPOTLIGHT;
            let phi = 1.15 * ((1.0 - src.cone / 256.0).powi(2)).clamp(-1.0, 1.0).acos();
            ShaderLight {
                kind: if spot { 2 } else { 0 },
                position: src.position,
                direction: src.aim,
                color,
                atten,
                range,
                cos_phi: (phi * 0.5).cos(),
                cos_theta: (phi * 0.05 * 0.5).cos(),
            }
        };
        out.push(shader);
    }
    ActorLight { ambient, lights: out, fade: 0.0 }
}

/// Prints what lights an actor (HP3_ACTOR_LIGHT_DEBUG=<name>).
pub(crate) fn debug_print(vm: &mut Vm<'_>, actor: ObjId, light: &ActorLight) {
    static NAME: OnceLock<Option<String>> = OnceLock::new();
    let Some(name) = NAME.get_or_init(|| std::env::var("HP3_ACTOR_LIGHT_DEBUG").ok()) else { return };
    if hp3_script::natives::object_path(vm, actor).ends_with(&format!(".{name}")) {
        eprintln!("actor light {}: ambient {}", hp3_script::natives::object_path(vm, actor), light.ambient);
        for l in &light.lights {
            eprintln!("  kind {} at {} color {} atten {:?} range {:.0}", l.kind, l.position, l.color, l.atten, l.range);
        }
    }
}
