//! The way into a match from inside the game, and the way back out: the league file next to the saves, the
//! request the front end leaves for the next level load, and the exit that hands the player back to the menu.
//!
//! The entry itself is the `quidditch` command (`apps/hp3/src/commands.rs`, on the F12 debug list): it asks
//! the league whom Gryffindor plays next, leaves that request here and sends the game to `MATCH_MAP`. The
//! level load calls `glue::start_from_env`, which takes the request and starts the match -- the same door
//! `HP3_MATCH=<opponent>` uses, so there is one start path and not two.
//!
//! When a match started that way ends, `glue::tick` records the result in the league, writes it next to the
//! game's own slots and leaves the result line up for `LINGER_TICKS`; then the match is stopped and the player
//! travels back to the front end. Leaving early (`quidditch stop`) is the same thing without the wait. Both
//! stop the match **before** travelling, because `count_down_travel` refuses a level switch while one runs.

use std::cell::RefCell;
use std::path::PathBuf;

use hp3_script::Vm;

use super::league::{self, League, Opponent};
use super::rules::{Outcome, Score};

/// The map a match is played on.
pub const MATCH_MAP: &str = "HP3_Quidditch";
/// The league's file, in the same folder as the game's save slots.
const LEAGUE_FILE: &str = "League.txt";
/// The front-end map, when the ini names none.
const FRONT_END: &str = "HP_Preamble";
/// How long the result line stays on screen before the player is taken back to the menu (ticks of 1/60 s).
pub const LINGER_TICKS: u64 = 300;

thread_local! {
    /// The match the front end asked for, waiting for the level that can play it. A thread local for the same
    /// reason the command queue is a static: it is set outside the VM (a menu pick) and read inside it.
    static REQUEST: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Where the league is saved: beside `Save<slot>.usa`, so a scratch data root (the tests, the headless
/// drivers) keeps its own league and the real game's is never touched.
pub fn league_path(vm: &Vm<'_>) -> PathBuf {
    crate::save::save_dir(vm).join(LEAGUE_FILE)
}

/// The saved league, or a fresh season when there is no file (or it is not one).
pub fn read_league(vm: &Vm<'_>) -> League {
    std::fs::read_to_string(league_path(vm)).ok().and_then(|t| League::from_text(&t)).unwrap_or_default()
}

/// Writes the league beside the save slots. Written to a temporary file and moved over, as `save::write` does,
/// so a failed write never leaves half a league.
pub fn write_league(vm: &mut Vm<'_>, l: &League) -> bool {
    let path = league_path(vm);
    let Some(dir) = path.parent().map(PathBuf::from) else { return false };
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    let part = path.with_extension("tmp");
    let ok = std::fs::write(&part, l.to_text()).and_then(|_| std::fs::rename(&part, &path)).is_ok();
    let line = format!("quidditch: league {} ({})", if ok { "saved" } else { "could NOT be saved" }, path.display());
    println!("{line}");
    vm.log.push(line);
    ok
}

/// Whom the saved league says Gryffindor plays next; `None` once the season is over.
pub fn next_opponent(vm: &Vm<'_>) -> Option<Opponent> {
    league::next_match_request(&read_league(vm))
}

/// Asks for a match against `opponent` on the next load of `MATCH_MAP`. A match already running is stopped
/// first, so the level that is left behind is a tidy one.
pub fn request(vm: &mut Vm<'_>, opponent: Opponent) {
    super::glue::stop(vm);
    REQUEST.with(|r| *r.borrow_mut() = Some(opponent.name().to_string()));
    let line = format!("quidditch: a match against {} was asked for, loading {MATCH_MAP}", opponent.name());
    println!("{line}");
    vm.log.push(line);
}

/// Asks for the league's next fixture. `None` (and nothing asked for) when the season is over.
pub fn request_next(vm: &mut Vm<'_>) -> Option<Opponent> {
    let o = next_opponent(vm)?;
    request(vm, o);
    Some(o)
}

/// The match the front end asked for, taken (so it is started once).
pub fn take_request() -> Option<String> {
    REQUEST.with(|r| r.borrow_mut().take())
}

/// Drops a request nothing has started yet (the player went somewhere else).
pub fn clear_request() {
    REQUEST.with(|r| *r.borrow_mut() = None);
}

/// The map the game's front end lives on (`URL/LocalMap` of the ini, `HP_Preamble` when it names none), in the
/// spelling the package set has for it.
pub fn front_end_map(vm: &mut Vm<'_>) -> String {
    let named = crate::world(vm).config.get("URL", "LocalMap").map(|m| m.trim_end_matches(".unr").trim_end_matches(".UNR").to_string());
    let name = named.filter(|m| !m.is_empty()).unwrap_or_else(|| FRONT_END.to_string());
    match vm.db.set.get(&name) {
        Ok(pkg) => pkg.name.to_string(),
        Err(_) => name,
    }
}

/// The result of a match that was started from the menu: recorded in the league, saved, and reported with the
/// table as it now stands. Called by `glue::tick` the moment the rules end the match.
pub fn record_result(vm: &mut Vm<'_>, opponent: &str, outcome: Outcome, score: Score) {
    let mut l = read_league(vm);
    let expected = l.next_opponent();
    // The round the league is on must be the match that was played; a mismatch (a match started by hand
    // against another house) is recorded all the same, against the round that is due, and said out loud.
    if expected.map(Opponent::name) != Some(opponent) {
        let line = format!("quidditch: {opponent} was played but the league is on {expected:?}; the result goes to that round");
        println!("{line}");
        vm.log.push(line);
    }
    l.record(outcome, score);
    write_league(vm, &l);
    let line = format!(
        "quidditch: league round recorded ({opponent}: {outcome:?} {}-{}), next {:?}, champion {:?}",
        score.home,
        score.away,
        l.next_opponent().map(Opponent::name),
        l.champion().map(league::Team::name),
    );
    println!("{line}");
    vm.log.push(line);
    for row in l.table() {
        println!("quidditch: table {row}");
    }
}

/// Ends the match and hands the player back to the front end: the match is stopped **first** (a level switch
/// is refused while one runs) and the map to go to is returned, for the caller to travel to.
pub fn leave(vm: &mut Vm<'_>) -> String {
    super::glue::stop(vm);
    clear_request();
    crate::world(vm).quidditch_travel_blocked = false;
    let map = front_end_map(vm);
    let line = format!("quidditch: leaving the match, back to {map}");
    println!("{line}");
    vm.log.push(line);
    map
}

/// The league as lines for the log (`quidditch league`).
pub fn report(vm: &mut Vm<'_>) -> Vec<String> {
    let l = read_league(vm);
    let mut out = vec![format!("league: {} ({})", league_path(vm).display(), if league_path(vm).exists() { "saved" } else { "nothing saved yet" })];
    for (i, r) in l.rounds.iter().enumerate() {
        let what = if r.final_ { "final" } else { "round" };
        let result = match r.result {
            Some((o, s)) => format!("{o:?} {}-{}", s.home, s.away),
            None => "not played".to_string(),
        };
        out.push(format!("  {what} {}: {:<11} {result}", i + 1, r.opponent.name()));
    }
    out.extend(l.table().into_iter().map(|t| format!("  {t}")));
    out.push(format!("  next: {:?}, champion: {:?}", l.next_opponent().map(Opponent::name), l.champion().map(league::Team::name)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The request the front end leaves is taken exactly once (so one pick starts one match).
    #[test]
    fn a_request_is_taken_once() {
        clear_request();
        assert_eq!(take_request(), None);
        REQUEST.with(|r| *r.borrow_mut() = Some("Ravenclaw".to_string()));
        assert_eq!(take_request(), Some("Ravenclaw".to_string()));
        assert_eq!(take_request(), None);
    }
}
