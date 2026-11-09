//! `hp3-run`: headless runner.
//!
//! - `classes <HP3 folder> [class]`: loads every script class, builds its defaults and reports problems; with a
//!   class name, prints that class's properties and default values.

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
use hp3_world::casttest;
mod itemcheck;
mod langcheck;
mod mounttest;
mod saveload;

use std::path::Path;
use std::process::ExitCode;

use hp3_package::PackageSet;
use hp3_script::ClassDb;

/// HP3_TRIGGER=<tag> (or @<actor name>) triggers the actors with that tag (as an event of that name would), instigated by the
/// player's pawn.
fn trigger_tag(vm: &mut hp3_script::Vm<'_>) {
    let Ok(tag) = std::env::var("HP3_TRIGGER") else { return };
    trigger_named(vm, &tag);
}

/// HP3_TRIGGERS=<tag>@<tick>,<tag>@<tick>: triggers several tags at their own ticks.
fn trigger_list(vm: &mut hp3_script::Vm<'_>, frame: u32) {
    let Ok(spec) = std::env::var("HP3_TRIGGERS") else { return };
    for item in spec.split(',') {
        if let Some((tag, tick)) = item.rsplit_once('@')
            && tick.trim().parse::<u32>() == Ok(frame)
        {
            trigger_named(vm, tag.trim());
        }
    }
}

fn trigger_named(vm: &mut hp3_script::Vm<'_>, tag: &str) {
    let tag = tag.to_string();
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
    // HP3_SPAWN=<class>: one actor of that class is made first (to read the defaults of an effect nothing in the level has made yet).
    if let Ok(name) = std::env::var("HP3_SPAWN")
        && let Some(c) = hp3_world::find_object(vm, &name)
    {
        let li = hp3_world::world(vm).level_info;
        match hp3_world::natives::spawn(vm, li, &[hp3_script::Value::Object(Some(c))]) {
            Ok(v) => println!("spawned {name}: {v:?}"),
            Err(e) => println!("spawn {name} failed: {e:?}"),
        }
    }
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
                    hp3_script::Value::Object(Some(x)) => hp3_script::natives::object_path(vm, x),
                    other => format!("{other:?}"),
                };
                info.push_str(&format!(" {n}={v}"));
            }
            let state = vm.state_of(a).map(|s| vm.db.names.get(vm.db.structs[s.0 as usize].name).to_string());
            if std::env::var_os("HP3_LIST_OFFSET").is_some() {
                let off = hp3_world::mesh_offset(vm, a);
                info.push_str(&format!(" mesh_offset_z={:.2}", off.z));
            }
            if let Some((lo, hi)) = hp3_world::volume_bounds(vm, a) {
                info.push_str(&format!(" bounds {:.0} .. {:.0}", lo, hi));
            }
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
                        if std::env::var_os("HP3_LIST_EMITTER_PROPS").is_some() {
                            let mut line = String::new();
                            for n in std::env::var("HP3_LIST_EMITTER_ONLY").map(|v| v.split(',').map(String::from).collect::<Vec<_>>()).unwrap_or_else(|_| ["RotationNormal", "UseRotationFrom", "RotationOffset", "EffectAxis", "CoordinateSystem", "SpinParticles", "UseDirectionAs", "MaxParticles", "InitialParticlesPerSecond", "ParticlesPerSecond", "AutomaticInitialSpawning", "StartLocationShape", "SphereRadiusRange", "StartLocationRange", "LifetimeRange", "StartLocationPolarRange", "ColorScale", "ColorMultiplierRange", "FadeOutStartTime", "FadeInEndTime", "StartSizeRange", "Opacity", "RespawnDeadParticles", "Disabled", "UseCollision", "SecondsBeforeInactive", "ParticleLifeTime"].iter().map(|s| s.to_string()).collect()) {
                                line.push_str(&format!(" {n}={:?}", hp3_world::get(vm, eo, &n)));
                            }
                            println!("        {line}");
                        }
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
        Ok(m) => {
            println!("travelled to {} ({url})", m.name);
            if std::env::var_os("HP3_TRAVEL_DEBUG").is_some() {
                let pc = hp3_world::world(vm).player;
                let pawn = pc.and_then(|pc| hp3_world::get(vm, pc, "Pawn").as_obj());
                println!("after travel: player {pc:?} pawn {pawn:?}");
                if let Some(pawn) = pawn {
                    let name = vm.db.names.intern("Jellybean");
                    let count = hp3_world::fire(vm, pawn, "GetInventoryCount", &mut [hp3_script::Value::Name(name)]);
                    println!("after travel: beans {count:?}");
                }
            }
        }
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
        Some("items") if args.len() >= 4 => itemcheck::run(Path::new(&args[1]), &args[2], &args[3]),
        Some("lang") if args.len() >= 3 => langcheck::check(Path::new(&args[1]), &args[2], args.get(3).map(Path::new)),
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
    hp3_world::memprobe("pre-load");
    hp3_world::load(&mut vm, &pkg, hp3_core::Config::load(root))?;
    let load_time = t.elapsed();
    hp3_world::memprobe("load returned");
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
    // HP3_POLYS=x0,y0,z0,x1,y1,z1: every BSP polygon of the level that touches that box, with its flags.
    if let Ok(spec) = std::env::var("HP3_POLYS") {
        let v: Vec<f32> = spec.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        if v.len() == 6 {
            for line in hp3_world::world(&mut vm).collision.debug_polygons_in(glam::Vec3::new(v[0], v[1], v[2]), glam::Vec3::new(v[3], v[4], v[5])) {
                println!("{line}");
            }
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
                    let hit = hp3_world::world(&mut vm).collision.trace_as(std::env::var_os("HP3_PROBE_NONPLAYER").is_none(), from, glam::Vec3::new(x, y, v[6]), glam::Vec3::new(2.0, 2.0, 2.0) * if std::env::var_os("HP3_PROBE_PAWN").is_some() { glam::Vec3::new(7.5, 7.5, 19.0) } else { glam::Vec3::ONE }, None);
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
                let hit = hp3_world::world(&mut vm).collision.trace_as(true, glam::Vec3::new(v[0], v[4], z), glam::Vec3::new(v[0], v[5], z), probe_ext(), None);
                match hit {
                    Some(h) => println!("z {z:7.1}: stops at y {:8.1} normal ({:.2},{:.2},{:.2}) actor {:?}", h.location.y, h.normal.x, h.normal.y, h.normal.z, h.actor.map(|o| hp3_script::natives::object_path(&vm, o))),
                    None => println!("z {z:7.1}: free"),
                }
                z += v[3];
            }
        }
    }
    // HP3_BONES=<actor name>: every bone of that actor's mesh, as a height against its location.
    if let Ok(name) = std::env::var("HP3_BONES") {
        let actors = hp3_world::world(&mut vm).actors.clone();
        for a in actors {
            if !hp3_script::natives::object_path(&vm, a).ends_with(&format!(".{name}")) {
                continue;
            }
            let at = hp3_world::physics::vec(&hp3_world::get(&mut vm, a, "Location"));
            let Some(mesh) = hp3_world::get(&mut vm, a, "Mesh").as_obj() else { continue };
            let Some(skel) = hp3_world::anim::skeleton_of(&mut vm, mesh) else { continue };
            for n in skel.skeleton.names.clone() {
                if let Some(m) = hp3_world::anim::bone_matrix(&mut vm, a, &n) {
                    let p = m.w_axis.truncate() - at;
                    println!("bone {n}: ({:.1},{:.1},{:.1})", p.x, p.y, p.z);
                }
            }
            println!("mesh offset {:?}", hp3_world::mesh_offset(&mut vm, a));
            println!("view offset {}", hp3_world::physics::view_offset(&mut vm, a));
        }
    }
    // HP3_PROBE_GATE=cx,cy,dirx,diry,reach,lat0,lat1,latstep,z0,z1,zstep: lines (a pawn's box, HP3_PROBE_EXT) are driven through
    // a wall opening centred at cx,cy along the direction (dirx,diry); a '#' is a line stopped within the reach, '.' one
    // that passes. Rows are heights (top first), columns are sideways offsets.
    if let Ok(spec) = std::env::var("HP3_PROBE_GATE") {
        let v: Vec<f32> = spec.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        if v.len() == 11 {
            let dir = glam::Vec2::new(v[2], v[3]).normalize();
            let side = glam::Vec2::new(-dir.y, dir.x);
            let mut z = v[9];
            while z >= v[8] {
                let mut row = String::new();
                let mut lat = v[5];
                while lat <= v[6] {
                    let c = glam::Vec2::new(v[0], v[1]) + side * lat;
                    let a = c - dir * v[4];
                    let b = c + dir * v[4];
                    let hit = hp3_world::world(&mut vm).collision.trace_as(true, glam::Vec3::new(a.x, a.y, z), glam::Vec3::new(b.x, b.y, z), probe_ext(), None);
                    row.push(if hit.is_some_and(|h| h.time < 1.0) { '#' } else { '.' });
                    lat += v[7];
                }
                println!("z {z:6.0} {row}");
                z -= v[10];
            }
        }
    }
    // HP3_TAGS=<text>: lists the actors whose tag contains the text (class, place, collision flags).
    if let Ok(text) = std::env::var("HP3_TAGS") {
        for a in hp3_world::world(&mut vm).actors.clone() {
            let tag = hp3_world::get(&mut vm, a, "Tag").as_name();
            let tag = vm.db.names.get(tag).to_string();
            if tag.to_lowercase().contains(&text.to_lowercase()) {
                let class = vm.db.objects.get(a).class.map(|c| vm.db.names.get(vm.db.get(c).name).to_string()).unwrap_or_default();
                let at = hp3_world::physics::vec(&hp3_world::get(&mut vm, a, "Location"));
                println!("tag {tag}: {} [{class}] at {at}", hp3_script::natives::object_path(&vm, a));
                if class == "KWSlideJump" {
                    let dest = hp3_world::get(&mut vm, a, "JumpDest").as_obj().map(|d| hp3_script::natives::object_path(&vm, d));
                    let want = hp3_world::get(&mut vm, a, "TagOfJumpDest").as_name();
                    println!("  jump dest {dest:?} wants tag {}", vm.db.names.get(want));
                }
            }
        }
    }
    // HP3_PROBE_Z=x,y,z_top,z_bottom: drops a line from the top to the bottom there, printing each surface it meets.
    if let Ok(spec) = std::env::var("HP3_PROBE_Z") {
        let v: Vec<f32> = spec.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        if v.len() == 4 {
            let mut top = v[2];
            for _ in 0..12 {
                let Some(h) = hp3_world::world(&mut vm).collision.trace_static(glam::Vec3::new(v[0], v[1], top), glam::Vec3::new(v[0], v[1], v[3]), glam::Vec3::ZERO) else { break };
                println!("surface z {:.1} actor {:?}", h.location.z, h.actor.map(|o| hp3_script::natives::object_path(&vm, o)));
                top = h.location.z - 0.5;
            }
        }
    }
    // HP3_PROBE_PATH=x,y,z;x,y,z;...: sweeps a box (HP3_PROBE_EXT=x,y,z, default the hippogriff's 30,30,60) as the player
    // from each point to the next, printing every blocker on the way (to find invisible walls along a flight).
    if let Ok(spec) = std::env::var("HP3_PROBE_PATH") {
        let ext: Vec<f32> = std::env::var("HP3_PROBE_EXT").unwrap_or("30,30,60".into()).split(',').filter_map(|p| p.trim().parse().ok()).collect();
        let ext = glam::Vec3::new(ext[0], ext[1], ext[2]);
        let pts: Vec<glam::Vec3> = spec
            .split(';')
            .filter_map(|p| {
                let v: Vec<f32> = p.split(',').filter_map(|c| c.trim().parse().ok()).collect();
                (v.len() == 3).then(|| glam::Vec3::new(v[0], v[1], v[2]))
            })
            .collect();
        for w in pts.windows(2) {
            let (mut from, to) = (w[0], w[1]);
            println!("path {} -> {}", w[0], w[1]);
            for _ in 0..20 {
                let Some(h) = hp3_world::world(&mut vm).collision.trace_as(true, from, to, ext, None) else { break };
                println!("  blocked at {} normal {} actor {:?}", h.location, h.normal, h.actor.map(|o| hp3_script::natives::object_path(&vm, o)));
                let dir = (to - from).normalize_or_zero();
                from = h.location + dir * 40.0;
                if (to - from).dot(dir) <= 0.0 {
                    break;
                }
            }
        }
    }
    // HP3_PROBE_X=y,z0,z1,step,x0,x1: traces a pawn-sized box along +x at each height, printing where it stops.
    if let Ok(spec) = std::env::var("HP3_PROBE_X") {
        let v: Vec<f32> = spec.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        if v.len() == 6 {
            let mut z = v[1];
            while z <= v[2] {
                let hit = hp3_world::world(&mut vm).collision.trace_as(true, glam::Vec3::new(v[4], v[0], z), glam::Vec3::new(v[5], v[0], z), probe_ext(), None);
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
        // HP3_TRACE_ACTORS=sx,sy,sz,ex,ey,ez@<tick>: what the player's spell cursor's TraceActors (extent 1) meets on that line.
        if let Some((line, tick)) = std::env::var("HP3_TRACE_ACTORS").ok().as_deref().and_then(|s| s.split_once('@')).map(|(a, b)| (a.to_string(), b.parse::<u32>().unwrap_or(0)))
            && tick == frame
        {
            let v: Vec<f32> = line.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            let cursor = hp3_world::world(&mut vm).player.and_then(|pc| hp3_world::get(&mut vm, pc, "Cursor").as_obj());
            if let (6, Some(c)) = (v.len(), cursor) {
                let (s, e) = (glam::Vec3::new(v[0], v[1], v[2]), glam::Vec3::new(v[3], v[4], v[5]));
                for (name, time, at) in hp3_world::natives::debug_trace_actors(&mut vm, c, s, e, glam::Vec3::ONE) {
                    println!("traceactors {frame}: {name} at {time:.3} {at}");
                }
            }
        }
        // HP3_DYN_LIGHTS=<every>: prints the live dynamic lights (bDynamicLight actors with a LightType) every so many ticks.
        if std::env::var("HP3_DYN_LIGHTS").ok().and_then(|v| v.parse::<u32>().ok()).is_some_and(|e| e > 0 && frame % e == 0) {
            for a in hp3_world::world(&mut vm).actors.clone() {
                if vm.db.objects.get(a).deleted || hp3_world::get(&mut vm, a, "LightType").as_byte() == 0 || !hp3_world::get(&mut vm, a, "bDynamicLight").as_bool() {
                    continue;
                }
                let path = hp3_script::natives::object_path(&mut vm, a);
                let at = hp3_world::physics::vec(&hp3_world::get(&mut vm, a, "Location"));
                let b = |vm: &mut hp3_script::Vm<'_>, n: &str| hp3_world::get(vm, a, n).as_byte();
                println!(
                    "dynlight {frame} {path} at {at} type {} effect {} hue {} sat {} bright {} radius {} hidden {:?}",
                    b(&mut vm, "LightType"),
                    b(&mut vm, "LightEffect"),
                    b(&mut vm, "LightHue"),
                    b(&mut vm, "LightSaturation"),
                    hp3_world::get(&mut vm, a, "LightBrightness").as_float(),
                    hp3_world::get(&mut vm, a, "LightRadius").as_float(),
                    hp3_world::get(&mut vm, a, "bHidden")
                );
            }
        }
        // HP3_SPAWN_FADE=<Package.Class>: that class is spawned at tick 50 and sent `FadeOut(0.1)` at tick 60 (an effect that should fade
        // and destroy itself); whether it is still there is printed from tick 60 on, every 20 ticks.
        if let Ok(class) = std::env::var("HP3_SPAWN_FADE") {
            thread_local! { static FADER: std::cell::Cell<Option<hp3_script::ObjId>> = const { std::cell::Cell::new(None) }; }
            if frame == 50 && let Some(c) = hp3_world::find_object(&mut vm, &class) {
                let li = hp3_world::world(&mut vm).level_info;
                if let Ok(hp3_script::Value::Object(Some(o))) = hp3_world::natives::spawn(&mut vm, li, &[hp3_script::Value::Object(Some(c))]) {
                    FADER.with(|f| f.set(Some(o)));
                    println!("fade test: spawned {}", hp3_script::natives::object_path(&vm, o));
                }
            }
            if let Some(o) = FADER.with(|f| f.get()) {
                if frame == 60 {
                    hp3_world::fire(&mut vm, o, "FadeOut", &mut [hp3_script::Value::Float(0.1)]);
                }
                if frame >= 60 && frame % 20 == 0 {
                    let state = vm.state_of(o).map(|s| vm.db.names.get(vm.db.get(s).name).to_string());
                    println!("fade test {frame}: deleted {} state {state:?}", vm.db.objects.get(o).deleted);
                }
            }
        }
        // HP3_GOD=1: the player's controller stays in god mode (as the `cheat god` command), so a fight can be left running.
        if std::env::var_os("HP3_GOD").is_some()
            && let Some(pc) = hp3_world::world(&mut vm).player
        {
            hp3_world::set(&mut vm, pc, "bGodMode", hp3_script::Value::Bool(true));
        }
        // HP3_CAMERA_LOG=<every>: the game's camera (position, turn, field of view) and the player's pawn place every so many ticks.
        if let Ok(every) = std::env::var("HP3_CAMERA_LOG").map(|v| v.parse::<u32>().unwrap_or(0))
            && every > 0
            && frame % every == 0
        {
            let view = hp3_world::player_view(&mut vm);
            let pawn = hp3_world::world(&mut vm).player.and_then(|pc| hp3_world::get(&mut vm, pc, "Pawn").as_obj());
            let at = pawn.map(|p| hp3_world::physics::vec(&hp3_world::get(&mut vm, p, "Location")));
            let game = hp3_world::world(&mut vm).level_info.and_then(|li| hp3_world::get(&mut vm, li, "Game").as_obj());
            let state = game.map(|g| hp3_world::get(&mut vm, g, "CurrentGameState").as_str().to_string());
            println!("camera {frame}: game state {state:?}");
            println!("camera {frame}: view {view:?} pawn {at:?}");
        }
        // HP3_WATCH=<name>,<name>@<every>: prints those actors' place, turn and state every so many ticks.
        if let Ok(spec) = std::env::var("HP3_WATCH")
            && let Some((names, every)) = spec.split_once('@')
            && every.parse::<u32>().is_ok_and(|e| e > 0 && frame % e == 0)
        {
            let actors = hp3_world::world(&mut vm).actors.clone();
            let player_pawn = hp3_world::world(&mut vm).player.and_then(|pc| hp3_world::get(&mut vm, pc, "Pawn").as_obj());
            for a in actors {
                let path = hp3_script::natives::object_path(&mut vm, a);
                if names.split(',').any(|n| (n == "!player" && Some(a) == player_pawn) || path.ends_with(&format!(".{n}")) || n.strip_prefix('~').is_some_and(|w| path.contains(w))) {
                    let state = vm.state_of(a).map(|s| vm.db.names.get(vm.db.get(s).name).to_string()).unwrap_or_default();
                    let at = hp3_world::physics::vec(&hp3_world::get(&mut vm, a, "Location"));
                    let rot: Vec<i32> = hp3_world::get(&mut vm, a, "Rotation").fields().iter().map(hp3_script::Value::as_int).collect();
                    if std::env::var_os("HP3_WATCH_DRAWN").is_some() {
                        // Where the actor is drawn: its location plus the view offset the stair smoothing keeps.
                        println!("drawn {frame} {path} z {:.2} offset {:.2} drawn_z {:.2}", at.z, hp3_world::physics::vec(&hp3_world::get(&mut vm, a, "SavedPrePivot")).z, at.z + hp3_world::physics::vec(&hp3_world::get(&mut vm, a, "SavedPrePivot")).z);
                    }
                    println!(
                        "watch {frame} {path} label {:?} coll {} at {at} rot {rot:?} state {state} phys {:?} hidden {:?} draw {:?} mesh {:?} vel {:?} accel {:?}",
                        { let t = hp3_world::get(&mut vm, a, "Tag").as_name(); format!("{:?} tag {}", hp3_world::get(&mut vm, a, "Label"), vm.db.names.get(t)) },
                        { let e = hp3_world::get(&mut vm, a, "Event").as_name(); format!("r{} h{} scale{} event {}", hp3_world::get(&mut vm, a, "CollisionRadius").as_float(), hp3_world::get(&mut vm, a, "CollisionHeight").as_float(), hp3_world::get(&mut vm, a, "DrawScale").as_float(), vm.db.names.get(e)) },
                        hp3_world::get(&mut vm, a, "Physics"),
                        hp3_world::get(&mut vm, a, "bHidden"),
                        hp3_world::get(&mut vm, a, "DrawType"),
                        hp3_world::get(&mut vm, a, "Mesh").as_obj().is_some(),
                        hp3_world::physics::vec(&hp3_world::get(&mut vm, a, "Velocity")),
                        hp3_world::physics::vec(&hp3_world::get(&mut vm, a, "Acceleration"))
                    );
                    // HP3_WATCH_PROPS=A,B: more properties of the watched actors.
                    if let Ok(props) = std::env::var("HP3_WATCH_PROPS") {
                        let mut extra = String::new();
                        for n in props.split(',') {
                            if n == "bounds" {
                                let b = hp3_world::world(&mut vm).collision.mover(a).map(|m| m.world_bounds());
                                extra.push_str(&format!(" bounds={b:?}"));
                                continue;
                            }
                            // `Name[i]` reads element i of a fixed array.
                            let (base, idx) = n.split_once('[').map_or((n, 0usize), |(b, i)| (b, i.trim_end_matches(']').parse().unwrap_or(0)));
                            let v = match hp3_world::slot(&mut vm, a, base) {
                                Some(sl) => vm.db.objects.values(a).and_then(|v| v.get(sl + idx).cloned()).unwrap_or_default(),
                                None => hp3_script::Value::Empty,
                            };
                            let v = match v {
                                hp3_script::Value::Name(x) => vm.db.names.get(x).to_string(),
                                hp3_script::Value::Object(Some(x)) => hp3_script::natives::object_path(&mut vm, x),
                                other => format!("{other:?}"),
                            };
                            extra.push_str(&format!(" {n}={v}"));
                        }
                        println!("watchprops {frame} {path}{extra}");
                    }
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
                        format!("ch{i} {n} a{:.2} f{:.1}/{} r{:.2} bone {:?}", c.alpha, c.frame, c.sequence.and_then(|s| set.as_ref().map(|set| set.sequences[s].frames)).unwrap_or(0), c.rate, c.bone)
                    })
                    .collect();
                let vel = hp3_world::physics::vec(&hp3_world::get(&mut vm, a, "Velocity"));
                println!("channels {frame} speed {:.0}: {}", vel.truncate().length(), line.join(" | "));
            }
        }
        trigger_list(&mut vm, frame);
        // HP3_SET=<actor name>.<bool property>=<true|false>@<tick>: sets a flag on an actor.
        if let Ok(spec) = std::env::var("HP3_SET")
            && let Some((what, tick)) = spec.split_once('@')
            && tick.parse::<u32>() == Ok(frame)
            && let Some((target, value)) = what.split_once('=')
            && let Some((name, prop)) = target.split_once('.')
        {
            let actors = hp3_world::world(&mut vm).actors.clone();
            for a in actors {
                if hp3_script::natives::object_path(&vm, a).ends_with(&format!(".{name}")) {
                    hp3_world::set(&mut vm, a, prop, hp3_script::Value::Bool(value == "true"));
                }
            }
        }
        // HP3_WALK=<actor name>:<x>,<y>,<z>><x>,<y>@<tick>: from that tick the actor is put at the first point and pushed towards the
        // second (flat), as a mover that wants to go there would, to see whether it gets there.
        if let Ok(spec) = std::env::var("HP3_WALK")
            && let Some((what, tick)) = spec.split_once('@')
            && let Ok(tick) = tick.parse::<u32>()
            && frame >= tick
            && let Some((name, route)) = what.split_once(':')
            && let Some((from, to)) = route.split_once('>')
        {
            let num = |s: &str| -> Vec<f32> { s.split(',').filter_map(|v| v.parse().ok()).collect() };
            let (from, to) = (num(from), num(to));
            let actors = hp3_world::world(&mut vm).actors.clone();
            for a in actors {
                if !hp3_script::natives::object_path(&vm, a).ends_with(&format!(".{name}")) {
                    continue;
                }
                if frame == tick && from.len() == 3 {
                    hp3_world::physics::far_move(&mut vm, a, glam::Vec3::new(from[0], from[1], from[2]), false, false);
                }
                if to.len() == 2 {
                    let at = hp3_world::physics::vec(&hp3_world::get(&mut vm, a, "Location"));
                    let dir = (glam::Vec2::new(to[0], to[1]) - at.truncate()).normalize_or_zero();
                    let accel = hp3_script::Value::Struct(vec![hp3_script::Value::Float(dir.x * 389.0), hp3_script::Value::Float(dir.y * 389.0), hp3_script::Value::Float(0.0)].into());
                    hp3_world::set(&mut vm, a, "Acceleration", accel);
                }
            }
        }
        // HP3_UNCOLLIDE=<actor name>[,<actor name>]@<tick>: those actors stop colliding (as SetCollision(false) does).
        if let Ok(spec) = std::env::var("HP3_UNCOLLIDE")
            && let Some((names, tick)) = spec.split_once('@')
            && tick.parse::<u32>() == Ok(frame)
        {
            let actors = hp3_world::world(&mut vm).actors.clone();
            for a in actors {
                let path = hp3_script::natives::object_path(&mut vm, a);
                if names.split(',').any(|n| path.ends_with(&format!(".{n}"))) {
                    let bounds = hp3_world::world(&mut vm).collision.baked_bounds(a);
                    hp3_world::set(&mut vm, a, "bCollideActors", hp3_script::Value::Bool(false));
                    hp3_world::refresh_baked_collision(&mut vm, a);
                    println!("uncollide {path} baked bounds {bounds:?}");
                }
            }
        }
        // HP3_FORCE_YAW=<yaw>@<tick>: from that tick on the player's controller and pawn are turned to that yaw every tick, to walk a way.
        if let Ok(spec) = std::env::var("HP3_FORCE_YAW")
            && let Some((yaw, at)) = spec.split_once('@')
            && let (Ok(yaw), Ok(at)) = (yaw.parse::<i32>(), at.parse::<u32>())
            && frame >= at
            && let Some(pc) = hp3_world::world(&mut vm).player
        {
            let r = hp3_script::Value::Struct(vec![hp3_script::Value::Int(0), hp3_script::Value::Int(yaw), hp3_script::Value::Int(0)].into());
            hp3_world::set(&mut vm, pc, "Rotation", r.clone());
            if let Some(pawn) = hp3_world::get(&mut vm, pc, "Pawn").as_obj() {
                hp3_world::set(&mut vm, pawn, "Rotation", r.clone());
                hp3_world::set(&mut vm, pawn, "DesiredRotation", r);
            }
        }
        // HP3_TELEPORT=<x>,<y>,<z>@<tick>[;<x>,<y>,<z>@<tick>...]: moves the player's pawn there (as SetLocation does), to walk into a trigger.
        for spec in std::env::var("HP3_TELEPORT").unwrap_or_default().split(';') {
            let Some((at, tick)) = spec.split_once('@') else { continue };
            if tick.parse::<u32>() != Ok(frame) {
                continue;
            }
            let c: Vec<f32> = at.split(',').filter_map(|v| v.parse().ok()).collect();
            // HP3_TELEPORT_ACTOR=<name>: the pawn to move when the player has none (a cutscene holds it).
            let held = std::env::var("HP3_TELEPORT_ACTOR").ok().and_then(|n| hp3_world::world(&mut vm).actors.clone().into_iter().find(|&a| hp3_script::natives::object_path(&vm, a).ends_with(&format!(".{n}"))));
            let player = hp3_world::world(&mut vm).player;
            let pawn = player.and_then(|pc| hp3_world::get(&mut vm, pc, "Pawn").as_obj()).or(held);
            if let (&[x, y, z], Some(pc), Some(pawn)) = (c.as_slice(), player, pawn)
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
        // HP3_PLAYER_TO=<actor name>@<tick>: puts the player's pawn at that actor (a slide volume's or trigger's middle).
        if let Ok(spec) = std::env::var("HP3_PLAYER_TO")
            && let Some((name, tick)) = spec.rsplit_once('@')
            && tick.parse::<u32>() == Ok(frame)
        {
            let pawn = hp3_world::world(&mut vm).player.and_then(|pc| hp3_world::get(&mut vm, pc, "Pawn").as_obj());
            let actors = hp3_world::world(&mut vm).actors.clone();
            let target = actors.into_iter().find(|&a| hp3_script::natives::object_path(&vm, a).ends_with(&format!(".{name}")));
            if let (Some(pawn), Some(t)) = (pawn, target) {
                let at = hp3_world::physics::vec(&hp3_world::get(&mut vm, t, "Location"));
                println!("player to {name} at {at}");
                hp3_world::physics::far_move(&mut vm, pawn, at + glam::Vec3::Z * 40.0, false, false);
            }
        }
        // HP3_SWITCH=<0|1|2|next>@<tick>[,...]: switches the played character (Harry, Ron, Hermione, or the next one), as the
        // app's 1/2/3/C keys do.
        for spec in std::env::var("HP3_SWITCH").unwrap_or_default().split(',') {
            if let Some((which, tick)) = spec.split_once('@')
                && tick.parse::<u32>() == Ok(frame)
            {
                let w = which.parse::<usize>().ok();
                println!("switch {which} at {frame}: {}", hp3_world::character_switch::switch(&mut vm, w));
            }
        }
        // HP3_TRAVEL=<url>@<tick>: asks the level to switch to <url> with the player's items, as a ServerTravel does.
        if let Ok(spec) = std::env::var("HP3_TRAVEL")
            && let Some((url, tick)) = spec.rsplit_once('@')
            && tick.parse::<u32>() == Ok(frame)
            && let Some(li) = hp3_world::world(&mut vm).level_info
        {
            // ServerTravel(URL, true) has every actor save what it keeps across levels first.
            for a in hp3_world::world(&mut vm).actors.clone() {
                hp3_world::fire(&mut vm, a, "SavePersistentData", &mut []);
            }
            hp3_world::set(&mut vm, li, "bNextItems", hp3_script::Value::Bool(true));
            hp3_world::set(&mut vm, li, "NextURL", hp3_script::Value::Str(url.to_string()));
            hp3_world::set(&mut vm, li, "NextSwitchCountdown", hp3_script::Value::Float(0.0));
        }
        // HP3_GIVE_BEANS=<n>@<tick>: the player's controller is given n beans; the bean count is printed after.
        if let Ok(spec) = std::env::var("HP3_GIVE_BEANS")
            && let Some((n, tick)) = spec.split_once('@')
            && let (Ok(n), Ok(tick)) = (n.parse::<i32>(), tick.parse::<u32>())
            && (frame == tick || frame == tick + 5 || ((frame > tick || std::env::var_os("HP3_BEANS_WATCH").is_some()) && frame % 100 == 0))
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
        // HP3_FIRE=<actor name>.<Event>@<tick>: raises a script event with no arguments on the first actor with that name (e.g. a vendor item's
        // `Purchased`), and prints what the level's inventory carrier is and how many cards the manager says are owned before and after.
        let fire_specs = std::env::var("HP3_FIRE").unwrap_or_default();
        for spec in fire_specs.split(';') {
            let Some((what, tick)) = spec.rsplit_once('@') else { continue };
            if tick.trim().parse::<u32>() != Ok(frame) {
                continue;
            }
            let Some((name, event)) = what.split_once('.') else { continue };
            let actors = hp3_world::world(&mut vm).actors.clone();
            let carrier = hp3_world::world(&mut vm).level_info.and_then(|li| hp3_world::get(&mut vm, li, "InventoryCarrierPawn").as_obj());
            let cname = carrier.map(|c| hp3_script::natives::object_path(&vm, c));
            let cclass = carrier.and_then(|c| vm.db.objects.get(c).class).map(|c| vm.db.names.get(vm.db.get(c).name).to_string());
            println!("fire: carrier {cname:?} class {cclass:?}");
            let mgr = actors.iter().copied().find(|&a| hp3_script::natives::object_path(&vm, a).contains("WizardCardManager"));
            let count = |vm: &mut hp3_script::Vm<'_>| mgr.map(|m| hp3_world::fire(vm, m, "GetFoundCardCount", &mut []).as_int());
            let before = count(&mut vm);
            if let Some(a) = actors.iter().copied().find(|&a| hp3_script::natives::object_path(&vm, a).rsplit('.').next() == Some(name)) {
                println!("fire: {event} on {}", hp3_script::natives::object_path(&vm, a));
                if let Some(state) = event.strip_prefix("goto:") {
                    let n = vm.db.names.intern(state);
                    let _ = vm.goto_state(a, n, hp3_script::Name::NONE);
                } else {
                    hp3_world::fire(&mut vm, a, event, &mut []);
                }
            } else {
                println!("fire: no actor {name}");
            }
            println!("fire: cards owned {before:?} -> {:?}", count(&mut vm));
        }
        // HP3_THROW=<x>,<y>,<z>,<vx>,<vy>,<vz>@<tick>: a wizard card is made there in its thrown state (as a student's sale does) and its place is
        // printed every 10 ticks for the next 200: whether it lands, bounces or falls through.
        if let Ok(spec) = std::env::var("HP3_THROW")
            && let Some((what, tick)) = spec.rsplit_once('@')
            && let Ok(tick) = tick.trim().parse::<u32>()
        {
            let v: Vec<f32> = what.split(',').filter_map(|s| s.trim().parse().ok()).collect();
            if frame == tick && v.len() == 6 {
                if let Some(c) = hp3_world::find_object(&mut vm, &std::env::var("HP3_THROW_CLASS").unwrap_or_else(|_| "HGame.WizardCard".into())) {
                    let li = hp3_world::world(&mut vm).level_info;
                    if let Ok(card) = hp3_world::natives::spawn(&mut vm, li, &[hp3_script::Value::Object(Some(c))]) && let Some(card) = card.as_obj() {
                        let mk = |a: f32, b: f32, c: f32| hp3_script::Value::Struct(vec![hp3_script::Value::Float(a), hp3_script::Value::Float(b), hp3_script::Value::Float(c)].into());
                        hp3_world::fire(&mut vm, card, "SetLocation", &mut [mk(v[0], v[1], v[2])]);
                        let n = vm.db.names.intern(&std::env::var("HP3_THROW_STATE").unwrap_or_else(|_| "ThrownPickup".into()));
                        let _ = vm.goto_state(card, n, hp3_script::Name::NONE);
                        if std::env::var_os("HP3_THROW_COLLIDE").is_some() { hp3_world::set(&mut vm, card, "bCollideWorld", hp3_script::Value::Bool(true)); }
                        if std::env::var_os("HP3_THROW_FALL").is_some() { hp3_world::set(&mut vm, card, "Physics", hp3_script::Value::Byte(2)); }
                        hp3_world::set(&mut vm, card, "Velocity", mk(v[3], v[4], v[5]));
                        THROWN.with(|t| t.set(Some((card, frame))));
                    }
                }
            }
        }
        if let Some((card, from)) = THROWN.with(|t| t.get())
            && frame >= from
            && frame <= from + 200
            && (frame - from) % 10 == 0
        {
            let deleted = vm.db.objects.get(card).deleted;
            let at = hp3_world::physics::vec(&hp3_world::get(&mut vm, card, "Location"));
            let st = vm.state_of(card).map(|s| vm.db.names.get(vm.db.get(s).name).to_string());
            println!("throw +{}: card at {at} state {st:?} physics {} hidden {} deleted {deleted}", frame - from, hp3_world::get(&mut vm, card, "Physics").as_byte(), hp3_world::get(&mut vm, card, "bHidden").as_bool());
        }
        // HP3_PRESS=<key>@<tick>[,<key>@<tick>...]: taps a key (down at that tick, up five ticks later), e.g. Enter to end a creature's control.
        if let Ok(spec) = std::env::var("HP3_PRESS") {
            for item in spec.split(',') {
                if let Some((key, tick)) = item.rsplit_once('@')
                    && let Ok(tick) = tick.trim().parse::<u32>()
                {
                    if frame == tick {
                        hp3_world::world(&mut vm).input.key_down(key.trim());
                    }
                    if frame == tick + 5 {
                        hp3_world::world(&mut vm).input.key_up(key.trim());
                    }
                }
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
        // HP3_STATE_HASH=<every>: a hash of every live actor's place, to compare two runs.
        if let Some(every) = std::env::var("HP3_STATE_HASH").ok().and_then(|v| v.parse::<u32>().ok())
            && frame % every == 0
        {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            for a in hp3_world::world(&mut vm).actors.clone() {
                if vm.db.objects.get(a).deleted {
                    continue;
                }
                let at = hp3_world::physics::vec(&hp3_world::get(&mut vm, a, "Location"));
                (a.0, at.x.to_bits(), at.y.to_bits(), at.z.to_bits()).hash(&mut h);
            }
            println!("statehash {frame}: {:016x}", h.finish());
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
        // HP3_STATUS_LOG=<every>: whether the player has a pawn, what state the controller is in and whether a menu holds the input.
        if let Some(every) = std::env::var("HP3_STATUS_LOG").ok().and_then(|v| v.parse::<u32>().ok())
            && frame % every == 0
        {
            let pc = hp3_world::world(&mut vm).player;
            let pawn = pc.and_then(|p| hp3_world::get(&mut vm, p, "Pawn").as_obj());
            let state = pc.and_then(|p| vm.state_of(p)).map(|s| vm.db.names.get(vm.db.get(s).name).to_string());
            let menu = hp3_world::gui::menu_open(&mut vm);
            let cutscene = pc.map(|p| hp3_world::get(&mut vm, p, "bInCutScene").as_bool());
            let ignore = pc.map(|p| (hp3_world::get(&mut vm, p, "bIgnoreMoveInput").as_bool(), hp3_world::get(&mut vm, p, "bIgnoreLookInput").as_bool()));
            let own = pawn.map(|p| hp3_world::get(&mut vm, p, "Controller").as_obj() == pc);
            let who = pawn.map(|p| { let l = hp3_world::physics::vec(&hp3_world::get(&mut vm, p, "Location")); let off = hp3_world::physics::view_offset(&mut vm, p); format!("{} at ({:.0},{:.0},{:.0}) drawn z {:.1}", hp3_script::natives::object_path(&vm, p), l.x, l.y, l.z, l.z + off) });
            println!("status {frame}: {who:?} controlled by the player {own:?}; pawn {} controller state {state:?} menu {menu} bInCutScene {cutscene:?} ignore(move,look) {ignore:?}", pawn.is_some());
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
        if std::env::var("HP3_SAVELOAD").ok().and_then(|v| v.parse::<u32>().ok()) == Some(frame) {
            saveload::round_trip(&mut vm, map);
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
    // HP3_PARTICLE_COLORS=<texture substring>: the colours (red, green, blue, alpha) of the live particle quads drawn with a matching texture:
    // the brightest, as the renderer gets them (before the texture and blending).
    // HP3_PROBE_ACTOR=<actor name>[,<extent half width>,<half height>]: box traces of a player-sized box (a player's mover: blocked by world and
    // by `bBlockPlayers` actors) towards that actor's middle from eight sides, from 300 away at its height: how close the box gets and what stops it.
    if let Ok(spec) = std::env::var("HP3_PROBE_ACTOR") {
        let mut it = spec.split(',');
        let name = it.next().unwrap_or("").to_string();
        let ext = glam::Vec3::new(it.next().and_then(|v| v.parse().ok()).unwrap_or(15.0), it.next().and_then(|v| v.parse().ok()).unwrap_or(15.0), it.next().and_then(|v| v.parse().ok()).unwrap_or(46.0));
        let ext = glam::Vec3::new(ext.x, ext.x, ext.y.max(ext.z));
        let actors = hp3_world::world(&mut vm).actors.clone();
        if let Some(a) = actors.into_iter().find(|&a| hp3_script::natives::object_path(&vm, a).ends_with(&format!(".{name}"))) {
            let at = hp3_world::physics::vec(&hp3_world::get(&mut vm, a, "Location"));
            // HP3_PROBE_ITEM: a thrown pickup instead (blocked by the world only, not a player).
            let item = std::env::var_os("HP3_PROBE_ITEM").is_some();
            let movers = std::env::var("HP3_PROBE_ITEM").is_ok_and(|v| v == "movers");
            let how = hp3_world::collision::Mover { player: !item, blocks_actors: !item || movers, collide_world: true };
            for k in 0..8 {
                let ang = k as f32 * std::f32::consts::FRAC_PI_4;
                let dir = glam::Vec3::new(ang.cos(), ang.sin(), 0.0);
                let (start, end) = if item { (at, at + dir * 300.0) } else { (at + dir * 300.0, at) };
                let hit = hp3_world::world(&mut vm).collision.trace_for(how, start, end, ext, None);
                let hit_name = hit.and_then(|h| h.actor).map(|o| hp3_script::natives::object_path(&vm, o));
                let stop = hit.map(|h| (start - end).length() * h.time);
                let shown = stop.map(|t| if item { t.round() } else { (300.0 - t).round() });
                println!("probe {name} from {k}: stopped {shown:?} from the middle: hit {hit_name:?}");
            }
        } else {
            println!("probe: no actor {name}");
        }
    }
    // HP3_PROBE_DOWN=<actor name>,<half width>,<half height>: player-sized boxes dropped 200 above the actor's middle onto it on a grid of
    // offsets (-120..120): where the box comes to rest, to see whether a platform holds a pawn everywhere.
    if let Ok(spec) = std::env::var("HP3_PROBE_DOWN") {
        let mut it = spec.split(',');
        let name = it.next().unwrap_or("").to_string();
        let w: f32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(15.0);
        let hh: f32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(46.0);
        let actors = hp3_world::world(&mut vm).actors.clone();
        if let Some(a) = actors.into_iter().find(|&a| hp3_script::natives::object_path(&vm, a).ends_with(&format!(".{name}"))) {
            let mut at = hp3_world::physics::vec(&hp3_world::get(&mut vm, a, "Location"));
            // HP3_PROBE_AT=x,y,z puts the drop at a place instead of at the actor.
            if let Ok(p) = std::env::var("HP3_PROBE_AT") {
                let v: Vec<f32> = p.split(',').filter_map(|s| s.parse().ok()).collect();
                if v.len() == 3 {
                    at = glam::Vec3::new(v[0], v[1], v[2]);
                }
            }
            let how = hp3_world::collision::Mover { player: true, blocks_actors: true, collide_world: true };
            let from_h: f32 = std::env::var("HP3_PROBE_DOWN_FROM").ok().and_then(|v| v.parse().ok()).unwrap_or(500.0);
            for dy in (-120..=120).step_by(40) {
                let mut row = String::new();
                for dx in (-120..=120).step_by(40) {
                    let p = at + glam::Vec3::new(dx as f32, dy as f32, 0.0);
                    let hit = hp3_world::world(&mut vm).collision.trace_for(how, p + glam::Vec3::Z * from_h, p - glam::Vec3::Z * 300.0, glam::Vec3::new(w, w, hh), None);
                    row.push_str(&match hit {
                        Some(h) => format!(" {:>6.0}", p.z + from_h - (from_h + 300.0) * h.time),
                        None => "   none".into(),
                    });
                }
                println!("down {name} dy {dy:>4}:{row}");
            }
            let h = hp3_world::world(&mut vm).collision.trace_for(how, at + glam::Vec3::Z * 500.0, at - glam::Vec3::Z * 300.0, glam::Vec3::new(w, w, hh), None);
            let what = h.and_then(|h| h.actor).map(|o| hp3_script::natives::object_path(&vm, o));
            println!("down {name} middle hit: {what:?} {:?}", h.map(|h| h.normal));
            println!("down {name} at z {}", at.z);
        }
    }
    // HP3_SLICE=x0,y0,x1,y1,step,z,half width,half height: a map of where a box centred there fits ('.') or is blocked ('#', by
    // geometry with no actor 'W' for the level's BSP, 'm' for a mesh), seen from above at that height.
    if let Ok(spec) = std::env::var("HP3_SLICE") {
        let v: Vec<f32> = spec.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        if v.len() == 8 {
            let ext = glam::Vec3::new(v[6], v[6], v[7]);
            let mut y = v[3];
            while y >= v[1] {
                let mut line = String::new();
                let mut x = v[0];
                while x <= v[2] {
                    let p = glam::Vec3::new(x, y, v[5]);
                    let h = hp3_world::world(&mut vm).collision.trace_static(p, p + glam::Vec3::X * 0.01, ext);
                    line.push(match h {
                        Some(h) if h.time <= 0.0 => if h.actor.is_some() { 'm' } else { 'W' },
                        _ => '.',
                    });
                    x += v[4];
                }
                println!("y {y:8.0} {line}");
                y -= v[4];
            }
        }
    }
    // HP3_WALK=x,y,z: the BSP nodes the point walks through.
    if let Ok(spec) = std::env::var("HP3_WALK") {
        let v: Vec<f32> = spec.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        if v.len() == 3 {
            for line in hp3_world::world(&mut vm).collision.debug_walk(glam::Vec3::new(v[0], v[1], v[2])) {
                println!("{line}");
            }
        }
    }
    // HP3_PROBE_WALK=x,y,z,half width,half height,dx,dy,max step: a walker that goes 3 units a time along (dx,dy), stepping up
    // when blocked and settling down onto the floor, to where it stops and what holds it.
    if let Ok(spec) = std::env::var("HP3_PROBE_WALK") {
        let n: Vec<f32> = spec.split(',').filter_map(|s| s.parse().ok()).collect();
        if n.len() == 8 {
            let how = hp3_world::collision::Mover { player: true, blocks_actors: true, collide_world: true };
            let ext = glam::Vec3::new(n[3], n[3], n[4]);
            let dir = glam::Vec3::new(n[5], n[6], 0.0).normalize() * 3.0;
            let mut p = glam::Vec3::new(n[0], n[1], n[2]);
            for i in 0..400 {
                let mut moved = false;
                for lift in [0.0, 1.0, 2.5, 5.0, 10.0, n[7]] {
                    let from = p + glam::Vec3::Z * lift;
                    let up = hp3_world::world(&mut vm).collision.trace_for(how, p, from, ext, None);
                    if up.is_some_and(|h| h.time < 1.0) { continue; }
                    let h = hp3_world::world(&mut vm).collision.trace_for(how, from, from + dir, ext, None);
                    if h.is_none() {
                        let to = from + dir;
                        let d = hp3_world::world(&mut vm).collision.trace_for(how, to, to - glam::Vec3::Z * (lift + 6.0), ext, None);
                        p = match d { Some(d) => to - glam::Vec3::Z * ((lift + 6.0) * d.time), None => to - glam::Vec3::Z * (lift + 6.0) };
                        moved = true;
                        break;
                    }
                }
                if !moved {
                    let h = hp3_world::world(&mut vm).collision.trace_for(how, p, p + dir, ext, None);
                    println!("walk stopped after {i} steps at {p:?}; blocked by {:?}", h.map(|h| (h.time, h.normal, h.actor.map(|o| hp3_script::natives::object_path(&vm, o)))));
                    break;
                }
                if i % 10 == 0 { println!("walk {i}: {p:?}"); }
            }
        }
    }
    // HP3_PROBE_PROFILE=x,y0,y1,step,half width,half height: along a line in y, the highest box position that fits (floor)
    // within 80 above/below -55 and the headroom above it.
    if let Ok(spec) = std::env::var("HP3_PROBE_PROFILE") {
        let n: Vec<f32> = spec.split(',').filter_map(|s| s.parse().ok()).collect();
        if n.len() == 6 {
            let how = hp3_world::collision::Mover { player: true, blocks_actors: true, collide_world: true };
            let ext = glam::Vec3::new(n[4], n[4], n[5]);
            let mut y = n[1];
            while y <= n[2] {
                let top = glam::Vec3::new(n[0], y, -35.0);
                let d = hp3_world::world(&mut vm).collision.trace_for(how, top, top - glam::Vec3::Z * 200.0, ext, None);
                match d {
                    Some(d) => {
                        let z = -35.0 - 200.0 * d.time;
                        let at = glam::Vec3::new(n[0], y, z);
                        let u = hp3_world::world(&mut vm).collision.trace_for(how, at, at + glam::Vec3::Z * 120.0, ext, None);
                        println!("profile y {y}: floor centre z {z:.1} floor normal z {:.2} headroom {:?}", d.normal.z, u.map(|h| h.time * 120.0));
                    }
                    None => println!("profile y {y}: nothing"),
                }
                y += n[3];
            }
        }
    }
    // HP3_PROBE_EXACT=x,y,z,half width,half height,dx,dy,dz: how far along that direction (up to 80) a box first overlaps a
    // static-mesh triangle exactly, against how far the swept trace says it gets.
    if let Ok(spec) = std::env::var("HP3_PROBE_EXACT") {
        let n: Vec<f32> = spec.split(',').filter_map(|s| s.parse().ok()).collect();
        if n.len() == 8 {
            let at = glam::Vec3::new(n[0], n[1], n[2]);
            let half = glam::Vec3::new(n[3], n[3], n[4]);
            let dir = glam::Vec3::new(n[5], n[6], n[7]).normalize();
            let mut exact = None;
            let mut d = 0.0f32;
            while d <= 80.0 {
                if hp3_world::world(&mut vm).collision.box_overlaps_triangles(at + dir * d, half, None) { exact = Some(d); break; }
                d += 0.25;
            }
            let how = hp3_world::collision::Mover { player: true, blocks_actors: true, collide_world: true };
            let tr = hp3_world::world(&mut vm).collision.trace_for(how, at, at + dir * 80.0, half, None);
            println!("exact: first overlap at {exact:?}; trace says {:?} normal {:?}", tr.map(|h| h.time * 80.0), tr.map(|h| h.normal));
        }
    }
    // HP3_PROBE_RING=x,y,z,half width,half height: a box swept 80 out in 16 directions from there, at that height and at
    // 10, 20 and 30 above it: how far each gets and what stops it.
    if let Ok(spec) = std::env::var("HP3_PROBE_RING") {
        let n: Vec<f32> = spec.split(',').filter_map(|s| s.parse().ok()).collect();
        if n.len() == 5 {
            let how = hp3_world::collision::Mover { player: true, blocks_actors: true, collide_world: true };
            let at = glam::Vec3::new(n[0], n[1], n[2]);
            let ext = glam::Vec3::new(n[3], n[3], n[4]);
            for up in [0.0f32, 10.0, 20.0, 30.0] {
                let start = at + glam::Vec3::Z * up;
                let ov = hp3_world::world(&mut vm).collision.trace_for(how, start, start, ext, None);
                println!("ring up {up}: in place {:?}", ov.map(|h| (h.time, h.normal, h.actor.map(|o| hp3_script::natives::object_path(&vm, o)))));
                let mut row = String::new();
                for k in 0..16 {
                    let a = k as f32 * std::f32::consts::TAU / 16.0;
                    let end = start + glam::Vec3::new(a.cos(), a.sin(), 0.0) * 80.0;
                    let h = hp3_world::world(&mut vm).collision.trace_for(how, start, end, ext, None);
                    if let Some(h) = h && up == 0.0 {
                        println!("  dir {k}: {:.1} normal {:?} actor {:?}", h.time * 80.0, h.normal, h.actor.map(|o| hp3_script::natives::object_path(&vm, o)));
                    }
                    row.push_str(&match h {
                        Some(h) => format!(" {:>3.0}", h.time * 80.0),
                        None => "  80".into(),
                    });
                }
                println!("ring up {up}:{row}");
                let hu = hp3_world::world(&mut vm).collision.trace_for(how, start, start + glam::Vec3::Z * 120.0, ext, None);
                println!("ring up {up}: straight up reaches {:?}", hu.map(|h| h.time * 120.0));
            }
        }
    }
    // HP3_PROBE_OVERLAP=<mover>,<x>,<y>,<z>,<half width>,<half height>: does the mover overlap a box there, as it is raised from 400 below its place to 400 above?
    if let Ok(spec) = std::env::var("HP3_PROBE_OVERLAP") {
        let p: Vec<&str> = spec.split(',').collect();
        let n: Vec<f32> = p.iter().skip(1).filter_map(|s| s.parse().ok()).collect();
        let actors = hp3_world::world(&mut vm).actors.clone();
        if let (Some(name), 5) = (p.first(), n.len()) && let Some(a) = actors.into_iter().find(|&a| hp3_script::natives::object_path(&vm, a).ends_with(&format!(".{name}"))) {
            let home = hp3_world::physics::vec(&hp3_world::get(&mut vm, a, "Location"));
            let (at, ext) = (glam::Vec3::new(n[0], n[1], n[2]), glam::Vec3::new(n[3], n[3], n[4]));
            let mut row = String::new();
            for dz in (-400..=400).step_by(20) {
                let loc = home + glam::Vec3::Z * dz as f32;
                hp3_world::set(&mut vm, a, "Location", hp3_script::Value::Struct(vec![hp3_script::Value::Float(loc.x), hp3_script::Value::Float(loc.y), hp3_script::Value::Float(loc.z)].into()));
                hp3_world::physics::place_mover_shape(&mut vm, a);
                let hit = hp3_world::world(&mut vm).collision.mover(a).is_some_and(|m| m.overlaps(at, ext));
                row.push_str(&format!(" {dz}:{}", if hit { "X" } else { "." }));
            }
            println!("overlap {name}{row}");
        }
    }
    if let Ok(filter) = std::env::var("HP3_PARTICLE_COLORS") {
        let quads = hp3_world::particles::collect(&mut vm, glam::Vec3::X, glam::Vec3::Z);
        let mut by: std::collections::BTreeMap<String, (usize, [f32; 4], hp3_world::particles::Blend)> = std::collections::BTreeMap::new();
        for q in &quads {
            let Some(m) = q.material else { continue };
            let name = hp3_script::natives::object_path(&vm, m);
            if !name.to_ascii_lowercase().contains(&filter.to_ascii_lowercase()) {
                continue;
            }
            let e = by.entry(name).or_insert((0, [0.0; 4], q.blend));
            e.0 += 1;
            if q.color[0] + q.color[1] + q.color[2] > e.1[0] + e.1[1] + e.1[2] {
                e.1 = q.color;
            }
        }
        for (name, (n, c, blend)) in by {
            println!("  quads {name}: {n} quads, brightest colour {:.2} {:.2} {:.2} alpha {:.2}, blend {blend:?}", c[0], c[1], c[2], c[3]);
        }
    }
    // HP3_PARTICLE_NEAR=x,y,z,radius: the live particle quads whose middle is within that radius of the point, per texture: how many, their
    // average colour and alpha, the blend and the average width (what is drawn at one spot, such as a pickup's glow).
    if let Ok(spec) = std::env::var("HP3_PARTICLE_NEAR") {
        let v: Vec<f32> = spec.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        if v.len() == 4 {
            let at = glam::Vec3::new(v[0], v[1], v[2]);
            let quads = hp3_world::particles::collect(&mut vm, glam::Vec3::X, glam::Vec3::Z);
            type Sum = (usize, [f32; 4], f32, hp3_world::particles::Blend);
            let mut by: std::collections::BTreeMap<String, Sum> = std::collections::BTreeMap::new();
            for q in &quads {
                let mid = (q.corners[0] + q.corners[1] + q.corners[2] + q.corners[3]) * 0.25;
                if (mid - at).length() > v[3] {
                    continue;
                }
                let name = q.material.map(|m| hp3_script::natives::object_path(&vm, m)).unwrap_or_else(|| "(none)".into());
                let e = by.entry(name).or_insert((0, [0.0; 4], 0.0, q.blend));
                e.0 += 1;
                for k in 0..4 {
                    e.1[k] += q.color[k];
                }
                e.2 += (q.corners[1] - q.corners[0]).length();
            }
            for (name, (n, c, w, blend)) in by {
                let n_f = n as f32;
                println!("  near {name}: {n} quads, mean colour {:.2} {:.2} {:.2} alpha {:.2}, width {:.0}, blend {blend:?}", c[0] / n_f, c[1] / n_f, c[2] / n_f, c[3] / n_f, w / n_f);
            }
        }
    }
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
    // HP3_LIST_LIGHTS=1: the lit actors of the world when the run ends, and which of them are dynamic lights.
    if std::env::var_os("HP3_LIST_LIGHTS").is_some() {
        let mut counts: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
        for a in hp3_world::world(&mut vm).actors.clone() {
            if vm.db.objects.get(a).deleted || hp3_world::get(&mut vm, a, "LightType").as_byte() == 0 {
                continue;
            }
            let class = vm.db.objects.get(a).class.map(|c| vm.db.names.get(vm.db.get(c).name).to_string()).unwrap_or_default();
            let e = counts.entry(class).or_default();
            e.0 += 1;
            e.1 += hp3_world::get(&mut vm, a, "bDynamicLight").as_bool() as usize;
        }
        for (class, (n, dynamic)) in counts {
            println!("lit actors: {class}: {n} ({dynamic} dynamic)");
        }
    }
    if let Some(pc) = hp3_world::world(&mut vm).player {
        let class = vm.db.objects.get(pc).class.map(|c| vm.db.names.get(vm.db.get(c).name).to_string());
        let pawn = hp3_world::get(&mut vm, pc, "Pawn").as_obj();
        let state = vm.state_of(pc).map(|s| vm.db.names.get(vm.db.get(s).name).to_string());
        println!("player controller {class:?} in state {state:?}");
        // HP3_OPEN_PAGE=Package.Class opens that menu page; HP3_MENU_KEYS=27,... then presses those keys (27 = Escape) on it
        // and says whether the menu is still up (the trade page must close on Escape).
        if let Ok(page) = std::env::var("HP3_OPEN_PAGE") {
            let opened = if page == "ShowMenu" {
                println!("  strGameMenu={:?} save={:?}", hp3_world::get(&mut vm, pc, "strGameMenu"), hp3_world::get(&mut vm, pc, "strGameMenuSave"));
                hp3_world::fire(&mut vm, pc, "ShowMenu", &mut []);
                true
            } else {
                hp3_world::interactions::open_menu(&mut vm, &page)
            };
            for _ in 0..10 {
                hp3_world::tick(&mut vm, 1.0 / 60.0);
            }
            let tiles = hp3_world::canvas::render(&mut vm, 1280, 720);
            if std::env::var_os("HP3_TILES").is_some() {
                for t in &tiles {
                    let m = t.material.map(|m| hp3_script::natives::object_path(&vm, m)).unwrap_or_default();
                    println!("  tile {m} at ({:.0},{:.0}) {:.0}x{:.0}", t.x, t.y, t.w, t.h);
                }
            }
            println!("  opened {page}: {opened}, menu up {}", hp3_world::gui::menu_open(&mut vm));
            if let Ok(click) = std::env::var("HP3_CLICK") {
                let v: Vec<f32> = click.split(',').filter_map(|s| s.parse().ok()).collect();
                if v.len() == 2 {
                    hp3_world::interactions::open_menu(&mut vm, "HGame.HPPageExit");
                    for _ in 0..5 { hp3_world::tick(&mut vm, 1.0 / 60.0); }
                    let _ = hp3_world::canvas::render(&mut vm, 1280, 720);
                    if let Some(top) = hp3_world::gui::top_menu(&mut vm) {
                        for n in ["btnYes", "btnNo", "lblMsg"] {
                            let o = hp3_world::get(&mut vm, top, n).as_obj();
                            let name = o.map(|o| vm.db.names.get(vm.db.objects.get(o).name).to_string());
                            println!("  {n} = {name:?}");
                        }
                    }
                    for l in hp3_world::gui::dump(&mut vm).iter().filter(|l| !l.starts_with(' ')) {
                        println!("  before click: {l}");
                    }
                    if std::env::var_os("HP3_TRACE_CALLS").is_none() { vm.trace_calls = Some("page".into()); }
                    hp3_world::gui::mouse_move(&mut vm, v[0], v[1]);
                    hp3_world::gui::mouse_button(&mut vm, true);
                    hp3_world::gui::mouse_button(&mut vm, false);
                    for _ in 0..10 { hp3_world::tick(&mut vm, 1.0 / 60.0); }
                    for l in hp3_world::gui::dump(&mut vm).iter().filter(|l| !l.starts_with(' ')) {
                        println!("  after click: {l}");
                    }
                }
            }
            // HP3_CLICKS=x,y;x,y... moves to each point and clicks it (drawing between, so the controls have their boxes);
            // HP3_DRAG=x,y>x,y drags from one point to the other. The settings the menu changed are printed after each step.
            if let Ok(clicks) = std::env::var("HP3_CLICKS") {
                for step in clicks.split(';') {
                    let v: Vec<f32> = step.split(',').filter_map(|s| s.trim().parse().ok()).collect();
                    if v.len() != 2 { continue; }
                    let _ = hp3_world::canvas::render(&mut vm, 1280, 720);
                    hp3_world::gui::mouse_move(&mut vm, v[0], v[1]);
                    hp3_world::gui::mouse_button(&mut vm, true);
                    hp3_world::gui::mouse_button(&mut vm, false);
                    for _ in 0..10 { hp3_world::tick(&mut vm, 1.0 / 60.0); }
                    let tiles = hp3_world::canvas::render(&mut vm, 1280, 720);
                    println!("  click {step}: {} tiles, settings v{}", tiles.len(), hp3_world::world(&mut vm).settings_version);
                    if std::env::var_os("HP3_TILES").is_some() {
                        for t in tiles.iter().filter(|t| t.material.is_some() && t.y > 230.0 && t.y < 700.0) {
                            let m = t.material.map(|m| hp3_script::natives::object_path(&vm, m)).unwrap_or_default();
                            println!("    tile {m} at ({:.0},{:.0}) {:.0}x{:.0}", t.x, t.y, t.w, t.h);
                        }
                    }
                }
            }
            if let Ok(drag) = std::env::var("HP3_DRAG") {
                let pts: Vec<Vec<f32>> = drag.split('>').map(|p| p.split(',').filter_map(|s| s.trim().parse().ok()).collect()).collect();
                if pts.len() == 2 && pts.iter().all(|p| p.len() == 2) {
                    let _ = hp3_world::canvas::render(&mut vm, 1280, 720);
                    hp3_world::gui::mouse_move(&mut vm, pts[0][0], pts[0][1]);
                    hp3_world::gui::mouse_button(&mut vm, true);
                    hp3_world::gui::mouse_move(&mut vm, pts[1][0], pts[1][1]);
                    hp3_world::gui::mouse_button(&mut vm, false);
                    for _ in 0..5 { hp3_world::tick(&mut vm, 1.0 / 60.0); }
                    println!("  drag {drag}: settings v{}", hp3_world::world(&mut vm).settings_version);
                }
            }
            if std::env::var_os("HP3_DUMP_GUI").is_some() {
                for l in hp3_world::gui::dump(&mut vm) {
                    println!("  gui {l}");
                }
            }
            for key in std::env::var("HP3_MENU_KEYS").unwrap_or_default().split(',').filter_map(|k| k.trim().parse::<u8>().ok()) {
                hp3_world::gui::key(&mut vm, key, 1, 0.0);
                hp3_world::gui::key(&mut vm, key, 3, 0.0);
                for _ in 0..10 {
                    hp3_world::tick(&mut vm, 1.0 / 60.0);
                }
                println!("  key {key}: menu up {}", hp3_world::gui::menu_open(&mut vm));
            }
            for e in hp3_world::world(&mut vm).errors.iter().rev().take(5) {
                println!("    error: {e}");
            }
        }
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
                let tiles = hp3_world::canvas::render(&mut vm, 1280, 720);
            if std::env::var_os("HP3_TILES").is_some() {
                for t in &tiles {
                    let m = t.material.map(|m| hp3_script::natives::object_path(&vm, m)).unwrap_or_default();
                    println!("  tile {m} at ({:.0},{:.0}) {:.0}x{:.0}", t.x, t.y, t.w, t.h);
                }
            }
                for click in c.split(';') {
                    let v: Vec<f32> = click.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                    if let Some(top) = hp3_world::gui::top_menu(&mut vm) {
                        for n in ["btnYes", "btnNo", "lblMsg"] {
                            let o = hp3_world::get(&mut vm, top, n).as_obj();
                            let name = o.map(|o| vm.db.names.get(vm.db.objects.get(o).name).to_string());
                            println!("  {n} = {name:?}");
                        }
                    }
                    for l in hp3_world::gui::dump(&mut vm).iter().filter(|l| !l.starts_with(' ')) {
                        println!("  before click: {l}");
                    }
                    if std::env::var_os("HP3_TRACE_CALLS").is_none() { vm.trace_calls = Some("page".into()); }
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
                    let tiles = hp3_world::canvas::render(&mut vm, 1280, 720);
            if std::env::var_os("HP3_TILES").is_some() {
                for t in &tiles {
                    let m = t.material.map(|m| hp3_script::natives::object_path(&vm, m)).unwrap_or_default();
                    println!("  tile {m} at ({:.0},{:.0}) {:.0}x{:.0}", t.x, t.y, t.w, t.h);
                }
            }
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
        let mut classes: Vec<_> = hp3_world::world(&mut vm).tick_classes.clone().into_iter().collect();
        classes.sort_by(|a, b| b.1.1.cmp(&a.1.1));
        println!("time per phase and class (top 25):");
        for ((phase, c), (n, t)) in classes.iter().take(25) {
            println!("  {:>9.1} ms {:>7} calls {phase}: {}", t.as_secs_f64() * 1000.0, n, vm.db.names.get(vm.db.get(*c).name));
        }
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
    hp3_world::memprobe("level end");
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
        for p in def.props.iter() {
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

/// The box the probes sweep: a pawn's by default, `HP3_PROBE_EXT=<r>,<r>,<h>` for another.
fn probe_ext() -> glam::Vec3 {
    let v: Vec<f32> = std::env::var("HP3_PROBE_EXT").ok().map(|s| s.split(',').filter_map(|p| p.trim().parse().ok()).collect()).unwrap_or_default();
    if v.len() == 3 { glam::Vec3::new(v[0], v[1], v[2]) } else { glam::Vec3::new(15.0, 15.0, 38.0) }
}

thread_local! {
    /// The card `HP3_THROW` made and the tick it was thrown.
    static THROWN: std::cell::Cell<Option<(hp3_script::ObjId, u32)>> = const { std::cell::Cell::new(None) };
}
