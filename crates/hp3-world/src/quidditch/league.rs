//! The Quidditch cup: whom Gryffindor plays next, what a result does to the house table, and the short text
//! the whole thing saves as.
//!
//! Pure Rust, no VM: the menu side (`quidditch::menu`) is what reads and writes it next to the game's own save
//! slots and what starts a match against `next_opponent`.
//!
//! The season is six group rounds -- Slytherin, Ravenclaw and Hufflepuff twice each, the first three at home
//! and the last three away -- and then a final, which is played only if Gryffindor **qualifies**: at least
//! three wins of the six (`WINS_TO_QUALIFY`). The final is against the best-placed of the other three houses.
//!
//! A win is worth `WIN_POINTS` (two), as the real tables do it; a draw would be worth `DRAW_POINTS` (one), but
//! `rules::Outcome` has only `Won` and `Lost` (a tied match is settled as a loss for the home side, see
//! `Rules::on_snitch`), so no draw ever reaches the league and `DRAW_POINTS` is documentation.
//!
//! The other houses play each other in every round Gryffindor plays, and those results are **simulated
//! deterministically** from `SIM_SEED` and the round's number, so the table is complete without anyone having
//! to fly those matches. The same league always simulates the same way, which is what makes the save a
//! replay of the recorded results and nothing more.

use super::rules::{Outcome, Score};

/// Points for a win.
pub const WIN_POINTS: u32 = 2;
/// Points a draw would be worth. No match can end in one (see the module note), so nothing adds it.
pub const DRAW_POINTS: u32 = 1;
/// Group rounds before the final.
pub const GROUP_ROUNDS: usize = 6;
/// Wins of the six that put Gryffindor in the final.
pub const WINS_TO_QUALIFY: u32 = 3;
/// What the other houses' rounds are decided from (see the module note).
const SIM_SEED: u64 = 0x9e37_79b9_7f4a_7c15;
/// The first line of a saved league, with its format number.
const HEADER: &str = "HP3LEAGUE 1";

/// A house Gryffindor is drawn against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opponent {
    Slytherin,
    Ravenclaw,
    Hufflepuff,
}

/// A house in the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Team {
    Gryffindor,
    Slytherin,
    Ravenclaw,
    Hufflepuff,
}

/// The houses in the order `League::points` holds them.
pub const TEAMS: [Team; 4] = [Team::Gryffindor, Team::Slytherin, Team::Ravenclaw, Team::Hufflepuff];

/// The three houses Gryffindor plays, in the order the season draws them.
pub const OPPONENTS: [Opponent; 3] = [Opponent::Slytherin, Opponent::Ravenclaw, Opponent::Hufflepuff];

impl Opponent {
    /// The name the match and the HUD use (`QuidditchMatch::opponent`).
    pub fn name(self) -> &'static str {
        match self {
            Opponent::Slytherin => "Slytherin",
            Opponent::Ravenclaw => "Ravenclaw",
            Opponent::Hufflepuff => "Hufflepuff",
        }
    }

    /// A house by name, however it was typed (the `quidditch <house>` command). An abbreviation is enough as
    /// long as it is the start of exactly one house's name.
    pub fn parse(s: &str) -> Option<Opponent> {
        let s = s.trim().to_ascii_lowercase();
        if s.is_empty() {
            return None;
        }
        let mut found = None;
        for o in OPPONENTS {
            if o.name().to_ascii_lowercase().starts_with(&s) {
                if found.is_some() {
                    return None;
                }
                found = Some(o);
            }
        }
        found
    }

    pub fn team(self) -> Team {
        match self {
            Opponent::Slytherin => Team::Slytherin,
            Opponent::Ravenclaw => Team::Ravenclaw,
            Opponent::Hufflepuff => Team::Hufflepuff,
        }
    }

    fn letter(self) -> char {
        match self {
            Opponent::Slytherin => 'S',
            Opponent::Ravenclaw => 'R',
            Opponent::Hufflepuff => 'H',
        }
    }

    fn from_letter(c: char) -> Option<Opponent> {
        OPPONENTS.into_iter().find(|o| o.letter() == c.to_ascii_uppercase())
    }
}

impl Team {
    pub fn name(self) -> &'static str {
        match self {
            Team::Gryffindor => "Gryffindor",
            Team::Slytherin => "Slytherin",
            Team::Ravenclaw => "Ravenclaw",
            Team::Hufflepuff => "Hufflepuff",
        }
    }

    /// Where the house sits in `League::points`.
    pub fn index(self) -> usize {
        TEAMS.iter().position(|&t| t == self).unwrap_or(0)
    }

    /// The house as an opponent of Gryffindor, which Gryffindor itself is not.
    pub fn as_opponent(self) -> Option<Opponent> {
        OPPONENTS.into_iter().find(|o| o.team() == self)
    }
}

/// One fixture of the season: whom, whether it is the final, and how it went once it has been played.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Round {
    pub opponent: Opponent,
    /// Whether this is the final rather than one of the six group rounds.
    pub final_: bool,
    pub result: Option<(Outcome, Score)>,
}

/// The season: its fixtures and the house table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct League {
    pub rounds: Vec<Round>,
    /// Points per house, in `TEAMS` order (Gryffindor, Slytherin, Ravenclaw, Hufflepuff).
    pub points: [u32; 4],
}

/// A small deterministic mix, for the other houses' results.
fn mix(seed: u64, n: u64) -> u64 {
    let mut h = seed ^ n.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 29;
    h
}

impl League {
    /// A fresh season: six group rounds (Slytherin, Ravenclaw, Hufflepuff at home, then the same three away)
    /// and the final, whose opponent is only known once the group is over -- until then it holds the first
    /// house as a placeholder and `next_opponent` refuses to name it.
    pub fn new() -> League {
        let mut rounds: Vec<Round> = Vec::with_capacity(GROUP_ROUNDS + 1);
        for _leg in 0..2 {
            for o in OPPONENTS {
                rounds.push(Round { opponent: o, final_: false, result: None });
            }
        }
        rounds.push(Round { opponent: OPPONENTS[0], final_: true, result: None });
        League { rounds, points: [0; 4] }
    }

    /// The group rounds, played or not.
    fn group(&self) -> impl Iterator<Item = &Round> {
        self.rounds.iter().filter(|r| !r.final_)
    }

    /// Group rounds Gryffindor has won.
    pub fn wins(&self) -> u32 {
        self.group().filter(|r| matches!(r.result, Some((Outcome::Won, _)))).count() as u32
    }

    /// Whether all six group rounds have been played.
    pub fn group_done(&self) -> bool {
        self.group().all(|r| r.result.is_some())
    }

    /// Whether Gryffindor reached the final: the whole group played and at least `WINS_TO_QUALIFY` wins of it.
    pub fn qualified(&self) -> bool {
        self.group_done() && self.wins() >= WINS_TO_QUALIFY
    }

    /// The index of the round a result would be recorded into, if there is one.
    fn next_round(&self) -> Option<usize> {
        let qualified = self.qualified();
        self.rounds.iter().position(|r| r.result.is_none() && (!r.final_ || qualified))
    }

    /// Whom Gryffindor plays next: the next unplayed group round, then the final if they qualified. `None`
    /// once the season is over, and once a group that fell short of the final is.
    pub fn next_opponent(&self) -> Option<Opponent> {
        self.next_round().map(|i| self.rounds[i].opponent)
    }

    /// The best-placed house apart from Gryffindor: Gryffindor's opponent in the final, and the champion of a
    /// season with no final. Ties go to the house that comes first in `TEAMS`.
    fn best_rival(&self) -> Opponent {
        let mut best = OPPONENTS[0];
        for o in OPPONENTS {
            if self.points[o.team().index()] > self.points[best.team().index()] {
                best = o;
            }
        }
        best
    }

    /// Records the result of the round `next_opponent` named, gives out its points, plays the other two houses
    /// against each other (simulated from the seed), and, once the group is over, puts the best-placed rival
    /// in the final. A result offered when there is no round left is dropped.
    pub fn record(&mut self, r: Outcome, score: Score) {
        let Some(i) = self.next_round() else { return };
        let (opponent, is_final) = (self.rounds[i].opponent, self.rounds[i].final_);
        self.rounds[i].result = Some((r, score));
        match r {
            Outcome::Won => self.points[Team::Gryffindor.index()] += WIN_POINTS,
            Outcome::Lost => self.points[opponent.team().index()] += WIN_POINTS,
        }
        // The two houses with nothing else to do this round play each other. Deterministic, so a saved league
        // always comes back to the same table.
        if !is_final {
            let others: Vec<Opponent> = OPPONENTS.into_iter().filter(|&o| o != opponent).collect();
            let winner = others[(mix(SIM_SEED, i as u64) & 1) as usize];
            self.points[winner.team().index()] += WIN_POINTS;
        }
        if !is_final && self.group_done() {
            let rival = self.best_rival();
            if let Some(f) = self.rounds.iter_mut().find(|r| r.final_) {
                f.opponent = rival;
            }
        }
    }

    /// Who won the cup, once it has been decided: the winner of the final if one was played, else the house
    /// at the top of the table of a season Gryffindor did not qualify from. `None` while anything is still to
    /// be played.
    pub fn champion(&self) -> Option<Team> {
        let f = self.rounds.iter().find(|r| r.final_)?;
        if let Some((o, _)) = f.result {
            return Some(match o {
                Outcome::Won => Team::Gryffindor,
                Outcome::Lost => f.opponent.team(),
            });
        }
        if !self.group_done() || self.qualified() {
            return None;
        }
        Some(self.best_rival().team())
    }

    /// The season as one short text, for the save: a header line and one line per played round (whom, won or
    /// lost, and the score). Everything else -- the points, the final's opponent -- comes back out of
    /// replaying those results, so there is nothing in the file that can disagree with itself.
    pub fn to_text(&self) -> String {
        let mut out = String::from(HEADER);
        for r in &self.rounds {
            let Some((o, score)) = r.result else { continue };
            let w = if o == Outcome::Won { 'W' } else { 'L' };
            out.push_str(&format!("\n{} {w} {} {}", r.opponent.letter(), score.home, score.away));
        }
        out.push('\n');
        out
    }

    /// A league read back from `to_text`. `None` when the text is not one (a missing or wrong header); a line
    /// that makes no sense is skipped, so a half-written file still gives back the rounds before it.
    pub fn from_text(s: &str) -> Option<League> {
        let mut lines = s.lines();
        let header = lines.next()?.trim();
        if !header.eq_ignore_ascii_case(HEADER) {
            return None;
        }
        let mut league = League::new();
        for line in lines {
            let f: Vec<&str> = line.split_whitespace().collect();
            let [house, result, home, away] = f[..] else { continue };
            let (Some(_), Some(home), Some(away)) = (
                house.chars().next().and_then(Opponent::from_letter),
                home.parse::<u32>().ok(),
                away.parse::<u32>().ok(),
            ) else {
                continue;
            };
            let outcome = match result.to_ascii_uppercase().as_str() {
                "W" => Outcome::Won,
                "L" => Outcome::Lost,
                _ => continue,
            };
            league.record(outcome, Score { home, away });
        }
        Some(league)
    }

    /// One line per house, best first: the table, for the log and the result screen.
    pub fn table(&self) -> Vec<String> {
        let mut rows: Vec<(u32, Team)> = TEAMS.iter().map(|&t| (self.points[t.index()], t)).collect();
        rows.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.index().cmp(&b.1.index())));
        rows.into_iter().map(|(p, t)| format!("{:<11} {p:2}", t.name())).collect()
    }
}

impl Default for League {
    fn default() -> League {
        League::new()
    }
}

/// What the menu entry asks the league for: whom to start a match against, or `None` when the season is over
/// (nothing is started then). The one function between the front end and the league.
pub fn next_match_request(league: &League) -> Option<Opponent> {
    league.next_opponent()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(home: u32, away: u32) -> Score {
        Score { home, away }
    }

    /// A win and a loss, as a match ends them.
    fn win() -> (Outcome, Score) {
        (Outcome::Won, score(170, 20))
    }

    fn loss() -> (Outcome, Score) {
        (Outcome::Lost, score(20, 170))
    }

    #[test]
    fn league_has_six_rounds_then_a_final() {
        let mut l = League::new();
        assert_eq!(l.rounds.len(), GROUP_ROUNDS + 1);
        // The six group rounds: the three houses at home, then the three away.
        let group: Vec<Opponent> = l.group().map(|r| r.opponent).collect();
        assert_eq!(group, vec![Opponent::Slytherin, Opponent::Ravenclaw, Opponent::Hufflepuff, Opponent::Slytherin, Opponent::Ravenclaw, Opponent::Hufflepuff]);
        assert!(l.rounds[GROUP_ROUNDS].final_ && l.rounds[..GROUP_ROUNDS].iter().all(|r| !r.final_));
        assert_eq!(l.next_opponent(), Some(Opponent::Slytherin));
        // Winning the group: each result lands in its own round, in order, and the final comes last.
        for (i, want) in group.iter().enumerate() {
            assert_eq!(l.next_opponent(), Some(*want), "round {i}");
            let (o, s) = win();
            l.record(o, s);
            assert_eq!(l.rounds[i].result, Some((o, s)));
        }
        assert!(l.group_done() && l.qualified() && l.wins() == 6);
        // Six wins is twelve points and a place in the final, against a house (not Gryffindor itself).
        assert_eq!(l.points[Team::Gryffindor.index()], 6 * WIN_POINTS);
        let f = l.next_opponent().expect("a final to play");
        assert_eq!(l.rounds[GROUP_ROUNDS].opponent, f);
        assert_eq!(l.champion(), None, "the final decides it");
        let (o, s) = win();
        l.record(o, s);
        assert_eq!(l.champion(), Some(Team::Gryffindor));
        assert_eq!(l.next_opponent(), None, "the season is over");
        // The other houses played too, so the table is complete.
        assert!(TEAMS.iter().all(|&t| t == Team::Gryffindor || l.points[t.index()] > 0), "{:?}", l.points);
    }

    #[test]
    fn losing_every_round_gives_no_final() {
        let mut l = League::new();
        for _ in 0..GROUP_ROUNDS {
            let (o, s) = loss();
            l.record(o, s);
        }
        assert!(l.group_done() && !l.qualified() && l.wins() == 0);
        assert_eq!(l.next_opponent(), None);
        assert_eq!(next_match_request(&l), None);
        // The final is never played, and the cup goes to the best of the others.
        let (o, s) = win();
        l.record(o, s);
        assert_eq!(l.rounds[GROUP_ROUNDS].result, None);
        let champion = l.champion().expect("a champion without Gryffindor");
        assert_ne!(champion, Team::Gryffindor);
        assert_eq!(l.points[Team::Gryffindor.index()], 0);
        // Three wins of six is exactly enough.
        let mut l = League::new();
        for i in 0..GROUP_ROUNDS {
            let (o, s) = if i < 3 { win() } else { loss() };
            l.record(o, s);
        }
        assert!(l.qualified() && l.next_opponent().is_some());
    }

    #[test]
    fn league_round_trips_through_save() {
        let mut l = League::new();
        for (i, _) in (0..4).enumerate() {
            let (o, s) = if i % 2 == 0 { win() } else { loss() };
            l.record(o, s);
        }
        let text = l.to_text();
        let back = League::from_text(&text).expect("the text is a league");
        assert_eq!(back, l);
        assert_eq!(back.to_text(), text);
        assert_eq!(back.next_opponent(), l.next_opponent());
        // A whole season, final and all, comes back the same.
        let mut played = l.clone();
        while played.next_opponent().is_some() {
            let (o, s) = win();
            played.record(o, s);
        }
        let back = League::from_text(&played.to_text()).expect("a finished season");
        assert_eq!(back, played);
        assert_eq!(back.champion(), played.champion());
        // A fresh league is a header and nothing else, and it comes back fresh.
        assert_eq!(League::from_text(&League::new().to_text()), Some(League::new()));
        // Not a league: nothing comes back rather than a wrong one.
        assert_eq!(League::from_text(""), None);
        assert_eq!(League::from_text("rubbish\nS W 10 20"), None);
        // A line that makes no sense is skipped; the ones before it stand.
        let partial = League::from_text("HP3LEAGUE 1\nS W 170 20\nnonsense\nX ? x y\n").expect("a league");
        assert_eq!(partial.rounds[0].result, Some(win()));
        assert_eq!(partial.next_opponent(), Some(Opponent::Ravenclaw));
    }

    #[test]
    fn menu_entry_starts_match_with_next_opponent() {
        let mut l = League::new();
        // The entry asks the league, and the league names the first fixture.
        assert_eq!(next_match_request(&l), Some(Opponent::Slytherin));
        assert_eq!(next_match_request(&l).map(Opponent::name), Some("Slytherin"));
        let (o, s) = win();
        l.record(o, s);
        // The next time the entry is used it is the next fixture, not the same one again.
        assert_eq!(next_match_request(&l), Some(Opponent::Ravenclaw));
        let (o, s) = loss();
        l.record(o, s);
        assert_eq!(next_match_request(&l), Some(Opponent::Hufflepuff));
        // However it was typed, the command names the same house.
        assert_eq!(Opponent::parse("ravenclaw"), Some(Opponent::Ravenclaw));
        assert_eq!(Opponent::parse(" HUF "), Some(Opponent::Hufflepuff));
        assert_eq!(Opponent::parse("s"), Some(Opponent::Slytherin));
        assert_eq!(Opponent::parse("gryffindor"), None);
        assert_eq!(Opponent::parse(""), None);
    }

    /// The other houses' rounds are made up, but always the same way: the same results give the same table.
    #[test]
    fn the_simulated_table_is_the_same_every_time() {
        let play = || {
            let mut l = League::new();
            for i in 0..GROUP_ROUNDS {
                let (o, s) = if i % 3 == 0 { win() } else { loss() };
                l.record(o, s);
            }
            l
        };
        assert_eq!(play(), play());
        let l = play();
        // Every round hands out exactly two lots of points: Gryffindor's fixture and the other two houses'.
        assert_eq!(l.points.iter().sum::<u32>(), 2 * WIN_POINTS * GROUP_ROUNDS as u32);
        assert!(l.table().len() == 4 && l.table()[0].split_whitespace().count() == 2, "{:?}", l.table());
    }
}
