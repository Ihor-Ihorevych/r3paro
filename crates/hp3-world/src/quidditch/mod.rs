//! Quidditch match logic. Every module but `glue` is pure Rust with no script VM dependency; `glue` is where
//! a match meets the level that is loaded.

pub mod arena;
pub mod bludger;
pub mod camera;
pub mod flight;
pub mod glue;
pub mod input;
mod rng;
pub mod rules;
pub mod snitch;
