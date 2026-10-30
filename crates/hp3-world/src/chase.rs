//! The chase camera: the camera stays behind the player's character and swings with its heading, as in the
//! older third-person action games, instead of orbiting freely with the mouse. Left and right turn the heading
//! (they do not strafe); forward and back move along it.
//!
//! HP3's own camera script (`BaseCam`) keeps a destination rotation that the mouse normally changes; here the
//! heading is the destination.

use hp3_script::{Value, Vm};

use crate::{get, set, world};

/// Heading turn rate, Unreal rotation units per second (about 140 degrees).
const TURN_RATE: f32 = 25_000.0;
/// Left and right keys give this much on `aStrafe`.
const FULL_AXIS: f32 = 1200.0;

#[derive(Default)]
pub struct Chase {
    /// Off means free mouse look.
    pub off: bool,
    heading: Option<f32>,
}

/// Whether the camera is the ordinary follow camera (not in a cutscene, a fixed shot or free-flying).
fn following(vm: &mut Vm<'_>, cam: hp3_script::ObjId) -> bool {
    let Some(state) = vm.state_of(cam) else { return false };
    let name = vm.db.names.get(vm.db.get(state).name).to_ascii_lowercase();
    name == "statestandardcam"
}

/// Applies the held turn keys to the heading and points the camera along it. `turn` is the sum of the
/// strafe-axis keys (A is negative, D positive); `dt` the tick length.
pub fn update(vm: &mut Vm<'_>, pc: hp3_script::ObjId, turn: f32, dt: f32) {
    if world(vm).chase.off {
        world(vm).chase.heading = None;
        return;
    }
    let Some(cam) = get(vm, pc, "Camera").as_obj() else { return };
    if !following(vm, cam) {
        world(vm).chase.heading = None;
        return;
    }
    let current = get(vm, cam, "rDestRotation").fields().get(1).map(Value::as_int).unwrap_or(0) as f32;
    let heading = world(vm).chase.heading.unwrap_or(current);
    // D (positive) turns right, which lowers the yaw.
    let heading = (heading - (turn / FULL_AXIS) * TURN_RATE * dt).rem_euclid(65536.0);
    world(vm).chase.heading = Some(heading);
    let mut rot: Vec<Value> = get(vm, cam, "rDestRotation").fields().to_vec();
    if rot.len() == 3 {
        rot[1] = Value::Int(heading as i32);
        set(vm, cam, "rDestRotation", Value::Struct(rot.into()));
    }
}

/// Switches between the chase camera and free mouse look; returns whether chase is now on.
pub fn toggle(vm: &mut Vm<'_>) -> bool {
    let c = &mut world(vm).chase;
    c.off = !c.off;
    c.heading = None;
    !c.off
}
