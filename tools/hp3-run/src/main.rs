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
