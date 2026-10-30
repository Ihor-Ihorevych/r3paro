//! `hp3-run`: headless runner.
//!
//! - `classes <HP3 folder> [class]`: loads every script class, builds its defaults and reports problems; with a
//!   class name, prints that class's properties and default values.

use std::path::Path;
use std::process::ExitCode;

use hp3_package::PackageSet;
use hp3_script::ClassDb;

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
    let steps_after_load = vm.steps;
    // Keys held for the whole run, e.g. HP3_HOLD=W,D.
    if let Ok(keys) = std::env::var("HP3_HOLD") {
        for k in keys.split(',') {
            hp3_world::world(&mut vm).input.key_down(k.trim());
        }
    }
    let t = std::time::Instant::now();
    for _ in 0..frames {
        hp3_world::tick(&mut vm, 1.0 / 60.0);
    }
    let tick_time = t.elapsed();
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
    if vm.profile {
        let mut p: Vec<_> = vm.profile_counts.iter().map(|(f, n)| (*n, *f)).collect();
        p.sort_by(|a, b| b.0.cmp(&a.0));
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
