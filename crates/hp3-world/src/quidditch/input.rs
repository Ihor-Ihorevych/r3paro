//! The desktop broom controls, as pure as the flight model: the broom keys and the mouse motion of one frame
//! in, a `BroomInput` out.
//!
//! The app's own key state is not pure (it lives in the VM's world, with every binding the ini names), so
//! `crate::input::apply` reads the few keys the broom cares about into a `KeyState` and `quidditch::glue`
//! turns it into steering here. The scheme is HP1's: the mouse or the arrow keys steer, boost is a toggle (a
//! tap turns it on, the next turns it off) and the brake is held and beats the boost.

use super::flight::BroomInput;

/// The broom keys of one frame. Everything but `boost` is "held now"; `boost` is a press *this frame*, an
/// edge, because the boost is a toggle.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct KeyState {
    /// `Left`: turn left, the way `BroomState.yaw` grows.
    pub left: bool,
    /// `Right`: turn right.
    pub right: bool,
    /// `Up`: nose up.
    pub up: bool,
    /// `Down`: nose down.
    pub down: bool,
    /// `RightMouse` or `A` pressed this frame (not held): each press flips the boost.
    pub boost: bool,
    /// `LeftMouse` or `Z` held.
    pub brake: bool,
}

/// Mouse pixels in one frame that make a full stick deflection.
const MOUSE_FULL: f32 = 12.0;

/// This frame's steering. `mouse` is the frame's motion in pixels as the app reports it (x to the right, y
/// downwards), `invert_pitch` is the game's own mouse-invert setting, and `boost_toggle` is the boost the
/// match remembers between frames: a press in `keys.boost` flips it, nothing else touches it.
pub fn desktop_input(keys: &KeyState, mouse: (f32, f32), invert_pitch: bool, boost_toggle: &mut bool) -> BroomInput {
    if keys.boost {
        *boost_toggle = !*boost_toggle;
    }
    let held = |on: bool| if on { 1.0 } else { 0.0 };
    let pixels = |v: f32| if v.is_finite() { v / MOUSE_FULL } else { 0.0 };
    // The mouse to the right turns right, which is where yaw falls; moving it up (a negative y, as the app
    // reports it) pulls the nose up, as the game's own mouse look does until the player inverts it.
    let yaw = (held(keys.left) - held(keys.right) - pixels(mouse.0)).clamp(-1.0, 1.0);
    let pitch = (held(keys.up) - held(keys.down) - pixels(mouse.1)).clamp(-1.0, 1.0);
    BroomInput {
        yaw,
        pitch: if invert_pitch { -pitch } else { pitch },
        boost: *boost_toggle,
        brake: keys.brake,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boost_toggles_on_one_tap_and_off_on_second() {
        let mut boost = false;
        let tap = KeyState { boost: true, ..KeyState::default() };
        let quiet = KeyState::default();
        assert!(desktop_input(&tap, (0.0, 0.0), false, &mut boost).boost, "one tap turns the boost on");
        // Holding the key down (no new press) keeps it on, it does not flicker.
        for _ in 0..10 {
            assert!(desktop_input(&quiet, (0.0, 0.0), false, &mut boost).boost);
        }
        assert!(!desktop_input(&tap, (0.0, 0.0), false, &mut boost).boost, "the second tap turns it off");
        for _ in 0..10 {
            assert!(!desktop_input(&quiet, (0.0, 0.0), false, &mut boost).boost);
        }
        assert!(desktop_input(&tap, (0.0, 0.0), false, &mut boost).boost);
    }

    #[test]
    fn brake_and_boost_together_report_brake() {
        let mut boost = false;
        let both = KeyState { boost: true, brake: true, ..KeyState::default() };
        let i = desktop_input(&both, (0.0, 0.0), false, &mut boost);
        assert!(i.brake, "the brake is held, so it is reported");
        // `flight::step` is what rules that the brake beats the boost, and it does.
        let t = super::super::flight::BroomTuning::default();
        let s = super::super::flight::BroomState { pos: glam::Vec3::ZERO, vel: glam::Vec3::X * t.speed, yaw: 0.0, pitch: 0.0 };
        let braked = super::super::flight::step(&s, &i, &t, 1.0 / 60.0);
        assert!(braked.vel.length() < t.speed, "the brake must win over the boost: {:?}", braked.vel);
    }

    #[test]
    fn invert_pitch_flips_pitch_sign() {
        let mut boost = false;
        let keys = KeyState { up: true, ..KeyState::default() };
        let plain = desktop_input(&keys, (0.0, -6.0), false, &mut boost);
        let inverted = desktop_input(&keys, (0.0, -6.0), true, &mut boost);
        assert!(plain.pitch > 0.0, "{}", plain.pitch);
        assert_eq!(inverted.pitch, -plain.pitch);
        // The mouse alone, both ways round: pushed forward (a negative y) the nose goes up unless inverted.
        let none = KeyState::default();
        let up = desktop_input(&none, (0.0, -12.0), false, &mut boost);
        let down = desktop_input(&none, (0.0, -12.0), true, &mut boost);
        assert_eq!((up.pitch, down.pitch), (1.0, -1.0));
        // Steering: the keys and the mouse both turn the broom the same way round.
        assert!(desktop_input(&KeyState { left: true, ..none }, (0.0, 0.0), false, &mut boost).yaw > 0.0);
        assert!(desktop_input(&none, (6.0, 0.0), false, &mut boost).yaw < 0.0, "the mouse to the right turns right");
        assert_eq!(desktop_input(&none, (f32::NAN, f32::INFINITY), false, &mut boost), BroomInput { yaw: 0.0, pitch: 0.0, boost: false, brake: false });
    }
}
