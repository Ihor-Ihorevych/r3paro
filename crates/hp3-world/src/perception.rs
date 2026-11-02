//! What the engine tells an AI controller about the player: `SeePlayer` when the player's pawn is in its pawn's sight.
//!
//! The scripts of creatures (a fire crab's attack, for one) start from that event, which the engine raised in the
//! original game; a controller that does not define it is left alone.

use hp3_script::{ObjId, Value, Vm};

use crate::{fire, get, physics::vec, world};

/// How often (in ticks) each controller looks.
const LOOK_EVERY: u64 = 5;
/// Head height above a pawn's location that sight is measured from.
const EYE_HEIGHT: f32 = 30.0;

/// Lets every AI controller whose pawn can see the player's pawn know it.
pub fn tick(vm: &mut Vm<'_>) {
    if world(vm).frame % LOOK_EVERY != 0 {
        return;
    }
    let Some(player_pawn) = world(vm).player.and_then(|pc| get(vm, pc, "Pawn").as_obj()) else { return };
    let debug = std::env::var_os("HP3_SEE_DEBUG").is_some();
    if get(vm, player_pawn, "Health").as_int() <= 0 || get(vm, player_pawn, "bHidden").as_bool() {
        if std::env::var_os("HP3_SEE_DEBUG").is_some() {
            eprintln!("see: player pawn health {} hidden {}", get(vm, player_pawn, "Health").as_int(), get(vm, player_pawn, "bHidden").as_bool());
        }
        return;
    }
    let Some(controller_class) = vm.db.find("Controller") else { return };
    let Some(see) = vm.db.names.find("SeePlayer") else { return };
    let target = vec(&get(vm, player_pawn, "Location")) + glam::Vec3::Z * EYE_HEIGHT;
    let actors = world(vm).actors.clone();
    for c in actors {
        if vm.db.objects.get(c).deleted || Some(c) == world(vm).player {
            continue;
        }
        let Some(class) = vm.db.objects.get(c).class else { continue };
        if !vm.is_child_of(class, controller_class) || vm.find_function(Some(c), None, see).is_none() {
            continue;
        }
        let Some(pawn) = get(vm, c, "Pawn").as_obj() else { continue };
        if debug {
            let eye = vec(&get(vm, pawn, "Location"));
            eprintln!("see: {} radius {} periph {} dist {} sees {}", hp3_script::natives::object_path(vm, c), get(vm, pawn, "SightRadius").as_float(), get(vm, pawn, "PeripheralVision").as_float(), (eye - target).length(), sees(vm, pawn, target));
        }
        if pawn == player_pawn || sees(vm, pawn, target) {
            if pawn != player_pawn {
                fire(vm, c, "SeePlayer", &mut [Value::Object(Some(player_pawn))]);
            }
        }
    }
}

/// True when `pawn` has `target` within its sight radius and field of view, with nothing of the level in between.
fn sees(vm: &mut Vm<'_>, pawn: ObjId, target: glam::Vec3) -> bool {
    let eye = vec(&get(vm, pawn, "Location")) + glam::Vec3::Z * EYE_HEIGHT;
    let to = target - eye;
    let dist = to.length();
    let radius = get(vm, pawn, "SightRadius").as_float();
    if dist < 1.0 || dist > radius {
        return false;
    }
    // `PeripheralVision` is the cosine of the half-angle of the field of view; the game's creatures that look all around
    // (a fire crab) have it at 1, which does not mean a field of no width.
    let peripheral = get(vm, pawn, "PeripheralVision").as_float();
    let rotation = get(vm, pawn, "Rotation");
    let yaw = rotation.fields().get(1).map(Value::as_int).unwrap_or(0);
    let yaw = yaw as f32 * std::f32::consts::TAU / 65536.0;
    let forward = glam::Vec3::new(yaw.cos(), yaw.sin(), 0.0);
    if peripheral < 0.99 && (to / dist).dot(forward) < peripheral {
        return false;
    }
    world(vm).collision.trace_static(eye, target, glam::Vec3::ZERO).is_none()
}
