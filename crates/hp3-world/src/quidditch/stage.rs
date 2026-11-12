//! The loaded level's own gameplay, taken out of the way of a match.
//!
//! A match is played on `HP3_Quidditch`, which in the retail game is not a playable pitch at all: it is the
//! stage of the Dementor cutscene (`7010QuidDem_ALL`). Left alone, that cutscene takes the camera, flies four
//! Dementors and two black dogs over the pitch, fades the screen and, when it is done, switches the level to
//! `HP3_GroundsDada` -- which is exactly what ended the owner's match. So the first thing a match does is
//! clear the stage: the cutscene machinery and everything on the map that moves or attacks is destroyed, and
//! `count_down_travel` refuses the level switch for as long as the match runs.
//!
//! Everything here is reached from `glue::start` only, so a level that never starts a match is untouched.
//! This is a VM-touching module like `glue` and `presentation`; nothing pure lives in it.

use hp3_script::{ObjId, Value, Vm};

/// The cutscene machinery of the level: the scenes the factory built, the factory itself, the controllers that
/// drive a scene's pawns and camera, and the triggers that would start another scene. Named classes, matched
/// with their subclasses (`KWCutControllerII` is one of `KWCutController`), because a cutscene is pure script:
/// the engine has nothing of its own to switch off.
const CUT_CLASSES: [&str; 5] = ["KWCutScene", "KWCutController", "KWCutTextController", "CutFactory", "CutSceneTrigger"];

/// The cutscene's own cast: the Dementors, Sirius's black dog and the converging-Dementor effects. The crowd
/// in the stands (`HPGenericStudent`) and the two teams' players stay: they stand still and do nothing, and
/// they are the only audience the pitch has.
const STAGE_CLASSES: [&str; 3] = ["Dementor", "BlackDog", "DementorsConverging"];

/// Clears the level's cutscene and its cutscene-only cast, so the match is played over an empty stadium.
/// Returns the line to log: what was removed, by class.
pub fn clear(vm: &mut Vm<'_>) -> String {
    let mut gone: Vec<String> = Vec::new();
    for class in CUT_CLASSES.iter().chain(STAGE_CLASSES.iter()) {
        let n = remove_class(vm, class);
        if n > 0 {
            gone.push(format!("{n} {class}"));
        }
    }
    if gone.is_empty() {
        return "quidditch: the level has no cutscene and no cutscene cast, nothing to clear".into();
    }
    format!("quidditch: the level's own scripts cleared for the match: {}", gone.join(", "))
}

/// Destroys every actor of `name`'s class (and its subclasses), with the controller of each one. Returns how
/// many went.
fn remove_class(vm: &mut Vm<'_>, name: &str) -> usize {
    let Some(class) = vm.db.find(name) else { return 0 };
    let actors = crate::world(vm).actors.clone();
    let player = crate::world(vm).player;
    let player_pawn = player.and_then(|pc| crate::get(vm, pc, "Pawn").as_obj());
    let mut gone = 0;
    for a in actors {
        if vm.db.objects.get(a).deleted || Some(a) == player || Some(a) == player_pawn {
            continue;
        }
        if !vm.db.objects.get(a).class.is_some_and(|c| vm.is_child_of(c, class)) {
            continue;
        }
        // A pawn's controller goes with the pawn: nothing in the engine clears references to a destroyed
        // actor, so a controller left behind would steer a deleted pawn every tick of the match.
        let controller = crate::get(vm, a, "Controller").as_obj().filter(|&c| Some(c) != player);
        if remove(vm, a) {
            gone += 1;
            if let Some(c) = controller {
                remove(vm, c);
            }
        }
    }
    gone
}

/// Destroys one of the level's actors, scenery flags and all (`glue::stop` does the same with the match's own:
/// `destroy_actor` refuses a `bStatic` or `bNoDelete` actor).
fn remove(vm: &mut Vm<'_>, a: ObjId) -> bool {
    if vm.db.objects.get(a).deleted {
        return false;
    }
    crate::set(vm, a, "bStatic", Value::Bool(false));
    crate::set(vm, a, "bNoDelete", Value::Bool(false));
    crate::destroy_actor(vm, a)
}
