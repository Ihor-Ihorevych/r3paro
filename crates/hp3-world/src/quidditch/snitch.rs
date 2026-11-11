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
            // Harry must be this close with the catch key held.
            catch_radius: 350.0,
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
    lo: Vec3,
    hi: Vec3,
    pos: Vec3,
    waypoint: Vec3,
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
        let mut s = Snitch {
            tuning,
            rng: Rng::new(seed),
            lo,
            hi,
            pos: Vec3::ZERO,
            waypoint: Vec3::ZERO,
            progress: 0.0,
            tries: 0,
            phase: Phase::Chasing,
        };
        s.pos = s.random_point();
        s.waypoint = s.random_point();
        s
    }

    fn random_point(&mut self) -> Vec3 {
        let r = Vec3::new(self.rng.unit(), self.rng.unit(), self.rng.unit());
        self.lo + (self.hi - self.lo) * r
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

    /// Move toward the current waypoint; faster the closer Harry is.
    fn fly(&mut self, dist: f32, dt: f32) {
        let near = (1.0 - dist / self.tuning.max_loss_radius.max(1.0)).clamp(0.0, 1.0);
        let speed = SPEED_IDLE + (SPEED_NEAR - SPEED_IDLE) * near;
        let to = self.waypoint - self.pos;
        let left = to.length();
        let reach = speed * dt;
        if left <= reach || left <= f32::EPSILON {
            self.pos = self.waypoint;
            self.waypoint = self.random_point();
        } else {
            self.pos += to * (reach / left);
        }
        self.pos = self.pos.clamp(self.lo, self.hi);
    }

    pub fn pos(&self) -> Vec3 {
        self.pos
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
