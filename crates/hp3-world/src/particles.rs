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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Blend {
    #[default]
    Alpha,
    Additive,
    Modulate,
}

/// How wide the line segments of a spark are (the original draws 1-pixel lines).
const SPARK_WIDTH: f32 = 0.6;

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
#[derive(Clone, Default)]
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
    /// `StartLocationPolarRange`: angle around, angle from the top (65536ths of a turn) and radius.
    polar: RangeVec,
    /// `UseRotationFrom` (0 none, 1 the actor's rotation and the offset, 2 the offset, 3 the normal), `RotationOffset`,
    /// `RotationNormal` and `EffectAxis` (1: the effect was made along +Z instead of -X).
    rot_from: u8,
    rot_offset: [i32; 3],
    rot_normal: Vec3,
    effect_axis: u8,
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
    /// `UseSkeletalLocationAs` (0 none, 1 spawn offset, 2 follows the bone), the actor whose bones are used, their scale
    /// and which part of the bone list to pick from (0..1).
    skel_mode: u8,
    skel_actor: Option<ObjId>,
    skel_scale: Vec3,
    bone_range: Range,
    auto_destroy: bool,
    respawn: bool,
    // Beams.
    beam_distance: Range,
    beam_end_by: u8,
    beam_offset: RangeVec,
    beam_segments: i32,
    noise: RangeVec,
    beam_u_scale: f32,
    // Mesh emitters: each particle is this static mesh, spinning on three axes, and may bounce off the floor.
    // Spark emitters: each particle is a trail of line segments along its path (`LineSegmentsRange`, `TimeBetweenSegmentsRange`).
    is_spark: bool,
    line_segments: Range,
    time_between_segments: Range,
    is_mesh: bool,
    mesh: Option<ObjId>,
    spins3: RangeVec,
    start_spin3: RangeVec,
    collide: bool,
    damping: RangeVec,
    use_max_collisions: bool,
    max_collisions: Range,
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

/// An Unreal `Color` (declared `B, G, R, A`) as red, green, blue, alpha in 0..1.
fn colour(v: &Value) -> [f32; 4] {
    let f = v.fields();
    let c = |i: usize| f.get(i).map(|x| x.as_int() as f32 / 255.0).unwrap_or(1.0);
    [c(2), c(1), c(0), c(3)]
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
            polar: RangeVec::of(&get(vm, o, "StartLocationPolarRange")),
            rot_from: get(vm, o, "UseRotationFrom").as_byte(),
            rot_offset: {
                let r: Vec<i32> = get(vm, o, "RotationOffset").fields().iter().map(Value::as_int).collect();
                [r.first().copied().unwrap_or(0), r.get(1).copied().unwrap_or(0), r.get(2).copied().unwrap_or(0)]
            },
            rot_normal: vec3(vm, o, "RotationNormal"),
            effect_axis: get(vm, o, "EffectAxis").as_byte(),
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
            skel_mode: get(vm, o, "UseSkeletalLocationAs").as_byte(),
            skel_actor: get(vm, o, "SkeletalMeshActor").as_obj(),
            skel_scale: vec3(vm, o, "SkeletalScale"),
            bone_range: Range::of(&get(vm, o, "RelativeBoneIndexRange")),
            auto_destroy: flag(vm, o, "AutoDestroy"),
            respawn: flag(vm, o, "RespawnDeadParticles"),
            beam_distance: Range::of(&get(vm, o, "BeamDistanceRange")),
            beam_end_by: get(vm, o, "DetermineEndPointBy").as_byte(),
            beam_offset: RangeVec::of(&get(vm, o, "OffsetRange")),
            beam_segments: get(vm, o, "BeamSegments").as_int().max(1),
            noise: RangeVec::of(&get(vm, o, "LowFrequencyNoiseRange")),
            beam_u_scale: float(vm, o, "RotatingSheets").max(1.0),
            is_spark: class.contains("sparkemitter"),
            line_segments: Range::of(&get(vm, o, "LineSegmentsRange")),
            time_between_segments: Range::of(&get(vm, o, "TimeBetweenSegmentsRange")),
            is_mesh: class.contains("meshemitter"),
            mesh: get(vm, o, "StaticMesh").as_obj(),
            spins3: RangeVec::of(&get(vm, o, "SpinsPerSecondRange")),
            start_spin3: RangeVec::of(&get(vm, o, "StartSpinRange")),
            collide: flag(vm, o, "UseCollision"),
            damping: RangeVec::of(&get(vm, o, "DampingFactorRange")),
            use_max_collisions: flag(vm, o, "UseMaxCollisions"),
            max_collisions: Range::of(&get(vm, o, "MaxCollisions")),
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
    /// Skeletal emitters: the bone it was born at and where that bone (scaled) was last seen.
    bone: usize,
    old_mesh: Vec3,
    /// Where it was at the last step, and for a mesh particle its turn and spin (radians, per axis), scale and bounces so far.
    /// Sparks: where and how fast it was born, and the time between the points of its trail.
    start: Vec3,
    start_vel: Vec3,
    segment_dt: f32,
    prev: Vec3,
    rot3: Vec3,
    spin3: Vec3,
    scale3: Vec3,
    hits: u32,
}

/// A line the script gives an emitter to spawn along (`LineExpression`): points of the line from its start to its
/// end, each with the origin and the three axes the script gives for it, and how far along the line particles
/// may be born so far.
#[derive(Clone)]
struct Line {
    points: Vec<(Vec3, [Vec3; 3])>,
    alpha_max: f32,
}

struct EmitterState {
    line: Option<Line>,
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
    /// How long each emitter actor with a line has been running (the line fills over time).
    line_age: crate::HashMap<ObjId, f32>,
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

/// The turns `UseRotationFrom` gives a newborn particle's offset and velocity (the normal's turn of the offset also
/// maps an effect made along +Z onto -X).
fn rotation_source(config: &Config, actor: Mat3) -> (Mat3, Mat3) {
    let turn = |r: [i32; 3]| Mat3::from_mat4(hp3_anim::rotator_matrix(r));
    match config.rot_from {
        1 => {
            let m = actor * turn(config.rot_offset);
            (m, m)
        }
        2 => {
            let m = turn(config.rot_offset);
            (m, m)
        }
        3 => {
            let n = config.rot_normal;
            let unit = 32768.0 / std::f32::consts::PI;
            let yaw = (n.y.atan2(n.x) * unit) as i32;
            let pitch = (n.z.atan2(n.x.hypot(n.y)) * unit) as i32;
            let axis_fix = if config.effect_axis == 1 { 16384 } else { 0 };
            (turn([pitch - axis_fix, yaw, 0]), turn([pitch, yaw, 0]))
        }
        _ => (Mat3::IDENTITY, Mat3::IDENTITY),
    }
}

fn spawn(config: &Config, rng: &mut Rng, origin: Vec3, rot: Mat3, fill_age: f32, line: Option<&Line>, bones: &[Vec3]) -> Particle {
    let local = match config.shape {
        _ if line.is_some_and(|l| !l.points.is_empty()) => {
            // Born along the script's line: a point of it, and the usual offset taken along its axes.
            let l = line.unwrap();
            let at = rng.next() * l.alpha_max.clamp(0.0, 1.0) * (l.points.len() - 1) as f32;
            let (i, f) = (at.floor() as usize, at.fract());
            let (a, b) = (&l.points[i], &l.points[(i + 1).min(l.points.len() - 1)]);
            let origin = a.0 + (b.0 - a.0) * f;
            let axes = [0, 1, 2].map(|k| a.1[k] + (b.1[k] - a.1[k]) * f);
            let r = config.start_offset + config.start_range.pick(rng);
            origin + axes[0] * r.x + axes[1] * r.y + axes[2] * r.z
        }
        shape => {
            // The shapes add up: a box (0), a sphere (1), polar coordinates (2), all three (3).
            let all = shape == 3;
            let mut at = config.start_offset;
            if all || shape == 0 {
                at += config.start_range.pick(rng);
            }
            if all || shape == 1 {
                let d = Vec3::new(rng.next() * 2.0 - 1.0, rng.next() * 2.0 - 1.0, rng.next() * 2.0 - 1.0).normalize_or_zero();
                at += d * config.sphere_radius.pick(rng);
            }
            if all || shape == 2 {
                let p = config.polar.pick(rng);
                // The first angle is the heading around the vertical and the second the angle down from straight up (65536ths
                // of a turn): a fixed heading and a sweeping second angle is a circle standing in the x-z plane. The emitter's
                // rotation source turns it (lying flat for the dementors' circle, across the flight for the hippogriff rings).
                let (around, down) = (p.x * std::f32::consts::PI / 32768.0, p.y * std::f32::consts::PI / 32768.0);
                at += p.z * Vec3::new(around.cos() * down.sin(), around.sin() * down.sin(), down.cos());
            }
            at
        }
    };
    // A skeletal emitter is born at one of the bones, scaled.
    let (mut bone, mut old_mesh) = (0, Vec3::ZERO);
    let mut local = local;
    if config.skel_mode != 0 && !bones.is_empty() {
        bone = ((config.bone_range.pick(rng) * bones.len() as f32) as usize).min(bones.len() - 1);
        old_mesh = bones[bone] * config.skel_scale;
        local += old_mesh;
    }
    let vel = config.start_velocity.pick(rng);
    let (pos, vel) = if line.is_some() {
        // A script's line is laid out in the actor's frame.
        let relative = config.coordinates == 1;
        let pos = if relative { local } else if config.coordinates == 2 { local } else { origin + rot * local };
        (pos, if config.coordinates == 2 { vel } else { rot * vel })
    } else {
        // The rotation source turns the born offset and the velocity; an independent particle is then placed at the actor.
        let (turn_pos, turn_vel) = rotation_source(config, rot);
        let local = turn_pos * local;
        (if config.coordinates == 0 { origin + local } else { local }, turn_vel * vel)
    };
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
        bone,
        old_mesh,
        start: pos,
        start_vel: vel,
        segment_dt: if config.is_spark { config.time_between_segments.pick(rng) } else { 0.0 },
        prev: pos,
        rot3: if config.is_mesh { config.start_spin3.pick(rng) * std::f32::consts::TAU } else { Vec3::ZERO },
        spin3: if config.is_mesh && config.spin { config.spins3.pick(rng) * std::f32::consts::TAU } else { Vec3::ZERO },
        scale3: if config.uniform_size { Vec3::splat(size.x.abs().max(0.01)) } else { Vec3::new(size.x.abs().max(0.01), size.y.abs().max(0.01), size.z.abs().max(0.01)) },
        hits: 0,
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
            // The default emitter may not be loaded yet: bring it in before copying it.
            vm.ensure_loaded(o);
            let entry = vm.db.objects.get(o).clone();
            let (class, values) = (entry.class?, vm.db.objects.values(o).cloned()?);
            Some(vm.db.objects.add(entry.name, class, values))
        });
        if copy.is_none() {
            vm.log.push(format!("emitter copy failed for {:?}", e.as_obj()));
        }
        own.push(copy.map_or(e.clone(), |c| Value::Object(Some(c))));
    }
    let count = own.len();
    set(vm, a, "Emitters", Value::Array(own.into()));
    let path = hp3_script::natives::object_path(vm, a);
    vm.log.push(format!("emitter spawned: {path} with {count} emitters"));
}

/// A line about an emitter actor for the game log: where it is and how each of its emitters is doing.
pub fn debug_emitter(vm: &mut Vm<'_>, a: ObjId) -> String {
    let at = crate::physics::vec(&get(vm, a, "Location"));
    let hidden = get(vm, a, "bHidden").as_bool();
    let scale = get(vm, a, "DrawScale").as_float();
    let physics = get(vm, a, "Physics").as_byte();
    let owner = get(vm, a, "Owner").as_obj().map(|o| hp3_script::natives::object_path(vm, o));
    let mut out = format!("at {at} hidden {hidden} drawscale {scale} physics {physics} owner {owner:?}");
    let mut keys: Vec<_> = world(vm).particles.states.keys().filter(|k| k.0 == a).copied().collect();
    keys.sort();
    for k in keys {
        if let Some(s) = world(vm).particles.states.get(&k) {
            let c = &s.config;
            out += &format!(
                " | [{}] live {} max {} pps {} initial {} auto {} disabled {} respawn {} kill {} slots {} material {} size {:?} life {:?}",
                k.1,
                s.particles.len(),
                c.max_particles,
                c.pps,
                c.initial_pps,
                c.auto_initial,
                c.disabled,
                c.respawn,
                s.kill_pending,
                s.slots_used,
                c.material.is_some(),
                (c.start_size.0[0].mid(), c.start_size.0[1].mid()),
                c.lifetime.mid()
            );
        }
    }
    out
}

/// Steps every emitter actor's emitters by `dt`.
/// An emitter set on an actor (a creature's flames: `SetBase` with a relative location, not on a bone) rides with it. The
/// engine carries the riders of movers only, so without this the flames stay where the creature was when it spawned them.
fn follow_bases(vm: &mut Vm<'_>) {
    let mut emitters: Vec<ObjId> = world(vm).particles.states.keys().map(|k| k.0).collect();
    emitters.sort_unstable();
    emitters.dedup();
    for a in emitters {
        if vm.db.objects.get(a).deleted {
            continue;
        }
        let Some(base) = get(vm, a, "Base").as_obj() else { continue };
        if base == a || vm.db.objects.get(base).deleted || get(vm, base, "bWorldGeometry").as_bool() {
            continue;
        }
        // A bone attachment is placed by the animation, and a mover carries its own riders.
        if world(vm).anim.attachments.contains_key(&a) || get(vm, a, "AttachmentBone").as_name() != hp3_script::Name::NONE || world(vm).collision.mover(base).is_some() {
            continue;
        }
        let want = crate::physics::vec(&get(vm, base, "Location")) + crate::physics::vec(&get(vm, a, "RelativeLocation"));
        if want.distance_squared(crate::physics::vec(&get(vm, a, "Location"))) > 1e-4 {
            set(vm, a, "Location", crate::physics::value(want));
        }
    }
}

pub fn tick(vm: &mut Vm<'_>, dt: f32) {
    let Some(emitter_class) = vm.db.find("Emitter") else { return };
    follow_bases(vm);
    collide_meshes(vm);
    let actors = world(vm).actors.clone();
    let frame = world(vm).frame;
    let mut seen: Vec<(ObjId, usize)> = Vec::new();
    let mut skeletons: crate::HashMap<ObjId, Vec<Vec3>> = crate::HashMap::default();
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
        let line = if hidden { None } else { line_of(vm, a, dt) };
        for (i, e) in emitters.iter().enumerate() {
            let Some(obj) = e.as_obj() else { continue };
            let key = (a, i);
            seen.push(key);
            let needs = world(vm).particles.states.get(&key).is_none_or(|s| frame.wrapping_sub(s.refreshed) >= REFRESH_FRAMES);
            if needs {
                let mut config = Config::read(vm, obj);
                // The circle of fake dementors stands 700-900 units out as the level has it, which is beyond the standing stones
                // and the trees, out of sight of the fight. Closer, they ring the arena.
                if vm.db.objects.get(a).class.is_some_and(|c| vm.db.names.get(vm.db.get(c).name).eq_ignore_ascii_case("DementorsRing")) {
                    const DEMENTOR_CIRCLE_SCALE: f32 = 0.6;
                    config.polar.0[2] = Range(config.polar.0[2].0 * DEMENTOR_CIRCLE_SCALE, config.polar.0[2].1 * DEMENTOR_CIRCLE_SCALE);
                }
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
                            EmitterState { line: None, config, refreshed: frame, particles: Vec::new(), accum: 0.0, rng: Rng(seed), slots_used: 0, object: obj, kill_pending: false, deferred: 0, last_origin: origin },
                        );
                    }
                }
            }
            // An emitter on a skeletal mesh spawns at its bones (the pose is computed once per actor each step).
            let bones = match world(vm).particles.states.get(&key).map(|s| (s.config.skel_mode, s.config.skel_actor)) {
                Some((mode, Some(actor))) if mode != 0 && !vm.db.objects.get(actor).deleted => {
                    if !skeletons.contains_key(&actor) {
                        let origins = crate::anim::mesh_bone_origins(vm, actor).unwrap_or_default();
                        skeletons.insert(actor, origins);
                    }
                    skeletons[&actor].clone()
                }
                _ => Vec::new(),
            };
            let name = vm.db.objects.get(a).name;
            let name = vm.db.names.get(name).to_string();
            if std::env::var_os("HP3_PARTICLE_DEBUG").is_some() && name.starts_with("ChallengeShield") && world(vm).frame % 120 == 0 {
                let mat = world(vm).particles.states.get(&key).and_then(|s| s.config.material).map(|m| hp3_script::natives::object_path(vm, m));
                eprintln!("emitter-material {name} #{i} {mat:?}");
                if let Some(s) = world(vm).particles.states.get(&key) {
                    let c = &s.config;
                    eprintln!("emitter-config {name} #{i}: size {:?} uniform {} velocity {:?} accel {} start_range {:?} shape {} spin {} {}..{} grid {}x{} sub {}..{} random_sub {} coords {} respawn {} opacity {}", c.start_size.0.map(|r| (r.0, r.1)), c.uniform_size, c.start_velocity.0.map(|r| (r.0, r.1)), c.acceleration, c.start_range.0.map(|r| (r.0, r.1)), c.shape, c.spin, c.spins_per_second.0, c.spins_per_second.1, c.u_div, c.v_div, c.sub_start, c.sub_end, c.random_sub, c.coordinates, c.respawn, c.opacity);
                }
            }
            let Some(s) = world(vm).particles.states.get_mut(&key) else { continue };
            s.line = line.clone();
            step(s, dt, origin, rot, hidden, &bones);
            if needs && std::env::var_os("HP3_PARTICLE_DEBUG").is_some() {
                let c = &s.config;
                eprintln!(
                    "emitter {} #{i}: hidden {hidden} beam {} disabled {} max {} pps {} initial {} auto {} life {}..{} size {}..{} material {:?} blend {:?} coords {} alive {}",
                    name,
                    c.beam, c.disabled, c.max_particles, c.pps, c.initial_pps, c.auto_initial, c.lifetime.0, c.lifetime.1,
                    c.start_size.0[0].0, c.start_size.0[0].1, c.material, c.blend, c.coordinates, s.particles.len()
                );
                for p in s.particles.iter().take(3) {
                    eprintln!("   particle at {:?} (emitter origin {:?}) size {:?} age {:.1}/{:.1}", p.pos, origin, p.size, p.age, p.life);
                }
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

/// The line an emitter actor has been given: its `LineExpression` delegate (the Carpe Retractum spell binds its
/// own line function) asked for 25 points from the start of the line to its end. The line fills from the start
/// over `fTimeToFill` seconds when `bAddOverTime`.
fn line_of(vm: &mut Vm<'_>, a: ObjId, dt: f32) -> Option<Line> {
    let Value::Delegate(Some(owner), name) = get(vm, a, "__LineExpression__Delegate") else { return None };
    if name == hp3_script::Name::NONE || vm.db.objects.get(owner).deleted {
        return None;
    }
    let name = vm.db.names.get(name).to_string();
    let age = {
        let t = world(vm).particles.line_age.entry(a).or_insert(0.0);
        *t += dt;
        *t
    };
    let fill = get(vm, a, "fTimeToFill").as_float();
    let alpha_max = if get(vm, a, "bAddOverTime").as_bool() && fill > 0.0 { (age / fill).clamp(0.02, 1.0) } else { 1.0 };
    let mut points = Vec::with_capacity(25);
    for i in 0..25 {
        let alpha = i as f32 / 24.0;
        let coords = crate::fire(vm, owner, &name, &mut [Value::Float(alpha)]);
        let f = coords.fields();
        if f.len() < 4 {
            return None;
        }
        points.push((crate::physics::vec(&f[0]), [crate::physics::vec(&f[1]), crate::physics::vec(&f[2]), crate::physics::vec(&f[3])]));
    }
    Some(Line { points, alpha_max })
}

fn step(s: &mut EmitterState, dt: f32, origin: Vec3, rot: Mat3, hidden: bool, bones: &[Vec3]) {
    // A hidden emitter stands still: its particles keep their age and come back as they were when it is shown
    // again (the aim symbols are hidden and shown by the spell cursor).
    if hidden {
        return;
    }
    let c = s.config.clone();
    s.last_origin = origin;
    let off = c.disabled;
    let new_particle = |s: &mut EmitterState, c: &Config, age: f32| {
        let mut p = spawn(c, &mut s.rng, origin, rot, age, s.line.as_ref(), bones);
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
        p.prev = p.pos;
        p.rot3 += p.spin3 * dt;
        p.vel += c.acceleration * dt;
        p.pos += p.vel * dt;
        // Particles of an emitter that follows its bones move with the bone they were born at.
        if c.skel_mode == 2 && !bones.is_empty() {
            let now = bones[p.bone.min(bones.len() - 1)] * c.skel_scale;
            let delta = now - p.old_mesh;
            p.pos += if c.coordinates == 1 { delta } else { rot * delta };
            p.old_mesh = now;
        }
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

/// Mesh particles that moved last step bounce off the level's geometry: they are put back where they met it and their
/// velocity is mirrored and damped (`DampingFactorRange`), and after `MaxCollisions` bounces they are done.
fn collide_meshes(vm: &mut Vm<'_>) {
    let keys: Vec<(ObjId, usize)> = world(vm).particles.states.iter().filter(|(_, s)| s.config.is_mesh && s.config.collide && s.config.coordinates == 0).map(|(k, _)| *k).collect();
    for key in keys {
        let Some(s) = world(vm).particles.states.get_mut(&key) else { continue };
        let mut particles = std::mem::take(&mut s.particles);
        let (config, mut rng) = (s.config.clone(), Rng(s.rng.0));
        for p in &mut particles {
            if p.prev == p.pos {
                continue;
            }
            let Some(hit) = world(vm).collision.trace_static(p.prev, p.pos, Vec3::splat(2.0)) else { continue };
            if hit.time <= 0.0 && p.prev == hit.location {
                continue;
            }
            p.pos = hit.location + hit.normal * 0.5;
            let bounced = p.vel - 2.0 * p.vel.dot(hit.normal) * hit.normal;
            let damp = config.damping.pick(&mut rng);
            let damp = if damp == Vec3::ZERO { Vec3::splat(0.5) } else { damp };
            p.vel = bounced * damp;
            p.spin3 *= 0.7;
            p.hits += 1;
            if config.use_max_collisions && p.hits as f32 >= config.max_collisions.pick(&mut rng).max(1.0) {
                p.age = p.life;
            }
        }
        if let Some(s) = world(vm).particles.states.get_mut(&key) {
            s.rng = rng;
            s.particles = particles;
        }
    }
}

/// A piece thrown by a mesh emitter, to draw as the static mesh `mesh` at `pos`, turned by `rot` and scaled by `scale`.
#[derive(Clone, Debug)]
pub struct MeshParticle {
    pub mesh: ObjId,
    pub pos: Vec3,
    pub rot: Mat3,
    pub scale: Vec3,
}

/// The live particles of mesh emitters. A piece shrinks away over its fade-out (meshes have no alpha here).
pub fn collect_meshes(vm: &mut Vm<'_>) -> Vec<MeshParticle> {
    let keys: Vec<(ObjId, usize)> = world(vm).particles.states.iter().filter(|(_, s)| s.config.is_mesh).map(|(k, _)| *k).collect();
    let mut out = Vec::new();
    for key in keys {
        let a = key.0;
        if get(vm, a, "bHidden").as_bool() {
            continue;
        }
        let origin = crate::physics::vec(&get(vm, a, "Location"));
        let rot = rotation_of(vm, a);
        let Some(s) = world(vm).particles.states.get(&key) else { continue };
        let c = &s.config;
        let Some(mesh) = c.mesh else { continue };
        for p in &s.particles {
            let t = (p.age / p.life).clamp(0.0, 1.0);
            let mut grow = 1.0;
            if !c.size_scale.is_empty() {
                let u = (t * c.size_repeats).fract();
                grow = interpolate(&c.size_scale, if t >= 1.0 { 1.0 } else { u }, |a, b, f| a + (b - a) * f).unwrap_or(1.0);
            }
            if let Some(start) = c.fade_out
                && start < 1.0
                && t > start
            {
                grow *= ((1.0 - t) / (1.0 - start)).max(0.0);
            }
            if grow <= 0.001 {
                continue;
            }
            let pos = if c.coordinates == 1 { origin + rot * p.pos } else { p.pos };
            let turn = Mat3::from_quat(glam::Quat::from_euler(glam::EulerRot::XYZ, p.rot3.x, p.rot3.y, p.rot3.z));
            out.push(MeshParticle { mesh, pos, rot: turn, scale: p.scale3 * grow });
        }
    }
    out
}

/// All live particles as quads facing the camera (`right` and `up` are the camera's axes).
pub fn collect(vm: &mut Vm<'_>, right: Vec3, up: Vec3) -> Vec<Quad> {
    let keys: Vec<(ObjId, usize)> = world(vm).particles.states.keys().copied().collect();
    let mut out = Vec::new();
    for key in keys {
        let a = key.0;
        if get(vm, a, "bHidden").as_bool() {
            continue;
        }
        let origin = crate::physics::vec(&get(vm, a, "Location"));
        let rot = rotation_of(vm, a);
        let Some(s) = world(vm).particles.states.get(&key) else { continue };
        let c = &s.config;
        if c.is_mesh {
            continue;
        }
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
            if c.is_spark {
                // A trail of `NumSegments` lines, each from where the particle was `k` steps ago to `k + 1` steps ago (the original draws
                // them as 1-pixel lines; here thin quads facing the camera). Positions come from the birth state, not the live one.
                let segments = c.line_segments.1.max(c.line_segments.0).max(2.0) as usize;
                let lift = if c.coordinates == 1 { origin } else { Vec3::ZERO };
                let turn = if c.coordinates == 1 { rot } else { Mat3::IDENTITY };
                let at = |time: f32| lift + turn * (p.start + p.start_vel * time + c.acceleration * time * time);
                let forward = right.cross(up);
                for k in 0..segments {
                    let (t0, t1) = ((p.age - k as f32 * p.segment_dt).max(0.0), (p.age - (k + 1) as f32 * p.segment_dt).max(0.0));
                    let (a, b) = (at(t0), at(t1));
                    let along = b - a;
                    if along.length_squared() < 1e-6 {
                        continue;
                    }
                    let side = along.cross(forward).normalize_or_zero() * SPARK_WIDTH * 0.5;
                    out.push(Quad {
                        material: c.material,
                        blend: c.blend,
                        corners: [a - side, a + side, b + side, b - side],
                        uv: [0.0, k as f32 / segments as f32, 1.0, (k + 1) as f32 / segments as f32],
                        color: colour,
                    });
                }
                continue;
            }
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
        let path = hp3_script::natives::object_path(vm, actor);
        vm.log.push(format!("emitter killed: {path}"));
        // An actor spawned at run time has emitter copies of its own; the class's shared default objects (which
        // come from a package) are never touched, whatever the actor holds.
        own_emitters(vm, actor);
        let spawned = vm.db.objects.get(actor).key.is_none();
        if let Value::Array(emitters) = get(vm, actor, "Emitters") {
            for e in emitters.iter().filter_map(Value::as_obj) {
                if spawned && vm.db.objects.get(e).key.is_some() {
                    vm.log.push(format!("emitter kill skipped a shared default: {path}"));
                    continue;
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A polar circle: a heading of 0 and the second angle all the way round, of radius 100.
    fn circle(rot_from: u8, rot_offset: [i32; 3], rot_normal: Vec3) -> Vec<Vec3> {
        let config = Config {
            shape: 2,
            coordinates: 1,
            polar: RangeVec([Range(0.0, 0.0), Range(0.0, 65536.0), Range(100.0, 100.0)]),
            lifetime: Range(1.0, 1.0),
            rot_from,
            rot_offset,
            rot_normal,
            ..Default::default()
        };
        let mut rng = Rng(12345);
        (0..32).map(|_| spawn(&config, &mut rng, Vec3::ZERO, Mat3::IDENTITY, 0.0, None, &[]).pos).collect()
    }

    #[test]
    fn hippogriff_ring_stands_across_the_flight() {
        // `UseRotationFrom` normal with the normal along +y: the circle stands in the y-z plane.
        let points = circle(3, [0; 3], Vec3::Y);
        assert!(points.iter().all(|p| p.x.abs() < 0.5 && (p.length() - 100.0).abs() < 0.5));
        assert!(points.iter().any(|p| p.z.abs() > 50.0) && points.iter().any(|p| p.y.abs() > 50.0));
    }

    #[test]
    fn dementor_circle_lies_flat() {
        // A quarter turn of roll as the offset lays the circle on the ground.
        let points = circle(2, [0, 0, 16384], Vec3::ZERO);
        assert!(points.iter().all(|p| p.z.abs() < 0.5));
        assert!(points.iter().any(|p| p.x.abs() > 50.0) && points.iter().any(|p| p.y.abs() > 50.0));
    }
}
