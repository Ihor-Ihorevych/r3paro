//! Match rules: score and phase state machine, fed by snitch/bludger events.

use super::bludger::BludgerEvent;
use super::snitch::SnitchEvent;

/// Final result for the home side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Won,
    Lost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchPhase {
    Intro,
    Seeking,
    Catching,
    Ended(Outcome),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Score {
    pub home: u32,
    pub away: u32,
}

impl Score {
    /// Points for one goal.
    pub const GOAL: u32 = 10;
    /// Points for catching the snitch.
    pub const SNITCH: u32 = 150;
}

/// Everything a match can report to the layer above.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MatchEvent {
    Snitch(SnitchEvent),
    Bludger(BludgerEvent),
    Goal { home: bool },
    /// Harry flew through one of the Snitch's flight rings (`quidditch::trail`): the run he is on, and whether
    /// that run is now long enough for the catch (`TrailTuning::catch_rings`). Nothing in the rules changes --
    /// the rings add to the catch bar and gate the catch window, both inside `Snitch`.
    Rings { streak: u32, ready: bool },
    Started,
    Ended(Outcome),
}

#[derive(Debug, Clone)]
pub struct Rules {
    phase: MatchPhase,
    score: Score,
}

impl Rules {
    pub fn new() -> Rules {
        Rules { phase: MatchPhase::Intro, score: Score { home: 0, away: 0 } }
    }

    pub fn phase(&self) -> MatchPhase {
        self.phase
    }

    pub fn score(&self) -> Score {
        self.score
    }

    fn ended(&self) -> bool {
        matches!(self.phase, MatchPhase::Ended(_))
    }

    /// Whether events count now: only between the kickoff (`start`) and the end. A match still in its intro
    /// has no score and cannot be won or lost.
    fn live(&self) -> bool {
        !self.ended() && self.phase != MatchPhase::Intro
    }

    /// Intro -> Seeking. No effect in any other phase.
    pub fn start(&mut self) {
        if self.phase == MatchPhase::Intro {
            self.phase = MatchPhase::Seeking;
        }
    }

    /// Applies one snitch event; ignored once the match has ended.
    /// - `CatchStart`: Catching.
    /// - `Missed`: back to Seeking (the match is not over by itself; if tries are exhausted
    ///   the snitch emits `OutOfTries` in the same step, which ends it).
    /// - `Caught`: +150 to home, then the match ends. The outcome is decided by the TOTAL
    ///   score after the catch: home > away is Won; a tie or less is Lost.
    /// - `OutOfTries`: the opposing Seeker has the snitch: +150 to away, then the match ends, decided by
    ///   the total score in the same way.
    /// - `ClosingIn` / `Lost`: no rules effect.
    pub fn on_snitch(&mut self, e: SnitchEvent) {
        if !self.live() {
            return;
        }
        match e {
            SnitchEvent::CatchStart => self.phase = MatchPhase::Catching,
            SnitchEvent::Missed => self.phase = MatchPhase::Seeking,
            SnitchEvent::Caught => {
                self.score.home += Score::SNITCH;
                let outcome =
                    if self.score.home > self.score.away { Outcome::Won } else { Outcome::Lost };
                self.phase = MatchPhase::Ended(outcome);
            }
            SnitchEvent::OutOfTries => {
                // The Snitch Harry cannot catch any more goes to the other Seeker, and so do its points: the
                // result is then the score's, like any other (`Lost 30 - 20` would be no result at all).
                self.score.away += Score::SNITCH;
                let outcome =
                    if self.score.home > self.score.away { Outcome::Won } else { Outcome::Lost };
                self.phase = MatchPhase::Ended(outcome);
            }
            SnitchEvent::ClosingIn | SnitchEvent::Lost => {}
        }
    }

    /// Bludger events never change score or phase here; `control_lost` (Harry knocked off
    /// control by a Hit) is handled by the caller. Kept for a uniform event interface.
    pub fn on_bludger(&mut self, _e: BludgerEvent, _control_lost: bool) {}

    /// +10 to the scoring side; ignored before the kickoff and once the match has ended.
    pub fn add_goal(&mut self, home: bool) {
        if !self.live() {
            return;
        }
        if home {
            self.score.home += Score::GOAL;
        } else {
            self.score.away += Score::GOAL;
        }
    }
}

impl Default for Rules {
    fn default() -> Rules {
        Rules::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeking() -> Rules {
        let mut r = Rules::new();
        r.start();
        r
    }

    #[test]
    fn snitch_catch_adds_150_and_ends_won() {
        let mut r = seeking();
        r.on_snitch(SnitchEvent::CatchStart);
        r.on_snitch(SnitchEvent::Caught);
        assert_eq!(r.score(), Score { home: 150, away: 0 });
        assert_eq!(r.phase(), MatchPhase::Ended(Outcome::Won));
    }

    #[test]
    fn out_of_tries_ends_lost() {
        let mut r = seeking();
        r.on_snitch(SnitchEvent::CatchStart);
        r.on_snitch(SnitchEvent::Missed);
        r.on_snitch(SnitchEvent::OutOfTries);
        assert_eq!(r.phase(), MatchPhase::Ended(Outcome::Lost));
        // The Snitch Harry could not catch is the other Seeker's: their 150.
        assert_eq!(r.score(), Score { home: 0, away: 150 });
    }

    /// The result always agrees with the score: a side that is ahead on goals when the Snitch gets away is
    /// not told it lost 30 - 20, it lost by the Snitch -- and if it was far enough ahead, it won.
    #[test]
    fn out_of_tries_is_decided_by_the_total_score_too() {
        let mut r = seeking();
        for _ in 0..3 {
            r.add_goal(true);
        }
        r.add_goal(false);
        r.on_snitch(SnitchEvent::OutOfTries);
        assert_eq!(r.score(), Score { home: 30, away: 160 });
        assert_eq!(r.phase(), MatchPhase::Ended(Outcome::Lost));
        let mut r = seeking();
        for _ in 0..16 {
            r.add_goal(true);
        }
        r.on_snitch(SnitchEvent::OutOfTries);
        assert_eq!(r.score(), Score { home: 160, away: 150 });
        assert_eq!(r.phase(), MatchPhase::Ended(Outcome::Won));
    }

    #[test]
    fn missed_with_tries_left_returns_to_seeking_and_same_step_out_of_tries_ends_once() {
        let mut r = seeking();
        r.on_snitch(SnitchEvent::CatchStart);
        r.on_snitch(SnitchEvent::Missed);
        assert_eq!(r.phase(), MatchPhase::Seeking);
        r.on_snitch(SnitchEvent::CatchStart);
        r.on_snitch(SnitchEvent::Missed);
        r.on_snitch(SnitchEvent::OutOfTries);
        assert_eq!(r.phase(), MatchPhase::Ended(Outcome::Lost));
    }

    #[test]
    fn ended_match_ignores_later_events() {
        let mut r = seeking();
        r.on_snitch(SnitchEvent::Caught);
        let (phase, score) = (r.phase(), r.score());
        r.on_snitch(SnitchEvent::Caught);
        r.on_snitch(SnitchEvent::OutOfTries);
        r.on_snitch(SnitchEvent::Missed);
        r.on_bludger(BludgerEvent::Hit, true);
        r.add_goal(true);
        r.add_goal(false);
        assert_eq!(r.phase(), phase);
        assert_eq!(r.score(), score);
    }

    #[test]
    fn goal_adds_10_to_the_scoring_side() {
        let mut r = seeking();
        r.add_goal(true);
        r.add_goal(false);
        r.add_goal(false);
        assert_eq!(r.score(), Score { home: 10, away: 20 });
        assert_eq!(r.phase(), MatchPhase::Seeking);
    }

    #[test]
    fn winner_is_decided_by_total_score_after_catch() {
        // Behind by exactly 150: the catch ties, a tie is Lost.
        let mut r = seeking();
        for _ in 0..15 {
            r.add_goal(false);
        }
        r.on_snitch(SnitchEvent::Caught);
        assert_eq!(r.score(), Score { home: 150, away: 150 });
        assert_eq!(r.phase(), MatchPhase::Ended(Outcome::Lost));

        // Behind by 140: the catch wins.
        let mut r = seeking();
        for _ in 0..14 {
            r.add_goal(false);
        }
        r.on_snitch(SnitchEvent::Caught);
        assert_eq!(r.phase(), MatchPhase::Ended(Outcome::Won));

        // Behind by 160: Lost.
        let mut r = seeking();
        for _ in 0..16 {
            r.add_goal(false);
        }
        r.on_snitch(SnitchEvent::Caught);
        assert_eq!(r.phase(), MatchPhase::Ended(Outcome::Lost));
    }

    #[test]
    fn phase_order_intro_seeking_catching() {
        let mut r = Rules::new();
        assert_eq!(r.phase(), MatchPhase::Intro);
        r.start();
        assert_eq!(r.phase(), MatchPhase::Seeking);
        r.on_snitch(SnitchEvent::CatchStart);
        assert_eq!(r.phase(), MatchPhase::Catching);
    }

    #[test]
    fn bludger_hit_changes_neither_score_nor_phase() {
        let mut r = seeking();
        r.on_bludger(BludgerEvent::Hit, true);
        r.on_bludger(BludgerEvent::Hit, false);
        assert_eq!(r.score(), Score { home: 0, away: 0 });
        assert_eq!(r.phase(), MatchPhase::Seeking);
    }

    /// Nothing counts before the kickoff: a match still in its intro has no score and cannot end.
    #[test]
    fn events_before_the_start_are_ignored() {
        let mut r = Rules::new();
        r.add_goal(true);
        r.add_goal(false);
        r.on_snitch(SnitchEvent::CatchStart);
        r.on_snitch(SnitchEvent::Caught);
        r.on_snitch(SnitchEvent::OutOfTries);
        assert_eq!(r.phase(), MatchPhase::Intro);
        assert_eq!(r.score(), Score { home: 0, away: 0 });
        r.start();
        assert_eq!(r.phase(), MatchPhase::Seeking);
    }
}
