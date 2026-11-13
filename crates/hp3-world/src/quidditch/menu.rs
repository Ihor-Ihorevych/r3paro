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
//! travels back to where the match was asked for (the level and story state it was asked from, the front end when it was asked
//! there). Leaving early (`quidditch stop`) is the same thing without the wait. Both
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
pub const LINGER_TICKS: u64 = 540;
/// The save slot that holds the level a match was asked from, so the match ends with the player back on the very spot they
/// stood (next to the quick save's 99, outside the slots the game's own load page lists).
pub const RETURN_SLOT: i32 = 98;
/// What a won match gives: Bertie Bott's beans, cauldron cakes and pumpkin pasties.
pub const REWARD_BEANS: i32 = 50;
pub const REWARD_CAKES: i32 = 5;
pub const REWARD_PASTIES: i32 = 5;

thread_local! {
    /// Won matches whose reward is not paid yet: it waits for a level with a player in it (`after_level_ready`).
    static PENDING_WINS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    /// The match the front end asked for, waiting for the level that can play it. A thread local for the same
    /// reason the command queue is a static: it is set outside the VM (a menu pick) and read inside it.
    static REQUEST: RefCell<Option<String>> = const { RefCell::new(None) };
    /// Where the player was when the match was asked for (a map URL with its story state), so the match ends back there. None:
    /// the match was asked for from the front end, and ends there.
    static ORIGIN: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Where the league is saved: beside `Save<slot>.usa`, so a scratch data root (the tests, the headless
/// drivers) keeps its own league and the real game's is never touched.
///
/// Each New Game profile keeps its own season: profile 0 (the first slot, and a game with no profile chosen) uses `League.txt`
/// as before, profile `n` uses `League<n>.txt`.
pub fn league_path(vm: &mut Vm<'_>) -> PathBuf {
    let profile = crate::world(vm).viewport.gui.map_or(0, |g| crate::get(vm, g, "CurUserID").as_int());
    let file = if profile > 0 { format!("League{profile}.txt") } else { LEAGUE_FILE.to_string() };
    crate::save::save_dir(vm).join(file)
}

/// The saved league, or a fresh season when there is no file (or it is not one).
pub fn read_league(vm: &mut Vm<'_>) -> League {
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
pub fn next_opponent(vm: &mut Vm<'_>) -> Option<Opponent> {
    league::next_match_request(&read_league(vm))
}

/// Asks for a match against `opponent` on the next load of `MATCH_MAP`. A match already running is stopped
/// first, so the level that is left behind is a tidy one.
pub fn request(vm: &mut Vm<'_>, opponent: Opponent) {
    remember_origin(vm);
    super::glue::stop(vm);
    REQUEST.with(|r| *r.borrow_mut() = Some(opponent.name().to_string()));
    let line = format!("quidditch: a match against {} was asked for, loading {MATCH_MAP}", opponent.name());
    println!("{line}");
    vm.log.push(line);
}

/// Notes the level the player asks for a match from, with its story state, so the match can hand them back to it. Asked from the
/// front end or from a match (a rematch), the origin stays as it was: the front end, or wherever the first match came from.
fn remember_origin(vm: &mut Vm<'_>) {
    let map = crate::world(vm).map.name.to_string();
    if map.eq_ignore_ascii_case(MATCH_MAP) {
        return;
    }
    if map.eq_ignore_ascii_case(&front_end_map(vm)) {
        ORIGIN.with(|o| *o.borrow_mut() = None);
        return;
    }
    let state = crate::world(vm)
        .level_info
        .and_then(|li| crate::get(vm, li, "Game").as_obj())
        .map(|g| crate::get(vm, g, "CurrentGameState").as_str().to_string())
        .filter(|s| !s.is_empty());
    // The level as it is now goes into the return slot, so the match ends with the player where they stood (with what they
    // carry). Without it (a write that failed) they are put back at the start of the story state.
    let url = if crate::save::write(vm, RETURN_SLOT) {
        format!("{map}?load={RETURN_SLOT}")
    } else {
        match state {
            Some(s) => format!("{map}?GameState={s}"),
            None => map,
        }
    };
    println!("quidditch: the match was asked for from {url}");
    ORIGIN.with(|o| *o.borrow_mut() = Some(url));
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
    // A win is paid once the player is back in a level (`after_level_ready`).
    if outcome == Outcome::Won {
        PENDING_WINS.with(|w| w.set(w.get() + 1));
    }
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
    let map = match ORIGIN.with(|o| o.borrow_mut().take()) {
        Some(origin) => origin,
        None => front_end_map(vm),
    };
    let line = format!("quidditch: leaving the match, back to {map}");
    println!("{line}");
    vm.log.push(line);
    map
}

/// A level has finished loading (and a saved one has been put over it). Back from a match through the return slot, the slot
/// has done its job and goes. Wins not paid yet are paid on the first level that has a player in it: the one the match was
/// asked from, or, for a match asked from the front end (no level to go back to), the next level the player loads. The front
/// end and the match itself pay nothing (the front end has no player, the match's pawn is the broom).
pub fn after_level_ready(vm: &mut Vm<'_>, url: &str) {
    if crate::world(vm).map.name.eq_ignore_ascii_case(MATCH_MAP) {
        return;
    }
    if crate::save::option_slot(url) == Some(RETURN_SLOT) {
        crate::save::delete(vm, RETURN_SLOT);
    }
    let owed = PENDING_WINS.with(|w| w.get());
    if owed == 0 {
        return;
    }
    let front = front_end_map(vm);
    let has_pawn = crate::world(vm).player.is_some_and(|pc| crate::get(vm, pc, "Pawn").as_obj().is_some());
    if crate::world(vm).map.name.eq_ignore_ascii_case(&front) || !has_pawn {
        let line = format!("quidditch: {owed} won match(es) not paid yet: no player here, paid on the next level with one");
        println!("{line}");
        vm.log.push(line);
        return;
    }
    PENDING_WINS.with(|w| w.set(0));
    pay_wins(vm, owed);
}

/// Gives the player the reward of `wins` won matches: each is 50 beans, 5 cauldron cakes and 5 pumpkin pasties. The log line
/// has what the inventory carrier held before and after, so a reward that went missing shows where.
fn pay_wins(vm: &mut Vm<'_>, wins: u32) {
    let Some(pc) = crate::world(vm).player else { return };
    let n = wins as i32;
    let before = crate::gifts::collection_counts(vm);
    crate::fire(vm, pc, "GiveBeans", &mut [hp3_script::Value::Int(REWARD_BEANS * n)]);
    crate::fire(vm, pc, "GiveCakes", &mut [hp3_script::Value::Int(REWARD_CAKES * n)]);
    let pasties = crate::gifts::give_pasties(vm, REWARD_PASTIES * n);
    let after = crate::gifts::collection_counts(vm);
    let line = format!(
        "quidditch: {wins} win(s) paid: {} beans, {} cauldron cakes, {} pumpkin pasties (before: {before}; after: {after})",
        REWARD_BEANS * n,
        REWARD_CAKES * n,
        if pasties { REWARD_PASTIES * n } else { 0 }
    );
    println!("{line}");
    vm.log.push(line);
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
