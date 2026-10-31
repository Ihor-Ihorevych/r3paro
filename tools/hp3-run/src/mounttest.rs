//! `HP3_MOUNT_TEST=<n>@<tick>`: looks around the n-th mount volume of the level for places where the player's pawn
//! can stand with a wall just ahead, puts it there facing the wall and has it walk forward for a moment. Prints
//! the pawn's state and place, so a ledge climb (the `Mounting` states of the game's script) shows up. Meant to
//! be run with `HP3_TRACE_CALLS=mount`.

use glam::Vec3;
use hp3_script::{ObjId, Value, Vm};
use hp3_world::physics::vec;
use hp3_world::{fire, get, set};

pub struct MountTest {
    index: usize,
    /// `@Name@tick`: walk into the actor of that name from all sides instead of looking around a mount volume.
    actor: Option<String>,
    at: u32,
    started: Option<u32>,
    /// Where to try: a standing place and the yaw that faces the wall.
    spots: Vec<(Vec3, i32)>,
}

impl MountTest {
    pub fn from_env() -> Option<MountTest> {
        let spec = std::env::var("HP3_MOUNT_TEST").ok()?;
        if let Some(rest) = spec.strip_prefix('@') {
            let (name, at) = rest.split_once('@')?;
            return Some(MountTest { index: 0, actor: Some(name.to_string()), at: at.parse().ok()?, started: None, spots: Vec::new() });
        }
        let (n, at) = spec.split_once('@')?;
        Some(MountTest { index: n.parse().ok()?, actor: None, at: at.parse().ok()?, started: None, spots: Vec::new() })
    }
}

/// Frames each spot gets.
fn trial_len() -> u32 {
    std::env::var("HP3_MOUNT_TRIAL").ok().and_then(|v| v.parse().ok()).unwrap_or(60)
}

fn state_name(vm: &mut Vm<'_>, a: ObjId) -> String {
    vm.state_of(a).map(|s| vm.db.names.get(vm.db.get(s).name).to_string()).unwrap_or_else(|| "-".into())
}

fn v3(v: Vec3) -> Value {
    Value::Struct(vec![Value::Float(v.x), Value::Float(v.y), Value::Float(v.z)].into())
}

/// Standing places near `centre` with an upright wall 45 units ahead in one of eight directions that the pawn's
/// mount routine says it can climb (it is asked with the pawn put there).
fn find_spots(vm: &mut Vm<'_>, pawn: ObjId, volume: ObjId, centre: Vec3) -> Vec<(Vec3, i32)> {
    let r = get(vm, pawn, "CollisionRadius").as_float();
    let ext = Vec3::new(r, r, get(vm, pawn, "CollisionHeight").as_float());
    let mut spots = Vec::new();
    let was = vec(&get(vm, pawn, "Location"));
    for dx in (-48..=48).step_by(12) {
        for dy in (-48..=48).step_by(12) {
            let mut last_floor = f32::MIN;
            for dz in (-80..=80).step_by(8) {
                let from = centre + Vec3::new(dx as f32, dy as f32, dz as f32);
                let col = &hp3_world::world(vm).collision;
                if col.trace_static(from + Vec3::Z * 0.1, from, ext).is_some_and(|h| h.time <= 0.0) {
                    continue;
                }
                let Some(floor) = col.trace_static(from, from - Vec3::Z * 40.0, ext) else { continue };
                if floor.normal.z < 0.7 || (floor.location.z - last_floor).abs() < 4.0 {
                    continue;
                }
                last_floor = floor.location.z;
                let stand = floor.location + Vec3::Z * 3.0;
                for k in 0..8 {
                    let yaw = k * 8192;
                    let ang = yaw as f32 * std::f32::consts::TAU / 65536.0;
                    let ahead = stand + Vec3::new(ang.cos(), ang.sin(), 0.0) * 45.0;
                    let col = &hp3_world::world(vm).collision;
                    let Some(h) = col.trace_static(stand, ahead, ext) else { continue };
                    if h.time <= 0.0 || h.normal.z.abs() >= 0.17 {
                        continue;
                    }
                    // Face the wall square on, as a player climbing a ledge would.
                    let yaw = ((-h.normal.y).atan2(-h.normal.x) * 65536.0 / std::f32::consts::TAU).round() as i32;
                    fire(vm, pawn, "SetLocation", &mut [v3(stand)]);
                    set(vm, pawn, "Rotation", Value::Struct(vec![Value::Int(0), Value::Int(yaw), Value::Int(0)].into()));
                    // Only a pawn inside the volume can mount there (the game's script asks).
                    let inside = match get(vm, pawn, "Touching") {
                        Value::Array(v) => v.iter().any(|t| t.as_obj() == Some(volume)),
                        _ => false,
                    };
                    if inside && hp3_world::physics::mount_destination(vm, pawn, Vec3::Z, &h).is_some() {
                        spots.push((stand, yaw));
                    }
                }
            }
        }
    }
    fire(vm, pawn, "SetLocation", &mut [v3(was)]);
    spots
}

/// Standing places on the floor around `centre`, eight directions at two distances, each facing it.
fn ring_spots(vm: &mut Vm<'_>, pawn: ObjId, centre: Vec3) -> Vec<(Vec3, i32)> {
    let r = get(vm, pawn, "CollisionRadius").as_float();
    let ext = Vec3::new(r, r, get(vm, pawn, "CollisionHeight").as_float());
    let mut spots = Vec::new();
    for dist in [60.0f32, 90.0] {
        for k in 0..8 {
            let ang = k as f32 * std::f32::consts::TAU / 8.0;
            let dir = Vec3::new(ang.cos(), ang.sin(), 0.0);
            let above = centre - dir * dist + Vec3::Z * 150.0;
            let col = &hp3_world::world(vm).collision;
            let Some(floor) = col.trace_static(above, above - Vec3::Z * 400.0, ext) else { continue };
            if floor.normal.z < 0.7 {
                continue;
            }
            let stand = floor.location + Vec3::Z * 2.0;
            let yaw = (dir.y.atan2(dir.x) * 65536.0 / std::f32::consts::TAU).round() as i32;
            spots.push((stand, yaw));
        }
    }
    spots
}

pub fn step(vm: &mut Vm<'_>, t: &mut MountTest, frame: u32) {
    if frame < t.at {
        return;
    }
    let Some(pc) = hp3_world::world(vm).player else { return };
    let Some(pawn) = get(vm, pc, "Pawn").as_obj() else { return };
    let Some(class) = vm.db.find("MountVolume") else { std::process::exit(2) };
    if t.started.is_none() {
        if state_name(vm, pc) != "PlayerWalking" {
            t.at += 1;
            return;
        }
        if let Some(name) = t.actor.clone() {
            let actors = hp3_world::world(vm).actors.clone();
            let found = actors.into_iter().find(|&a| hp3_script::natives::object_path(vm, a).ends_with(&format!(".{name}")));
            let Some(target) = found else {
                println!("mount: no actor named {name}");
                std::process::exit(3)
            };
            let centre = vec(&get(vm, target, "Location"));
            t.spots = ring_spots(vm, pawn, centre);
            // How tall it is: the top found by tracing down through its middle, and the floor beside it.
            let col = &hp3_world::world(vm).collision;
            let top = col.trace_static(centre + Vec3::Z * 600.0, centre - Vec3::Z * 600.0, Vec3::ZERO);
            println!("mount: down through the middle: {:?}", top.map(|h| (h.location.z, h.actor.map(|a| a.0))));
            if let Some(&(stand, _)) = t.spots.first() {
                println!("mount: first standing place z {}", stand.z);
            }
            println!("mount: starting at tick {frame}: {name} at {centre}, {} places to try", t.spots.len());
            t.started = Some(frame);
        }
        if t.started.is_none() {
            let volumes: Vec<ObjId> = hp3_world::world(vm)
                .actors
                .clone()
                .into_iter()
                .filter(|&a| vm.db.objects.get(a).class.is_some_and(|c| vm.is_child_of(c, class)))
                .collect();
            let Some(&volume) = volumes.get(t.index) else {
                println!("mount: no volume #{} ({} in the level)", t.index, volumes.len());
                std::process::exit(3)
            };
            let centre = vec(&get(vm, volume, "Location"));
            t.spots = find_spots(vm, pawn, volume, centre);
            println!("mount: starting at tick {frame}: volume at {centre}, {} places to try", t.spots.len());
            t.started = Some(frame);
        }
    }
    let r = frame - t.started.unwrap_or(frame);
    let (trial, k) = ((r / trial_len()) as usize, r % trial_len());
    let Some(&(stand, yaw)) = t.spots.get(trial) else {
        println!("mount: no climb in {} tries", t.spots.len());
        std::process::exit(0)
    };
    if k == 0 {
        set(vm, pawn, "Velocity", v3(Vec3::ZERO));
        fire(vm, pawn, "SetLocation", &mut [v3(stand)]);
        if trial % 10 == 0 {
            println!("mount: try {trial}: pawn to {stand} facing yaw {yaw}");
        }
    }
    if k < 40 {
        // Keep the heading while placing and walking; the camera decides where Forward goes.
        let rot = Value::Struct(vec![Value::Int(0), Value::Int(yaw), Value::Int(0)].into());
        if let Some(cam) = get(vm, pc, "Camera").as_obj() {
            for name in ["rDestRotation", "rCurrRotation"] {
                set(vm, cam, name, rot.clone());
            }
        }
        set(vm, pc, "Rotation", rot.clone());
        set(vm, pawn, "Rotation", rot);
    }
    let input = &mut hp3_world::world(vm).input;
    if k == 5 {
        input.key_down("W");
    }
    if k == 40 {
        input.key_up("W");
    }
    if k % 10 == 9 && k >= 40 || k % 10 == 9 && trial_len() == 60 {
        let here = vec(&get(vm, pawn, "Location"));
        let r = get(vm, pawn, "CollisionRadius").as_float();
        let ext = Vec3::new(r, r, get(vm, pawn, "CollisionHeight").as_float());
        let below = hp3_world::world(vm).collision.trace_static(here, here - Vec3::Z * 300.0, ext).map(|h| (here.z - h.location.z, h.actor.map(|a| a.0)));
        println!("mount:   floor below (distance, actor) {below:?} cylinder {ext}");
        println!("mount:   k={k} at {} rot {} accel {} phys {:?}", vec(&get(vm, pawn, "Location")), get(vm, pawn, "Rotation").fields().iter().map(|f| f.as_int().to_string()).collect::<Vec<_>>().join(","), vec(&get(vm, pawn, "Acceleration")), get(vm, pawn, "Physics"));
    }
    let state = state_name(vm, pawn);
    if state.to_ascii_lowercase().contains("mount") {
        println!(
            "mount: try {trial} k={k}: CLIMB state {state}, from {stand} yaw {yaw}, now at {}",
            vec(&get(vm, pawn, "Location"))
        );
        if k == 30 {
            for e in hp3_world::world(vm).errors.iter().rev().take(6) {
                println!("mount: script error: {e}");
            }
            for line in vm.log.iter().rev().take(6) {
                println!("mount: warning: {line}");
            }
        }
        if k == trial_len() - 1 {
            std::process::exit(0);
        }
    }
}
