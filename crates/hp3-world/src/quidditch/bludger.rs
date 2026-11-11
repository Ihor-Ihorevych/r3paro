//! Bludgers: launched at Harry from a fixed home point while he is in range.
//!
//! Units are Unreal units per second and seconds. Behaviour follows the HP1 reference: a random interval between
//! launches, the shot aimed at Harry plus a fraction of his velocity, a limited life, and enough hits cost him
//! control of the broom.

use glam::Vec3;

use super::flight::BroomState;
use super::rng::Rng;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BludgerTuning {
    /// Launches are refused while Harry is closer than this to the Bludger home: too close to be a fair shot.
    /// Must exceed `hit_radius`, or a launch can be an instant hit.
    pub launch_inner: f32,
    /// Launches happen only while Harry is within this distance of the Bludger home.
    pub launch_outer: f32,
    /// Shortest launch interval (s). The interval counts down only while Harry is in the launch band.
    pub launch_min: f32,
    /// Longest launch interval (s).
    pub launch_max: f32,
    /// Bludger flight speed (uu/s).
    pub speed: f32,
    /// 0..1 fraction of Harry's velocity added to the aim point.
    pub velocity_compensation: f32,
    /// Contact distance between a Bludger and Harry.
    pub hit_radius: f32,
    /// Seconds a Bludger flies before it expires as a miss.
    pub life: f32,
    /// Once this many hits have landed, control is lost.
    pub bumps_to_lose_control: u32,
}

impl Default for BludgerTuning {
    /// PLACEHOLDER values in Unreal units (a walking human is ~440 uu/s, a broom cruises ~1200 uu/s); to be tuned
    /// by playtest.
    fn default() -> Self {
        BludgerTuning {
            launch_inner: 800.0,
            launch_outer: 3500.0,
            launch_min: 3.0,
            launch_max: 6.0,
            // A little faster than the broom, so it can be outrun only by boosting.
            speed: 1500.0,
            velocity_compensation: 0.5,
            hit_radius: 150.0,
            life: 6.0,
            bumps_to_lose_control: 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BludgerEvent {
    Launched,
    Hit,
    Missed,
}

/// Hard cap: at most this many Bludgers are in flight at once (a launch waits until one is gone).
const MAX_IN_FLIGHT: usize = 2;
/// Longest dt a single step integrates; larger values are clamped.
const MAX_DT: f32 = 1.0;

#[derive(Debug, Clone, Copy)]
struct Flying {
    pos: Vec3,
    vel: Vec3,
    age: f32,
}

pub struct Bludgers {
    tuning: BludgerTuning,
    rng: Rng,
    home: Vec3,
    flying: Vec<Flying>,
    /// Seconds of in-band time left until the next launch.
    countdown: f32,
    bumps: u32,
}

/// Closest distance from `p` to the segment `a..b` (a == b is fine).
fn segment_distance(a: Vec3, b: Vec3, p: Vec3) -> f32 {
    let ab = b - a;
    let len2 = ab.length_squared();
    let t = if len2 > 1e-12 { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) } else { 0.0 };
    (a + ab * t).distance(p)
}

impl Bludgers {
    pub fn new(seed: u64) -> Bludgers {
        Bludgers::with_tuning(seed, BludgerTuning::default())
    }

    pub fn with_tuning(seed: u64, tuning: BludgerTuning) -> Bludgers {
        let mut b = Bludgers {
            tuning,
            rng: Rng::new(seed),
            home: Vec3::ZERO,
            flying: Vec::new(),
            countdown: 0.0,
            bumps: 0,
        };
        b.countdown = b.next_interval();
        b
    }

    /// Where Bludgers are launched from (default: the origin). Call before the match starts.
    pub fn set_home(&mut self, home: Vec3) {
        self.home = home;
    }

    fn next_interval(&mut self) -> f32 {
        let lo = self.tuning.launch_min.max(0.0);
        let hi = self.tuning.launch_max.max(lo);
        lo + (hi - lo) * self.rng.unit()
    }

    pub fn step(&mut self, harry: &BroomState, dt: f32) -> Vec<BludgerEvent> {
        let mut events = Vec::new();
        if !dt.is_finite() || dt <= 0.0 {
            return events;
        }
        let dt = dt.min(MAX_DT);
        let t = self.tuning;

        // Fly, then resolve contact (Hit wins over expiry) so a Bludger is counted exactly once.
        let mut kept = Vec::with_capacity(self.flying.len());
        for mut f in self.flying.drain(..) {
            let from = f.pos;
            f.pos += f.vel * dt;
            f.age += dt;
            if segment_distance(from, f.pos, harry.pos) <= t.hit_radius {
                self.bumps = self.bumps.saturating_add(1);
                events.push(BludgerEvent::Hit);
            } else if f.age >= t.life {
                events.push(BludgerEvent::Missed);
            } else {
                kept.push(f);
            }
        }
        self.flying = kept;

        // Launch: the countdown runs only while Harry is in the band [launch_inner, launch_outer].
        let d = harry.pos.distance(self.home);
        if d <= t.launch_outer && d >= t.launch_inner {
            self.countdown -= dt;
            if self.countdown <= 0.0 && self.flying.len() < MAX_IN_FLIGHT {
                let aim = harry.pos + harry.vel * t.velocity_compensation.clamp(0.0, 1.0);
                let dir = (aim - self.home).try_normalize().unwrap_or(Vec3::X);
                self.flying.push(Flying { pos: self.home, vel: dir * t.speed, age: 0.0 });
                events.push(BludgerEvent::Launched);
                self.countdown = self.next_interval();
            }
        }
        events
    }

    pub fn positions(&self) -> Vec<Vec3> {
        self.flying.iter().map(|f| f.pos).collect()
    }

    pub fn bumps(&self) -> u32 {
        self.bumps
    }

    /// How many hits cost Harry the broom, for the HUD's control bar.
    pub fn bumps_to_lose(&self) -> u32 {
        self.tuning.bumps_to_lose_control
    }

    /// True once `bumps >= bumps_to_lose_control`; stays true (construct a new `Bludgers` to reset).
    pub fn control_lost(&self) -> bool {
        self.bumps >= self.tuning.bumps_to_lose_control
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn harry_at(pos: Vec3) -> BroomState {
        BroomState { pos, vel: Vec3::ZERO, yaw: 0.0, pitch: 0.0 }
    }

    fn tuning() -> BludgerTuning {
        BludgerTuning {
            launch_inner: 300.0,
            launch_outer: 3000.0,
            launch_min: 2.0,
            launch_max: 4.0,
            speed: 1000.0,
            velocity_compensation: 0.5,
            hit_radius: 100.0,
            life: 3.0,
            bumps_to_lose_control: 2,
        }
    }

    /// Steps until the first Launched; returns the elapsed time.
    fn time_to_launch(b: &mut Bludgers, harry: &BroomState, dt: f32) -> f32 {
        let mut elapsed = 0.0;
        for _ in 0..10_000 {
            elapsed += dt;
            if b.step(harry, dt).contains(&BludgerEvent::Launched) {
                return elapsed;
            }
        }
        panic!("never launched");
    }

    #[test]
    fn no_launch_outside_outer_radius() {
        let mut b = Bludgers::with_tuning(1, tuning());
        let far = harry_at(Vec3::new(5000.0, 0.0, 0.0));
        for _ in 0..1000 {
            assert!(b.step(&far, 0.1).is_empty());
        }
        assert!(b.positions().is_empty());
    }

    #[test]
    fn no_launch_inside_inner_radius() {
        let mut b = Bludgers::with_tuning(1, tuning());
        let close = harry_at(Vec3::new(100.0, 0.0, 0.0));
        for _ in 0..1000 {
            assert!(!b.step(&close, 0.1).contains(&BludgerEvent::Launched));
        }
    }

    #[test]
    fn launches_inside_proximity_at_random_interval_within_min_max() {
        let harry = harry_at(Vec3::new(1500.0, 0.0, 0.0));
        let dt = 0.05;
        let mut times = Vec::new();
        for seed in 0..20 {
            let mut b = Bludgers::with_tuning(seed, tuning());
            let t = time_to_launch(&mut b, &harry, dt);
            assert!(t >= 2.0 - 1e-3 && t <= 4.0 + dt + 1e-3, "seed {seed}: {t}");
            times.push(t);
        }
        assert!(times.iter().any(|t| (t - times[0]).abs() > 0.1), "interval should vary: {times:?}");
        // Deterministic from the seed.
        let mut a = Bludgers::with_tuning(7, tuning());
        let mut c = Bludgers::with_tuning(7, tuning());
        assert_eq!(time_to_launch(&mut a, &harry, dt), time_to_launch(&mut c, &harry, dt));
    }

    #[test]
    fn launch_is_aimed_at_harry_plus_compensated_velocity() {
        let mut t = tuning();
        t.launch_min = 0.0;
        t.launch_max = 0.0;
        let mut b = Bludgers::with_tuning(3, t);
        let harry = BroomState { pos: Vec3::new(1000.0, 0.0, 0.0), vel: Vec3::new(0.0, 800.0, 0.0), yaw: 0.0, pitch: 0.0 };
        assert!(b.step(&harry, 0.1).contains(&BludgerEvent::Launched));
        // Aim point is (1000, 400, 0); move Harry away so nothing hits, then check the heading after 0.5 s.
        let _ = b.step(&harry_at(Vec3::new(0.0, 0.0, 9000.0)), 0.5);
        let p = b.positions()[0];
        let want = Vec3::new(1000.0, 400.0, 0.0).normalize() * 500.0;
        assert!(p.distance(want) < 1.0, "{p:?} vs {want:?}");
    }

    #[test]
    fn bludger_expires_after_life_as_missed() {
        let mut t = tuning();
        t.speed = 10.0;
        let mut b = Bludgers::with_tuning(1, t);
        let harry = harry_at(Vec3::new(2000.0, 0.0, 0.0));
        let start = time_to_launch(&mut b, &harry, 0.1);
        assert!(start > 0.0);
        assert_eq!(b.positions().len(), 1);
        let mut events = Vec::new();
        for _ in 0..40 {
            events.extend(b.step(&harry, 0.1).into_iter().filter(|e| *e != BludgerEvent::Launched));
            if !events.is_empty() {
                break;
            }
        }
        assert_eq!(events, vec![BludgerEvent::Missed]);
        assert_eq!(b.bumps(), 0);
        assert!(b.positions().is_empty());
    }

    #[test]
    fn contact_inside_hit_radius_is_hit_and_adds_a_bump() {
        let mut t = tuning();
        t.launch_min = 0.0;
        t.launch_max = 0.0;
        let mut b = Bludgers::with_tuning(1, t);
        let harry = harry_at(Vec3::new(1000.0, 0.0, 0.0));
        assert!(b.step(&harry, 0.1).contains(&BludgerEvent::Launched));
        let mut hit = false;
        for _ in 0..15 {
            let ev = b.step(&harry, 0.1);
            if ev.contains(&BludgerEvent::Hit) {
                hit = true;
                break;
            }
        }
        assert!(hit);
        assert_eq!(b.bumps(), 1);
        assert!(!b.control_lost());
    }

    #[test]
    fn control_lost_after_bumps_to_lose_control() {
        let mut t = tuning();
        t.launch_min = 0.0;
        t.launch_max = 0.0;
        let mut b = Bludgers::with_tuning(1, t);
        let harry = harry_at(Vec3::new(1000.0, 0.0, 0.0));
        for _ in 0..400 {
            b.step(&harry, 0.1);
            if b.control_lost() {
                break;
            }
        }
        assert_eq!(b.bumps(), 2);
        assert!(b.control_lost());
        for _ in 0..50 {
            b.step(&harry_at(Vec3::new(9000.0, 0.0, 0.0)), 0.1);
        }
        assert!(b.control_lost());
    }

    #[test]
    fn hit_and_expire_same_step_counts_once() {
        let mut t = tuning();
        t.launch_min = 0.0;
        t.launch_max = 0.0;
        t.life = 0.5;
        let mut b = Bludgers::with_tuning(1, t);
        let harry = harry_at(Vec3::new(500.0, 0.0, 0.0));
        assert!(b.step(&harry, 0.1).contains(&BludgerEvent::Launched));
        // Flies 500 uu through Harry and reaches life exactly in this step.
        let ev = b.step(&harry, 0.5);
        let resolved: Vec<_> = ev.iter().filter(|e| **e != BludgerEvent::Launched).collect();
        assert_eq!(resolved, vec![&BludgerEvent::Hit]);
        assert_eq!(b.bumps(), 1);
    }

    #[test]
    fn at_most_two_in_flight() {
        let mut t = tuning();
        t.launch_min = 0.0;
        t.launch_max = 0.0;
        t.speed = 1.0;
        t.life = 1000.0;
        let mut b = Bludgers::with_tuning(1, t);
        let harry = harry_at(Vec3::new(2000.0, 0.0, 0.0));
        for _ in 0..100 {
            b.step(&harry, 0.1);
        }
        assert_eq!(b.positions().len(), 2);
    }

    #[test]
    fn odd_dt_and_harry_on_bludger_do_not_panic_or_nan() {
        let mut t = tuning();
        t.launch_min = 0.0;
        t.launch_max = 0.0;
        let mut b = Bludgers::with_tuning(1, t);
        let harry = harry_at(Vec3::ZERO);
        for dt in [0.0, -1.0, f32::NAN, f32::INFINITY, 1e-9, 100.0, 0.016] {
            b.step(&harry, dt);
            assert!(b.positions().iter().all(|p| p.is_finite()));
        }
        let on = harry_at(Vec3::new(1000.0, 0.0, 0.0));
        b.step(&on, 0.1);
        for _ in 0..30 {
            b.step(&on, 0.0);
            b.step(&on, 0.05);
        }
        assert!(b.positions().iter().all(|p| p.is_finite()));
    }
}
