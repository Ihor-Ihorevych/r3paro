//! What the engine tells an AI controller about the player: `SeePlayer` when the player's pawn is in its pawn's sight.
//!
//! The scripts of creatures (a fire crab's attack, for one) start from that event, which the engine raised in the
//! original game; a controller that does not define it is left alone.

use glam::Vec3;
use hp3_script::{ObjId, Value, Vm};

use crate::{fire, get, physics::{value, vec}, set, world};

/// How often (in ticks) each controller looks.
const LOOK_EVERY: u64 = 5;
/// The probe bit of the `SeePlayer` event in a state's mask (the engine's fixed event numbering: bit = name index - 300).
const SEE_PLAYER_PROBE: u32 = 40;

/// Head height above a pawn's location that sight is measured from.
const EYE_HEIGHT: f32 = 30.0;

/// Lets every AI controller whose pawn can see the player's pawn know it.
pub fn tick(vm: &mut Vm<'_>) {
    if world(vm).frame % LOOK_EVERY != 0 {
        return;
    }
    let Some(player_pawn) = world(vm).player.and_then(|pc| get(vm, pc, "Pawn").as_obj()) else { return };
    let debug = crate::envc::var_os("HP3_SEE_DEBUG").is_some();
    if get(vm, player_pawn, "Health").as_int() <= 0 || get(vm, player_pawn, "bHidden").as_bool() {
        if crate::envc::var_os("HP3_SEE_DEBUG").is_some() {
            eprintln!("see: player pawn health {} hidden {}", get(vm, player_pawn, "Health").as_int(), get(vm, player_pawn, "bHidden").as_bool());
        }
        return;
    }
    let Some(controller_class) = vm.db.find("Controller") else { return };
    let Some(see) = vm.db.names.find("SeePlayer") else { return };
    // The player's body is looked at in two places, its middle and its head: either one in the clear is enough.
    let (feet, height) = (vec(&get(vm, player_pawn, "Location")), get(vm, player_pawn, "CollisionHeight").as_float());
    let targets = [feet, feet + glam::Vec3::Z * height * 0.8];
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
        // The event reaches the controller only if its state probes it: the state's own and the class's probes, minus
        // what the state ignores (a crab in the middle of a flip is not told).
        if let Some(state) = vm.state_of(c) {
            let (probes, not_ignored) = (vm.db.get(state).state_info.0, vm.db.get(state).state_info.1);
            let mask = (probes | vm.db.get(class).state_info.0) & not_ignored;
            if crate::envc::var_os("HP3_SEE_DEBUG").is_some() {
                eprintln!("see: state {} mask {:#x}", vm.db.names.get(vm.db.get(state).name), mask);
            }
            if mask & (1u64 << SEE_PLAYER_PROBE) == 0 {
                continue;
            }
        }
        if pawn == player_pawn || targets.iter().any(|t| sees(vm, pawn, *t)) {
            if pawn != player_pawn {
                fire(vm, c, "SeePlayer", &mut [Value::Object(Some(player_pawn))]);
            }
        }
    }
}

/// True when `pawn` has `target` within its sight radius and field of view, with nothing of the level in between.
fn sees(vm: &mut Vm<'_>, pawn: ObjId, target: glam::Vec3) -> bool {
    let eye = vec(&get(vm, pawn, "Location")) + glam::Vec3::Z * (get(vm, pawn, "CollisionHeight").as_float() * 0.8).clamp(4.0, EYE_HEIGHT);
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

/// Whether nothing of the level lies on the line from `from` to `to`.
fn clear(vm: &mut Vm<'_>, from: Vec3, to: Vec3) -> bool {
    world(vm).collision.trace_static(from, to, Vec3::ZERO).is_none()
}

/// `Controller.LineOfSightTo`: whether the controller's pawn, from its eyes, sees any of `other`: its middle, then its
/// head, then the two middle side points of its box (only when it is wide enough next to the distance to matter).
/// Seeing the enemy updates the controller's last-seen record. Zone distance fog does not cut sight off here.
pub fn line_of_sight(vm: &mut Vm<'_>, c: ObjId, other: ObjId) -> bool {
    let pawn = get(vm, c, "Pawn").as_obj();
    let eye = match pawn {
        Some(p) => vec(&get(vm, p, "Location")) + Vec3::Z * get(vm, p, "BaseEyeHeight").as_float(),
        None => vec(&get(vm, c, "Location")),
    };
    let at = vec(&get(vm, other, "Location"));
    let radius = get(vm, other, "CollisionRadius").as_float();
    let dist_sq = (at - eye).length_squared().max(1.0);
    let is_enemy = get(vm, c, "Enemy").as_obj() == Some(other);
    if is_enemy {
        let head = at + Vec3::Z * get(vm, other, "BaseEyeHeight").as_float();
        if clear(vm, eye, at) || clear(vm, eye, head) {
            let now = get(vm, c, "Level").as_obj().map(|l| get(vm, l, "TimeSeconds")).unwrap_or(Value::Float(0.0));
            set(vm, c, "LastSeenTime", now);
            set(vm, c, "LastSeeingPos", value(eye));
            set(vm, c, "LastSeenPos", value(at));
            set(vm, c, "bEnemyInfoValid", Value::Bool(true));
            return true;
        }
        if radius * radius / dist_sq < 0.0001 {
            return false;
        }
    } else {
        if clear(vm, eye, at) {
            return true;
        }
        let other_is_pawn = vm.db.find("Pawn").zip(vm.db.objects.get(other).class).is_some_and(|(p, k)| vm.is_child_of(k, p));
        if dist_sq > 8000.0 * 8000.0 || (!other_is_pawn && dist_sq > 2000.0 * 2000.0) {
            return false;
        }
        let head = at + Vec3::Z * get(vm, other, "CollisionHeight").as_float() * 0.8;
        if clear(vm, eye, head) {
            return true;
        }
        if radius * radius / dist_sq < 0.00015 {
            return false;
        }
    }
    // The box's four corners less the nearest and the farthest (measured from the world origin, as the original does).
    let mut points = [
        at + Vec3::new(-radius, radius, 0.0),
        at + Vec3::new(radius, radius, 0.0),
        at + Vec3::new(-radius, -radius, 0.0),
        at + Vec3::new(radius, -radius, 0.0),
    ];
    points.sort_by(|a, b| a.length_squared().total_cmp(&b.length_squared()));
    clear(vm, eye, points[1]) || clear(vm, eye, points[2])
}

/// `Actor.PlayerCanSeeMe`: whether the local player could see the actor: near enough for its size, in front of the
/// view (or the view is behind the pawn), and in the clear from the player's controller. The view target itself
/// always counts.
fn player_can_see(vm: &mut Vm<'_>, a: ObjId) -> bool {
    let Some(pc) = world(vm).player else { return false };
    let Some(target) = get(vm, pc, "ViewTarget").as_obj().or_else(|| get(vm, pc, "Pawn").as_obj()) else { return false };
    if target == a {
        return true;
    }
    let to = vec(&get(vm, a, "Location")) - vec(&get(vm, target, "Location"));
    let dist_sq = to.length_squared();
    if dist_sq >= 100000.0 * (get(vm, a, "CollisionRadius").as_float() + 3.6) {
        return false;
    }
    if !get(vm, pc, "bBehindView").as_bool() {
        let rot = get(vm, pc, "Rotation");
        let r: Vec<i32> = rot.fields().iter().map(Value::as_int).collect();
        let axes = hp3_script::natives::rot_axes([r.first().copied().unwrap_or(0), r.get(1).copied().unwrap_or(0), r.get(2).copied().unwrap_or(0)]);
        let d = Vec3::from(axes[0]).dot(to);
        if d * d < 0.25 * dist_sq {
            return false;
        }
    }
    line_of_sight(vm, pc, a)
}

pub fn register(vm: &mut Vm<'_>) {
    vm.register("Controller.LineOfSightTo", |vm, this, a| {
        let (Some(c), Some(other)) = (this, a.first().and_then(Value::as_obj)) else { return Ok(Value::Bool(false)) };
        Ok(Value::Bool(line_of_sight(vm, c, other)))
    });
    vm.register("Actor.PlayerCanSeeMe", |vm, this, _a| Ok(Value::Bool(this.is_some_and(|a| player_can_see(vm, a)))));
}
