//! The phone's broom controls, as pure as the desktop's: one steering stick and two buttons of one frame in,
//! a `BroomInput` out.
//!
//! The scheme is the one the touch overlay already flies HP1's broom with (see Flipendo's
//! `docs/android.md`, "Flying the broom"): a stick that only steers and presses nothing, a boost button that
//! toggles (one tap keeps it on, so the thumb is free to steer) and a brake that is held. The brake beats the
//! boost, which is `flight::step`'s rule and not this module's.
//!
//! What the stick means is the broom's new steering model (`flight`): a **look rate**. A thumb held over
//! swings where the rider is looking at `BroomTuning::yaw_rate` / `pitch_rate`, and the broom turns into that
//! look -- the same thing the desktop's arrow keys do, so the phone needs no mouse.
//!
//! The boost toggle lives in the match (`QuidditchMatch::boost`), not here, so a pause that stops the tick
//! cannot reset it.

use glam::Vec2;

use super::flight::BroomInput;

/// How far from the middle of the stick counts as nothing: a thumb never sits still.
const DEADZONE: f32 = 0.12;

/// This frame's steering from the touch layout. `stick` is the knob's deflection, x to the right and y
/// upwards, each -1..=1 (`None`: no finger on the stick, so the broom holds its course). `boost_tap` is a tap
/// of the boost button *this frame* (an edge, as on the desktop), `brake_held` is the brake button held now,
/// and `boost` is the toggle the match remembers between frames: only a tap flips it.
pub fn touch_input(stick: Option<Vec2>, boost_tap: bool, brake_held: bool, boost: &mut bool) -> BroomInput {
    if boost_tap {
        *boost = !*boost;
    }
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
        boost: *boost,
        brake: brake_held,
        ..BroomInput::default()
    }
}

/// The steering stick of the touch layout: the finger that put it down owns it until it is lifted, and every
/// other finger on the screen (the boost and brake buttons' own, a stray palm) leaves it alone.
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
        // The boost is on and the thumb comes off the stick: the broom neither turns nor loses the boost.
        let mut boost = true;
        for _ in 0..10 {
            let i = touch_input(None, false, false, &mut boost);
            assert_eq!(i, BroomInput { boost: true, ..BroomInput::default() });
        }
        // And with the boost off it stays off.
        let mut boost = false;
        assert_eq!(touch_input(None, false, false, &mut boost), BroomInput::default());
        // The brake is the button's own business and is reported even with no finger steering.
        assert!(touch_input(None, false, true, &mut boost).brake);
        // A stick that reports nonsense (a lost finger) steers nothing either.
        assert_eq!(touch_input(Some(Vec2::new(f32::NAN, 1.0)), false, false, &mut boost).pitch, 0.0);
    }

    #[test]
    fn two_fingers_use_the_stick_finger_only() {
        let mut stick = Stick::new(100.0);
        assert!(stick.press(1, Vec2::new(200.0, 600.0)));
        stick.drag(1, Vec2::new(250.0, 600.0));
        let steering = stick.offset().expect("the first finger holds the stick");
        assert!(steering.x > 0.0, "{steering:?}");
        // A second finger (the boost button's) goes down and wanders: the stick does not take it.
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
        let mut boost = false;
        assert_eq!(touch_input(stick.offset(), false, false, &mut boost).yaw, -1.0, "full right turns right");
    }

    #[test]
    fn boost_survives_pause_resume() {
        // The toggle is the caller's (the match's) and nothing here keeps it: a tap turns it on...
        let mut boost = false;
        assert!(touch_input(None, true, false, &mut boost).boost);
        // ...the game is paused, so no frame is stepped at all...
        for _ in 0..100 {
            // nothing happens: no input is fed while the tick is stopped
        }
        // ...and when it resumes the boost is still on, with no finger anywhere.
        assert!(boost, "the pause must not touch the toggle");
        assert!(touch_input(None, false, false, &mut boost).boost);
        assert!(touch_input(Some(Vec2::ZERO), false, false, &mut boost).boost);
        // The next tap turns it off, and holding the button down (no new tap) does not flicker it.
        assert!(!touch_input(None, true, false, &mut boost).boost);
        for _ in 0..10 {
            assert!(!touch_input(None, false, false, &mut boost).boost);
        }
        // The brake is reported with the boost; `flight::step` is what makes it win.
        let both = touch_input(None, true, true, &mut boost);
        assert_eq!((both.boost, both.brake), (true, true));
        let t = super::super::flight::BroomTuning::default();
        let s = super::super::flight::BroomState::new(glam::Vec3::ZERO, glam::Vec3::X * t.speed, 0.0, 0.0);
        assert!(super::super::flight::step(&s, &both, &t, 1.0 / 60.0).vel.length() < t.speed, "the brake must win over the boost");
    }
}
