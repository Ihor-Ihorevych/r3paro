//! The Quaffle: the ball the Chasers throw at the hoops. Pure: nothing here touches the script VM.
//!
//! It is either carried (`holder` set, and whoever carries it writes its position) or free, in which case it
//! flies ballistically and `step_and_score` reports the hoop it goes through. There are no Chasers yet
//! (Task 13), so today only the debug shot of `quidditch::ball` throws it.
//!
//! **Every number here is a placeholder, tuned by feel like the rest of the match** (the design doc leaves
//! them to the builder; HP1's own values are unknown).

use glam::Vec3;

use super::arena::{self, ArenaBounds};
use super::goals::{goal_scored, Hoop};

/// Who is holding the ball. Players are a Task 13 thing; this is the smallest thing that can name one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerId(pub u32);

/// Downward acceleration on a free ball (uu/s^2). A placeholder close to the engine's own default gravity.
pub const GRAVITY: f32 = -980.0;
/// How fast a pass leaves the hand when the caller has no opinion (uu/s).
pub const PASS_SPEED: f32 = 1400.0;
/// How fast a shot at a hoop leaves the hand when the caller has no opinion (uu/s).
pub const SHOOT_SPEED: f32 = 2200.0;
/// How long after a goal the same ball may score again (s). The latch that keeps a ball sitting in the ring
/// from scoring every frame; in play the ball is put back to the centre anyway.
pub const GOAL_COOLDOWN: f32 = 1.5;
/// How big the ball is drawn (uu, radius: a 30 uu ball) and how high its centre rests over the turf.
pub const RADIUS: f32 = 15.0;

/// The Quaffle, free or carried.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quaffle {
    pub pos: Vec3,
    pub vel: Vec3,
    /// Who is carrying it, if anyone. A carried ball is not stepped: its holder places it.
    pub holder: Option<PlayerId>,
    /// Where it was before the last step, which is what the goal test needs.
    prev: Vec3,
    /// Seconds left of the no-second-goal latch.
    cooldown: f32,
}

impl Quaffle {
    /// A free ball at rest at `pos`.
    pub fn new(pos: Vec3) -> Quaffle {
        let pos = if pos.is_finite() { pos } else { Vec3::ZERO };
        Quaffle { pos, vel: Vec3::ZERO, holder: None, prev: pos, cooldown: 0.0 }
    }

    /// Where it was before the last step.
    pub fn prev(&self) -> Vec3 {
        self.prev
    }

    /// One step of the ball. A carried ball stays where its holder put it; a free one falls.
    ///
    /// Gravity is integrated exactly (`p + v*dt + g*dt^2/2`), so a pass lands where it was aimed whatever the
    /// frame rate is.
    pub fn step(&mut self, dt: f32) {
        let dt = if dt.is_finite() { dt.clamp(0.0, 0.25) } else { 0.0 };
        self.cooldown = (self.cooldown - dt).max(0.0);
        self.prev = self.pos;
        if self.holder.is_some() {
            self.vel = Vec3::ZERO;
            return;
        }
        if !self.pos.is_finite() || !self.vel.is_finite() {
            self.vel = Vec3::ZERO;
            self.pos = if self.pos.is_finite() { self.pos } else { Vec3::ZERO };
            self.prev = self.pos;
            return;
        }
        let g = Vec3::new(0.0, 0.0, GRAVITY);
        self.pos += self.vel * dt + g * (0.5 * dt * dt);
        self.vel += g * dt;
    }

    /// Steps the ball and reports which of `hoops` it went through, at most one goal per step and never two
    /// inside `GOAL_COOLDOWN` (the latch, so a ball that comes to rest in the ring scores once).
    pub fn step_and_score(&mut self, dt: f32, hoops: &[Hoop]) -> Option<usize> {
        self.step(dt);
        if self.cooldown > 0.0 {
            return None;
        }
        let i = hoops.iter().position(|h| goal_scored(self.prev, self.pos, h))?;
        self.cooldown = GOAL_COOLDOWN;
        Some(i)
    }

    /// Throws the free ball from `from` at `to`, fast enough to get there in one arc: `speed` is how fast it
    /// leaves the hand, and the lift that cancels the drop over the flight is added, so it arrives where it
    /// was aimed instead of short and low.
    fn throw(&mut self, from: Vec3, to: Vec3, speed: f32, fallback: f32) {
        self.holder = None;
        self.pos = if from.is_finite() { from } else { Vec3::ZERO };
        self.prev = self.pos;
        self.vel = Vec3::ZERO;
        let speed = if speed.is_finite() && speed > 1.0 { speed } else { fallback };
        let away = to - self.pos;
        if !away.is_finite() || away.length() < 1e-3 {
            return;
        }
        let t = away.length() / speed;
        self.vel = away / t + Vec3::new(0.0, 0.0, -GRAVITY * t * 0.5);
    }

    /// Passes the ball from `from` to `to` (another player's position).
    pub fn pass(&mut self, from: Vec3, to: Vec3, speed: f32) {
        self.throw(from, to, speed, PASS_SPEED);
    }

    /// Shoots the ball from `from` at a hoop's centre.
    pub fn shoot(&mut self, from: Vec3, hoop: Vec3, speed: f32) {
        self.throw(from, hoop, speed, SHOOT_SPEED);
    }

    /// Brings the ball inside the arena and stops it dead where it hits the ground or the ceiling, so a ball
    /// nobody is playing with lies on the pitch instead of falling out of the world.
    ///
    /// Its floor is the turf itself (`GROUND_Z` plus its own radius), not the flight volume's: that 150 uu of
    /// clearance is there to keep brooms out of the pitch mesh, and a ball that came to rest on it would
    /// hover over the grass.
    pub fn confine(&mut self, b: &ArenaBounds) {
        if !self.pos.is_finite() {
            self.pos = b.center;
            self.prev = self.pos;
            self.vel = Vec3::ZERO;
            return;
        }
        if !self.vel.is_finite() {
            self.vel = Vec3::ZERO;
        }
        let lo = Vec3::new(b.center.x - b.half.x, b.center.y - b.half.y, (arena::GROUND_Z + RADIUS).min(b.ceiling));
        let hi = Vec3::new(b.center.x + b.half.x, b.center.y + b.half.y, b.ceiling.max(lo.z));
        let p = self.pos.clamp(lo, hi);
        if p != self.pos {
            self.pos = p;
            // On the ground (or against the ceiling) it comes to rest rather than bouncing.
            if p.z <= lo.z || p.z >= hi.z {
                self.vel = Vec3::ZERO;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    /// A hoop like the north goal's: facing -y, which is into the pitch.
    fn hoop() -> Hoop {
        Hoop { center: Vec3::new(256.0, 4096.0, 745.0), radius: 120.0, normal: Vec3::new(0.0, -1.0, 0.0), home: true }
    }

    /// Steps `q` for `steps` frames and counts the goals through `hoops`.
    fn run(q: &mut Quaffle, hoops: &[Hoop], steps: u32) -> u32 {
        let mut goals = 0;
        for _ in 0..steps {
            if q.step_and_score(DT, hoops).is_some() {
                goals += 1;
            }
        }
        goals
    }

    #[test]
    fn quaffle_through_hoop_front_scores() {
        let h = hoop();
        let from = Vec3::new(h.center.x, 0.0, h.center.z);
        let mut q = Quaffle::new(from);
        q.shoot(from, h.center, SHOOT_SPEED);
        assert_eq!(run(&mut q, &[h], 240), 1, "a shot from the pitch through the hoop is a goal");
        assert!(q.pos.y > h.center.y, "it carried on through: {:?}", q.pos);
    }

    #[test]
    fn quaffle_through_hoop_from_behind_does_not_score() {
        let h = hoop();
        // Thrown from behind the goal line back at the pitch: through the same ring, the wrong way.
        let from = Vec3::new(h.center.x, h.center.y + 600.0, h.center.z);
        let mut q = Quaffle::new(from);
        q.shoot(from, Vec3::new(h.center.x, 0.0, h.center.z), SHOOT_SPEED);
        assert_eq!(run(&mut q, &[h], 240), 0, "the back of the hoop does not score");
        assert!(q.pos.y < h.center.y, "it did cross the plane: {:?}", q.pos);
    }

    #[test]
    fn quaffle_missing_the_ring_does_not_score() {
        let h = hoop();
        let from = Vec3::new(h.center.x, 0.0, h.center.z);
        // Aimed a ring and a half wide of the centre, at the same height: past the post.
        let mut q = Quaffle::new(from);
        q.shoot(from, h.center + Vec3::new(h.radius * 3.0, 0.0, 0.0), SHOOT_SPEED);
        assert_eq!(run(&mut q, &[h], 240), 0, "wide of the ring is no goal");
        assert!(q.pos.y > h.center.y, "it did cross the goal line: {:?}", q.pos);
    }

    #[test]
    fn quaffle_obeys_gravity_when_free() {
        let mut q = Quaffle::new(Vec3::new(0.0, 0.0, 1000.0));
        let start = q.pos.z;
        run(&mut q, &[], 60);
        assert!(q.pos.z < start - 100.0, "a free ball falls: {} -> {}", start, q.pos.z);
        assert!(q.vel.z < -100.0, "and it is still falling: {:?}", q.vel);
        // Held, it hangs where it is put.
        let mut q = Quaffle::new(Vec3::new(0.0, 0.0, 1000.0));
        q.holder = Some(PlayerId(3));
        run(&mut q, &[], 60);
        assert_eq!(q.pos, Vec3::new(0.0, 0.0, 1000.0), "a carried ball does not fall");
        // And it comes to rest on the turf instead of falling out of the world.
        let b = arena::bounds();
        let mut q = Quaffle::new(b.center);
        for _ in 0..600 {
            q.step(DT);
            q.confine(&b);
        }
        assert!((q.pos.z - (arena::GROUND_Z + RADIUS)).abs() < 1e-3, "{:?} is not on the turf", q.pos);
        assert_eq!(q.vel, Vec3::ZERO);
        // Rubbish is brought back to the middle of the pitch, never left as a NaN on an actor.
        let mut q = Quaffle::new(b.center);
        q.pos = Vec3::splat(f32::NAN);
        q.confine(&b);
        assert_eq!(q.pos, b.center);
    }

    #[test]
    fn pass_arrives_near_target() {
        let from = Vec3::new(0.0, 0.0, 500.0);
        for to in [Vec3::new(1200.0, 0.0, 500.0), Vec3::new(-800.0, 1600.0, 1200.0), Vec3::new(300.0, -2400.0, 100.0)] {
            let mut q = Quaffle::new(from);
            q.pass(from, to, PASS_SPEED);
            let mut best = f32::MAX;
            for _ in 0..600 {
                q.step(DT);
                best = best.min(q.pos.distance(to));
            }
            // Within one frame of travel of where it was aimed.
            assert!(best < PASS_SPEED * DT * 2.0, "pass to {to:?} got no closer than {best}");
        }
    }

    #[test]
    fn one_goal_scores_once() {
        let h = hoop();
        let from = Vec3::new(h.center.x, h.center.y - 300.0, h.center.z);
        let mut q = Quaffle::new(from);
        q.shoot(from, h.center, 600.0);
        let mut goals = 0;
        for _ in 0..100 {
            if q.step_and_score(DT, &[h]).is_some() {
                goals += 1;
            }
            // It hangs about in the ring rather than flying on: put back just in front of the disc every
            // frame, which without the latch would score again and again.
            if q.pos.y > h.center.y {
                q.pos = Vec3::new(h.center.x, h.center.y - 5.0, h.center.z);
                q.vel = Vec3::new(0.0, 600.0, 0.0);
            }
        }
        assert_eq!(goals, 1, "a ball that stays in the hoop scores once");
    }
}
