//! Pawns that follow a path of interpolation points (a cutscene's `FollowSpline`, the camera crane, the Patronus stag's run).
//!
//! The script (`KWPawnNative.FollowSplinePath`) spawns an `InterpolationManager` for the pawn, the manager's *owner*, and then
//! only watches it. The manager is what moves the owner: each tick it advances `PhysAlpha` along the segment ending at `Dest`
//! (at the owner's `IPSpeed`, or from the point's `PathDist` and `DesiredSpeed`), puts the owner on the cubic Bezier curve
//! between the previous point and `Dest` with a turn eased towards what the point wants, and raises `UpdateCamera`, the end
//! point's `InterpolateEnd` and `FinishedInterpolation` for the script. Without this the owner stood where it began, a stag
//! ran on the spot and the cutscene that waited for it never ended.
//!
//! Written from how the original behaves (the path points' properties, the events the scripts handle) and read against the
//! HP1/HP2 port of the same native; the curve and the speed maths are the same in both games.

use glam::Vec3;
use hp3_script::{ObjId, Value, Vm};

use crate::physics::{value, vec};
use crate::{fire, get, read, set, slot, world};

/// A set of axes (forward, right, up) and where they stand.
#[derive(Clone, Copy)]
struct Frame {
    x: Vec3,
    y: Vec3,
    z: Vec3,
    origin: Vec3,
}

type Rot = [i32; 3];

fn frame_of(rot: Rot, origin: Vec3) -> Frame {
    let a = hp3_script::natives::rot_axes(rot);
    Frame { x: Vec3::from(a[0]), y: Vec3::from(a[1]), z: Vec3::from(a[2]), origin }
}

/// Normalised unless it is too small to have a direction (as the original does it).
fn safe_normal(v: Vec3) -> Vec3 {
    let sq = v.length_squared();
    if sq >= 1e-8 { v / sq.sqrt() } else { v }
}

const TO_ANGLE: f32 = 32768.0 / std::f32::consts::PI;

/// The rotator (roll 0) that points along `v`.
fn rotator_of(v: Vec3) -> Rot {
    let yaw = (v.y.atan2(v.x) * TO_ANGLE) as i32;
    let pitch = (v.z.atan2((v.x * v.x + v.y * v.y).sqrt()) * TO_ANGLE) as i32;
    [pitch, yaw, 0]
}

/// The rotator of an orthonormal set of axes.
fn rotator_of_axes(x: Vec3, y: Vec3, z: Vec3) -> Rot {
    let yaw = (x.y.atan2(x.x) * TO_ANGLE) as i32;
    let pitch = (x.z.atan2((x.x * x.x + x.y * x.y).sqrt()) * TO_ANGLE) as i32;
    let flat = frame_of([pitch, yaw, 0], Vec3::ZERO);
    let roll = (flat.y.dot(z).atan2(flat.y.dot(y)) * TO_ANGLE) as i32;
    [pitch, yaw, roll]
}

fn rot_add(a: Rot, b: Rot) -> Rot {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn rot_sub(a: Rot, b: Rot) -> Rot {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn rot_scale(a: Rot, s: f32) -> Rot {
    a.map(|v| (v as f32 * s).round_ties_even() as i32)
}

fn bezier(p0: Vec3, c0: Vec3, c1: Vec3, p1: Vec3, t: f32) -> Vec3 {
    let u = 1.0 - t;
    p0 * (u * u * u) + c0 * (u * u * t * 3.0) + c1 * (u * t * t * 3.0) + p1 * (t * t * t)
}

fn float(vm: &mut Vm<'_>, o: ObjId, name: &str) -> f32 {
    get(vm, o, name).as_float()
}

fn flag(vm: &mut Vm<'_>, o: ObjId, name: &str) -> bool {
    get(vm, o, name).as_bool()
}

fn point(vm: &mut Vm<'_>, o: ObjId, name: &str) -> Vec3 {
    vec(&get(vm, o, name))
}

fn object(vm: &mut Vm<'_>, o: ObjId, name: &str) -> Option<ObjId> {
    get(vm, o, name).as_obj()
}

fn rotation(vm: &mut Vm<'_>, o: ObjId) -> Rot {
    crate::rd_rot(vm, o, "Rotation")
}

/// Element `i` of a static array property (its elements sit in consecutive slots).
fn element(vm: &mut Vm<'_>, o: ObjId, name: &str, i: usize) -> Value {
    slot(vm, o, name).map_or(Value::Empty, |s| read(vm, o, Some(s + i)).clone())
}

fn pause_of(vm: &mut Vm<'_>, p: ObjId, i: i32) -> f32 {
    element(vm, p, "Pause", i.clamp(0, 7) as usize).as_float()
}

/// The first actor in the level with that tag.
fn find_by_tag(vm: &mut Vm<'_>, tag: hp3_script::Name) -> Option<ObjId> {
    let actors = world(vm).actors.clone();
    actors.into_iter().find(|&a| !vm.db.objects.get(a).deleted && get(vm, a, "Tag").as_name() == tag)
}

/// What a point looks at while paused at pause `n`: the actor tagged by `ViewTargetTag[n]`, remembered in `ViewTarget[n]`.
fn view_target(vm: &mut Vm<'_>, p: ObjId, n: i32) -> Option<ObjId> {
    let n = n.clamp(0, 7) as usize;
    let tag = element(vm, p, "ViewTargetTag", n).as_name();
    if tag == hp3_script::Name::NONE {
        return None;
    }
    let cached = element(vm, p, "ViewTarget", n).as_obj();
    if let Some(c) = cached
        && get(vm, c, "Tag").as_name() == tag
    {
        return Some(c);
    }
    let found = find_by_tag(vm, tag);
    if let (Some(f), Some(s)) = (found, slot(vm, p, "ViewTarget")) {
        if let Some(v) = vm.db.objects.values_mut(p).and_then(|v| v.get_mut(s + n)) {
            *v = Value::Object(Some(f));
        }
    }
    found.or(cached)
}

fn desired_rotation_at_pause(vm: &mut Vm<'_>, p: ObjId, n: i32, from: Vec3) -> Rot {
    match view_target(vm, p, n) {
        Some(t) => rotator_of(vec(&get(vm, t, "Location")) - from),
        None => rotation(vm, p),
    }
}

/// The rotation wanted `offset` pauses before (negative) or after (positive) pause `n` of a point, walking into the
/// neighbouring points: what the turn is smoothed between.
fn desired_rotation_at_position(vm: &mut Vm<'_>, p: ObjId, offset: i32, n: i32, from: Vec3, depth: u32) -> Rot {
    if depth > 64 {
        return desired_rotation_at_pause(vm, p, n, from);
    }
    if offset < 0 {
        if n > 0 {
            return desired_rotation_at_position(vm, p, offset + 1, n - 1, from, depth + 1);
        }
        let prev = object(vm, p, "Prev");
        match prev {
            Some(prev) if !flag(vm, p, "bInstantNextPath") => {
                let mut i = 0;
                while i < 7 && pause_of(vm, prev, i) > 0.0 {
                    i += 1;
                }
                desired_rotation_at_position(vm, prev, offset + 1, i, from, depth + 1)
            }
            _ => desired_rotation_at_pause(vm, p, n, from),
        }
    } else if offset == 0 {
        desired_rotation_at_pause(vm, p, n, from)
    } else {
        let (mut q, mut left) = (n, offset);
        while q + 1 < 8 && pause_of(vm, p, q + 1) > 0.0 && left > 0 {
            q += 1;
            left -= 1;
        }
        match object(vm, p, "Next") {
            Some(next) if left != 0 && !flag(vm, next, "bInstantNextPath") => desired_rotation_at_position(vm, next, left, 0, from, depth + 1),
            _ => desired_rotation_at_pause(vm, p, q, from),
        }
    }
}

/// Where the owner is on the segment that ends at `p`, `t` of the way from `start`, and the orientation wanted there.
fn interpolated_position(vm: &mut Vm<'_>, p: ObjId, start: Frame, start_control: Vec3, t: f32, pause: i32) -> Frame {
    let here = point(vm, p, "Location");
    let prev_rotation = rotation(vm, p);
    if flag(vm, p, "bInstantNextPath") {
        return frame_of(prev_rotation, here);
    }
    let p0 = start.origin;
    let c0 = start.origin + start_control;
    let c1 = here + point(vm, p, "EndControlPoint");
    let pos = bezier(p0, c0, c1, here, t);
    let mut out;
    if flag(vm, p, "bFaceMoveDirection") {
        let t0 = t - 0.001;
        out = if t0 > 0.0 {
            frame_of(rotator_of(pos - bezier(p0, c0, c1, here, t0)), Vec3::ZERO)
        } else {
            let r = object(vm, p, "Prev").map_or(prev_rotation, |q| rotation(vm, q));
            frame_of(r, Vec3::ZERO)
        };
    } else if flag(vm, p, "bNewRotationSmoothing") {
        // Catmull-Rom over the wanted rotations around the segment.
        let rm2 = desired_rotation_at_position(vm, p, -2, pause, pos, 0);
        let rm1 = desired_rotation_at_position(vm, p, -1, pause, pos, 0);
        let r0 = desired_rotation_at_position(vm, p, 0, pause, pos, 0);
        let r1 = desired_rotation_at_position(vm, p, 1, pause, pos, 0);
        let (t2, t3) = (t * t, t * t * t);
        let m1 = rot_scale(rot_sub(r1, rm1), (t3 - t2) * 0.5);
        let m0 = rot_scale(rot_sub(r0, rm2), (t3 - 2.0 * t2 + t) * 0.5);
        let h1 = rot_scale(r0, 3.0 * t2 - 2.0 * t3);
        let h0 = rot_scale(rm1, 2.0 * t3 - 3.0 * t2 + 1.0);
        out = frame_of(rot_add(rot_add(h0, h1), rot_add(m0, m1)), Vec3::ZERO);
    } else if element(vm, p, "ViewTargetTag", pause.clamp(0, 7) as usize).as_name() == hp3_script::Name::NONE {
        // Blend the start axes into this point's; bTurnChange eases in faster.
        let w = if flag(vm, p, "bTurnChange") { t.sqrt() } else { t };
        let d = frame_of(prev_rotation, Vec3::ZERO);
        let x = safe_normal(d.x * w + start.x * (1.0 - t));
        let z = d.z * w + start.z * (1.0 - t);
        let y = safe_normal(z.cross(x));
        let z = safe_normal(x.cross(y));
        out = Frame { x, y, z, origin: Vec3::ZERO };
    } else {
        out = match view_target(vm, p, pause) {
            Some(target) => frame_of(rotator_of(vec(&get(vm, target, "Location")) - pos), Vec3::ZERO),
            None => start,
        };
    }
    out.origin = pos;
    out
}

/// One tick of an interpolation manager's physics: moves its owner along the path.
pub fn step(vm: &mut Vm<'_>, mgr: ObjId, dt: f32) {
    if dt <= 0.0 {
        return;
    }
    let Some(owner) = object(vm, mgr, "Owner").filter(|&o| !vm.db.objects.get(o).deleted) else {
        crate::destroy_actor(vm, mgr);
        return;
    };
    let ip_speed_slot = slot(vm, owner, "IPSpeed");
    let start_location = vec(&get(vm, owner, "Location"));
    let start_velocity = vec(&get(vm, owner, "Velocity"));
    let mut ip_speed = 0.0f32;
    let mut time_left = dt;
    let mut desired = frame_of(rotation(vm, owner), start_location);

    // The turn rates and the previous wanted axes are the manager's own, kept between ticks.
    let mut guard = 0;
    while float(vm, mgr, "PhysRate") != 0.0 && flag(vm, owner, "bInterpolating") && time_left > 0.0 && !vm.db.objects.get(mgr).deleted {
        guard += 1;
        if guard > 16 {
            break;
        }
        let mut speed_scale = 1.0f32;
        let old_location = vec(&get(vm, owner, "Location"));
        let owner_speed = read(vm, owner, ip_speed_slot).as_float();
        let mut dest = object(vm, mgr, "Dest");
        let mut reached_end = false;
        let mut new_alpha = float(vm, mgr, "PhysAlpha");
        let old_alpha = new_alpha;
        let mut move_time = time_left;

        if let Some(d) = dest {
            if owner_speed == 0.0 {
                if let Some(next) = object(vm, d, "Next") {
                    let desired_speed = float(vm, d, "DesiredSpeed");
                    let speed = if desired_speed > 0.0 { desired_speed } else { vec(&get(vm, owner, "Velocity")).length() };
                    let next_speed = float(vm, next, "DesiredSpeed");
                    ip_speed = if next_speed > 0.0 {
                        (1.0 - old_alpha) * speed + next_speed * old_alpha
                    } else if desired_speed > 0.0 {
                        desired_speed
                    } else {
                        0.0
                    };
                }
            } else {
                ip_speed = owner_speed;
            }
            if ip_speed <= 0.0 {
                // No speed: scale by how the control points' lengths change along the segment.
                let start_len = point(vm, d, "StartControlPoint").length();
                if start_len > 0.0 {
                    speed_scale = (point(vm, d, "EndControlPoint").length() / start_len - 1.0) * old_alpha + 1.0;
                }
            } else {
                let path_dist = float(vm, d, "PathDist");
                if path_dist > 0.0 {
                    speed_scale = ip_speed / path_dist;
                }
            }

            let rate = float(vm, mgr, "PhysRate");
            let step = speed_scale * rate * time_left;
            new_alpha = step + old_alpha;
            set(vm, mgr, "PhysAlpha", Value::Float(new_alpha.clamp(0.0, 1.0)));
            if new_alpha > 1.0 && float(vm, mgr, "RemainingPause") <= 0.0 {
                move_time = step - new_alpha + 1.0;
            }
            let mut location = vec(&get(vm, owner, "Location"));
            let pause_num = get(vm, mgr, "PauseNum").as_int();

            if float(vm, mgr, "RemainingPause") <= 0.0 {
                fire(vm, mgr, "UpdateCamera", &mut [Value::Float(old_alpha)]);
                dest = object(vm, mgr, "Dest");
                let Some(d) = dest.filter(|_| !vm.db.objects.get(mgr).deleted) else { break };
                let (start, start_control) = match object(vm, d, "Prev") {
                    Some(prev) => (frame_of(rotation(vm, prev), point(vm, prev, "Location")), point(vm, prev, "StartControlPoint")),
                    None => (frame_of(rotation(vm, d), vec(&get(vm, owner, "Location"))), Vec3::ZERO),
                };
                let alpha = float(vm, mgr, "PhysAlpha");
                desired = interpolated_position(vm, d, start, start_control, alpha, pause_num);
                location = desired.origin;

                if flag(vm, d, "bConstantSpeed") && ip_speed > 0.0 && alpha < 1.0 && !flag(vm, d, "bInstantNextPath") {
                    // Correct the alpha once, so that the speed along the curve is the wanted one.
                    let actual = (desired.origin - old_location).length() / move_time;
                    if (actual - ip_speed).abs() > ip_speed * 0.05 {
                        new_alpha = move_time * rate * speed_scale * ip_speed / actual + old_alpha;
                        set(vm, mgr, "PhysAlpha", Value::Float(new_alpha.clamp(0.0, 1.0)));
                        let alpha = float(vm, mgr, "PhysAlpha");
                        desired = interpolated_position(vm, d, start, start_control, alpha, pause_num);
                        location = desired.origin;
                        if new_alpha > 1.0 && float(vm, mgr, "RemainingPause") <= 0.0 {
                            move_time = speed_scale * rate * time_left - new_alpha + 1.0;
                        }
                    }
                }
                if flag(vm, d, "bFaceMoveDirection") {
                    desired = frame_of(rotator_of(desired.origin - vec(&get(vm, owner, "Location"))), location);
                }
            } else {
                // Paused at the point: it stands still and looks where the pause says.
                ip_speed = 0.0;
                set(vm, mgr, "PhysAlpha", Value::Float(0.0));
                new_alpha = 0.0;
                let remaining = float(vm, mgr, "RemainingPause") - time_left;
                if remaining < 0.0 {
                    set(vm, mgr, "RemainingPause", Value::Float(0.0));
                    time_left = 0.0;
                    reached_end = true;
                } else {
                    set(vm, mgr, "RemainingPause", Value::Float(remaining));
                }
                let (remaining_now, start_pause) = (float(vm, mgr, "RemainingPause"), float(vm, mgr, "StartPause"));
                fire(vm, mgr, "UpdateCamera", &mut [Value::Float((start_pause - remaining_now) / start_pause)]);
                dest = object(vm, mgr, "Dest");
                let Some(d) = dest.filter(|_| !vm.db.objects.get(mgr).deleted) else { break };
                let pause_now = get(vm, mgr, "PauseNum").as_int();
                if let Some(target) = view_target(vm, d, pause_now) {
                    desired = frame_of(rotator_of(vec(&get(vm, target, "Location")) - vec(&get(vm, owner, "Location"))), Vec3::ZERO);
                }
                desired.origin = vec(&get(vm, owner, "Location"));
                location = desired.origin;
            }

            let Some(d) = dest else { break };
            if !flag(vm, d, "bNewRotationSmoothing") {
                // The turn is smoothed: the axes follow a turn rate that eases towards the wanted one (the previous point's
                // `Smoothing`: 0 means none, larger is slower).
                let current = frame_of(rotation(vm, owner), Vec3::ZERO);
                let inverse = 1.0 / move_time.max(1e-6);
                let mut rate_x = (desired.x - point(vm, mgr, "OldDesiredX")) * inverse;
                let mut rate_z = (desired.z - point(vm, mgr, "OldDesiredZ")) * inverse;
                set(vm, mgr, "OldDesiredX", value(desired.x));
                set(vm, mgr, "OldDesiredZ", value(desired.z));
                let k = match object(vm, d, "Prev") {
                    Some(prev) => {
                        let s = float(vm, prev, "Smoothing");
                        if s == 0.0 { 0.0 } else { 1.0 / s }
                    }
                    None => 1.0,
                };
                if k != 0.0 {
                    let need_x = (desired.x - current.x) * (1.0 / k);
                    let need_z = (desired.z - current.z) * (1.0 / k);
                    if need_x.dot(rate_x) <= 0.0 {
                        rate_x = need_x;
                    }
                    if need_z.dot(rate_z) <= 0.0 {
                        rate_z = need_z;
                    }
                    let a = k * move_time;
                    let turn_x = point(vm, mgr, "TurnRateX") * (1.0 - a) + rate_x * a;
                    let turn_z = point(vm, mgr, "TurnRateZ") * (1.0 - a) + rate_z * a;
                    set(vm, mgr, "TurnRateX", value(turn_x));
                    set(vm, mgr, "TurnRateZ", value(turn_z));
                    desired.x = safe_normal(current.x + turn_x * move_time);
                    desired.z = safe_normal(current.z + turn_z * move_time);
                    desired.y = safe_normal(desired.z.cross(desired.x));
                    desired.z = safe_normal(desired.x.cross(desired.y));
                }
                set(vm, mgr, "TurnRateX", value((desired.x - current.x) * inverse));
                set(vm, mgr, "TurnRateZ", value((desired.z - current.z) * inverse));
            }

            // Move the owner (and turn it) to where the curve put it.
            let new_rotation = rotator_of_axes(desired.x, desired.y, desired.z);
            set(vm, owner, "Rotation", Value::Struct(new_rotation.map(Value::Int).to_vec().into()));
            set(vm, owner, "Location", value(location));
            crate::touch::settle(vm, owner);
        } else {
            reached_end = true;
        }

        let rate = float(vm, mgr, "PhysRate");
        let forward;
        if rate <= 0.0 {
            forward = false;
            if new_alpha < 0.0 {
                time_left = -new_alpha / (old_alpha - new_alpha) * time_left;
                set(vm, mgr, "PhysAlpha", Value::Float(1.0));
                reached_end = true;
            }
        } else {
            forward = true;
            if new_alpha > 1.0 {
                time_left = (new_alpha - 1.0) / (new_alpha - old_alpha) * time_left;
                set(vm, mgr, "PhysAlpha", Value::Float(0.0));
                reached_end = true;
            }
        }
        if !reached_end {
            time_left = 0.0;
            continue;
        }

        match object(vm, mgr, "Dest") {
            Some(end) => {
                // HP3's points handle it as `KWInterpolateEnd` (the UT2004 name, `InterpolateEnd`, does not exist there).
                let event = if crate::envc::var_os("HP3_OLD_INTERP_END").is_some() { "InterpolateEnd" } else { "KWInterpolateEnd" };
                fire(vm, end, event, &mut [Value::Object(Some(mgr)), Value::Bool(forward)]);
            }
            None => {
                fire(vm, mgr, "FinishedInterpolation", &mut [Value::Object(None)]);
            }
        }
        let mut old_location = old_location;
        if flag(vm, mgr, "bInstantMove") {
            // The owner was moved at once: its speed is measured from there.
            set(vm, mgr, "bInstantMove", Value::Bool(false));
            old_location = vec(&get(vm, owner, "Location"));
        }
        let velocity = (vec(&get(vm, owner, "Location")) - old_location) / dt;
        set(vm, owner, "Velocity", value(velocity));
        if velocity.length() >= ip_speed {
            time_left = 0.0;
        }
    }

    // The frame's speed is capped at 105% of IPSpeed (the alpha correction can overshoot on a tight curve).
    let end = vec(&get(vm, owner, "Location"));
    let mut velocity = (end - start_location) / dt;
    let speed = velocity.length();
    if ip_speed > 0.0 && ip_speed * 1.05 < speed {
        let target = start_location + (end - start_location) * (ip_speed / speed);
        set(vm, owner, "Location", value(target));
        crate::touch::settle(vm, owner);
        velocity = (target - start_location) / dt;
    }
    set(vm, owner, "Velocity", value(velocity));
    set(vm, owner, "Acceleration", value((velocity - start_velocity) / dt));
}
