//! `hp3-run quid-smoke <HP3 folder> [seed]`: a headless Quidditch match against Slytherin, flown by a scripted
//! Harry that steers at the Snitch and reaches for it when close, until the match ends or the tick cap.

use std::path::Path;
use std::process::ExitCode;

use glam::Vec3;

use hp3_package::PackageSet;
use hp3_script::ClassDb;
use hp3_world::quidditch::flight::{BroomInput, BroomState};
use hp3_world::quidditch::glue;
use hp3_world::quidditch::rules::{MatchPhase, Outcome};
use hp3_world::quidditch::snitch::SnitchTuning;

pub const TICK_CAP: u32 = 20000;
const MAP: &str = "HP3_Quidditch";

pub struct SmokeResult {
    pub outcome: Option<Outcome>,
    pub score: (u32, u32),
    pub ticks: u32,
    pub errors: usize,
}

/// The steering that points `broom` at `target`: yaw and pitch sticks proportional to the heading error.
pub fn steer(broom: &BroomState, target: Vec3, boost: bool) -> BroomInput {
    let d = target - broom.pos;
    let flat = d.truncate().length();
    let want_yaw = d.y.atan2(d.x);
    let want_pitch = d.z.atan2(flat.max(1e-3));
    let wrap = |a: f32| (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    BroomInput {
        yaw: (wrap(want_yaw - broom.yaw) * 3.0).clamp(-1.0, 1.0),
        pitch: ((want_pitch - broom.pitch) * 3.0).clamp(-1.0, 1.0),
        boost,
        brake: false,
    }
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
    let started = glue::start(&mut vm, "Slytherin");
    glue::set_seed_override(None);
    started?;
    let mut ticks = 0;
    let mut outcome = None;
    while ticks < TICK_CAP {
        // Steer from the state the last tick left; boost is a toggle in the real controls, so it is not used.
        let plan = hp3_world::world(&mut vm).quidditch.as_ref().map(|m| (steer(&m.broom, m.snitch.pos(), false), wants_catch(&m.broom, m.snitch.pos())));
        if let Some((input, catch)) = plan {
            glue::feed_scripted(&mut vm, input, catch);
        }
        hp3_world::tick(&mut vm, 1.0 / 60.0);
        ticks += 1;
        if let Some(m) = hp3_world::world(&mut vm).quidditch.as_ref()
            && let MatchPhase::Ended(o) = m.rules.phase()
        {
            outcome = Some(o);
            break;
        }
    }
    let w = hp3_world::world(&mut vm);
    let score = w.quidditch.as_ref().map(|m| (m.rules.score().home, m.rules.score().away)).unwrap_or((0, 0));
    let errors = w.errors.len();
    for e in w.errors.iter().take(10) {
        println!("script error: {e}");
    }
    Ok(SmokeResult { outcome, score, ticks, errors })
}

pub fn run(root: &Path, seed: u64) -> Result<ExitCode, Box<dyn std::error::Error>> {
    let r = play(root, seed)?;
    println!(
        "quid-smoke seed {seed}: outcome {:?}, score {}-{}, {} ticks, {} script errors",
        r.outcome, r.score.0, r.score.1, r.ticks, r.errors
    );
    Ok(if r.outcome.is_some() && r.errors == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(pos: Vec3, yaw: f32) -> BroomState {
        BroomState { pos, vel: Vec3::ZERO, yaw, pitch: 0.0 }
    }

    #[test]
    fn steer_turns_toward_the_target_the_short_way() {
        let left = steer(&at(Vec3::ZERO, 0.0), Vec3::new(0.0, 1000.0, 0.0), false);
        assert!(left.yaw > 0.5, "{left:?}");
        let right = steer(&at(Vec3::ZERO, 0.0), Vec3::new(0.0, -1000.0, 0.0), false);
        assert!(right.yaw < -0.5, "{right:?}");
        // Across the +-pi seam: heading 3.0, target just past pi (a small left turn, not a long right one).
        let seam = steer(&at(Vec3::ZERO, 3.0), Vec3::new(-1000.0, -140.0, 0.0), false);
        assert!(seam.yaw > 0.0, "{seam:?}");
        let up = steer(&at(Vec3::ZERO, 0.0), Vec3::new(1000.0, 0.0, 800.0), false);
        assert!(up.pitch > 0.0 && up.yaw.abs() < 1e-3, "{up:?}");
    }

    #[test]
    fn catch_is_asked_for_inside_twice_the_radius_only() {
        let r = SnitchTuning::default().catch_radius;
        let b = at(Vec3::ZERO, 0.0);
        assert!(wants_catch(&b, Vec3::new(2.0 * r - 1.0, 0.0, 0.0)));
        assert!(!wants_catch(&b, Vec3::new(2.0 * r + 1.0, 0.0, 0.0)));
    }

    #[test]
    fn smoke_match_ends_within_20000_ticks_for_seeds_1_to_5() {
        let root = std::env::var("HP3_DATA").unwrap_or_else(|_| r"C:\Users\Ihor\hpdroid\spike-hp3".into());
        if !Path::new(&root).is_dir() {
            eprintln!("skipping the smoke match: retail data not found at {root} (set HP3_DATA)");
            return;
        }
        for seed in 1..=5 {
            let r = play(Path::new(&root), seed).expect("smoke match runs");
            assert!(r.outcome.is_some(), "seed {seed}: no result in {} ticks", r.ticks);
            assert_eq!(r.errors, 0, "seed {seed}: script errors");
        }
    }
}
