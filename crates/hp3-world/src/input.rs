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
                }
            }
        }
        input
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

fn param(words: &[&str], name: &str) -> Option<f32> {
    words.iter().find_map(|w| {
        let (k, v) = w.split_once('=')?;
        k.eq_ignore_ascii_case(name).then(|| v.trim_start_matches('+').parse().ok()).flatten()
    })
}

/// Writes this tick's input into the player controller and runs the commands of keys pressed since the last
/// tick.
pub fn apply(vm: &mut Vm<'_>, pc: ObjId) {
    let input = std::mem::take(&mut world(vm).input);
    let mut axes: HashMap<String, f32> = HashMap::default();
    let mut buttons: HashMap<String, bool> = HashMap::default();
    let mut execs: Vec<String> = Vec::new();
    let held: Vec<String> = input.held.iter().cloned().collect();
    let mut keys: Vec<(String, f32)> = held.into_iter().map(|k| (k, 1.0)).collect();
    // The chase camera does not look with the mouse.
    let chase = !world(vm).chase.off;
    if !chase {
        keys.push(("mousex".into(), input.mouse.0));
        keys.push(("mousey".into(), -input.mouse.1));
    }
    for (key, amount) in &keys {
        for cmd in input.commands(key) {
            let words: Vec<&str> = cmd.split_whitespace().collect();
            match words.first().map(|w| w.to_ascii_lowercase()).as_deref() {
                Some("axis") if words.len() >= 2 => {
                    let speed = param(&words, "Speed").or_else(|| param(&words, "SpeedBase")).unwrap_or(1.0);
                    let invert = param(&words, "Invert").unwrap_or(1.0);
                    *axes.entry(words[1].to_string()).or_default() += amount * speed * invert;
                }
                Some("button") if words.len() >= 2 && *amount != 0.0 => {
                    buttons.insert(words[1].to_string(), true);
                }
                _ => {}
            }
        }
    }
    for key in &input.pressed {
        for cmd in input.commands(key) {
            let words: Vec<&str> = cmd.split_whitespace().collect();
            match words.first().map(|w| w.to_ascii_lowercase()).as_deref() {
                Some("axis") | Some("button") | Some("count") | None => {}
                Some("toggle") if words.len() >= 2 => {
                    let cur = get(vm, pc, words[1]).as_bool();
                    set(vm, pc, words[1], Value::Byte(!cur as u8));
                }
                _ => execs.push(cmd),
            }
        }
    }
    // With the chase camera, left and right turn the heading instead of strafing.
    if chase {
        let turn = axes.remove("aStrafe").unwrap_or(0.0);
        crate::chase::update(vm, pc, turn, 1.0 / 60.0);
    }
    // Axes reset every tick; the input script turns them into movement.
    for name in ["aBaseX", "aBaseY", "aBaseZ", "aMouseX", "aMouseY", "aForward", "aTurn", "aStrafe", "aUp", "aLookUp"] {
        set(vm, pc, name, Value::Float(0.0));
    }
    for (name, v) in axes {
        set(vm, pc, &name, Value::Float(v));
    }
    for name in ["bRun", "bDuck", "bLook", "bStrafe", "bFire", "bAltFire", "bFreeLook"] {
        set(vm, pc, name, Value::Byte(0));
    }
    for (name, on) in buttons {
        set(vm, pc, &name, Value::Byte(on as u8));
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

/// Runs a console command such as `Jump` or `Fire`: an exec function of the controller, its pawn or the HUD.
pub fn exec(vm: &mut Vm<'_>, pc: ObjId, cmd: &str) {
    let mut words = cmd.split_whitespace();
    let Some(name) = words.next() else { return };
    let args: Vec<Value> = words.map(|w| Value::Str(w.to_string())).collect();
    let targets = [Some(pc), get(vm, pc, "Pawn").as_obj(), get(vm, pc, "myHUD").as_obj()];
    for t in targets.into_iter().flatten() {
        let Some(n) = vm.db.names.find(name) else { return };
        if vm.find_function(Some(t), None, n).is_some() {
            let mut a = args.clone();
            crate::fire(vm, t, name, &mut a);
            return;
        }
    }
}
