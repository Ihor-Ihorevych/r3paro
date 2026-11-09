//! `HP3_CAST=<SpellClass>:<TargetClass>[:<n>][:<distance>]@<tick>`: puts Harry in front of the n-th actor of a
//! class, aims the game's camera at it, picks the spell on his wand and holds fire like a player would, then
//! reports what the target did. A test for "everything that can be enchanted can be disenchanted".

use std::collections::{BTreeMap, HashSet};

use glam::Vec3;
use hp3_script::{ObjId, Value, Vm};
use crate::physics::vec;
use crate::{fire, get, set};

/// The tick the scripted cast began (0 until it does), so screenshots can be taken at offsets from it.
pub static START_TICK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// `HP3_CAST_CAM=<distance>[,<height>]`: where the game should put its camera (position, pitch/yaw/roll) to look at the cast's target close up.
pub static CAMERA: std::sync::Mutex<Option<(Vec3, [i32; 3])>> = std::sync::Mutex::new(None);

pub struct CastTest {
    spell: String,
    class: String,
    index: usize,
    dist: f32,
    at: u32,
    target: Option<ObjId>,
    before: String,
    start_actors: HashSet<ObjId>,
    start_loc: Vec3,
    /// Every state the target went through after the cast, with the tick (from the cast) it entered it.
    history: Vec<(u32, String)>,
    report_at: u32,
    waited: u32,
    /// Where the player's pawn was put, and which pawn it was.
    caster: Option<(ObjId, Vec3)>,
    /// HP3_CAST_FLAME: the trigger the dragon was sent to burn, the tick it began, and what the trigger did.
    flame: Option<ObjId>,
    flame_from: u32,
    flame_pickup: u32,
    flame_calm: u32,
    flame_history: Vec<(u32, String)>,
}

impl CastTest {
    pub fn from_env() -> Option<CastTest> {
        let spec = std::env::var("HP3_CAST").ok()?;
        let (what, at) = spec.split_once('@')?;
        let mut parts = what.split(':');
        Some(CastTest {
            spell: parts.next()?.to_string(),
            class: parts.next()?.to_string(),
            index: parts.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            dist: parts.next().and_then(|v| v.parse().ok()).unwrap_or(260.0),
            at: at.parse().ok()?,
            target: None,
            before: String::new(),
            start_actors: Default::default(),
            start_loc: Vec3::ZERO,
            history: Vec::new(),
            waited: 0,
            caster: None,
            flame: None,
            flame_from: 0,
            flame_pickup: 0,
            flame_calm: 0,
            flame_history: Vec::new(),
            report_at: std::env::var("HP3_CAST_REPORT").ok().and_then(|v| v.parse().ok()).unwrap_or(260),
        })
    }
}

fn state_name(vm: &mut Vm<'_>, a: ObjId) -> String {
    vm.state_of(a).map(|s| vm.db.names.get(vm.db.get(s).name).to_string()).unwrap_or_else(|| "-".into())
}

/// A creature's state together with its controller's (where its behaviour usually lives).
fn behaviour(vm: &mut Vm<'_>, a: ObjId) -> String {
    let own = state_name(vm, a);
    match get(vm, a, "Controller").as_obj() {
        Some(c) => format!("{own}/{}", state_name(vm, c)),
        None => own,
    }
}

fn name_of(vm: &mut Vm<'_>, a: ObjId) -> String {
    vm.db.names.get(vm.db.objects.get(a).name).to_string()
}

fn rot(p: i32, y: i32) -> Value {
    Value::Struct(vec![Value::Int(p), Value::Int(y), Value::Int(0)].into())
}

fn v3(v: Vec3) -> Value {
    Value::Struct(vec![Value::Float(v.x), Value::Float(v.y), Value::Float(v.z)].into())
}

/// One step of the test, called before each tick.
pub fn step(vm: &mut Vm<'_>, t: &mut CastTest, frame: u32) {
    if frame == 1
        && let Some(id) = std::env::var("HP3_PATH_OF").ok().and_then(|v| v.parse::<u32>().ok())
    {
        println!("cast: object {id} is {}", hp3_script::natives::object_path(vm, ObjId(id)));
    }
    if frame < t.at {
        return;
    }
    let r = frame - t.at;
    if t.target.is_none() {
        // Wait until the player has control (opening cutscenes and loading screens are over).
        let ready = crate::world(vm).player.is_some_and(|pc| {
            get(vm, pc, "Pawn").as_obj().is_some() && matches!(state_name(vm, pc).as_str(), "PlayerWalking" | "StateNoPawnMoveCanTurn")
        });
        if !ready {
            if frame % 600 == 0 {
                println!("cast: waiting for the player to have control (tick {frame})");
            }
            t.at += 1;
            return;
        }
    }
    let r = if t.target.is_none() { 0 } else { r };
    if t.target.is_none() {
        t.at = frame;
    }
    if t.target.is_none() {
        println!("cast: starting at tick {frame}");
        START_TICK.store(frame, std::sync::atomic::Ordering::Relaxed);
    }
    let Some(pc) = crate::world(vm).player else { return };
    if t.target.is_some() {
        drive_flame(vm, t, pc, r);
    }
    let Some(pawn) = get(vm, pc, "Pawn").as_obj() else {
        // The player may have handed control to something else (a possessed creature): keep watching the target.
        if let Some(target) = t.target {
            let now_state = state_name(vm, target);
            if t.history.last().is_none_or(|(_, s)| *s != now_state) {
                t.history.push((r, now_state));
            }
            if r % 120 == 0 {
                let view = get(vm, pc, "ViewTarget").as_obj().map(|v| name_of(vm, v));
                println!("cast: r={r} the player has no pawn (controller state {}, view target {view:?})", state_name(vm, pc));
            }
            if r == t.report_at {
                finish(vm, t, target);
            }
        }
        return;
    };
    if t.target.is_none() {
        place(vm, t, pc, pawn);
        if std::env::var_os("HP3_CAST_SPAWNTEST").is_some()
            && let Some(c) = vm.db.find(&t.spell).map(|s| vm.db.get(s).object)
            && let Ok(Value::Object(Some(sp))) = crate::natives::spawn(vm, None, &[Value::Object(Some(c))])
        {
            println!("cast: spawn test {} speed {:?}", name_of(vm, sp), get(vm, sp, "Speed"));
            if let Some(cls) = vm.db.find(&t.spell) {
                let n = vm.db.names.intern("Speed");
                let slot = vm.db.get(cls).prop(n).map(|p| p.slot);
                println!("cast: class default slot {slot:?} value {:?} slots {} path {}", slot.and_then(|i| vm.db.get(cls).defaults.get(i).cloned()), vm.db.get(cls).slots, hp3_script::natives::object_path(vm, vm.db.get(cls).object));
            }
        }
    }
    let Some(target) = t.target else { return };
    // HP3_CAST_WAIT=1: an activated creature is only a target once it is vulnerable; wait for that.
    if std::env::var_os("HP3_CAST_WAIT").is_some() && t.waited < 2400 && get(vm, target, "vulnerableToClass").as_obj().is_none() {
        t.waited += 1;
        let now_state = behaviour(vm, target);
        if t.waited % 300 == 1 {
            let latent = format!("{:?}", vm.latent.get(&target));
            let vuln = get(vm, target, "vulnerableToClass").as_obj().map(|c| name_of(vm, c));
            println!(
                "cast: waiting, target {} state {now_state} latent {latent} hidden {} proj {} vuln {vuln:?} loc {}",
                name_of(vm, target),
                get(vm, target, "bHidden").as_bool(),
                get(vm, target, "bProjTarget").as_bool(),
                vec(&get(vm, target, "Location"))
            );
        }
        let tagged = format!("{now_state}(waiting)");
        if t.history.last().is_none_or(|(_, s)| *s != tagged) {
            t.history.push((0, tagged));
        }
        t.at += 1;
        return;
    }
    // Keep the camera on the target while the cursor settles.
    let aim_end = std::env::var("HP3_CAST_AIM").ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or(110);
    if (30..aim_end).contains(&r)
        && let Some(cam) = get(vm, pc, "Camera").as_obj()
    {
        let from = vec(&get(vm, cam, "Location"));
        let to = vec(&get(vm, target, "Location")) + Vec3::Z * 10.0;
        let d = to - from;
        let k = 65536.0 / std::f32::consts::TAU;
        let yaw = (d.y.atan2(d.x) * k).round() as i32;
        let pitch = (d.z.atan2(d.truncate().length()) * k).round() as i32;
        for name in ["rDestRotation", "rCurrRotation"] {
            set(vm, cam, name, rot(pitch, yaw));
        }
        set(vm, pc, "Rotation", rot(pitch, yaw));
    }
    // HP3_CAST_CAM: a close-up camera on the target, on the side Harry is.
    if let Ok(spec) = std::env::var("HP3_CAST_CAM") {
        let mut it = spec.split(',').filter_map(|v| v.parse::<f32>().ok());
        let (dist, height) = (it.next().unwrap_or(160.0), it.next().unwrap_or(60.0));
        let at = vec(&get(vm, target, "Location"));
        let from_harry = (vec(&get(vm, pawn, "Location")) - at).truncate().normalize_or_zero();
        let side = if from_harry == glam::Vec2::ZERO { glam::Vec2::X } else { from_harry };
        let eye = at + Vec3::new(-side.y * dist, side.x * dist, height);
        let d = at + Vec3::Z * 20.0 - eye;
        let k = 65536.0 / std::f32::consts::TAU;
        let rotation = [(d.z.atan2(d.truncate().length()) * k).round() as i32, (d.y.atan2(d.x) * k).round() as i32, 0];
        *CAMERA.lock().unwrap() = Some((eye, rotation));
    }
    let now_state = behaviour(vm, target);
    if t.history.last().is_none_or(|(_, s)| *s != now_state) {
        t.history.push((r, now_state.clone()));
    }
    // HP3_CAST_PROPS=A,B: print these properties of the target (and of the caster as "pawn.X") every 60 ticks (HP3_CAST_EVERY=n changes that).
    let every = std::env::var("HP3_CAST_EVERY").ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or(60);
    if r >= 100 && r % every == 0
        && let Ok(names) = std::env::var("HP3_CAST_PROPS")
    {
        let mut line = format!("cast: r={r} {now_state}");
        for n in names.split(',') {
            let (mut who, path) = match n.strip_prefix("pawn.") {
                Some(p) => (pawn, p),
                None => (target, n),
            };
            // `bell.Location`: follow the object properties before the last name.
            let mut parts: Vec<&str> = path.split('.').collect();
            let prop = parts.pop().unwrap_or(path);
            for hop in parts {
                match get(vm, who, hop).as_obj() {
                    Some(next) => who = next,
                    None => break,
                }
            }
            let v = get(vm, who, prop);
            let text = match &v {
                Value::Float(f) => format!("{f:.2}"),
                Value::Struct(_) => format!("{}", vec(&v)),
                other => format!("{other:?}"),
            };
            line.push_str(&format!(" {n}={text}"));
        }
        println!("{line}");
    }
    if r == 40 {
        let wand = get(vm, pawn, "Weapon").as_obj();
        let class = vm.db.find(&t.spell).map(|s| vm.db.get(s).object);
        println!("cast: wand {:?}, spell class found {}", wand.map(|w| name_of(vm, w)), class.is_some());
        if let (Some(w), Some(c)) = (wand, class) {
            fire(vm, w, "ChooseSpell", &mut [Value::Object(Some(c)), Value::Bool(true)]);
            let now = get(vm, w, "CurrentSpellClass").as_obj().map(|c| name_of(vm, c));
            println!("cast: wand now holds {now:?}");
        }
    }
    if r == 45 && std::env::var_os("HP3_CAST_GESTURE").is_some() {
        // Gives the first spell gesture the spell's icon, as the cursor does when it locks on.
        if let (Some(g), Some(spell)) = (actors_of(vm, "SpellGesture").first().copied(), vm.db.find(&t.spell)) {
            let n = vm.db.names.intern("SpellIcon");
            let icon = vm.db.get(spell).props.iter().find(|p| p.name == n).and_then(|p| vm.db.get(spell).defaults.get(p.slot).cloned()).unwrap_or_default();
            println!("cast: gesture icon {icon:?}");
            fire(vm, g, "ChangeGesture", &mut [icon]);
            if let Value::Array(list) = get(vm, g, "Emitters")
                && let Some(e) = list.first().and_then(Value::as_obj)
            {
                println!("cast: gesture emitter 0 texture now {:?}", get(vm, e, "Texture").as_obj().map(|t| hp3_script::natives::object_path(vm, t)));
            }
        }
    }
    // HP3_CAST_NOFIRE=1: Harry does not cast himself (with HP3_CAST_DIRECT only the direct hit lands).
    if r == 50 && std::env::var_os("HP3_CAST_NOFIRE").is_none() {
        crate::world(vm).input.key_down("LeftMouse");
    }
    // HP3_CAST_RELEASE=<r>: lets go of the fire button at that tick (otherwise Harry keeps casting).
    if std::env::var("HP3_CAST_RELEASE").ok().and_then(|v| v.parse::<u32>().ok()) == Some(r) {
        crate::world(vm).input.key_up("LeftMouse");
    }
    if std::env::var_os("HP3_CAST_FADE").is_some() && r >= 95 && r % 10 == 0 {
        for f in actors_of(vm, "FadeActorDelegate") {
            let (n, s, d, h) = (name_of(vm, f), state_name(vm, f), vm.db.objects.get(f).deleted, get(vm, f, "bHidden").as_bool());
            let target = get(vm, f, "ActorToFade").as_obj().map(|o| name_of(vm, o));
            let (cur, total, start, end, fading) = (get(vm, f, "CurTime"), get(vm, f, "FadeTime"), get(vm, f, "FadeStart"), get(vm, f, "FadeEnd"), get(vm, f, "bIsFading"));
            println!("cast: r={r} fade delegate {n} state {s} deleted {d} bHidden {h} target {target:?} cur {cur:?} total {total:?} start {start:?} end {end:?} fading {fading:?}");
        }
    }
    // HP3_CAST_DIRECT=1: the spell is born on the target instead of flying there, which exercises the target's
    // reaction without depending on the geometry between Harry and it.
    // HP3_CAST_DIRECT=<n>: n spells, 120 ticks apart.
    let direct = std::env::var("HP3_CAST_DIRECT").ok().map(|v| v.parse::<u32>().unwrap_or(1).max(1));
    if direct.is_some_and(|n| r >= 100 && (r - 100) % 120 == 0 && (r - 100) / 120 < n) {
        direct_hit(vm, &t.spell, pawn, target);
    }
    // HP3_CAST_THEN=<SpellClass>@<ticks after the cast starts>: a second spell born on the same target.
    if let Ok(spec) = std::env::var("HP3_CAST_THEN")
        && let Some((spell, at)) = spec.split_once('@')
        && at.parse::<u32>() == Ok(r)
    {
        direct_hit(vm, spell, pawn, target);
    }
    if r == 90 {
        report_cursor(vm);
    }
    // HP3_CAST_HOLD=<ticks>: keep fire held that much longer (a charged spell such as Patronum needs it).
    let hold = std::env::var("HP3_CAST_HOLD").ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
    if r == 110 + hold {
        crate::world(vm).input.key_up("LeftMouse");
    }
    // HP3_CAST_TAP=<r>: press and release fire again at that tick (letting go of a hook, for one).
    // HP3_CAST_TAP=<r>,<r>,...: several taps, one per tick listed (a spell cast again and again).
    for tap in std::env::var("HP3_CAST_TAP").unwrap_or_default().split(',').filter_map(|v| v.trim().parse::<u32>().ok()) {
        if r == tap {
            crate::world(vm).input.key_down("LeftMouse");
        }
        if r == tap + std::env::var("HP3_CAST_TAPHOLD").ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or(6) {
            crate::world(vm).input.key_up("LeftMouse");
        }
    }
    if std::env::var_os("HP3_CAST_STATES").is_some() && r >= 30 && r <= 600 && r % 8 == 0 {
        println!("cast: r={r} harry state {} bJustCast {}", state_name(vm, pawn), get(vm, pawn, "bJustCast").as_bool());
    }
    if (100..=200).contains(&r) && r % 2 == 0 {
        report_spell(vm, r);
    }
    // HP3_CAST_DROP=<tick>: at that tick the caster is dropped from above the target (a spongified pad bounces him).
    if let Some(drop) = std::env::var("HP3_CAST_DROP").ok().and_then(|v| v.parse::<u32>().ok()) {
        if r == drop {
            let at = vec(&get(vm, target, "Location"));
            let height = get(vm, target, "CollisionHeight").as_float();
            set(vm, pawn, "Location", v3(at + Vec3::Z * (height + 130.0)));
            set(vm, pawn, "Velocity", v3(Vec3::ZERO));
            fire(vm, pawn, "SetPhysics", &mut [Value::Byte(2)]);
            println!("cast: drop: caster to {} above the target", vec(&get(vm, pawn, "Location")));
        }
        if r >= drop && r < drop + 150 && (r - drop) % 3 == 0 {
            let touching = match get(vm, pawn, "Touching") {
                Value::Array(v) => v.len(),
                _ => 0,
            };
            println!(
                "cast: drop: r+{} z {:.1} vz {:.1} physics {:?} touching {touching} target {}",
                r - drop,
                vec(&get(vm, pawn, "Location")).z,
                vec(&get(vm, pawn, "Velocity")).z,
                get(vm, pawn, "Physics"),
                behaviour(vm, target)
            );
        }
    }
    if r == t.report_at {
        finish(vm, t, target);
    }
}

/// Prints the outcome and ends the run.
fn finish(vm: &mut Vm<'_>, t: &CastTest, target: ObjId) {
    report_result(vm, t, target);
    for (at, state) in &t.flame_history {
        println!("cast: flame trigger {state} at r={at}");
    }
    std::process::exit(0);
}

fn place(vm: &mut Vm<'_>, t: &mut CastTest, pc: ObjId, pawn: ObjId) {
    let class = vm.db.find(&t.class);
    let actors = crate::world(vm).actors.clone();
    t.start_actors = actors.iter().copied().collect();
    let list: Vec<ObjId> = actors
        .iter()
        .copied()
        .filter(|&a| class.is_some_and(|w| vm.db.objects.get(a).class.is_some_and(|c| vm.is_child_of(c, w))))
        .collect();
    println!("cast: {} candidates of {}", list.len(), t.class);
    let Some(&target) = list.get(t.index) else {
        println!("cast: no target");
        std::process::exit(3)
    };
    t.target = Some(target);
    let at = vec(&get(vm, target, "Location"));
    t.start_loc = at;
    t.before = state_name(vm, target);
    // Stand where there is floor and the corridor a spell would fly through (its own size, from the wand's
    // height to the target) is clear.
    let aim_up = get(vm, target, "CollisionHeight").as_float().max(0.0);
    let mut chosen = None;
    let mut best: Option<(f32, Vec3, i32)> = None;
    'search: for dist in [t.dist, 300.0, 200.0, 400.0, 150.0, 500.0] {
        for k in 0..32 {
            let yaw = k * 2048;
            let ang = yaw as f32 * std::f32::consts::TAU / 65536.0;
            let dir = Vec3::new(ang.cos(), ang.sin(), 0.0);
            let pos = at - dir * dist;
            let col = &crate::world(vm).collision;
            let Some(f) = col.trace_static(pos + Vec3::Z * 80.0, pos - Vec3::Z * 300.0, Vec3::ZERO) else { continue };
            let stand = f.location + Vec3::Z * 40.0;
            let corridor = Vec3::new(10.0, 10.0, 10.0);
            let top = at + Vec3::Z * (aim_up + 6.0);
            let low = col.trace_static(stand + Vec3::Z * 10.0, top, corridor).map_or(1.0, |h| h.time);
            let high = col.trace_static(stand + Vec3::Z * 30.0, top + Vec3::Z * 10.0, corridor).map_or(1.0, |h| h.time);
            // The camera sits behind and above the pawn; it has to see the target too.
            let cam = stand - dir * 140.0 + Vec3::Z * 90.0;
            let seen = col.trace_static(cam, at + Vec3::Z * aim_up, Vec3::ZERO).map_or(1.0, |h| h.time);
            if low >= 1.0 && high >= 1.0 && seen >= 1.0 {
                chosen = Some((stand, yaw));
                break 'search;
            }
            let score = low.min(high).min(seen);
            if best.is_none_or(|b| score > b.0) {
                best = Some((score, stand, yaw));
            }
        }
    }
    if chosen.is_none()
        && let Some((score, stand, yaw)) = best
    {
        println!("cast: no fully clear spot; using the best one ({:.0}% of the way clear)", score * 100.0);
        chosen = Some((stand, yaw));
    }
    let Some((stand, yaw)) = chosen else {
        println!("cast: no clear spot in front of the target");
        std::process::exit(4)
    };
    t.caster = Some((pawn, stand));
    set(vm, pawn, "Location", v3(stand));
    set(vm, pawn, "Velocity", v3(Vec3::ZERO));
    set(vm, pawn, "Rotation", rot(0, yaw));
    set(vm, pc, "Rotation", rot(0, yaw));
    // HP3_CAST_ACTIVATE=1 sends the target the Trigger event the level's own logic would (puzzle targets start inert).
    let inert = get(vm, target, "vulnerableToClass").as_obj().is_none() || !get(vm, target, "bProjTarget").as_bool();
    if std::env::var_os("HP3_CAST_ACTIVATE").is_some() && inert {
        fire(vm, target, "Trigger", &mut [Value::Object(Some(pawn)), Value::Object(Some(pawn))]);
    }
    let vulnerable = get(vm, target, "vulnerableToClass").as_obj().map(|c| name_of(vm, c));
    println!(
        "cast: target {} ({}) at {at}, state {}, vulnerable to {vulnerable:?}, bProjTarget {}; Harry at {stand}",
        name_of(vm, target),
        t.class,
        t.before,
        get(vm, target, "bProjTarget").as_bool()
    );
}

/// HP3_CAST_FLAME=<n>: once the player is the dragon (the Draconifors cast worked), put it in front of the n-th
/// trigger that burns (vulnerable to the dragon's fire), facing it, and hold fire like a player would.
fn drive_flame(vm: &mut Vm<'_>, t: &mut CastTest, pc: ObjId, r: u32) {
    let Some(n) = std::env::var("HP3_CAST_FLAME").ok().and_then(|v| v.parse::<usize>().ok()) else { return };
    let Some(dragon) = get(vm, pc, "ViewTarget").as_obj() else { return };
    if t.flame.is_none() {
        // The transformation has to be over: the dragon is in its active state.
        if r < 60 || state_name(vm, dragon) != "StateActive" {
            if r % 120 == 0 {
                println!("cast: flame: waiting for {} (state {}, controller {})", name_of(vm, dragon), behaviour(vm, dragon), state_name(vm, pc));
            }
            return;
        }
        let actors = crate::world(vm).actors.clone();
        let burns: Vec<ObjId> = actors
            .into_iter()
            .filter(|&a| get(vm, a, "vulnerableToClass").as_obj().is_some_and(|c| name_of(vm, c) == "DraconisForsFireSpell"))
            .collect();
        let Some(&trigger) = burns.get(n) else {
            println!("cast: flame: no trigger #{n} ({} burn)", burns.len());
            std::process::exit(5)
        };
        // The dragon only breathes fire once it holds ammunition: fly it through a fire powerup first, as a
        // player would, and let the pickup run.
        if t.flame_pickup == 0 {
            let powerup = actors_of(vm, "DraconisForsFirePowerup").into_iter().next();
            if let Some(p) = powerup {
                let at = vec(&get(vm, p, "Location"));
                println!("cast: flame: dragon picks up {} at {at} (ammo {:?})", name_of(vm, p), get(vm, dragon, "FlameAmmo"));
                fire(vm, dragon, "SetLocation", &mut [v3(at)]);
            }
            t.flame_pickup = r;
            return;
        }
        // Picking the powerup up may start a tutorial cutscene; wait until the player has the dragon back and it
        // has been still for a moment.
        let calm = r >= t.flame_pickup + 30
            && get(vm, pc, "Pawn").as_obj() == Some(dragon)
            && state_name(vm, dragon) == "StateActive";
        t.flame_calm = if calm { t.flame_calm + 1 } else { 0 };
        if t.flame_calm < 90 {
            return;
        }
        let controls = get(vm, pc, "Pawn").as_obj().map(|p| name_of(vm, p));
        println!("cast: flame: ammo {:?}, the player controls {controls:?}", get(vm, dragon, "FlameAmmo"));
        let at = vec(&get(vm, trigger, "Location"));
        // HP3_CAST_FLAME_SIDE=0..3: which side of the trigger the dragon comes from (a cutscene trigger may sit on one).
        let side = std::env::var("HP3_CAST_FLAME_SIDE").ok().and_then(|v| v.parse::<i32>().ok()).unwrap_or(0);
        let ang = side as f32 * std::f32::consts::FRAC_PI_2;
        let dir = Vec3::new(ang.cos(), ang.sin(), 0.0);
        let from = at - dir * std::env::var("HP3_CAST_FLAME_DIST").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(90.0);
        let yaw = side * 16384;
        set(vm, dragon, "Location", v3(from));
        set(vm, dragon, "Velocity", v3(Vec3::ZERO));
        set(vm, dragon, "Rotation", rot(0, yaw));
        set(vm, pc, "Rotation", rot(0, yaw));
        println!(
            "cast: flame: dragon {} to {from}, trigger {} at {at} (collide {}, projTarget {}, radius {}, height {})",
            name_of(vm, dragon),
            name_of(vm, trigger),
            get(vm, trigger, "bCollideActors").as_bool(),
            get(vm, trigger, "bProjTarget").as_bool(),
            get(vm, trigger, "CollisionRadius").as_float(),
            get(vm, trigger, "CollisionHeight").as_float()
        );
        crate::world(vm).input.key_down("LeftMouse");
        t.flame = Some(trigger);
        t.flame_from = r;
    }
    let Some(trigger) = t.flame else { return };
    let rel = r - t.flame_from;
    if (rel < 60 && rel % 4 == 0) || rel % 120 == 5 {
        println!(
            "cast: flame: r+{} bFire {} dragon state {} physics {:?} controller {:?} ammo {:?} shooting {}",
            r - t.flame_from,
            get(vm, pc, "bFire").as_bool(),
            state_name(vm, dragon),
            get(vm, dragon, "Physics"),
            get(vm, dragon, "Controller").as_obj().map(|c| name_of(vm, c)),
            get(vm, dragon, "FlameAmmo"),
            get(vm, dragon, "bDoFlameShoot").as_bool()
        );
    }
    if rel < 90 && rel % 3 == 0 {
        for sp in actors_of(vm, "DraconisForsFireSpell") {
            println!(
                "cast: flame: spell at {} vel {} phys {:?} touching {}",
                vec(&get(vm, sp, "Location")),
                vec(&get(vm, sp, "Velocity")),
                get(vm, sp, "Physics"),
                match get(vm, sp, "Touching") {
                    Value::Array(v) => v.len(),
                    _ => 0,
                }
            );
        }
    }
    let state = behaviour(vm, trigger);
    if t.flame_history.last().is_none_or(|(_, s)| *s != state) {
        t.flame_history.push((r - t.flame_from, state));
    }
}

/// Every actor of a class (or a subclass of it).
fn actors_of(vm: &mut Vm<'_>, class: &str) -> Vec<ObjId> {
    let Some(want) = vm.db.find(class) else { return Vec::new() };
    let all = crate::world(vm).actors.clone();
    all.into_iter().filter(|&a| vm.db.objects.get(a).class.is_some_and(|c| vm.is_child_of(c, want))).collect()
}

fn report_cursor(vm: &mut Vm<'_>) {
    let Some(sc) = vm.db.find("SpellCursor") else { return };
    let actors = crate::world(vm).actors.clone();
    let Some(cursor) = actors.into_iter().find(|&a| vm.db.objects.get(a).class.is_some_and(|c| vm.is_child_of(c, sc))) else {
        println!("cast: no cursor actor");
        return;
    };
    let touching: Vec<String> = match get(vm, cursor, "Touching") {
        Value::Array(items) => items.iter().filter_map(Value::as_obj).map(|o| name_of(vm, o)).collect(),
        _ => Vec::new(),
    };
    let possible = get(vm, cursor, "aPossibleTarget").as_obj().map(|o| name_of(vm, o));
    let current = get(vm, cursor, "aCurrentTarget").as_obj().map(|o| name_of(vm, o));
    println!(
        "cast: cursor collision r {} h {} collide {} ; flags bCollideActors {} bBlockActors {} bProjTarget {} bHidden {}",
        get(vm, cursor, "CollisionRadius").as_float(),
        get(vm, cursor, "CollisionHeight").as_float(),
        get(vm, cursor, "bCollideActors").as_bool(),
        get(vm, cursor, "bCollideActors").as_bool(),
        get(vm, cursor, "bBlockActors").as_bool(),
        get(vm, cursor, "bProjTarget").as_bool(),
        get(vm, cursor, "bHidden").as_bool()
    );
    println!(
        "cast: cursor state {} at {}, touching {touching:?}, possible {possible:?}, current {current:?}",
        state_name(vm, cursor),
        vec(&get(vm, cursor, "Location"))
    );
}

fn report_result(vm: &mut Vm<'_>, t: &CastTest, target: ObjId) {
    let after = behaviour(vm, target);
    let moved = vec(&get(vm, target, "Location")).distance(t.start_loc);
    let now = crate::world(vm).actors.clone();
    let mut kinds: BTreeMap<String, usize> = Default::default();
    for a in now.into_iter().filter(|a| !t.start_actors.contains(a)) {
        let n = name_of(vm, a);
        // What the spell made, with its tag (cutscenes find the dragon and the rabbit by it).
        if let Value::Name(tag) = get(vm, a, "Tag") {
            let (br, bl, ra) = (get(vm, a, "bBlockActors").as_bool(), get(vm, a, "bBlockPlayers").as_bool(), get(vm, a, "CollisionRadius").as_float());
            println!("cast: made {n} with tag {} blockactors {br} blockplayers {bl} radius {ra}", vm.db.names.get(tag));
        }
        *kinds.entry(n.trim_end_matches(|c: char| c.is_ascii_digit()).to_string()).or_default() += 1;
    }
    let deleted = vm.db.objects.get(target).deleted;
    if let Some((pawn, start)) = t.caster {
        let now = vec(&get(vm, pawn, "Location"));
        println!(
            "cast: caster {} moved {:.0} to {now}, physics {}, state {}",
            name_of(vm, pawn),
            now.distance(start),
            get(vm, pawn, "Physics").as_byte(),
            state_name(vm, pawn)
        );
    }
    let path: Vec<String> = t.history.iter().map(|(r, s)| format!("{s}@{r}")).collect();
    println!(
        "castresult {}->{}: states [{}], now {after}, hidden {}, deleted {deleted}, moved {moved:.0}; spawned {kinds:?}",
        t.spell,
        t.class,
        path.join(" > "),
        get(vm, target, "bHidden").as_bool()
    );
}

/// Position and motion of any spell actor in flight.
fn report_spell(vm: &mut Vm<'_>, r: u32) {
    let Some(sc) = vm.db.find("baseSpell") else { return };
    let actors = crate::world(vm).actors.clone();
    for a in actors {
        if !vm.db.objects.get(a).class.is_some_and(|c| vm.is_child_of(c, sc)) {
            continue;
        }
        let touching: Vec<String> = match get(vm, a, "Touching") {
            Value::Array(items) => items.iter().filter_map(Value::as_obj).map(|o| name_of(vm, o)).collect(),
            _ => Vec::new(),
        };
        println!(
            "cast: r={r} spell {} at {} vel {} phys {} state {} collide {} world {} speed {} maxspeed {} rot {:?} desired {:?} dir {} touching {touching:?}",
            name_of(vm, a),
            vec(&get(vm, a, "Location")),
            vec(&get(vm, a, "Velocity")),
            get(vm, a, "Physics").as_byte(),
            state_name(vm, a),
            get(vm, a, "bCollideActors").as_bool(),
            get(vm, a, "bCollideWorld").as_bool(),
            format!("{:?}", get(vm, a, "Speed")),
            get(vm, a, "MaxSpeed").as_float(),
            get(vm, a, "Rotation").fields().iter().map(Value::as_int).collect::<Vec<_>>(),
            get(vm, a, "DesiredRotation").fields().iter().map(Value::as_int).collect::<Vec<_>>(),
            vec(&get(vm, a, "CurrentDir"))
        );
    }
}

/// Spawns the spell on the target, initialises and launches it like the wand does.
fn direct_hit(vm: &mut Vm<'_>, spell_name: &str, pawn: ObjId, target: ObjId) {
    let Some(class) = vm.db.find(spell_name).map(|s| vm.db.get(s).object) else { return };
    let at = get(vm, target, "Location");
    let args = [Value::Object(Some(class)), Value::Object(Some(pawn)), Value::Empty, at.clone(), Value::Empty];
    let Ok(Value::Object(Some(spell))) = crate::natives::spawn(vm, None, &args) else {
        println!("cast: direct spawn failed");
        return;
    };
    let wand = get(vm, pawn, "Weapon");
    fire(
        vm,
        spell,
        "InitSpell",
        &mut [Value::Object(Some(pawn)), Value::Object(Some(target)), v3(Vec3::ZERO), Value::Float(0.0), wand],
    );
    fire(vm, spell, "Launch", &mut []);
    println!("cast: direct hit with {} at {}", name_of(vm, spell), vec(&at));
}
