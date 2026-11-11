//! Player input: key bindings from `[Engine.Input]` (`W=MoveForward`, aliases such as
//! `(Command="Axis aBaseY Speed=+1200.0",Alias=MoveForward)`), applied to the player controller's input
//! properties each tick. Keys are named as the ini names them (`W`, `Space`, `LeftMouse`, `MouseX`...).
//!
//! Written from the ini format and the scripts that read the properties; NOT yet verified against HP3's
//! Engine.dll input code.

use hp3_script::fx::{FxHashMap as HashMap, FxHashSet as HashSet};

use hp3_script::{ObjId, Value, Vm};

use crate::{get, set, world};

#[derive(Default)]
pub struct Input {
    bindings: HashMap<String, String>,
    aliases: HashMap<String, String>,
    held: HashSet<String>,
    /// Every `Button` name the bindings can set (cleared each tick, set while their key is held).
    button_names: Vec<String>,
    /// Keys pressed or released since the last tick.
    pressed: Vec<String>,
    released: Vec<String>,
    mouse: (f32, f32),
}

/// Parses `(Command="...",Alias=Name)`.
fn parse_alias(v: &str) -> Option<(String, String)> {
    let cmd_start = v.find("Command=\"")? + 9;
    let cmd_end = cmd_start + v[cmd_start..].find('"')?;
    let alias_start = v.find("Alias=")? + 6;
    let alias = v[alias_start..].trim_end_matches(')').trim();
    Some((alias.to_ascii_lowercase(), v[cmd_start..cmd_end].to_string()))
}

impl Input {
    pub fn from_config(config: &hp3_core::Config) -> Input {
        let mut input = Input::default();
        for ini in [&config.ini, &config.user] {
            for (key, value) in ini.entries("Engine.Input") {
                if key.to_ascii_lowercase().starts_with("aliases[") {
                    if let Some((alias, cmd)) = parse_alias(value) {
                        input.aliases.insert(alias, cmd);
                    }
                } else if !value.trim().is_empty() {
                    input.bindings.insert(key.to_ascii_lowercase(), value.to_string());
                } else {
                    // A key the player cleared in the options stays cleared over the install's own binding.
                    input.bindings.remove(&key.to_ascii_lowercase());
                }
            }
        }
        // Every button any alias or binding can press.
        let mut names: Vec<String> = Vec::new();
        for text in input.aliases.values().chain(input.bindings.values()) {
            for part in text.split('|') {
                let words: Vec<&str> = part.split_whitespace().collect();
                if words.len() >= 2 && words[0].eq_ignore_ascii_case("button") && !names.iter().any(|n| n == words[1]) {
                    names.push(words[1].to_string());
                }
            }
        }
        input.button_names = names;
        input
    }

    /// The command line a key runs (empty for none), as the ini writes it.
    pub fn binding(&self, key: &str) -> String {
        self.bindings.get(&key.to_ascii_lowercase()).cloned().unwrap_or_default()
    }

    /// Gives a key another command line.
    pub fn set_binding(&mut self, key: &str, binding: &str) {
        let k = key.to_ascii_lowercase();
        if binding.trim().is_empty() {
            self.bindings.remove(&k);
        } else {
            self.bindings.insert(k, binding.to_string());
        }
    }

    pub fn key_down(&mut self, key: &str) {
        let k = key.to_ascii_lowercase();
        if self.held.insert(k.clone()) {
            self.pressed.push(k);
        }
    }

    pub fn key_up(&mut self, key: &str) {
        let k = key.to_ascii_lowercase();
        if self.held.remove(&k) {
            self.released.push(k);
        }
    }

    pub fn mouse_move(&mut self, dx: f32, dy: f32) {
        self.mouse.0 += dx;
        self.mouse.1 += dy;
    }

    /// The commands a key runs, aliases expanded, split at `|`.
    fn commands(&self, key: &str) -> Vec<String> {
        let mut out = Vec::new();
        let Some(binding) = self.bindings.get(key) else { return out };
        let mut stack: Vec<(String, u32)> = binding.split('|').map(|s| (s.trim().to_string(), 0)).collect();
        stack.reverse();
        while let Some((cmd, depth)) = stack.pop() {
            let first = cmd.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
            match self.aliases.get(&first) {
                Some(expansion) if depth < 8 => {
                    let mut parts: Vec<(String, u32)> = expansion.split('|').map(|s| (s.trim().to_string(), depth + 1)).collect();
                    parts.reverse();
                    stack.extend(parts);
                }
                _ => out.push(cmd),
            }
        }
        out
    }
}

/// The keys a Quidditch match takes over whole: while one runs they fly the broom and run none of their own
/// bindings' buttons or commands. The arrow keys are not among them, because all they are ever bound to is
/// axes (turn and look), which the broom and the controller can both have. In lower case, as the held and
/// pressed sets keep them.
const BROOM_KEYS: [&str; 5] = ["leftmouse", "rightmouse", "a", "z", "space"];

/// The broom keys of this frame, for a Quidditch match: the arrow keys steer, `RightMouse` or `A` taps a
/// reach for the snitch (as does holding `Space` or `Enter`). There is no boost and no brake. Keys are
/// named as the ini names them, in lower case (as `key_down` keeps them); a tap is an edge of this frame. `Enter` is deliberately not in `BROOM_KEYS`: it stays the menus' key.
fn broom_keys(input: &Input) -> crate::quidditch::input::KeyState {
    let held = |k: &str| input.held.contains(k);
    let pressed = |k: &str| input.pressed.iter().any(|p| p == k);
    crate::quidditch::input::KeyState {
        left: held("left"),
        right: held("right"),
        up: held("up"),
        down: held("down"),
        // A tap: a press, or (the phone's JUMP is a RightMouse *toggle* on the broom) a release.
        reach: pressed("rightmouse") || pressed("a") || input.released.iter().any(|p| p == "rightmouse"),
        catch: held("space") || held("enter"),
    }
}

fn param(words: &[&str], name: &str) -> Option<f32> {
    words.iter().find_map(|w| {
        let (k, v) = w.split_once('=')?;
        k.eq_ignore_ascii_case(name).then(|| v.trim_start_matches('+').parse().ok()).flatten()
    })
}

/// Writes this tick's input into the player controller and runs the commands of keys pressed since the last
/// tick.
/// Reads the keys and the mouse into the controller's input properties for a tick of `dt` seconds, then runs
/// the commands of the keys pressed since the last tick.
pub fn apply(vm: &mut Vm<'_>, pc: ObjId, dt: f32) {
    let dt = dt.max(1.0 / 240.0);
    let input = std::mem::take(&mut world(vm).input);
    let mut axes: HashMap<String, f32> = HashMap::default();
    let mut buttons: HashSet<String> = HashSet::default();
    let mut counts: HashSet<String> = HashSet::default();
    let mut execs: Vec<String> = Vec::new();
    // Keys held now, and keys pressed and let go inside this frame (their buttons still count once).
    let mut keys: Vec<(String, f32)> = input.held.iter().map(|k| (k.clone(), 1.0)).collect();
    for k in &input.pressed {
        if !input.held.contains(k) {
            keys.push((k.clone(), 1.0));
        }
    }
    // A Quidditch match flies the broom from the same keys and mouse motion, before the axes below are made.
    // While one runs, the four keys that fly it do only that: their own bindings' buttons and commands
    // (`LeftMouse=Fire`, `RightMouse=Jump|WingFlap|bWingFlap`) would cast spells and flap from the broom, so
    // they are not run. Their axis commands still are, and every other key (Escape, the menus, the pause) is
    // untouched -- as is the whole path on a level with no match, which is every level but a match.
    let flying = world(vm).quidditch.is_some();
    if flying {
        let broom = broom_keys(&input);
        crate::quidditch::glue::feed_input(vm, broom, input.mouse);
    }
    let flies_the_broom = |key: &str| flying && BROOM_KEYS.contains(&key);
    // The options menu's mouse speed and invert (`PlayerInput`); the engine's own default of 3 is the speed the axes were tuned for.
    let (speed, invert_y) = crate::settings::mouse_look(vm, pc);
    keys.push(("mousex".into(), input.mouse.0 * speed));
    keys.push(("mousey".into(), if invert_y { input.mouse.1 } else { -input.mouse.1 } * speed));
    for (key, amount) in &keys {
        let mouse = key == "mousex" || key == "mousey";
        let broom = flies_the_broom(key);
        for cmd in input.commands(key) {
            let words: Vec<&str> = cmd.split_whitespace().collect();
            match words.first().map(|w| w.to_ascii_lowercase()).as_deref() {
                Some("axis") if words.len() >= 2 => {
                    let invert = param(&words, "Invert").unwrap_or(1.0);
                    let value = if mouse {
                        // Mouse pixels accumulate at 0.01 x Speed each, and every axis is then scaled by 20 / dt.
                        0.2 * param(&words, "Speed").unwrap_or(1.0) * amount / dt
                    } else if let Some(base) = param(&words, "SpeedBase") {
                        // Keys that ramp (the arrow keys) reach twice the plain rate.
                        40.0 * base
                    } else {
                        20.0 * param(&words, "Speed").unwrap_or(1.0)
                    };
                    *axes.entry(words[1].to_string()).or_default() += value * invert;
                }
                Some("button") if !broom && words.len() >= 2 && *amount != 0.0 => {
                    buttons.insert(words[1].to_string());
                }
                Some("count") if !broom && words.len() >= 2 && *amount != 0.0 => {
                    counts.insert(words[1].to_string());
                }
                _ => {}
            }
        }
    }
    for key in &input.pressed {
        if flies_the_broom(key) {
            if crate::envc::var_os("HP3_QUID_DEBUG").is_some() {
                // The axis commands are not among the dropped ones: they are made in the loop above, as they
                // are with no match. Everything else this key binds is what the match takes over.
                let mut dropped: Vec<String> = input.commands(key).into_iter().filter(|c| !c.split_whitespace().next().is_some_and(|w| w.eq_ignore_ascii_case("axis"))).collect();
                dropped.dedup();
                println!("quidditch: {key} flies the broom; its own commands {dropped:?} were not run");
            }
            continue;
        }
        for cmd in input.commands(key) {
            let words: Vec<&str> = cmd.split_whitespace().collect();
            match words.first().map(|w| w.to_ascii_lowercase()).as_deref() {
                Some("axis") | Some("button") | Some("count") | None => {}
                Some("toggle") if words.len() >= 2 => {
                    // A toggle flips bit 7 of the byte.
                    let cur = get(vm, pc, words[1]).as_int() as u8;
                    set(vm, pc, words[1], Value::Byte(cur ^ 0x80));
                }
                _ => execs.push(cmd),
            }
        }
    }
    // Axes and buttons start from nothing every tick; the input script turns them into movement.
    for name in ["aBaseX", "aBaseY", "aBaseZ", "aMouseX", "aMouseY", "aForward", "aTurn", "aStrafe", "aUp", "aLookUp", "aArrowRight", "aArrowUp"] {
        set(vm, pc, name, Value::Float(0.0));
    }
    for (name, v) in axes {
        set(vm, pc, &name, Value::Float(v));
    }
    for name in &input.button_names {
        set(vm, pc, name, Value::Byte(0));
    }
    for name in buttons {
        set(vm, pc, &name, Value::Byte(1));
    }
    for name in counts {
        let cur = get(vm, pc, &name).as_int() as u8;
        set(vm, pc, &name, Value::Byte(cur.wrapping_add(1)));
    }
    for cmd in execs {
        exec(vm, pc, &cmd);
    }
    let w = world(vm);
    w.input = input;
    w.input.pressed.clear();
    w.input.released.clear();
    w.input.mouse = (0.0, 0.0);
}

/// The input properties a script reads after `PlayerTick`: zero again, so actors that tick later this frame
/// see no input.
pub fn clear_axes(vm: &mut Vm<'_>, pc: ObjId) {
    for name in ["aBaseX", "aBaseY", "aBaseZ", "aMouseX", "aMouseY", "aForward", "aTurn", "aStrafe", "aUp", "aLookUp", "aArrowRight", "aArrowUp"] {
        set(vm, pc, name, Value::Float(0.0));
    }
}

/// Runs a console command such as `Jump` or `Fire`: an exec function of the controller, its pawn or the HUD.
pub fn exec(vm: &mut Vm<'_>, pc: ObjId, cmd: &str) {
    let mut words = cmd.split_whitespace();
    let Some(name) = words.next() else { return };
    let args: Vec<Value> = words.map(|w| Value::Str(w.to_string())).collect();
    // Enter skips every cutscene, including the ones the level marks as not to be skipped.
    if name.eq_ignore_ascii_case("BypassCutscene") {
        allow_bypass(vm);
    }
    let targets = [Some(pc), get(vm, pc, "Pawn").as_obj(), get(vm, pc, "myHUD").as_obj()];
    for t in targets.into_iter().flatten() {
        let Some(n) = vm.db.names.find(name) else { break };
        if vm.find_function(Some(t), None, n).is_some() {
            let path = hp3_script::natives::object_path(vm, t);
            let mut a = args.clone();
            let result = crate::fire(vm, t, name, &mut a);
            vm.log.push(format!("exec: {cmd} on {path} -> {result:?}"));
            if name.eq_ignore_ascii_case("EndCreaturePossess") {
                let kw = get(vm, pc, "KWPawn").as_obj().map(|o| hp3_script::natives::object_path(vm, o));
                let pawn = get(vm, pc, "Pawn").as_obj().map(|o| hp3_script::natives::object_path(vm, o));
                let main = get(vm, pc, "Pawn").as_obj().map(|o| get(vm, o, "bIsMainPlayer").as_bool());
                vm.log.push(format!("exec: EndCreaturePossess: controller KWPawn {kw:?}, Pawn {pawn:?}, bIsMainPlayer {main:?}"));
            }
            return;
        }
    }
    vm.log.push(format!("exec: {cmd}: no such function on the controller, its pawn or the HUD"));
}

/// Clears `bNoBypass` on every cutscene so the bypass is not refused.
fn allow_bypass(vm: &mut Vm<'_>) {
    let Some(class) = vm.db.find("KWCutScene") else { return };
    let actors = crate::world(vm).actors.clone();
    for a in actors {
        if vm.db.objects.get(a).class.is_some_and(|c| vm.is_child_of(c, class)) && get(vm, a, "bNoBypass").as_bool() {
            crate::set(vm, a, "bNoBypass", Value::Bool(false));
        }
    }
}

/// The name of a key by its code (`KEYNAME <code>`, as the Input options list the keys): the names the ini's bindings use.
pub fn key_name(code: u32) -> &'static str {
    const F: [&str; 24] = [
        "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "F13", "F14", "F15", "F16", "F17", "F18", "F19", "F20", "F21", "F22", "F23", "F24",
    ];
    const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
    const LETTERS: [&str; 26] = ["A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S", "T", "U", "V", "W", "X", "Y", "Z"];
    const PAD: [&str; 10] = ["NumPad0", "NumPad1", "NumPad2", "NumPad3", "NumPad4", "NumPad5", "NumPad6", "NumPad7", "NumPad8", "NumPad9"];
    match code {
        1 => "LeftMouse",
        2 => "RightMouse",
        4 => "MiddleMouse",
        8 => "Backspace",
        9 => "Tab",
        13 => "Enter",
        16 => "Shift",
        17 => "Ctrl",
        18 => "Alt",
        19 => "Pause",
        20 => "CapsLock",
        27 => "Escape",
        32 => "Space",
        33 => "PageUp",
        34 => "PageDown",
        35 => "End",
        36 => "Home",
        37 => "Left",
        38 => "Up",
        39 => "Right",
        40 => "Down",
        45 => "Insert",
        46 => "Delete",
        48..=57 => DIGITS[(code - 48) as usize],
        65..=90 => LETTERS[(code - 65) as usize],
        96..=105 => PAD[(code - 96) as usize],
        106 => "GreyStar",
        107 => "GreyPlus",
        109 => "GreyMinus",
        110 => "NumPadPeriod",
        111 => "GreySlash",
        112..=135 => F[(code - 112) as usize],
        144 => "NumLock",
        145 => "ScrollLock",
        160 => "LShift",
        161 => "RShift",
        162 => "LControl",
        163 => "RControl",
        164 => "LAlt",
        165 => "RAlt",
        186 => "Semicolon",
        187 => "Equals",
        188 => "Comma",
        189 => "Minus",
        190 => "Period",
        191 => "Slash",
        192 => "Tilde",
        219 => "LeftBracket",
        220 => "Backslash",
        221 => "RightBracket",
        222 => "SingleQuote",
        239 => "MouseWheelUp",
        240 => "MouseWheelDown",
        _ => "",
    }
}
