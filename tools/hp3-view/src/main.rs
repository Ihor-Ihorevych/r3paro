//! `hp3-view shot <HP3 folder> <map> <out.png> [x,y,z,pitch,yaw]`: renders a map headless to a PNG, from the
//! first player start unless a camera is given.

use std::path::Path;
use std::process::ExitCode;

use hp3_assets::loader::Loader;
use hp3_package::PackageSet;
use hp3_render::gpu::Renderer;
use hp3_render::scene::{self, Camera};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 4 || args[0] != "shot" {
        eprintln!("usage: hp3-view shot <HP3 folder> <map> <out.png> [x,y,z,pitch,yaw]");
        return ExitCode::from(2);
    }
    match shot(Path::new(&args[1]), &args[2], Path::new(&args[3]), args.get(4).map(String::as_str)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
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
            Camera { position: glam::Vec3::new(v[0], v[1], v[2]), rotation: [v[3] as i32, v[4] as i32, 0], fov_degrees: 90.0 }
        }
        None => {
            let (p, r) = scene.player_starts.first().copied().ok_or("map has no player start; give a camera")?;
            Camera { position: p + glam::Vec3::Z * 40.0, rotation: [r[0], r[1], 0], fov_degrees: 90.0 }
        }
    };
    eprintln!("camera {:?}", camera);
    let renderer = Renderer::new_headless()?;
    eprintln!("GPU: {}", renderer.adapter_info);
    let (w, h) = (1280, 720);
    let rgba = renderer.render_to_rgba(&scene, &camera, w, h);
    let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(out)?), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&rgba)?;
    eprintln!("wrote {}", out.display());
    Ok(())
}
