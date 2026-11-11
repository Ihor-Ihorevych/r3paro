//! Broom flight model.
//!
//! Units are Unreal units per second (a walking human is about 440 uu/s) and radians. The world is Z-up; yaw 0
//! points along +X and pitch is positive upward.

use glam::Vec3;

use super::arena::{self, ArenaBounds};

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

/// An angle folded into -pi..pi, so the difference of two headings is the short way round.
pub(crate) fn wrap_pi(a: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    if !a.is_finite() {
        return 0.0;
    }
    (a + PI).rem_euclid(TAU) - PI
}

/// Takes out of `vel` whatever pushes into a surface whose outward `normal` points away from it (into the free
/// space the broom flies in), which leaves the velocity that slides along it. Velocity that already moves away
/// from the surface is returned unchanged, as is anything with an unusable normal.
pub fn slide(vel: Vec3, normal: Vec3) -> Vec3 {
    let n = normal.normalize_or_zero();
    let into = vel.dot(n);
    if n == Vec3::ZERO || !into.is_finite() || into >= 0.0 {
        return vel;
    }
    vel - n * into
}

/// The soft boundary: the yaw turn-rate (-1..1) to add to the player's own yaw input while `pos` lies outside
/// `b`, and exactly 0 while it is inside (and while it is only out through the floor or the ceiling, which
/// `confine` deals with instead).
///
/// The sign is `BroomInput.yaw`'s own: positive makes `BroomState.yaw` grow, which swings the heading from +X
/// towards +Y, anticlockwise seen from above (the convention at the top of this file). The rate is the short
/// way round from `yaw` to the heading that points at the nearest point back inside, as a fraction of half a
/// turn: a broom already flying back in is left alone, one flying straight out gets the full rate.
pub fn boundary_turn(pos: Vec3, yaw: f32, b: &ArenaBounds) -> f32 {
    if !pos.is_finite() || !yaw.is_finite() {
        return 0.0;
    }
    let back = (arena::clamp_into(b, pos) - pos).truncate();
    if back.length_squared() <= 1e-6 {
        return 0.0;
    }
    (wrap_pi(back.y.atan2(back.x) - yaw) / std::f32::consts::PI).clamp(-1.0, 1.0)
}

/// Keeps the broom in the flight volume, which is all the collision a Quidditch match has (the stadium is
/// spawned after the level's collision was baked, so it has none of its own): the position is clamped into the
/// volume and the velocity loses what pushed through the face it crossed, so the broom skims the wall instead
/// of pressing into it. Held against the floor or the ceiling it also levels out, so it flies along them
/// rather than grinding nose-first. Nothing here ever moves the broom out of the world.
pub fn confine(s: &BroomState, b: &ArenaBounds) -> BroomState {
    let mut out = *s;
    out.pos = arena::clamp_into(b, s.pos);
    if !out.vel.is_finite() {
        out.vel = Vec3::ZERO;
    }
    if !out.yaw.is_finite() || !out.pitch.is_finite() {
        out.yaw = 0.0;
        out.pitch = 0.0;
    }
    let p = if s.pos.is_finite() { s.pos } else { out.pos };
    for (i, axis) in [Vec3::X, Vec3::Y, Vec3::Z].into_iter().enumerate() {
        if p[i] < out.pos[i] {
            out.vel = slide(out.vel, axis);
        } else if p[i] > out.pos[i] {
            out.vel = slide(out.vel, -axis);
        }
    }
    // The floor and the ceiling level the broom out, so a player holding the nose into them flies on instead
    // of sticking.
    if (p.z < out.pos.z && out.pitch < 0.0) || (p.z > out.pos.z && out.pitch > 0.0) {
        out.pitch = 0.0;
    }
    out
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
    fn slide_removes_velocity_into_surface() {
        // Flying into a wall whose outward normal is -X: the x part goes, the rest slides along it.
        let v = Vec3::new(500.0, 200.0, -100.0);
        let along = slide(v, -Vec3::X);
        assert!(along.x.abs() < 1e-5, "{along:?} still pushes into the wall");
        assert_eq!((along.y, along.z), (v.y, v.z));
        assert!(along.dot(-Vec3::X) >= -1e-5);
        // A velocity that already moves away from the surface is untouched, and so is a slanted one's
        // tangent.
        assert_eq!(slide(Vec3::new(-500.0, 200.0, 0.0), -Vec3::X), Vec3::new(-500.0, 200.0, 0.0));
        let floor = slide(Vec3::new(0.0, 0.0, -300.0), Vec3::Z);
        assert!(floor.z.abs() < 1e-5, "{floor:?} still sinks through the floor");
        // Nothing to slide along leaves the velocity as it is.
        assert_eq!(slide(v, Vec3::ZERO), v);
    }

    #[test]
    fn boundary_turn_points_inward_outside_bounds() {
        let b = crate::quidditch::arena::bounds();
        // Out past the +x wall, flying straight on: the turn must swing the heading towards -x, and it is the
        // short way round whichever way the broom points.
        let out = Vec3::new(b.center.x + b.half.x + 400.0, b.center.y, b.center.z);
        for yaw in [-1.4f32, -0.7, 0.0, 0.7, 1.4] {
            let t = boundary_turn(out, yaw, &b);
            assert!(t.abs() <= 1.0, "{t} out of range at yaw {yaw}");
            // One step of the turn brings the heading closer to the inward direction (-x, i.e. pi).
            let after = yaw + t * 0.2;
            let before_gap = wrap_pi(std::f32::consts::PI - yaw).abs();
            let after_gap = wrap_pi(std::f32::consts::PI - after).abs();
            assert!(after_gap < before_gap, "yaw {yaw}: turn {t} does not point inward ({after_gap} >= {before_gap})");
        }
        // Flying back in already: nothing to correct.
        assert!(boundary_turn(out, std::f32::consts::PI, &b).abs() < 1e-5);
        // Out past -y and pointing straight out: a half turn either way is as good, so it is the full rate.
        let south = Vec3::new(b.center.x, b.center.y - b.half.y - 400.0, b.center.z);
        assert!((boundary_turn(south, -std::f32::consts::FRAC_PI_2, &b).abs() - 1.0).abs() < 1e-4);
        // Pointing along the -y wall: a quarter turn back towards +y, anticlockwise (yaw grows).
        assert!((boundary_turn(south, 0.0, &b) - 0.5).abs() < 1e-4);
    }

    #[test]
    fn boundary_turn_zero_inside() {
        let b = crate::quidditch::arena::bounds();
        for p in [b.center, crate::quidditch::arena::harry_start(&b), Vec3::new(b.center.x + b.half.x, b.center.y, b.ceiling)] {
            for yaw in [0.0f32, 1.0, -2.5] {
                assert_eq!(boundary_turn(p, yaw, &b), 0.0, "{p:?} is inside the arena");
            }
        }
        // Only out through the ceiling: the turn says nothing (the floor and the ceiling are `confine`'s).
        let high = Vec3::new(b.center.x, b.center.y, b.ceiling + 1000.0);
        assert_eq!(boundary_turn(high, 0.3, &b), 0.0);
        // Nothing usable in, nothing out.
        assert_eq!(boundary_turn(Vec3::splat(f32::NAN), 0.0, &b), 0.0);
        assert_eq!(boundary_turn(b.center, f32::NAN, &b), 0.0);
    }

    #[test]
    fn confine_keeps_the_broom_inside_and_kills_velocity_into_the_wall() {
        let b = crate::quidditch::arena::bounds();
        let (lo, hi) = (b.center - b.half, Vec3::new(b.center.x + b.half.x, b.center.y + b.half.y, b.ceiling));
        // Through the +x wall and the floor at once, nose down.
        let s = BroomState { pos: Vec3::new(hi.x + 500.0, 0.0, lo.z - 300.0), vel: Vec3::new(900.0, 100.0, -700.0), yaw: 0.0, pitch: -0.9 };
        let c = confine(&s, &b);
        assert_eq!(c.pos, Vec3::new(hi.x, 0.0, lo.z));
        assert!(c.vel.x.abs() < 1e-5 && c.vel.z.abs() < 1e-5, "{:?} still pushes through", c.vel);
        assert_eq!(c.vel.y, 100.0);
        assert_eq!(c.pitch, 0.0, "a broom on the floor levels out");
        // Inside, nothing changes at all.
        let inside = BroomState { pos: b.center, vel: Vec3::new(0.0, 1200.0, 0.0), yaw: 1.0, pitch: -0.3 };
        assert_eq!(confine(&inside, &b), inside);
        // A broom flown at the ceiling for a while never leaves the volume, whatever it is told to do.
        let t = BroomTuning::default();
        let mut s = BroomState { pos: Vec3::new(b.center.x, b.center.y, b.ceiling - 10.0), vel: Vec3::ZERO, yaw: 0.6, pitch: 1.0 };
        for _ in 0..2000 {
            s = confine(&step(&s, &BroomInput { yaw: 0.2, pitch: 1.0, boost: true, brake: false }, &t, 1.0 / 60.0), &b);
            assert!(s.pos.x >= lo.x - 1e-3 && s.pos.x <= hi.x + 1e-3, "{:?}", s.pos);
            assert!(s.pos.y >= lo.y - 1e-3 && s.pos.y <= hi.y + 1e-3, "{:?}", s.pos);
            assert!(s.pos.z >= lo.z - 1e-3 && s.pos.z <= hi.z + 1e-3, "{:?}", s.pos);
        }
        // Rubbish in, something usable out: the broom must never be put out of the world.
        let bad = BroomState { pos: Vec3::splat(f32::NAN), vel: Vec3::splat(f32::INFINITY), yaw: f32::NAN, pitch: 0.0 };
        let c = confine(&bad, &b);
        assert!(c.pos.is_finite() && c.vel.is_finite() && c.yaw.is_finite() && c.pitch.is_finite(), "{c:?}");
    }

    #[test]
    fn step_is_deterministic() {
        let t = BroomTuning::default();
        let i = BroomInput { yaw: 0.5, pitch: -0.2, boost: true, brake: false };
        assert_eq!(step(&start(), &i, &t, 0.016), step(&start(), &i, &t, 0.016));
    }
}
