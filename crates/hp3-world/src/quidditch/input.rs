//! The desktop broom controls, as pure as the flight model: the broom keys and the mouse motion of one frame
//! in, a `BroomInput` out.
//!
//! The app's own key state is not pure (it lives in the VM's world, with every binding the ini names), so
//! `crate::input::apply` reads the few keys the broom cares about into a `KeyState` and `quidditch::glue`
//! turns it into steering here. The scheme is HP1's with AdamJD's movement mod on top: the mouse is free
//! look (its motion is an angular delta that moves where the rider looks, and the broom turns into it), the
//! arrow keys swing the same look at a constant rate, and a tap (right mouse, `A`) or `Space` reaches for the
//! snitch. There is no boost and no brake: Harry's speed is his momentum (`quidditch::momentum`), charged by
//! flight rings.

use super::flight::BroomInput;

/// The broom keys of one frame. Everything but `reach` is "held now"; `reach` is a tap *this frame*, an edge.
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
    /// `RightMouse` or `A` pressed this frame, or the phone's JUMP button changing (not held): a tap, which
    /// reaches out for the snitch for a moment. There is no boost and no brake: speed comes from rings.
    pub reach: bool,
    /// `Space` or `Enter` held: reach out for the snitch while the catch window is open.
    pub catch: bool,
}

/// How far the look swings per mouse pixel (radians). It is a constant of ours, not the mod's: the mod's gain
/// is the player's own `MouseSensitivity`, which is in nobody's `.ini` here. 0.003 rad/px (0.17 deg) is what
/// the stick model it replaces felt like -- its 12 pixels for a full 2.0 rad/s deflection is 0.0028 rad per
/// pixel at 60 fps -- and it is **not** multiplied by `dt`: mouse pixels are already a per-frame quantity.
const MOUSE_GAIN: f32 = 0.003;

/// This frame's steering. `mouse` is the frame's motion in pixels as the app reports it (x to the right, y
/// downwards) and `invert_pitch` is the game's own mouse-invert setting.
pub fn desktop_input(keys: &KeyState, mouse: (f32, f32), invert_pitch: bool) -> BroomInput {
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
        look_yaw,
        look_pitch: if invert_pitch { -look_pitch } else { look_pitch },
        ..BroomInput::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_is_no_boost_or_brake() {
        let i = desktop_input(&KeyState { reach: true, ..KeyState::default() }, (0.0, 0.0), false);
        assert!(!i.boost && !i.brake, "{i:?}");
    }

    #[test]
    fn invert_pitch_flips_pitch_sign() {
        let keys = KeyState { up: true, ..KeyState::default() };
        let plain = desktop_input(&keys, (0.0, -6.0), false);
        let inverted = desktop_input(&keys, (0.0, -6.0), true);
        assert!(plain.pitch > 0.0 && plain.look_pitch > 0.0, "{plain:?}");
        assert_eq!(inverted.pitch, -plain.pitch);
        assert_eq!(inverted.look_pitch, -plain.look_pitch);
        // The mouse alone, both ways round: moved up (a negative y) the nose goes up unless the game's own
        // invert setting is on.
        let none = KeyState::default();
        let up = desktop_input(&none, (0.0, -10.0), false);
        let down = desktop_input(&none, (0.0, -10.0), true);
        assert!((up.look_pitch - 10.0 * MOUSE_GAIN).abs() < 1e-6, "{up:?}");
        assert_eq!(down.look_pitch, -up.look_pitch);
        assert_eq!((up.pitch, down.pitch), (0.0, 0.0), "the mouse is a look, not a stick");
        // Steering: the keys and the mouse both turn the broom the same way round, the keys as a rate and the
        // mouse as this frame's angle.
        assert!(desktop_input(&KeyState { left: true, ..none }, (0.0, 0.0), false).yaw > 0.0);
        let right = desktop_input(&none, (10.0, 0.0), false);
        assert!(right.look_yaw > 0.0 && right.yaw == 0.0, "the mouse to the right turns left (inverted): {right:?}");
        assert!((right.look_yaw - 10.0 * MOUSE_GAIN).abs() < 1e-6, "{right:?}");
        // Twice the motion is twice the angle: nothing saturates, and a flick is a flick.
        let far = desktop_input(&none, (20.0, 0.0), false);
        assert!((far.look_yaw - 2.0 * right.look_yaw).abs() < 1e-6, "{far:?}");
        // Moved down (a positive y) the nose dives.
        assert!(desktop_input(&none, (0.0, 10.0), false).look_pitch < 0.0);
        assert_eq!(desktop_input(&none, (f32::NAN, f32::INFINITY), false), BroomInput::default());
    }
}
