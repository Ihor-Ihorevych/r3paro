//! The desktop broom controls, as pure as the flight model: the broom keys and the mouse motion of one frame
//! in, a `BroomInput` out.
//!
//! The app's own key state is not pure (it lives in the VM's world, with every binding the ini names), so
//! `crate::input::apply` reads the few keys the broom cares about into a `KeyState` and `quidditch::glue`
//! turns it into steering here. The scheme is HP1's with AdamJD's movement mod on top: the mouse is free
//! look (its motion is an angular delta that moves where the rider looks, and the broom turns into it), the
//! arrow keys swing the same look at a constant rate, boost is a toggle (a tap turns it on, the next turns it
//! off) and the brake is held and beats the boost. The mod's "every turn brakes" is **not** reproduced: the
//! brake is the brake button and nothing else.

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
    /// `Space` or `Enter` held: reach out for the snitch while the catch window is open.
    pub catch: bool,
}

/// How far the look swings per mouse pixel (radians). It is a constant of ours, not the mod's: the mod's gain
/// is the player's own `MouseSensitivity`, which is in nobody's `.ini` here. 0.003 rad/px (0.17 deg) is what
/// the stick model it replaces felt like -- its 12 pixels for a full 2.0 rad/s deflection is 0.0028 rad per
/// pixel at 60 fps -- and it is **not** multiplied by `dt`: mouse pixels are already a per-frame quantity.
const MOUSE_GAIN: f32 = 0.003;

/// This frame's steering. `mouse` is the frame's motion in pixels as the app reports it (x to the right, y
/// downwards), `invert_pitch` is the game's own mouse-invert setting, and `boost_toggle` is the boost the
/// match remembers between frames: a press in `keys.boost` flips it, nothing else touches it.
pub fn desktop_input(keys: &KeyState, mouse: (f32, f32), invert_pitch: bool, boost_toggle: &mut bool) -> BroomInput {
    if keys.boost {
        *boost_toggle = !*boost_toggle;
    }
    let held = |on: bool| -> f32 { if on { 1.0 } else { 0.0 } };
    let pixels = |v: f32| if v.is_finite() { v * MOUSE_GAIN } else { 0.0 };
    // At the owner's word the yaw is the opposite way round to the game's own mouse look (the mouse to the
    // right turns the broom left: yaw rises) and the pitch is the game's own (moving the mouse up, a negative
    // y as the app reports it, pulls the nose up). The keys are the same two axes as a rate (left/up as
    // before), so flying with the keyboard alone still works.
    let yaw = (held(keys.left) - held(keys.right)).clamp(-1.0, 1.0);
    let pitch = (held(keys.up) - held(keys.down)).clamp(-1.0, 1.0);
    let look_yaw = pixels(mouse.0);
    let look_pitch = -pixels(mouse.1);
    BroomInput {
        yaw,
        pitch: if invert_pitch { -pitch } else { pitch },
        boost: *boost_toggle,
        brake: keys.brake,
        look_yaw,
        look_pitch: if invert_pitch { -look_pitch } else { look_pitch },
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
        let s = super::super::flight::BroomState::new(glam::Vec3::ZERO, glam::Vec3::X * t.speed, 0.0, 0.0);
        let braked = super::super::flight::step(&s, &i, &t, 1.0 / 60.0);
        assert!(braked.vel.length() < t.speed, "the brake must win over the boost: {:?}", braked.vel);
    }

    #[test]
    fn invert_pitch_flips_pitch_sign() {
        let mut boost = false;
        let keys = KeyState { up: true, ..KeyState::default() };
        let plain = desktop_input(&keys, (0.0, -6.0), false, &mut boost);
        let inverted = desktop_input(&keys, (0.0, -6.0), true, &mut boost);
        assert!(plain.pitch > 0.0 && plain.look_pitch > 0.0, "{plain:?}");
        assert_eq!(inverted.pitch, -plain.pitch);
        assert_eq!(inverted.look_pitch, -plain.look_pitch);
        // The mouse alone, both ways round: moved up (a negative y) the nose goes up unless the game's own
        // invert setting is on.
        let none = KeyState::default();
        let up = desktop_input(&none, (0.0, -10.0), false, &mut boost);
        let down = desktop_input(&none, (0.0, -10.0), true, &mut boost);
        assert!((up.look_pitch - 10.0 * MOUSE_GAIN).abs() < 1e-6, "{up:?}");
        assert_eq!(down.look_pitch, -up.look_pitch);
        assert_eq!((up.pitch, down.pitch), (0.0, 0.0), "the mouse is a look, not a stick");
        // Steering: the keys and the mouse both turn the broom the same way round, the keys as a rate and the
        // mouse as this frame's angle.
        assert!(desktop_input(&KeyState { left: true, ..none }, (0.0, 0.0), false, &mut boost).yaw > 0.0);
        let right = desktop_input(&none, (10.0, 0.0), false, &mut boost);
        assert!(right.look_yaw > 0.0 && right.yaw == 0.0, "the mouse to the right turns left (inverted): {right:?}");
        assert!((right.look_yaw - 10.0 * MOUSE_GAIN).abs() < 1e-6, "{right:?}");
        // Twice the motion is twice the angle: nothing saturates, and a flick is a flick.
        let far = desktop_input(&none, (20.0, 0.0), false, &mut boost);
        assert!((far.look_yaw - 2.0 * right.look_yaw).abs() < 1e-6, "{far:?}");
        // Moved down (a positive y) the nose dives.
        assert!(desktop_input(&none, (0.0, 10.0), false, &mut boost).look_pitch < 0.0);
        assert_eq!(desktop_input(&none, (f32::NAN, f32::INFINITY), false, &mut boost), BroomInput::default());
    }
}
