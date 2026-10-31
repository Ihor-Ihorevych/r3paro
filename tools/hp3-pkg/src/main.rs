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
        Some("natives") if args.len() == 2 => natives(Path::new(&args[1])),
        Some("sounds") if args.len() >= 3 => sounds(Path::new(&args[1]), &args[2], args.get(3).map(Path::new)),
        Some("lightmaps") if args.len() == 3 => lightmaps(Path::new(&args[1]), &args[2]),
        Some("disasm") if args.len() == 4 => disasm(Path::new(&args[1]), &args[2], &args[3]),
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
            for t in defaults {
                println!("  default {}[{}] {:?} {} bytes{}", pkg.name(t.name), t.array_index, t.ty, t.data.len(), if t.ty == hp3_package::props::TagType::Bool { format!(" = {}", t.bool_value) } else { format!(" {:02x?}", &t.data[..t.data.len().min(12)]) });
            }
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
/// Summary of the lighting data in a map's BSP models.
/// Summarises a package's sounds by file type and (with an output folder) writes each sound's file out.
fn sounds(root: &Path, package: &str, out: Option<&Path>) -> R {
    use hp3_assets::Asset;
    use std::collections::BTreeMap;
    let set = PackageSet::open(root)?;
    let pkg = set.get(package)?;
    let mut by_type: BTreeMap<String, (usize, usize, Vec<u8>)> = BTreeMap::new();
    let mut decoded: Vec<(f64, f64, f64, bool, f64)> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for i in 0..pkg.exports.len() {
        if !pkg.export_class_name(i).eq_ignore_ascii_case("Sound") {
            continue;
        }
        let chain = set.export_chain(&pkg, i);
        let o = object::read_export(&pkg, i, &chain)?;
        let base = pkg.exports[i].serial_offset + pkg.exports[i].serial_size - o.tail.len();
        let mut r = hp3_package::Reader::with_base(o.tail, base);
        let Some(Ok(Asset::Sound(snd))) = hp3_assets::read_native("Sound", &pkg, &mut r) else { continue };
        let data = &pkg.data[snd.data_offset..snd.data_offset + snd.data_len];
        let e = by_type.entry(snd.file_type.clone()).or_insert((0, 0, data[..data.len().min(48)].to_vec()));
        e.0 += 1;
        e.1 += snd.data_len;
        let name = pkg.ref_path(ObjRef::from_export(i)).replace(['.', '/', '\\'], "_");
        // HP3_SOUND_FILTER=<text>: only sounds whose name contains it are decoded.
        if std::env::var("HP3_SOUND_FILTER").is_ok_and(|f| !name.to_ascii_lowercase().contains(&f.to_ascii_lowercase())) {
            continue;
        }
        if snd.file_type == "bik" {
            match hp3_audio::bink::decode_with_stats(data) {
                Ok((pcm, st)) => {
                    let peak = pcm.samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
                    let rms = (pcm.samples.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / pcm.samples.len().max(1) as f64).sqrt();
                    decoded.push((st.overlap_mismatch as f64, peak as f64, rms, st.dct, st.roughness as f64));
                    if let Some(dir) = out {
                        std::fs::create_dir_all(dir)?;
                        std::fs::write(dir.join(format!("{name}.wav")), hp3_audio::wav::to_wav(&pcm))?;
                    }
                }
                Err(e) => failures.push(format!("{name}: {e}")),
            }
        } else if let Some(dir) = out {
            std::fs::create_dir_all(dir)?;
            std::fs::write(dir.join(format!("{name}.{}", snd.file_type)), data)?;
        }
    }
    if !decoded.is_empty() {
        let n = decoded.len() as f64;
        let mean = |f: &dyn Fn(&(f64, f64, f64, bool, f64)) -> f64| decoded.iter().map(f).sum::<f64>() / n;
        println!(
            "decoded {} bink sounds ({} DCT): mean overlap mismatch {:.3}, roughness {:.3}, mean peak {:.0}, mean rms {:.0}, {} failed",
            decoded.len(),
            decoded.iter().filter(|d| d.3).count(),
            mean(&|d| d.0),
            mean(&|d| d.4),
            mean(&|d| d.1),
            mean(&|d| d.2),
            failures.len()
        );
        for f in failures.iter().take(5) {
            println!("  failed: {f}");
        }
    }
    for (ty, (n, bytes, head)) in &by_type {
        println!("{ty}: {n} sounds, {bytes} bytes; first starts {:02x?} {:?}", &head[..head.len().min(16)], String::from_utf8_lossy(&head[..head.len().min(16)]));
    }
    Ok(ExitCode::SUCCESS)
}

fn lightmaps(root: &Path, map: &str) -> R {
    use hp3_assets::Asset;
    let set = PackageSet::open(root)?;
    let pkg = set.get(map)?;
    let mut shown = 0;
    for i in 0..pkg.exports.len() {
        if shown < 4 && pkg.export_class_name(i).eq_ignore_ascii_case("StaticMeshInstance") {
            let chain = set.export_chain(&pkg, i);
            let o = object::read_export(&pkg, i, &chain)?;
            let base = pkg.exports[i].serial_offset + pkg.exports[i].serial_size - o.tail.len();
            let mut r = hp3_package::Reader::with_base(o.tail, base);
            if let Some(Ok(Asset::StaticMeshInstance(smi))) = hp3_assets::read_native("StaticMeshInstance", &pkg, &mut r) {
                shown += 1;
                println!(
                    "{}: {} colors (first {:?}), {} lights: {:?}",
                    pkg.ref_path(ObjRef::from_export(i)),
                    smi.colors.len(),
                    smi.colors.first(),
                    smi.lights.len(),
                    smi.lights.iter().take(3).map(|l| (l.light.export(), l.visibility.len(), l.apply)).collect::<Vec<_>>()
                );
            }
        }
    }
    for i in 0..pkg.exports.len() {
        let chain = set.export_chain(&pkg, i);
        let o = object::read_export(&pkg, i, &chain)?;
        if o.native_class != "Model" {
            continue;
        }
        let base = pkg.exports[i].serial_offset + pkg.exports[i].serial_size - o.tail.len();
        let mut r = hp3_package::Reader::with_base(o.tail, base);
        let Some(Ok(Asset::Model(m))) = hp3_assets::read_native("Model", &pkg, &mut r) else { continue };
        if m.sections.is_empty() && m.light_maps.is_empty() {
            continue;
        }
        let lit_sections = m.sections.iter().filter(|s| s.light_map_texture >= 0).count();
        println!(
            "{}: {} nodes, {} sections ({} with a lightmap page), {} lights, {} lightmaps, {} pages",
            pkg.ref_path(ObjRef::from_export(i)),
            m.nodes.len(),
            m.sections.len(),
            lit_sections,
            m.lights.len(),
            m.light_maps.len(),
            m.light_map_textures.len()
        );
        for (pi, t) in m.light_map_textures.iter().take(4).enumerate() {
            println!("  page {pi}: {} lightmaps, data {:?}, lead {}, trailer {:?}", t.light_maps.len(), t.data_lens, t.lead, t.trailer);
        }
        for lm in m.light_maps.iter().take(2) {
            let surf = m.surfs.get(lm.surf as usize);
            println!("  lightmap base {:?} axis_x {:?} axis_y {:?} surf scale {:?} matrix {:?}", lm.base, lm.axis_x, lm.axis_y, surf.map(|s| s.light_map_scale), lm.world_to_light_map);
            // Vertices of nodes on this surface, with their lightmap UVs.
            for n in m.nodes.iter().filter(|n| n.surf == lm.surf && n.section >= 0).take(1) {
                let sec = &m.sections[n.section as usize];
                for v in sec.vertices.iter().skip(n.first_vertex as usize).take(n.num_vertices as usize) {
                    println!("    vertex {:?} lm_uv {:?} (page {})", v.position, v.light_map_uv, sec.light_map_texture);
                }
            }
        }
        for lm in m.light_maps.iter().take(3) {
            println!(
                "  lightmap surf {} offset {:?} size {:?} flags {} bitmaps {} (first {:?})",
                lm.surf,
                lm.offset,
                lm.size,
                lm.flags,
                lm.bitmaps.len(),
                lm.bitmaps.first().map(|b| (b.size, b.stride, b.bits.len()))
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

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

const FUNC_NATIVE: u32 = 0x0000_0400;

/// Every native function the script packages declare: package, class, function, native index and flags.
fn natives(root: &Path) -> R {
    let set = PackageSet::open(root)?;
    let mut total = 0;
    let mut indexed = 0;
    let mut per_package: std::collections::BTreeMap<String, usize> = Default::default();
    for path in set.files() {
        if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("u")) {
            continue;
        }
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        let pkg = set.get(&stem)?;
        for i in 0..pkg.exports.len() {
            if !pkg.export_class_name(i).eq_ignore_ascii_case("Function") {
                continue;
            }
            let chain = set.export_chain(&pkg, i);
            let Ok(o) = object::read_export(&pkg, i, &chain) else { continue };
            let Body::Function { function, .. } = &o.body else { continue };
            if function.function_flags & FUNC_NATIVE == 0 {
                continue;
            }
            total += 1;
            if function.native_index != 0 {
                indexed += 1;
            }
            *per_package.entry(stem.clone()).or_default() += 1;
            println!(
                "{}\t{}\t{}\t{:#010x}",
                pkg.ref_path(ObjRef::from_export(i)),
                function.native_index,
                function.operator_precedence,
                function.function_flags
            );
        }
        set.unload_all();
    }
    eprintln!("{total} native functions ({indexed} with a fixed index): {per_package:?}");
    Ok(ExitCode::SUCCESS)
}

fn token_text(pkg: &Package, t: &hp3_package::bytecode::Token, depth: usize, out: &mut String) {
    use hp3_package::bytecode::Operand;
    use std::fmt::Write;
    let pad = "  ".repeat(depth);
    let ops: Vec<String> = t
        .operands
        .iter()
        .map(|o| match o {
            Operand::Obj(r) => pkg.ref_path(*r),
            Operand::Name(n) => format!("'{}'", pkg.name(*n)),
            Operand::U8(v) => v.to_string(),
            Operand::U16(v) => format!("{v:#x}"),
            Operand::I32(v) => v.to_string(),
            Operand::F32(v) => format!("{v}"),
            Operand::Str(s) => format!("{s:?}"),
            Operand::Labels(l) => l.iter().map(|(n, at)| format!("{}@{at:#x}", pkg.name(*n))).collect::<Vec<_>>().join(","),
        })
        .collect();
    let head = match t.native {
        Some(n) => format!("native {n}"),
        None => format!("{:#04x}", t.op),
    };
    let _ = writeln!(out, "{:04x} {pad}{head} {}", t.offset, ops.join(" "));
    for a in &t.args {
        token_text(pkg, a, depth + 1, out);
    }
}

/// Bytecode of every function and state of one class.
fn disasm(root: &Path, package: &str, class: &str) -> R {
    let set = PackageSet::open(root)?;
    let pkg = set.get(package)?;
    // `*` disassembles every class of the package.
    let all = class == "*";
    let ci = if all { 0 } else { pkg.find_export_path(&format!("{class}@Class")).ok_or("no such class")? };
    let mut out = String::new();
    for i in 0..pkg.exports.len() {
        let path = pkg.ref_path(ObjRef::from_export(i));
        let under = all
            || pkg.exports[i].outer == ObjRef::from_export(ci)
            || pkg.ref_outer(pkg.exports[i].outer) == ObjRef::from_export(ci)
            || i == ci;
        if !under {
            continue;
        }
        let chain = set.export_chain(&pkg, i);
        let o = object::read_export(&pkg, i, &chain)?;
        let structure = match &o.body {
            Body::Function { structure, .. } | Body::State { structure, .. } | Body::Class { structure, .. } => structure,
            _ => continue,
        };
        if structure.script.is_empty() {
            continue;
        }
        out.push_str(&format!("\n== {} {path} ({} bytes)\n", pkg.export_class_name(i), structure.script_size));
        for t in &structure.script {
            token_text(&pkg, t, 0, &mut out);
        }
    }
    print!("{out}");
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
