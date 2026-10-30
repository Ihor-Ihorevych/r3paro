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
        Some("tails") if args.len() >= 3 => tails(Path::new(&args[1]), &args[2], args.get(3).map(|s| s.parse().unwrap_or(20)).unwrap_or(20)),
        Some("textures") if args.len() >= 4 => {
            textures(Path::new(&args[1]), &args[2], Path::new(&args[3]), args.get(4).map(String::as_str))
        }
        Some("hex") if args.len() == 6 => hex(Path::new(&args[1]), &args[2], &args[3], &args[4], &args[5]),
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

/// For exports of one native class: what the asset decoder leaves over, grouped by length.
fn tails(root: &Path, class: &str, max: usize) -> R {
    let set = PackageSet::open(root)?;
    let mut by_len: std::collections::BTreeMap<usize, Vec<String>> = Default::default();
    let mut errors = 0;
    for path in set.files() {
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        let pkg = set.get(&stem)?;
        for i in 0..pkg.exports.len() {
            let chain = set.export_chain(&pkg, i);
            let Ok(o) = object::read_export(&pkg, i, &chain) else { continue };
            if !o.native_class.eq_ignore_ascii_case(class) || !matches!(o.body, Body::Unparsed) {
                continue;
            }
            let size = pkg.exports[i].serial_size;
            let base = pkg.exports[i].serial_offset + size - o.tail.len();
            let mut r = hp3_package::Reader::with_base(o.tail, base);
            let at = |r: &hp3_package::Reader<'_>| r.file_pos() - pkg.exports[i].serial_offset;
            match hp3_assets::read_native(o.native_class, &pkg, &mut r) {
                Some(Ok(_)) => {
                    let off = at(&r);
                    let rest = r.rest();
                    let hex: Vec<String> = rest.iter().take(48).map(|b| format!("{b:02x}")).collect();
                    by_len.entry(rest.len()).or_default().push(format!(
                        "{}:{} @{off:#x}: {}",
                        pkg.name,
                        pkg.ref_path(ObjRef::from_export(i)),
                        hex.join(" ")
                    ));
                }
                Some(Err(e)) => {
                    errors += 1;
                    if errors <= max {
                        println!("error {}:{}: {e}", pkg.name, pkg.ref_path(ObjRef::from_export(i)));
                    }
                }
                None => {}
            }
        }
        set.unload_all();
    }
    for (len, items) in &by_len {
        println!("{len} bytes left: {} exports", items.len());
        for s in items.iter().take(max) {
            println!("  {s}");
        }
    }
    println!("{errors} errors");
    Ok(ExitCode::SUCCESS)
}

/// Writes the first mip of every texture in a package (optionally only names containing `filter`) as PNG.
fn textures(root: &Path, package: &str, out: &Path, filter: Option<&str>) -> R {
    use hp3_assets::texture::{self, Format};
    use hp3_package::props;
    let set = PackageSet::open(root)?;
    let pkg = set.get(package)?;
    std::fs::create_dir_all(out)?;
    let (mut written, mut skipped) = (0, 0);
    for i in 0..pkg.exports.len() {
        let name = pkg.ref_path(ObjRef::from_export(i));
        if filter.is_some_and(|f| !name.to_ascii_lowercase().contains(&f.to_ascii_lowercase())) {
            continue;
        }
        let chain = set.export_chain(&pkg, i);
        let o = object::read_export(&pkg, i, &chain)?;
        if !matches!(o.native_class, "Texture" | "FireTexture" | "WetTexture" | "IceTexture" | "MovieTexture") {
            continue;
        }
        let format = props::find(&pkg, &o.tags, "Format", 0).and_then(|t| t.as_u8()).unwrap_or(0);
        let Some(format) = Format::from_u8(format) else {
            skipped += 1;
            continue;
        };
        let base = pkg.exports[i].serial_offset + pkg.exports[i].serial_size - o.tail.len();
        let mut r = hp3_package::Reader::with_base(o.tail, base);
        let tex = texture::read_texture(&mut r)?;
        let Some(mip) = tex.mips.first() else {
            skipped += 1;
            continue;
        };
        let palette = props::find(&pkg, &o.tags, "Palette", 0)
            .and_then(|t| t.as_obj())
            .and_then(|p| p.export())
            .and_then(|pi| {
                let pchain = set.export_chain(&pkg, pi);
                let po = object::read_export(&pkg, pi, &pchain).ok()?;
                let mut pr = hp3_package::Reader::new(po.tail);
                texture::read_palette(&mut pr).ok()
            });
        let data = &pkg.data[mip.data_offset..mip.data_offset + mip.data_len];
        let (w, h) = (mip.width as usize, mip.height as usize);
        let Some(rgba) = texture::decode_rgba(format, w, h, data, palette.as_ref()) else {
            eprintln!("skip {name}: {format:?} {w}x{h} ({} bytes)", data.len());
            skipped += 1;
            continue;
        };
        let file = out.join(format!("{}.png", name.replace(['.', '/', '\\'], "_")));
        let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(&file)?), w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        written += 1;
    }
    println!("{written} textures written to {}, {skipped} skipped", out.display());
    Ok(ExitCode::SUCCESS)
}

fn parse_num(s: &str) -> Result<usize, std::num::ParseIntError> {
    match s.strip_prefix("0x") {
        Some(h) => usize::from_str_radix(h, 16),
        None => s.parse(),
    }
}

/// Hex dump of part of an export's serialized data (offsets relative to the export).
fn hex(root: &Path, package: &str, path: &str, offset: &str, len: &str) -> R {
    let set = PackageSet::open(root)?;
    let pkg = set.get(package)?;
    let index = pkg.find_export_path(path).ok_or_else(|| format!("no export {path} in {package}"))?;
    let data = pkg.export_data(index);
    let start = parse_num(offset)?.min(data.len());
    let end = (start + parse_num(len)?).min(data.len());
    for (i, chunk) in data[start..end].chunks(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|x| format!("{x:02x}")).collect();
        let floats: Vec<String> = chunk
            .chunks_exact(4)
            .map(|c| format!("{:.3}", f32::from_le_bytes([c[0], c[1], c[2], c[3]])))
            .collect();
        println!("  {:06x}: {:<47}  {}", start + i * 16, hex.join(" "), floats.join(" "));
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
    let decode = |class: &str, pkg: &Package, r: &mut hp3_package::Reader<'_>| {
        hp3_assets::read_native(class, pkg, r).map(|res| res.map(|_| ()))
    };
    let rep = check::check(&set, filter, &decode, |name| {
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
