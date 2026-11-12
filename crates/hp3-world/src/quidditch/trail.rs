//! The Snitch's ring trail: the flight rings it drops behind itself, and what Harry gets for flying through
//! them. Pure: nothing here touches the script VM (`quidditch::rings` is the actors' side).
//!
//! HP1's Snitch optionally leaves a hoop trail behind it (`BroomHoopTrail`, with `HoopTrailLen` and
//! `fHoopSpacing`), and its Seeker game has three modes: plain proximity, proximity with the hoops shown as
//! guides, and `PM_Hoops`, in which the Snitch is only caught by flying through N hoops **in a row**. Ours is
//! the middle one: the rings are a guide and a bonus, never a requirement -- see
//! `docs/formats/quidditch-arena.md`.
//!
//! A ring is dropped on the Snitch's **flight path** -- the line it corkscrews around (`Snitch::path_pos`),
//! not the loop itself. Dropping them where the Snitch is really seen was tried and rendered: the loop is 150
//! uu wide, so consecutive rings sit on alternate sides of the path and the trail reads as a cloud of beads
//! instead of a row of hoops. On the path they are a chain the player can fly down, which is what HP1's hoop
//! trail is for. The normal is the Snitch's travel direction there, so each ring faces the chase. Every centre
//! therefore comes out of the Snitch, which flies inside the arena, and the trail is bounded by it.

use glam::Vec3;

use super::goals;

/// One flight ring of the trail.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ring {
    pub center: Vec3,
    /// Unit normal: the Snitch's travel direction where the ring was dropped.
    pub normal: Vec3,
    pub radius: f32,
    /// Seconds since it was dropped.
    pub age: f32,
    /// Which ring of the match this is, counted from the first one dropped. It is what `TrailEvent::Through`
    /// reports (the place in the live list shifts as old rings go) and what gives a ring its actor slot.
    pub id: u64,
    /// Whether it has been flown through or gone past: either way it is finished with and never fires again.
    done: bool,
}

/// What the ring trail reports in a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrailEvent {
    /// Harry flew through ring `index` (its `Ring::id`); `streak` is how many he has now taken in a row.
    Through { index: u64, streak: u32 },
    /// He went past a ring without going through it; `streak_lost` is the run it cost him (0 if there was
    /// none). It costs nothing else.
    Missed { streak_lost: u32 },
}

/// The numbers of the ring trail, all placeholders tuned by feel like the rest of the match.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrailTuning {
    /// How much of the Snitch's path there is between rings (uu). A ring and a half of clear air between
    /// rims, so the chain reads as a line of hoops rather than a row of separate targets, and short enough
    /// against the path's turn radius that the turn from one ring to the next stays inside `CHAIN_MAX_TURN`.
    pub spacing: f32,
    /// How many rings of the trail are alive at once; the oldest goes when a new one is dropped. Six of them
    /// is about seven seconds of the Snitch's flying with Harry on it, plenty for a run of three, and little
    /// enough that the sky over the pitch is not a cloud of gold (and that the phone draws six rings).
    pub max_rings: usize,
    /// How wide a ring is (uu). Wider than a goal hoop (120): a ring has to be fair to fly through at a
    /// broom's 450 (900 boosted) uu/s.
    pub radius: f32,
    /// How long a ring lasts (s). 6 s against the 875 uu/s the Snitch flies with Harry on it and a `spacing`
    /// of 700 is 7.5 rings' worth, so in a chase it is `max_rings` that decides and the full six are always
    /// up; a Snitch left alone (375 uu/s) shows about three, which is what a trail going cold should look
    /// like. (With the old 900 `spacing` this wanted 7 s for six rings; 700 buys the second back.)
    pub life: f32,
    /// How far past a ring's plane Harry must be before it counts as gone past (uu).
    pub miss_behind: f32,
    /// and how near its centre he must have been for that to count as a miss at all (uu): a ring on the far
    /// side of the pitch is one he never went for, and it expires without a word.
    pub miss_near: f32,
    /// What one ring adds to the catch bar (which spans 0..1).
    pub ring_bonus: f32,
    /// How many in a row earn the bigger bonus, and what it is (on top of the ring's own).
    pub streak_len: u32,
    pub streak_bonus: f32,
    /// How many rings Harry must take **in a row** before the Snitch may be caught at all: HP1's `PM_Hoops`
    /// rule. Off by default (0): the rings are a guide and a bonus, and the catch works by proximity as before.
    /// When set, the bar still fills by closing in, but the catch window stays shut until the run is in
    /// (`Snitch::set_rings_ready`), so nobody loses a try waiting for it.
    pub catch_rings: u32,
}

impl Default for TrailTuning {
    fn default() -> TrailTuning {
        TrailTuning {
            // Half as many rings: twice the gap, and half as many standing at once (the same stretch of sky).
            spacing: 1200.0,
            max_rings: 3,
            radius: 220.0,
            life: 7.0,
            miss_behind: 300.0,
            miss_near: 900.0,
            // About four rings' worth of the proximity gain (`SnitchTuning::gain_rate` is 0.25 a second), so
            // the trail is worth flying and still not a way of catching the Snitch without closing on it.
            ring_bonus: 0.06,
            streak_len: 3,
            streak_bonus: 0.15,
            // HP1's `PM_Hoops` rule: no catch window until six rings in a row (the owner's word: harder).
            catch_rings: 6,
        }
    }
}

/// Longest dt a single step integrates; larger values are clamped (as the Snitch's own step does).
const MAX_DT: f32 = 1.0;

/// The rings behind the Snitch and the streak Harry is on.
#[derive(Debug, Clone)]
pub struct RingTrail {
    t: TrailTuning,
    /// Oldest first.
    rings: Vec<Ring>,
    next_id: u64,
    streak: u32,
    /// Snitch path flown since the last ring was dropped (uu).
    since: f32,
    last_snitch: Option<Vec3>,
    /// While set, no ring is dropped (the Snitch's dodge: its path bends harder than a chain may).
    paused: bool,
}

impl RingTrail {
    /// A trail of `max_rings` rings of `radius`, one every `spacing` uu of the Snitch's path, each lasting
    /// `life` seconds. Everything else is `TrailTuning::default`.
    pub fn new(spacing: f32, max_rings: usize, radius: f32, life: f32) -> RingTrail {
        RingTrail::with_tuning(TrailTuning { spacing, max_rings, radius, life, ..TrailTuning::default() })
    }

    pub fn with_tuning(t: TrailTuning) -> RingTrail {
        RingTrail { t, rings: Vec::new(), next_id: 0, streak: 0, since: 0.0, last_snitch: None, paused: false }
    }

    /// Stops (or resumes) dropping rings. The rings already up carry on as they were.
    pub fn pause_drops(&mut self, paused: bool) {
        self.paused = paused;
    }

    pub fn tuning(&self) -> TrailTuning {
        self.t
    }

    /// The rings alive now, oldest first.
    pub fn rings(&self) -> &[Ring] {
        &self.rings
    }

    /// Rings flown through in a row; a ring gone past resets it.
    pub fn streak(&self) -> u32 {
        self.streak
    }

    /// The oldest ring still in play (neither flown through nor gone past): the one Harry is meant to take
    /// next, and what the debug autofly steers at.
    pub fn next_ring(&self) -> Option<&Ring> {
        self.rings.iter().find(|r| !r.done)
    }

    /// Whether Harry has the run of rings the catch needs (`TrailTuning::catch_rings`): HP1's `PM_Hoops`
    /// rule. Handed to `Snitch::set_rings_ready` every frame by `quidditch::rings`.
    pub fn catch_ready(&self) -> bool {
        self.streak >= self.t.catch_rings
    }

    /// Whether the rings up now read as one smooth road (`chain_is_smooth` against this trail's `spacing`).
    pub fn chain_is_smooth(&self) -> bool {
        chain_is_smooth(&self.rings, self.t.spacing)
    }

    /// One frame: ages the rings, drops a new one when the Snitch has flown `spacing` since the last, and
    /// answers what Harry's step from `harry_prev` to `harry_now` did to them.
    pub fn step(&mut self, snitch_pos: Vec3, snitch_dir_or_vel: Vec3, harry_prev: Vec3, harry_now: Vec3, dt: f32) -> Vec<TrailEvent> {
        let dt = if dt.is_finite() { dt.clamp(0.0, MAX_DT) } else { 0.0 };
        let mut out = Vec::new();
        let life = if self.t.life.is_finite() { self.t.life.max(0.0) } else { 0.0 };
        for r in self.rings.iter_mut() {
            r.age += dt;
        }
        // Expiry is silent: a ring nobody went for is not a miss.
        self.rings.retain(|r| r.age <= life);
        self.drop_ring(snitch_pos, snitch_dir_or_vel);
        self.check_harry(harry_prev, harry_now, &mut out);
        out
    }

    /// Drops a ring at the Snitch when it has flown `spacing` since the last one (and one straight away the
    /// first time it is seen, so the trail starts with the chase).
    fn drop_ring(&mut self, snitch_pos: Vec3, dir: Vec3) {
        if !snitch_pos.is_finite() || self.t.max_rings == 0 || !(self.t.radius > 0.0) {
            return;
        }
        let was = self.last_snitch.replace(snitch_pos);
        if self.paused {
            return;
        }
        let spacing = if self.t.spacing.is_finite() { self.t.spacing.max(1.0) } else { 1.0 };
        match was {
            Some(p) => {
                let moved = p.distance(snitch_pos);
                self.since += if moved.is_finite() { moved } else { 0.0 };
                if self.since < spacing {
                    return;
                }
            }
            None => {}
        }
        // The overshoot is carried over rather than thrown away, so the gaps along the chain stay a `spacing`
        // apart to within a frame's flying instead of drifting by one every ring. Half a spacing of credit at
        // most: a frame's overshoot is tens of uu, and a Snitch that jumped the length of the pitch must not
        // then drop a ring a frame for ever.
        self.since = (self.since - spacing).clamp(0.0, spacing * 0.5);
        // The travel direction, or the step the Snitch just took, or anything at all rather than a zero
        // normal (which no segment can ever cross).
        let mut n = dir.normalize_or_zero();
        if n == Vec3::ZERO {
            n = was.map(|p| snitch_pos - p).unwrap_or(Vec3::X).normalize_or_zero();
        }
        if n == Vec3::ZERO {
            n = Vec3::X;
        }
        if self.rings.len() >= self.t.max_rings {
            self.rings.remove(0);
        }
        self.rings.push(Ring { center: snitch_pos, normal: n, radius: self.t.radius, age: 0.0, id: self.next_id, done: false });
        self.next_id += 1;
    }

    /// Harry's step against every ring still in play: through it (either way round) or past it.
    fn check_harry(&mut self, prev: Vec3, now: Vec3, out: &mut Vec<TrailEvent>) {
        let (behind, near) = (self.t.miss_behind.max(0.0), self.t.miss_near.max(0.0));
        for r in self.rings.iter_mut() {
            if r.done {
                continue;
            }
            if goals::disc_crossing(prev, now, r.center, r.normal, r.radius).is_some() {
                r.done = true;
                self.streak += 1;
                out.push(TrailEvent::Through { index: r.id, streak: self.streak });
            } else if now.is_finite() && (now - r.center).dot(r.normal) > behind && now.distance(r.center) < near {
                r.done = true;
                out.push(TrailEvent::Missed { streak_lost: std::mem::take(&mut self.streak) });
            }
        }
    }
}

/// The most a ring may be turned from the one before it (rad, about 25 degrees) for the chain to still read
/// as one curve. The path's own curvature cap is set from this: `SnitchTuning::path_turn_rate` holds the
/// Snitch's path to a turn radius of `spacing / CHAIN_MAX_TURN` or wider, so this bound is met by
/// construction -- see `docs/formats/quidditch-arena.md`.
/// (0.85 rad since the rings are 1200 apart: the same 1508 uu turn radius bends twice as far between two.)
pub const CHAIN_MAX_TURN: f32 = 0.85;
/// How far a ring's centre may be from the one before it, as a fraction of `spacing`: a chain whose rings
/// jump about is not a road.
pub const CHAIN_GAP: (f32, f32) = (0.7, 1.3);

/// The chain's two numbers, over consecutive rings (ones whose ids follow on): the widest turn from one ring
/// to the next (rad), and the smallest and largest gap between their centres (uu). All zero for a chain of
/// fewer than two rings.
pub fn chain_metrics(rings: &[Ring]) -> (f32, f32, f32) {
    let mut turn = 0.0f32;
    let (mut lo, mut hi) = (f32::MAX, 0.0f32);
    let mut pairs = 0;
    for w in rings.windows(2) {
        let (a, b) = (w[0], w[1]);
        if b.id != a.id + 1 {
            continue;
        }
        pairs += 1;
        turn = turn.max(a.normal.normalize_or_zero().dot(b.normal.normalize_or_zero()).clamp(-1.0, 1.0).acos());
        let d = a.center.distance(b.center);
        lo = lo.min(d);
        hi = hi.max(d);
    }
    if pairs == 0 { (0.0, 0.0, 0.0) } else { (turn, lo, hi) }
}

/// Whether the chain is a smooth, readable road: every ring turned no more than `CHAIN_MAX_TURN` from the one
/// before it, and every gap within `CHAIN_GAP` of `spacing`. A chain of fewer than two rings is smooth.
pub fn chain_is_smooth(rings: &[Ring], spacing: f32) -> bool {
    let (turn, lo, hi) = chain_metrics(rings);
    if !(turn.is_finite() && lo.is_finite() && hi.is_finite()) {
        return false;
    }
    if lo == 0.0 && hi == 0.0 {
        return true;
    }
    turn <= CHAIN_MAX_TURN && lo >= CHAIN_GAP.0 * spacing && hi <= CHAIN_GAP.1 * spacing
}

/// What `e` adds to the Snitch's catch bar: a ring's own bonus, and the bigger one on every `streak_len` in a
/// row. A miss adds nothing (it has already cost the streak).
pub fn bonus_for(t: &TrailTuning, e: &TrailEvent) -> f32 {
    let TrailEvent::Through { streak, .. } = *e else { return 0.0 };
    let full = t.streak_len.max(1);
    let run = if streak % full == 0 { t.streak_bonus } else { 0.0 };
    let gain = t.ring_bonus + run;
    if gain.is_finite() { gain.max(0.0) } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::quidditch::arena::ArenaBounds;
    use crate::quidditch::flight::BroomState;
    use crate::quidditch::snitch::Snitch;

    fn trail() -> RingTrail {
        RingTrail::with_tuning(TrailTuning::default())
    }

    /// Flies the Snitch straight along +x, dropping rings, with Harry parked out of the way.
    fn fly(t: &mut RingTrail, from: f32, to: f32, step: f32) {
        let away = Vec3::new(0.0, 100_000.0, 0.0);
        let mut x = from;
        while x <= to {
            t.step(Vec3::new(x, 0.0, 0.0), Vec3::X, away, away, 1.0 / 60.0);
            x += step;
        }
    }

    #[test]
    fn a_ring_is_dropped_every_spacing_of_snitch_path() {
        let d = TrailTuning::default();
        let mut t = trail();
        // The first sighting drops one at once.
        t.step(Vec3::ZERO, Vec3::X, Vec3::splat(9e4), Vec3::splat(9e4), 0.0);
        assert_eq!(t.rings().len(), 1);
        assert_eq!(t.rings()[0].center, Vec3::ZERO);
        assert_eq!(t.rings()[0].normal, Vec3::X);
        assert_eq!(t.rings()[0].radius, d.radius);
        // Half a spacing is not enough; a full one is.
        fly(&mut t, 10.0, d.spacing - 10.0, 10.0);
        assert_eq!(t.rings().len(), 1, "{:?}", t.rings());
        fly(&mut t, d.spacing, d.spacing, 10.0);
        assert_eq!(t.rings().len(), 2);
        assert!((t.rings()[1].center.x - d.spacing).abs() < 1.0, "{:?}", t.rings()[1]);
    }

    #[test]
    fn the_trail_keeps_at_most_max_rings_and_the_newest_ones() {
        let d = TrailTuning::default();
        let mut t = trail();
        fly(&mut t, 0.0, d.spacing * (d.max_rings as f32 + 4.0), 50.0);
        assert_eq!(t.rings().len(), d.max_rings);
        // The oldest went: the ids left are the last `max_rings` dropped, in order.
        let ids: Vec<u64> = t.rings().iter().map(|r| r.id).collect();
        assert!(ids.windows(2).all(|w| w[1] == w[0] + 1), "{ids:?}");
        assert!(ids[0] >= 4, "the oldest rings were kept: {ids:?}");
    }

    #[test]
    fn rings_expire_after_life_seconds() {
        let mut t = trail();
        t.step(Vec3::ZERO, Vec3::X, Vec3::splat(9e4), Vec3::splat(9e4), 0.0);
        assert_eq!(t.rings().len(), 1);
        // A Snitch standing still drops nothing more, and the ring goes when its life is up.
        for _ in 0..(TrailTuning::default().life * 2.0) as u32 + 2 {
            t.step(Vec3::ZERO, Vec3::X, Vec3::splat(9e4), Vec3::splat(9e4), 0.5);
        }
        assert!(t.rings().is_empty(), "{:?}", t.rings());
    }

    #[test]
    fn flying_through_a_ring_counts_either_way_and_only_once() {
        let mut t = trail();
        t.step(Vec3::ZERO, Vec3::X, Vec3::splat(9e4), Vec3::splat(9e4), 0.0);
        // Straight through the middle, against the normal (the chase comes from behind).
        assert_eq!(t.next_ring().map(|r| r.id), Some(0));
        let ev = t.step(Vec3::ZERO, Vec3::X, Vec3::new(-100.0, 0.0, 0.0), Vec3::new(100.0, 0.0, 0.0), 1.0 / 60.0);
        assert_eq!(ev, vec![TrailEvent::Through { index: 0, streak: 1 }]);
        // The ring taken is behind him: there is no next one until the Snitch drops another.
        assert_eq!(t.next_ring(), None);
        // Back through the same ring: it is finished with, and the streak stays where it was.
        let ev = t.step(Vec3::ZERO, Vec3::X, Vec3::new(100.0, 0.0, 0.0), Vec3::new(-100.0, 0.0, 0.0), 1.0 / 60.0);
        assert_eq!(ev, vec![]);
        assert_eq!(t.streak(), 1);
        // And the other way round on a fresh ring.
        let mut t = trail();
        t.step(Vec3::ZERO, -Vec3::X, Vec3::splat(9e4), Vec3::splat(9e4), 0.0);
        let ev = t.step(Vec3::ZERO, -Vec3::X, Vec3::new(-100.0, 0.0, 0.0), Vec3::new(100.0, 0.0, 0.0), 1.0 / 60.0);
        assert_eq!(ev, vec![TrailEvent::Through { index: 0, streak: 1 }]);
    }

    #[test]
    fn missing_a_ring_resets_the_streak_and_costs_nothing_else() {
        let d = TrailTuning::default();
        let mut t = trail();
        let away = Vec3::new(0.0, 9e4, 0.0);
        // Three rings, well over a spacing apart along +x: one is dropped at each sighting.
        for i in 0..3 {
            t.step(Vec3::new(i as f32 * 2000.0, 0.0, 0.0), Vec3::X, away, away, 0.0);
        }
        let snitch = Vec3::new(4000.0, 0.0, 0.0);
        assert_eq!(t.rings().len(), 3);
        // The first two, flown through: a streak of two.
        for i in 0..2 {
            let at = Vec3::new(i as f32 * 2000.0, 0.0, 0.0);
            let ev = t.step(snitch, Vec3::X, at - Vec3::new(100.0, 0.0, 0.0), at + Vec3::new(100.0, 0.0, 0.0), 1.0 / 60.0);
            assert_eq!(ev, vec![TrailEvent::Through { index: i, streak: i as u32 + 1 }]);
        }
        assert_eq!(t.streak(), 2);
        // The third, gone past wide of the rim but close by: a miss, and the run is lost.
        let wide = snitch + Vec3::new(d.miss_behind + 50.0, d.radius + 100.0, 0.0);
        let ev = t.step(snitch, Vec3::X, away, wide, 1.0 / 60.0);
        assert_eq!(ev, vec![TrailEvent::Missed { streak_lost: 2 }]);
        assert_eq!(t.streak(), 0);
        // It fires once, and nothing else happens to the trail: the rings are still there to be flown.
        assert!(t.step(snitch, Vec3::X, wide, wide, 1.0 / 60.0).is_empty());
        assert!(!t.rings().is_empty());
    }

    /// A ring on the other side of the pitch is one Harry never went for: it expires without a miss.
    #[test]
    fn a_ring_far_away_is_never_counted_as_missed() {
        let d = TrailTuning::default();
        let mut t = trail();
        t.step(Vec3::ZERO, Vec3::X, Vec3::splat(9e4), Vec3::splat(9e4), 0.0);
        let far = Vec3::new(d.miss_near + 1000.0, 0.0, 0.0);
        assert!(t.step(Vec3::ZERO, Vec3::X, far, far, 1.0 / 60.0).is_empty());
        assert_eq!(t.streak(), 0);
    }

    /// The bonus: every ring is worth `ring_bonus`, and every `streak_len` in a row the bigger one too.
    #[test]
    fn the_streak_bonus_lands_every_streak_len_rings() {
        let d = TrailTuning::default();
        assert_eq!(bonus_for(&d, &TrailEvent::Missed { streak_lost: 3 }), 0.0);
        for streak in 1..=6u32 {
            let got = bonus_for(&d, &TrailEvent::Through { index: 0, streak });
            let want = if streak % d.streak_len == 0 { d.ring_bonus + d.streak_bonus } else { d.ring_bonus };
            assert!((got - want).abs() < 1e-6, "streak {streak}: {got} not {want}");
        }
    }

    /// The trail is the Snitch's, so it is bounded by the Snitch: every ring of a real chase is inside the
    /// arena, with room for its own radius around the centre only where the Snitch has it.
    #[test]
    fn every_ring_of_a_real_chase_is_inside_the_arena() {
        let a = ArenaBounds { center: Vec3::new(0.0, 0.0, 2000.0), half: Vec3::new(5000.0, 5000.0, 2000.0), ceiling: 3500.0 };
        let mut s = Snitch::new(9, &a);
        let mut t = trail();
        let zmax = a.ceiling.min(a.center.z + a.half.z);
        let mut dropped = 0;
        for i in 0..3000 {
            let th = i as f32 * 0.03;
            let harry = Vec3::new(th.cos() * 2000.0, th.sin() * 2000.0, 2000.0);
            s.step(&BroomState::new(harry, Vec3::ZERO, 0.0, 0.0), false, 1.0 / 60.0);
            let before = t.rings().last().map(|r| r.id);
            t.step(s.pos(), s.travel_dir(), harry, harry, 1.0 / 60.0);
            dropped += u32::from(t.rings().last().map(|r| r.id) != before);
            for r in t.rings() {
                assert!(r.center.is_finite() && r.normal.is_finite(), "{r:?}");
                assert!((r.normal.length() - 1.0).abs() < 1e-3, "{r:?}");
                assert!((r.center.x - a.center.x).abs() <= a.half.x, "{r:?}");
                assert!((r.center.y - a.center.y).abs() <= a.half.y, "{r:?}");
                assert!(r.center.z >= a.center.z - a.half.z && r.center.z <= zmax, "{r:?}");
            }
            assert!(t.rings().len() <= TrailTuning::default().max_rings);
        }
        assert!(dropped > 10, "a Snitch flying for 50 s dropped only {dropped} rings");
    }

    /// The owner's complaint: the chain of rings was not obvious and could be jerky, because the Snitch's path
    /// turned on a sixpence at every waypoint. Over fifty seeds and 20 000 steps each, every ring of a real
    /// chase must now be turned no more than `CHAIN_MAX_TURN` from the one before it and sit within
    /// `CHAIN_GAP` of a `spacing` from it. The worst turn seen is printed, so the number can be read off a
    /// `--nocapture` run and compared with the old path's.
    #[test]
    fn the_ring_chain_of_a_real_chase_is_smooth_for_every_seed() {
        let a = ArenaBounds { center: Vec3::new(0.0, 0.0, 2000.0), half: Vec3::new(5000.0, 5000.0, 2000.0), ceiling: 3500.0 };
        let d = TrailTuning::default();
        let mut worst = 0.0f32;
        let (mut worst_lo, mut worst_hi) = (f32::MAX, 0.0f32);
        for seed in 1..=50u64 {
            let mut s = Snitch::new(seed, &a);
            let mut t = trail();
            for i in 0..20_000 {
                // Harry sweeps the pitch, so the Snitch sees both its speeds and both its moods.
                let th = i as f32 * 0.013;
                let harry = Vec3::new(th.cos() * 3000.0, th.sin() * 3500.0, 1500.0 + (th * 0.7).sin() * 1200.0);
                s.step(&BroomState::new(harry, Vec3::ZERO, 0.0, 0.0), false, 1.0 / 60.0);
                t.step(s.path_pos(), s.travel_dir(), harry, harry, 1.0 / 60.0);
                let (turn, lo, hi) = chain_metrics(t.rings());
                if hi > 0.0 {
                    worst = worst.max(turn);
                    worst_lo = worst_lo.min(lo);
                    worst_hi = worst_hi.max(hi);
                }
                assert!(
                    t.chain_is_smooth(),
                    "seed {seed} step {i}: turn {:.1} deg, gaps {lo:.0}..{hi:.0} of {spacing} -- {:?}",
                    turn.to_degrees(),
                    t.rings().iter().map(|r| (r.id, r.center)).collect::<Vec<_>>(),
                    spacing = d.spacing,
                );
            }
        }
        println!("ring chain over 50 seeds: worst turn {:.1} deg, gaps {worst_lo:.0}..{worst_hi:.0} uu of {:.0}", worst.to_degrees(), d.spacing);
        assert!(worst > 0.0, "the chase dropped no chain at all");
    }

    #[test]
    fn odd_input_never_panics_or_nans() {
        let mut t = trail();
        for dt in [0.0, 1e-9, 10.0, f32::NAN, f32::INFINITY, -1.0] {
            t.step(Vec3::splat(f32::NAN), Vec3::ZERO, Vec3::ZERO, Vec3::splat(f32::INFINITY), dt);
            t.step(Vec3::ZERO, Vec3::splat(f32::NAN), Vec3::splat(f32::NAN), Vec3::ZERO, dt);
            for r in t.rings() {
                assert!(r.center.is_finite() && r.normal.is_finite() && r.age.is_finite(), "{r:?}");
            }
        }
        // A trail that may hold no rings at all holds none, and never divides by its spacing.
        let mut none = RingTrail::new(0.0, 0, 0.0, 0.0);
        assert!(none.step(Vec3::ZERO, Vec3::X, Vec3::ZERO, Vec3::X, 0.016).is_empty());
        assert!(none.rings().is_empty());
    }
}
