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
}

/// The control hints, in the order they are shown.
pub const HINTS: [&str; 4] = ["Arrows/Mouse: fly", "A: boost", "Z: brake", "Space: catch"];

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
        item(HudKind::CatchPips, m.tries_left as f32),
        item(HudKind::Speed, unit(num(m.speed) / num(m.top_speed).max(1.0))),
        // Hits left before the Bludgers take the broom off him, as a fraction.
        item(HudKind::Control, unit(1.0 - m.bumps as f32 / (m.bumps_to_lose.max(1)) as f32)),
    ];
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
        HudInput { phase: MatchPhase::Seeking, tries_left: 3, top_speed: 1200.0, bumps_to_lose: 3, ..HudInput::default() }
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
        let m = HudInput { score_home: 150, score_away: 30, speed: 600.0, bumps: 1, ..seeking() };
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
}
