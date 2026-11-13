//! The Quidditch captain: a Gryffindor rider in uniform who stands in the Grounds hub, a little off the way in, and offers
//! the player the next match of the league. The story has no Quidditch of its own to hook a match to, so this is the in-game
//! door to one that is *in* the world: a person to walk up to, instead of a menu button.
//!
//! This module only puts the person in the level and says how far the player is from them; the talking (a speech box with
//! YES and NO, the key to start it) belongs to the app, which asks `near` each frame and sends the `quidditch` command when
//! the player says yes.

use std::cell::Cell;

use glam::Vec3;
use hp3_script::{ObjId, Value, Vm};

/// The one level the captain stands in.
pub const MAP: &str = "HP3_GroundsHub";
/// A Gryffindor rider, as the retail map places them for the cutscene: a skeletal mesh in the team's robes and gear.
const CLASS: &str = "HGame.QuidPlayerGriffMale";
/// Where the captain stands (world units, the hub's own coordinates): by the stone bench near the castle wall, the place the user
/// picked in game (F4 screenshot of 2026-11-14, Harry's own position there). The floor is found under this point.
const SPOT: Vec3 = Vec3::new(-1223.56, -5147.68, 870.03);
/// The yaw (65536ths of a turn) the captain faces: the way the player faced on that spot (49545).
const YAW: i32 = 49545 - 65536;
/// Where the captain stood before: a save made then holds him there, and a load moves him to `SPOT`.
const OLD_SPOTS: [Vec3; 1] = [Vec3::new(-700.0, -6740.0, 900.0)];
/// The `Tag` the captain carries, so a load finds him wherever the save had him.
const TAG: &str = "QuidditchCaptain";
/// How close the player has to be to talk.
pub const TALK_RANGE: f32 = 170.0;

thread_local! {
    static CAPTAIN: Cell<Option<ObjId>> = const { Cell::new(None) };
}

fn v3(v: Vec3) -> Value {
    Value::Struct(vec![Value::Float(v.x), Value::Float(v.y), Value::Float(v.z)].into())
}

fn rot(r: [i32; 3]) -> Value {
    Value::Struct(vec![Value::Int(r[0]), Value::Int(r[1]), Value::Int(r[2])].into())
}

/// Where the captain stands: `SPOT`, or `HP3_CAPTAIN_AT=x,y,z` (to try other places).
fn spot() -> Vec3 {
    std::env::var("HP3_CAPTAIN_AT")
        .ok()
        .and_then(|v| {
            let n: Vec<f32> = v.split(',').filter_map(|p| p.trim().parse().ok()).collect();
            (n.len() == 3).then(|| Vec3::new(n[0], n[1], n[2]))
        })
        .unwrap_or(SPOT)
}

/// Puts the captain into the loaded level when it is the hub (called once the level has begun). Any other level leaves none.
pub fn spawn(vm: &mut Vm<'_>) {
    CAPTAIN.with(|c| c.set(None));
    if !crate::world(vm).map.name.eq_ignore_ascii_case(MAP) {
        return;
    }
    let Some(class) = crate::find_object(vm, CLASS) else { return };
    let spot = spot();
    let args = [Value::Object(Some(class)), Value::Object(None), Value::Empty, v3(spot), rot([0, YAW, 0])];
    let Some(captain) = crate::natives::spawn(vm, None, &args).ok().and_then(|v| v.as_obj()) else { return };
    CAPTAIN.with(|c| c.set(Some(captain)));
    let tag = vm.db.names.intern(TAG);
    crate::set(vm, captain, "Tag", Value::Name(tag));
    stand(vm, captain);
    let at = crate::physics::vec(&crate::get(vm, captain, "Location"));
    let line = format!("quidditch: the captain stands in {MAP} at ({:.0}, {:.0}, {:.0})", at.x, at.y, at.z);
    println!("{line}");
    vm.log.push(line);
}

/// After a saved level was put over the hub (a quick load, the way back from a match): the captain the load spawned may have been
/// dropped for the one in the save, or both may stand, and a save from before the captain moved has him at an old spot. Every
/// captain (one carrying `TAG`, or a rider standing on his spot or an old one) is gathered; the one nearest his spot is kept and put
/// on it, any other is removed. With none left (a save from before the captain existed) a new one is spawned.
pub fn relink(vm: &mut Vm<'_>) {
    if !crate::world(vm).map.name.eq_ignore_ascii_case(MAP) {
        CAPTAIN.with(|c| c.set(None));
        return;
    }
    let Some(class) = vm.db.find(CLASS.rsplit('.').next().unwrap_or(CLASS)) else { return };
    let spot = spot();
    let tag = vm.db.names.intern(TAG);
    let player_pawn = crate::world(vm).player.and_then(|pc| crate::get(vm, pc, "Pawn").as_obj());
    let riders: Vec<ObjId> = crate::world(vm)
        .actors
        .clone()
        .into_iter()
        .filter(|&a| !vm.db.objects.get(a).deleted && Some(a) != player_pawn && vm.db.objects.get(a).class == Some(class))
        .collect();
    // The hub's own riders (in the Quidditch cutscene's places) carry no tag and stand elsewhere: they are left alone. Distances are
    // across the ground only: the captain stands on the floor found under his spot, below it.
    let mut found: Vec<(f32, ObjId)> = Vec::new();
    for a in riders {
        let at = crate::physics::vec(&crate::get(vm, a, "Location")).truncate();
        let near = |p: Vec3| at.distance(p.truncate()) < 300.0;
        if crate::get(vm, a, "Tag").as_name() == tag || near(spot) || OLD_SPOTS.iter().any(|&p| near(p)) {
            found.push((at.distance(spot.truncate()), a));
        }
    }
    found.sort_by(|a, b| a.0.total_cmp(&b.0));
    let Some(&(_, keep)) = found.first() else {
        spawn(vm);
        return;
    };
    for &(_, extra) in &found[1..] {
        crate::destroy_actor(vm, extra);
    }
    // Back on his spot when the save had him elsewhere (from before he moved), standing on its floor.
    if found[0].0 > 50.0 {
        crate::set(vm, keep, "Location", v3(spot));
        crate::set(vm, keep, "Rotation", rot([0, YAW, 0]));
        crate::physics::lift_out_of_floor(vm, keep);
    }
    crate::set(vm, keep, "Tag", Value::Name(tag));
    CAPTAIN.with(|c| c.set(Some(keep)));
    stand(vm, keep);
}

/// How far the player's pawn is from the captain; None when there is no captain or no player. No match is running when this is
/// asked (a match removes the captain's level).
pub fn distance(vm: &mut Vm<'_>) -> Option<f32> {
    let captain = CAPTAIN.with(|c| c.get())?;
    if vm.db.objects.get(captain).deleted {
        return None;
    }
    if std::env::var_os("HP3_CAPTAIN_DEBUG").is_some() && crate::world(vm).frame % 120 == 0 {
        let pc = crate::world(vm).player;
        let pawn = pc.map(|p| crate::get(vm, p, "Pawn").as_obj());
        eprintln!("captain: deleted {} player {pc:?} pawn {pawn:?}", vm.db.objects.get(captain).deleted);
    }
    let pawn = crate::world(vm).player.and_then(|pc| crate::get(vm, pc, "Pawn").as_obj())?;
    let (a, b) = (crate::physics::vec(&crate::get(vm, captain, "Location")), crate::physics::vec(&crate::get(vm, pawn, "Location")));
    Some(a.distance(b))
}

/// Whether the player is close enough to talk to the captain.
pub fn near(vm: &mut Vm<'_>) -> bool {
    if std::env::var_os("HP3_CAPTAIN_DEBUG").is_some() && crate::world(vm).frame % 120 == 0 {
        eprintln!("captain distance {:?}", distance(vm));
    }
    distance(vm).is_some_and(|d| d <= TALK_RANGE)
}

/// Poses the captain tries to stand in, best first: the first one his animation set has wins.
const STAND_ANIMS: [&str; 6] = ["Stand", "Idle", "idle_stand", "LookAround", "Idle01", "Walk_Idle"];

/// Puts the captain in a standing pose (the rider's rest pose has them seated on the broom).
fn stand(vm: &mut Vm<'_>, captain: ObjId) {
    let Some(set) = crate::anim::set_of(vm, captain) else { return };
    if std::env::var_os("HP3_CAPTAIN_DEBUG").is_some() {
        let names: Vec<&str> = set.sequences.iter().map(|s| s.name.as_str()).take(60).collect();
        eprintln!("captain: animation set holds {names:?}");
    }
    let Some(found) = STAND_ANIMS.iter().find(|a| set.find(a).is_some()).copied() else { return };
    let name = vm.db.names.intern(found);
    crate::anim::play(vm, captain, name, 1.0, 0.2, 0, true);
}
