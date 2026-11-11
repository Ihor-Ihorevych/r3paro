//! Quidditch match logic. Every module but `glue`, `presentation`, `ball` and `stage` is pure Rust with no
//! script VM dependency; `glue` is where a match meets the level that is loaded, `presentation` (called only
//! from `glue` and `canvas::render`) is what that match looks, sounds and reads like, `ball` (called only
//! from `glue`) is the Quaffle's and the hoop rings' actors, and `stage` (called only from `glue::start`)
//! takes the level's own cutscene and cast out of the way of the match.

pub mod arena;
pub mod ball;
pub mod bludger;
pub mod camera;
pub mod commentary;
pub mod flight;
pub mod glue;
pub mod goals;
pub mod hud;
pub mod input;
pub mod lod;
pub mod phone;
pub mod presentation;
pub mod quaffle;
mod rng;
pub mod rules;
pub mod snitch;
pub mod stage;
pub mod touch;
