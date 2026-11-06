//! The game's option menus: the console commands their scripts run (`GAMMA`, `set ini:...`, `get ...`) and what the
//! player's choices are stored as.
//!
//! The menus ask for values the way the original engine's console does: `get ini:Engine.Engine.<Object> <Key>` and
//! `set ini:Engine.Engine.<Object> <Key> <Value>` for a driver's own settings, `get <Class> <Property>` and
//! `set <Class> <Property> <Value>` for a class's default. Driver settings are kept in the sections of the ini file
//! that name the real driver (`SDLDrv.SDLClient`, `ALAudio.ALAudioSubsystem`...), so what Default.ini says is the
//! starting point; every change is also written to `System/hp3-settings.ini`. The app watches `World::settings_version`
//! to learn that something changed and reads the values it needs again.

use hp3_script::{Value, Vm};

use crate::{get, set, world};

pub const DISPLAY: &str = "SDLDrv.SDLClient";
pub const AUDIO: &str = "ALAudio.ALAudioSubsystem";
pub const RENDER: &str = "PixoDrv.PixoRenderDevice";
/// Where class defaults the player changed (`PlayerInput.MouseSensitivity`) are remembered.
const CLASS_DEFAULTS: &str = "hp3.classdefaults";

/// The ini section of a driver object the menus name (`Engine.Engine.AudioDevice` is the audio driver).
fn section_of(object: &str) -> String {
    let name = object.rsplit('.').next().unwrap_or(object);
    match name.to_ascii_lowercase().as_str() {
        "audiodevice" => AUDIO.into(),
        "viewportmanager" | "client" => DISPLAY.into(),
        "renderdevice" => RENDER.into(),
        _ => object.to_string(),
    }
}

/// A setting as text.
pub fn text(vm: &Vm<'_>, section: &str, key: &str) -> Option<String> {
    let w = vm.ext.as_ref().and_then(|e| e.downcast_ref::<crate::World>())?;
    w.config.get(section, key).map(str::to_string)
}

/// A number setting, `default` when it is missing or not a number.
pub fn float(vm: &Vm<'_>, section: &str, key: &str, default: f32) -> f32 {
    text(vm, section, key).and_then(|t| t.trim().parse().ok()).unwrap_or(default)
}

/// A yes/no setting.
pub fn flag(vm: &Vm<'_>, section: &str, key: &str, default: bool) -> bool {
    match text(vm, section, key) {
        Some(t) => matches!(t.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes" | "on"),
        None => default,
    }
}

/// Stores a setting (remembered, and visible to `settings_version` watchers).
pub fn store(vm: &mut Vm<'_>, section: &str, key: &str, value: &str) {
    let w = world(vm);
    w.config.set_setting(section, key, value);
    w.settings_version += 1;
}

/// `GAMMA`, `BRIGHTNESS` and `CONTRAST` take one number. The display's range is 0..1 for the last two and 0.5..2.5 for
/// gamma, so a slider's value is clamped there.
fn display_value(key: &str, value: f32) -> f32 {
    match key {
        "Gamma" => value.clamp(0.1, 4.0),
        _ => value.clamp(0.0, 1.0),
    }
}

fn class_struct(vm: &mut Vm<'_>, name: &str) -> Option<hp3_script::StructId> {
    let paths: Vec<String> = if name.contains('.') {
        vec![name.to_string()]
    } else {
        ["Engine", "KWGame", "HGame"].iter().map(|p| format!("{p}.{name}")).collect()
    };
    paths.iter().find_map(|p| crate::find_object(vm, p).and_then(|o| vm.class_of(o)))
}

/// A class default as the console prints it.
fn class_default(vm: &mut Vm<'_>, class: &str, prop: &str) -> Option<String> {
    let c = class_struct(vm, class)?;
    let n = vm.db.names.find(prop)?;
    let p = vm.db.get(c).prop(n)?.clone();
    let v = vm.db.get(c).defaults.get(p.slot)?.clone();
    Some(value_text(vm, &v))
}

fn value_text(vm: &Vm<'_>, v: &Value) -> String {
    match v {
        Value::Int(i) => i.to_string(),
        Value::Float(f) => hp3_script::natives::float_to_string(*f),
        Value::Byte(b) => b.to_string(),
        Value::Bool(b) => if *b { "True".into() } else { "False".into() },
        Value::Name(n) => vm.db.names.get(*n).to_string(),
        Value::Str(s) => s.clone(),
        _ => String::new(),
    }
}

/// Sets a class's default and the matching property of the player's own object of that class (the live
/// `PlayerInput` was made from the old default).
fn set_class_default(vm: &mut Vm<'_>, class: &str, prop: &str, text: &str) -> bool {
    let Some(c) = class_struct(vm, class) else { return false };
    let Some(n) = vm.db.names.find(prop) else { return false };
    let Some(p) = vm.db.get(c).prop(n).cloned() else { return false };
    let Some(v) = crate::config::parse(vm, &p.ty, text) else { return false };
    if let Some(slot) = vm.db.structs[c.0 as usize].defaults.get_mut(p.slot) {
        *slot = v.clone();
    }
    let short = class.rsplit('.').next().unwrap_or(class).to_string();
    if let Some(pc) = world(vm).player
        && let Some(o) = get(vm, pc, &short).as_obj()
    {
        set(vm, o, prop, v);
    }
    true
}

/// Puts back the class defaults the player changed in an earlier run. Called once the config's values are in.
pub fn apply_saved(vm: &mut Vm<'_>) {
    let saved = world(vm).config.saved_settings(CLASS_DEFAULTS);
    for (key, value) in saved {
        if let Some((class, prop)) = key.split_once('.') {
            set_class_default(vm, class, prop, &value);
        }
    }
}

/// The console commands of the option menus. `None` when the command is not one of theirs.
pub fn console(vm: &mut Vm<'_>, verb: &str, rest: &[&str]) -> Option<String> {
    let verb = verb.to_ascii_lowercase();
    match verb.as_str() {
        "gamma" | "brightness" | "contrast" => {
            let value: f32 = rest.first()?.parse().ok()?;
            let key = match verb.as_str() {
                "gamma" => "Gamma",
                "brightness" => "Brightness",
                _ => "Contrast",
            };
            store(vm, DISPLAY, key, &format!("{:.6}", display_value(key, value)));
            Some(String::new())
        }
        // The audio settings were stored by `set`; the app reads them again when it sees the version change.
        "setvolumes" => {
            world(vm).settings_version += 1;
            Some(String::new())
        }
        // Texture caches are not kept per quality here.
        "flush" => Some(String::new()),
        "keyname" => Some(crate::input::key_name(rest.first()?.parse().ok()?).to_string()),
        "keybinding" => Some(world(vm).input.binding(rest.first()?)),
        "set" if rest.first().is_some_and(|t| t.eq_ignore_ascii_case("input")) => {
            let key = *rest.get(1)?;
            let binding = rest[2..].join(" ");
            world(vm).input.set_binding(key, &binding);
            // Remembered in `[Engine.Input]`, which the bindings are read from at start.
            world(vm).config.set_setting("Engine.Input", key, &binding);
            Some(String::new())
        }
        "get" => {
            let target = *rest.first()?;
            if let Some(object) = strip_ini(target) {
                let section = section_of(object);
                Some(text(vm, &section, rest.get(1)?).unwrap_or_default())
            } else {
                Some(class_default(vm, target, rest.get(1)?).unwrap_or_default())
            }
        }
        "set" => {
            let target = *rest.first()?;
            let key = *rest.get(1)?;
            let value = rest[2..].join(" ");
            if let Some(object) = strip_ini(target) {
                store(vm, &section_of(object), key, &value);
            } else if set_class_default(vm, target, key, &value) {
                let short = target.rsplit('.').next().unwrap_or(target);
                world(vm).config.set_setting(CLASS_DEFAULTS, &format!("{short}.{key}"), &value);
                world(vm).settings_version += 1;
            }
            Some(String::new())
        }
        _ => None,
    }
}

fn strip_ini(target: &str) -> Option<&str> {
    let t = target.get(..4)?;
    t.eq_ignore_ascii_case("ini:").then(|| &target[4..])
}

/// The picture adjustment of the display options as `[gamma exponent, brightness added, contrast multiplier]`;
/// `[1, 0, 1]` leaves the picture as drawn. The install's own defaults (`Default.ini`) are the neutral point: the game
/// starts looking as it always did, and the sliders move it from there.
pub fn look(vm: &Vm<'_>) -> [f32; 3] {
    let Some(w) = vm.ext.as_ref().and_then(|e| e.downcast_ref::<crate::World>()) else { return [1.0, 0.0, 1.0] };
    let value = |key: &str, fallback: f32| w.config.get(DISPLAY, key).and_then(|t| t.trim().parse::<f32>().ok()).unwrap_or(fallback);
    let reference = |key: &str, fallback: f32| w.config.ini.get(DISPLAY, key).and_then(|t| t.trim().parse::<f32>().ok()).unwrap_or(fallback);
    let (gamma, bright, contrast) = (value("Gamma", 0.8), value("Brightness", 0.8), value("Contrast", 0.7));
    let (gamma0, bright0, contrast0) = (reference("Gamma", 0.8), reference("Brightness", 0.8), reference("Contrast", 0.7));
    [(gamma0 / gamma.max(0.1)).clamp(0.2, 5.0), (bright - bright0) * 0.6, (1.0 + (contrast - contrast0)).clamp(0.2, 2.0)]
}

/// Puts Gamma, Brightness and Contrast back to what the install's `Default.ini` says (the neutral point of `look`).
pub fn reset_look(vm: &mut Vm<'_>) {
    for (key, fallback) in [("Gamma", 0.8f32), ("Brightness", 0.8), ("Contrast", 0.7)] {
        let default = world(vm).config.ini.get(DISPLAY, key).and_then(|t| t.trim().parse::<f32>().ok()).unwrap_or(fallback);
        store(vm, DISPLAY, key, &format!("{:.6}", display_value(key, default)));
    }
}

/// The mouse speed (relative to the engine's own default of 3) and whether looking up and down is inverted, as the
/// player set them in the Input options: the player's `PlayerInput` when it has one, else what was stored.
pub fn mouse_look(vm: &mut Vm<'_>, pc: hp3_script::ObjId) -> (f32, bool) {
    if let Some(pi) = get(vm, pc, "PlayerInput").as_obj() {
        return (get(vm, pi, "MouseSensitivity").as_float() / 3.0, get(vm, pi, "bInvertMouse").as_bool());
    }
    (float(vm, CLASS_DEFAULTS, "PlayerInput.MouseSensitivity", 3.0) / 3.0, flag(vm, CLASS_DEFAULTS, "PlayerInput.bInvertMouse", false))
}
