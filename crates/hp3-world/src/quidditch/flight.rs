//! Broom flight model.
//!
//! Units are Unreal units per second (a walking human is about 440 uu/s) and radians. The world is Z-up; yaw 0
//! points along +X and pitch is positive upward.

use glam::Vec3;

/// Player steering for one tick. `yaw` and `pitch` are stick deflections in -1..1.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BroomInput {
    pub yaw: f32,
    pub pitch: f32,
    pub boost: bool,
    pub brake: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BroomState {
    pub pos: Vec3,
    pub vel: Vec3,
    pub yaw: f32,
    pub pitch: f32,
}

/// Placeholder tuning, to be adjusted by feel in a later task.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BroomTuning {
    /// Cruise speed (uu/s).
    pub speed: f32,
    /// Target speed while boosting (uu/s).
    pub boost_speed: f32,
    /// Speed change toward the target (uu/s^2).
    pub accel: f32,
    /// Target speed while braking (uu/s); speed falls toward it at `accel`.
    pub decel: f32,
    /// Yaw rate at full deflection (rad/s).
    pub yaw_rate: f32,
    /// Pitch rate at full deflection (rad/s).
    pub pitch_rate: f32,
    /// Pitch is clamped to +-this (rad).
    pub max_pitch: f32,
}

impl Default for BroomTuning {
    fn default() -> Self {
        BroomTuning {
            speed: 1200.0,
            boost_speed: 2400.0,
            accel: 1500.0,
            decel: 300.0,
            yaw_rate: 2.0,
            pitch_rate: 1.5,
            max_pitch: 1.2,
        }
    }
}

/// Advance the broom by `dt` seconds. Brake wins over boost.
pub fn step(s: &BroomState, i: &BroomInput, t: &BroomTuning, dt: f32) -> BroomState {
    let yaw = s.yaw + i.yaw.clamp(-1.0, 1.0) * t.yaw_rate * dt;
    let pitch = (s.pitch + i.pitch.clamp(-1.0, 1.0) * t.pitch_rate * dt).clamp(-t.max_pitch, t.max_pitch);
    let target = if i.brake {
        t.decel
    } else if i.boost {
        t.boost_speed
    } else {
        t.speed
    };
    let cur = s.vel.length();
    let max_delta = t.accel * dt;
    let speed = cur + (target - cur).clamp(-max_delta, max_delta);
    let fwd = Vec3::new(pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), pitch.sin());
    let vel = fwd * speed;
    BroomState { pos: s.pos + vel * dt, vel, yaw, pitch }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start() -> BroomState {
        BroomState { pos: Vec3::ZERO, vel: Vec3::ZERO, yaw: 0.3, pitch: 0.1 }
    }

    fn none() -> BroomInput {
        BroomInput { yaw: 0.0, pitch: 0.0, boost: false, brake: false }
    }

    fn run(i: &BroomInput, secs: usize) -> BroomState {
        let t = BroomTuning::default();
        let mut s = start();
        for _ in 0..secs * 60 {
            s = step(&s, i, &t, 1.0 / 60.0);
        }
        s
    }

    #[test]
    fn boost_raises_target_speed() {
        let plain = run(&none(), 10);
        let boosted = run(&BroomInput { boost: true, ..none() }, 10);
        assert!(boosted.vel.length() > plain.vel.length() + 1.0);
    }

    #[test]
    fn brake_overrides_boost() {
        let plain = run(&none(), 10);
        let both = run(&BroomInput { boost: true, brake: true, ..none() }, 10);
        assert!(both.vel.length() <= plain.vel.length());
    }

    #[test]
    fn pitch_is_clamped_to_max_pitch() {
        let t = BroomTuning::default();
        let up = BroomInput { pitch: 1.0, ..none() };
        let down = BroomInput { pitch: -1.0, ..none() };
        let (mut a, mut b) = (start(), start());
        for _ in 0..600 {
            a = step(&a, &up, &t, 1.0 / 60.0);
            b = step(&b, &down, &t, 1.0 / 60.0);
        }
        assert!((a.pitch - t.max_pitch).abs() < 1e-5);
        assert!((b.pitch + t.max_pitch).abs() < 1e-5);
    }

    #[test]
    fn no_input_holds_heading() {
        let s = run(&none(), 5);
        assert_eq!(s.yaw, 0.3);
        assert_eq!(s.pitch, 0.1);
        let dir = s.vel.normalize();
        let want = Vec3::new(0.1f32.cos() * 0.3f32.cos(), 0.1f32.cos() * 0.3f32.sin(), 0.1f32.sin());
        assert!((dir - want).length() < 1e-4);
    }

    #[test]
    fn step_is_deterministic() {
        let t = BroomTuning::default();
        let i = BroomInput { yaw: 0.5, pitch: -0.2, boost: true, brake: false };
        assert_eq!(step(&start(), &i, &t, 0.016), step(&start(), &i, &t, 0.016));
    }
}
