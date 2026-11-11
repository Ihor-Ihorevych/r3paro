//! Golden Snitch AI, catch progress bar and catch window.

use glam::Vec3;

use super::arena::ArenaBounds;
use super::flight::BroomState;
use super::rng::Rng;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnitchTuning {
    pub neutral_radius: f32,
    pub max_gain_radius: f32,
    pub max_loss_radius: f32,
    pub gain_rate: f32,
    pub loss_rate: f32,
    pub max_catch_time: f32,
    pub max_tries: u32,
    pub catch_radius: f32,
    /// How wide the Snitch loops around its own flight path (uu), and how fast it goes round (rad/s). HP1's
    /// Snitch visibly corkscrews as it travels, which is how the player picks it out of the sky; ours does the
    /// same. 0 for either is a Snitch that flies the bare path.
    pub circle_radius: f32,
    pub circle_rate: f32,
}

impl Default for SnitchTuning {
    /// Values in Unreal units (a walking human is ~440 uu/s, a broom cruises
    /// ~600 uu/s). The radii are distances, so halving the speeds left them alone.
    fn default() -> Self {
        SnitchTuning {
            // Full gain at or inside this distance.
            max_gain_radius: 600.0,
            // Progress neither rises nor falls at exactly this distance.
            neutral_radius: 1500.0,
            // Full loss at or beyond this distance; progress is zeroed (Lost).
            max_loss_radius: 4000.0,
            // Progress per second (the bar spans 0..1).
            gain_rate: 0.25,
            loss_rate: 0.15,
            // Seconds the catch window stays open once the bar is full.
            max_catch_time: 3.0,
            max_tries: 3,
            // Harry must be this close with the catch key held. **350 + `circle_radius`**: every distance
            // here is measured to the Snitch's real, circling position, so a Snitch looping 150 uu around its
            // path is up to 150 uu farther away than one flying the bare path. Widening the catch by exactly
            // the loop leaves the catch as hard as it was before the circling; nothing else was widened (the
            // progress radii are thousands of uu, and 150 uu of wobble in them is noise).
            catch_radius: 500.0,
            circle_radius: 150.0,
            circle_rate: 4.5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnitchEvent {
    ClosingIn,
    Lost,
    CatchStart,
    Caught,
    Missed,
    OutOfTries,
}

/// Snitch speeds (uu/s): idle, and when Harry is right on top of it. Half of what the first pass flew at,
/// the same factor the broom's own speeds were halved by, so the chase is as catchable as it was.
const SPEED_IDLE: f32 = 300.0;
const SPEED_NEAR: f32 = 700.0;
/// Longest dt a single step integrates; larger values are clamped.
const MAX_DT: f32 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Chasing,
    Catching { elapsed: f32 },
    Done,
}

pub struct Snitch {
    tuning: SnitchTuning,
    rng: Rng,
    /// The arena, and the same box brought in by `circle_radius`: the path is flown inside the inner box, so
    /// the loops around it never leave the arena and the radius never has to be cut short.
    lo: Vec3,
    hi: Vec3,
    path_lo: Vec3,
    path_hi: Vec3,
    /// Where the Snitch would be with no circling (the point that walks the waypoints), and where it really
    /// is: the path point plus this frame's loop offset. `pos` is what everything else sees -- the catch, the
    /// progress and the actor the VM moves.
    path: Vec3,
    pos: Vec3,
    waypoint: Vec3,
    /// How far round the loop it is (rad).
    angle: f32,
    progress: f32,
    tries: u32,
    phase: Phase,
}

impl Snitch {
    pub fn new(seed: u64, arena: &ArenaBounds) -> Snitch {
        Snitch::with_tuning(seed, arena, SnitchTuning::default())
    }

    pub fn with_tuning(seed: u64, arena: &ArenaBounds, tuning: SnitchTuning) -> Snitch {
        let lo = arena.center - arena.half;
        let hi_z = arena.ceiling.min(arena.center.z + arena.half.z).max(lo.z);
        let hi = Vec3::new(arena.center.x + arena.half.x, arena.center.y + arena.half.y, hi_z);
        // Room for the loop on every axis, and never more than half the box (a flat or tiny arena just gets a
        // smaller circle rather than an inside-out one).
        let span = (hi - lo).min_element().max(0.0);
        let r = if tuning.circle_radius.is_finite() { tuning.circle_radius.clamp(0.0, span * 0.5) } else { 0.0 };
        let mut s = Snitch {
            tuning: SnitchTuning { circle_radius: r, ..tuning },
            rng: Rng::new(seed),
            lo,
            hi,
            path_lo: lo + Vec3::splat(r),
            path_hi: hi - Vec3::splat(r),
            path: Vec3::ZERO,
            pos: Vec3::ZERO,
            waypoint: Vec3::ZERO,
            angle: 0.0,
            progress: 0.0,
            tries: 0,
            phase: Phase::Chasing,
        };
        // The loop starts somewhere of its own, so two Snitches of different seeds are not in step.
        s.angle = s.rng.unit() * std::f32::consts::TAU;
        s.path = s.random_point();
        s.waypoint = s.random_point();
        s.pos = s.circling();
        s
    }

    fn random_point(&mut self) -> Vec3 {
        let r = Vec3::new(self.rng.unit(), self.rng.unit(), self.rng.unit());
        self.path_lo + (self.path_hi - self.path_lo) * r
    }

    /// This frame's visible position: the path point with the loop offset added, in the plane across the
    /// direction of travel, so the Snitch corkscrews along its path instead of sliding down it.
    fn circling(&self) -> Vec3 {
        let r = self.tuning.circle_radius;
        if !(r > 0.0) {
            return self.path;
        }
        let to = self.waypoint - self.path;
        // A travel direction, and a pair of unit vectors across it. Up is the axis to cross with unless the
        // travel is itself nearly vertical, in which case X is.
        let dir = if to.length_squared() > 1e-6 { to.normalize() } else { Vec3::X };
        let away = if dir.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
        let u = dir.cross(away);
        let u = if u.length_squared() > 1e-6 { u.normalize() } else { Vec3::Y };
        let v = dir.cross(u);
        let p = self.path + (u * self.angle.cos() + v * self.angle.sin()) * r;
        if p.is_finite() {
            p.clamp(self.lo, self.hi)
        } else {
            self.path
        }
    }

    pub fn step(&mut self, harry: &BroomState, wants_catch: bool, dt: f32) -> Vec<SnitchEvent> {
        let dt = if dt.is_finite() { dt.clamp(0.0, MAX_DT) } else { 0.0 };
        let mut events = Vec::new();
        if self.phase == Phase::Done {
            return events;
        }
        let dist = (harry.pos - self.pos).length();
        let dist = if dist.is_finite() { dist } else { f32::MAX };
        let t = self.tuning;

        match self.phase {
            Phase::Chasing => self.update_progress(dist, dt, &mut events),
            Phase::Catching { elapsed } => {
                if wants_catch && dist <= t.catch_radius {
                    self.phase = Phase::Done;
                    events.push(SnitchEvent::Caught);
                } else if elapsed + dt > t.max_catch_time {
                    self.progress = 0.0;
                    self.tries += 1;
                    events.push(SnitchEvent::Missed);
                    if self.tries >= t.max_tries {
                        self.phase = Phase::Done;
                        events.push(SnitchEvent::OutOfTries);
                    } else {
                        self.phase = Phase::Chasing;
                    }
                } else {
                    self.phase = Phase::Catching { elapsed: elapsed + dt };
                }
            }
            Phase::Done => {}
        }

        self.fly(dist, dt);
        events
    }

    fn update_progress(&mut self, dist: f32, dt: f32, events: &mut Vec<SnitchEvent>) {
        let t = self.tuning;
        if dist >= t.max_loss_radius {
            if self.progress > 0.0 {
                self.progress = 0.0;
                events.push(SnitchEvent::Lost);
            }
            return;
        }
        let before = self.progress;
        if dist < t.neutral_radius {
            let span = (t.neutral_radius - t.max_gain_radius).max(f32::EPSILON);
            let k = ((t.neutral_radius - dist) / span).clamp(0.0, 1.0);
            self.progress += t.gain_rate * k * dt;
        } else if dist > t.neutral_radius {
            let span = (t.max_loss_radius - t.neutral_radius).max(f32::EPSILON);
            let k = ((dist - t.neutral_radius) / span).clamp(0.0, 1.0);
            self.progress -= t.loss_rate * k * dt;
        }
        self.progress = self.progress.clamp(0.0, 1.0);
        if before <= 0.0 && self.progress > 0.0 {
            events.push(SnitchEvent::ClosingIn);
        }
        if self.progress >= 1.0 {
            self.phase = Phase::Catching { elapsed: 0.0 };
            events.push(SnitchEvent::CatchStart);
        }
    }

    /// Move toward the current waypoint; faster the closer Harry is. The path speed is untouched by the
    /// circling: the loop is an offset around the path, not extra ground covered.
    fn fly(&mut self, dist: f32, dt: f32) {
        let near = (1.0 - dist / self.tuning.max_loss_radius.max(1.0)).clamp(0.0, 1.0);
        let speed = SPEED_IDLE + (SPEED_NEAR - SPEED_IDLE) * near;
        let to = self.waypoint - self.path;
        let left = to.length();
        let reach = speed * dt;
        if left <= reach || left <= f32::EPSILON {
            self.path = self.waypoint;
            self.waypoint = self.random_point();
        } else {
            self.path += to * (reach / left);
        }
        self.path = self.path.clamp(self.path_lo, self.path_hi);
        let rate = if self.tuning.circle_rate.is_finite() { self.tuning.circle_rate } else { 0.0 };
        self.angle = (self.angle + rate * dt).rem_euclid(std::f32::consts::TAU);
        self.pos = self.circling();
    }

    pub fn pos(&self) -> Vec3 {
        self.pos
    }

    /// Where the Snitch would be with no circling: the point on its flight path that `pos` loops around.
    /// Nothing in the game uses it -- it is here for the `HP3_QUID_DEBUG` trace, which prints the loop offset
    /// so the helix can be read off a headless run.
    pub fn path_pos(&self) -> Vec3 {
        self.path
    }

    pub fn progress(&self) -> f32 {
        self.progress
    }

    /// Catch attempts still to come, for the HUD's pips.
    pub fn tries_left(&self) -> u32 {
        self.tuning.max_tries.saturating_sub(self.tries)
    }

    /// True while the catch window is open (the bar is full and Harry has `max_catch_time` to grab it).
    pub fn in_catch_window(&self) -> bool {
        matches!(self.phase, Phase::Catching { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arena() -> ArenaBounds {
        ArenaBounds {
            center: Vec3::new(0.0, 0.0, 2000.0),
            half: Vec3::new(5000.0, 5000.0, 2000.0),
            ceiling: 3500.0,
        }
    }

    fn harry_at(pos: Vec3) -> BroomState {
        BroomState::new(pos, Vec3::ZERO, 0.0, 0.0)
    }

    /// Harry glued to the snitch, offset along +x by `d`.
    fn near(s: &Snitch, d: f32) -> BroomState {
        harry_at(s.pos() + Vec3::new(d, 0.0, 0.0))
    }

    /// Step with Harry at the snitch until `CatchStart` appears.
    fn raise_to_catch(s: &mut Snitch) {
        for _ in 0..1000 {
            let h = near(s, 0.0);
            if s.step(&h, false, 0.25).contains(&SnitchEvent::CatchStart) {
                return;
            }
        }
        panic!("never reached CatchStart");
    }

    fn raise_some(s: &mut Snitch) {
        let h = near(s, 100.0);
        s.step(&h, false, 0.5);
        let h = near(s, 100.0);
        s.step(&h, false, 0.5);
        assert!(s.progress() > 0.0 && s.progress() < 1.0);
    }

    #[test]
    fn progress_rises_when_close() {
        let mut s = Snitch::new(1, &arena());
        let h = near(&s, 100.0);
        let ev = s.step(&h, false, 0.1);
        assert!(s.progress() > 0.0);
        assert!(ev.contains(&SnitchEvent::ClosingIn));
    }

    #[test]
    fn progress_falls_when_far() {
        let mut s = Snitch::new(1, &arena());
        raise_some(&mut s);
        let before = s.progress();
        let far = SnitchTuning::default().neutral_radius + 1000.0;
        let h = near(&s, far);
        s.step(&h, false, 0.1);
        assert!(s.progress() < before);
        assert!(s.progress() > 0.0);
    }

    #[test]
    fn progress_holds_in_neutral_band() {
        let mut s = Snitch::new(1, &arena());
        raise_some(&mut s);
        let before = s.progress();
        let h = near(&s, SnitchTuning::default().neutral_radius);
        s.step(&h, false, 0.1);
        assert!((s.progress() - before).abs() < 1e-3, "{} vs {}", s.progress(), before);
    }

    #[test]
    fn losing_snitch_zeroes_progress() {
        let mut s = Snitch::new(1, &arena());
        raise_some(&mut s);
        let h = near(&s, SnitchTuning::default().max_loss_radius + 500.0);
        let ev = s.step(&h, false, 0.1);
        assert!(ev.contains(&SnitchEvent::Lost));
        assert_eq!(s.progress(), 0.0);
    }

    #[test]
    fn catch_succeeds_inside_catch_radius_when_wants_catch() {
        let mut s = Snitch::new(2, &arena());
        raise_to_catch(&mut s);
        let h = near(&s, 0.0);
        let ev = s.step(&h, true, 0.01);
        assert!(ev.contains(&SnitchEvent::Caught));
    }

    #[test]
    fn catch_fails_after_max_catch_time() {
        let mut s = Snitch::new(2, &arena());
        raise_to_catch(&mut s);
        let mut missed = false;
        for _ in 0..100 {
            let h = near(&s, SnitchTuning::default().catch_radius + 400.0);
            if s.step(&h, true, 0.25).contains(&SnitchEvent::Missed) {
                missed = true;
                break;
            }
        }
        assert!(missed);
        assert_eq!(s.progress(), 0.0);
    }

    #[test]
    fn out_of_tries_after_max_tries() {
        let tuning = SnitchTuning::default();
        let mut s = Snitch::new(3, &arena());
        let mut misses = 0;
        'outer: for _ in 0..tuning.max_tries + 2 {
            raise_to_catch(&mut s);
            for _ in 0..100 {
                let h = near(&s, tuning.catch_radius + 400.0);
                let ev = s.step(&h, true, 0.25);
                if ev.contains(&SnitchEvent::Missed) {
                    misses += 1;
                }
                if ev.contains(&SnitchEvent::OutOfTries) {
                    break 'outer;
                }
                if ev.contains(&SnitchEvent::Missed) {
                    break;
                }
            }
        }
        assert_eq!(misses, tuning.max_tries);
        assert!(s.step(&near(&s, 0.0), true, 0.25).is_empty());
    }

    #[test]
    fn snitch_stays_inside_bounds_for_10000_steps() {
        let a = arena();
        let mut s = Snitch::new(42, &a);
        let zmax = a.ceiling.min(a.center.z + a.half.z);
        for i in 0..10000 {
            let t = i as f32 * 0.05;
            let h = harry_at(Vec3::new(t.cos() * 3000.0, t.sin() * 3000.0, 2000.0));
            s.step(&h, i % 7 == 0, 1.0 / 60.0);
            let p = s.pos();
            assert!(p.is_finite());
            assert!((p.x - a.center.x).abs() <= a.half.x);
            assert!((p.y - a.center.y).abs() <= a.half.y);
            assert!(p.z >= a.center.z - a.half.z && p.z <= zmax);
        }
    }

    #[test]
    fn same_seed_same_path() {
        let a = arena();
        let mut s1 = Snitch::new(7, &a);
        let mut s2 = Snitch::new(7, &a);
        let mut s3 = Snitch::new(8, &a);
        let h = harry_at(Vec3::new(500.0, 0.0, 1500.0));
        for _ in 0..500 {
            s1.step(&h, false, 1.0 / 60.0);
            s2.step(&h, false, 1.0 / 60.0);
            s3.step(&h, false, 1.0 / 60.0);
            assert_eq!(s1.pos(), s2.pos());
        }
        assert_ne!(s1.pos(), s3.pos());
    }

    /// The Snitch really loops: its visible position sits a full `circle_radius` off the path it is flying,
    /// in the plane across the travel, and it goes round at `circle_rate`.
    #[test]
    fn circling_keeps_its_radius_around_the_path() {
        let t = SnitchTuning::default();
        let mut s = Snitch::new(11, &arena());
        let h = harry_at(Vec3::new(4000.0, 4000.0, 1000.0));
        let mut seen = [false; 4];
        let mut last = s.angle;
        for _ in 0..600 {
            s.step(&h, false, 1.0 / 60.0);
            let off = s.pos() - s.path;
            assert!((off.length() - t.circle_radius).abs() < 1e-2, "offset {} is not the radius", off.length());
            // Across the travel, so the loop shows from the side rather than being flown through.
            let dir = (s.waypoint - s.path).normalize();
            assert!(off.normalize().dot(dir).abs() < 1e-2, "the loop is not across the travel");
            // It turns at the tuned rate, and goes right round.
            let step = (s.angle - last).rem_euclid(std::f32::consts::TAU);
            assert!((step - t.circle_rate / 60.0).abs() < 1e-3, "turned {step} in a frame");
            last = s.angle;
            seen[(s.angle / std::f32::consts::FRAC_PI_2) as usize % 4] = true;
        }
        assert!(seen.iter().all(|&q| q), "the loop never went all the way round: {seen:?}");
    }

    /// The loop is an offset, not a detour: the path under it still covers exactly `speed * dt` a frame, and
    /// a Snitch tuned with no circling flies that bare path.
    #[test]
    fn circling_leaves_the_path_and_its_speed_alone() {
        let a = arena();
        let dt = 1.0 / 60.0;
        // Harry out past `max_loss_radius`, so the speed is the idle one and known.
        let h = harry_at(Vec3::new(50_000.0, 0.0, 2000.0));
        let mut s = Snitch::new(13, &a);
        let mut steps = 0;
        for _ in 0..600 {
            let was = s.path;
            s.step(&h, false, dt);
            let moved = s.path.distance(was);
            // Either a full step along the path, or the frame it reached a waypoint and turned.
            if (moved - SPEED_IDLE * dt).abs() < 1e-2 {
                steps += 1;
            } else {
                assert!(moved <= SPEED_IDLE * dt + 1e-2, "the loop sped the path up: {moved}");
            }
        }
        assert!(steps > 580, "only {steps} of 600 frames flew a clean path step");
        let bare = SnitchTuning { circle_radius: 0.0, ..SnitchTuning::default() };
        let mut straight = Snitch::with_tuning(13, &a, bare);
        for _ in 0..600 {
            straight.step(&h, false, dt);
        }
        assert_eq!(straight.pos(), straight.path, "a Snitch with no circling flies the bare path");
        // Same seed, same loop: the whole thing is deterministic, offset and all.
        let mut again = Snitch::new(13, &a);
        for _ in 0..600 {
            again.step(&h, false, dt);
        }
        assert_eq!(s.pos(), again.pos());
    }

    /// Circling never puts the Snitch outside the arena, however long it flies: the path is flown inside the
    /// arena brought in by the radius, so the loop around it still fits.
    #[test]
    fn circling_snitch_stays_inside_bounds_for_10000_steps() {
        let a = arena();
        let mut s = Snitch::new(21, &a);
        let zmax = a.ceiling.min(a.center.z + a.half.z);
        let mut off_path = 0.0f32;
        for i in 0..10000 {
            let t = i as f32 * 0.05;
            let h = harry_at(Vec3::new(t.cos() * 4800.0, t.sin() * 4800.0, 3400.0));
            s.step(&h, i % 7 == 0, 1.0 / 60.0);
            let p = s.pos();
            assert!(p.is_finite());
            assert!((p.x - a.center.x).abs() <= a.half.x, "{p:?}");
            assert!((p.y - a.center.y).abs() <= a.half.y, "{p:?}");
            assert!(p.z >= a.center.z - a.half.z && p.z <= zmax, "{p:?}");
            off_path = off_path.max(p.distance(s.path));
        }
        assert!((off_path - SnitchTuning::default().circle_radius).abs() < 1e-2, "the loop was cut short: {off_path}");
    }

    /// A circling Snitch is still catchable: the catch radius covers the loop, so flying onto the path point
    /// and reaching still gets it.
    #[test]
    fn a_circling_snitch_is_still_caught_from_its_path() {
        let mut s = Snitch::new(4, &arena());
        for _ in 0..1000 {
            let h = harry_at(s.path);
            if s.step(&h, false, 0.25).contains(&SnitchEvent::CatchStart) {
                break;
            }
        }
        assert!(s.in_catch_window(), "the bar never filled with Harry on the path");
        let h = harry_at(s.path);
        assert!(s.step(&h, true, 0.01).contains(&SnitchEvent::Caught), "a looping Snitch could not be caught");
    }

    #[test]
    fn odd_dt_never_panics_or_nans() {
        let mut s = Snitch::new(5, &arena());
        let h = harry_at(Vec3::new(10.0, 0.0, 1500.0));
        for dt in [0.0, 1e-9, 1e-3, 10.0, 1e9, f32::NAN, f32::INFINITY, -1.0] {
            s.step(&h, true, dt);
            assert!(s.pos().is_finite());
            assert!(s.progress().is_finite());
        }
    }
}
