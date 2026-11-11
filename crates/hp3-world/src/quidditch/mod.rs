//! Quidditch match logic. Every module but `glue` and `presentation` is pure Rust with no script VM
//! dependency; `glue` is where a match meets the level that is loaded, and `presentation` (called only from
//! `glue` and `canvas::render`) is what that match looks, sounds and reads like.

pub mod arena;
pub mod bludger;
pub mod camera;
pub mod commentary;
pub mod flight;
pub mod glue;
pub mod hud;
pub mod input;
pub mod presentation;
mod rng;
pub mod rules;
pub mod snitch;
