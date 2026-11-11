//! Quidditch match logic. Every module but `glue`, `presentation` and `stage` is pure Rust with no script VM
//! dependency; `glue` is where a match meets the level that is loaded, `presentation` (called only from
//! `glue` and `canvas::render`) is what that match looks, sounds and reads like, and `stage` (called only
//! from `glue::start`) takes the level's own cutscene and cast out of the way of the match.

pub mod arena;
pub mod bludger;
pub mod camera;
pub mod commentary;
pub mod flight;
pub mod glue;
pub mod hud;
pub mod input;
pub mod lod;
pub mod phone;
pub mod presentation;
mod rng;
pub mod rules;
pub mod snitch;
pub mod stage;
pub mod touch;
