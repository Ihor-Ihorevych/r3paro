//! The chase camera that watches Harry fly. Pure: a `BroomState` and the camera of the frame before go in, the
//! camera of this frame comes out, which `quidditch::glue` keeps in the match and `player_view` hands the
//! renderer while a match is running.
//!
//! It is the orbit camera of AdamJD's HP1 "Movement mod" (`docs/formats/hp1-broom-movement-mod.md`): a short
//! arm from a point above the rider, aimed along the rider's *look* direction (`BroomState::want_yaw` /
//! `want_pitch`) rather than along the broom's heading, so steering is camera-relative and the broom is seen
//! turning into the look. All the softness is in the rotation -- it closes part of the gap to the look each
//! frame -- and none in the position, which sits exactly on the arm (a little over it, since the owner asked
//! for the camera "a little bit up": see `AIM_UP`/`RISE`). Looking up from below pulls the camera in towards
//! the rider, and the arm is shortened so the camera never leaves the flight volume. The one thing here that
//! is neither the mod's nor the retail game's is the field of view, which opens with speed and is low-passed
//! so it never jumps (`fov_for`, `FOV_TAU`); it is the only such mechanism a match has.

use glam::Vec3;

use super::arena::{self, ArenaBounds};
use super::flight::{wrap_pi, BroomState};

/// Where the camera is, where it looks (Unreal's (pitch, yaw, roll) in 65536ths of a turn), how wide its view
/// is (horizontal degrees, already low-passed towards the speed's target) -- which is what `player_view`
/// returns -- and how long its arm is this frame. The arm and the field of view are the two pieces of camera
/// state that are not in the rotation, and both are carried from frame to frame.
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
/// rig is scaled with it (`CAM_UP`/`AIM_UP` below), so only the size of Harry in the frame changes.
const ARM: f32 = 200.0;
/// How high over the broom the camera hangs when the rig is level (uu) and how high over it the camera
/// *looks* -- the mod has one number for both (`vLookAtOffset.Z` of 55, 86 here once scaled by our 200/128
/// arm), and so had we until the owner asked for the camera "a little bit up". Now the camera stands `RISE`
/// off the arm, square to it, and looks back down at a point `AIM_UP` over the rider: it sits over Harry's
/// shoulder looking down at him, he is lower in the frame (27.6 deg under the middle, was 23.3), and more of
/// the pitch ahead is in the picture. Standing square to the arm rather than straight up is what keeps that
/// framing the same at every pitch and every arm -- the whole rig simply tips with the look.
const AIM_UP: f32 = 116.0;
const RISE: f32 = 30.0;
/// The level height that comes to, for the doc and the tests: `AIM_UP + ARM * SLANT * sin(TILT)`.
const CAM_UP: f32 = AIM_UP + RISE;
/// What that rise does to the rig, with `r = RISE / ARM`: the camera's own facing is `TILT` below the look
/// direction and its distance from the look-at point is `SLANT` arms. Both are fixed, whatever the pitch and
/// whatever the arm, which is the one thing `chase_camera` needs to read the look back out of the rotation it
/// handed out the frame before. (`the_rig_is_raised_and_looks_down` checks the two against `RISE`.)
const TILT: f32 = 0.148_890; // r.atan()
const SLANT: f32 = 1.011_187; // (1.0 + r * r).sqrt()
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
/// the arm, see `chase_camera`), which the mod does not: shortening the arm alone while still aiming `AIM_UP`
/// over the rider swings him off the bottom of the screen as soon as the arm is halved, and at 0.15 of the arm
/// the camera would be inside him. Scaled, the rider keeps the same place in the picture whatever the arm is.
const PULL_SPREAD: f32 = 0.9587;
const MIN_SCALE: f32 = 0.3;
/// The arm grows back at this rate (1/s, exponential): the mod's `fMoveBackTightness`. Shrinking is immediate.
const BACK_TIGHT: f32 = 2.5;
/// How far short of a wall the camera stops (uu): the mod's trace cushion.
const CUSHION: f32 = 5.0;
/// Field of view at rest (horizontal degrees) and how much wider it opens at the boost's top speed (so speed
/// reads). Ours, not the mod's -- neither HP1 package changes the field of view with speed. The two speeds it
/// is measured between are the broom's own cruise and boost speeds (`BroomTuning::default`), read from there
/// rather than written down twice, and the opening is a smoothstep between them so it neither starts nor ends
/// with a corner. On top of that the field of view is low-passed towards that target with `FOV_TAU`, so a
/// boost switched on or off, or a brake, eases in and out instead of snapping.
const FOV: f32 = 115.0; // 90, plus 25 at the owner's word
const FOV_BOOST: f32 = 15.0;
/// The time constant of that low pass (s): about a third of a second to most of the way.
const FOV_TAU: f32 = 0.35;

/// Where the field of view wants to be at this speed (uu/s): `FOV` up to the cruise speed, `FOV + FOV_BOOST`
/// at the boost speed, a smoothstep in between.
fn fov_for(speed: f32) -> f32 {
    let t = super::momentum::MomentumTuning::default();
    let (cruise, fast) = (t.base_speed, t.max_speed.max(t.base_speed + 1.0));
    let x = if speed.is_finite() { ((speed - cruise) / (fast - cruise)).clamp(0.0, 1.0) } else { 0.0 };
    FOV + FOV_BOOST * (x * x * (3.0 - 2.0 * x))
}

/// Radians to Unreal's rotation units.
const UU: f32 = 65536.0 / std::f32::consts::TAU;

/// A pitch and a yaw in radians as an Unreal rotator (pitch, yaw, roll), the yaw folded into one turn.
pub fn rotator(pitch: f32, yaw: f32) -> [i32; 3] {
    let unit = |a: f32| if a.is_finite() { (a * UU).round().rem_euclid(65536.0) as i32 } else { 0 };
    [unit(pitch), unit(yaw), 0]
}

/// The camera as it starts: straight behind the broom on a full arm, nothing to lag behind yet. (The seed's
/// rotation is a *given* rotation, so the rig facing `chase_camera` reads back out of it is the look.)
pub fn behind(s: &BroomState) -> CamState {
    let seed = CamState { pos: s.pos, rot: rotator(s.want_pitch - TILT, s.want_yaw), fov: FOV, arm: ARM };
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
        ceiling: b.ceiling + ARM + CAM_UP,
    }
}

/// How far the camera may go from `from` along `dir` before it leaves `b`, less the cushion: the mod traces
/// look-at to camera against the world and pulls the camera in to the first hit. The arena is all the
/// collision a match has, so the wall it stops at is the camera volume's (`volume`). `dir` is the camera's own
/// facing, which is `TILT` under the arm, so what comes back is a slant and the arm in it is `/ SLANT`.
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
    (hit - CUSHION).clamp(0.0, ARM * SLANT)
}

/// This frame's camera, from the broom and the camera of the frame before.
pub fn chase_camera(s: &BroomState, prev: &CamState, dt: f32) -> CamState {
    let dt = if dt.is_finite() { dt.clamp(0.0, 0.25) } else { 0.0 };
    let finite = |v: f32, or: f32| if v.is_finite() { v } else { or };
    // Where the rider is looking is where the camera wants to point (camera-relative steering: the broom then
    // turns into the camera, not the other way round).
    // (`prev.rot` is the rotation the camera was *given*, already tilted down by the rise; the rig's own
    // facing, which is what is smoothed, is that much higher.)
    let was_yaw = wrap_pi(prev.rot[1] as f32 / UU);
    let was_pitch = wrap_pi(prev.rot[0] as f32 / UU) + TILT;
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
    // The look-at point, a little over the rider, and the camera `arm` back along the arm and `RISE` square
    // off it -- shortened so it stops short of the arena wall instead of hanging in the stands. The height
    // over the rider shrinks with the arm, so he keeps his place in the picture however far the camera is
    // pulled in.
    let rider = if s.pos.is_finite() { s.pos } else { prev.pos };
    let rider = if rider.is_finite() { rider } else { Vec3::ZERO };
    // The camera stands `RISE` off the arm, square to it: that is the same rig with its arm `SLANT` times as
    // long and its facing `TILT` lower, so `fwd` here is the camera's own facing and everything below is the
    // old rig along it.
    let (cam_pitch, fwd) = {
        let p = pitch - TILT;
        (p, Vec3::new(p.cos() * yaw.cos(), p.cos() * yaw.sin(), p.sin()))
    };
    //
    // The whole rig is confined, not only the arm: the look-at point is itself kept inside the volume (a ray
    // cast from outside it finds no wall to stop at), the arm is measured twice (shortening it lowers the look-at point, which moves the ray), and whatever
    // rounding is left is clamped away, so the camera is never outside the walls, under the floor or over the
    // top whatever the broom and the look are doing.
    let bounds = volume();
    let look_at = |arm: f32| arena::clamp_into(&bounds, rider + Vec3::Z * AIM_UP * (arm / ARM));
    let arm = arm.min(room_along(look_at(arm), -fwd, &bounds) / SLANT);
    let arm = arm.min(room_along(look_at(arm), -fwd, &bounds) / SLANT);
    let at = look_at(arm);
    // The field of view: where this speed wants it, approached with a time constant of `FOV_TAU` so it never
    // jumps. `dt` is already clamped and finite, and a rubbish previous frame starts again from `FOV`.
    let speed = if s.vel.is_finite() { s.vel.length() } else { 0.0 };
    let was_fov = finite(prev.fov, FOV).clamp(FOV, FOV + FOV_BOOST);
    let fov = was_fov + (fov_for(speed) - was_fov) * (1.0 - (-dt / FOV_TAU).exp());
    CamState {
        pos: arena::clamp_into(&bounds, at - fwd * (arm * SLANT)),
        rot: rotator(cam_pitch, yaw),
        fov: finite(fov, FOV).clamp(FOV, FOV + FOV_BOOST),
        arm,
    }
}

/// The catch orbit (the HP1/HP2 finish): the camera swings slowly round Harry. How fast (rad/s, about 18 s a
/// turn), how far from him and how high over him it settles (uu), where on him it looks, and how long it
/// takes to come out of the chase pose and into the orbit (s).
pub const ORBIT_RATE: f32 = 0.35;
const ORBIT_RADIUS: f32 = 340.0;
const ORBIT_UP: f32 = 90.0;
const ORBIT_AIM: f32 = 40.0;
const ORBIT_EASE: f32 = 1.5;
const ORBIT_TURN_EASE: f32 = 1.0;

fn smooth01(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// The camera `t` seconds into the orbit round `center` (Harry), which began from the chase pose `from`.
/// Everything eases in -- the angular speed from nothing, the radius and height from where the chase left
/// them, the view from the chase's rotation -- so the orbit takes over from the chase without a cut.
pub fn orbit_camera(center: Vec3, from: &CamState, t: f32) -> CamState {
    let t = if t.is_finite() { t.max(0.0) } else { 0.0 };
    let center = if center.is_finite() { center } else { from.pos };
    let rel = from.pos - center;
    let (r0, h0) = (rel.truncate().length().max(1.0), rel.z);
    let a0 = rel.y.atan2(rel.x);
    // The angle turned: the integral of a rate that eases in over `ORBIT_EASE`.
    let u = (t / ORBIT_EASE).min(1.0);
    let turned = ORBIT_RATE * (ORBIT_EASE * (u * u * u - u * u * u * u * 0.5) + (t - ORBIT_EASE).max(0.0));
    let k = smooth01(t / ORBIT_EASE);
    let radius = r0 + (ORBIT_RADIUS - r0) * k;
    let up = h0 + (ORBIT_UP - h0) * k;
    let a = a0 + turned;
    let pos = arena::clamp_into(&volume(), center + Vec3::new(a.cos() * radius, a.sin() * radius, up));
    let to = center + Vec3::Z * ORBIT_AIM - pos;
    let (aim_yaw, aim_pitch) = (to.y.atan2(to.x), (to.z / to.length().max(1e-3)).clamp(-1.0, 1.0).asin());
    // The view turns from the chase's own rotation to the aim.
    let (was_yaw, was_pitch) = (wrap_pi(from.rot[1] as f32 / UU), wrap_pi(from.rot[0] as f32 / UU));
    let b = smooth01(t / ORBIT_TURN_EASE);
    let yaw = was_yaw + wrap_pi(aim_yaw - was_yaw) * b;
    let pitch = was_pitch + (aim_pitch - was_pitch) * b;
    let fov = if from.fov.is_finite() { from.fov + (FOV - from.fov) * (1.0 - (-t / FOV_TAU).exp()) } else { FOV };
    CamState { pos, rot: rotator(pitch, yaw), fov, arm: from.arm }
}

#[cfg(test)]
mod tests {
    use super::super::flight::{self, BroomInput, BroomTuning};
    use super::*;

    #[test]
    fn the_orbit_takes_over_without_a_cut_and_goes_slowly_round() {
        let s = flying();
        let chase = chase_camera(&s, &behind(&s), 1.0 / 60.0);
        let first = orbit_camera(s.pos, &chase, 0.0);
        assert!(first.pos.distance(chase.pos) < 1.0, "{:?} vs {:?}", first.pos, chase.pos);
        let mut last = first;
        let mut angle = 0.0f32;
        let mut prev_a = (first.pos - s.pos).y.atan2((first.pos - s.pos).x);
        for n in 1..=60 * 20 {
            let c = orbit_camera(s.pos, &chase, n as f32 / 60.0);
            assert!(c.pos.is_finite());
            assert!(c.pos.distance(last.pos) < 14.0, "a jump of {} in one frame", c.pos.distance(last.pos));
            let a = (c.pos - s.pos).y.atan2((c.pos - s.pos).x);
            angle += wrap_pi(a - prev_a);
            prev_a = a;
            last = c;
        }
        // After 20 s it has turned at about the orbit rate (less the ease-in).
        let want = ORBIT_RATE * (20.0 - 0.75);
        assert!((angle - want).abs() < 0.15, "turned {angle}, wanted {want}");
        // It has settled at its distance, looking at Harry.
        assert!(((last.pos - s.pos).truncate().length() - ORBIT_RADIUS).abs() < 5.0);
        let look = Vec3::new(cam_pitch(&last).cos() * cam_yaw(&last).cos(), cam_pitch(&last).cos() * cam_yaw(&last).sin(), cam_pitch(&last).sin());
        assert!((s.pos + Vec3::Z * ORBIT_AIM - last.pos).normalize().dot(look) > 0.999);
        // Rubbish in gives something usable.
        assert!(orbit_camera(Vec3::splat(f32::NAN), &chase, f32::NAN).pos.is_finite());
    }

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
            // Close, but never inside the rider (he is about 40 uu across) and never farther than the whole
            // rig: the arm and the camera's height over him.
            let d = (c.pos - s.pos).length();
            let full = Vec3::new(ARM, 0.0, CAM_UP).length();
            // (Looking down, the arm and the height over the rider add up: the ceiling levels the broom out in
            // this run and the look then dips below level.)
            let reach = if pitch < 0.0 { ARM + CAM_UP } else { full };
            assert!(d <= reach + 1e-2, "{d} is farther than the arm");
            assert!(d >= 40.0, "{d} is inside the rider");
            // And it looks at Harry: it aims exactly at the point over him, and he is well inside the view
            // (a little lower in it than the aim, which is the rise).
            assert!(pitch.abs() <= MAX_CAM_PITCH + 1e-3, "camera pitch {pitch}");
            let look = Vec3::new(pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), pitch.sin());
            let to_at = arena::clamp_into(&volume(), s.pos + Vec3::Z * AIM_UP * (c.arm / ARM)) - c.pos;
            assert!(to_at.normalize().dot(look) > 0.999, "the camera does not look at the point over the broom");
            assert!((s.pos - c.pos).normalize().dot(look) > 0.80, "the broom is out of the picture: {} at pitch {pitch}", (s.pos - c.pos).normalize().dot(look).acos());
            assert!(c.fov >= FOV && c.fov <= FOV + FOV_BOOST, "fov {}", c.fov);
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

    /// The camera sits over Harry's shoulder: `CAM_UP` over him in level flight (it was `AIM_UP`), looking
    /// down at him rather than level, and the two numbers the rig is built from are what they claim to be.
    #[test]
    fn the_rig_is_raised_and_looks_down() {
        let r = RISE / ARM;
        assert!((TILT - r.atan()).abs() < 1e-5 && (SLANT - (1.0 + r * r).sqrt()).abs() < 1e-5);
        let s = BroomState::new(Vec3::new(100.0, -200.0, 500.0), Vec3::ZERO, 0.4, 0.0);
        let c = chase_camera(&s, &behind(&s), 1.0 / 60.0);
        assert!((c.pos.z - s.pos.z - CAM_UP).abs() < 1.0, "the camera is {} over Harry, not {CAM_UP}", c.pos.z - s.pos.z);
        assert!((c.arm - ARM).abs() < 1e-3, "{c:?}");
        // Level flight, and the view still dips: about 8.5 deg, the rise over the arm.
        assert!(cam_pitch(&c) < -0.13 && cam_pitch(&c) > -0.17, "the view does not look down: {}", cam_pitch(&c));
        // A held look is held: the rig settles on it, `TILT` lower, instead of drifting off by the tilt every
        // frame (which is what reading the look back out of the rotation wrongly would do).
        for want in [-0.8_f32, -0.2, 0.0, 0.5, 1.0] {
            let held = BroomState { want_pitch: want, ..flying() };
            let mut c = behind(&flying());
            for _ in 0..240 {
                c = chase_camera(&held, &c, 1.0 / 60.0);
            }
            assert!((cam_pitch(&c) + TILT - want).abs() < 0.01, "the rig drifted to {} from a look of {want}", cam_pitch(&c) + TILT);
            // and Harry keeps his place in the frame whatever the look is doing: always the same angle under
            // the middle of the picture.
            let look = Vec3::new(cam_pitch(&c).cos() * cam_yaw(&c).cos(), cam_pitch(&c).cos() * cam_yaw(&c).sin(), cam_pitch(&c).sin());
            let under = (held.pos - c.pos).normalize().dot(look).acos();
            assert!(under > 0.2 && under < 0.62, "Harry is {under} rad under the middle at a look of {want}");
        }
    }

    /// The field of view opens with speed and closes again, and never jumps: it is a smoothstep between the
    /// broom's own cruise and boost speeds, low-passed with `FOV_TAU`.
    #[test]
    fn fov_grows_with_speed_and_is_approached_gradually() {
        // The broom's cruise and top speed are the momentum's base and full-charge speeds now.
        let m = crate::quidditch::momentum::MomentumTuning::default();
        let t = BroomTuning { speed: m.base_speed, boost_speed: m.max_speed, ..BroomTuning::default() };
        let dt = 1.0 / 60.0;
        let at_speed = |v: f32| BroomState { vel: Vec3::new(v, 0.0, 0.0), ..flying() };
        // Monotonic in speed, from one and the same frame before.
        let seed = behind(&flying());
        let mut was = 0.0;
        // The in-between speeds are fractions of the cruise..boost span, so retuning the broom cannot leave
        // this test testing speeds that are both past the boost.
        let mid = |k: f32| t.speed + (t.boost_speed - t.speed) * k;
        for v in [0.0, 100.0, t.speed, mid(0.25), mid(0.5), mid(0.75), t.boost_speed, t.boost_speed * 2.0] {
            let f = chase_camera(&at_speed(v), &seed, dt).fov;
            assert!(f >= was - 1e-6, "fov fell from {was} to {f} at {v} uu/s");
            was = f;
        }
        // Cruising is the base; the boost's top speed is the base plus the whole extra, reached in a few
        // seconds and not before.
        let mut c = seed;
        for _ in 0..600 {
            c = chase_camera(&at_speed(t.speed), &c, dt);
        }
        assert!((c.fov - FOV).abs() < 1e-3, "cruising opened the view to {}", c.fov);
        let mut steps = Vec::new();
        for _ in 0..180 {
            let next = chase_camera(&at_speed(t.boost_speed), &c, dt);
            steps.push(next.fov - c.fov);
            c = next;
        }
        assert!(steps[0] < 1.0, "the field of view jumped {} deg in one frame", steps[0]);
        assert!(steps.iter().all(|s| *s >= -1e-6 && *s < 1.0), "a step out of bounds: {steps:?}");
        assert!((c.fov - (FOV + FOV_BOOST)).abs() < 0.2, "three seconds of boost only reached {}", c.fov);
        // Half a second in, it is well on its way but not there (a time constant of about a third of a second).
        let mut half = seed;
        for _ in 0..30 {
            half = chase_camera(&at_speed(t.boost_speed), &half, dt);
        }
        assert!(half.fov > FOV + FOV_BOOST * 0.5 && half.fov < FOV + FOV_BOOST * 0.95, "{}", half.fov);
        // Frame-rate independent: two small steps land where one big one does, and dt = 0 moves nothing.
        let one = chase_camera(&at_speed(t.boost_speed), &seed, 0.1).fov;
        let a = chase_camera(&at_speed(t.boost_speed), &seed, 0.05);
        let two = chase_camera(&at_speed(t.boost_speed), &a, 0.05).fov;
        assert!((one - two).abs() < 0.05, "{one} vs {two}");
        assert_eq!(chase_camera(&at_speed(t.boost_speed), &seed, 0.0).fov, seed.fov);
        // And it comes back down when the boost stops.
        for _ in 0..180 {
            c = chase_camera(&at_speed(t.speed), &c, dt);
        }
        assert!((c.fov - FOV).abs() < 0.2, "the view never closed again: {}", c.fov);
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
        let cam_hi = Vec3::new(hi.x, hi.y, hi.z + ARM + CAM_UP);
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
