//! What a match frame may spend, and who is cheap to show.
//!
//! The phone has 16.6 ms a frame and no more. When frames go over, the match gives up work rather than the
//! frame rate: fewer team players are steered at once (`max_active_ai`, Task 13's budget) and the proxy that
//! stands in for a distant player takes over nearer the camera (`proxy_distance`). What was given up comes
//! back slowly: only after 120 frames inside the budget, one player at a time, so a single slow frame does
//! not make the picture pump in and out.
//!
//! Lowering is one frame's decision and is therefore a free function (`choose_budget`); giving back needs the
//! count of quiet frames, which a free function cannot hold, so `BudgetGovernor` owns it.

/// The most team players a match ever steers itself. Nothing steers any yet (Task 13), so this is the cap the
/// budget is measured against and the number it starts at.
pub const AI_CAP: usize = 14;
/// The fewest players slow frames can leave steered: the six Chasers and the two Keepers, who are the ones
/// `ai::active_agents` keeps first. Below it nobody plays the Quaffle and the match stands still -- and the
/// team AI costs microseconds a frame, so there is nothing to win down there. A device that never reaches
/// 60 fps would otherwise sit at a budget of none for the whole match.
pub const MIN_ACTIVE_AI: usize = 8;
/// A frame of a 60 fps game, in milliseconds. Over this the budget drops.
pub const FRAME_MS: f32 = 16.6;
/// Frames inside the budget before the governor gives one player back.
pub const STABLE_FRAMES: u32 = 120;
/// How many players a frame over the budget costs.
const DROP: usize = 2;
/// Where a proxy takes over when the budget is at its smallest and when it is whole (world units).
const PROXY_NEAR: f32 = 2500.0;
const PROXY_FAR: f32 = 9000.0;

/// What the match may spend on a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Budget {
    /// Team players steered at once; the rest hold their course or are shown by a proxy.
    pub max_active_ai: usize,
    /// Beyond this (world units from the camera) a player is shown by a proxy rather than in full.
    pub proxy_distance: f32,
}

impl Default for Budget {
    fn default() -> Self {
        Budget { max_active_ai: AI_CAP, proxy_distance: proxy_distance(AI_CAP) }
    }
}

/// The distance a proxy takes over at for a given number of steered players: the tighter the budget, the
/// nearer the camera a proxy is good enough.
pub fn proxy_distance(max_active_ai: usize) -> f32 {
    let share = max_active_ai.min(AI_CAP) as f32 / AI_CAP as f32;
    PROXY_NEAR + (PROXY_FAR - PROXY_NEAR) * share
}

/// One frame's verdict: frames over 16.6 ms cost the budget `DROP` players (never fewer than none), and a
/// frame inside it leaves the budget alone. Giving back is `BudgetGovernor`'s, which counts the quiet frames.
/// An unmeasured frame (NaN) changes nothing.
pub fn choose_budget(avg_frame_ms: f32, current: Budget) -> Budget {
    if !(avg_frame_ms > FRAME_MS) {
        return current;
    }
    // Never below the players the Quaffle is played by, and never a rise: a budget already under that floor
    // (pinned by hand) stays where it is.
    let floor = MIN_ACTIVE_AI.min(current.max_active_ai);
    budget_for(current.max_active_ai.saturating_sub(DROP).max(floor))
}

/// The budget that steers `max_active_ai` players, with the proxy distance that goes with it (never over the
/// cap).
pub fn budget_for(max_active_ai: usize) -> Budget {
    let max_active_ai = max_active_ai.min(AI_CAP);
    Budget { max_active_ai, proxy_distance: proxy_distance(max_active_ai) }
}

/// Whether a player that far from the camera is shown by its proxy.
pub fn use_proxy(distance: f32, b: &Budget) -> bool {
    // An unmeasured distance takes the cheap answer.
    !(distance <= b.proxy_distance)
}

/// Holds the budget between frames: `choose_budget`'s drop, and the 120 quiet frames that give one player
/// back.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BudgetGovernor {
    budget: Budget,
    stable: u32,
}

impl BudgetGovernor {
    pub fn budget(&self) -> Budget {
        self.budget
    }

    /// One frame, `avg_frame_ms` long on average: the new budget.
    pub fn frame(&mut self, avg_frame_ms: f32) -> Budget {
        if avg_frame_ms > FRAME_MS {
            self.budget = choose_budget(avg_frame_ms, self.budget);
            self.stable = 0;
            return self.budget;
        }
        if !avg_frame_ms.is_finite() {
            return self.budget;
        }
        self.stable += 1;
        if self.stable >= STABLE_FRAMES {
            self.stable = 0;
            if self.budget.max_active_ai < AI_CAP {
                self.budget = budget_for(self.budget.max_active_ai + 1);
            }
        }
        self.budget
    }

    /// Pins the budget (`HP3_QUID_BUDGET=<players>`, for testing a tight budget on a fast machine).
    pub fn force(&mut self, max_active_ai: usize) {
        self.budget = budget_for(max_active_ai);
        self.stable = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_drops_ai_when_over_16_6ms() {
        let whole = Budget::default();
        assert_eq!(whole.max_active_ai, AI_CAP);
        // A frame inside the budget leaves it alone, one over it costs players and pulls the proxies in.
        assert_eq!(choose_budget(16.0, whole), whole);
        assert_eq!(choose_budget(f32::NAN, whole), whole);
        let tight = choose_budget(20.0, whole);
        assert!(tight.max_active_ai < whole.max_active_ai, "{tight:?}");
        assert!(tight.proxy_distance < whole.proxy_distance, "{tight:?}");
        // However bad it gets the budget never goes below the players the Quaffle is played by (a phone that
        // never reaches 60 fps would otherwise have no match at all), and the proxy distance stays positive.
        let mut b = whole;
        for _ in 0..100 {
            b = choose_budget(100.0, b);
        }
        assert_eq!(b.max_active_ai, MIN_ACTIVE_AI);
        // A budget pinned lower by hand is not raised by a slow frame.
        assert_eq!(choose_budget(100.0, budget_for(2)).max_active_ai, 2);
        assert!(b.proxy_distance > 0.0, "{b:?}");
        // And it never climbs over the cap, whatever it is handed.
        assert_eq!(budget_for(1000).max_active_ai, AI_CAP);
    }

    #[test]
    fn budget_recovers_only_after_120_stable_frames() {
        let mut g = BudgetGovernor::default();
        let tight = g.frame(20.0);
        assert!(tight.max_active_ai < AI_CAP, "one slow frame drops the budget: {tight:?}");
        // 119 good frames change nothing.
        for _ in 0..STABLE_FRAMES - 1 {
            assert_eq!(g.frame(10.0), tight);
        }
        // The 120th gives one player back.
        let eased = g.frame(10.0);
        assert_eq!(eased.max_active_ai, tight.max_active_ai + 1, "{eased:?}");
        // A slow frame in between starts the count again: 119 good frames after it still change nothing.
        let dropped = g.frame(30.0);
        for _ in 0..STABLE_FRAMES - 1 {
            assert_eq!(g.frame(10.0), dropped);
        }
        assert_eq!(g.frame(10.0).max_active_ai, dropped.max_active_ai + 1);
        // A whole budget is not raised over the cap, however long the frames stay quiet.
        let mut g = BudgetGovernor::default();
        for _ in 0..STABLE_FRAMES * 3 {
            assert_eq!(g.frame(8.0), Budget::default());
        }
    }

    #[test]
    fn far_players_use_proxy() {
        let b = Budget::default();
        assert!(!use_proxy(0.0, &b));
        assert!(!use_proxy(b.proxy_distance - 1.0, &b));
        assert!(use_proxy(b.proxy_distance + 1.0, &b));
        assert!(use_proxy(f32::NAN, &b), "an unmeasured distance takes the cheap answer");
        // A tighter budget shows the proxy from nearer: what was full at this distance no longer is.
        let tight = choose_budget(20.0, b);
        let between = (tight.proxy_distance + b.proxy_distance) * 0.5;
        assert!(!use_proxy(between, &b));
        assert!(use_proxy(between, &tight));
    }
}
