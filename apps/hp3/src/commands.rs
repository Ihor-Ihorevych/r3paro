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
/// `cheat <god|ghost|fly|walk|giveall|beans|cakes|pasties|beatboss|defeatpeeves|heal>`.
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
        "dumpnear" => dump_near(vm, arg.and_then(|a| a.parse().ok()).unwrap_or(400.0)),
        "level" => *level = arg.map(str::to_string),
        "cheat" => cheat(vm, arg.unwrap_or("")),
        _ => eprintln!("command: unknown '{command}'"),
    }
}

/// `dumpnear [radius]` (F6): writes every actor within `radius` of the player's pawn to the log, with what decides whether it can
/// be touched or picked up: state, collision size and flags, base, owner, physics, velocity. For bugs such as a pickup that
/// cannot be taken, to be read from the game log afterwards.
fn dump_near(vm: &mut Vm<'_>, radius: f32) {
    use hp3_world::{get, physics::vec};
    let Some(pc) = hp3_world::world(vm).player else { return };
    let Some(pawn) = get(vm, pc, "Pawn").as_obj() else {
        vm.log.push("dumpnear: the player has no pawn".to_string());
        return;
    };
    let me = vec(&get(vm, pawn, "Location"));
    let head = format!("dumpnear: player {} at {me}, radius {radius}", hp3_script::natives::object_path(vm, pawn));
    vm.log.push(head);
    let mut near: Vec<(f32, hp3_script::ObjId)> = Vec::new();
    for a in hp3_world::world(vm).actors.clone() {
        if vm.db.objects.get(a).deleted || a == pawn {
            continue;
        }
        let d = (vec(&get(vm, a, "Location")) - me).length();
        if d <= radius {
            near.push((d, a));
        }
    }
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (d, a) in near {
        let path = hp3_script::natives::object_path(vm, a);
        let state = vm.state_of(a).map(|s| vm.db.names.get(vm.db.get(s).name).to_string()).unwrap_or_default();
        let name_of = |vm: &mut Vm<'_>, v: hp3_script::Value| v.as_obj().map(|o| hp3_script::natives::object_path(vm, o));
        let (base, owner) = (get(vm, a, "Base"), get(vm, a, "Owner"));
        let (base, owner) = (name_of(vm, base), name_of(vm, owner));
        let line = format!(
            "  {d:6.1} {path} at {} state {state} r{} h{} collideActors {} blockActors {} blockPlayers {} hidden {} phys {:?} vel {} base {base:?} owner {owner:?}",
            vec(&get(vm, a, "Location")),
            get(vm, a, "CollisionRadius").as_float(),
            get(vm, a, "CollisionHeight").as_float(),
            get(vm, a, "bCollideActors").as_bool(),
            get(vm, a, "bBlockActors").as_bool(),
            get(vm, a, "bBlockPlayers").as_bool(),
            get(vm, a, "bHidden").as_bool(),
            get(vm, a, "Physics").as_byte(),
            vec(&get(vm, a, "Velocity")),
        );
        vm.log.push(line);
        // An emitter actor: what each of its particle emitters draws (texture, draw style, opacity, first colour, size), for effects that
        // look wrong on screen.
        if let hp3_script::Value::Array(list) = get(vm, a, "Emitters") {
            for (i, e) in list.iter().enumerate() {
                let Some(eo) = e.as_obj() else { continue };
                let tex = get(vm, eo, "Texture").as_obj().map(|t| hp3_script::natives::object_path(vm, t)).unwrap_or_default();
                let class = vm.db.objects.get(eo).class.map(|c| vm.db.names.get(vm.db.get(c).name).to_string()).unwrap_or_default();
                let first_colour = match get(vm, eo, "ColorScale") {
                    hp3_script::Value::Array(c) => c.first().map(|x| format!("{:?}", x.fields().get(1).map(|f| f.fields().iter().map(hp3_script::Value::as_int).collect::<Vec<_>>()))).unwrap_or_default(),
                    _ => String::new(),
                };
                let size = format!("{:?}", get(vm, eo, "StartSizeRange").fields().iter().map(|r| r.fields().iter().map(hp3_script::Value::as_float).collect::<Vec<_>>()).collect::<Vec<_>>());
                let (style, opacity) = (get(vm, eo, "DrawStyle").as_byte(), get(vm, eo, "Opacity").as_float());
                let (disabled, max) = (get(vm, eo, "Disabled").as_bool(), get(vm, eo, "MaxParticles").as_int());
                let text = format!("        emitter {i} {class} texture {tex} drawstyle {style} opacity {opacity} disabled {disabled} maxparticles {max} colour0(BGRA) {first_colour} startsize {size}");
                vm.log.push(text);
            }
        }
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
        // Cauldron cakes (the cupcakes): the hero controller's own helper, as the vendor's `VI_CauldronCake.Purchased` adds them.
        "cakes" => {
            hp3_world::fire(vm, pc, "GiveCakes", &mut [Value::Int(10)]);
            eprintln!("cheat: 10 cauldron cakes");
        }
        // Pumpkin pasties: no helper exists, so they are added to the level's inventory carrier the way `VI_PumpkinPasty.Purchased` does.
        "pasties" => {
            let carrier = hp3_world::world(vm)
                .level_info
                .and_then(|li| hp3_world::get(vm, li, "InventoryCarrierPawn").as_obj())
                .or_else(|| hp3_world::get(vm, pc, "Pawn").as_obj());
            match (carrier, hp3_world::find_object(vm, "HGame.PumpkinPastyCollection")) {
                (Some(carrier), Some(class)) => {
                    hp3_world::fire(vm, carrier, "AddToInventoryCollection", &mut [Value::Object(Some(class)), Value::Int(10)]);
                    eprintln!("cheat: 10 pumpkin pasties");
                }
                _ => eprintln!("cheat: no pawn or no PumpkinPastyCollection class"),
            }
        }
        "beatboss" => {
            hp3_world::fire(vm, pc, "BeatBoss", &mut []);
            eprintln!("cheat: the boss is beaten");
        }
        "defeatpeeves" => {
            // Peeves of the current story state that has not been beaten goes straight to his defeated state, which sends the
            // event that plays the exit cutscene (a way to test it without the fight).
            let Some(peeves) = vm.db.find("Peeves") else { return };
            let actors = hp3_world::world(vm).actors.clone();
            let mut count = 0;
            for a in actors {
                let is_peeves = vm.db.objects.get(a).class.is_some_and(|c| vm.is_child_of(c, peeves));
                if is_peeves && hp3_world::get(vm, a, "bInCurrentGameState").as_bool() && !hp3_world::get(vm, a, "bHasBeenDefeated").as_bool() {
                    let state = vm.db.names.intern("stateDefeated");
                    let _ = vm.goto_state(a, state, hp3_script::Name::NONE);
                    count += 1;
                }
            }
            eprintln!("cheat: {count} Peeves defeated");
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
