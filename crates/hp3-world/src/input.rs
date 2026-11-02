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
    keys.push(("mousex".into(), input.mouse.0));
    keys.push(("mousey".into(), -input.mouse.1));
    for (key, amount) in &keys {
        let mouse = key == "mousex" || key == "mousey";
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
                Some("button") if words.len() >= 2 && *amount != 0.0 => {
                    buttons.insert(words[1].to_string());
                }
                Some("count") if words.len() >= 2 && *amount != 0.0 => {
                    counts.insert(words[1].to_string());
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
