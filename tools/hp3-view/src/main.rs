//! `hp3-view`: headless renders to PNG.
//!
//! - `shot <HP3 folder> <map> <out.png> [x,y,z,pitch,yaw]`: the level's static geometry, from the first
//!   player start unless a camera is given.
//! - `play <HP3 folder> <map> <out.png> [ticks]`: loads the level into the world, runs it, and renders the
//!   static geometry plus the live actors from behind the player's pawn.

use std::path::Path;
use std::process::ExitCode;

use glam::Vec3;
use hp3_assets::loader::Loader;
use hp3_package::PackageSet;
use hp3_render::dynamic::Dynamic;
use hp3_render::gpu::Renderer;
use hp3_render::scene::{self, Camera};
use hp3_script::{ClassDb, Value, Vm};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("shot") if args.len() >= 4 => shot(Path::new(&args[1]), &args[2], Path::new(&args[3]), args.get(4).map(String::as_str)),
        Some("play") if args.len() >= 4 => play(Path::new(&args[1]), &args[2], Path::new(&args[3]), args.get(4).and_then(|t| t.parse().ok()).unwrap_or(60)),
        _ => {
            eprintln!("usage: hp3-view shot <HP3 folder> <map> <out.png> [x,y,z,pitch,yaw]\n       hp3-view play <HP3 folder> <map> <out.png> [ticks]");
            return ExitCode::from(2);
        }
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn write_png(out: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(out)?), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(rgba)?;
    eprintln!("wrote {}", out.display());
    Ok(())
}

fn shot(root: &Path, map: &str, out: &Path, cam: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let set = PackageSet::open(root)?;
    let loader = Loader::new(&set);
    let pkg = set.get(map)?;
    let t = std::time::Instant::now();
    let scene = scene::build(&loader, &pkg);
    eprintln!(
        "scene: {} batches, {} textures, {:?}, {} player starts, built in {:?}",
        scene.batches.len(),
        scene.images.len(),
        scene.stats,
        scene.player_starts.len(),
        t.elapsed()
    );
    let camera = match cam {
        Some(s) => {
            let v: Vec<f32> = s.split(',').map(|x| x.trim().parse()).collect::<Result<_, _>>()?;
            if v.len() != 5 {
                return Err("camera is x,y,z,pitch,yaw".into());
            }
            Camera { position: Vec3::new(v[0], v[1], v[2]), rotation: [v[3] as i32, v[4] as i32, 0], fov_degrees: 90.0 }
        }
        None => {
            let (p, r) = scene.player_starts.first().copied().ok_or("map has no player start; give a camera")?;
            Camera { position: p + Vec3::Z * 40.0, rotation: [r[0], r[1], 0], fov_degrees: 90.0 }
        }
    };
    let renderer = Renderer::new_headless()?;
    eprintln!("GPU: {}", renderer.adapter_info);
    let (w, h) = (1280, 720);
    let rgba = renderer.render_to_rgba(&scene, &camera, w, h);
    write_png(out, w, h, &rgba)
}

fn play(root: &Path, map: &str, out: &Path, ticks: u32) -> Result<(), Box<dyn std::error::Error>> {
    let set = PackageSet::open(root)?;
    let loader = Loader::new(&set);
    let pkg = set.get(map)?;
    let mut db = ClassDb::new(&set);
    db.load_all_classes();
    let mut vm = Vm::new(db);
    hp3_world::load(&mut vm, &pkg, hp3_core::Config::load(root))?;
    if let Ok(keys) = std::env::var("HP3_HOLD") {
        for k in keys.split(',') {
            hp3_world::world(&mut vm).input.key_down(k.trim());
        }
    }
    for _ in 0..ticks {
        hp3_world::tick(&mut vm, 1.0 / 60.0);
    }
    let statics = hp3_engine::static_exports(&mut vm, &pkg);
    let scene = scene::build_with(&loader, &pkg, &|i| statics.contains(&i));
    let renderer = Renderer::new_headless()?;
    let dynamic = Dynamic::new(&renderer);
    let mut cache = hp3_engine::MeshCache::default();
    let instances = hp3_engine::actor_instances(&mut vm, &loader, &renderer, &dynamic, &mut cache);
    eprintln!("{} static batches, {} live actors drawn", scene.batches.len(), instances.len());
    let pc = hp3_world::world(&mut vm).player.ok_or("no player")?;
    let pawn = hp3_world::get(&mut vm, pc, "Pawn").as_obj().ok_or("no pawn")?;
    let p = hp3_world::physics::vec(&hp3_world::get(&mut vm, pawn, "Location"));
    let yaw = hp3_world::get(&mut vm, pawn, "Rotation").fields().get(1).map(Value::as_int).unwrap_or(0);
    let k = std::f32::consts::TAU / 65536.0;
    let back = Vec3::new((yaw as f32 * k).cos(), (yaw as f32 * k).sin(), 0.0);
    let camera = Camera { position: p - back * 220.0 + Vec3::Z * 90.0, rotation: [-2500, yaw, 0], fov_degrees: 90.0 };
    let (w, h) = (1280, 720);
    let gpu_scene = renderer.upload_scene(&scene);
    let refs: Vec<_> = instances.iter().map(|(m, t)| (m.as_ref(), *t)).collect();
    let rgba = renderer.render_frame_to_rgba(w, h, |color, depth| dynamic.draw(&renderer, &gpu_scene, &refs, &camera, color, depth, w as f32 / h as f32));
    write_png(out, w, h, &rgba)
}
