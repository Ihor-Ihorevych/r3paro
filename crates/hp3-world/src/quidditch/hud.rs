//! What the match wants on screen, as pure data: a list of `HudItem`s the presentation layer draws through
//! the `Canvas`. Nothing here knows about the VM, fonts or pixels -- only what the numbers are.

use super::rules::{MatchPhase, Outcome};

/// One thing the HUD shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HudKind {
    /// The snitch catch bar, 0..1.
    SnitchProgress,
    /// Gryffindor's score, in points.
    ScoreHome,
    /// The opponent's score, in points.
    ScoreAway,
    /// How many catch attempts are left (one pip each).
    CatchPips,
    /// Harry's speed as a fraction of the fastest his broom goes, 0..1.
    Speed,
    /// How much control a Bludger has left him: 1 untouched, 0 knocked off.
    Control,
    /// How many of the Snitch's flight rings Harry has taken in a row (`quidditch::trail`). Shown next to the
    /// catch bar as `RINGS n/N`, where N is what the catch needs (`RingsNeeded`).
    RingStreak,
    /// Rings in a row the catch needs (`TrailTuning::catch_rings`); 0 means they are not required.
    RingsNeeded,
    /// The broom's charge from flight rings, 0..1 (what the speed wings show).
    Charge,
    /// A ring was just taken: 1 fading to 0 (the wings' pulse).
    ChargeGlow,
    /// The boost: 2 while it runs, else how ready the next one is (0..1, 1 = ready).
    Boost,
    /// Seconds left of the catch window; present only while it is open.
    CatchWindow,
    /// The control hints; the value is how many lines there are (`HINTS`).
    Hints,
    /// Shown once the match is over: 1 won, 0 lost.
    Result,
}

/// One HUD element: what it is and the number behind it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HudItem {
    pub kind: HudKind,
    pub value: f32,
}

/// Everything the HUD needs out of a running match. `QuidditchMatch` lives in `glue.rs` (the VM side), so the
/// glue fills one of these each frame and the HUD stays pure.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HudInput {
    pub phase: MatchPhase,
    /// The snitch catch bar, 0..1.
    pub progress: f32,
    /// Catch attempts left.
    pub tries_left: u32,
    pub score_home: u32,
    pub score_away: u32,
    /// Harry's speed and the fastest his broom goes, both in uu/s.
    pub speed: f32,
    pub top_speed: f32,
    /// Bludger hits taken, and how many cost him the broom.
    pub bumps: u32,
    pub bumps_to_lose: u32,
    /// Flight rings of the Snitch's trail taken in a row, and how many in a row the catch needs.
    pub ring_streak: u32,
    pub rings_needed: u32,
    /// The broom's charge (0..1), the pulse of the last ring taken, and the seconds the catch window has left.
    pub charge: f32,
    pub charge_glow: f32,
    pub window_left: f32,
    /// `Momentum::boost_state`.
    pub boost: f32,
}

/// The control hints, in the order they are shown.
pub const HINTS: [&str; 3] = ["Arrows/Mouse: fly", "Fly through rings to charge the broom", "Right click / A / JUMP: boost and catch"];

/// The ring counter's words, or `None` when there is nothing to say (no rings are wanted and none have been
/// taken). `n` is the run Harry is on, `needed` how many in a row the catch wants.
pub fn ring_line(n: u32, needed: u32) -> Option<String> {
    match (n, needed) {
        (0, 0) => None,
        (_, 0) => Some(format!("RINGS x{n}")),
        _ => Some(format!("RINGS {}/{needed}", n.min(needed))),
    }
}

impl Default for HudInput {
    fn default() -> HudInput {
        HudInput {
            phase: MatchPhase::Intro,
            progress: 0.0,
            tries_left: 0,
            score_home: 0,
            score_away: 0,
            speed: 0.0,
            top_speed: 1.0,
            bumps: 0,
            bumps_to_lose: 1,
            ring_streak: 0,
            rings_needed: 0,
            charge: 0.0,
            charge_glow: 0.0,
            window_left: 0.0,
            boost: 1.0,
        }
    }
}

/// A finite `v` clamped to 0..1 (rubbish in gives 0, never a NaN on screen).
fn unit(v: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

/// Everything the HUD shows this frame, in drawing order. The catch bar, the score and the pips are always
/// there; the hints only while the match is being played, the result line only once it is over.
pub fn hud_items(m: &HudInput) -> Vec<HudItem> {
    let ended = matches!(m.phase, MatchPhase::Ended(_));
    let item = |kind, value| HudItem { kind, value };
    let num = |v: f32| if v.is_finite() { v } else { 0.0 };
    let mut out = vec![
        item(HudKind::SnitchProgress, unit(m.progress)),
        item(HudKind::ScoreHome, m.score_home as f32),
        item(HudKind::ScoreAway, m.score_away as f32),
        // No attempts are left to show once the match is over, whatever was unused when it ended.
        item(HudKind::CatchPips, if ended { 0.0 } else { m.tries_left as f32 }),
        item(HudKind::Speed, unit(num(m.speed) / num(m.top_speed).max(1.0))),
        // Hits left before the Bludgers take the broom off him, as a fraction.
        item(HudKind::Control, unit(1.0 - m.bumps as f32 / (m.bumps_to_lose.max(1)) as f32)),
    ];
    // The ring run, while the match is on: a finished match has no rings to fly through.
    if !ended {
        out.push(item(HudKind::RingStreak, m.ring_streak as f32));
        out.push(item(HudKind::RingsNeeded, m.rings_needed as f32));
    }
    out.push(item(HudKind::Charge, unit(m.charge)));
    out.push(item(HudKind::ChargeGlow, unit(m.charge_glow)));
    out.push(item(HudKind::Boost, if m.boost.is_finite() { m.boost.clamp(0.0, 2.0) } else { 1.0 }));
    if !ended && m.window_left.is_finite() && m.window_left > 0.0 {
        out.push(item(HudKind::CatchWindow, m.window_left));
    }
    if ended {
        out.push(item(HudKind::Result, if m.phase == MatchPhase::Ended(Outcome::Won) { 1.0 } else { 0.0 }));
    } else {
        out.push(item(HudKind::Hints, HINTS.len() as f32));
    }
    out
}

/// The end-of-match line, once there is one.
pub fn result_line(m: &HudInput, opponent: &str) -> Option<String> {
    let MatchPhase::Ended(outcome) = m.phase else { return None };
    let word = match outcome {
        Outcome::Won => "Gryffindor won",
        Outcome::Lost => "Gryffindor lost",
    };
    Some(format!("{word} against {opponent}, {} - {}", m.score_home, m.score_away))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeking() -> HudInput {
        HudInput { phase: MatchPhase::Seeking, tries_left: 3, top_speed: 900.0, bumps_to_lose: 3, ..HudInput::default() }
    }

    fn value_of(items: &[HudItem], kind: HudKind) -> Option<f32> {
        items.iter().find(|i| i.kind == kind).map(|i| i.value)
    }

    #[test]
    fn hud_progress_item_matches_snitch_progress() {
        for p in [0.0, 0.37, 1.0] {
            let m = HudInput { progress: p, ..seeking() };
            let items = hud_items(&m);
            assert_eq!(value_of(&items, HudKind::SnitchProgress), Some(p), "progress {p}: {items:?}");
        }
        // Rubbish and out-of-range progress never reach the screen.
        for bad in [f32::NAN, -1.0, 2.0, f32::INFINITY] {
            let v = value_of(&hud_items(&HudInput { progress: bad, ..seeking() }), HudKind::SnitchProgress).unwrap();
            assert!((0.0..=1.0).contains(&v), "{bad} -> {v}");
        }
    }

    #[test]
    fn hud_shows_score_pips_speed_and_hints_while_seeking() {
        let m = HudInput { score_home: 150, score_away: 30, speed: 450.0, bumps: 1, ..seeking() };
        let items = hud_items(&m);
        assert_eq!(value_of(&items, HudKind::ScoreHome), Some(150.0));
        assert_eq!(value_of(&items, HudKind::ScoreAway), Some(30.0));
        assert_eq!(value_of(&items, HudKind::CatchPips), Some(3.0));
        assert_eq!(value_of(&items, HudKind::Speed), Some(0.5));
        // Two of three bumps left.
        assert!(value_of(&items, HudKind::Control).is_some_and(|v| (v - 2.0 / 3.0).abs() < 1e-6), "{items:?}");
        assert_eq!(value_of(&items, HudKind::Hints), Some(HINTS.len() as f32));
        // The match is not over, so there is no result line.
        assert_eq!(value_of(&items, HudKind::Result), None);
        assert_eq!(result_line(&m, "Slytherin"), None);
    }

    #[test]
    fn hud_shows_the_result_once_the_match_has_ended() {
        let won = HudInput { phase: MatchPhase::Ended(Outcome::Won), score_home: 150, ..seeking() };
        assert_eq!(value_of(&hud_items(&won), HudKind::Result), Some(1.0));
        let line = result_line(&won, "Slytherin").expect("a result line");
        assert!(line.contains("150") && line.to_lowercase().contains("won"), "{line}");
        let lost = HudInput { phase: MatchPhase::Ended(Outcome::Lost), score_away: 40, ..seeking() };
        assert_eq!(value_of(&hud_items(&lost), HudKind::Result), Some(0.0));
        assert!(result_line(&lost, "Slytherin").is_some_and(|l| l.to_lowercase().contains("lost")));
        // The hints go away with the match.
        assert_eq!(value_of(&hud_items(&lost), HudKind::Hints), None);
    }

    #[test]
    fn hud_survives_rubbish_input() {
        let m = HudInput { progress: f32::NAN, speed: f32::INFINITY, top_speed: 0.0, bumps: 9, bumps_to_lose: 0, ..seeking() };
        for i in hud_items(&m) {
            assert!(i.value.is_finite(), "{i:?}");
        }
    }

    /// The ring run: on screen while the match is on, gone with it, and its words say how many of the run the
    /// catch still wants.
    #[test]
    fn hud_shows_the_ring_streak_while_the_match_is_on() {
        let m = HudInput { ring_streak: 2, rings_needed: 3, ..seeking() };
        let items = hud_items(&m);
        assert_eq!(value_of(&items, HudKind::RingStreak), Some(2.0));
        assert_eq!(value_of(&items, HudKind::RingsNeeded), Some(3.0));
        assert_eq!(ring_line(2, 3).as_deref(), Some("RINGS 2/3"));
        // A run longer than the catch needs is not shown as more than it needs.
        assert_eq!(ring_line(5, 3).as_deref(), Some("RINGS 3/3"));
        // No rule, but a run: a plain count. Nothing either way: nothing on screen.
        assert_eq!(ring_line(4, 0).as_deref(), Some("RINGS x4"));
        assert_eq!(ring_line(0, 0), None);
        // The match ends and the rings go with it.
        let over = HudInput { phase: MatchPhase::Ended(Outcome::Won), ..m };
        assert_eq!(value_of(&hud_items(&over), HudKind::RingStreak), None);
        assert_eq!(value_of(&hud_items(&over), HudKind::RingsNeeded), None);
    }

    /// A match that is over has no catch attempts to show, however many were left when it ended.
    #[test]
    fn hud_shows_no_catch_pips_once_the_match_has_ended() {
        for o in [Outcome::Won, Outcome::Lost] {
            let m = HudInput { phase: MatchPhase::Ended(o), tries_left: 3, ..seeking() };
            assert_eq!(value_of(&hud_items(&m), HudKind::CatchPips), Some(0.0), "{o:?}");
        }
    }
}
