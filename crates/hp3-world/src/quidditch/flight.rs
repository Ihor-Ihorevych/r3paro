//! Broom flight model.
//!
//! Units are Unreal units per second (a walking human is about 440 uu/s) and radians. The world is Z-up; yaw 0
//! points along +X and pitch is positive upward.
//!
//! The broom is steered the way AdamJD's HP1 "Movement mod" steers it (see
//! `docs/formats/hp1-broom-movement-mod.md`): the mouse integrates a free look direction in both axes and the
//! broom turns towards that direction at a rate limit, instead of the stick being the turn rate itself. Speed
//! is unchanged -- cruise, boost and a brake that beats the boost.

use glam::Vec3;

use super::arena::{self, ArenaBounds};

/// Player steering for one tick.
///
/// Steering is a *look direction*, as AdamJD's HP1 movement mod flies: the mouse (and the phone's stick, and
/// the arrow keys) move where the rider is looking, and the broom turns towards that look at a rate limit.
/// `look_yaw` and `look_pitch` are the mouse's own angular motion **this frame** in radians, already a
/// per-frame quantity and therefore never multiplied by `dt` again (the mod does multiply by `dt`, which makes
/// its sensitivity depend on the frame rate; that is a bug and is not reproduced here). `yaw` and `pitch` are
/// the key/stick deflections in -1..1, which move the same look direction at a constant rate
/// (`BroomTuning::yaw_rate` / `pitch_rate`), so flying with the keyboard or a thumbstick alone works.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BroomInput {
    pub yaw: f32,
    pub pitch: f32,
    pub boost: bool,
    pub brake: bool,
    /// Mouse yaw motion this frame (rad), positive turning the way `yaw` grows.
    pub look_yaw: f32,
    /// Mouse pitch motion this frame (rad), positive nose-up.
    pub look_pitch: f32,
}

/// Where the broom is, where it points, and where the rider is *looking* (`want_yaw` / `want_pitch`, the
/// mouse-integrated heading the broom chases). Build one with `BroomState::new`, which starts the look on the
/// broom's own heading.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BroomState {
    pub pos: Vec3,
    pub vel: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    /// The look direction's yaw (rad): what the mouse steers and the broom turns towards.
    pub want_yaw: f32,
    /// The look direction's pitch (rad), clamped to `BroomTuning::max_look_pitch`.
    pub want_pitch: f32,
}

impl BroomState {
    /// A broom at `pos` flying `vel`, pointing (and looking) at `yaw`/`pitch`.
    pub fn new(pos: Vec3, vel: Vec3, yaw: f32, pitch: f32) -> Self {
        BroomState { pos, vel, yaw, pitch, want_yaw: yaw, want_pitch: pitch }
    }
}

/// How the broom flies, as the playtest asked for it: half the speed the first pass flew at (the pitch is
/// 12000 uu long, which at 600 uu/s is a flight the eye can follow), with `accel` halved with it so a boost
/// still takes the same second to come up to speed. The turn rates are the ones of the faster broom, so at
/// half the speed the same stick turns twice as tightly, which is what makes the snitch catchable.
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
    /// How fast the keys and the thumbstick swing the *look* in yaw at full deflection (rad/s). 2.0 rad/s is
    /// retail HP1's own 110 deg/s steering rate.
    pub yaw_rate: f32,
    /// The same for pitch (rad/s).
    pub pitch_rate: f32,
    /// The broom's own pitch is clamped to +-this (rad): about 80 deg, a backstop beyond the look's clamp.
    pub max_pitch: f32,
    /// How much of the gap to the look direction the broom closes per second (1/s, an exponential approach);
    /// the mod's camera tightness of 8/s, a 1/8 s time constant.
    pub chase_tightness: f32,
    /// and the most the broom may ever turn in a second (rad/s): the mod's effective `RotationRate`, yaw
    /// 274.7 deg/s and pitch 131.8 deg/s. This is the one hard cap on how fast the broom comes round.
    pub yaw_rate_max: f32,
    pub pitch_rate_max: f32,
    /// The look direction's own pitch clamp (rad): the mod's camera limit of 14000 units, 76.9 deg.
    pub max_look_pitch: f32,
}

impl Default for BroomTuning {
    fn default() -> Self {
        BroomTuning {
            speed: 600.0,
            boost_speed: 1200.0,
            accel: 750.0,
            decel: 150.0,
            yaw_rate: 2.0,
            pitch_rate: 1.5,
            max_pitch: 1.4,
            chase_tightness: 8.0,
            yaw_rate_max: 4.80,
            pitch_rate_max: 2.30,
            max_look_pitch: 1.342,
        }
    }
}

/// Advance the broom by `dt` seconds. Brake wins over boost.
///
/// The look direction moves first (mouse radians this frame, plus the keys' constant rate), then the broom
/// turns towards it: part of the gap each frame and never faster than the rate caps, so a flick of the mouse
/// is a turn the broom takes a moment to fly, not a snap.
pub fn step(s: &BroomState, i: &BroomInput, t: &BroomTuning, dt: f32) -> BroomState {
    let dt = if dt.is_finite() { dt.clamp(0.0, 0.25) } else { 0.0 };
    let look = |v: f32| if v.is_finite() { v } else { 0.0 };
    let want_yaw = fold(s.want_yaw + look(i.look_yaw) + i.yaw.clamp(-1.0, 1.0) * t.yaw_rate * dt);
    let want_pitch = (s.want_pitch + look(i.look_pitch) + i.pitch.clamp(-1.0, 1.0) * t.pitch_rate * dt)
        .clamp(-t.max_look_pitch, t.max_look_pitch);
    let want_pitch = if want_pitch.is_finite() { want_pitch } else { 0.0 };
    // Part of the gap, capped by the turn rate.
    let blend = (t.chase_tightness * dt).clamp(0.0, 1.0);
    let toward = |now: f32, want: f32, cap: f32| {
        let now = fold(now);
        now + (wrap_pi(want - now) * blend).clamp(-cap * dt, cap * dt)
    };
    let yaw = toward(s.yaw, want_yaw, t.yaw_rate_max);
    let pitch = toward(s.pitch, want_pitch, t.pitch_rate_max).clamp(-t.max_pitch, t.max_pitch);
    let target = if i.brake {
        t.decel
    } else if i.boost {
        t.boost_speed
    } else {
        t.speed
    };
    let cur = if s.vel.is_finite() { s.vel.length() } else { 0.0 };
    let max_delta = t.accel * dt;
    let speed = cur + (target - cur).clamp(-max_delta, max_delta);
    let fwd = Vec3::new(pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), pitch.sin());
    let vel = fwd * speed;
    BroomState { pos: s.pos + vel * dt, vel, yaw, pitch, want_yaw, want_pitch }
}

/// `wrap_pi`, but an angle already inside -pi..pi is handed back bit for bit: a heading nothing turns stays
/// exactly where it was instead of drifting by a rounding error every frame.
pub(crate) fn fold(a: f32) -> f32 {
    if a.is_finite() && a.abs() <= std::f32::consts::PI {
        a
    } else {
        wrap_pi(a)
    }
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

/// The soft boundary: the yaw turn-rate (-1..1) to add to the player's own yaw input (which steers the *look*
/// direction, so this is what swings the look back over the pitch) while `pos` lies outside
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
    if !out.want_yaw.is_finite() || !out.want_pitch.is_finite() {
        out.want_yaw = out.yaw;
        out.want_pitch = out.pitch;
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
    // of sticking. The *look* is levelled with it, or the broom would spend the whole slide fighting the look
    // the mouse left pointing into the wall.
    if (p.z < out.pos.z && out.pitch < 0.0) || (p.z > out.pos.z && out.pitch > 0.0) {
        out.pitch = 0.0;
        out.want_pitch = 0.0;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start() -> BroomState {
        BroomState::new(Vec3::ZERO, Vec3::ZERO, 0.3, 0.1)
    }

    fn none() -> BroomInput {
        BroomInput::default()
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

    /// Pitch is clamped: the look saturates at `max_look_pitch` (the mod's camera limit) and the broom, which
    /// chases the look, settles there and never passes its own `max_pitch` backstop.
    #[test]
    fn pitch_is_clamped() {
        let t = BroomTuning::default();
        let up = BroomInput { pitch: 1.0, ..none() };
        let down = BroomInput { pitch: -1.0, ..none() };
        let jump = BroomInput { look_pitch: 100.0, ..none() };
        let (mut a, mut b, mut c) = (start(), start(), start());
        for _ in 0..600 {
            a = step(&a, &up, &t, 1.0 / 60.0);
            b = step(&b, &down, &t, 1.0 / 60.0);
            c = step(&c, &jump, &t, 1.0 / 60.0);
            for s in [a, b, c] {
                assert!(s.pitch.abs() <= t.max_pitch + 1e-5, "{}", s.pitch);
                assert!(s.want_pitch.abs() <= t.max_look_pitch + 1e-5, "{}", s.want_pitch);
            }
        }
        assert!((a.want_pitch - t.max_look_pitch).abs() < 1e-5);
        assert!((a.pitch - t.max_look_pitch).abs() < 1e-3, "{}", a.pitch);
        assert!((b.pitch + t.max_look_pitch).abs() < 1e-3, "{}", b.pitch);
        // However far the mouse is thrown, the look never goes past the clamp either.
        assert!((c.want_pitch - t.max_look_pitch).abs() < 1e-5);
    }

    /// The mouse integrates the look direction: deltas add up, and nothing scales them by `dt` (the same
    /// motion in one long frame or in several short ones turns the broom the same amount).
    #[test]
    fn look_integrates_into_the_desired_heading() {
        let t = BroomTuning::default();
        let s = start();
        let one = step(&s, &BroomInput { look_yaw: 0.2, ..none() }, &t, 1.0 / 60.0);
        assert!((one.want_yaw - (s.want_yaw + 0.2)).abs() < 1e-5, "{}", one.want_yaw);
        let two = step(&one, &BroomInput { look_yaw: 0.2, ..none() }, &t, 1.0 / 60.0);
        assert!((two.want_yaw - (s.want_yaw + 0.4)).abs() < 1e-5, "{}", two.want_yaw);
        // The same motion over a frame four times as long moves the look by the same amount, not four times
        // as much: mouse pixels are already per-frame (the mod's own `dt` factor is the bug not ported).
        let long = step(&s, &BroomInput { look_yaw: 0.2, ..none() }, &t, 4.0 / 60.0);
        assert!((long.want_yaw - one.want_yaw).abs() < 1e-5, "{} vs {}", long.want_yaw, one.want_yaw);
        // The keys are a rate instead, so they do scale with the frame.
        let keyed = step(&s, &BroomInput { yaw: 1.0, ..none() }, &t, 1.0 / 60.0);
        assert!((keyed.want_yaw - (s.want_yaw + t.yaw_rate / 60.0)).abs() < 1e-5, "{}", keyed.want_yaw);
        // And the broom has started to come round towards the look, but is not there yet.
        assert!(one.yaw > s.yaw && one.yaw < one.want_yaw, "{} not between {} and {}", one.yaw, s.yaw, one.want_yaw);
    }

    /// The broom follows the look, and never turns faster than the rate caps however the look is thrown
    /// about -- including a mouse jump of half a turn in one frame.
    #[test]
    fn heading_follows_the_look_within_the_rate_limits() {
        let t = BroomTuning::default();
        let dt = 1.0 / 60.0;
        let jump = BroomInput { look_yaw: 2.9, look_pitch: -2.0, ..none() };
        let mut s = start();
        // The first frame: the look has moved a long way, the broom only a rate-limited step of it.
        let first = step(&s, &jump, &t, dt);
        assert!((first.yaw - s.yaw).abs() <= t.yaw_rate_max * dt + 1e-5, "{} rad in one frame", first.yaw - s.yaw);
        assert!((first.pitch - s.pitch).abs() <= t.pitch_rate_max * dt + 1e-5, "{}", first.pitch - s.pitch);
        assert!(wrap_pi(first.want_yaw - first.yaw).abs() > 1.0, "the broom must lag the look after one frame");
        // Held there, every frame keeps to the caps, and the broom does arrive.
        for n in 0..600 {
            let i = if n == 0 { jump } else { none() };
            let next = step(&s, &i, &t, dt);
            assert!(wrap_pi(next.yaw - s.yaw).abs() <= t.yaw_rate_max * dt + 1e-5, "frame {n}: yaw step too big");
            assert!((next.pitch - s.pitch).abs() <= t.pitch_rate_max * dt + 1e-5, "frame {n}: pitch step too big");
            s = next;
        }
        assert!(wrap_pi(s.yaw - s.want_yaw).abs() < 1e-3, "{} never reached {}", s.yaw, s.want_yaw);
        assert!((s.pitch - s.want_pitch).abs() < 1e-3, "{} never reached {}", s.pitch, s.want_pitch);
        // A look held hard over turns the broom at the cap, not faster: a quarter turn needs at least
        // `pi/2 / yaw_rate_max` seconds.
        let mut s = start();
        let mut frames = 0;
        while wrap_pi(s.yaw - start().yaw).abs() < std::f32::consts::FRAC_PI_2 && frames < 600 {
            s = step(&s, &BroomInput { look_yaw: 1.0, ..none() }, &t, dt);
            frames += 1;
        }
        assert!(frames < 600, "the broom never made a quarter turn");
        assert!(frames as f32 * dt >= std::f32::consts::FRAC_PI_2 / t.yaw_rate_max - dt, "{frames} frames is too quick");
    }

    /// Odd frame times (zero, enormous, negative, not a number) never put a NaN in the state.
    #[test]
    fn odd_dt_never_makes_a_nan() {
        let t = BroomTuning::default();
        let i = BroomInput { yaw: 1.0, pitch: -1.0, look_yaw: 0.3, look_pitch: f32::NAN, boost: true, brake: false };
        let mut s = start();
        for dt in [0.0, 1e-9, 1e6, -0.5, f32::NAN, f32::INFINITY, 0.25, 1.0 / 60.0] {
            s = step(&s, &i, &t, dt);
            assert!(s.pos.is_finite() && s.vel.is_finite(), "{s:?} at dt {dt}");
            assert!(s.yaw.is_finite() && s.pitch.is_finite(), "{s:?} at dt {dt}");
            assert!(s.want_yaw.is_finite() && s.want_pitch.is_finite(), "{s:?} at dt {dt}");
        }
        // Rubbish in the state itself comes out usable too.
        let bad = step(&BroomState { pos: Vec3::splat(f32::NAN), vel: Vec3::splat(f32::INFINITY), yaw: f32::NAN, pitch: f32::NAN, want_yaw: f32::NAN, want_pitch: f32::NAN }, &i, &t, 1.0 / 60.0);
        assert!(bad.vel.is_finite() && bad.yaw.is_finite() && bad.pitch.is_finite() && bad.want_yaw.is_finite() && bad.want_pitch.is_finite(), "{bad:?}");
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
        let s = BroomState::new(Vec3::new(hi.x + 500.0, 0.0, lo.z - 300.0), Vec3::new(900.0, 100.0, -700.0), 0.0, -0.9);
        let c = confine(&s, &b);
        assert_eq!(c.pos, Vec3::new(hi.x, 0.0, lo.z));
        assert!(c.vel.x.abs() < 1e-5 && c.vel.z.abs() < 1e-5, "{:?} still pushes through", c.vel);
        assert_eq!(c.vel.y, 100.0);
        assert_eq!(c.pitch, 0.0, "a broom on the floor levels out");
        assert_eq!(c.want_pitch, 0.0, "and so does where it is looking, or it fights the floor");
        // Inside, nothing changes at all.
        let inside = BroomState::new(b.center, Vec3::new(0.0, 1200.0, 0.0), 1.0, -0.3);
        assert_eq!(confine(&inside, &b), inside);
        // A broom flown at the ceiling for a while never leaves the volume, whatever it is told to do.
        let t = BroomTuning::default();
        let mut s = BroomState::new(Vec3::new(b.center.x, b.center.y, b.ceiling - 10.0), Vec3::ZERO, 0.6, 1.0);
        for _ in 0..2000 {
            s = confine(&step(&s, &BroomInput { yaw: 0.2, pitch: 1.0, boost: true, brake: false, ..none() }, &t, 1.0 / 60.0), &b);
            assert!(s.pos.x >= lo.x - 1e-3 && s.pos.x <= hi.x + 1e-3, "{:?}", s.pos);
            assert!(s.pos.y >= lo.y - 1e-3 && s.pos.y <= hi.y + 1e-3, "{:?}", s.pos);
            assert!(s.pos.z >= lo.z - 1e-3 && s.pos.z <= hi.z + 1e-3, "{:?}", s.pos);
        }
        // Rubbish in, something usable out: the broom must never be put out of the world.
        let bad = BroomState { pos: Vec3::splat(f32::NAN), vel: Vec3::splat(f32::INFINITY), yaw: f32::NAN, pitch: 0.0, want_yaw: f32::NAN, want_pitch: f32::INFINITY };
        let c = confine(&bad, &b);
        assert!(c.pos.is_finite() && c.vel.is_finite() && c.yaw.is_finite() && c.pitch.is_finite(), "{c:?}");
        assert!(c.want_yaw.is_finite() && c.want_pitch.is_finite(), "{c:?}");
    }

    #[test]
    fn step_is_deterministic() {
        let t = BroomTuning::default();
        let i = BroomInput { yaw: 0.5, pitch: -0.2, boost: true, brake: false, look_yaw: 0.01, look_pitch: -0.02 };
        assert_eq!(step(&start(), &i, &t, 0.016), step(&start(), &i, &t, 0.016));
    }
}
