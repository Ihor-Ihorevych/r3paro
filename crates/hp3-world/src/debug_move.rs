//! Debug movement for the player's pawn: walking as normal, flying with collision, or a ghost that passes
//! through everything (the ordinary `Walk`, `Fly` and `Ghost` cheats).

use hp3_script::{Name, Value, Vm};

use crate::physics::{PHYS_FALLING, PHYS_FLYING};
use crate::{fire, get, set, world};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Walk,
    Fly,
    Ghost,
}

/// Speed a flying or ghost pawn may reach, units per second.
const FLY_SPEED: f32 = 900.0;

fn goto(vm: &mut Vm<'_>, controller: hp3_script::ObjId, state: &str) {
    let n = vm.db.names.intern(state);
    let _ = vm.goto_state(controller, n, Name::NONE);
}

/// Switches the player's pawn to `mode`; returns whether there was a pawn to switch.
pub fn set_mode(vm: &mut Vm<'_>, mode: Mode) -> bool {
    let Some(pc) = world(vm).player else { return false };
    let Some(pawn) = get(vm, pc, "Pawn").as_obj() else { return false };
    let collide = mode != Mode::Ghost;
    for name in ["bCollideWorld", "bCollideActors", "bBlockActors", "bBlockPlayers"] {
        set(vm, pawn, name, Value::Bool(collide));
    }
    match mode {
        Mode::Walk => {
            if let Some(speed) = world(vm).debug_air_speed.take() {
                set(vm, pawn, "AirSpeed", Value::Float(speed));
            }
            set(vm, pawn, "Physics", Value::Byte(PHYS_FALLING));
            fire(vm, pawn, "Falling", &mut []);
            goto(vm, pc, "PlayerWalking");
        }
        Mode::Fly | Mode::Ghost => {
            if world(vm).debug_air_speed.is_none() {
                let speed = get(vm, pawn, "AirSpeed").as_float();
                world(vm).debug_air_speed = Some(speed);
            }
            set(vm, pawn, "AirSpeed", Value::Float(FLY_SPEED));
            set(vm, pawn, "Velocity", crate::physics::value(glam::Vec3::ZERO));
            set(vm, pawn, "Physics", Value::Byte(PHYS_FLYING));
            goto(vm, pc, "PlayerFlying");
        }
    }
    true
}
