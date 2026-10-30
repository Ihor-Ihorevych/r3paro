//! `HP3_CAST=<SpellClass>:<TargetClass>[:<n>][:<distance>]@<tick>`: puts Harry in front of the n-th actor of a
//! class, aims the game's camera at it, picks the spell on his wand and holds fire like a player would, then
//! reports what the target did. A test for "everything that can be enchanted can be disenchanted".

use std::collections::{BTreeMap, HashSet};

use glam::Vec3;
use hp3_script::{ObjId, Value, Vm};
use hp3_world::physics::vec;
use hp3_world::{fire, get, set};

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
        })
    }
}

fn state_name(vm: &mut Vm<'_>, a: ObjId) -> String {
    vm.state_of(a).map(|s| vm.db.names.get(vm.db.get(s).name).to_string()).unwrap_or_else(|| "-".into())
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
    if frame < t.at {
        return;
    }
    let r = frame - t.at;
    if r == 0 {
        println!("cast: starting at tick {frame}");
    }
    let Some(pc) = hp3_world::world(vm).player else { return };
    let Some(pawn) = get(vm, pc, "Pawn").as_obj() else { return };
    if r == 0 {
        place(vm, t, pc, pawn);
        if std::env::var_os("HP3_CAST_SPAWNTEST").is_some()
            && let Some(c) = vm.db.find(&t.spell).map(|s| vm.db.get(s).object)
            && let Ok(Value::Object(Some(sp))) = hp3_world::natives::spawn(vm, None, &[Value::Object(Some(c))])
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
    // Keep the camera on the target while the cursor settles.
    if (30..110).contains(&r)
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
    if r == 50 {
        hp3_world::world(vm).input.key_down("LeftMouse");
    }
    if r == 90 {
        report_cursor(vm);
    }
    if r == 110 {
        hp3_world::world(vm).input.key_up("LeftMouse");
    }
    if r >= 30 && r <= 150 && r % 8 == 0 {
        println!("cast: r={r} harry state {} bJustCast {}", state_name(vm, pawn), get(vm, pawn, "bJustCast").as_bool());
    }
    if (100..=200).contains(&r) && r % 2 == 0 {
        report_spell(vm, r);
    }
    if r == 260 {
        report_result(vm, t, target);
    }
}

fn place(vm: &mut Vm<'_>, t: &mut CastTest, pc: ObjId, pawn: ObjId) {
    let class = vm.db.find(&t.class);
    let actors = hp3_world::world(vm).actors.clone();
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
    // Stand where the line to the target is clear and there is floor.
    let mut chosen = None;
    for k in 0..16 {
        let yaw = k * 4096;
        let ang = yaw as f32 * std::f32::consts::TAU / 65536.0;
        let dir = Vec3::new(ang.cos(), ang.sin(), 0.0);
        let pos = at - dir * t.dist;
        let col = &hp3_world::world(vm).collision;
        let Some(f) = col.trace_static(pos + Vec3::Z * 80.0, pos - Vec3::Z * 300.0, Vec3::ZERO) else { continue };
        let stand = f.location + Vec3::Z * 40.0;
        if col.trace_static(stand + Vec3::Z * 20.0, at + Vec3::Z * 20.0, Vec3::ZERO).is_none() {
            chosen = Some((stand, yaw));
            break;
        }
    }
    let Some((stand, yaw)) = chosen else {
        println!("cast: no clear spot in front of the target");
        std::process::exit(4)
    };
    set(vm, pawn, "Location", v3(stand));
    set(vm, pawn, "Velocity", v3(Vec3::ZERO));
    set(vm, pawn, "Rotation", rot(0, yaw));
    set(vm, pc, "Rotation", rot(0, yaw));
    // HP3_CAST_ACTIVATE=1 sends the target the Trigger event the level's own logic would (puzzle targets start inert).
    if std::env::var_os("HP3_CAST_ACTIVATE").is_some() {
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

fn report_cursor(vm: &mut Vm<'_>) {
    let Some(sc) = vm.db.find("SpellCursor") else { return };
    let actors = hp3_world::world(vm).actors.clone();
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
    let after = state_name(vm, target);
    let moved = vec(&get(vm, target, "Location")).distance(t.start_loc);
    let now = hp3_world::world(vm).actors.clone();
    let mut kinds: BTreeMap<String, usize> = Default::default();
    for a in now.into_iter().filter(|a| !t.start_actors.contains(a)) {
        let n = name_of(vm, a);
        *kinds.entry(n.trim_end_matches(|c: char| c.is_ascii_digit()).to_string()).or_default() += 1;
    }
    let deleted = vm.db.objects.get(target).deleted;
    println!(
        "castresult {}->{}: state {} -> {after}, hidden {}, deleted {deleted}, moved {moved:.0}; spawned {kinds:?}",
        t.spell,
        t.class,
        t.before,
        get(vm, target, "bHidden").as_bool()
    );
}

/// Position and motion of any spell actor in flight.
fn report_spell(vm: &mut Vm<'_>, r: u32) {
    let Some(sc) = vm.db.find("baseSpell") else { return };
    let actors = hp3_world::world(vm).actors.clone();
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
