//! `hp3-run`: headless runner.
//!
//! - `classes <HP3 folder> [class]`: loads every script class, builds its defaults and reports problems; with a
//!   class name, prints that class's properties and default values.

mod casttest;
mod mounttest;

use std::path::Path;
use std::process::ExitCode;

use hp3_package::PackageSet;
use hp3_script::ClassDb;

/// HP3_TRIGGER=<tag> (or @<actor name>) triggers the actors with that tag (as an event of that name would), instigated by the
/// player's pawn.
fn trigger_tag(vm: &mut hp3_script::Vm<'_>) {
    let Ok(tag) = std::env::var("HP3_TRIGGER") else { return };
    {
        let n = vm.db.names.intern(&tag);
        let pawn = hp3_world::world(vm).player.and_then(|pc| hp3_world::get(vm, pc, "Pawn").as_obj());
        let actors = hp3_world::world(vm).actors.clone();
        for a in actors {
            // "@Name" picks the actor by its own name rather than by tag.
            let by_name = tag.strip_prefix('@').is_some_and(|w| hp3_script::natives::object_path(vm, a).ends_with(&format!(".{w}")));
            if by_name || (!tag.starts_with('@') && hp3_world::get(vm, a, "Tag").as_name() == n) {
                println!("triggering {}", hp3_script::natives::object_path(vm, a));
                hp3_world::fire(vm, a, "Trigger", &mut [hp3_script::Value::Object(pawn), hp3_script::Value::Object(pawn)]);
            }
        }
    }
}

/// HP3_LIST_CLASS=<class> lists the level's actors of that class with their main properties.
fn list_class(vm: &mut hp3_script::Vm<'_>) {
    if let Ok(class) = std::env::var("HP3_LIST_CLASS") {
        let actors = hp3_world::world(vm).actors.clone();
        let want = vm.db.find(&class);
        for a in actors {
            if want.is_none() || !vm.db.objects.get(a).class.zip(want).is_some_and(|(c, w)| vm.is_child_of(c, w)) {
                continue;
            }
            let name = hp3_script::natives::object_path(vm, a);
            let mut info = String::new();
            let default_props = "Tag,Event,Location,CollisionRadius,CollisionHeight,bCollideActors,TriggerType,bTriggerOnceOnly,CutName,bInitiallyActive".to_string();
        let props = std::env::var("HP3_LIST_PROPS").unwrap_or(default_props);
        for n in props.split(',') {
                let v = hp3_world::get(vm, a, n);
                let v = match v {
                    hp3_script::Value::Empty => continue,
                    hp3_script::Value::Name(x) => vm.db.names.get(x).to_string(),
                    other => format!("{other:?}"),
                };
                info.push_str(&format!(" {n}={v}"));
            }
            let state = vm.state_of(a).map(|s| vm.db.names.get(vm.db.structs[s.0 as usize].name).to_string());
            println!("  {name} state {state:?}{info}");
            // Emitter actors: what each emitter shows (its texture object and class).
            if let hp3_script::Value::Array(list) = hp3_world::get(vm, a, "Emitters") {
                for (i, e) in list.iter().enumerate() {
                    if let Some(eo) = e.as_obj() {
                        let tex = hp3_world::get(vm, eo, "Texture").as_obj();
                        let tex_desc = tex.map(|t| {
                            let class = vm.db.objects.get(t).class.map(|c| vm.db.names.get(vm.db.get(c).name).to_string()).unwrap_or_default();
                            format!("{} ({class})", hp3_script::natives::object_path(vm, t))
                        });
                        println!("      emitter {i}: texture {tex_desc:?} blend {:?}", hp3_world::get(vm, eo, "ColorMultiplierRange"));
                    }
                }
            }
        }
    }
}

/// Performs a level switch the game asked for; returns whether one happened.
fn follow_travel(vm: &mut hp3_script::Vm<'_>) -> bool {
    let Some(url) = hp3_world::take_travel(vm) else { return false };
    match hp3_world::travel(vm, &url) {
        Ok(m) => println!("travelled to {} ({url})", m.name),
        Err(e) => println!("travel to {url} failed: {e}"),
    }
    true
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("classes") if args.len() >= 2 => classes(Path::new(&args[1]), args.get(2).map(String::as_str)),
        Some("instances") if args.len() >= 2 => instances(Path::new(&args[1]), args.get(2).map(String::as_str)),
        Some("compile") if args.len() == 2 => compile_all(Path::new(&args[1])),
        Some("levels") if args.len() >= 2 => levels(Path::new(&args[1]), args.get(2).and_then(|f| f.parse().ok()).unwrap_or(120)),
        Some("level") if args.len() >= 3 => level(Path::new(&args[1]), &args[2], args.get(3).and_then(|f| f.parse().ok()).unwrap_or(600)),
        Some("call") if args.len() >= 4 => call(Path::new(&args[1]), &args[2], &args[3], &args[4..]),
        _ => {
            eprintln!("usage: hp3-run classes <HP3 folder> [class]");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Runs every map for `frames` ticks and summarizes errors and missing natives across all of them.
fn levels(root: &Path, frames: u32) -> Result<ExitCode, Box<dyn std::error::Error>> {
    use std::collections::BTreeMap;
    let set = PackageSet::open(root)?;
    let mut maps: Vec<String> = set
        .files()
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("unr")))
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    maps.sort();
    let mut missing_total: BTreeMap<String, (u64, usize)> = BTreeMap::new();
    let mut errors_total: BTreeMap<String, usize> = BTreeMap::new();
    for map in &maps {
        let mut db = ClassDb::new(&set);
        db.load_all_classes();
        let mut vm = hp3_script::Vm::new(db);
        let pkg = set.get(map)?;
        let t = std::time::Instant::now();
        let loaded = hp3_world::load(&mut vm, &pkg, hp3_core::Config::load(root));
        if let Err(e) = loaded {
            println!("{map}: load failed: {e}");
            continue;
        }
        for _ in 0..frames {
            hp3_world::tick(&mut vm, 1.0 / 60.0);
        }
        let w = hp3_world::world(&mut vm);
        let (actors, errors, player) = (w.actors.len(), std::mem::take(&mut w.errors), w.player.is_some());
        println!(
            "{map:32} {actors:5} actors, player {}, {:4} errors, {:3} missing natives, {:?}",
            if player { "yes" } else { "NO " },
            errors.len(),
            vm.missing.len(),
            t.elapsed()
        );
        for e in errors {
            let key = e.split_once(' ').map(|(_, r)| r.to_string()).unwrap_or(e);
            *errors_total.entry(key).or_default() += 1;
        }
        for (n, c) in &vm.missing {
            let e = missing_total.entry(n.clone()).or_default();
            e.0 += c;
            e.1 += 1;
        }
    }
    let mut m: Vec<_> = missing_total.into_iter().collect();
    m.sort_by(|a, b| b.1.1.cmp(&a.1.1).then(b.1.0.cmp(&a.1.0)));
    println!("\nmissing natives (maps, calls):");
    for (n, (c, maps)) in &m {
        println!("  {maps:3} {c:8} {n}");
    }
    let mut e: Vec<_> = errors_total.into_iter().collect();
    e.sort_by(|a, b| b.1.cmp(&a.1));
    println!("\nscript errors:");
    for (k, n) in e.iter().take(40) {
        println!("  {n:6} {k}");
    }
    Ok(ExitCode::SUCCESS)
}

/// Loads a map, runs begin-play and `frames` ticks at 60 Hz, and reports what went wrong.
fn level(root: &Path, map: &str, frames: u32) -> Result<ExitCode, Box<dyn std::error::Error>> {
    use std::collections::BTreeMap;
    let set = PackageSet::open(root)?;
    let mut db = ClassDb::new(&set);
    db.load_all_classes();
    let mut vm = hp3_script::Vm::new(db);
    vm.profile = std::env::var_os("HP3_PROFILE").is_some();
    vm.trace_states = std::env::var_os("HP3_TRACE_STATES").is_some();
    let pkg = set.get(map)?;
    let t = std::time::Instant::now();
    hp3_world::load(&mut vm, &pkg, hp3_core::Config::load(root))?;
    let load_time = t.elapsed();
    if let Some(pc) = hp3_world::world(&mut vm).player
        && let Some(pawn) = hp3_world::get(&mut vm, pc, "Pawn").as_obj()
    {
        let loc = hp3_world::physics::vec(&hp3_world::get(&mut vm, pawn, "Location"));
        let w = hp3_world::world(&mut vm);
        println!("collision: {} triangles", w.collision.triangle_count());
        let (nodes, links) = hp3_world::nav::network_size(&mut vm);
        println!("path network: {nodes} navigation points, {links} links");
        let w = hp3_world::world(&mut vm);
        for (what, d) in [("down", -glam::Vec3::Z * 2000.0), ("up", glam::Vec3::Z * 2000.0), ("forward", glam::Vec3::X * 4000.0)] {
            let hit = w.collision.trace(loc, loc + d, glam::Vec3::ZERO, Some(pawn));
            println!("  trace {what} from pawn at {loc}: {:?}", hit.map(|h| (h.time * d.length(), h.normal, h.actor.is_some())));
        }
        let r = hp3_world::get(&mut vm, pawn, "CollisionRadius").as_float();
        let hgt = hp3_world::get(&mut vm, pawn, "CollisionHeight").as_float();
        let ext = glam::Vec3::new(r, r, hgt);
        let w = hp3_world::world(&mut vm);
        for (what, d) in [("down", -glam::Vec3::Z * 200.0), ("forward", glam::Vec3::X * 4000.0)] {
            let hit = w.collision.trace(loc, loc + d, ext, Some(pawn));
            println!("  box {ext} trace {what}: {:?}", hit.map(|h| (h.time * d.length(), h.normal, h.actor.is_some())));
        }
    }
    // HP3_PROBE_GRID=x0,y0,x1,y1,step,z_top,z_bottom: where a thin box dropped from z_top lands, over a grid.
    if let Ok(spec) = std::env::var("HP3_PROBE_GRID") {
        let v: Vec<f32> = spec.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        if v.len() == 7 {
            let mut y = v[1];
            while y <= v[3] {
                let mut line = String::new();
                let mut x = v[0];
                while x <= v[2] {
                    let from = glam::Vec3::new(x, y, v[5]);
                    let hit = hp3_world::world(&mut vm).collision.trace_as(true, from, glam::Vec3::new(x, y, v[6]), glam::Vec3::new(2.0, 2.0, 2.0) * if std::env::var_os("HP3_PROBE_PAWN").is_some() { glam::Vec3::new(7.5, 7.5, 19.0) } else { glam::Vec3::ONE }, None);
                    match hit {
                        Some(h) => line += &format!(" {:6.1}{}", h.location.z, if h.actor.is_some() { "m" } else { " " }),
                        None => line += "   --   ",
                    }
                    x += v[4];
                }
                println!("y {y:8.1}:{line}");
                y += v[4];
            }
        }
    }
    // HP3_PROBE_Y=x,z0,z1,step,y0,y1: like HP3_PROBE_X, along +y.
    if let Ok(spec) = std::env::var("HP3_PROBE_Y") {
        let v: Vec<f32> = spec.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        if v.len() == 6 {
            let mut z = v[1];
            while z <= v[2] {
                let hit = hp3_world::world(&mut vm).collision.trace_as(true, glam::Vec3::new(v[0], v[4], z), glam::Vec3::new(v[0], v[5], z), glam::Vec3::new(15.0, 15.0, 38.0), None);
                match hit {
                    Some(h) => println!("z {z:7.1}: stops at y {:8.1} normal ({:.2},{:.2},{:.2}) actor {:?}", h.location.y, h.normal.x, h.normal.y, h.normal.z, h.actor.map(|o| hp3_script::natives::object_path(&vm, o))),
                    None => println!("z {z:7.1}: free"),
                }
                z += v[3];
            }
        }
    }
    // HP3_PROBE_X=y,z0,z1,step,x0,x1: traces a pawn-sized box along +x at each height, printing where it stops.
    if let Ok(spec) = std::env::var("HP3_PROBE_X") {
        let v: Vec<f32> = spec.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        if v.len() == 6 {
            let mut z = v[1];
            while z <= v[2] {
                let hit = hp3_world::world(&mut vm).collision.trace_as(true, glam::Vec3::new(v[4], v[0], z), glam::Vec3::new(v[5], v[0], z), glam::Vec3::new(15.0, 15.0, 38.0), None);
                match hit {
                    Some(h) => println!("z {z:7.1}: stops at x {:8.1} normal ({:.2},{:.2},{:.2}) actor {:?}", h.location.x, h.normal.x, h.normal.y, h.normal.z, h.actor.map(|o| hp3_script::natives::object_path(&vm, o))),
                    None => println!("z {z:7.1}: free"),
                }
                z += v[3];
            }
        }
    }
    // HP3_DUMP_MOVER=<name>: the mover's collision triangles in the world.
    if let Ok(name) = std::env::var("HP3_DUMP_MOVER") {
        let actors = hp3_world::world(&mut vm).actors.clone();
        for a in actors {
            if hp3_script::natives::object_path(&vm, a).ends_with(&format!(".{name}"))
                && let Some(m) = hp3_world::world(&mut vm).collision.mover(a)
            {
                for t in m.world_triangles() {
                    println!("tri ({:.0},{:.0},{:.0}) ({:.0},{:.0},{:.0}) ({:.0},{:.0},{:.0})", t[0].x, t[0].y, t[0].z, t[1].x, t[1].y, t[1].z, t[2].x, t[2].y, t[2].z);
                }
            }
        }
    }
    let steps_after_load = vm.steps;
    // Keys held for the whole run, e.g. HP3_HOLD=W,D.
    if let Ok(keys) = std::env::var("HP3_HOLD") {
        for k in keys.split(',') {
            hp3_world::world(&mut vm).input.key_down(k.trim());
        }
    }
    let t = std::time::Instant::now();
    let trigger_at: u32 = std::env::var("HP3_TRIGGER_AT").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
    // HP3_TRACE_FROM=<tick>: HP3_TRACE_CALLS only starts printing at that tick.
    let trace_from: Option<u32> = std::env::var("HP3_TRACE_FROM").ok().and_then(|v| v.parse().ok());
    let held_trace = trace_from.and_then(|_| vm.trace_calls.take());
    let walkabout_from: Option<u32> = std::env::var("HP3_WALKABOUT").ok().and_then(|v| v.parse().ok());
    #[derive(Default)]
    struct Walk {
        last: Option<glam::Vec3>,
        distance: f32,
        stuck: u32,
        longest_stuck: u32,
        max_tick_ms: f32,
        max_tick_frame: u32,
    }
    let mut walk = Walk::default();
    let mut cast = casttest::CastTest::from_env();
    let mut mount = mounttest::MountTest::from_env();
    for frame in 0..frames {
        if trace_from == Some(frame) {
            vm.trace_calls = held_trace.clone();
        }
        if frame == trigger_at {
            trigger_tag(&mut vm);
        }
        // HP3_WATCH=<name>,<name>@<every>: prints those actors' place, turn and state every so many ticks.
        if let Ok(spec) = std::env::var("HP3_WATCH")
            && let Some((names, every)) = spec.split_once('@')
            && every.parse::<u32>().is_ok_and(|e| e > 0 && frame % e == 0)
        {
            let actors = hp3_world::world(&mut vm).actors.clone();
            for a in actors {
                let path = hp3_script::natives::object_path(&mut vm, a);
                if names.split(',').any(|n| path.ends_with(&format!(".{n}")) || n.strip_prefix('~').is_some_and(|w| path.contains(w))) {
                    let state = vm.state_of(a).map(|s| vm.db.names.get(vm.db.get(s).name).to_string()).unwrap_or_default();
                    let at = hp3_world::physics::vec(&hp3_world::get(&mut vm, a, "Location"));
                    let rot: Vec<i32> = hp3_world::get(&mut vm, a, "Rotation").fields().iter().map(hp3_script::Value::as_int).collect();
                    println!(
                        "watch {frame} {path} at {at} rot {rot:?} state {state} phys {:?} hidden {:?} draw {:?} mesh {:?} vel {:?} accel {:?}",
                        hp3_world::get(&mut vm, a, "Physics"),
                        hp3_world::get(&mut vm, a, "bHidden"),
                        hp3_world::get(&mut vm, a, "DrawType"),
                        hp3_world::get(&mut vm, a, "Mesh").as_obj().is_some(),
                        hp3_world::physics::vec(&hp3_world::get(&mut vm, a, "Velocity")),
                        hp3_world::physics::vec(&hp3_world::get(&mut vm, a, "Acceleration"))
                    );
                }
            }
        }
        // HP3_SAVE=<slot>@<tick> queues a save as the game's own script does; HP3_LOAD=<slot>@<tick> opens a slot.
        if let Ok(spec) = std::env::var("HP3_SAVE")
            && let Some((slot, tick)) = spec.split_once('@')
            && tick.parse::<u32>() == Ok(frame)
            && let Some(pc) = hp3_world::world(&mut vm).player
        {
            hp3_world::set(&mut vm, pc, "iSaveSlotID", hp3_script::Value::Int(slot.parse().unwrap_or(0)));
            hp3_world::set(&mut vm, pc, "bQueuedToSaveGame", hp3_script::Value::Bool(true));
        }
        if let Ok(spec) = std::env::var("HP3_LOAD")
            && let Some((slot, tick)) = spec.split_once('@')
            && tick.parse::<u32>() == Ok(frame)
        {
            println!("console: OPEN ?LOAD={slot}");
            hp3_world::save::console(&mut vm, &format!("OPEN ?LOAD={slot}"));
        }
        // HP3_CHANNELS=<name>@<every>: prints the blend weights and rates of an actor's animation channels.
        if let Ok(spec) = std::env::var("HP3_CHANNELS")
            && let Some((name, every)) = spec.split_once('@')
            && every.parse::<u32>().is_ok_and(|e| e > 0 && frame % e == 0)
        {
            let actors = hp3_world::world(&mut vm).actors.clone();
            for a in actors {
                if !hp3_script::natives::object_path(&vm, a).ends_with(&format!(".{name}")) {
                    continue;
                }
                let set = hp3_world::anim::set_of(&mut vm, a);
                let chans = hp3_world::world(&mut vm).anim.actors.get(&a).map(|x| x.channels.clone()).unwrap_or_default();
                let line: Vec<String> = chans
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.sequence.is_some() && c.alpha > 0.001 || *(&c.playing) && c.alpha > 0.001)
                    .map(|(i, c)| {
                        let n = c.sequence.and_then(|s| set.as_ref().map(|set| set.sequences[s].name.clone())).unwrap_or_default();
                        format!("ch{i} {n} a{:.2} f{:.1} r{:.2}", c.alpha, c.frame, c.rate)
                    })
                    .collect();
                let vel = hp3_world::physics::vec(&hp3_world::get(&mut vm, a, "Velocity"));
                println!("channels {frame} speed {:.0}: {}", vel.truncate().length(), line.join(" | "));
            }
        }
        // HP3_TELEPORT=<x>,<y>,<z>@<tick>: moves the player's pawn there (as SetLocation does), to walk into a trigger.
        if let Ok(spec) = std::env::var("HP3_TELEPORT")
            && let Some((at, tick)) = spec.split_once('@')
            && tick.parse::<u32>() == Ok(frame)
        {
            let c: Vec<f32> = at.split(',').filter_map(|v| v.parse().ok()).collect();
            if let (&[x, y, z], Some(pc)) = (c.as_slice(), hp3_world::world(&mut vm).player)
                && let Some(pawn) = hp3_world::get(&mut vm, pc, "Pawn").as_obj()
            {
                hp3_world::physics::far_move(&mut vm, pawn, glam::Vec3::new(x, y, z), false, false);
                if let Ok(yaw) = std::env::var("HP3_TELEPORT_YAW").map(|v| v.parse::<i32>().unwrap_or(0)) {
                    let r = hp3_script::Value::Struct(vec![hp3_script::Value::Int(0), hp3_script::Value::Int(yaw), hp3_script::Value::Int(0)].into());
                    hp3_world::set(&mut vm, pawn, "Rotation", r.clone());
                    hp3_world::set(&mut vm, pawn, "DesiredRotation", r.clone());
                    hp3_world::set(&mut vm, pc, "Rotation", r);
                }
            }
        }
        // HP3_TRAVEL=<url>@<tick>: asks the level to switch to <url> with the player's items, as a ServerTravel does.
        if let Ok(spec) = std::env::var("HP3_TRAVEL")
            && let Some((url, tick)) = spec.rsplit_once('@')
            && tick.parse::<u32>() == Ok(frame)
            && let Some(li) = hp3_world::world(&mut vm).level_info
        {
            hp3_world::set(&mut vm, li, "bNextItems", hp3_script::Value::Bool(true));
            hp3_world::set(&mut vm, li, "NextURL", hp3_script::Value::Str(url.to_string()));
            hp3_world::set(&mut vm, li, "NextSwitchCountdown", hp3_script::Value::Float(0.0));
        }
        // HP3_GIVE_BEANS=<n>@<tick>: the player's controller is given n beans; the bean count is printed after.
        if let Ok(spec) = std::env::var("HP3_GIVE_BEANS")
            && let Some((n, tick)) = spec.split_once('@')
            && let (Ok(n), Ok(tick)) = (n.parse::<i32>(), tick.parse::<u32>())
            && (frame == tick || frame == tick + 5)
            && let Some(pc) = hp3_world::world(&mut vm).player
        {
            if frame == tick {
                hp3_world::fire(&mut vm, pc, "GiveBeans", &mut [hp3_script::Value::Int(n)]);
            }
            if let Some(pawn) = hp3_world::get(&mut vm, pc, "Pawn").as_obj() {
                let name = vm.db.names.intern("Jellybean");
                let count = hp3_world::fire(&mut vm, pawn, "GetInventoryCount", &mut [hp3_script::Value::Name(name)]);
                println!("beans at {frame}: {count:?}");
            }
        }
        // HP3_HOLD=<key>@<tick>[-<tick>]: holds a key (as the app would) over a span of ticks.
        for var in ["HP3_HOLD", "HP3_HOLD2"] {
            let Ok(spec) = std::env::var(var) else { continue };
            if let Some((key, span)) = spec.split_once('@') {
                let (from, to) = span.split_once('-').map_or((span, ""), |(a, b)| (a, b));
                let from: u32 = from.parse().unwrap_or(0);
                let to: u32 = to.parse().unwrap_or(u32::MAX);
                if frame == from {
                    hp3_world::world(&mut vm).input.key_down(key);
                }
                if frame == to {
                    hp3_world::world(&mut vm).input.key_up(key);
                }
            }
        }
        if false
            && let Ok(spec) = std::env::var("HP3_HOLD")
            && let Some((key, span)) = spec.split_once('@')
        {
            let (from, to) = span.split_once('-').map_or((span, ""), |(a, b)| (a, b));
            let from: u32 = from.parse().unwrap_or(0);
            let to: u32 = to.parse().unwrap_or(u32::MAX);
            if frame == from {
                hp3_world::world(&mut vm).input.key_down(key);
            }
            if frame == to {
                hp3_world::world(&mut vm).input.key_up(key);
            }
        }
        // HP3_DEBUG_MODE=<walk|fly|ghost>@<tick>: the debug movement modes.
        if let Ok(spec) = std::env::var("HP3_DEBUG_MODE")
            && let Some((mode, at)) = spec.split_once('@')
            && at.parse::<u32>().ok() == Some(frame)
        {
            let mode = match mode {
                "fly" => hp3_world::debug_move::Mode::Fly,
                "ghost" => hp3_world::debug_move::Mode::Ghost,
                _ => hp3_world::debug_move::Mode::Walk,
            };
            println!("debug mode {mode:?}: {}", hp3_world::debug_move::set_mode(&mut vm, mode));
        }
        // HP3_MOUSE=<dx>,<dy>@<from>-<to>: mouse motion applied every tick over a span.
        if let Ok(spec) = std::env::var("HP3_MOUSE")
            && let Some((delta, span)) = spec.split_once('@')
        {
            let d: Vec<f32> = delta.split(',').filter_map(|v| v.trim().parse().ok()).collect();
            let (from, to) = span.split_once('-').unwrap_or((span, "4294967295"));
            if (from.parse().unwrap_or(0)..=to.parse().unwrap_or(u32::MAX)).contains(&frame) && d.len() == 2 {
                hp3_world::world(&mut vm).input.mouse_move(d[0], d[1]);
            }
        }
        // HP3_WALKABOUT=<tick>: from then on hold forward and sweep the camera, measuring how the player gets on.
        if let Some(start) = walkabout_from
            && frame >= start
        {
            if frame == start {
                hp3_world::world(&mut vm).input.key_down("W");
            }
            hp3_world::world(&mut vm).input.mouse_move(2.0, 0.0);
        }
        // HP3_PAWN_LOG=<every>: the player pawn's heading, velocity direction and the controller's view yaw.
        if let Some(every) = std::env::var("HP3_PAWN_LOG").ok().and_then(|v| v.parse::<u32>().ok())
            && frame % every == 0
            && let Some(pc) = hp3_world::world(&mut vm).player
            && let Some(p) = hp3_world::get(&mut vm, pc, "Pawn").as_obj()
        {
            let yaw = |v: hp3_script::Value| v.fields().get(1).map(|f| f.as_int() & 0xFFFF).unwrap_or(0);
            let (pr, cr) = (hp3_world::get(&mut vm, p, "Rotation"), hp3_world::get(&mut vm, pc, "Rotation"));
            let vel = hp3_world::physics::vec(&hp3_world::get(&mut vm, p, "Velocity"));
            let vyaw = (vel.y.atan2(vel.x) * 65536.0 / std::f32::consts::TAU).round() as i32 & 0xFFFF;
            let anim = hp3_world::get(&mut vm, p, "AnimSequence").as_name();
            let cam_yaw = hp3_world::get(&mut vm, pc, "Camera").as_obj().map(|c| yaw(hp3_world::get(&mut vm, c, "rCurrRotation"))).unwrap_or(0);
            let phys = hp3_world::get(&mut vm, p, "Physics").as_byte();
            let acc = hp3_world::physics::vec(&hp3_world::get(&mut vm, p, "Acceleration"));
            let at = hp3_world::physics::vec(&hp3_world::get(&mut vm, p, "Location"));
            let cam = hp3_world::get(&mut vm, pc, "Camera").as_obj().map(|c| hp3_world::physics::vec(&hp3_world::get(&mut vm, c, "Location"))).unwrap_or_default();
            println!("pawnpos {frame}: at ({:.1}, {:.1}, {:.1}) accel {:.0} camera ({:.1}, {:.1}, {:.1})", at.x, at.y, at.z, acc.length(), cam.x, cam.y, cam.z);
            println!(
                "pawnlog {frame}: phys {phys} pawn yaw {} vel yaw {vyaw} speed {:.0} view yaw {} camera yaw {cam_yaw} anim {}",
                yaw(pr),
                vel.truncate().length(),
                yaw(cr),
                vm.db.names.get(anim)
            );
        }
        if let Some(t) = cast.as_mut() {
            casttest::step(&mut vm, t, frame);
        }
        if let Some(t) = mount.as_mut() {
            mounttest::step(&mut vm, t, frame);
        }
        let tick_start = std::time::Instant::now();
        hp3_world::tick(&mut vm, std::env::var("HP3_TICK_DT").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0 / 60.0));
        // HP3_CALC_VIEW=1: computes the player's view every tick, as the app does when drawing (the spell cursor
        // is placed by it).
        if std::env::var_os("HP3_CALC_VIEW").is_some() {
            let _ = hp3_world::player_view(&mut vm);
        }
        follow_travel(&mut vm);
        if let Some(start) = walkabout_from
            && frame >= start
        {
            let ms = tick_start.elapsed().as_secs_f32() * 1000.0;
            if ms > walk.max_tick_ms {
                walk.max_tick_ms = ms;
                walk.max_tick_frame = frame;
            }
            let pawn = hp3_world::world(&mut vm).player.and_then(|pc| hp3_world::get(&mut vm, pc, "Pawn").as_obj());
            if let Some(p) = pawn {
                let at = hp3_world::physics::vec(&hp3_world::get(&mut vm, p, "Location"));
                if let Some(last) = walk.last {
                    let step = at.distance(last);
                    walk.distance += step;
                    if step < 0.5 {
                        walk.stuck += 1;
                        walk.longest_stuck = walk.longest_stuck.max(walk.stuck);
                    } else {
                        walk.stuck = 0;
                    }
                }
                walk.last = Some(at);
            }
        }
    }
    if walkabout_from.is_some() {
        println!(
            "walkabout: walked {:.0} units, longest stuck {} ticks, slowest tick {:.1} ms (tick {})",
            walk.distance, walk.longest_stuck, walk.max_tick_ms, walk.max_tick_frame
        );
    }
    let tick_time = t.elapsed();
    list_class(&mut vm);
    // HP3_FLOAT_CHECK=1: pawns whose feet are well above the floor, with their physics and animation.
    if std::env::var_os("HP3_FLOAT_CHECK").is_some() {
        for line in float_check(&mut vm) {
            println!("{line}");
        }
    }
    println!("particles alive: {}", hp3_world::particles::live(&mut vm));
    if let Ok(filter) = std::env::var("HP3_PARTICLE_LIST") {
        for (name, at, n) in hp3_world::particles::summary(&mut vm) {
            if name.to_ascii_lowercase().contains(&filter.to_ascii_lowercase()) {
                println!("  emitter {name} at {at} alive {n}");
            }
        }
    }
    // HP3_AUDIO_DEBUG=1: what the game asked to hear.
    if std::env::var_os("HP3_AUDIO_DEBUG").is_some() {
        let events = hp3_world::audio::drain(&mut vm);
        println!("audio: {} events", events.len());
        // Which sound is asked for how often, and by whom.
        let mut counts: std::collections::BTreeMap<String, u32> = Default::default();
        for e in &events {
            if let hp3_world::audio::Event::Play { sound, who, .. } = e {
                *counts.entry(format!("{} by {who}", hp3_world::audio::sound_name(&vm, sound))).or_default() += 1;
            }
        }
        let mut counts: Vec<_> = counts.into_iter().collect();
        counts.sort_by_key(|c| std::cmp::Reverse(c.1));
        for (k, n) in counts.iter().take(15) {
            println!("  {n:5} x {k}");
        }
        for e in events.iter().take(40) {
            match e {
                hp3_world::audio::Event::Play { sound, volume, pitch, radius, slot, .. } => {
                    let secs = hp3_world::audio::sound_file(&mut vm, sound)
                        .and_then(|(ty, d)| if ty == "bik" { hp3_audio::bink::duration(&d) } else { hp3_audio::wav::duration(&d) });
                    let name = vm.db.set.get(&sound.0).ok().map(|p| p.ref_path(hp3_package::ObjRef::from_export(sound.1))).unwrap_or_default();
                    println!("  play {name} vol {volume} pitch {pitch} radius {radius} slot {slot} ({secs:?} s)");
                }
                other => println!("  {other:?}"),
            }
        }
    }
    let w = hp3_world::world(&mut vm);
    let (actors, spawned, errors) = (w.actors.len(), w.spawned, std::mem::take(&mut w.errors));
    println!(
        "{map}: {actors} actors ({spawned} spawned), begin-play {load_time:?} ({steps_after_load} statements),          {frames} ticks {tick_time:?} ({} statements)",
        vm.steps - steps_after_load
    );
    if let Some(pc) = hp3_world::world(&mut vm).player {
        let class = vm.db.objects.get(pc).class.map(|c| vm.db.names.get(vm.db.get(c).name).to_string());
        let pawn = hp3_world::get(&mut vm, pc, "Pawn").as_obj();
        let state = vm.state_of(pc).map(|s| vm.db.names.get(vm.db.get(s).name).to_string());
        println!("player controller {class:?} in state {state:?}");
        for n in ["aBaseY", "aForward", "aStrafe", "bMovePawn", "bUpdatePosition", "bCanMove", "Pawn"] {
            println!("  pc.{n} = {:?}", hp3_world::get(&mut vm, pc, n));
        }
        if let Some(p) = pawn {
            if let Some(ch) = hp3_world::world(&mut vm).anim.actors.get(&p).and_then(|x| x.channels.first().cloned()) {
                let set = hp3_world::anim::set_of(&mut vm, p);
                let name = ch.sequence.and_then(|s| set.map(|set| set.sequences[s].name.clone()));
                println!("  pawn anim channel 0: {name:?} frame {:.1} rate {:.1} looping {} playing {}", ch.frame, ch.rate, ch.looping, ch.playing);
            }
            if std::env::var("HP3_ZONE_DEBUG").is_ok() {
                let actors = hp3_world::world(&mut vm).actors.clone();
                let zone = vm.db.find("ZoneInfo");
                for a in actors {
                    if !vm.db.objects.get(a).class.zip(zone).is_some_and(|(c, z)| vm.is_child_of(c, z)) {
                        continue;
                    }
                    let name = hp3_script::natives::object_path(&vm, a);
                    let v: Vec<_> = ["AmbientBrightness", "AmbientHue", "AmbientSaturation", "AmbientVector", "DistanceFogColor", "DistanceFogStart", "DistanceFogEnd"]
                        .iter()
                        .map(|n| format!("{n}={:?}", hp3_world::get(&mut vm, a, n)))
                        .collect();
                    println!("  zone {name}: {}", v.join(" "));
                }
            }
            if std::env::var("HP3_TERRAIN_DEBUG").is_ok() {
                let actors = hp3_world::world(&mut vm).actors.clone();
                let ti = vm.db.find("TerrainInfo");
                for a in actors {
                    if vm.db.objects.get(a).class != ti {
                        continue;
                    }
                    println!("  terrain {} scale {:?} at {:?}", hp3_script::natives::object_path(&vm, a), hp3_world::get(&mut vm, a, "TerrainScale"), hp3_world::get(&mut vm, a, "Location"));
                    let Some(slot) = hp3_world::slot(&mut vm, a, "Layers") else { continue };
                    for i in 0..32 {
                        let Some(l) = vm.db.objects.values(a).and_then(|v| v.get(slot + i)).cloned() else { continue };
                        let f = l.fields().to_vec();
                        if f.first().and_then(hp3_script::Value::as_obj).is_none() {
                            continue;
                        }
                        let tex = f[0].as_obj().map(|t| hp3_script::natives::object_path(&vm, t));
                        let alpha = f[1].as_obj().map(|t| hp3_script::natives::object_path(&vm, t));
                        println!("    layer {i}: {tex:?} alpha {alpha:?} uscale {:?} vscale {:?} axis {:?} matrix {:?}", f[2], f[3], f[6], f.get(9));
                    }
                }
            }
            if std::env::var("HP3_HUD_DEBUG").is_ok() {
                let errors_before = vm.log.len();
                if let Some(hud) = hp3_world::get(&mut vm, pc, "myHUD").as_obj()
                    && let hp3_script::Value::Array(items) = hp3_world::get(&mut vm, hud, "HudItems")
                {
                    for it in items.iter().filter_map(hp3_script::Value::as_obj) {
                        let class = vm.db.objects.get(it).class.map(|c| vm.db.names.get(vm.db.structs[c.0 as usize].name).to_string());
                        let state = vm.state_of(it).map(|s| vm.db.names.get(vm.db.structs[s.0 as usize].name).to_string());
                        println!("    item {class:?} state {state:?}");
                    }
                }
                let tiles = hp3_world::canvas::render(&mut vm, 1280, 720);
                println!("  hud: {} tiles", tiles.len());
                let mut by_mat: std::collections::BTreeMap<String, usize> = Default::default();
                for t in &tiles {
                    let m = t.material.map(|m| hp3_script::natives::object_path(&vm, m)).unwrap_or_else(|| "None".into());
                    let m = format!("{m} style {} alpha {}", t.style, t.color[3]);
                    *by_mat.entry(m).or_default() += 1;
                }
                for (m, n) in by_mat.iter().take(25) {
                    println!("    {n:4} {m}");
                }
                for l in vm.log.iter().skip(errors_before).take(10) {
                    println!("    log: {l}");
                }
            }
            // HP3_CLICK=x,y clicks the menu at that point of a 1280x720 screen.
            if let Ok(c) = std::env::var("HP3_CLICK") {
                let _ = hp3_world::canvas::render(&mut vm, 1280, 720);
                for click in c.split(';') {
                    let v: Vec<f32> = click.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                    hp3_world::gui::mouse_move(&mut vm, v[0], v[1]);
                    hp3_world::gui::mouse_button(&mut vm, true);
                    hp3_world::gui::mouse_button(&mut vm, false);
                    for _ in 0..30 {
                        hp3_world::tick(&mut vm, 1.0 / 60.0);
                        if follow_travel(&mut vm) {
                            // The new level runs a while, as after a normal load.
                            let after: u32 = std::env::var("HP3_AFTER_TRAVEL").ok().and_then(|v| v.parse().ok()).unwrap_or(120);
                            for _ in 0..after {
                                hp3_world::tick(&mut vm, 1.0 / 60.0);
                            }
                        }
                    }
                    let _ = hp3_world::canvas::render(&mut vm, 1280, 720);
                }
                let gui = hp3_world::world(&mut vm).viewport.gui.unwrap();
                let ac = hp3_world::get(&mut vm, gui, "ActiveControl");
                println!("    active control after click: {ac:?}");
            }
            if std::env::var("HP3_GUI_DEBUG").is_ok()
                && let Some(gui) = hp3_world::world(&mut vm).viewport.gui
            {
                fn dump(vm: &mut hp3_script::Vm<'_>, o: hp3_script::ObjId, depth: usize) {
                    let class = vm.db.objects.get(o).class.map(|c| vm.db.names.get(vm.db.structs[c.0 as usize].name).to_string()).unwrap_or_default();
                    let mut info = String::new();
                    for n in ["WinLeft", "WinTop", "Caption", "MenuState", "bAcceptsInput", "RenderWeight", "__OnClick__Delegate"] {
                        let v = hp3_world::get(vm, o, n);
                        let v = match v {
                            hp3_script::Value::Empty => continue,
                            hp3_script::Value::Object(Some(x)) => hp3_script::natives::object_path(vm, x),
                            other => format!("{other:?}"),
                        };
                        info.push_str(&format!(" {n}={v}"));
                    }
                    println!("    {}{class}{info}", "  ".repeat(depth));
                    if let hp3_script::Value::Array(c) = hp3_world::get(vm, o, "Controls") {
                        for x in c.iter().filter_map(hp3_script::Value::as_obj) {
                            dump(vm, x, depth + 1);
                        }
                    }
                }
                if let hp3_script::Value::Array(styles) = hp3_world::get(&mut vm, gui, "StyleStack") {
                    for st in styles.iter().filter_map(hp3_script::Value::as_obj).take(40) {
                        let key = hp3_world::get(&mut vm, st, "KeyName").as_str().to_string();
                        if key != "HPSolidBoxFE" {
                            continue;
                        }
                        let slot = hp3_world::slot(&mut vm, st, "Fonts").unwrap();
                        let fonts: Vec<String> = (0..5).map(|i| vm.db.objects.values(st).unwrap()[slot + i].as_obj().map(|f| hp3_script::natives::object_path(&vm, f)).unwrap_or_default()).collect();
                        let ns = hp3_world::slot(&mut vm, st, "FontNames").unwrap();
                        let names: Vec<String> = (0..5).map(|i| vm.db.objects.values(st).unwrap()[ns + i].as_str().to_string()).collect();
                        println!("    style {key}: fonts {fonts:?} names {names:?}");
                    }
                }
                if let hp3_script::Value::Array(pages) = hp3_world::get(&mut vm, gui, "MenuStack") {
                    for p in pages.iter().filter_map(hp3_script::Value::as_obj) {
                        dump(&mut vm, p, 0);
                    }
                }
            }
            if std::env::var("HP3_ATTACH_DEBUG").is_ok() {
                let list: Vec<_> = hp3_world::world(&mut vm).anim.attachments.iter().map(|(k, v)| (*k, v.clone())).collect();
                for (child, (parent, bone)) in list {
                    let class = vm.db.objects.get(child).class.map(|c| vm.db.names.get(vm.db.structs[c.0 as usize].name).to_string()).unwrap_or_default();
                    let mesh = hp3_world::get(&mut vm, child, "Mesh").as_obj().map(|m| hp3_script::natives::object_path(&vm, m));
                    let sm = hp3_world::get(&mut vm, child, "StaticMesh").as_obj().map(|m| hp3_script::natives::object_path(&vm, m));
                    let dt = hp3_world::get(&mut vm, child, "DrawType").as_byte();
                    let ds = hp3_world::get(&mut vm, child, "DrawScale");
                    let ds3 = hp3_world::get(&mut vm, child, "DrawScale3D");
                    let rl = hp3_world::get(&mut vm, child, "RelativeLocation");
                    println!("  attach {class} to {parent:?}.{bone}: dt {dt} mesh {mesh:?} sm {sm:?} scale {ds:?} {ds3:?} rel {rl:?}");
                }
            }
            if std::env::var("HP3_ANIM_DEBUG").is_ok() {
                let actors = hp3_world::world(&mut vm).actors.clone();
                for a in actors {
                    if hp3_world::get(&mut vm, a, "DrawType").as_byte() != 2 || vm.db.objects.get(a).deleted {
                        continue;
                    }
                    let class = vm.db.objects.get(a).class.map(|c| vm.db.names.get(vm.db.structs[c.0 as usize].name).to_string()).unwrap_or_default();
                    let ch = hp3_world::world(&mut vm).anim.actors.get(&a).and_then(|x| x.channels.first().cloned());
                    let seq = hp3_world::get(&mut vm, a, "AnimSequence");
                    let phys = hp3_world::get(&mut vm, a, "Physics").as_byte();
                    let state = vm.state_of(a).map(|s| vm.db.names.get(vm.db.structs[s.0 as usize].name).to_string());
                    let upd = hp3_world::get(&mut vm, a, "bPhysicsAnimUpdate").as_bool();
                    println!("  anim {class:<28} phys {phys} state {state:?} AnimSequence {seq:?} physanim {upd} ch0 {:?}", ch.map(|c| (c.sequence, c.frame as i32, c.playing)));
                }
            }
            if let Some(m) = hp3_world::get(&mut vm, p, "Mesh").as_obj() {
                println!("  pawn mesh {}", hp3_script::natives::object_path(&vm, m));
            }
            for n in ["Acceleration", "Velocity", "GroundSpeed", "AccelRate", "Controller", "AnimSequence"] {
                println!("  pawn.{n} = {:?}", hp3_world::get(&mut vm, p, n));
            }
            if let Some(c) = hp3_world::get(&mut vm, p, "Controller").as_obj() {
                let cc = vm.db.objects.get(c).class.map(|c| vm.db.names.get(vm.db.get(c).name).to_string());
                let st = vm.state_of(c).map(|s| vm.db.names.get(vm.db.get(s).name).to_string());
                println!("  pawn's controller is {cc:?} in state {st:?}");
            }
            println!("  pc.PlayerInput = {:?}", hp3_world::get(&mut vm, pc, "PlayerInput"));
            let mut args = [hp3_script::Value::Object(None), hp3_script::Value::Empty, hp3_script::Value::Empty];
            let r = vm.event(pc, "PlayerCalcView", &mut args);
            println!("  PlayerCalcView -> {r:?}: view actor {:?}, location {:?}, rotation {:?}", args[0], args[1], args[2]);
            if let Some(cam) = args[0].as_obj() {
                let cc = vm.db.objects.get(cam).class.map(|c| vm.db.names.get(vm.db.get(c).name).to_string());
                let st = vm.state_of(cam).map(|s| vm.db.names.get(vm.db.get(s).name).to_string());
                let loc = hp3_world::get(&mut vm, cam, "Location");
                let phys = hp3_world::get(&mut vm, cam, "Physics");
                println!("  view actor {cc:?} state {st:?} at {loc:?} physics {phys:?} latent {:?}", vm.latent.get(&cam));
            }
            let pclass = vm.db.objects.get(p).class.map(|c| vm.db.names.get(vm.db.get(c).name).to_string());
            let loc = hp3_world::get(&mut vm, p, "Location");
            let phys = hp3_world::get(&mut vm, p, "Physics");
            let pstate = vm.state_of(p).map(|s| vm.db.names.get(vm.db.get(s).name).to_string());
            println!("pawn {pclass:?} at {loc:?}, physics {phys:?}, state {pstate:?}");
        } else {
            println!("no pawn");
        }
    }
    let mut grouped: BTreeMap<String, usize> = BTreeMap::new();
    for e in &errors {
        let key = e.split_once(' ').map(|(_, r)| r.to_string()).unwrap_or_else(|| e.clone());
        *grouped.entry(key).or_default() += 1;
    }
    let mut g: Vec<_> = grouped.into_iter().collect();
    g.sort_by(|a, b| b.1.cmp(&a.1));
    println!("{} script errors, {} kinds:", errors.len(), g.len());
    for (e, n) in g.iter().take(25) {
        println!("  {n:5} {e}");
    }
    let mut missing: Vec<_> = vm.missing.iter().collect();
    missing.sort_by(|a, b| b.1.cmp(a.1));
    println!("{} unimplemented natives hit:", missing.len());
    for (n, c) in missing.iter().take(40) {
        println!("  {c:7} {n}");
    }
    if std::env::var_os("HP3_TICK_PROFILE").is_some() {
        println!("time per tick phase:");
        for (phase, t) in hp3_world::world(&mut vm).tick_profile.clone() {
            println!("  {:>10.1} ms {phase}", t.as_secs_f64() * 1000.0);
        }
    }
    if vm.profile {
        let mut p: Vec<_> = vm.profile_counts.iter().map(|(f, n)| (*n, *f)).collect();
        p.sort_by(|a, b| b.0.cmp(&a.0));
        let mut n: Vec<_> = vm.profile_natives.iter().map(|(f, (c, t))| (*t, *c, *f)).collect();
        n.sort_by(|a, b| b.0.cmp(&a.0));
        println!("time per native:");
        for (t, c, f) in n.iter().take(15) {
            println!("  {:>10.1} ms {c:9} calls {}", t.as_secs_f64() * 1000.0, vm.function_path(*f));
        }
        println!("statements per function:");
        let top = std::env::var("HP3_PROFILE").ok().and_then(|v| v.parse().ok()).unwrap_or(20);
        for (n, f) in p.iter().take(top) {
            println!("  {n:10} {}", vm.function_path(*f));
        }
    }
    let mut logs: BTreeMap<&str, usize> = BTreeMap::new();
    for l in &vm.log {
        *logs.entry(l.as_str()).or_default() += 1;
    }
    println!("{} log lines ({} distinct); first ones:", vm.log.len(), logs.len());
    if vm.trace_states {
        for l in vm.log.iter().filter(|l| l.starts_with("state:")) {
            println!("  {l}");
        }
    }
    let shown = std::env::var("HP3_LOG_LINES").ok().and_then(|v| v.parse().ok()).unwrap_or(20);
    for (l, n) in logs.iter().take(shown) {
        println!("  {n:5} {l}");
    }
    Ok(ExitCode::SUCCESS)
}

/// Compiles every script and reports opcodes the VM lacks and natives that are called but not implemented.
fn compile_all(root: &Path) -> Result<ExitCode, Box<dyn std::error::Error>> {
    use hp3_script::compile::Expr;
    use std::collections::BTreeMap;
    let set = PackageSet::open(root)?;
    let mut db = ClassDb::new(&set);
    db.load_all_classes();
    let mut vm = hp3_script::Vm::new(db);
    let ids: Vec<_> = (0..vm.db.structs.len()).map(|i| hp3_script::StructId(i as u32)).collect();
    let mut unsupported: BTreeMap<u8, usize> = BTreeMap::new();
    let mut missing: BTreeMap<String, usize> = BTreeMap::new();
    let mut statements = 0;
    fn walk(e: &Expr, f: &mut dyn FnMut(&Expr)) {
        f(e);
        let kids: Vec<&Expr> = match e {
            Expr::Let(a, b) | Expr::ArrayElement(a, b) | Expr::DynArrayElement(a, b) | Expr::StructCmp(_, a, b)
            | Expr::Context(a, b) | Expr::ClassContext(a, b) => vec![a, b],
            Expr::JumpIfNot(_, a) | Expr::Return(a) | Expr::Switch(a) | Expr::GotoLabel(a) | Expr::Assert(_, a)
            | Expr::Discard(a) | Expr::DynArrayLength(a) | Expr::StructMember(_, a) | Expr::Cast(_, a)
            | Expr::DynamicCast(_, a) | Expr::MetaCast(_, a) | Expr::Iterator(a, _) => vec![a],
            Expr::Case(_, Some(a)) => vec![a],
            Expr::DynArrayInsert(a, b, c) | Expr::DynArrayRemove(a, b, c) => vec![a, b, c],
            Expr::Virtual(_, v) | Expr::Global(_, v) | Expr::Final(_, v) | Expr::Native(_, _, v) | Expr::New(v) => v.iter().collect(),
            _ => vec![],
        };
        for k in kids {
            walk(k, f);
        }
    }
    let t = std::time::Instant::now();
    for id in ids {
        if vm.db.get(id).script.is_empty() {
            continue;
        }
        let code = vm.code(id);
        statements += code.statements.len();
        let mut calls = Vec::new();
        for s in &code.statements {
            walk(s, &mut |e| match e {
                Expr::Unsupported(op) => *unsupported.entry(*op).or_default() += 1,
                Expr::Final(f, _) | Expr::Native(_, Some(f), _) => calls.push(*f),
                Expr::Native(i, None, _) => *missing.entry(format!("native #{i} (no declaration)")).or_default() += 1,
                _ => {}
            });
        }
        for f in calls {
            if vm.db.get(f).function_flags & hp3_script::vm::FUNC_NATIVE != 0 && !vm.has_native(f) {
                *missing.entry(vm.function_path(f)).or_default() += 1;
            }
        }
    }
    println!("compiled {statements} statements in {:?}", t.elapsed());
    println!("unsupported opcodes: {unsupported:?}");
    let mut by_count: Vec<_> = missing.into_iter().collect();
    by_count.sort_by(|a, b| b.1.cmp(&a.1));
    println!("{} natives called but not implemented (call sites):", by_count.len());
    for (n, c) in by_count.iter().take(60) {
        println!("  {c:5} {n}");
    }
    Ok(ExitCode::SUCCESS)
}

fn parse_arg(s: &str) -> hp3_script::Value {
    use hp3_script::Value;
    if let Ok(i) = s.parse::<i32>() {
        Value::Int(i)
    } else if let Ok(f) = s.parse::<f32>() {
        Value::Float(f)
    } else if s.eq_ignore_ascii_case("true") || s.eq_ignore_ascii_case("false") {
        Value::Bool(s.eq_ignore_ascii_case("true"))
    } else {
        Value::Str(s.to_string())
    }
}

/// Calls `Class.Function` on a fresh instance of the class with the given arguments.
fn call(root: &Path, class: &str, func: &str, args: &[String]) -> Result<ExitCode, Box<dyn std::error::Error>> {
    let set = PackageSet::open(root)?;
    let mut db = ClassDb::new(&set);
    db.load_all_classes();
    let mut vm = hp3_script::Vm::new(db);
    let cid = vm.db.find(class).ok_or("no such class")?;
    let fname = vm.db.names.find(func).ok_or("no such function name")?;
    let f = vm.find_function(None, Some(cid), fname).ok_or("no such function")?;
    let name = vm.db.names.intern("TestObject");
    let obj = vm.new_object(cid, name);
    let mut values: Vec<_> = args.iter().map(|a| parse_arg(a)).collect();
    let t = std::time::Instant::now();
    let r = vm.invoke(f, Some(obj), &mut values);
    println!("{} -> {:?} ({} statements, {:?})", vm.function_path(f), r, vm.steps, t.elapsed());
    for (i, v) in values.iter().enumerate() {
        println!("  arg {i} after: {v:?}");
    }
    for l in &vm.log {
        println!("  log: {l}");
    }
    Ok(if r.is_ok() { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

/// Builds the property values of every scripted object in every map (or one map) and reports problems.
fn instances(root: &Path, only: Option<&str>) -> Result<ExitCode, Box<dyn std::error::Error>> {
    let set = PackageSet::open(root)?;
    let mut db = ClassDb::new(&set);
    db.load_all_classes();
    let before = (db.stats.tags_applied, db.stats.unknown_tags, db.stats.mismatched_tags, db.stats.unresolved_objects);
    let (mut maps, mut objects, mut native_only, mut deleted) = (0, 0, 0, 0);
    for path in set.files() {
        if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("unr")) {
            continue;
        }
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        if only.is_some_and(|m| !m.eq_ignore_ascii_case(&stem)) {
            continue;
        }
        let pkg = set.get(&stem)?;
        maps += 1;
        for i in 0..pkg.exports.len() {
            if pkg.exports[i].class.is_none() {
                continue;
            }
            // Actors deleted in the editor stay in the file but are never spawned; their last struct tag is
            // written without its terminator.
            let chain = set.export_chain(&pkg, i);
            if let Ok(o) = hp3_package::object::read_export(&pkg, i, &chain)
                && matches!(o.body, hp3_package::object::Body::Deleted { .. })
            {
                deleted += 1;
                continue;
            }
            match db.instance_values(&pkg, i) {
                Some(_) => objects += 1,
                None => native_only += 1,
            }
        }
    }
    let s = &db.stats;
    println!(
        "{maps} maps, {objects} scripted objects, {native_only} objects of native-only classes, {deleted} deleted \
         actors skipped; {} tags applied, \
         {} unknown, {} mismatched, {} unresolved objects",
        s.tags_applied - before.0,
        s.unknown_tags - before.1,
        s.mismatched_tags - before.2,
        s.unresolved_objects - before.3
    );
    for p in &s.problems {
        println!("  {p}");
    }
    Ok(if s.unknown_tags == 0 && s.mismatched_tags == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

fn classes(root: &Path, show: Option<&str>) -> Result<ExitCode, Box<dyn std::error::Error>> {
    let set = PackageSet::open(root)?;
    let mut db = ClassDb::new(&set);
    let t = std::time::Instant::now();
    db.load_all_classes();
    let s = &db.stats;
    println!(
        "{} classes, {} structs/states/functions in all, {} properties, {} names, loaded in {:?}",
        s.classes,
        s.structs,
        s.props,
        db.names.len(),
        t.elapsed()
    );
    println!(
        "defaults: {} tags applied, {} for unknown properties, {} not matching their property, {} unresolved objects",
        s.tags_applied, s.unknown_tags, s.mismatched_tags, s.unresolved_objects
    );
    for p in &s.problems {
        println!("  {p}");
    }
    if let Some(name) = show {
        let id = db.find(name).ok_or("no such class")?;
        let def = db.get(id);
        println!("\n{} ({:?}), {} slots", db.names.get(def.name), def.kind, def.slots);
        for p in &def.props {
            let owner = db.names.get(db.get(p.owner).name);
            println!("  {owner}.{} {:?}[{}] = {:?}", db.names.get(p.name), p.ty, p.dim, &def.defaults[p.slot]);
        }
    }
    Ok(if s.unknown_tags == 0 && s.mismatched_tags == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

/// One line per visible pawn that hovers more than 6 units over the floor below it.
fn float_check(vm: &mut hp3_script::Vm<'_>) -> Vec<String> {
    use hp3_world::{get, physics::vec};
    let Some(pawn_class) = vm.db.find("Pawn") else { return Vec::new() };
    let actors = hp3_world::world(vm).actors.clone();
    let mut out = Vec::new();
    for a in actors {
        let Some(c) = vm.db.objects.get(a).class else { continue };
        if vm.db.objects.get(a).deleted || !vm.is_child_of(c, pawn_class) || get(vm, a, "bHidden").as_bool() || get(vm, a, "Mesh").as_obj().is_none() {
            continue;
        }
        let loc = vec(&get(vm, a, "Location"));
        let (r, h) = (get(vm, a, "CollisionRadius").as_float(), get(vm, a, "CollisionHeight").as_float());
        let hit = hp3_world::world(vm).collision.trace_static(loc, loc - glam::Vec3::Z * 400.0, glam::Vec3::new(r.min(8.0), r.min(8.0), 1.0));
        // The mesh's feet are drawn at Location + PrePivot; the floor is `d` below Location.
        let prepivot = hp3_world::mesh_offset(vm, a).z;
        let gap = hit.map(|h2| h2.time * 400.0 + prepivot);
        let name = vm.db.names.get(vm.db.objects.get(a).name).to_string();
        let anim = get(vm, a, "AnimSequence").as_name();
        let anim = vm.db.names.get(anim).to_string();
        if gap.is_none_or(|g| g.abs() > 6.0) {
            out.push(format!(
                "floating: {name} at {loc} gap {} physics {} anim {anim} (collision radius {r} height {h})",
                gap.map_or("no floor".into(), |g| format!("{g:.1}")),
                get(vm, a, "Physics").as_byte()
            ));
        }
    }
    out
}
