//! Graphics options the player can change while the game runs (the phone's graphics button, `gfx` commands): the characters'
//! shadow and its quality tier. Plain global switches the render passes read each frame; the first read takes its starting values
//! from the environment (`HP3_ACTOR_SHADOWS=0` turns the shadow off, `HP3_SHADOW_QUALITY=low|high` picks the tier).
//!
//! Two tiers: Low is the phone's single shadow map; High (the PC's default) nests several maps of growing reach around the player.

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Once;

static START: Once = Once::new();
static SHADOWS: AtomicBool = AtomicBool::new(true);
static HIGH: AtomicBool = AtomicBool::new(!cfg!(target_os = "android"));

fn start() {
    START.call_once(|| {
        if std::env::var("HP3_ACTOR_SHADOWS").is_ok_and(|v| v == "0") {
            SHADOWS.store(false, Relaxed);
        }
        match std::env::var("HP3_SHADOW_QUALITY").as_deref() {
            Ok("low") => HIGH.store(false, Relaxed),
            Ok("high") => HIGH.store(true, Relaxed),
            _ => {}
        }
    });
}

/// Whether the characters cast real shadows (otherwise only their blob).
pub fn actor_shadows() -> bool {
    start();
    SHADOWS.load(Relaxed)
}

pub fn set_actor_shadows(on: bool) {
    start();
    SHADOWS.store(on, Relaxed);
}

/// Whether the shadow takes the high tier (nested maps). The phone has no room for it: its shadow texture has one layer.
pub fn high_shadows() -> bool {
    start();
    HIGH.load(Relaxed) && !cfg!(target_os = "android")
}

pub fn set_high_shadows(on: bool) {
    start();
    HIGH.store(on, Relaxed);
}

/// How many nested maps the shadow uses now.
pub fn cascades() -> usize {
    if high_shadows() { crate::shadow::ACTOR_LAYERS as usize } else { 1 }
}

/// The options packed for the phone's dialog: bit 0 the characters' shadow (bits 1 and 2 belong to the Java side, bit 3 is the
/// shadow's high tier).
pub fn packed() -> i32 {
    actor_shadows() as i32 | (high_shadows() as i32) << 3
}
