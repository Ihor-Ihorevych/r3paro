//! The phone's broom controls, as pure as the desktop's: one steering stick of one frame in, a `BroomInput` out.
//!
//! The scheme is the one the touch overlay already flies HP1's broom with (see Flipendo's
//! `docs/android.md`, "Flying the broom"): a stick that only steers and presses nothing. The overlay's JUMP
//! button used to be the boost; there is no boost any more (speed comes from flight rings), so a tap of it now
//! reaches for the Snitch (`phone::feed_touch`).
//!
//! What the stick means is the broom's steering model (`flight`): a **look rate**. A thumb held over swings
//! where the rider is looking at `BroomTuning::yaw_rate` / `pitch_rate`, and the broom turns into that look --
//! the same thing the desktop's arrow keys do, so the phone needs no mouse.

use glam::Vec2;

use super::flight::BroomInput;

/// How far from the middle of the stick counts as nothing: a thumb never sits still.
const DEADZONE: f32 = 0.12;

/// This frame's steering from the touch layout. `stick` is the knob's deflection, x to the right and y
/// upwards, each -1..=1 (`None`: no finger on the stick, so the broom holds its course).
pub fn touch_input(stick: Option<Vec2>) -> BroomInput {
    let knob = stick.filter(|v| v.is_finite()).unwrap_or(Vec2::ZERO);
    let axis = |v: f32| {
        let m = v.abs();
        if m <= DEADZONE { 0.0 } else { v.signum() * ((m.min(1.0) - DEADZONE) / (1.0 - DEADZONE)) }
    };
    BroomInput {
        // The stick to the right turns right, which is where yaw falls, exactly as the mouse does on the
        // desktop. These are the look-rate axes, not the broom's own turn rate (`flight::step`).
        yaw: -axis(knob.x),
        pitch: axis(knob.y),
        ..BroomInput::default()
    }
}

/// The steering stick of the touch layout: the finger that put it down owns it until it is lifted, and every
/// other finger on the screen (the JUMP button's own, a stray palm) leaves it alone.
///
/// Positions are the screen's, in pixels, y downwards as a touch screen reports them; `offset` hands back the
/// deflection `touch_input` wants, y upwards.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stick {
    owner: Option<u64>,
    center: Vec2,
    knob: Vec2,
    radius: f32,
}

impl Stick {
    /// A stick whose knob reaches `radius` pixels from where the finger went down.
    pub fn new(radius: f32) -> Self {
        Stick { owner: None, center: Vec2::ZERO, knob: Vec2::ZERO, radius: radius.max(1.0) }
    }

    /// A finger went down at `at`. True when the stick took it; false when another finger already has it.
    pub fn press(&mut self, finger: u64, at: Vec2) -> bool {
        if self.owner.is_some() {
            return false;
        }
        self.owner = Some(finger);
        self.center = at;
        self.knob = Vec2::ZERO;
        true
    }

    /// A finger moved. Only the stick's own finger steers.
    pub fn drag(&mut self, finger: u64, at: Vec2) {
        if self.owner != Some(finger) {
            return;
        }
        let d = at - self.center;
        self.knob = if d.length() > self.radius { d * (self.radius / d.length()) } else { d };
    }

    /// A finger was lifted. The stick springs back only when it was its own.
    pub fn release(&mut self, finger: u64) {
        if self.owner == Some(finger) {
            *self = Stick::new(self.radius);
        }
    }

    /// This frame's deflection for `touch_input`, y upwards; `None` while no finger holds the stick.
    pub fn offset(&self) -> Option<Vec2> {
        self.owner.map(|_| Vec2::new(self.knob.x, -self.knob.y) / self.radius)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_finger_holds_course() {
        assert_eq!(touch_input(None), BroomInput::default());
        // A stick that reports nonsense (a lost finger) steers nothing either.
        assert_eq!(touch_input(Some(Vec2::new(f32::NAN, 1.0))).pitch, 0.0);
    }

    #[test]
    fn two_fingers_use_the_stick_finger_only() {
        let mut stick = Stick::new(100.0);
        assert!(stick.press(1, Vec2::new(200.0, 600.0)));
        stick.drag(1, Vec2::new(250.0, 600.0));
        let steering = stick.offset().expect("the first finger holds the stick");
        assert!(steering.x > 0.0, "{steering:?}");
        // A second finger (the JUMP button's) goes down and wanders: the stick does not take it.
        assert!(!stick.press(2, Vec2::new(900.0, 600.0)));
        stick.drag(2, Vec2::new(900.0, 300.0));
        assert_eq!(stick.offset(), Some(steering), "only the stick's own finger steers");
        // Lifting it leaves the stick where the first finger holds it.
        stick.release(2);
        assert_eq!(stick.offset(), Some(steering));
        // The stick's own finger steers to the end and springs back when it goes.
        stick.drag(1, Vec2::new(200.0, 500.0));
        let up = stick.offset().expect("still held");
        assert!(up.y > 0.0 && up.x == 0.0, "{up:?}, a finger pushed up the screen pulls the nose up");
        stick.release(1);
        assert_eq!(stick.offset(), None);
        // The knob never leaves its ring, however far the finger goes.
        assert!(stick.press(1, Vec2::ZERO));
        stick.drag(1, Vec2::new(1000.0, 0.0));
        assert_eq!(stick.offset(), Some(Vec2::X));
        assert_eq!(touch_input(stick.offset()).yaw, -1.0, "full right turns right");
    }
}
