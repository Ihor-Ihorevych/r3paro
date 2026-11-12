//! `hp3-run quid-smoke <HP3 folder> [seed]`: a headless Quidditch match against Slytherin, flown by a scripted
//! Harry that steers at the Snitch and reaches for it when close, until the match ends or the tick cap.

use std::path::Path;
use std::process::ExitCode;

use glam::Vec3;

use hp3_package::PackageSet;
use hp3_script::ClassDb;
use hp3_world::quidditch::flight::{BroomInput, BroomState};
use hp3_world::quidditch::glue;
use hp3_world::quidditch::rules::{MatchPhase, Outcome, Score};
use hp3_world::quidditch::snitch::SnitchTuning;

pub const TICK_CAP: u32 = 20000;
/// The most goals a side may put away in one smoke match (about 1600 ticks, 27 s) before it counts as the
/// goal-fest the Keepers are there to prevent: single digits.
pub const MAX_GOALS: u32 = 9;
const MAP: &str = "HP3_Quidditch";

pub struct SmokeResult {
    pub outcome: Option<Outcome>,
    pub score: (u32, u32),
    /// Quaffle goals each side put away, counted as the score moved: the one thing that says the team AI
    /// played rather than only flew about. The home side's score also carries the 150 for the snitch, so a
    /// goal is a rise of exactly `Score::GOAL`.
    pub goals: (u32, u32),
    pub ticks: u32,
    pub errors: usize,
}

/// The steering that points `broom` at `target`: the engine's own scripted pilot (`flight::look_at`), which is
/// also what the `HP3_QUID_RING_AUTOFLY` chase of the ring trail flies.
pub fn steer(broom: &BroomState, target: Vec3, boost: bool) -> BroomInput {
    hp3_world::quidditch::flight::look_at(broom, target, boost)
}

/// Whether the scripted pilot reaches for the snitch: inside twice the catch radius.
pub fn wants_catch(broom: &BroomState, snitch: Vec3) -> bool {
    broom.pos.distance(snitch) <= 2.0 * SnitchTuning::default().catch_radius
}

pub fn play(root: &Path, seed: u64) -> Result<SmokeResult, Box<dyn std::error::Error>> {
    let set = PackageSet::open(root)?;
    let mut db = ClassDb::new(&set);
    db.load_all_classes();
    let mut vm = hp3_script::Vm::new(db);
    let pkg = set.get(MAP)?;
    hp3_world::load(&mut vm, &pkg, hp3_core::Config::load(root))?;
    glue::set_seed_override(Some(seed));
    // The easy Snitch (no ring run, caught from anywhere once the bar is full): this smoke run checks that a match is played,
    // scored and ended cleanly, and a scripted pilot cannot out-fly the Snitch under the game's own tuning (Harry's base speed
    // is below the Snitch's), which is for a player to do.
    glue::set_easy_override(Some(true));
    let started = glue::start(&mut vm, "Slytherin");
    glue::set_easy_override(None);
    glue::set_seed_override(None);
    started?;
    let mut ticks = 0;
    let mut outcome = None;
    let mut goals = (0u32, 0u32);
    let mut was = (0u32, 0u32);
    while ticks < TICK_CAP {
        // Steer from the state the last tick left. **The boost is held down** while the Snitch is further off
        // than the catch radius: a broom cruises at 450 and the Snitch runs at up to 875 with Harry on it, so
        // a pilot that never boosts never closes at all (it is the boost's 900 that beats the Snitch, barely).
        // A scripted input is fed straight in rather than through the player's boost *toggle*, so holding it
        // every frame is simply the boost on.
        // The Snitch is only caught after a run of its flight rings (`quidditch::trail`) where that rule is
        // on, so the pilot flies the next ring while there is one and the Snitch itself when there is not --
        // which is how a player plays it, and the only way a scripted match can end in a catch at all.
        let plan = hp3_world::world(&mut vm).quidditch.as_ref().map(|m| {
            let target = m.rings.trail.next_ring().map(|r| r.center).unwrap_or_else(|| m.snitch.pos());
            let boost = m.broom.pos.distance(m.snitch.pos()) > SnitchTuning::default().catch_radius;
            (steer(&m.broom, target, boost), wants_catch(&m.broom, m.snitch.pos()))
        });
        if let Some((input, catch)) = plan {
            glue::feed_scripted(&mut vm, input, catch);
        }
        hp3_world::tick(&mut vm, 1.0 / 60.0);
        ticks += 1;
        if let Some(m) = hp3_world::world(&mut vm).quidditch.as_ref() {
            let now = (m.rules.score().home, m.rules.score().away);
            goals.0 += u32::from(now.0 == was.0 + Score::GOAL);
            goals.1 += u32::from(now.1 == was.1 + Score::GOAL);
            was = now;
            if let MatchPhase::Ended(o) = m.rules.phase() {
                outcome = Some(o);
                break;
            }
        }
    }
    let w = hp3_world::world(&mut vm);
    let score = w.quidditch.as_ref().map(|m| (m.rules.score().home, m.rules.score().away)).unwrap_or((0, 0));
    let errors = w.errors.len();
    for e in w.errors.iter().take(10) {
        println!("script error: {e}");
    }
    Ok(SmokeResult { outcome, score, goals, ticks, errors })
}

pub fn run(root: &Path, seed: u64) -> Result<ExitCode, Box<dyn std::error::Error>> {
    let r = play(root, seed)?;
    println!(
        "quid-smoke seed {seed}: outcome {:?}, score {}-{} ({} Gryffindor goals, {} Slytherin goals), {} ticks, {} script errors",
        r.outcome, r.score.0, r.score.1, r.goals.0, r.goals.1, r.ticks, r.errors
    );
    // The Keepers keep now, so one side can be held scoreless over a single short match: what one match has
    // to show is that the Quaffle was played at all and that it was not a goal-fest. That both sides score is
    // asserted over seeds 1 to 5 by the test below.
    let played = r.goals.0 + r.goals.1 > 0 && r.goals.0 <= MAX_GOALS && r.goals.1 <= MAX_GOALS;
    if !played {
        println!("quid-smoke seed {seed}: a match needs at least one goal and at most {MAX_GOALS} a side, got {:?}", r.goals);
    }
    Ok(if r.outcome.is_some() && r.errors == 0 && played { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(pos: Vec3, yaw: f32) -> BroomState {
        BroomState::new(pos, Vec3::ZERO, yaw, 0.0)
    }

    #[test]
    fn steer_turns_toward_the_target_the_short_way() {
        let left = steer(&at(Vec3::ZERO, 0.0), Vec3::new(0.0, 1000.0, 0.0), false);
        assert!(left.look_yaw > 0.1, "{left:?}");
        let right = steer(&at(Vec3::ZERO, 0.0), Vec3::new(0.0, -1000.0, 0.0), false);
        assert!(right.look_yaw < -0.1, "{right:?}");
        // Across the +-pi seam: heading 3.0, target just past pi (a small left turn, not a long right one).
        let seam = steer(&at(Vec3::ZERO, 3.0), Vec3::new(-1000.0, -140.0, 0.0), false);
        assert!(seam.look_yaw > 0.0, "{seam:?}");
        let up = steer(&at(Vec3::ZERO, 0.0), Vec3::new(1000.0, 0.0, 800.0), false);
        assert!(up.look_pitch > 0.0 && up.look_yaw.abs() < 1e-3, "{up:?}");
        // The look is only ever nudged, never thrown: a target straight behind is a turn over many frames.
        let behind = steer(&at(Vec3::ZERO, 0.0), Vec3::new(-1000.0, 10.0, 0.0), false);
        assert!(behind.look_yaw <= 0.25 + 1e-6, "{behind:?}");
    }

    #[test]
    fn catch_is_asked_for_inside_twice_the_radius_only() {
        let r = SnitchTuning::default().catch_radius;
        let b = at(Vec3::ZERO, 0.0);
        assert!(wants_catch(&b, Vec3::new(2.0 * r - 1.0, 0.0, 0.0)));
        assert!(!wants_catch(&b, Vec3::new(2.0 * r + 1.0, 0.0, 0.0)));
    }

    /// A match plays itself out, and **both teams score**: the opposing side's Chasers have to take the
    /// Quaffle the length of the pitch and put it through a hoop, which is the only proof from here that the
    /// team AI plays rather than only flies about. A match lasts about 1600 ticks (27 s, the snitch's three
    /// tries running out while the scripted Harry chases it). The Keepers save shots now, so that is a
    /// handful of goals and not a dozen a side: every match must have a goal and none may be a goal-fest, and
    /// each side must score in most of the five (a Keeper may hold one side out of a single short match).
    #[test]
    fn smoke_match_ends_within_20000_ticks_for_seeds_1_to_5() {
        let root = std::env::var("HP3_DATA").unwrap_or_else(|_| r"C:\Users\Ihor\hpdroid\spike-hp3".into());
        if !Path::new(&root).is_dir() {
            eprintln!("skipping the smoke match: retail data not found at {root} (set HP3_DATA)");
            return;
        }
        let mut scored_in = (0, 0);
        for seed in 1..=5 {
            let r = play(Path::new(&root), seed).expect("smoke match runs");
            println!("seed {seed}: {:?} {}-{}, goals {:?}, {} ticks", r.outcome, r.score.0, r.score.1, r.goals, r.ticks);
            assert!(r.outcome.is_some(), "seed {seed}: no result in {} ticks", r.ticks);
            assert_eq!(r.errors, 0, "seed {seed}: script errors");
            assert!(r.goals.0 + r.goals.1 > 0, "seed {seed}: nobody scored, the Quaffle was not played");
            assert!(r.goals.0 <= MAX_GOALS && r.goals.1 <= MAX_GOALS, "seed {seed}: a goal-fest ({:?}), the Keepers are not keeping", r.goals);
            scored_in.0 += u32::from(r.goals.0 > 0);
            scored_in.1 += u32::from(r.goals.1 > 0);
        }
        assert!(scored_in.0 >= 3, "Gryffindor scored in only {} of 5 matches", scored_in.0);
        assert!(scored_in.1 >= 3, "Slytherin scored in only {} of 5 matches", scored_in.1);
    }

    /// The ways out of a match that are not its own end (named `quid_...` so `cargo test quid` runs it): no
    /// save is taken while one runs, a level switch asked for from the pause menu ends it instead of being
    /// refused, and leaving the level by any other road stops it first, with the player back on the pawn he
    /// walked in as. Nothing is written anywhere: the save is refused before it touches the disk.
    #[test]
    fn quid_match_is_left_cleanly_by_menu_and_by_travel() {
        use hp3_script::Value;
        let root = std::env::var("HP3_DATA").unwrap_or_else(|_| r"C:\Users\Ihor\hpdroid\spike-hp3".into());
        if !Path::new(&root).is_dir() {
            eprintln!("skipping: retail data not found at {root} (set HP3_DATA)");
            return;
        }
        let set = PackageSet::open(Path::new(&root)).expect("data opens");
        let mut db = ClassDb::new(&set);
        db.load_all_classes();
        let mut vm = hp3_script::Vm::new(db);
        let pkg = set.get(MAP).expect("the Quidditch map");
        hp3_world::load(&mut vm, &pkg, hp3_core::Config::load(Path::new(&root))).expect("the map loads");
        let pc = hp3_world::world(&mut vm).player.expect("a player");
        let walked_in_as = hp3_world::get(&mut vm, pc, "Pawn").as_obj();
        let playing = |vm: &mut hp3_script::Vm<'_>| hp3_world::world(vm).quidditch.is_some();

        // 1. No save while a match runs.
        glue::start(&mut vm, "Slytherin").expect("a match starts");
        for _ in 0..30 {
            hp3_world::tick(&mut vm, 1.0 / 60.0);
        }
        assert!(playing(&mut vm));
        assert_ne!(hp3_world::get(&mut vm, pc, "Pawn").as_obj(), walked_in_as, "the player flies the broom pawn");
        assert!(!hp3_world::save::write(&mut vm, 9999), "a save was taken in the middle of a match");

        // 2. The level's own scripts cannot switch the level under a match...
        let li = hp3_world::world(&mut vm).level_info.expect("a LevelInfo");
        hp3_world::set(&mut vm, li, "NextURL", Value::Str("HP3_GroundsDada".into()));
        hp3_world::tick(&mut vm, 1.0 / 60.0);
        assert!(playing(&mut vm), "a level script ended the match");
        assert!(hp3_world::take_travel(&mut vm).is_none());
        // ...but the player's own "leave" from the pause menu ends the match and goes.
        hp3_world::set(&mut vm, li, "Pauser", Value::Object(Some(pc)));
        hp3_world::set(&mut vm, li, "NextURL", Value::Str("HP_Preamble".into()));
        hp3_world::set(&mut vm, li, "NextSwitchCountdown", Value::Float(0.0));
        hp3_world::tick(&mut vm, 1.0 / 60.0);
        assert!(!playing(&mut vm), "the pause menu's leave was refused: the player is stuck in the match");
        assert_eq!(hp3_world::get(&mut vm, pc, "Pawn").as_obj(), walked_in_as, "the player did not get his pawn back");
        assert_eq!(hp3_world::take_travel(&mut vm).as_deref(), Some("HP_Preamble"));
        hp3_world::set(&mut vm, li, "Pauser", Value::Object(None));

        // 3. Any other road out of the level (the level list, a quick load) stops the match first.
        glue::start(&mut vm, "Slytherin").expect("a second match starts");
        for _ in 0..30 {
            hp3_world::tick(&mut vm, 1.0 / 60.0);
        }
        assert!(playing(&mut vm));
        hp3_world::travel(&mut vm, MAP).expect("the level is loaded again");
        assert!(!playing(&mut vm), "a match survived leaving its level");
        let pc = hp3_world::world(&mut vm).player.expect("a player after the travel");
        let pawn = hp3_world::get(&mut vm, pc, "Pawn").as_obj().expect("a pawn after the travel");
        let class = hp3_script::natives::object_path(&mut vm, pawn);
        assert!(!class.contains("HarryQuid"), "the player arrived as the broom pawn: {class}");
        assert!(!vm.db.objects.get(pawn).deleted, "the player holds a deleted pawn");
        assert_eq!(hp3_world::world(&mut vm).errors.len(), 0, "{:?}", hp3_world::world(&mut vm).errors);
    }
}
