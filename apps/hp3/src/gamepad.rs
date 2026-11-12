//! Gamepad support: SDL gamepads (an Xbox, PlayStation or Switch pad, USB or Bluetooth, on the desktop and the phone alike)
//! drive the game through the same inputs the keyboard and mouse use, so the ini's key bindings stay the single source of truth.
//!
//! Play: left stick moves (analog: a half-pushed stick walks slower), right stick looks, right trigger casts (left mouse),
//! left trigger zooms, A jumps, B uses / skips a cutscene / leaves a creature (Enter), Y switches character, left and right
//! bumpers cycle the spells (mouse wheel), Back shows the map, Start opens the menu.
//! Menus: the D-pad or left stick moves, A confirms (Enter), B / Start goes back (Escape).

use hp3_world::input::Input;
use sdl3::gamepad::{Axis, Button, Gamepad};
use sdl3::{GamepadSubsystem, event::Event};

/// Stick positions inside this radius count as centred; the rest of the range is stretched to 0..1.
const DEAD_ZONE: f32 = 0.22;
/// Looking at full right-stick deflection turns the view as fast as this many mouse pixels per second.
const LOOK_PIXELS_PER_SECOND: f32 = 900.0;
/// A trigger pulled further than this counts as a pressed button.
const TRIGGER_THRESHOLD: f32 = 0.35;
/// In a menu a stick pushed further than this counts as a D-pad press.
const MENU_STICK_THRESHOLD: f32 = 0.6;
/// A menu direction held down repeats after this long, then at the shorter interval.
const MENU_REPEAT_FIRST: f32 = 0.4;
const MENU_REPEAT: f32 = 0.14;

/// Unreal key codes of the keys menus use (the same ones `menu_key` in the main loop gives the keyboard).
pub const MENU_ENTER: u8 = 0x0d;
pub const MENU_ESCAPE: u8 = 0x1b;
pub const MENU_LEFT: u8 = 0x25;
pub const MENU_UP: u8 = 0x26;
pub const MENU_RIGHT: u8 = 0x27;
pub const MENU_DOWN: u8 = 0x28;

/// What a pad button asks of the game (the main loop carries it out, since it owns the VM and the menu state).
#[derive(Debug, PartialEq)]
pub enum Action {
    /// A key, by the ini's name, pressed or let go.
    Key(&'static str, bool),
    /// A mouse-wheel notch (the spells cycle): the key is pressed and released at once.
    Notch(&'static str),
    /// Switch to the next character.
    SwitchCharacter,
    /// A menu key, with the state the menu code wants (1 press, 2 repeat, 3 release).
    Menu(u8, u8),
}

#[derive(Default)]
pub struct Pad {
    subsystem: Option<GamepadSubsystem>,
    open: Vec<Gamepad>,
    left: (f32, f32),
    right: (f32, f32),
    triggers: (bool, bool),
    dpad: [bool; 4], // up, down, left, right
    /// The menu direction held now (a key code) and the time until it repeats.
    menu_held: Option<(u8, f32)>,
    /// Whether the movement keys are currently driven by the stick (so letting go zeroes them once).
    moving: bool,
}

/// A stick axis as -1..1 with the dead zone taken out of the middle.
fn shape(x: f32, y: f32) -> (f32, f32) {
    let len = (x * x + y * y).sqrt();
    if len <= DEAD_ZONE {
        return (0.0, 0.0);
    }
    let scaled = ((len - DEAD_ZONE) / (1.0 - DEAD_ZONE)).min(1.0);
    (x / len * scaled, y / len * scaled)
}

impl Pad {
    pub fn new(subsystem: GamepadSubsystem) -> Pad {
        let mut pad = Pad { subsystem: Some(subsystem), ..Pad::default() };
        // Pads already plugged in at start (SDL also reports them as added events, which open() ignores the second time).
        if let Some(ids) = pad.subsystem.as_ref().and_then(|s| s.gamepads().ok()) {
            for id in ids {
                pad.add(id);
            }
        }
        pad
    }

    fn add(&mut self, id: sdl3::joystick::JoystickId) {
        let Some(sub) = &self.subsystem else { return };
        if self.open.iter().any(|g| g.id().ok() == Some(id)) {
            return;
        }
        match sub.open(id) {
            Ok(g) => {
                eprintln!("gamepad: {} connected", g.name().unwrap_or_else(|| "unnamed pad".into()));
                self.open.push(g);
            }
            Err(e) => eprintln!("gamepad: could not open pad: {e}"),
        }
    }

    /// Handles a gamepad event; anything else is none of its business. Returns what the game should do.
    pub fn event(&mut self, event: &Event, in_menu: bool) -> Vec<Action> {
        let mut out = Vec::new();
        match *event {
            Event::GamepadAdded { which, .. } => self.add(which),
            Event::GamepadRemoved { which, .. } => {
                self.open.retain(|g| g.id().ok() != Some(which));
                eprintln!("gamepad: disconnected");
                // Nothing may stay held down when the pad goes away.
                out.extend(self.release_all(in_menu));
            }
            Event::GamepadAxisMotion { axis, value, .. } => {
                let v = value as f32 / 32768.0;
                match axis {
                    Axis::LeftX => self.left.0 = v,
                    Axis::LeftY => self.left.1 = v,
                    Axis::RightX => self.right.0 = v,
                    Axis::RightY => self.right.1 = v,
                    Axis::TriggerLeft | Axis::TriggerRight => {
                        let pressed = v > TRIGGER_THRESHOLD;
                        let slot = if axis == Axis::TriggerLeft { &mut self.triggers.0 } else { &mut self.triggers.1 };
                        if *slot != pressed {
                            *slot = pressed;
                            let key = if axis == Axis::TriggerLeft { "MiddleMouse" } else { "LeftMouse" };
                            if in_menu {
                                // The right trigger is a click-free confirm in menus.
                                if axis == Axis::TriggerRight {
                                    out.push(Action::Menu(MENU_ENTER, if pressed { 1 } else { 3 }));
                                }
                            } else {
                                out.push(Action::Key(key, pressed));
                            }
                        }
                    }
                }
            }
            Event::GamepadButtonDown { button, .. } => out.extend(self.button(button, true, in_menu)),
            Event::GamepadButtonUp { button, .. } => out.extend(self.button(button, false, in_menu)),
            _ => {}
        }
        out
    }

    fn button(&mut self, button: Button, down: bool, in_menu: bool) -> Option<Action> {
        match button {
            Button::DPadUp => self.dpad[0] = down,
            Button::DPadDown => self.dpad[1] = down,
            Button::DPadLeft => self.dpad[2] = down,
            Button::DPadRight => self.dpad[3] = down,
            _ => {}
        }
        let state = if down { 1 } else { 3 };
        if in_menu {
            return match button {
                Button::South => Some(Action::Menu(MENU_ENTER, state)),
                Button::East | Button::Start => Some(Action::Menu(MENU_ESCAPE, state)),
                _ => None, // directions come from `frame`, which repeats them
            };
        }
        Some(match button {
            Button::South => Action::Key("Space", down),
            Button::East => Action::Key("Enter", down),
            Button::West => Action::Key("Shift", down), // walk slowly
            Button::North if down => Action::SwitchCharacter,
            Button::LeftShoulder if down => Action::Notch("MouseWheelDown"),
            Button::RightShoulder if down => Action::Notch("MouseWheelUp"),
            Button::Back => Action::Key("Tab", down),
            Button::Start => Action::Key("Escape", down),
            Button::LeftStick => Action::Key("Ctrl", down), // jump alias in flight
            _ => return None,
        })
    }

    /// Ends everything the pad holds (it left, or a menu opened over play).
    fn release_all(&mut self, in_menu: bool) -> Vec<Action> {
        let mut out = Vec::new();
        if self.triggers.0 {
            out.push(Action::Key("MiddleMouse", false));
        }
        if self.triggers.1 {
            if in_menu {
                out.push(Action::Menu(MENU_ENTER, 3));
            } else {
                out.push(Action::Key("LeftMouse", false));
            }
        }
        if let Some((k, _)) = self.menu_held.take() {
            out.push(Action::Menu(k, 3));
        }
        *self = Pad { subsystem: self.subsystem.take(), open: std::mem::take(&mut self.open), moving: self.moving, ..Pad::default() };
        out
    }

    /// Once per frame: the sticks move and turn the player (play) or step through the entries (menus).
    pub fn frame(&mut self, input: &mut Input, in_menu: bool, dt: f32) -> Vec<Action> {
        let mut out = Vec::new();
        if in_menu {
            if self.moving {
                self.stop_moving(input);
            }
            let (lx, ly) = self.left;
            let want = if self.dpad[0] || ly < -MENU_STICK_THRESHOLD {
                Some(MENU_UP)
            } else if self.dpad[1] || ly > MENU_STICK_THRESHOLD {
                Some(MENU_DOWN)
            } else if self.dpad[2] || lx < -MENU_STICK_THRESHOLD {
                Some(MENU_LEFT)
            } else if self.dpad[3] || lx > MENU_STICK_THRESHOLD {
                Some(MENU_RIGHT)
            } else {
                None
            };
            match (self.menu_held, want) {
                (Some((k, t)), Some(w)) if k == w => {
                    let t = t - dt;
                    if t <= 0.0 {
                        out.push(Action::Menu(k, 2));
                        self.menu_held = Some((k, MENU_REPEAT));
                    } else {
                        self.menu_held = Some((k, t));
                    }
                }
                (held, want) => {
                    if let Some((k, _)) = held {
                        out.push(Action::Menu(k, 3));
                    }
                    self.menu_held = want.map(|w| {
                        out.push(Action::Menu(w, 1));
                        (w, MENU_REPEAT_FIRST)
                    });
                }
            }
            return out;
        }
        if let Some((k, _)) = self.menu_held.take() {
            out.push(Action::Menu(k, 3));
        }
        // Left stick (and the D-pad) move: forward is up on the stick. The D-pad adds full-strength digital steps.
        let (mut mx, mut my) = shape(self.left.0, self.left.1);
        mx += (self.dpad[3] as i32 - self.dpad[2] as i32) as f32;
        my += (self.dpad[1] as i32 - self.dpad[0] as i32) as f32;
        let (mx, my) = (mx.clamp(-1.0, 1.0), my.clamp(-1.0, 1.0));
        if mx != 0.0 || my != 0.0 || self.moving {
            input.set_analog("W", (-my).max(0.0));
            input.set_analog("S", my.max(0.0));
            input.set_analog("A", (-mx).max(0.0));
            input.set_analog("D", mx.max(0.0));
            self.moving = mx != 0.0 || my != 0.0;
        }
        // Right stick turns the view; the response is squared so small pushes aim finely.
        let (rx, ry) = shape(self.right.0, self.right.1);
        if rx != 0.0 || ry != 0.0 {
            let curve = |v: f32| v * v.abs();
            input.mouse_move(curve(rx) * LOOK_PIXELS_PER_SECOND * dt, curve(ry) * LOOK_PIXELS_PER_SECOND * dt);
        }
        out
    }

    fn stop_moving(&mut self, input: &mut Input) {
        for k in ["W", "S", "A", "D"] {
            input.set_analog(k, 0.0);
        }
        self.moving = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dead_zone_swallows_small_pushes() {
        assert_eq!(shape(0.1, 0.1), (0.0, 0.0));
        let (x, y) = shape(1.0, 0.0);
        assert!((x - 1.0).abs() < 1e-5 && y == 0.0);
    }

    #[test]
    fn menu_and_play_buttons_differ() {
        let mut pad = Pad::default();
        assert_eq!(pad.button(Button::South, true, false), Some(Action::Key("Space", true)));
        assert_eq!(pad.button(Button::South, true, true), Some(Action::Menu(MENU_ENTER, 1)));
    }
}
