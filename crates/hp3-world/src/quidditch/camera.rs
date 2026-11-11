//! The chase camera that watches Harry fly. Pure: a `BroomState` and the camera of the frame before go in, the
//! camera of this frame comes out, which `quidditch::glue` keeps in the match and `player_view` hands the
//! renderer while a match is running.
//!
//! The camera rides behind and above the broom and looks at it. Only its yaw lags (it closes part of the gap to
//! the broom's heading each frame, and never turns faster than `MAX_TURN`), because that is what a sudden turn
//! needs: the height and the pitch follow the broom at once, so the picture never swings about by itself.

use glam::Vec3;

use super::flight::{wrap_pi, BroomState};

/// Where the camera is, where it looks (Unreal's (pitch, yaw, roll) in 65536ths of a turn) and how wide its
/// view is (horizontal degrees), exactly what `player_view` returns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CamState {
    pub pos: Vec3,
    pub rot: [i32; 3],
    pub fov: f32,
}

/// How far behind the broom the camera sits (uu).
const DIST: f32 = 420.0;
/// How far above it (uu), before the climb correction.
const HEIGHT: f32 = 140.0;
/// How much of a climb or a dive moves the camera down or up behind the broom, so where it is heading stays in
/// the picture (fraction of `DIST`).
const PITCH_FOLLOW: f32 = 0.55;
/// The camera stays at least this far above the broom (uu), whatever the broom does.
const MIN_ABOVE: f32 = 40.0;
/// It looks at a point this far above the broom (uu), so Harry sits in the lower half of the picture.
const LOOK_UP: f32 = 70.0;
/// How quickly the camera's yaw closes the gap to the broom's heading (1/s, as an exponential approach).
const TURN_LAG: f32 = 6.0;
/// and the most it may turn in a second (rad/s), which is what bounds a sudden turn.
const MAX_TURN: f32 = 3.2;
/// Field of view at rest (horizontal degrees), and how much wider it opens at `FAST` (so speed reads).
const FOV: f32 = 90.0;
const FOV_SPEED: f32 = 10.0;
const FAST: f32 = 2400.0;

/// Radians to Unreal's rotation units.
const UU: f32 = 65536.0 / std::f32::consts::TAU;

/// A pitch and a yaw in radians as an Unreal rotator (pitch, yaw, roll), the yaw folded into one turn.
pub fn rotator(pitch: f32, yaw: f32) -> [i32; 3] {
    let unit = |a: f32| if a.is_finite() { (a * UU).round().rem_euclid(65536.0) as i32 } else { 0 };
    [unit(pitch), unit(yaw), 0]
}

/// The camera as it starts: straight behind the broom, nothing to lag behind yet.
pub fn behind(s: &BroomState) -> CamState {
    let seed = CamState { pos: s.pos, rot: rotator(0.0, s.yaw), fov: FOV };
    chase_camera(s, &seed, 0.0)
}

/// This frame's camera, from the broom and the camera of the frame before.
pub fn chase_camera(s: &BroomState, prev: &CamState, dt: f32) -> CamState {
    let dt = if dt.is_finite() { dt.clamp(0.0, 0.25) } else { 0.0 };
    let was = prev.rot[1] as f32 / UU;
    let want = if s.yaw.is_finite() { s.yaw } else { was };
    // Part of the gap each frame, and never more than a bounded turn: a broom that spins round does not throw
    // the camera round with it.
    let blend = 1.0 - (-TURN_LAG * dt).exp();
    let yaw = was + (wrap_pi(want - was) * blend).clamp(-MAX_TURN * dt, MAX_TURN * dt);
    let at = if s.pos.is_finite() { s.pos } else { prev.pos };
    let pitch = if s.pitch.is_finite() { s.pitch } else { 0.0 };
    let back = Vec3::new(yaw.cos(), yaw.sin(), 0.0);
    let pos = at - back * DIST + Vec3::Z * (HEIGHT - pitch.sin() * DIST * PITCH_FOLLOW).max(MIN_ABOVE);
    // It looks at the broom (a little over it), so the picture holds him wherever the lag has left the camera.
    let look = at + Vec3::Z * LOOK_UP - pos;
    let flat = look.truncate().length();
    let speed = if s.vel.is_finite() { s.vel.length() } else { 0.0 };
    CamState {
        pos,
        rot: rotator(look.z.atan2(flat), yaw),
        fov: FOV + (speed / FAST).clamp(0.0, 1.0) * FOV_SPEED,
    }
}

#[cfg(test)]
mod tests {
    use super::super::flight::{self, BroomInput, BroomTuning};
    use super::*;

    fn flying() -> BroomState {
        BroomState { pos: Vec3::new(100.0, -200.0, 500.0), vel: Vec3::ZERO, yaw: 0.4, pitch: 0.0 }
    }

    /// The camera's own yaw, as `chase_camera` left it.
    fn cam_yaw(c: &CamState) -> f32 {
        c.rot[1] as f32 / UU
    }

    #[test]
    fn chase_camera_stays_behind_and_above_harry() {
        let t = BroomTuning::default();
        let mut s = flying();
        let mut c = behind(&s);
        // A turn, a climb and a dive, all at once, for ten seconds.
        for n in 0..600 {
            let i = BroomInput { yaw: 0.6, pitch: ((n as f32) / 60.0).sin(), boost: n % 120 < 60, brake: false };
            s = flight::step(&s, &i, &t, 1.0 / 60.0);
            c = chase_camera(&s, &c, 1.0 / 60.0);
            assert!(c.pos.is_finite(), "{:?}", c.pos);
            assert!(c.pos.z > s.pos.z, "camera at {:?} is not above the broom at {:?}", c.pos, s.pos);
            // Behind: the way from the camera to Harry points the way the camera looks, give or take the lag.
            let yaw = cam_yaw(&c);
            let ahead = Vec3::new(yaw.cos(), yaw.sin(), 0.0);
            assert!((s.pos - c.pos).dot(ahead) > 0.0, "the broom at {:?} is not ahead of the camera at {:?}", s.pos, c.pos);
            assert!((c.pos - s.pos).truncate().length() > DIST * 0.5, "camera too close: {:?} vs {:?}", c.pos, s.pos);
            // And it looks at him: his place is inside the view, not behind it.
            let pitch = c.rot[0] as f32 / UU;
            let look = Vec3::new(pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), pitch.sin());
            assert!((s.pos - c.pos).normalize().dot(look) > 0.9, "the camera does not look at the broom");
            assert!(c.fov >= FOV && c.fov <= FOV + FOV_SPEED, "fov {}", c.fov);
        }
    }

    #[test]
    fn chase_camera_smooths_a_sudden_turn() {
        let dt = 1.0 / 60.0;
        let s = flying();
        let mut c = behind(&s);
        // Harry whips round half a turn in one frame (a Bludger, or the soft boundary at full rate).
        let spun = BroomState { yaw: s.yaw + std::f32::consts::PI * 0.9, ..s };
        let first = chase_camera(&spun, &c, dt);
        assert!(wrap_pi(cam_yaw(&first) - cam_yaw(&c)).abs() <= MAX_TURN * dt + 1e-4, "the camera jumped with him");
        // Every step is bounded, and the camera does catch up.
        for _ in 0..240 {
            let next = chase_camera(&spun, &c, dt);
            let step = wrap_pi(cam_yaw(&next) - cam_yaw(&c)).abs();
            assert!(step <= MAX_TURN * dt + 1e-4, "step {step} over the bound");
            c = next;
        }
        assert!(wrap_pi(cam_yaw(&c) - spun.yaw).abs() < 0.01, "the camera never caught up: {} vs {}", cam_yaw(&c), spun.yaw);
    }

    #[test]
    fn chase_camera_survives_rubbish() {
        let bad = BroomState { pos: Vec3::splat(f32::NAN), vel: Vec3::splat(f32::INFINITY), yaw: f32::NAN, pitch: f32::NAN };
        let c = chase_camera(&bad, &behind(&flying()), f32::NAN);
        assert!(c.pos.is_finite() && c.fov.is_finite(), "{c:?}");
    }
}
