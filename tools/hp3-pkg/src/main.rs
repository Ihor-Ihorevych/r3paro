//! `hp3-pkg`: package inspection and the whole-install check.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use hp3_package::object::{self, Body};
use hp3_package::{ObjRef, Package, PackageSet, check};

const USAGE: &str = "usage:
  hp3-pkg info <package file>
  hp3-pkg exports <package file>
  hp3-pkg imports <package file>
  hp3-pkg names <package file>
  hp3-pkg dump <HP3 folder> <package name> <object path>
  hp3-pkg check <HP3 folder> [package name] [-v]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("info") if args.len() == 2 => info(Path::new(&args[1])),
        Some("exports") if args.len() == 2 => exports(Path::new(&args[1])),
        Some("imports") if args.len() == 2 => imports(Path::new(&args[1])),
        Some("names") if args.len() == 2 => names(Path::new(&args[1])),
        Some("dump") if args.len() == 4 => dump(Path::new(&args[1]), &args[2], &args[3]),
        Some("check") if args.len() >= 2 => {
            let verbose = args.iter().any(|a| a == "-v");
            let filter = args.iter().skip(2).find(|a| !a.starts_with('-')).cloned();
            run_check(Path::new(&args[1]), filter.as_deref(), verbose)
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

type R = Result<ExitCode, Box<dyn std::error::Error>>;

fn info(path: &Path) -> R {
    let p = Package::open(path)?;
    let s = &p.summary;
    println!("package     {}", p.name);
    println!("version     {} (licensee {})", s.version, s.licensee);
    println!("flags       {:#010x}", s.flags);
    println!("names       {} at {:#x}", s.name_count, s.name_offset);
    println!("imports     {} at {:#x}", s.import_count, s.import_offset);
    println!("exports     {} at {:#x}", s.export_count, s.export_offset);
    println!("generations {:?}", s.generations);
    Ok(ExitCode::SUCCESS)
}

fn exports(path: &Path) -> R {
    let p = Package::open(path)?;
    for (i, e) in p.exports.iter().enumerate() {
        println!(
            "{i:5} {:<24} {:<50} flags={:#010x} size={} at={:#x}",
            p.export_class_name(i),
            p.ref_path(ObjRef::from_export(i)),
            e.flags,
            e.serial_size,
            e.serial_offset
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn imports(path: &Path) -> R {
    let p = Package::open(path)?;
    for (i, imp) in p.imports.iter().enumerate() {
        println!(
            "{i:5} {}.{:<24} {}",
            p.name(imp.class_package),
            p.name(imp.class_name),
            p.ref_path(ObjRef(-(i as i32) - 1))
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn names(path: &Path) -> R {
    let p = Package::open(path)?;
    for (i, n) in p.names.iter().enumerate() {
        println!("{i:5} {:#010x} {}", n.flags, n.name);
    }
    Ok(ExitCode::SUCCESS)
}

fn dump(root: &Path, package: &str, path: &str) -> R {
    let set = PackageSet::open(root)?;
    let pkg = set.get(package)?;
    let index = pkg.find_export_path(path).ok_or_else(|| format!("no export {path} in {package}"))?;
    let chain = set.export_chain(&pkg, index);
    println!("class chain: {}", chain.join(" > "));
    let e = &pkg.exports[index];
    println!("flags {:#010x}, {} bytes at {:#x}", e.flags, e.serial_size, e.serial_offset);
    let o = object::read_export(&pkg, index, &chain)?;
    println!("read as {}", o.native_class);
    if let Some(sf) = &o.state_frame {
        println!("state frame: {sf:?}");
    }
    for t in &o.tags {
        let sname = t.struct_name.map(|n| format!(" ({})", pkg.name(n))).unwrap_or_default();
        println!(
            "  {}[{}] {:?}{sname} {} bytes{}",
            pkg.name(t.name),
            t.array_index,
            t.ty,
            t.data.len(),
            if t.ty == hp3_package::props::TagType::Bool { format!(" = {}", t.bool_value) } else { String::new() }
        );
    }
    match &o.body {
        Body::Class { structure, class, defaults, .. } => {
            println!("class flags {:#010x}, script {} bytes, {} defaults", class.class_flags, structure.script_size, defaults.len());
        }
        Body::Function { structure, function } => {
            println!("function native={} flags={:#010x} script {} bytes", function.native_index, function.function_flags, structure.script_size);
            for t in &structure.script {
                println!("  {t:?}");
            }
        }
        other => println!("{other:#?}"),
    }
    if !o.tail.is_empty() {
        println!("{} undecoded bytes:", o.tail.len());
        hexdump(&o.tail[..o.tail.len().min(256)]);
    }
    Ok(ExitCode::SUCCESS)
}

fn hexdump(b: &[u8]) {
    for (i, chunk) in b.chunks(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|x| format!("{x:02x}")).collect();
        println!("  {:04x}: {}", i * 16, hex.join(" "));
    }
}

fn run_check(root: &Path, filter: Option<&str>, verbose: bool) -> R {
    let set = PackageSet::open(root)?;
    let files: Vec<PathBuf> = set.files();
    eprintln!("{} package files under {}", files.len(), root.display());
    let rep = check::check(&set, filter, |name| {
        if verbose {
            eprintln!("  {name}");
        }
    });
    println!("packages: {} read, {} failed", rep.packages, rep.package_errors.len());
    for (p, e) in &rep.package_errors {
        println!("  {}: {e}", p.display());
    }
    println!(
        "exports:  {} total, {} decoded exactly, {} with leftover bytes, {} errors",
        rep.exports,
        rep.decoded,
        rep.leftover_exports(),
        rep.errors()
    );
    println!("scripts:  {} structs with {} bytes of bytecode", rep.scripts, rep.script_bytes);
    println!("\nproblems by class (native layout (own class)):");
    for (k, s) in &rep.by_class {
        if s.errors == 0 && s.leftover_exports == 0 {
            continue;
        }
        println!(
            "  {k}: {} exports, {} errors, {} with {} leftover bytes\n      first: {}",
            s.exports,
            s.errors,
            s.leftover_exports,
            s.leftover_bytes,
            s.first_problem.as_deref().unwrap_or("-")
        );
    }
    println!("\nnative data not decoded yet:");
    for (k, (n, b)) in &rep.unparsed {
        println!("  {k}: {n} exports, {b} bytes");
    }
    if verbose {
        println!("\nall classes:");
        for (k, s) in &rep.by_class {
            println!("  {k}: {} exports, {} bytes", s.exports, s.bytes);
        }
    }
    Ok(if rep.errors() == 0 && rep.leftover_exports() == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}
