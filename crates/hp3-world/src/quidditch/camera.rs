//! The chase camera that watches Harry fly. Pure: a `BroomState` and the camera of the frame before go in, the
//! camera of this frame comes out, which `quidditch::glue` keeps in the match and `player_view` hands the
//! renderer while a match is running.
//!
//! It is the orbit camera of AdamJD's HP1 "Movement mod" (`docs/formats/hp1-broom-movement-mod.md`): a short
//! arm from a point above the rider, aimed along the rider's *look* direction (`BroomState::want_yaw` /
//! `want_pitch`) rather than along the broom's heading, so steering is camera-relative and the broom is seen
//! turning into the look. All the softness is in the rotation -- it closes part of the gap to the look each
//! frame -- and none in the position, which sits exactly on the arm. Looking up from below pulls the camera in
//! towards the rider, and the arm is shortened so the camera never leaves the flight volume.

use glam::Vec3;

use super::arena::{self, ArenaBounds};
use super::flight::{wrap_pi, BroomState};

/// Where the camera is, where it looks (Unreal's (pitch, yaw, roll) in 65536ths of a turn), how wide its view
/// is (horizontal degrees) -- which is what `player_view` returns -- and how long its arm is this frame, the
/// one piece of camera state that is not in the rotation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CamState {
    pub pos: Vec3,
    pub rot: [i32; 3],
    pub fov: f32,
    /// The arm the camera hangs on (uu): `ARM`, shortened by the pull-in and by the arena wall.
    pub arm: f32,
}

/// How far behind the look-at point the camera sits (uu). The mod's `fLookAtDistance` is 128; **ours is 200**,
/// deliberately, because at 128 the rider fills the middle of the screen and blocks the view ahead. The whole
/// rig is scaled with it (`LOOK_UP` below), so only the size of Harry in the frame changes.
const ARM: f32 = 200.0;
/// It looks at a point this far above the broom (uu): the mod's `vLookAtOffset.Z` of 55, scaled by the same
/// 200/128 as the arm, so the rider keeps the place in the picture the mod's rig gives him.
const LOOK_UP: f32 = 86.0;
/// How much of the gap to the rider's look direction the camera's own rotation closes per second (1/s, an
/// exponential approach): the mod's `fRotTightness`, a 1/8 s time constant.
const ROT_TIGHT: f32 = 8.0;
/// and the most it may turn in one step (rad): the mod's +-20000 / +-10000 rotation units.
const MAX_YAW_STEP: f32 = 1.917;
const MAX_PITCH_STEP: f32 = 0.959;
/// The camera's own pitch limit (rad): the mod's +-14000 units, 76.9 deg, which keeps it out of the ground and
/// off the top of Harry's head.
const MAX_CAM_PITCH: f32 = 1.342;
/// The pull-in: a camera looking up from below shortens its arm by `1 - pitch / PULL_SPREAD` (the mod's
/// `1 - pitch/10000` in rotation units), never below `MIN_SCALE` of it.
///
/// `MIN_SCALE` is **0.3, not the mod's 0.15**, and the pull-in scales the whole rig (the look-at offset with
/// the arm, see `chase_camera`), which the mod does not: shortening the arm alone while still aiming `LOOK_UP`
/// over the rider swings him off the bottom of the screen as soon as the arm is halved, and at 0.15 of the arm
/// the camera would be inside him. Scaled, the rider keeps the same place in the picture whatever the arm is.
const PULL_SPREAD: f32 = 0.9587;
const MIN_SCALE: f32 = 0.3;
/// The arm grows back at this rate (1/s, exponential): the mod's `fMoveBackTightness`. Shrinking is immediate.
const BACK_TIGHT: f32 = 2.5;
/// How far short of a wall the camera stops (uu): the mod's trace cushion.
const CUSHION: f32 = 5.0;
/// Field of view at rest (horizontal degrees), and how much wider it opens at `FAST` (so speed reads). Ours,
/// not the mod's -- neither HP1 package changes the field of view with speed.
const FOV: f32 = 90.0;
const FOV_SPEED: f32 = 10.0;
const FAST: f32 = 1200.0;

/// Radians to Unreal's rotation units.
const UU: f32 = 65536.0 / std::f32::consts::TAU;

/// A pitch and a yaw in radians as an Unreal rotator (pitch, yaw, roll), the yaw folded into one turn.
pub fn rotator(pitch: f32, yaw: f32) -> [i32; 3] {
    let unit = |a: f32| if a.is_finite() { (a * UU).round().rem_euclid(65536.0) as i32 } else { 0 };
    [unit(pitch), unit(yaw), 0]
}

/// The camera as it starts: straight behind the broom on a full arm, nothing to lag behind yet.
pub fn behind(s: &BroomState) -> CamState {
    let seed = CamState { pos: s.pos, rot: rotator(s.want_pitch, s.want_yaw), fov: FOV, arm: ARM };
    chase_camera(s, &seed, 0.0)
}

/// How near the turf the camera may come (uu over `arena::GROUND_Z`).
const FLOOR_CLEARANCE: f32 = 30.0;

/// The volume the camera is kept in. Its walls are the flight volume's own (the stands start there, and a
/// camera in the stands looks at the back of a wall). Its floor is the turf rather than the flight floor
/// 150 uu over it, and its top is the rig's own height over the flight ceiling: there is only sky up there,
/// and a camera held under the flight ceiling would have nowhere to be while the broom skims it and the rider
/// looks down -- it would end up inside his head.
fn volume() -> ArenaBounds {
    let b = arena::bounds();
    let floor = (arena::GROUND_Z + FLOOR_CLEARANCE).min(b.center.z - b.half.z);
    ArenaBounds {
        center: b.center,
        half: Vec3::new(b.half.x, b.half.y, b.center.z - floor),
        ceiling: b.ceiling + ARM + LOOK_UP,
    }
}

/// How far the camera may go from `from` along `dir` before it leaves `b`, less the cushion: the mod traces
/// look-at to camera against the world and pulls the camera in to the first hit. The arena is all the
/// collision a match has, so the wall it stops at is the camera volume's (`volume`).
fn room_along(from: Vec3, dir: Vec3, b: &ArenaBounds) -> f32 {
    if !from.is_finite() || !dir.is_finite() {
        return ARM;
    }
    let lo = Vec3::new(b.center.x - b.half.x, b.center.y - b.half.y, b.center.z - b.half.z);
    let hi = Vec3::new(b.center.x + b.half.x, b.center.y + b.half.y, b.ceiling);
    let mut hit = f32::INFINITY;
    for i in 0..3 {
        if dir[i].abs() > 1e-6 {
            let face = if dir[i] > 0.0 { hi[i] } else { lo[i] };
            let t = (face - from[i]) / dir[i];
            if t >= 0.0 {
                hit = hit.min(t);
            }
        }
    }
    (hit - CUSHION).clamp(0.0, ARM)
}

/// This frame's camera, from the broom and the camera of the frame before.
pub fn chase_camera(s: &BroomState, prev: &CamState, dt: f32) -> CamState {
    let dt = if dt.is_finite() { dt.clamp(0.0, 0.25) } else { 0.0 };
    let finite = |v: f32, or: f32| if v.is_finite() { v } else { or };
    // Where the rider is looking is where the camera wants to point (camera-relative steering: the broom then
    // turns into the camera, not the other way round).
    let was_yaw = wrap_pi(prev.rot[1] as f32 / UU);
    let was_pitch = wrap_pi(prev.rot[0] as f32 / UU);
    let want_yaw = finite(s.want_yaw, was_yaw);
    let want_pitch = finite(s.want_pitch, 0.0).clamp(-MAX_CAM_PITCH, MAX_CAM_PITCH);
    // Part of the gap each frame, and never more than one bounded step: a broom whipped round half a turn
    // does not throw the picture round with it.
    let blend = (ROT_TIGHT * dt).clamp(0.0, 1.0);
    let yaw = was_yaw + (wrap_pi(want_yaw - was_yaw) * blend).clamp(-MAX_YAW_STEP, MAX_YAW_STEP);
    let pitch = (was_pitch + (wrap_pi(want_pitch - was_pitch) * blend).clamp(-MAX_PITCH_STEP, MAX_PITCH_STEP))
        .clamp(-MAX_CAM_PITCH, MAX_CAM_PITCH);
    // The arm: the pull-in while the camera looks up from below (immediate), easing back out when it does not.
    let want_arm = if pitch > 0.0 { ARM * (1.0 - pitch / PULL_SPREAD).max(MIN_SCALE) } else { ARM };
    let was_arm = finite(prev.arm, ARM).clamp(ARM * MIN_SCALE, ARM);
    let arm = if want_arm <= was_arm {
        want_arm
    } else {
        was_arm + (want_arm - was_arm) * (1.0 - (-BACK_TIGHT * dt).exp())
    };
    // The look-at point, a little over the rider, and the camera exactly `arm` back along its own facing --
    // shortened so it stops short of the arena wall instead of hanging in the stands. The height over the
    // rider shrinks with the arm, so he keeps his place in the picture however far the camera is pulled in.
    let rider = if s.pos.is_finite() { s.pos } else { prev.pos };
    let rider = if rider.is_finite() { rider } else { Vec3::ZERO };
    let fwd = Vec3::new(pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), pitch.sin());
    //
    // The whole rig is confined, not only the arm: the look-at point is itself kept inside the volume (a ray
    // cast from outside it finds no wall to stop at), the arm is measured twice (shortening it lowers the look-at point, which moves the ray), and whatever
    // rounding is left is clamped away, so the camera is never outside the walls, under the floor or over the
    // top whatever the broom and the look are doing.
    let bounds = volume();
    let look_at = |arm: f32| arena::clamp_into(&bounds, rider + Vec3::Z * LOOK_UP * (arm / ARM));
    let arm = arm.min(room_along(look_at(arm), -fwd, &bounds));
    let arm = arm.min(room_along(look_at(arm), -fwd, &bounds));
    let at = look_at(arm);
    let speed = if s.vel.is_finite() { s.vel.length() } else { 0.0 };
    CamState {
        pos: arena::clamp_into(&bounds, at - fwd * arm),
        rot: rotator(pitch, yaw),
        fov: FOV + (speed / FAST).clamp(0.0, 1.0) * FOV_SPEED,
        arm,
    }
}

#[cfg(test)]
mod tests {
    use super::super::flight::{self, BroomInput, BroomTuning};
    use super::*;

    fn flying() -> BroomState {
        BroomState::new(Vec3::new(100.0, -200.0, 500.0), Vec3::ZERO, 0.4, 0.0)
    }

    /// The camera's own yaw and pitch, as `chase_camera` left them.
    fn cam_yaw(c: &CamState) -> f32 {
        wrap_pi(c.rot[1] as f32 / UU)
    }

    fn cam_pitch(c: &CamState) -> f32 {
        wrap_pi(c.rot[0] as f32 / UU)
    }

    #[test]
    fn chase_camera_stays_behind_and_above_harry() {
        let t = BroomTuning::default();
        let mut s = flying();
        let mut c = behind(&s);
        // A turn, a climb and a dive, all at once, for ten seconds, flown with the mouse.
        for n in 0..600 {
            let i = BroomInput {
                look_yaw: 0.01,
                look_pitch: ((n as f32) / 30.0).sin() * 0.01,
                boost: n % 120 < 60,
                ..BroomInput::default()
            };
            // Confined as the match confines it: the camera is, so a broom let out of the volume would be
            // watched from inside it and none of the below would hold.
            s = flight::confine(&flight::step(&s, &i, &t, 1.0 / 60.0), &arena::bounds());
            c = chase_camera(&s, &c, 1.0 / 60.0);
            assert!(c.pos.is_finite(), "{:?}", c.pos);
            // Behind: the way from the camera to Harry points the way the camera looks.
            let yaw = cam_yaw(&c);
            let pitch = cam_pitch(&c);
            let ahead = Vec3::new(yaw.cos(), yaw.sin(), 0.0);
            assert!((s.pos - c.pos).dot(ahead) > 0.0, "the broom at {:?} is not ahead of the camera at {:?}", s.pos, c.pos);
            // Above him too, except while he is looking up, which is the one thing that puts the camera low
            // (that is what the pull-in is for).
            if pitch < 0.4 {
                assert!(c.pos.z > s.pos.z, "camera at {:?} is not above the broom at {:?}", c.pos, s.pos);
            }
            // Close, but never closer than the pull-in's floor (so never inside the rider) and never farther
            // than the whole rig.
            let d = (c.pos - s.pos).length();
            let full = Vec3::new(ARM, 0.0, LOOK_UP).length();
            // (Looking down, the arm and the height over the rider add up: the ceiling levels the broom out in
            // this run and the look then dips below level.)
            let reach = if pitch < 0.0 { ARM + LOOK_UP } else { full };
            assert!(d <= reach + 1e-2, "{d} is farther than the arm");
            assert!(d >= full * MIN_SCALE - CUSHION - 1e-2, "{d} is inside the rider");
            // And it looks at Harry: it aims exactly at the point over him, and he is well inside the view.
            assert!(pitch.abs() <= MAX_CAM_PITCH + 1e-3, "camera pitch {pitch}");
            let look = Vec3::new(pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), pitch.sin());
            let to_at = arena::clamp_into(&volume(), s.pos + Vec3::Z * LOOK_UP * (c.arm / ARM)) - c.pos;
            assert!(to_at.normalize().dot(look) > 0.999, "the camera does not look at the point over the broom");
            assert!((s.pos - c.pos).normalize().dot(look) > 0.88, "the broom is out of the picture");
            assert!(c.fov >= FOV && c.fov <= FOV + FOV_SPEED, "fov {}", c.fov);
        }
    }

    #[test]
    fn chase_camera_smooths_a_sudden_turn() {
        let dt = 1.0 / 60.0;
        let s = flying();
        let mut c = behind(&s);
        // The rider flicks the mouse: his look jumps most of a half turn in one frame.
        let spun = BroomState { want_yaw: wrap_pi(s.want_yaw + std::f32::consts::PI * 0.9), ..s };
        // The bound on one step: part of the gap, and never more than the per-step clamp.
        let bound = std::f32::consts::PI * (ROT_TIGHT * dt).min(1.0) + 1e-4;
        let first = chase_camera(&spun, &c, dt);
        assert!(wrap_pi(cam_yaw(&first) - cam_yaw(&c)).abs() <= bound, "the camera jumped with him");
        // Every step is bounded, and the camera does catch up.
        for _ in 0..240 {
            let next = chase_camera(&spun, &c, dt);
            let step = wrap_pi(cam_yaw(&next) - cam_yaw(&c)).abs();
            assert!(step <= bound.min(MAX_YAW_STEP + 1e-4), "step {step} over the bound");
            c = next;
        }
        assert!(wrap_pi(cam_yaw(&c) - spun.want_yaw).abs() < 0.01, "the camera never caught up: {} vs {}", cam_yaw(&c), spun.want_yaw);
    }

    /// Looking up from below pulls the camera in towards Harry, and letting go eases it back out.
    #[test]
    fn looking_up_pulls_the_camera_in_and_it_eases_back_out() {
        let dt = 1.0 / 60.0;
        let up = BroomState { want_pitch: 1.2, ..flying() };
        let mut c = behind(&flying());
        for _ in 0..120 {
            c = chase_camera(&up, &c, dt);
        }
        assert!(c.arm < ARM * 0.5, "the camera did not pull in: arm {}", c.arm);
        assert!(c.arm >= ARM * MIN_SCALE - 1e-3, "{} is past the floor", c.arm);
        let close = c.arm;
        // Level again: the arm grows back, and not all at once (half a second in it is still on the way).
        for _ in 0..30 {
            c = chase_camera(&flying(), &c, dt);
        }
        assert!(c.arm > close && c.arm < ARM * 0.95, "arm {} -> {} is not a gradual recovery", close, c.arm);
        for _ in 0..240 {
            c = chase_camera(&flying(), &c, dt);
        }
        assert!((c.arm - ARM).abs() < 1.0, "the arm never came back: {}", c.arm);
    }

    #[test]
    fn chase_camera_survives_rubbish() {
        let bad = BroomState {
            pos: Vec3::splat(f32::NAN),
            vel: Vec3::splat(f32::INFINITY),
            yaw: f32::NAN,
            pitch: f32::NAN,
            want_yaw: f32::NAN,
            want_pitch: f32::NAN,
        };
        let c = chase_camera(&bad, &behind(&flying()), f32::NAN);
        assert!(c.pos.is_finite() && c.fov.is_finite() && c.arm.is_finite(), "{c:?}");
        let worse = chase_camera(&flying(), &CamState { pos: Vec3::splat(f32::NAN), rot: [0, 0, 0], fov: f32::NAN, arm: f32::NAN }, 1.0 / 60.0);
        assert!(worse.pos.is_finite() && worse.arm.is_finite(), "{worse:?}");
    }

    /// Wherever the broom is in the flight volume -- on a wall, in a corner, on the floor, under the ceiling --
    /// and wherever the rider looks, the camera stays inside the stadium: never past a wall of the flight
    /// volume (the stands), never under the turf, and no higher over the flight ceiling than its own rig.
    #[test]
    fn chase_camera_never_leaves_the_flight_volume() {
        let b = arena::bounds();
        let lo = Vec3::new(b.center.x - b.half.x, b.center.y - b.half.y, b.center.z - b.half.z);
        let hi = Vec3::new(b.center.x + b.half.x, b.center.y + b.half.y, b.ceiling);
        let at = |f: f32, a: f32, z: f32| a + (z - a) * f;
        // What the camera is held to: the flight volume's walls, the turf, and the rig's height of sky.
        let cam_lo = Vec3::new(lo.x, lo.y, arena::GROUND_Z + FLOOR_CLEARANCE);
        let cam_hi = Vec3::new(hi.x, hi.y, hi.z + ARM + LOOK_UP);
        assert!(cam_lo.z > arena::GROUND_Z && cam_lo.z <= lo.z);
        // Skimming the ceiling and looking a little down -- the commonest thing a player does up there -- the
        // camera keeps its whole arm instead of collapsing into the rider.
        let top = BroomState::new(Vec3::new(b.center.x, b.center.y, b.ceiling), Vec3::ZERO, 0.0, -0.3);
        let c = chase_camera(&top, &behind(&top), 1.0 / 60.0);
        assert!((c.arm - ARM).abs() < 1e-3 && c.pos.distance(top.pos) > ARM * 0.9, "{c:?}");
        let (lo, hi) = (cam_lo, cam_hi);
        for fx in [0.0, 0.003, 0.5, 0.997, 1.0] {
            for fy in [0.0, 0.003, 0.5, 0.997, 1.0] {
                for fz in [0.0, 0.01, 0.5, 0.99, 1.0] {
                    let pos = Vec3::new(at(fx, lo.x, hi.x), at(fy, lo.y, hi.y), at(fz, lo.z, hi.z));
                    for yaw_n in 0..16 {
                        for pitch in [-1.342, -0.7, 0.0, 0.3, 0.7, 1.342] {
                            let yaw = wrap_pi(yaw_n as f32 * std::f32::consts::TAU / 16.0);
                            let s = BroomState::new(pos, Vec3::ZERO, yaw, pitch);
                            let mut c = behind(&s);
                            for _ in 0..3 {
                                c = chase_camera(&s, &c, 1.0 / 60.0);
                            }
                            let out = (lo - c.pos).max(c.pos - hi).max_element();
                            assert!(out <= 1e-2, "broom {pos:?} yaw {yaw} pitch {pitch}: camera {:?} is {out} outside", c.pos);
                            assert!(c.arm >= 0.0 && c.arm <= ARM, "{c:?}");
                        }
                    }
                }
            }
        }
    }
}
