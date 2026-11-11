//! Lee Jordan-style commentary: one line picked for each thing that happens in the match, never the same line
//! twice inside `MIN_GAP` seconds.
//!
//! HP3's own Quidditch commentary audio is unconfirmed (the design doc leaves the choice to the builder), so a
//! `CommentId` is a line of text the presentation layer puts on screen as a subtitle; a sound is played only
//! where a suitable one exists in `soundeffects.Quidditch` (`sound_for`).

use super::bludger::BludgerEvent;
use super::rules::{MatchEvent, Outcome};
use super::snitch::SnitchEvent;

/// One commentary line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentId {
    Kickoff,
    SnitchSeen,
    SnitchClosing,
    SnitchGone,
    SnitchLost,
    CatchWindow,
    CatchNow,
    Caught,
    Missed,
    MissedAgain,
    OutOfTries,
    BludgerOut,
    BludgerIncoming,
    BludgerHit,
    BludgerStung,
    BludgerWide,
    ThroughRings,
    RingRun,
    RingsReady,
    GoalHome,
    GoalHomeAgain,
    GoalAway,
    GoalAwayAgain,
    Won,
    Lost,
}

/// Seconds that must pass before the same line may be said again.
pub const MIN_GAP: f32 = 8.0;

/// The words of a line, for the on-screen subtitle.
pub fn line(id: CommentId) -> &'static str {
    match id {
        CommentId::Kickoff => "And they're off! Gryffindor take to the air!",
        CommentId::SnitchSeen => "Potter's seen something -- could that be the Snitch?",
        CommentId::SnitchClosing => "He's closing in, look at him go!",
        CommentId::SnitchGone => "And the Snitch is away again. Hard luck, Potter.",
        CommentId::SnitchLost => "He's lost it. Back to square one for Gryffindor.",
        CommentId::CatchWindow => "He's right on it -- reach out, Harry!",
        CommentId::CatchNow => "This is it! Grab it!",
        CommentId::Caught => "HE'S GOT THE SNITCH! Potter has caught the Snitch!",
        CommentId::Missed => "Ooh, so close. Through his fingers!",
        CommentId::MissedAgain => "Missed it again! The crowd groans.",
        CommentId::OutOfTries => "That's it -- their Seeker has the Snitch.",
        CommentId::BludgerOut => "A Bludger is loose and it's looking for Potter.",
        CommentId::BludgerIncoming => "Watch out, Harry -- Bludger on your tail!",
        CommentId::BludgerHit => "OOF! That Bludger caught him square.",
        CommentId::BludgerStung => "He's taken another one. He's hanging on by his knees!",
        CommentId::BludgerWide => "And the Bludger sails harmlessly by.",
        CommentId::ThroughRings => "Through the rings! He's on the Snitch's tail!",
        CommentId::RingRun => "Another one! Potter is flying the Snitch's own trail!",
        CommentId::RingsReady => "That's his run of rings -- the Snitch is his to take!",
        CommentId::GoalHome => "Gryffindor score! Ten points!",
        CommentId::GoalHomeAgain => "Another one through the hoop for Gryffindor!",
        CommentId::GoalAway => "They've scored. Ten points to the other lot.",
        CommentId::GoalAwayAgain => "And again! The Keeper never saw it.",
        CommentId::Won => "Gryffindor win the match!",
        CommentId::Lost => "And that's the match. Not Gryffindor's day.",
    }
}

/// The `soundeffects.Quidditch` sound to play under a line, where one fits. HP3 ships no spoken commentary we
/// could confirm, so most lines are text only; these are the crowd reactions that do exist.
pub fn sound_for(id: CommentId) -> Option<&'static str> {
    Some(match id {
        CommentId::Kickoff => "SS_QUID_Bigcheers_0001",
        CommentId::Caught | CommentId::Won => "SS_QUID_Crowd_roar_01",
        CommentId::GoalHome | CommentId::GoalHomeAgain | CommentId::RingsReady => "SS_QUID_Big_cheer_01",
        CommentId::GoalAway | CommentId::GoalAwayAgain | CommentId::Lost => "SS_QUID_boo_long_01",
        CommentId::OutOfTries => "SS_QUID_boo_long_01",
        _ => return None,
    })
}

/// The lines that fit an event, best first. Several of them is what lets a repeated event sound different.
fn choices(e: &MatchEvent) -> &'static [CommentId] {
    match e {
        MatchEvent::Started => &[CommentId::Kickoff],
        MatchEvent::Snitch(SnitchEvent::ClosingIn) => &[CommentId::SnitchSeen, CommentId::SnitchClosing],
        MatchEvent::Snitch(SnitchEvent::Lost) => &[CommentId::SnitchGone, CommentId::SnitchLost],
        MatchEvent::Snitch(SnitchEvent::CatchStart) => &[CommentId::CatchWindow, CommentId::CatchNow],
        MatchEvent::Snitch(SnitchEvent::Caught) => &[CommentId::Caught],
        MatchEvent::Snitch(SnitchEvent::Missed) => &[CommentId::Missed, CommentId::MissedAgain],
        MatchEvent::Snitch(SnitchEvent::OutOfTries) => &[CommentId::OutOfTries],
        MatchEvent::Bludger(BludgerEvent::Launched) => &[CommentId::BludgerOut, CommentId::BludgerIncoming],
        MatchEvent::Bludger(BludgerEvent::Hit) => &[CommentId::BludgerHit, CommentId::BludgerStung],
        MatchEvent::Bludger(BludgerEvent::Missed) => &[CommentId::BludgerWide],
        // The ring that completes the run the catch needs has a line of its own, so the player is told the
        // Snitch is now his to take; every other ring shares two.
        MatchEvent::Rings { ready: true, .. } => &[CommentId::RingsReady],
        MatchEvent::Rings { .. } => &[CommentId::ThroughRings, CommentId::RingRun],
        MatchEvent::Goal { home: true } => &[CommentId::GoalHome, CommentId::GoalHomeAgain],
        MatchEvent::Goal { home: false } => &[CommentId::GoalAway, CommentId::GoalAwayAgain],
        MatchEvent::Ended(Outcome::Won) => &[CommentId::Won],
        MatchEvent::Ended(Outcome::Lost) => &[CommentId::Lost],
    }
}

/// The line to say for `e`, or `None` when everything that fits was just said.
///
/// `rng` is any source of random bits (the match's own stream), and `last` is the line said last with how many
/// seconds ago -- the caller ages it with `age` every frame. A line that was said less than `MIN_GAP` seconds
/// ago is skipped for the next one that fits; with nothing left, nothing is said.
pub fn comment_for(e: &MatchEvent, rng: &mut impl FnMut() -> u32, last: &mut Option<(CommentId, f32)>) -> Option<CommentId> {
    let fits = choices(e);
    if fits.is_empty() {
        return None;
    }
    let start = rng() as usize % fits.len();
    // Said less than `MIN_GAP` ago: that exact line is skipped, the others are fair game.
    let blocked = |id: CommentId| matches!(*last, Some((l, t)) if l == id && t < MIN_GAP);
    for k in 0..fits.len() {
        let id = fits[(start + k) % fits.len()];
        if !blocked(id) {
            *last = Some((id, 0.0));
            return Some(id);
        }
    }
    None
}

/// Ages the last line by `dt` seconds. Called once a frame, whether or not anything happened.
pub fn age(last: &mut Option<(CommentId, f32)>, dt: f32) {
    if let Some((_, t)) = last.as_mut()
        && dt.is_finite()
        && dt > 0.0
    {
        *t = (*t + dt).min(MIN_GAP * 4.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `MatchEvent` there is, one of each kind.
    fn every_event() -> Vec<MatchEvent> {
        let mut v = vec![MatchEvent::Started, MatchEvent::Goal { home: true }, MatchEvent::Goal { home: false }];
        for e in [
            SnitchEvent::ClosingIn,
            SnitchEvent::Lost,
            SnitchEvent::CatchStart,
            SnitchEvent::Caught,
            SnitchEvent::Missed,
            SnitchEvent::OutOfTries,
        ] {
            v.push(MatchEvent::Snitch(e));
        }
        for e in [BludgerEvent::Launched, BludgerEvent::Hit, BludgerEvent::Missed] {
            v.push(MatchEvent::Bludger(e));
        }
        v.push(MatchEvent::Rings { streak: 1, ready: false });
        v.push(MatchEvent::Rings { streak: 3, ready: true });
        v.push(MatchEvent::Ended(Outcome::Won));
        v.push(MatchEvent::Ended(Outcome::Lost));
        v
    }

    #[test]
    fn comment_chosen_for_each_event_kind() {
        for e in every_event() {
            let mut last = None;
            let mut rng = || 0u32;
            let id = comment_for(&e, &mut rng, &mut last).unwrap_or_else(|| panic!("no line for {e:?}"));
            assert_eq!(last.map(|l| l.0), Some(id), "{e:?} must be remembered as the last line");
            assert!(!line(id).is_empty(), "{id:?} has no words");
        }
    }

    #[test]
    fn comment_not_repeated_within_gap() {
        // An event with two lines: the second time it happens straight away, the other line is used.
        let e = MatchEvent::Goal { home: true };
        let mut rng = || 0u32;
        let mut last = None;
        let first = comment_for(&e, &mut rng, &mut last).unwrap();
        let second = comment_for(&e, &mut rng, &mut last).unwrap();
        assert_ne!(first, second, "the same line must not be said twice inside the gap");

        // An event with one line: nothing is said again until the gap has passed.
        let e = MatchEvent::Snitch(SnitchEvent::Caught);
        let mut last = None;
        let first = comment_for(&e, &mut rng, &mut last).unwrap();
        assert_eq!(comment_for(&e, &mut rng, &mut last), None);
        age(&mut last, MIN_GAP * 0.5);
        assert_eq!(comment_for(&e, &mut rng, &mut last), None, "half the gap is not enough");
        age(&mut last, MIN_GAP);
        assert_eq!(comment_for(&e, &mut rng, &mut last), Some(first), "after the gap the line may be said again");
    }

    #[test]
    fn the_rng_picks_between_the_lines_that_fit() {
        let e = MatchEvent::Bludger(BludgerEvent::Hit);
        let mut seen = std::collections::BTreeSet::new();
        for seed in 0..8u32 {
            let mut last = None;
            let mut rng = || seed;
            if let Some(id) = comment_for(&e, &mut rng, &mut last) {
                seen.insert(format!("{id:?}"));
            }
        }
        assert!(seen.len() >= 2, "both Bludger-hit lines should come up: {seen:?}");
    }
}
