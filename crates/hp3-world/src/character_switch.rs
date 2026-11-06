//! Switching the played character among Harry, Ron and Hermione at will (keys 1, 2, 3 and C on the PC, a touch
//! button on the phone). The retail game switches only where its triggers say; this hands the switch to the game's own
//! `KWPawn.SwitchControlToPawn`, the routine those triggers use, so companions, camera and HUD follow as they do there.

use hp3_script::{ObjId, Value, Vm};

use crate::{fire, get, world};

/// The three heroes, in the order C cycles through them.
const HEROES: [&str; 3] = ["Harry", "Ron", "Hermione"];

/// Which hero class (index into `HEROES`) an actor is, if any.
fn hero_of(vm: &Vm<'_>, a: ObjId) -> Option<usize> {
    HEROES.iter().position(|h| vm.db.find(h).is_some_and(|c| vm.db.objects.get(a).class.is_some_and(|k| vm.is_child_of(k, c))))
}

/// A hero pawn that can take over: in the level, drawn, and controlled by someone (a following companion), not a
/// placeholder the map keeps for cutscenes.
fn candidates(vm: &mut Vm<'_>) -> Vec<(usize, ObjId)> {
    let actors = world(vm).actors.clone();
    let mut out = Vec::new();
    for a in actors {
        if vm.db.objects.get(a).deleted {
            continue;
        }
        let Some(h) = hero_of(vm, a) else { continue };
        if get(vm, a, "bHidden").as_bool() || get(vm, a, "Controller").as_obj().is_none() {
            continue;
        }
        out.push((h, a));
    }
    out
}

/// Switches to `which` (0 Harry, 1 Ron, 2 Hermione), or to the next hero after the played one with `None`. Returns
/// what happened, for the log.
/// Levels played by one hero alone (the spell chapters and their exams, the hero-named flights): no switching there
/// (lower case map names).
const SINGLE_HERO_MAPS: [&str; 9] = [
    "hp3_ch1carperetractum",
    "hp3_ch1carperetractumb",
    "hp3_ch2draconifors",
    "hp3_ch2draconiforsb",
    "hp3_ch3glacius",
    "hp3_ch3glaciusb",
    "hp3_fa1ron",
    "hp3_fa2hermione",
    "hp3_fa3harry",
];

pub fn switch(vm: &mut Vm<'_>, which: Option<usize>) -> String {
    let map = world(vm).level_info.map(|li| hp3_script::natives::object_path(vm, li)).unwrap_or_default();
    let map = map.split('.').next().unwrap_or("").to_ascii_lowercase();
    if SINGLE_HERO_MAPS.contains(&map.as_str()) {
        return format!("one hero only on this level ({map})");
    }
    let Some(pc) = world(vm).player else { return "no player".into() };
    let Some(current) = get(vm, pc, "Pawn").as_obj() else { return "no pawn".into() };
    if get(vm, pc, "bInCutScene").as_bool() {
        return "in a cutscene".into();
    }
    let Some(now) = hero_of(vm, current) else { return "the played pawn is not Harry, Ron or Hermione".into() };
    let list = candidates(vm);
    let target = match which {
        Some(w) => list.iter().find(|(h, a)| *h == w && *a != current).map(|&(_, a)| a),
        None => (1..=HEROES.len()).map(|step| (now + step) % HEROES.len()).find_map(|h| list.iter().find(|(k, a)| *k == h && *a != current).map(|&(_, a)| a)),
    };
    let Some(target) = target else { return format!("no other hero here (playing {})", HEROES[now]) };
    // Whoever trailed the played hero (the companions) must trail the new one afterwards: the routine below only trades
    // the two pawns' controllers, so the controller that takes over the old pawn would keep its old lead and the rest would
    // keep following a pawn nobody plays (the companion that was switched to stood idle for good once switched back).
    let followers: Vec<ObjId> = list
        .iter()
        .map(|&(_, a)| a)
        .filter(|&a| a != current && a != target)
        .filter(|&a| get(vm, a, "Controller").as_obj().and_then(|c| get(vm, c, "LeadChar").as_obj()) == Some(current))
        .collect();
    fire(vm, current, "SwitchControlToPawn", &mut [Value::Str(String::new()), Value::Object(Some(target))]);
    let played = get(vm, pc, "Pawn").as_obj();
    if let Some(lead) = played {
        for follower in std::iter::once(current).chain(followers) {
            fire(vm, lead, "AddTrailingChar", &mut [Value::Object(Some(follower))]);
        }
    }
    let name = |vm: &Vm<'_>, o: Option<ObjId>| o.map(|o| hp3_script::natives::object_path(vm, o)).unwrap_or_default();
    format!("{} -> {}", name(vm, Some(current)), name(vm, played))
}
