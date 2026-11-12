//! Graphics options the player can change while the game runs (the phone's graphics button, `gfx` commands): for now the
//! characters' shadow. A plain global switch the render passes read each frame; the first read takes its starting value from
//! the environment (`HP3_ACTOR_SHADOWS=0`).

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Once;

static START: Once = Once::new();
static SHADOWS: AtomicBool = AtomicBool::new(true);

fn start() {
    START.call_once(|| {
        if std::env::var("HP3_ACTOR_SHADOWS").is_ok_and(|v| v == "0") {
            SHADOWS.store(false, Relaxed);
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

/// The options packed for the phone's dialog: bit 0 the characters' shadow.
pub fn packed() -> i32 {
    actor_shadows() as i32
}
