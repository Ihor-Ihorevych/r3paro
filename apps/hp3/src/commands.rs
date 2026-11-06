//! Commands from outside the game loop (the Android touch menus, the desktop's quick keys): a queue the loop empties
//! every frame, and what each command does to the running game.

use std::sync::Mutex;

use hp3_script::{Value, Vm};

/// The slot quick save and quick load use, apart from the slots the game's own menu lists.
pub const QUICK_SLOT: i32 = 99;

static QUEUE: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Asks the game loop to run `command` (see `run`).
pub fn push(command: impl Into<String>) {
    if let Ok(mut q) = QUEUE.lock() {
        q.push(command.into());
    }
}

/// The commands asked for since the last call, oldest first.
pub fn take() -> Vec<String> {
    QUEUE.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
}

/// Runs one command: `quicksave`, `quickload`, `level <map>` (sets `level`, which the loop opens), or
/// `cheat <god|ghost|fly|walk|giveall|beans|beatboss|heal>`.
pub fn run(vm: &mut Vm<'_>, command: &str, level: &mut Option<String>) {
    let mut words = command.split_whitespace();
    let (Some(verb), arg) = (words.next(), words.next()) else { return };
    match verb {
        "quicksave" => {
            let ok = hp3_world::save::write(vm, QUICK_SLOT);
            eprintln!("quick save: {}", if ok { "written" } else { "failed" });
            if ok {
                crate::platform::note_saved();
            }
        }
        "quickload" => {
            if hp3_world::save::exists(vm, QUICK_SLOT) {
                hp3_world::save::console(vm, &format!("OPEN ?LOAD={QUICK_SLOT}"));
            } else {
                eprintln!("quick load: no quick save yet");
            }
        }
        "resetpicture" => {
            hp3_world::settings::reset_look(vm);
            eprintln!("settings: gamma, brightness and contrast are back to the defaults");
        }
        "level" => *level = arg.map(str::to_string),
        "cheat" => cheat(vm, arg.unwrap_or("")),
        _ => eprintln!("command: unknown '{command}'"),
    }
}

fn cheat(vm: &mut Vm<'_>, name: &str) {
    use hp3_world::debug_move::{Mode, set_mode};
    let Some(pc) = hp3_world::world(vm).player else { return };
    match name {
        "god" => {
            let on = !hp3_world::get(vm, pc, "bGodMode").as_bool();
            hp3_world::set(vm, pc, "bGodMode", Value::Bool(on));
            eprintln!("cheat: god mode {}", if on { "on" } else { "off" });
        }
        "ghost" => eprintln!("cheat: ghost ({})", set_mode(vm, Mode::Ghost)),
        "fly" => eprintln!("cheat: fly ({})", set_mode(vm, Mode::Fly)),
        "walk" => eprintln!("cheat: walk ({})", set_mode(vm, Mode::Walk)),
        "giveall" => {
            hp3_world::fire(vm, pc, "GiveAll", &mut []);
            eprintln!("cheat: all shields, cards and pixies");
        }
        "beans" => {
            hp3_world::fire(vm, pc, "GiveBeans", &mut [Value::Int(100)]);
            eprintln!("cheat: 100 beans");
        }
        "beatboss" => {
            hp3_world::fire(vm, pc, "BeatBoss", &mut []);
            eprintln!("cheat: the boss is beaten");
        }
        "heal" => {
            if let Some(pawn) = hp3_world::get(vm, pc, "Pawn").as_obj() {
                let max = hp3_world::get(vm, pawn, "HealthMax").as_float();
                hp3_world::set(vm, pawn, "Health", Value::Float(max));
                eprintln!("cheat: healed to {max}");
            }
        }
        other => eprintln!("cheat: unknown '{other}'"),
    }
}
