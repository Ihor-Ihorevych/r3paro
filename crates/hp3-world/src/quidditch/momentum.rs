//! Harry's speed. There is no boost and no brake: the broom flies at a slow base speed, and every flight ring
//! he takes charges it up. The charge fills smoothly (a ring adds to a pending pool that drains into the
//! charge at a bounded rate) and, once he has gone a while without a ring, drains away just as smoothly (the
//! decay eases in from nothing, so there is no corner in the speed when it starts).

/// The numbers of the momentum, all placeholders tuned by feel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MomentumTuning {
    /// Speed with no charge at all (uu/s).
    pub base_speed: f32,
    /// Speed with a full charge (uu/s).
    pub max_speed: f32,
    /// What one ring adds to the charge (which spans 0..1).
    pub ring_charge: f32,
    /// What every `streak_len`-th ring in a row adds on top.
    pub streak_charge: f32,
    pub streak_len: u32,
    /// How fast pending charge turns into real charge (1/s).
    pub fill_rate: f32,
    /// Seconds after a ring before the charge starts to drain.
    pub grace: f32,
    /// Seconds the drain takes to ease in to its full rate, and that rate (charge per second).
    pub decay_ramp: f32,
    pub decay: f32,
    /// What a Bludger hit takes off the charge.
    pub hit_drain: f32,
    /// The boost: how long it lasts (s), how much speed it adds to the target (uu/s), and how long the
    /// cooldown after it is, drawn between the two (s).
    pub boost_time: f32,
    pub boost_add: f32,
    pub cooldown: (f32, f32),
}

impl Default for MomentumTuning {
    fn default() -> Self {
        MomentumTuning {
            base_speed: 370.0,
            max_speed: 760.0,
            // 0.22 and 0.15 originally, cut by 35% and then by another 25%.
            ring_charge: 0.107,
            streak_charge: 0.073,
            streak_len: 3,
            fill_rate: 0.6,
            grace: 1.2,
            decay_ramp: 1.5,
            decay: 0.12,
            hit_drain: 0.25,
            boost_time: 3.0,
            boost_add: 320.0,
            cooldown: (4.0, 5.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Momentum {
    tuning: MomentumTuning,
    /// The charge, 0..1.
    charge: f32,
    /// Charge a ring has earned that has not been poured in yet.
    pending: f32,
    /// Seconds since the last ring.
    since_ring: f32,
    /// Seconds of boost left, and seconds of cooldown left before the next may start.
    boost_left: f32,
    cooldown: f32,
    /// The stream the cooldown's length is drawn from.
    rng: u64,
}

impl Default for Momentum {
    fn default() -> Self {
        Momentum::new(MomentumTuning::default())
    }
}

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

impl Momentum {
    pub fn new(tuning: MomentumTuning) -> Momentum {
        Momentum { tuning, charge: 0.0, pending: 0.0, since_ring: 0.0, boost_left: 0.0, cooldown: 0.0, rng: 0x9E37_79B9_7F4A_7C15 }
    }

    /// Starts a boost if one is allowed (none running, none cooling down). It lasts `boost_time`, then the
    /// cooldown runs for a random 4 to 5 seconds. True if it started.
    pub fn try_boost(&mut self) -> bool {
        if self.boost_left > 0.0 || self.cooldown > 0.0 {
            return false;
        }
        self.boost_left = self.tuning.boost_time;
        true
    }

    /// 2 while a boost is running; otherwise how ready the next is, 0 (just ended) to 1 (ready).
    pub fn boost_state(&self) -> f32 {
        if self.boost_left > 0.0 {
            2.0
        } else if self.cooldown > 0.0 {
            let (lo, hi) = self.tuning.cooldown;
            (1.0 - self.cooldown / hi.max(lo).max(1e-3)).clamp(0.0, 0.99)
        } else {
            1.0
        }
    }

    fn next_cooldown(&mut self) -> f32 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        let u = (self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32 / (1u64 << 24) as f32;
        let (lo, hi) = self.tuning.cooldown;
        lo + (hi - lo) * u
    }

    pub fn tuning(&self) -> MomentumTuning {
        self.tuning
    }

    /// Harry took a ring, the `streak`-th in a row.
    pub fn on_ring(&mut self, streak: u32) {
        let t = self.tuning;
        let mut add = t.ring_charge;
        if t.streak_len > 0 && streak > 0 && streak % t.streak_len == 0 {
            add += t.streak_charge;
        }
        self.pending = (self.pending + add).min(1.0);
        self.since_ring = 0.0;
    }

    /// A Bludger hit him: part of the charge goes, and what was about to arrive goes with it.
    pub fn hit(&mut self) {
        self.charge = (self.charge - self.tuning.hit_drain).max(0.0);
        self.pending *= 0.5;
    }

    /// Knocked off the broom, or a missed catch: the charge is cut to `keep` of itself.
    pub fn cut(&mut self, keep: f32) {
        let keep = if keep.is_finite() { keep.clamp(0.0, 1.0) } else { 0.0 };
        self.charge *= keep;
        self.pending *= keep;
    }

    pub fn step(&mut self, dt: f32) {
        let dt = if dt.is_finite() { dt.clamp(0.0, 0.25) } else { 0.0 };
        let t = self.tuning;
        self.since_ring += dt;
        if self.boost_left > 0.0 {
            self.boost_left -= dt;
            if self.boost_left <= 0.0 {
                self.boost_left = 0.0;
                self.cooldown = self.next_cooldown();
            }
        } else if self.cooldown > 0.0 {
            self.cooldown = (self.cooldown - dt).max(0.0);
        }
        let pour = (self.pending).min(t.fill_rate * dt);
        self.pending -= pour;
        self.charge = (self.charge + pour).min(1.0);
        if self.pending <= 1e-4 && self.since_ring > t.grace {
            let ramp = smooth((self.since_ring - t.grace) / t.decay_ramp.max(1e-3));
            self.charge = (self.charge - t.decay * ramp * dt).max(0.0);
        }
    }

    /// The charge, 0..1 (for the HUD).
    pub fn charge(&self) -> f32 {
        self.charge
    }

    /// The speed the broom is easing toward (uu/s).
    pub fn target_speed(&self) -> f32 {
        let t = self.tuning;
        let boost = if self.boost_left > 0.0 { t.boost_add } else { 0.0 };
        t.base_speed + (t.max_speed - t.base_speed) * smooth(self.charge) + boost
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    #[test]
    fn no_rings_means_base_speed() {
        let mut m = Momentum::default();
        for _ in 0..600 {
            m.step(DT);
        }
        assert_eq!(m.target_speed(), MomentumTuning::default().base_speed);
    }

    #[test]
    fn a_ring_raises_the_target_smoothly() {
        let mut m = Momentum::default();
        m.on_ring(1);
        let mut last = m.target_speed();
        let mut biggest = 0.0f32;
        // A second of it: inside the grace, so nothing drains yet.
        for _ in 0..60 {
            m.step(DT);
            let now = m.target_speed();
            assert!(now >= last - 1e-3);
            biggest = biggest.max(now - last);
            last = now;
        }
        // One ring adds 0.107 of charge, which the smoothstep turns into about 12 uu/s.
        assert!(last > MomentumTuning::default().base_speed + 10.0, "{last}");
        assert!(biggest < 8.0, "a step of {biggest} uu/s in one frame is not smooth");
    }

    #[test]
    fn the_charge_holds_through_the_grace_then_drains_smoothly() {
        let t = MomentumTuning::default();
        let mut m = Momentum::default();
        for n in 1..=3 {
            m.on_ring(n);
        }
        // A second: the three rings and the streak bonus (0.39) have poured in, and it is still inside the grace.
        for _ in 0..60 {
            m.step(DT);
        }
        let full = m.charge();
        assert!(full > 0.35, "{full}");
        // Inside the grace nothing drains.
        let mut m2 = m;
        m2.since_ring = 0.5;
        m2.step(DT);
        assert!((m2.charge() - full).abs() < 1e-6);
        // Past it the drain eases in: the first frames lose almost nothing.
        let mut d = m;
        d.since_ring = t.grace + 0.01;
        let before = d.charge();
        d.step(DT);
        assert!(before - d.charge() < 0.0005);
        for _ in 0..60 * 20 {
            d.step(DT);
        }
        assert!(d.charge() < 1e-4, "{}", d.charge());
    }

    #[test]
    fn hits_and_cuts_take_charge() {
        let mut m = Momentum::default();
        for n in 1..=3 {
            m.on_ring(n);
        }
        // Poured in (0.39) and not yet drained, so a hit's 0.25 comes off something that is there.
        for _ in 0..60 {
            m.step(DT);
        }
        let c = m.charge();
        m.hit();
        assert!((m.charge() - (c - 0.25)).abs() < 1e-5);
        m.cut(0.0);
        assert_eq!(m.charge(), 0.0);
        m.step(f32::NAN);
        assert!(m.target_speed().is_finite());
    }
}
