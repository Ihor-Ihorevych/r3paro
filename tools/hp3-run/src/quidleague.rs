//! `hp3-run quid-league <HP3 folder> [matches]`: plays the league's next fixtures through the **menu entry**,
//! headless, and prints what the league file holds afterwards.
//!
//! It takes exactly the path the `quidditch` command takes: `menu::request_next` leaves the request, the map
//! is loaded, `glue::start_from_env` picks the request up and starts the match, the scripted pilot of
//! `quid-smoke` flies it to the end, and the end of the match records the round and writes the league.
//!
//! The data folder it is given is also where the league is saved (`<root>/Save/League.txt`), so it must be a
//! **scratch** root -- a folder of junctions to the retail data with a `Save` folder of its own -- and never
//! the real install. `HP3_QUID_EASY=1` makes a match end in a few hundred ticks, which is what makes this
//! runnable at all.

use std::path::Path;
use std::process::ExitCode;

use hp3_package::PackageSet;
use hp3_script::ClassDb;
use hp3_world::quidditch::glue;
use hp3_world::quidditch::menu;
use hp3_world::quidditch::rules::MatchPhase;

use crate::quidsmoke::{steer, wants_catch};

/// Ticks a match may take before the run gives up on it.
const TICK_CAP: u32 = 20000;
/// Ticks run after the match has ended, so the result line, the league write and the travel back to the front
/// end all happen (`menu::LINGER_TICKS` plus a little).
const AFTER_END: u64 = 20;

pub fn run(root: &Path, matches: u32) -> Result<ExitCode, Box<dyn std::error::Error>> {
    let set = PackageSet::open(root)?;
    if std::env::var_os("HP3_QUID_EASY").is_none() {
        println!("quid-league: set HP3_QUID_EASY=1, or a match takes a player to finish");
    }
    let mut ok = true;
    for round in 1..=matches {
        let mut db = ClassDb::new(&set);
        db.load_all_classes();
        let mut vm = hp3_script::Vm::new(db);
        // The front end: the league says whom, the request is left, and the map is loaded as the command's
        // level switch would load it.
        let pkg = set.get("HP3_Quidditch")?;
        hp3_world::load(&mut vm, &pkg, hp3_core::Config::load(root))?;
        let asked = menu::request_next(&mut vm);
        let Some(opponent) = asked else {
            println!("quid-league: the season is over after {} matches", round - 1);
            break;
        };
        println!("quid-league: match {round}, the league's next opponent is {}", opponent.name());
        // What the end of the level load does with the request: the same call, on the loaded Quidditch map.
        glue::start_from_env(&mut vm)?;
        let started = hp3_world::world(&mut vm).quidditch.as_ref().map(|m| m.opponent.clone());
        match &started {
            Some(o) => println!("quid-league: the match the entry started is against {o}"),
            None => {
                println!("quid-league: NO match started");
                ok = false;
                continue;
            }
        }
        let mut ticks = 0;
        let mut ended_at: Option<u64> = None;
        // HP3_QUID_LEAVE_AT=<tick>: the player walks out of the match at that tick instead of playing it to
        // the end (what `quidditch stop` and the F12 entry do), to show that the exit path is clean.
        let leave_at: Option<u32> = std::env::var("HP3_QUID_LEAVE_AT").ok().and_then(|v| v.parse().ok());
        while ticks < TICK_CAP {
            if leave_at == Some(ticks) {
                let map = menu::leave(&mut vm);
                let gone = hp3_world::world(&mut vm).quidditch.is_none();
                println!("quid-league: left the match at tick {ticks}, match state {}, back to {map}", if gone { "cleared" } else { "STILL THERE" });
                ok &= gone;
                // The level switch the front end now asks for must be allowed: nothing blocks it any more.
                hp3_world::tick(&mut vm, 1.0 / 60.0);
                println!("quid-league: a tick after the exit went through, {} script errors", hp3_world::world(&mut vm).errors.len());
                break;
            }
            let plan = hp3_world::world(&mut vm).quidditch.as_ref().map(|m| (steer(&m.broom, m.snitch.pos(), false), wants_catch(&m.broom, m.snitch.pos())));
            if let Some((input, catch)) = plan {
                glue::feed_scripted(&mut vm, input, catch);
            }
            hp3_world::tick(&mut vm, 1.0 / 60.0);
            ticks += 1;
            let state = hp3_world::world(&mut vm).quidditch.as_ref().map(|m| (m.rules.phase(), m.rules.score()));
            match state {
                Some((MatchPhase::Ended(o), score)) => {
                    if ended_at.is_none() {
                        println!("quid-league: the match ended {o:?} {}-{} at tick {ticks}", score.home, score.away);
                        ended_at = Some(u64::from(ticks));
                    }
                    // The match stays up for its result line, then stops itself and asks for the travel back.
                    if u64::from(ticks) >= ended_at.unwrap_or(0) + menu::LINGER_TICKS + AFTER_END {
                        break;
                    }
                }
                // The match stopped itself (the exit path) -- which is the end of this round.
                None if ended_at.is_some() => break,
                _ => {}
            }
        }
        if ended_at.is_none() && leave_at.is_none() {
            println!("quid-league: NO result in {ticks} ticks");
            ok = false;
        }
        let gone = hp3_world::world(&mut vm).quidditch.is_none();
        let travel = hp3_world::take_travel(&mut vm);
        println!("quid-league: after the match the match state is {}, the travel asked for is {travel:?}", if gone { "cleared" } else { "STILL THERE" });
        ok &= gone;
        // What was saved, and whom the entry would start next.
        let path = menu::league_path(&mut vm);
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        println!("quid-league: {} holds:\n{}", path.display(), text.trim_end());
        for line in menu::report(&mut vm) {
            println!("quid-league: {line}");
        }
        let errors = hp3_world::world(&mut vm).errors.len();
        for e in hp3_world::world(&mut vm).errors.iter().take(5) {
            println!("quid-league: script error: {e}");
        }
        println!("quid-league: match {round} done, {errors} script errors");
        ok &= errors == 0;
    }
    Ok(if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}
