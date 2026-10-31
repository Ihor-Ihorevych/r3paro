//! Particle emitters: the `Emitter` actors' `ParticleEmitter` objects (sprites and beams), simulated on the CPU
//! and handed to the renderer as world-space quads.
//!
//! Supported: spawn rates and the initial fill, lifetimes, start location (box or sphere), start velocity,
//! acceleration, size and colour over a particle's life, fades, spin, texture sub-frames, the draw styles,
//! relative / independent / absolute coordinate systems, and beams between the emitter and an end point.
//! Not yet: collision, attractors, revolution, mesh spawning, spawning from other emitters, mesh emitters
//! and trails.

use glam::{Mat3, Vec3};
use hp3_script::{ObjId, Value, Vm};

use crate::{get, set, world};

/// How a particle is blended with what is behind it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Blend {
    Alpha,
    Additive,
    Modulate,
}

/// A particle (or a piece of a beam) as a quad in world space.
#[derive(Clone, Debug)]
pub struct Quad {
    pub material: Option<ObjId>,
    pub blend: Blend,
    /// Corners in the order top-left, top-right, bottom-right, bottom-left (of the texture).
    pub corners: [Vec3; 4],
    /// Texture rectangle: u0, v0, u1, v1.
    pub uv: [f32; 4],
    pub color: [f32; 4],
}

#[derive(Clone, Copy, Default)]
struct Range(f32, f32);

impl Range {
    fn of(v: &Value) -> Range {
        let f = v.fields();
        Range(f.first().map(Value::as_float).unwrap_or(0.0), f.get(1).map(Value::as_float).unwrap_or(0.0))
    }

    fn pick(self, r: &mut Rng) -> f32 {
        self.0 + (self.1 - self.0) * r.next()
    }

    fn mid(self) -> f32 {
        (self.0 + self.1) * 0.5
    }
}

#[derive(Clone, Copy, Default)]
struct RangeVec([Range; 3]);

impl RangeVec {
    fn of(v: &Value) -> RangeVec {
        let f = v.fields();
        let at = |i: usize| f.get(i).map(Range::of).unwrap_or_default();
        RangeVec([at(0), at(1), at(2)])
    }

    fn pick(self, r: &mut Rng) -> Vec3 {
        Vec3::new(self.0[0].pick(r), self.0[1].pick(r), self.0[2].pick(r))
    }
}

struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }
}

/// One emitter's settings, read from its object.
#[derive(Clone)]
struct Config {
    beam: bool,
    disabled: bool,
    max_particles: usize,
    pps: f32,
    initial_pps: f32,
    auto_initial: bool,
    lifetime: Range,
    start_offset: Vec3,
    start_range: RangeVec,
    shape: u8,
    sphere_radius: Range,
    start_velocity: RangeVec,
    acceleration: Vec3,
    start_size: RangeVec,
    uniform_size: bool,
    size_scale: Vec<(f32, f32)>,
    size_repeats: f32,
    color_scale: Vec<(f32, [f32; 4])>,
    color_repeats: f32,
    color_mult: RangeVec,
    opacity: f32,
    fade_in: Option<f32>,
    fade_out: Option<f32>,
    spin: bool,
    spins_per_second: Range,
    start_spin: Range,
    coordinates: u8,
    blend: Blend,
    material: Option<ObjId>,
    u_div: i32,
    v_div: i32,
    sub_start: i32,
    sub_end: i32,
    random_sub: bool,
    auto_destroy: bool,
    respawn: bool,
    // Beams.
    beam_distance: Range,
    beam_end_by: u8,
    beam_offset: RangeVec,
    beam_segments: i32,
    noise: RangeVec,
    beam_u_scale: f32,
}

fn float(vm: &mut Vm<'_>, o: ObjId, name: &str) -> f32 {
    get(vm, o, name).as_float()
}

fn flag(vm: &mut Vm<'_>, o: ObjId, name: &str) -> bool {
    get(vm, o, name).as_bool()
}

fn vec3(vm: &mut Vm<'_>, o: ObjId, name: &str) -> Vec3 {
    crate::physics::vec(&get(vm, o, name))
}

fn colour(v: &Value) -> [f32; 4] {
    let f = v.fields();
    let c = |i: usize| f.get(i).map(|x| x.as_int() as f32 / 255.0).unwrap_or(1.0);
    [c(0), c(1), c(2), c(3)]
}

impl Config {
    fn read(vm: &mut Vm<'_>, o: ObjId) -> Config {
        let class = vm.db.objects.get(o).class.map(|c| vm.db.names.get(vm.db.get(c).name).to_ascii_lowercase()).unwrap_or_default();
        let draw_style = get(vm, o, "DrawStyle").as_byte();
        let blend = match draw_style {
            // Translucent and Brighten add light; Modulated and Darken darken what is behind.
            3 | 6 => Blend::Additive,
            2 | 5 => Blend::Modulate,
            _ => Blend::Alpha,
        };
        let regular = flag(vm, o, "UseRegularSizeScale");
        let size_scale = if flag(vm, o, "UseSizeScale") {
            let entries: Vec<f32> = match get(vm, o, "SizeScale") {
                Value::Array(a) => a.iter().map(|e| e.fields().get(1).map(Value::as_float).unwrap_or(1.0)).collect(),
                _ => Vec::new(),
            };
            let times: Vec<f32> = match get(vm, o, "SizeScale") {
                Value::Array(a) => a.iter().map(|e| e.fields().first().map(Value::as_float).unwrap_or(0.0)).collect(),
                _ => Vec::new(),
            };
            let n = entries.len();
            entries.iter().enumerate().map(|(i, &s)| (if regular && n > 1 { i as f32 / (n - 1) as f32 } else { times[i] }, s)).collect()
        } else {
            Vec::new()
        };
        let color_scale = if flag(vm, o, "UseColorScale") {
            match get(vm, o, "ColorScale") {
                Value::Array(a) => a.iter().map(|e| (e.fields().first().map(Value::as_float).unwrap_or(0.0), e.fields().get(1).map(colour).unwrap_or([1.0; 4]))).collect(),
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        };
        let fade_in = flag(vm, o, "FadeIn").then(|| float(vm, o, "FadeInEndTime"));
        let fade_out = flag(vm, o, "FadeOut").then(|| float(vm, o, "FadeOutStartTime"));
        let opacity = float(vm, o, "Opacity");
        Config {
            beam: class.contains("beam"),
            disabled: flag(vm, o, "Disabled"),
            max_particles: float(vm, o, "MaxParticles").max(0.0) as usize,
            pps: float(vm, o, "ParticlesPerSecond"),
            initial_pps: float(vm, o, "InitialParticlesPerSecond"),
            auto_initial: flag(vm, o, "AutomaticInitialSpawning"),
            lifetime: Range::of(&get(vm, o, "LifetimeRange")),
            start_offset: vec3(vm, o, "StartLocationOffset"),
            start_range: RangeVec::of(&get(vm, o, "StartLocationRange")),
            shape: get(vm, o, "StartLocationShape").as_byte(),
            sphere_radius: Range::of(&get(vm, o, "SphereRadiusRange")),
            start_velocity: RangeVec::of(&get(vm, o, "StartVelocityRange")),
            acceleration: vec3(vm, o, "Acceleration"),
            start_size: RangeVec::of(&get(vm, o, "StartSizeRange")),
            uniform_size: flag(vm, o, "UniformSize"),
            size_scale,
            size_repeats: float(vm, o, "SizeScaleRepeats").max(1.0),
            color_scale,
            color_repeats: float(vm, o, "ColorScaleRepeats").max(1.0),
            color_mult: RangeVec::of(&get(vm, o, "ColorMultiplierRange")),
            opacity: if opacity == 0.0 { 1.0 } else { opacity },
            fade_in,
            fade_out,
            spin: flag(vm, o, "SpinParticles"),
            spins_per_second: Range::of(&get(vm, o, "SpinsPerSecondRange")),
            start_spin: Range::of(&get(vm, o, "StartSpinRange")),
            coordinates: get(vm, o, "CoordinateSystem").as_byte(),
            blend,
            material: get(vm, o, "Texture").as_obj(),
            u_div: get(vm, o, "TextureUSubdivisions").as_int().max(1),
            v_div: get(vm, o, "TextureVSubdivisions").as_int().max(1),
            sub_start: get(vm, o, "SubdivisionStart").as_int(),
            sub_end: get(vm, o, "SubdivisionEnd").as_int(),
            random_sub: flag(vm, o, "UseRandomSubdivision"),
            auto_destroy: flag(vm, o, "AutoDestroy"),
            respawn: flag(vm, o, "RespawnDeadParticles"),
            beam_distance: Range::of(&get(vm, o, "BeamDistanceRange")),
            beam_end_by: get(vm, o, "DetermineEndPointBy").as_byte(),
            beam_offset: RangeVec::of(&get(vm, o, "OffsetRange")),
            beam_segments: get(vm, o, "BeamSegments").as_int().max(1),
            noise: RangeVec::of(&get(vm, o, "LowFrequencyNoiseRange")),
            beam_u_scale: float(vm, o, "RotatingSheets").max(1.0),
        }
    }
}

#[derive(Clone)]
struct Particle {
    pos: Vec3,
    vel: Vec3,
    age: f32,
    life: f32,
    size: [f32; 2],
    spin: f32,
    spin_rate: f32,
    mult: Vec3,
    frame: i32,
    /// Beams: the end point relative to the emitter (or absolute).
    end: Vec3,
}

struct EmitterState {
    config: Config,
    refreshed: u64,
    particles: Vec<Particle>,
    accum: f32,
    rng: Rng,
    /// The emitter object this state belongs to (the natives find states by it).
    object: ObjId,
    /// `Kill` was called: nothing more is spawned and the particles live out their lives.
    kill_pending: bool,
    /// Particles asked for by `SpawnParticle`, spawned next step whatever the rates.
    deferred: usize,
    /// How many particle slots the emitter has used so far (up to its maximum): the filling phase.
    slots_used: usize,
    /// Where the owner was last frame, for relative systems.
    last_origin: Vec3,
}

#[derive(Default)]
pub struct Particles {
    states: crate::HashMap<(ObjId, usize), EmitterState>,
    /// Emitter actors that already have their own emitter objects.
    owned: std::collections::HashSet<ObjId>,
    pub debug: bool,
}

const REFRESH_FRAMES: u64 = 8;

fn rotation_of(vm: &mut Vm<'_>, a: ObjId) -> Mat3 {
    let r: Vec<i32> = get(vm, a, "Rotation").fields().iter().map(Value::as_int).collect();
    let m = hp3_anim::rotator_matrix([r.first().copied().unwrap_or(0), r.get(1).copied().unwrap_or(0), r.get(2).copied().unwrap_or(0)]);
    Mat3::from_mat4(m)
}

fn interpolate<T: Copy>(table: &[(f32, T)], t: f32, mix: impl Fn(T, T, f32) -> T) -> Option<T> {
    let first = table.first()?;
    if t <= first.0 {
        return Some(first.1);
    }
    for w in table.windows(2) {
        if t <= w[1].0 {
            let span = (w[1].0 - w[0].0).max(1e-5);
            return Some(mix(w[0].1, w[1].1, (t - w[0].0) / span));
        }
    }
    table.last().map(|e| e.1)
}

fn spawn(config: &Config, rng: &mut Rng, origin: Vec3, rot: Mat3, fill_age: f32) -> Particle {
    let local = match config.shape {
        1 => {
            // Inside a sphere.
            let d = Vec3::new(rng.next() * 2.0 - 1.0, rng.next() * 2.0 - 1.0, rng.next() * 2.0 - 1.0).normalize_or_zero();
            config.start_offset + d * config.sphere_radius.pick(rng)
        }
        _ => config.start_offset + config.start_range.pick(rng),
    };
    let relative = config.coordinates == 1;
    let pos = if relative { local } else if config.coordinates == 2 { local } else { origin + rot * local };
    let vel = if config.coordinates == 2 { config.start_velocity.pick(rng) } else { rot * config.start_velocity.pick(rng) };
    let size = config.start_size.pick(rng);
    let life = config.lifetime.pick(rng).max(0.05);
    let (min, max) = (config.sub_start, config.sub_end.max(config.sub_start));
    Particle {
        pos,
        vel,
        age: fill_age.min(life),
        life,
        size: [size.x.abs().max(0.01), if config.uniform_size { size.x.abs().max(0.01) } else { size.y.abs().max(0.01) }],
        spin: if config.spin { config.start_spin.pick(rng) * std::f32::consts::TAU } else { 0.0 },
        spin_rate: if config.spin { config.spins_per_second.pick(rng) * std::f32::consts::TAU } else { 0.0 },
        mult: {
            let m = config.color_mult.pick(rng);
            if m == Vec3::ZERO { Vec3::ONE } else { m }
        },
        frame: if config.random_sub { min + (rng.next() * (max - min + 1) as f32) as i32 } else { min },
        end: Vec3::ZERO,
    }
}

/// Gives an emitter actor spawned at run time copies of its own of the class's default emitter objects, which are
/// shared with every other of its kind (scripts change them, `Kill` among others, and that must reach only this
/// actor). Done when the actor is spawned, before any script touches it; a level's own actors have their own.
pub fn own_emitters(vm: &mut Vm<'_>, a: ObjId) {
    if vm.db.objects.get(a).key.is_some() || world(vm).particles.owned.contains(&a) {
        return;
    }
    let Value::Array(emitters) = get(vm, a, "Emitters") else { return };
    world(vm).particles.owned.insert(a);
    let mut own = Vec::with_capacity(emitters.len());
    for e in emitters.iter() {
        let copy = e.as_obj().and_then(|o| {
            // Reading a property loads the object if it is still lazy.
            let _ = get(vm, o, "MaxParticles");
            let entry = vm.db.objects.get(o).clone();
            let (class, values) = (entry.class?, vm.db.objects.values(o).cloned()?);
            Some(vm.db.objects.add(entry.name, class, values))
        });
        own.push(copy.map_or(e.clone(), |c| Value::Object(Some(c))));
    }
    set(vm, a, "Emitters", Value::Array(own.into()));
}

/// Steps every emitter actor's emitters by `dt`.
pub fn tick(vm: &mut Vm<'_>, dt: f32) {
    let Some(emitter_class) = vm.db.find("Emitter") else { return };
    let actors = world(vm).actors.clone();
    let frame = world(vm).frame;
    let mut seen: Vec<(ObjId, usize)> = Vec::new();
    for a in actors {
        let Some(class) = vm.db.objects.get(a).class else { continue };
        if vm.db.objects.get(a).deleted || !vm.is_child_of(class, emitter_class) {
            continue;
        }
        own_emitters(vm, a);
        let Value::Array(emitters) = get(vm, a, "Emitters") else { continue };
        let origin = crate::physics::vec(&get(vm, a, "Location"));
        let rot = rotation_of(vm, a);
        let hidden = get(vm, a, "bHidden").as_bool();
        for (i, e) in emitters.iter().enumerate() {
            let Some(obj) = e.as_obj() else { continue };
            let key = (a, i);
            seen.push(key);
            let needs = world(vm).particles.states.get(&key).is_none_or(|s| frame.wrapping_sub(s.refreshed) >= REFRESH_FRAMES);
            if needs {
                let config = Config::read(vm, obj);
                let w = world(vm);
                match w.particles.states.get_mut(&key) {
                    Some(s) => {
                        s.config = config;
                        s.refreshed = frame;
                    }
                    None => {
                        let seed = (a.0.wrapping_mul(2654435761) ^ (i as u32).wrapping_mul(40503)).max(1);
                        w.particles.states.insert(
                            key,
                            EmitterState { config, refreshed: frame, particles: Vec::new(), accum: 0.0, rng: Rng(seed), slots_used: 0, object: obj, kill_pending: false, deferred: 0, last_origin: origin },
                        );
                    }
                }
            }
            let name = vm.db.objects.get(a).name;
            let name = vm.db.names.get(name).to_string();
            let Some(s) = world(vm).particles.states.get_mut(&key) else { continue };
            step(s, dt, origin, rot, hidden);
            if needs && std::env::var_os("HP3_PARTICLE_DEBUG").is_some() {
                let c = &s.config;
                eprintln!(
                    "emitter {} #{i}: beam {} disabled {} max {} pps {} initial {} auto {} life {}..{} size {}..{} material {:?} blend {:?} coords {} alive {}",
                    name,
                    c.beam, c.disabled, c.max_particles, c.pps, c.initial_pps, c.auto_initial, c.lifetime.0, c.lifetime.1,
                    c.start_size.0[0].0, c.start_size.0[0].1, c.material, c.blend, c.coordinates, s.particles.len()
                );
                eprintln!(
                    "   size scale {:?} x{} colour scale {:?} opacity {} fade in {:?} out {:?} mult {:?}",
                    c.size_scale, c.size_repeats, c.color_scale, c.opacity, c.fade_in, c.fade_out, c.color_mult.0.map(|r| (r.0, r.1))
                );
            }
        }
    }
    // States of emitters that no longer exist.
    world(vm).particles.states.retain(|k, _| seen.contains(k));
}

fn step(s: &mut EmitterState, dt: f32, origin: Vec3, rot: Mat3, hidden: bool) {
    let c = s.config.clone();
    s.last_origin = origin;
    let off = c.disabled || hidden;
    let new_particle = |s: &mut EmitterState, c: &Config, age: f32| {
        let mut p = spawn(c, &mut s.rng, origin, rot, age);
        if c.beam {
            let d = c.beam_distance.pick(&mut s.rng);
            p.end = Vec3::new(d, 0.0, 0.0) + c.beam_offset.pick(&mut s.rng);
        }
        p
    };
    // Move and age the particles. A particle that dies is replaced in place when the emitter respawns its
    // dead ones (so a full emitter stays full), and dropped otherwise.
    let mut i = 0;
    while i < s.particles.len() {
        let p = &mut s.particles[i];
        p.vel += c.acceleration * dt;
        p.pos += p.vel * dt;
        p.age += dt;
        p.spin += p.spin_rate * dt;
        if p.age >= p.life {
            if c.respawn && !off && !s.kill_pending {
                let carried = p.age - p.life;
                let fresh = new_particle(s, &c, carried);
                s.particles[i] = fresh;
            } else {
                s.particles.swap_remove(i);
                continue;
            }
        }
        i += 1;
    }
    if off || s.kill_pending {
        s.deferred = 0;
        return;
    }
    let cap = if c.max_particles == 0 { 128 } else { c.max_particles };
    for _ in 0..s.deferred.min(cap) {
        let p = new_particle(s, &c, 0.0);
        s.particles.push(p);
    }
    s.deferred = 0;
    // Until the emitter has used all its slots once it fills at one rate (the initial one, or enough to be full
    // in one lifetime when it spawns automatically); after that it emits at `ParticlesPerSecond`.
    let filling = s.slots_used < cap;
    let rate = if filling {
        if c.auto_initial { cap as f32 / c.lifetime.mid().max(0.05) } else { c.initial_pps }
    } else {
        c.pps
    };
    s.accum += rate * dt;
    while s.accum >= 1.0 && s.particles.len() < cap {
        s.accum -= 1.0;
        let p = new_particle(s, &c, 0.0);
        s.particles.push(p);
        s.slots_used = (s.slots_used + 1).min(cap);
    }
    s.accum = s.accum.min(1.0);
}

/// All live particles as quads facing the camera (`right` and `up` are the camera's axes).
pub fn collect(vm: &mut Vm<'_>, right: Vec3, up: Vec3) -> Vec<Quad> {
    let keys: Vec<(ObjId, usize)> = world(vm).particles.states.keys().copied().collect();
    let mut out = Vec::new();
    for key in keys {
        let a = key.0;
        let origin = crate::physics::vec(&get(vm, a, "Location"));
        let rot = rotation_of(vm, a);
        let Some(s) = world(vm).particles.states.get(&key) else { continue };
        let c = &s.config;
        for p in &s.particles {
            let t = (p.age / p.life).clamp(0.0, 1.0);
            let mut size_scale = 1.0;
            if !c.size_scale.is_empty() {
                let u = (t * c.size_repeats).fract();
                size_scale = interpolate(&c.size_scale, if t >= 1.0 { 1.0 } else { u }, |a, b, f| a + (b - a) * f).unwrap_or(1.0);
            }
            let mut color = [1.0f32; 4];
            if !c.color_scale.is_empty() {
                let u = (t * c.color_repeats).fract();
                color = interpolate(&c.color_scale, if t >= 1.0 { 1.0 } else { u }, |a, b, f| {
                    [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f, a[3] + (b[3] - a[3]) * f]
                })
                .unwrap_or(color);
            }
            // Visibility comes from the opacity and the fades; the colour's own alpha channel is not used.
            let mut alpha = c.opacity;
            if let Some(end) = c.fade_in
                && end > 0.0
                && t < end
            {
                alpha *= t / end;
            }
            if let Some(start) = c.fade_out
                && start < 1.0
                && t > start
            {
                alpha *= ((1.0 - t) / (1.0 - start)).max(0.0);
            }
            let rgb = [color[0] * p.mult.x, color[1] * p.mult.y, color[2] * p.mult.z];
            let colour = [rgb[0], rgb[1], rgb[2], alpha.clamp(0.0, 1.0)];
            // Texture sub-frame.
            let (ud, vd) = (c.u_div.max(1), c.v_div.max(1));
            let frame = if c.sub_end > c.sub_start && !c.random_sub { c.sub_start + (t * (c.sub_end - c.sub_start + 1) as f32) as i32 } else { p.frame };
            let (fu, fv) = (frame.rem_euclid(ud), (frame / ud).rem_euclid(vd));
            let uv = [fu as f32 / ud as f32, fv as f32 / vd as f32, (fu + 1) as f32 / ud as f32, (fv + 1) as f32 / vd as f32];
            let centre = if c.coordinates == 1 { origin + rot * p.pos } else { p.pos };
            if c.beam {
                let start = centre;
                let end = if c.coordinates == 2 { p.end } else { origin + rot * p.end };
                let dir = (end - start).normalize_or_zero();
                if dir == Vec3::ZERO {
                    continue;
                }
                let width = p.size[0] * size_scale * 0.5;
                let side = dir.cross(up.cross(right)).normalize_or_zero();
                let side = if side == Vec3::ZERO { right } else { side } * width;
                out.push(Quad { material: c.material, blend: c.blend, corners: [start - side, end - side, end + side, start + side], uv, color: colour });
                continue;
            }
            let (sn, cs) = p.spin.sin_cos();
            let (r, u) = (right * cs + up * sn, up * cs - right * sn);
            let (hw, hh) = (p.size[0] * size_scale * 0.5, p.size[1] * size_scale * 0.5);
            out.push(Quad {
                material: c.material,
                blend: c.blend,
                corners: [centre - r * hw + u * hh, centre + r * hw + u * hh, centre + r * hw - u * hh, centre - r * hw - u * hh],
                uv,
                color: colour,
            });
        }
    }
    out
}

/// How many particles are alive (for tests and debugging).
pub fn live(vm: &mut Vm<'_>) -> usize {
    world(vm).particles.states.values().map(|s| s.particles.len()).sum()
}

/// Per emitter actor: its name, location and live particle count (for debugging).
pub fn summary(vm: &mut Vm<'_>) -> Vec<(String, Vec3, usize)> {
    let keys: Vec<(ObjId, usize)> = world(vm).particles.states.keys().copied().collect();
    let mut out = Vec::new();
    for key in keys {
        let n = world(vm).particles.states.get(&key).map_or(0, |s| s.particles.len());
        let name = vm.db.objects.get(key.0).name;
        let name = format!("{}#{}", vm.db.names.get(name), key.1);
        out.push((name, crate::physics::vec(&get(vm, key.0, "Location")), n));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Whether an emitter can still produce particles.
fn finished(s: &EmitterState) -> bool {
    let c = &s.config;
    s.kill_pending || (!c.respawn && s.slots_used >= if c.max_particles == 0 { 128 } else { c.max_particles } && c.pps <= 0.0 && s.deferred == 0)
}

/// Emitter actors that asked to be destroyed when finished (`AutoDestroy`) and have nothing left on screen.
pub fn finished_actors(vm: &mut Vm<'_>) -> Vec<ObjId> {
    let mut by_actor: crate::HashMap<ObjId, (bool, bool)> = crate::HashMap::default();
    for (key, s) in &world(vm).particles.states {
        let e = by_actor.entry(key.0).or_insert((true, true));
        e.0 &= s.particles.is_empty();
        e.1 &= finished(s);
    }
    let mut out: Vec<ObjId> = by_actor.into_iter().filter(|(_, (empty, done))| *empty && *done).map(|(a, _)| a).collect();
    out.sort();
    out.retain(|&a| get(vm, a, "AutoDestroy").as_bool());
    out
}

pub fn register(vm: &mut Vm<'_>) {
    // Emitter.Kill(): stop all emission, let what is on screen fade, and destroy the actor when it is empty.
    vm.register("Emitter.Kill", |vm, this, _a| {
        let Some(actor) = this else { return Ok(Value::Empty) };
        if let Value::Array(emitters) = get(vm, actor, "Emitters") {
            for e in emitters.iter().filter_map(Value::as_obj) {
                for name in ["RespawnDeadParticles", "AutomaticInitialSpawning", "AutoReset", "AutoDestroy"] {
                    set(vm, e, name, Value::Bool(false));
                }
                for name in ["InitialParticlesPerSecond", "ParticlesPerSecond"] {
                    set(vm, e, name, Value::Float(0.0));
                }
            }
        }
        set(vm, actor, "AutoDestroy", Value::Bool(true));
        set(vm, actor, "AutoReset", Value::Bool(false));
        for (key, s) in world(vm).particles.states.iter_mut() {
            if key.0 == actor {
                s.kill_pending = true;
            }
        }
        Ok(Value::Empty)
    });
    // ParticleEmitter.SpawnParticle(Amount)
    vm.register("ParticleEmitter.SpawnParticle", |vm, this, a| {
        let (Some(e), Some(n)) = (this, a.first().map(Value::as_int)) else { return Ok(Value::Empty) };
        for s in world(vm).particles.states.values_mut() {
            if s.object == e && n > 0 {
                s.deferred += n as usize;
            }
        }
        Ok(Value::Empty)
    });
    // ParticleEmitter.Trigger(): toggles a trigger-disabled emitter and restarts one that resets on trigger.
    vm.register("ParticleEmitter.Trigger", |vm, this, _a| {
        let Some(e) = this else { return Ok(Value::Empty) };
        if get(vm, e, "TriggerDisabled").as_bool() {
            let now = get(vm, e, "Disabled").as_bool();
            set(vm, e, "Disabled", Value::Bool(!now));
        }
        if get(vm, e, "ResetOnTrigger").as_bool() {
            for s in world(vm).particles.states.values_mut() {
                if s.object == e {
                    s.particles.clear();
                    s.slots_used = 0;
                    s.accum = 0.0;
                }
            }
        }
        Ok(Value::Empty)
    });
}
