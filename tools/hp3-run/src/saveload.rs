//! `HP3_SAVELOAD=<tick>`: a save and load round trip in the middle of a level run. At that tick the level is written to a
//! scratch slot and loaded back the way the game's Load does (the map is loaded afresh and the slot put over it); what
//! changed between the level before and the level after is printed: actors that vanished or appeared, and actors whose
//! look or behaviour (hidden, collision, mesh, draw type, state, place) differs, and the persistent `WorldInfo` values.

use std::collections::BTreeMap;

use hp3_script::{ObjId, Value, Vm};

/// The slot the test writes to (and removes again).
const SLOT: i32 = 98;

/// One line per actor: what a player would notice.
fn snapshot(vm: &mut Vm<'_>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let actors: Vec<ObjId> = hp3_world::world(vm).actors.clone();
    for a in actors {
        if vm.db.objects.get(a).deleted {
            continue;
        }
        let name = vm.db.names.get(vm.db.objects.get(a).name).to_string();
        let class = vm.db.objects.get(a).class.map(|c| vm.db.names.get(vm.db.get(c).name).to_string()).unwrap_or_default();
        let state = vm.state_of(a).map(|s| vm.db.names.get(vm.db.get(s).name).to_string()).unwrap_or_default();
        let mesh = hp3_world::get(vm, a, "StaticMesh").as_obj().or_else(|| hp3_world::get(vm, a, "Mesh").as_obj()).map(|m| hp3_script::natives::object_path(vm, m)).unwrap_or_default();
        let flag = |vm: &mut Vm<'_>, n: &str| u8::from(hp3_world::get(vm, a, n).as_bool());
        let (hidden, collide, block) = (flag(vm, "bHidden"), flag(vm, "bCollideActors"), flag(vm, "bBlockActors"));
        let draw = hp3_world::get(vm, a, "DrawType").as_byte();
        let phys = hp3_world::get(vm, a, "Physics").as_byte();
        let at = hp3_world::physics::vec(&hp3_world::get(vm, a, "Location"));
        out.insert(
            name,
            format!("{class} state={state} mesh={mesh} hidden={hidden} collide={collide} block={block} draw={draw} phys={phys} at=({:.0},{:.0},{:.0})", at.x, at.y, at.z),
        );
    }
    out
}

/// The values of the persistent game state object, one line each.
fn world_info(vm: &mut Vm<'_>) -> Vec<String> {
    match hp3_world::world(vm).world_info {
        Some(w) => vm.db.objects.values(w).map(|v| v.iter().map(|x| format!("{x:?}")).collect()).unwrap_or_default(),
        None => Vec::new(),
    }
}

fn shown(v: &Value) -> String {
    let s = format!("{v:?}");
    if s.len() > 160 { format!("{}...", &s[..160]) } else { s }
}

/// Saves the level, loads it back and prints the differences. `map` is the level's name.
pub fn round_trip(vm: &mut Vm<'_>, map: &str) {
    let before = snapshot(vm);
    let info_before = world_info(vm);
    // The values of the player's controller, which hold what the player has done (counters, collected things).
    let pc = hp3_world::world(vm).player;
    let controller_before: Vec<String> = pc.and_then(|p| vm.db.objects.values(p).map(|v| v.iter().map(shown).collect())).unwrap_or_default();
    hp3_world::save::console(vm, &format!("SAVEGAME {SLOT}"));
    let url = format!("{map}?load={SLOT}");
    if let Err(e) = hp3_world::travel(vm, &url) {
        println!("saveload: loading {url} failed: {e}");
        return;
    }
    hp3_world::save::delete(vm, SLOT);
    let after = snapshot(vm);
    let info_after = world_info(vm);
    println!("saveload: {} actors before, {} after", before.len(), after.len());
    let mut shown_n = 0;
    for (name, line) in &before {
        match after.get(name) {
            None => {
                if shown_n < 40 {
                    println!("  GONE    {name}: {line}");
                }
                shown_n += 1;
            }
            Some(a) if a != line => {
                if shown_n < 40 {
                    println!("  CHANGED {name}:\n      was {line}\n      now {a}");
                }
                shown_n += 1;
            }
            _ => {}
        }
    }
    for (name, line) in &after {
        if !before.contains_key(name) {
            if shown_n < 40 {
                println!("  NEW     {name}: {line}");
            }
            shown_n += 1;
        }
    }
    println!("saveload: {shown_n} differences");
    let changed = info_before.iter().zip(&info_after).enumerate().filter(|(_, (a, b))| a != b).count();
    println!("saveload: WorldInfo {} values before, {} after, {changed} differ", info_before.len(), info_after.len());
    for (i, (a, b)) in info_before.iter().zip(&info_after).enumerate().filter(|(_, (a, b))| a != b).take(12) {
        println!("  worldinfo[{i}] was {a}\n             now {b}");
    }
    let pc = hp3_world::world(vm).player;
    let controller_after: Vec<String> = pc.and_then(|p| vm.db.objects.values(p).map(|v| v.iter().map(shown).collect())).unwrap_or_default();
    let cdiff = controller_before.iter().zip(&controller_after).enumerate().filter(|(_, (a, b))| a != b).count();
    println!("saveload: player controller {} values, {cdiff} differ", controller_before.len());
    if let Some(class) = pc.and_then(|p| vm.db.objects.get(p).class) {
        let names: BTreeMap<usize, String> = vm.db.get(class).props.iter().map(|p| (p.slot, vm.db.names.get(p.name).to_string())).collect();
        for (i, (a, b)) in controller_before.iter().zip(&controller_after).enumerate().filter(|(_, (a, b))| a != b).take(12) {
            println!("  controller.{} was {a}\n  {:w$} now {b}", names.get(&i).map_or("?", String::as_str), "", w = names.get(&i).map_or(1, |n| n.len() + 11));
        }
    }
}
