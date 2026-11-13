//! Things the engine hands the player outside the game's own scripts (the debug cheats, a won Quidditch match), given the
//! way the game's own vendors give them.

use hp3_script::{Value, Vm};

/// What the level's inventory carrier holds of each collection (beans, cakes, pasties, ...): `Class=count` pairs, from each
/// item's `iNumItems`, as `KWPawn.AddToInventoryCollection` files them. For the log (to see a gift arrive).
pub fn collection_counts(vm: &mut Vm<'_>) -> String {
    let Some(carrier) = crate::world(vm).level_info.and_then(|li| crate::get(vm, li, "InventoryCarrierPawn").as_obj()) else {
        return "no inventory carrier".to_string();
    };
    let Some(collection) = vm.db.find("KWInventoryCollection") else { return "no collection class".to_string() };
    let mut out = Vec::new();
    let mut at = crate::get(vm, carrier, "Inventory").as_obj();
    for _ in 0..256 {
        let Some(a) = at else { break };
        if let Some(c) = vm.db.objects.get(a).class
            && vm.is_child_of(c, collection)
        {
            let name = vm.db.names.get(vm.db.get(c).name).to_string();
            out.push(format!("{name}={}", crate::get(vm, a, "iNumItems").as_int()));
        }
        at = crate::get(vm, a, "Inventory").as_obj();
    }
    let who = hp3_script::natives::object_path(vm, carrier);
    format!("{who}: {}", if out.is_empty() { "nothing".to_string() } else { out.join(", ") })
}

/// Adds `count` pumpkin pasties to the player's inventory. The hero controller has no helper for them (it has `GiveBeans` and
/// `GiveCakes`), so they go to the level's inventory carrier the way `VI_PumpkinPasty.Purchased` adds them. False when there
/// is no carrier or no pasty class.
pub fn give_pasties(vm: &mut Vm<'_>, count: i32) -> bool {
    let Some(pc) = crate::world(vm).player else { return false };
    let carrier = crate::world(vm)
        .level_info
        .and_then(|li| crate::get(vm, li, "InventoryCarrierPawn").as_obj())
        .or_else(|| crate::get(vm, pc, "Pawn").as_obj());
    match (carrier, crate::find_object(vm, "HGame.PumpkinPastyCollection")) {
        (Some(carrier), Some(class)) => {
            crate::fire(vm, carrier, "AddToInventoryCollection", &mut [Value::Object(Some(class)), Value::Int(count)]);
            true
        }
        _ => false,
    }
}
