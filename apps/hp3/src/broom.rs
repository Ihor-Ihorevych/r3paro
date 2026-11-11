//! The phone's broom controls: what Flipendo's touch overlay reports while a Quidditch match runs, and what
//! the overlay asks the game about to show its broom layout.
//!
//! The layout itself is the overlay's (`FlipendoActivity.Overlay`, Flipendo's own Java), and it is the one it
//! already flies HP1's broom with: a steering stick that presses nothing, a boost button that toggles and a
//! brake that is held (`docs/android.md`, "Flying the broom"). `nativeOnBroom` is what turns it on, and this
//! library now answers it with "a match is running" instead of a flat no.
//!
//! The overlay reports the stick and the two buttons through the natives below; `take` hands one frame of it
//! to `hp3_world::quidditch::phone::feed_touch`, which is the same `pending_input` path the desktop's keys
//! go down. Until an overlay is built that calls them, the broom layout's stick still steers through the
//! relative mouse motion it already sends and its JUMP button still toggles the boost (`RightMouse`), so the
//! match is playable either way; `TOUCHED` is what keeps the two from fighting over a frame.
//!
//! Nothing here does anything while no match is running: `set_on_broom(false)` forgets the lot.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};

use glam::Vec2;

/// A match is running: the overlay shows the broom layout (`nativeOnBroom`).
static ON_BROOM: AtomicBool = AtomicBool::new(false);
/// The steering stick's deflection in -1..1, x then y (y upwards), as two f32 in one word.
static STICK: AtomicU64 = AtomicU64::new(0);
/// A finger holds the stick (without one the broom holds its course).
static STICK_DOWN: AtomicBool = AtomicBool::new(false);
/// Taps of the boost button no frame has taken yet.
static BOOST_TAPS: AtomicU32 = AtomicU32::new(0);
/// The brake button held now.
static BRAKE: AtomicBool = AtomicBool::new(false);
/// Whether a touch control has ever reported anything. Without one (every desktop, and a phone whose overlay
/// is older than these natives) the keys and mouse path is left exactly as it was.
static TOUCHED: AtomicBool = AtomicBool::new(false);

/// One frame of the touch layout.
pub struct Touch {
    /// The steering stick's deflection, `None` while no finger holds it.
    pub stick: Option<Vec2>,
    /// The boost button was tapped since the last frame (an edge: it toggles).
    pub boost_tap: bool,
    /// The brake button is held now.
    pub brake: bool,
}

/// Whether a match is running, which is what the overlay asks before showing the broom layout.
pub fn on_broom() -> bool {
    ON_BROOM.load(Relaxed)
}

/// A match started or ended. The end lets go of the stick and the brake and drops the taps nobody took, so
/// the next match starts from nothing.
pub fn set_on_broom(on: bool) {
    if !on && ON_BROOM.swap(false, Relaxed) {
        STICK_DOWN.store(false, Relaxed);
        BRAKE.store(false, Relaxed);
        BOOST_TAPS.store(0, Relaxed);
        return;
    }
    ON_BROOM.store(on, Relaxed);
}

/// This frame's steering, or `None` when no touch control has ever spoken (the desktop, where the keys and
/// the mouse fly the broom). The boost taps are consumed: each one flips the toggle exactly once.
pub fn take() -> Option<Touch> {
    if !TOUCHED.load(Relaxed) {
        return None;
    }
    let packed = STICK.load(Relaxed);
    let stick = STICK_DOWN
        .load(Relaxed)
        .then(|| Vec2::new(f32::from_bits(packed as u32), f32::from_bits((packed >> 32) as u32)));
    Some(Touch { stick, boost_tap: BOOST_TAPS.swap(0, Relaxed) > 0, brake: BRAKE.load(Relaxed) })
}

/// Where the steering stick stands now: `x` to the right and `y` upwards, each -1..1, and whether a finger is
/// on it at all.
pub fn set_stick(x: f32, y: f32, down: bool) {
    TOUCHED.store(true, Relaxed);
    let clamp = |v: f32| if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 };
    let packed = clamp(x).to_bits() as u64 | ((clamp(y).to_bits() as u64) << 32);
    STICK.store(packed, Relaxed);
    STICK_DOWN.store(down, Relaxed);
}

/// The boost button was tapped: the toggle flips once, on whichever frame takes it.
pub fn boost_tap() {
    TOUCHED.store(true, Relaxed);
    BOOST_TAPS.fetch_add(1, Relaxed);
}

/// The brake button went down or came up.
pub fn set_brake(down: bool) {
    TOUCHED.store(true, Relaxed);
    BRAKE.store(down, Relaxed);
}

/// The JNI natives the overlay's broom layout calls. Declared in `io.github.flipendo.spike.FlipendoActivity`
/// beside the ones in `android`; an overlay that does not declare them simply never calls them.
#[cfg(target_os = "android")]
mod jni {
    use std::ffi::c_void;

    type Env = *mut *const *const c_void;

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_io_github_flipendo_spike_FlipendoActivity_nativeSetBroomStick(_env: Env, _class: *mut c_void, x: f32, y: f32, down: u8) {
        super::set_stick(x, y, down != 0);
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_io_github_flipendo_spike_FlipendoActivity_nativeBroomBoostTap(_env: Env, _class: *mut c_void) {
        super::boost_tap();
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_io_github_flipendo_spike_FlipendoActivity_nativeSetBroomBrake(_env: Env, _class: *mut c_void, down: u8) {
        super::set_brake(down != 0);
    }
}
