//! `hp3-run items <HP3 folder> <first map> <steps>`: checks that what the player has collected survives level changes, saves
//! and loads. The steps are a comma-separated list:
//!
//! - `give`: the player is given beans, cauldron cakes, pumpkin pasties, wizard cards and shields (challenge stars), the way the
//!   game's own functions give them; what the player then has is what every later step must still show.
//! - `<map>`: the level asks to travel there with the player's items (`Level.ServerTravel(map, true)`, as a hub door or a
//!   cutscene's `ChangeLevel` does).
//! - `save`: the level is saved to a scratch slot; `load`: that slot is loaded in the same session (`OPEN ?load=`).
//! - `restart`: a new session (as after quitting the app) opens the front end and loads the slot from it.
//! - `newgame:<map>`: the front end starts a new game on that map (`ServerTravel(map, false)`): everything must be zero.
//!
//! After every step the counts are read the way the HUD and the menus read them (the level's inventory carrier's
//! `GetInventoryCount`, `WizardCardManager.GetFoundCardCount`, the shields found in `ChallengeStarManager.FoundStatus`) and
//! compared; a mismatch prints `ITEMCHECK FAIL` and the run exits with a failure.

use std::path::Path;
use std::process::ExitCode;

use hp3_package::PackageSet;
use hp3_script::{ClassDb, ObjId, Value, Vm};
use hp3_world::{fire, get, world};

/// The slot the check writes to (and removes again).
const SLOT: i32 = 97;

/// What is given by `give`.
const BEANS: i32 = 37;
const CAKES: i32 = 3;
const PASTIES: i32 = 2;
const CARDS: usize = 3;
const SHIELDS: usize = 2;

#[derive(Clone, Debug, Default, PartialEq)]
struct Counts {
    beans: i32,
    cakes: i32,
    pasties: i32,
    cards: i32,
    shields: i32,
}

fn new_vm(set: &PackageSet) -> Vm<'_> {
    let mut db = ClassDb::new(set);
    db.load_all_classes();
    Vm::new(db)
}

fn obj_name(vm: &Vm<'_>, o: ObjId) -> String {
    vm.db.names.get(vm.db.objects.get(o).name).to_string()
}

fn class_name(vm: &Vm<'_>, o: ObjId) -> String {
    vm.db.objects.get(o).class.map(|c| vm.db.names.get(vm.db.get(c).name).to_string()).unwrap_or_default()
}

/// The pawn whose inventory the HUD shows (`Level.InventoryCarrierPawn`), else the player's pawn.
fn carrier(vm: &mut Vm<'_>) -> Option<ObjId> {
    let li = world(vm).level_info?;
    get(vm, li, "InventoryCarrierPawn").as_obj().or_else(|| {
        let pc = world(vm).player?;
        get(vm, pc, "Pawn").as_obj()
    })
}

fn game_object(vm: &mut Vm<'_>, prop: &str) -> Option<ObjId> {
    let g = world(vm).game?;
    get(vm, g, prop).as_obj()
}

/// Where the `Found` flags sit in a `LevelFoundStatus` row of the star manager's table: first field and count.
fn found_fields(vm: &mut Vm<'_>) -> Option<(usize, usize)> {
    let s = vm.db.find("LevelFoundStatus")?;
    let found = vm.db.names.intern("Found");
    vm.db.get(s).props.iter().find(|p| p.name == found).map(|p| (p.slot, p.dim.max(1)))
}

/// The shields found: every `Found` flag set in the star manager's table (9 rows, one per challenge level).
fn shields_found(vm: &mut Vm<'_>, csm: ObjId) -> i32 {
    let Some((at, n)) = found_fields(vm) else { return -1 };
    let Some(first) = hp3_world::slot(vm, csm, "FoundStatus") else { return -1 };
    let Some(values) = vm.db.objects.values(csm) else { return -1 };
    let mut count = 0;
    for v in values.iter().skip(first).take(9) {
        if let Value::Struct(fields) = v {
            count += fields.iter().skip(at).take(n).filter(|f| f.as_int() != 0).count() as i32;
        }
    }
    count
}

fn counts(vm: &mut Vm<'_>) -> Counts {
    let mut c = Counts::default();
    if let Some(p) = carrier(vm) {
        for (name, out) in [("Jellybean", &mut c.beans), ("CauldronCake", &mut c.cakes), ("PumpkinPasty", &mut c.pasties)] {
            let n = vm.db.names.intern(name);
            *out = fire(vm, p, "GetInventoryCount", &mut [Value::Name(n)]).as_int();
        }
    }
    c.cards = game_object(vm, "WizardCardManager").map_or(-1, |m| fire(vm, m, "GetFoundCardCount", &mut []).as_int());
    c.shields = game_object(vm, "ChallengeStarManager").map_or(-1, |m| shields_found(vm, m));
    c
}

/// Who holds what: each pawn of interest with its inventory chain (class and count).
fn holders(vm: &mut Vm<'_>) -> String {
    let mut pawns = Vec::new();
    if let Some(p) = carrier(vm) {
        pawns.push(("carrier", p));
    }
    if let Some(pc) = world(vm).player
        && let Some(p) = get(vm, pc, "Pawn").as_obj()
    {
        pawns.push(("pawn", p));
    }
    let mut out = String::new();
    for (role, p) in pawns {
        out += &format!(" {role}={}[", obj_name(vm, p));
        let mut at = get(vm, p, "Inventory").as_obj();
        for _ in 0..64 {
            let Some(i) = at else { break };
            let n = if hp3_world::slot(vm, i, "iNumItems").is_some() { format!(":{}", get(vm, i, "iNumItems").as_int()) } else { String::new() };
            out += &format!("{}{n} ", class_name(vm, i));
            at = get(vm, i, "Inventory").as_obj();
        }
        out += "]";
    }
    out
}

/// Ticks until the level asks to switch, then switches; true when it did.
fn follow(vm: &mut Vm<'_>, max_ticks: u32) -> bool {
    for _ in 0..max_ticks {
        hp3_world::tick(vm, 1.0 / 60.0);
        if let Some(url) = hp3_world::take_travel(vm) {
            return match hp3_world::travel(vm, &url) {
                Ok(_) => true,
                Err(e) => {
                    println!("ITEMCHECK travel to {url} failed: {e}");
                    false
                }
            };
        }
    }
    false
}

fn settle(vm: &mut Vm<'_>, ticks: u32) {
    for _ in 0..ticks {
        hp3_world::tick(vm, 1.0 / 60.0);
        // A level that moves on by itself (a cutscene's ChangeLevel) is not followed here: the check controls the route.
        let _ = hp3_world::take_travel(vm);
    }
}

fn server_travel(vm: &mut Vm<'_>, url: &str, items: bool) -> bool {
    let Some(li) = world(vm).level_info else { return false };
    fire(vm, li, "ServerTravel", &mut [Value::Str(url.to_string()), Value::Bool(items)]);
    follow(vm, 600)
}

fn give(vm: &mut Vm<'_>) {
    let Some(pc) = world(vm).player else {
        println!("ITEMCHECK give: no player");
        return;
    };
    // Beans, cakes and pasties as every pickup gives them: `KWPawn.AddToInventoryCollection`, which always files them on the
    // level's inventory carrier (the controller's GiveBeans does the same through its pawn, which a level opened mid-cutscene lacks).
    if let Some(p) = carrier(vm) {
        for (class, n) in [("JellybeanCollection", BEANS), ("CauldronCakeCollection", CAKES), ("PumpkinPastyCollection", PASTIES)] {
            if let Some(class) = vm.db.find(class) {
                let class_obj = vm.db.get(class).object;
                fire(vm, p, "AddToInventoryCollection", &mut [Value::Object(Some(class_obj)), Value::Int(n)]);
            }
        }
    }
    // Cards: the first few the manager lists that are not owned yet, through MarkWizardCardAsOwned(class).
    if let Some(m) = game_object(vm, "WizardCardManager") {
        let wizard_card = vm.db.find("WizardCard");
        let cards = match get(vm, m, "Cards") {
            Value::Array(a) => a,
            _ => Vec::new(),
        };
        let mut given = 0;
        for card in cards {
            if given == CARDS {
                break;
            }
            let Value::Struct(fields) = card else { continue };
            let class_obj = fields.iter().find_map(|f| match f {
                Value::Object(Some(o)) => vm.class_of(*o).filter(|c| wizard_card.is_some_and(|w| vm.is_child_of(*c, w))).map(|_| *o),
                _ => None,
            });
            let owned = fields.iter().any(|f| matches!(f, Value::Byte(5)));
            if let (Some(class_obj), false) = (class_obj, owned) {
                fire(vm, m, "MarkWizardCardAsOwned", &mut [Value::Object(Some(class_obj))]);
                given += 1;
            }
        }
        if given < CARDS {
            println!("ITEMCHECK give: only {given} cards could be marked");
        }
    }
    // Shields: the first flags of the first level's row of the star manager's table.
    if let Some(m) = game_object(vm, "ChallengeStarManager")
        && let Some((at, n)) = found_fields(vm)
        && let Some(first) = hp3_world::slot(vm, m, "FoundStatus")
        && let Some(values) = vm.db.objects.values_mut(m)
        && let Some(Value::Struct(fields)) = values.get_mut(first)
    {
        let mut left = SHIELDS;
        for f in fields.iter_mut().skip(at).take(n) {
            if left > 0 && f.as_int() == 0 {
                *f = Value::Int(1);
                left -= 1;
            }
        }
    }
}

/// Opens the front end in a fresh VM, as the app does at start.
fn front_end<'a>(set: &'a PackageSet, root: &Path) -> Result<Vm<'a>, Box<dyn std::error::Error>> {
    let config = hp3_core::Config::load(root);
    let local = config.get("URL", "LocalMap").map(|m| m.trim_end_matches(".unr").to_string()).ok_or("no LocalMap")?;
    let mut vm = new_vm(set);
    let pkg = set.get(&local)?;
    hp3_world::load(&mut vm, &pkg, config)?;
    settle(&mut vm, 30);
    Ok(vm)
}

pub fn run(root: &Path, first: &str, steps: &str) -> Result<ExitCode, Box<dyn std::error::Error>> {
    let set = PackageSet::open(root)?;
    let mut vm = new_vm(&set);
    let pkg = set.get(first)?;
    hp3_world::load(&mut vm, &pkg, hp3_core::Config::load(root))?;
    settle(&mut vm, 60);
    let mut expected = counts(&mut vm);
    println!("ITEMCHECK start {first}: {expected:?}{}", holders(&mut vm));
    let mut failures = 0;
    let mut saved: Option<Counts> = None;
    for step in steps.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let ok = match step {
            "give" => {
                world(&mut vm).errors.clear();
                give(&mut vm);
                settle(&mut vm, 10);
                if std::env::var_os("HP3_ITEMCHECK_DEBUG").is_some() {
                    for e in world(&mut vm).errors.iter().take(20) {
                        println!("ITEMCHECK give error: {e}");
                    }
                    if let Some(m) = game_object(&mut vm, "ChallengeStarManager")
                        && let Some(first) = hp3_world::slot(&mut vm, m, "FoundStatus")
                        && let Some(values) = vm.db.objects.values(m)
                    {
                        println!("ITEMCHECK FoundStatus[0] = {:?}", values.get(first));
                    }
                    if let Some(pc) = world(&mut vm).player {
                        println!("ITEMCHECK controller class {}", class_name(&vm, pc));
                    }
                }
                let now = counts(&mut vm);
                let want = Counts {
                    beans: expected.beans + BEANS,
                    cakes: expected.cakes + CAKES,
                    pasties: expected.pasties + PASTIES,
                    cards: expected.cards + CARDS as i32,
                    shields: expected.shields + SHIELDS as i32,
                };
                if now != want {
                    println!("ITEMCHECK FAIL give: want {want:?} got {now:?}");
                    failures += 1;
                }
                expected = now;
                true
            }
            "save" => {
                hp3_world::save::delete(&vm, SLOT);
                hp3_world::save::console(&mut vm, &format!("SAVEGAME {SLOT}"));
                saved = Some(expected.clone());
                hp3_world::save::exists(&vm, SLOT)
            }
            // A load brings back what the player had when saving, whatever was collected since.
            "load" => {
                expected = saved.clone().unwrap_or_default();
                hp3_world::save::console(&mut vm, &format!("OPEN ?load={SLOT}"));
                follow(&mut vm, 5)
            }
            "restart" => {
                expected = saved.clone().unwrap_or_default();
                vm = front_end(&set, root)?;
                hp3_world::save::console(&mut vm, &format!("OPEN ?load={SLOT}"));
                follow(&mut vm, 5)
            }
            s if s.starts_with("newgame:") => {
                let map = &s["newgame:".len()..];
                // Back at the front end in the same session (what was played stays in memory), then New Game.
                let local = world(&mut vm).config.get("URL", "LocalMap").map(|m| m.trim_end_matches(".unr").to_string()).unwrap_or_default();
                let at_menu = server_travel(&mut vm, &local, false);
                expected = Counts::default();
                at_menu && server_travel(&mut vm, map, false)
            }
            map => server_travel(&mut vm, map, true),
        };
        if !ok {
            let li = world(&mut vm).level_info;
            let state = li.map(|li| {
                let next = get(&mut vm, li, "NextURL").as_str().to_string();
                (next, get(&mut vm, li, "NextSwitchCountdown").as_float(), get(&mut vm, li, "bLevelChange").as_bool())
            });
            println!("ITEMCHECK FAIL {step}: the step did not happen (level NextURL, countdown, bLevelChange: {state:?})");
            failures += 1;
            continue;
        }
        settle(&mut vm, 60);
        if std::env::var_os("HP3_ITEMCHECK_DEBUG").is_some() {
            let store = game_object(&mut vm, "PersistentStore");
            let keys = store.map(|s| get(&mut vm, s, "Keys"));
            let wi = world(&mut vm).world_info;
            let temp = wi.map(|w| get(&mut vm, w, "TempPersKeys"));
            let len = |v: &Option<Value>| match v {
                Some(Value::Array(a)) => a.len() as i32,
                _ => -1,
            };
            let wc: Vec<String> = match &keys {
                Some(Value::Array(a)) => a.iter().map(|k| k.as_str().to_string()).filter(|k| k.starts_with("WC_") || k.starts_with("CR")).take(8).collect(),
                _ => Vec::new(),
            };
            println!("ITEMCHECK debug store {:?} keys {} (cards/shields {wc:?}), worldinfo temp keys {}", store.map(|s| obj_name(&vm, s)), len(&keys), len(&temp));
        }
        let map = world(&mut vm).map.name.to_string();
        let now = counts(&mut vm);
        let held = holders(&mut vm);
        if now == expected {
            println!("ITEMCHECK ok   {step:>16} -> {map}: {now:?}{held}");
        } else {
            println!("ITEMCHECK FAIL {step:>16} -> {map}: want {expected:?} got {now:?}{held}");
            failures += 1;
        }
    }
    hp3_world::save::delete(&vm, SLOT);
    println!("ITEMCHECK {failures} failures");
    Ok(if failures == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}
