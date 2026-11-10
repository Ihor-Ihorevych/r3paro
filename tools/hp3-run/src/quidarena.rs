//! `hp3-run quid-arena <HP3 folder>`: inventory of the Quidditch stadium. Prints every static mesh of the
//! `Quid_Stadium` group (name, bounding box, triangle count), the terrain extents of `HP3_Quidditch` and the placed
//! actors that carry a static mesh, with their locations.

use std::path::Path;

use hp3_assets::Asset;
use hp3_package::object;
use hp3_package::props::{Tag, TagType};
use hp3_package::{ObjRef, Package, PackageSet};

/// Decodes export `index` of `pkg` natively.
fn decode(set: &PackageSet, pkg: &Package, index: usize) -> Option<Asset> {
    let chain = set.export_chain(pkg, index);
    let o = object::read_export(pkg, index, &chain).ok()?;
    let base = pkg.exports[index].serial_offset + pkg.exports[index].serial_size - o.tail.len();
    let mut r = hp3_package::Reader::with_base(o.tail, base);
    hp3_assets::read_native(o.native_class, pkg, &mut r)?.ok()
}

/// Three little-endian floats of a vector or rotator-like tag.
fn floats(t: &Tag<'_>) -> Option<Vec<f32>> {
    let d = t.data;
    if d.len() < 12 {
        return None;
    }
    Some((0..d.len() / 4).map(|k| f32::from_le_bytes([d[k * 4], d[k * 4 + 1], d[k * 4 + 2], d[k * 4 + 3]])).collect())
}

fn show(pkg: &Package, t: &Tag<'_>) -> String {
    match t.ty {
        TagType::Object => t.as_obj().map(|o| pkg.ref_path(o)).unwrap_or_default(),
        TagType::Vector | TagType::Struct => match floats(t) {
            Some(f) => format!("{:.1?}", f),
            None => String::new(),
        },
        TagType::Rotator => {
            let d = t.data;
            if d.len() < 12 {
                return String::new();
            }
            let a: Vec<i32> = (0..3).map(|k| i32::from_le_bytes([d[k * 4], d[k * 4 + 1], d[k * 4 + 2], d[k * 4 + 3]])).collect();
            format!("[{}, {}, {}]", a[0], a[1], a[2])
        }
        TagType::Float => format!("{:?}", t.as_f32()),
        _ => String::new(),
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    let root = args.first().ok_or("usage: hp3-run quid-arena <HP3 folder>")?;
    let set = PackageSet::open(Path::new(root)).map_err(|e| e.to_string())?;
    let pkg = set.get("HP3_ExteriorArch_SM").map_err(|e| e.to_string())?;
    println!("package HP3_ExteriorArch_SM: {} exports", pkg.exports.len());
    println!("name  min  max  size  tris");
    for i in 0..pkg.exports.len() {
        if !pkg.export_class_name(i).eq_ignore_ascii_case("StaticMesh") {
            continue;
        }
        let path = pkg.ref_path(ObjRef::from_export(i));
        if !path.to_ascii_lowercase().contains("quid") {
            continue;
        }
        let Some(Asset::StaticMesh(m)) = decode(&set, &pkg, i) else {
            println!("{path}  (undecodable)");
            continue;
        };
        let b = &m.bounds.bounding_box;
        let (lo, hi) = (b.min, b.max);
        println!(
            "{path}  min=({:.1}, {:.1}, {:.1})  max=({:.1}, {:.1}, {:.1})  size=({:.1}, {:.1}, {:.1})  tris={}  rawtris={}",
            lo.x, lo.y, lo.z, hi.x, hi.y, hi.z, hi.x - lo.x, hi.y - lo.y, hi.z - lo.z,
            m.indices.len() / 3, m.raw_triangles.len()
        );
    }
    // The goal post mesh is tiny: dump its triangles to see whether hoops are part of it.
    if let Some(i) = pkg.find_export_path("Quid_Stadium.Quid_GoalPosts_SM") {
        if let Some(Asset::StaticMesh(m)) = decode(&set, &pkg, i) {
            println!("goal posts triangles (local):");
            for t in &m.raw_triangles {
                let v = |k: usize| format!("({:.0},{:.0},{:.0})", t.vertices[k].x, t.vertices[k].y, t.vertices[k].z);
                println!("  {} {} {}", v(0), v(1), v(2));
            }
        }
    }
    let map = set.get("HP3_Quidditch").map_err(|e| e.to_string())?;
    println!("\nmap HP3_Quidditch: {} exports", map.exports.len());
    for i in 0..map.exports.len() {
        if !map.export_class_name(i).eq_ignore_ascii_case("TerrainInfo") {
            continue;
        }
        if let Some(Asset::Terrain(t)) = decode(&set, &map, i) {
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for v in &t.vertices {
                for (k, c) in [v.x, v.y, v.z].into_iter().enumerate() {
                    lo[k] = lo[k].min(c);
                    hi[k] = hi[k].max(c);
                }
            }
            println!(
                "terrain {}: {} verts, sectors {}x{}, heightmap {}x{}, vertex min=({:.1}, {:.1}, {:.1}) max=({:.1}, {:.1}, {:.1}), to_world origin=({:.1}, {:.1}, {:.1})",
                map.ref_path(ObjRef::from_export(i)), t.vertices.len(), t.sectors_x, t.sectors_y, t.heightmap_x, t.heightmap_y,
                lo[0], lo[1], lo[2], hi[0], hi[1], hi[2], t.to_world.origin.x, t.to_world.origin.y, t.to_world.origin.z
            );
        }
    }
    // Placed actors with a static mesh (or named like a stadium part).
    let mut count = 0;
    for i in 0..map.exports.len() {
        let class = map.export_class_name(i).to_string();
        let chain = set.export_chain(&map, i);
        let Ok(o) = object::read_export(&map, i, &chain) else { continue };
        let mut mesh = String::new();
        let mut extra = String::new();
        for t in &o.tags {
            match map.name(t.name) {
                "StaticMesh" => mesh = show(&map, t),
                "Rotation" => {
                    let d = t.data;
                    if d.len() >= 12 {
                        let a: Vec<i32> = (0..3).map(|k| i32::from_le_bytes([d[k * 4], d[k * 4 + 1], d[k * 4 + 2], d[k * 4 + 3]])).collect();
                        extra.push_str(&format!(" Rotation(pitch,yaw,roll uu)={a:?}"));
                    }
                }
                n @ ("Location" | "DrawScale" | "DrawScale3D") => extra.push_str(&format!(" {n}={}", show(&map, t))),
                _ => {}
            }
        }
        let lc = class.to_ascii_lowercase();
        if mesh.is_empty() && !(lc.contains("quid") || lc.contains("hoop") || lc.contains("goal")) {
            continue;
        }
        if !mesh.to_ascii_lowercase().contains("quid") && !lc.contains("quid") && !lc.contains("hoop") && !lc.contains("goal") {
            continue;
        }
        count += 1;
        println!("actor {} class={class} mesh={mesh}{extra}", map.ref_path(ObjRef::from_export(i)));
    }
    println!("{count} quidditch actors listed");
    // Every placed static mesh by name, to see which stadium parts the map really uses.
    let mut used: std::collections::BTreeMap<String, usize> = Default::default();
    for i in 0..map.exports.len() {
        if !map.export_class_name(i).eq_ignore_ascii_case("StaticMeshActor") {
            continue;
        }
        let chain = set.export_chain(&map, i);
        let Ok(o) = object::read_export(&map, i, &chain) else { continue };
        for t in &o.tags {
            if map.name(t.name) == "StaticMesh" {
                *used.entry(show(&map, t)).or_default() += 1;
            }
        }
    }
    println!("static meshes placed in HP3_Quidditch:");
    for (m, n) in &used {
        println!("  {n:3} x {m}");
    }
    // Hoop-like exports in the static mesh packages and the map.
    for path in set.files() {
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if ext != "usx" && stem != "HP3_Quidditch" {
            continue;
        }
        let Ok(p) = set.get(&stem) else { continue };
        for i in 0..p.exports.len() {
            let path = p.ref_path(ObjRef::from_export(i));
            let lc = path.to_ascii_lowercase();
            if lc.contains("hoop") || lc.contains("goalpost") || lc.contains("ring") && lc.contains("quid") {
                println!("hoop-like {path} ({})", p.export_class_name(i));
            }
        }
    }
    Ok(())
}
