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
        Some("pose") if args.len() >= 5 => pose(
            Path::new(&args[1]),
            &args[2],
            &args[3],
            Path::new(&args[4]),
            args.get(5).map(String::as_str),
            args.get(6).and_then(|f| f.parse().ok()).unwrap_or(0.0),
        ),
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

/// Renders one skeletal mesh posed at `frame` of a sequence of its default animation set.
fn pose(root: &Path, package: &str, mesh: &str, out: &Path, seq: Option<&str>, frame: f32) -> Result<(), Box<dyn std::error::Error>> {
    use hp3_assets::Asset;
    let set = PackageSet::open(root)?;
    let loader = Loader::new(&set);
    let pkg = set.get(package)?;
    let mi = pkg.find_export_path(&format!("{mesh}@SkeletalMesh")).ok_or("no such skeletal mesh")?;
    let asset = loader.asset(&pkg, mi).ok_or("mesh does not decode")?;
    let Asset::SkeletalMesh(m) = &*asset else { return Err("not a skeletal mesh".into()) };
    let skel = hp3_anim::Skeleton::from_mesh(m, &pkg);
    let anim = set.resolve(&pkg, m.default_anim).and_then(|(ap, ai)| match loader.asset(&ap, ai).as_deref() {
        Some(Asset::MeshAnimation(a)) => Some(hp3_anim::AnimSet::new((**a).clone(), &ap)),
        _ => None,
    });
    // HP3_PROBE_SEQ=<sequence>: how far a few bones travel over the sequence (root motion and sway).
    if let (Ok(name), Some(set)) = (std::env::var("HP3_PROBE_SEQ"), &anim)
        && let Some(seq) = set.find(&name)
    {
        let map = set.bone_map(&skel);
        let frames = set.sequences[seq].frames;
        println!("{name}: {frames} frames at {} per second", set.sequences[seq].rate);
        for bone in 0..skel.len().min(6) {
            let (mut lo, mut hi) = (glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN));
            for f in 0..frames {
                let m = hp3_anim::bones(&skel, Some((set, &map)), Some(seq), f as f32)[bone];
                let p = m.w_axis.truncate();
                lo = lo.min(p);
                hi = hi.max(p);
            }
            println!("  bone {bone} {:20} travels x {:.1} y {:.1} z {:.1}", skel.names[bone], hi.x - lo.x, hi.y - lo.y, hi.z - lo.z);
            if let Some(ab) = map[bone] {
                println!("    track: {}", set.describe_track(seq, ab));
            }
        }
    }
    if std::env::var_os("HP3_PROBE_SKIN").is_some() {
        for (i, b) in m.ref_skeleton.iter().enumerate() {
            println!("ref bone {i} {} parent {} children {} pos ({:.1},{:.1},{:.1}) orient ({:.3},{:.3},{:.3},{:.3})", skel.names[i], b.parent, b.num_children, b.position.x, b.position.y, b.position.z, b.orientation.x, b.orientation.y, b.orientation.z, b.orientation.w);
        }
        for bone in 0..skel.len() {
            let (mut lo, mut hi, mut n) = (glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN), 0);
            for inf in m.influences.iter().filter(|i| i.bone as usize == bone && i.weight > 0.5) {
                if let Some(p) = m.points.get(inf.vertex as usize) {
                    let v = glam::Vec3::new(p.x, p.y, p.z);
                    lo = lo.min(v);
                    hi = hi.max(v);
                    n += 1;
                }
            }
            println!("skin bone {bone} {}: {n} points lo ({:.1},{:.1},{:.1}) hi ({:.1},{:.1},{:.1})", skel.names[bone], lo.x, lo.y, lo.z, hi.x, hi.y, hi.z);
        }
    }
    // The mesh's vertical extent around its origin in actor space (reference pose).
    {
        let to_actor = hp3_anim::mesh_to_actor(m);
        let zs: Vec<f32> = m.points.iter().map(|p| to_actor.transform_point3(glam::Vec3::new(p.x, p.y, p.z)).z).collect();
        let (lo, hi) = zs.iter().fold((f32::MAX, f32::MIN), |(a, b), &z| (a.min(z), b.max(z)));
        println!("mesh {mesh}: reference pose z from {lo:.1} to {hi:.1} (origin at 0)");
        let h = &m.header;
        let raw: Vec<f32> = m.points.iter().map(|p| p.z).collect();
        let (rlo, rhi) = raw.iter().fold((f32::MAX, f32::MIN), |(a, b), &z| (a.min(z), b.max(z)));
        println!(
            "  header: mesh_origin ({:.1}, {:.1}, {:.1}) rot_origin {:?} scale ({:.2}, {:.2}, {:.2}); raw point z {rlo:.1}..{rhi:.1}",
            h.mesh_origin.x, h.mesh_origin.y, h.mesh_origin.z, h.rot_origin, h.mesh_scale.x, h.mesh_scale.y, h.mesh_scale.z
        );
    }
    let (points, label) = match &anim {
        Some(set_) => {
            let map = set_.bone_map(&skel);
            eprintln!("{} bones, {} mapped; {} sequences: {}", skel.len(), map.iter().flatten().count(), set_.sequences.len(),
                set_.sequences.iter().map(|s| format!("{}({})", s.name, s.frames)).collect::<Vec<_>>().join(" "));
            let si = seq.and_then(|s| set_.find(s));
            let skin = hp3_anim::pose(&skel, Some((set_, &map)), si, frame);
            (hp3_anim::skin_points(m, &skin), format!("{seq:?} frame {frame}"))
        }
        None => (hp3_anim::skin_points(m, &hp3_anim::pose(&skel, None, None, 0.0)), "reference".into()),
    };
    eprintln!("posing {label}");
    let geom = hp3_render::meshes::build_posed(&loader, &pkg, mi, &[], Some(&points)).ok_or("no geometry")?;
    if std::env::var_os("HP3_POSE_DEBUG").is_some() {
        for (i, b) in geom.batches.iter().enumerate() {
            eprintln!("batch {i}: {} triangles, texture {:?}", b.indices.len() / 3, b.texture);
        }
        eprintln!("mesh materials (flags, texture index): {:?}", m.header.materials);
        for (i, t) in m.header.textures.iter().enumerate() {
            let path = set.resolve(&pkg, *t).map(|(p, x)| format!("{} ({})", p.ref_path(hp3_package::ObjRef::from_export(x)), p.export_class_name(x)));
            eprintln!("material {i}: {path:?}");
        }
    }
    let renderer = Renderer::new_headless()?;
    let dynamic = Dynamic::new(&renderer);
    let images = |k: &hp3_assets::loader::ObjKey| loader.image(&set.get(&k.0).ok()?, k.1);
    let gpu = dynamic.upload(&renderer, &geom, &images);
    let gpu_scene = renderer.upload_scene(&scene::Scene::default());
    let camera = Camera { position: Vec3::new(180.0, 0.0, 10.0), rotation: [0, 32768, 0], fov_degrees: 70.0 };
    let (w, h) = (800, 800);
    let refs = [(&gpu, glam::Mat4::IDENTITY, glam::Vec3::splat(0.5))];
    let rgba = renderer.render_frame_to_rgba(w, h, |c, d| dynamic.draw(&renderer, &gpu_scene, &refs, &camera, c, d, 1.0));
    write_png(out, w, h, &rgba)
}

fn play(root: &Path, map: &str, out: &Path, ticks: u32) -> Result<(), Box<dyn std::error::Error>> {
    let set = PackageSet::open(root)?;
    let loader = Loader::new(&set);
    let mut pkg = set.get(map)?;
    let mut db = ClassDb::new(&set);
    db.load_all_classes();
    let mut vm = Vm::new(db);
    hp3_world::load(&mut vm, &pkg, hp3_core::Config::load(root))?;
    if let Ok(keys) = std::env::var("HP3_HOLD") {
        for k in keys.split(',') {
            hp3_world::world(&mut vm).input.key_down(k.trim());
        }
    }
    // HP3_TRIGGER=<tag> triggers the actors with that tag at tick HP3_TRIGGER_AT (default 60).
    let trigger_at: u32 = std::env::var("HP3_TRIGGER_AT").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
    for frame in 0..ticks {
        if frame == trigger_at
            && let Ok(tag) = std::env::var("HP3_TRIGGER")
        {
            let n = vm.db.names.intern(&tag);
            let pawn = hp3_world::world(&mut vm).player.and_then(|pc| hp3_world::get(&mut vm, pc, "Pawn").as_obj());
            let actors = hp3_world::world(&mut vm).actors.clone();
            for a in actors {
                if hp3_world::get(&mut vm, a, "Tag").as_name() == n {
                    hp3_world::fire(&mut vm, a, "Trigger", &mut [Value::Object(pawn), Value::Object(pawn)]);
                }
            }
        }
        // HP3_TRIGGERS=<tag>@<tick>,...: more tags to trigger, each at its own tick.
        if let Ok(spec) = std::env::var("HP3_TRIGGERS") {
            for item in spec.split(',') {
                if let Some((tag, tick)) = item.rsplit_once('@')
                    && tick.trim().parse::<u32>() == Ok(frame)
                {
                    let n = vm.db.names.intern(tag.trim());
                    let pawn = hp3_world::world(&mut vm).player.and_then(|pc| hp3_world::get(&mut vm, pc, "Pawn").as_obj());
                    let actors = hp3_world::world(&mut vm).actors.clone();
                    for a in actors {
                        if hp3_world::get(&mut vm, a, "Tag").as_name() == n {
                            hp3_world::fire(&mut vm, a, "Trigger", &mut [Value::Object(pawn), Value::Object(pawn)]);
                        }
                    }
                }
            }
        }
        hp3_world::tick(&mut vm, 1.0 / 60.0);
    }
    // HP3_SHOW_HUD=1 fades every HUD item in (they rest hidden until the game shows them).
    if std::env::var_os("HP3_SHOW_HUD").is_some()
        && let Some(pc) = hp3_world::world(&mut vm).player
        && let Some(hud) = hp3_world::get(&mut vm, pc, "myHUD").as_obj()
        && let Value::Array(items) = hp3_world::get(&mut vm, hud, "HudItems")
    {
        for it in items.iter().filter_map(Value::as_obj) {
            let _ = vm.event(it, "DoFadeIn", &mut []);
        }
        for _ in 0..30 {
            hp3_world::tick(&mut vm, 1.0 / 60.0);
        }
    }
    // HP3_CLICK=x,y[;x,y...] clicks the menus at those points (of the output size) before rendering.
    if let Ok(clicks) = std::env::var("HP3_CLICK") {
        for c in clicks.split(';') {
            let _ = hp3_world::canvas::render(&mut vm, 1280, 720);
            let v: Vec<f32> = c.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            if v.len() == 2 {
                hp3_world::gui::mouse_move(&mut vm, v[0], v[1]);
                hp3_world::gui::mouse_button(&mut vm, true);
                hp3_world::gui::mouse_button(&mut vm, false);
            }
            for _ in 0..30 {
                hp3_world::tick(&mut vm, 1.0 / 60.0);
                // A level switch the game asks for is followed; the new level then runs a while.
                if let Some(url) = hp3_world::take_travel(&mut vm) {
                    pkg = hp3_world::travel(&mut vm, &url)?;
                    eprintln!("travelled to {}", pkg.name);
                    for _ in 0..ticks {
                        hp3_world::tick(&mut vm, 1.0 / 60.0);
                    }
                }
            }
        }
    }
    let statics = hp3_engine::static_exports(&mut vm, &pkg);
    // HP3_UNLIT=1 draws everything at full brightness.
    let inputs = hp3_engine::map_inputs(&mut vm, &loader, &pkg, std::env::var_os("HP3_UNLIT").is_none());
    let scene = scene::build_map(&loader, &pkg, &|i| statics.contains(&i), &inputs);
    let lighting = inputs.lighting;
    let renderer = Renderer::new_headless()?;
    let dynamic = Dynamic::new(&renderer);
    let mut cache = hp3_engine::MeshCache::default();
    let instances = hp3_engine::actor_instances(&mut vm, &loader, &renderer, &dynamic, &mut cache, lighting.as_ref(), None, None);
    eprintln!("{} static batches, {} live actors drawn", scene.batches.len(), instances.len());
    // HP3_SOAK=<n>: keeps ticking and drawing for n more frames (HP3_HOLD=<key> held), to find per-frame leaks.
    if let Some(n) = std::env::var("HP3_SOAK").ok().and_then(|v| v.parse::<u32>().ok()) {
        if let Ok(key) = std::env::var("HP3_HOLD") {
            hp3_world::world(&mut vm).input.key_down(&key);
        }
        let before = cache.uploads;
        let mut slow: Vec<(f32, f32, u32)> = Vec::new();
        let mut soak_hud = hp3_engine::HudCache::default();
        for i in 0..n {
            let t = std::time::Instant::now();
            hp3_world::tick(&mut vm, 1.0 / 60.0);
            let tick_ms = t.elapsed().as_secs_f32() * 1000.0;
            let t = std::time::Instant::now();
            let _ = hp3_world::player_view(&mut vm);
            let view_ms = t.elapsed().as_secs_f32() * 1000.0;
            let t = std::time::Instant::now();
            let _ = hp3_engine::actor_instances(&mut vm, &loader, &renderer, &dynamic, &mut cache, lighting.as_ref(), None, None);
            let inst_ms = t.elapsed().as_secs_f32() * 1000.0;
            let t = std::time::Instant::now();
            let _ = hp3_engine::hud_quads(&mut vm, &loader, &mut soak_hud, 1280, 720);
            slow.push((tick_ms + view_ms + inst_ms, t.elapsed().as_secs_f32() * 1000.0, i));
        }
        slow.sort_by(|a, b| (b.0 + b.1).total_cmp(&(a.0 + a.1)));
        for (t, d, i) in slow.iter().take(5) {
            eprintln!("soak: slowest frame +{i}: world {t:.1} ms, hud {d:.1} ms");
        }
        eprintln!("soak: {} frames, {} mesh uploads (was {before})", n, cache.uploads - before);
    }
    let pc = hp3_world::world(&mut vm).player.ok_or("no player")?;
    // The pawn, or (in a cutscene, which takes the pawn from the player) the controller itself.
    let mut pawn = hp3_world::get(&mut vm, pc, "Pawn").as_obj().unwrap_or(pc);
    // HP3_VIEW_CLASS=<class>[:n] frames the n-th actor of that class instead of the player.
    if let Ok(spec) = std::env::var("HP3_VIEW_CLASS") {
        let (class, n) = spec.split_once(':').map(|(c, n)| (c.to_string(), n.parse().unwrap_or(0))).unwrap_or((spec.clone(), 0));
        let actors = hp3_world::world(&mut vm).actors.clone();
        let matching: Vec<_> = actors
            .into_iter()
            .filter(|&a| {
                vm.db.objects.get(a).class.is_some_and(|c| vm.db.names.get(vm.db.structs[c.0 as usize].name).eq_ignore_ascii_case(&class))
            })
            .collect();
        pawn = *matching.get(n).ok_or("no actor of that class")?;
        eprintln!("framing {spec} at {:?}", hp3_world::get(&mut vm, pawn, "Location"));
    }
    let p = hp3_world::physics::vec(&hp3_world::get(&mut vm, pawn, "Location"));
    let yaw = hp3_world::get(&mut vm, pawn, "Rotation").fields().get(1).map(Value::as_int).unwrap_or(0);
    let k = std::f32::consts::TAU / 65536.0;
    let back = Vec3::new((yaw as f32 * k).cos(), (yaw as f32 * k).sin(), 0.0);
    let mut camera = Camera { position: p - back * 160.0 + Vec3::Z * 50.0, rotation: [-1500, yaw, 0], fov_degrees: 90.0 };
    // The game's own camera, when it runs and no actor was asked for.
    if std::env::var_os("HP3_VIEW_CLASS").is_none() {
        if let Some((position, rotation, fov)) = hp3_world::player_view(&mut vm) {
            eprintln!("game camera at {position:?} rotation {rotation:?} fov {fov}");
            camera = Camera { position, rotation, fov_degrees: fov };
        }
    }
    // HP3_VIEW_CAM=x,y,z,pitch,yaw places the camera explicitly.
    if let Ok(c) = std::env::var("HP3_VIEW_CAM") {
        let v: Vec<f32> = c.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        if v.len() == 5 {
            camera = Camera { position: Vec3::new(v[0], v[1], v[2]), rotation: [v[3] as i32, v[4] as i32, 0], fov_degrees: 90.0 };
        }
    }
    let (w, h) = (1280, 720);
    let gpu_scene = renderer.upload_scene(&scene);
    let refs: Vec<_> = instances.iter().map(|(m, t, l)| (m.as_ref(), *t, *l)).collect();
    // The HUD over the scene (HP3_NO_HUD=1 leaves it out).
    let mut hud_cache = hp3_engine::HudCache::default();
    let quads = if std::env::var_os("HP3_NO_HUD").is_none() {
        hp3_engine::hud_quads(&mut vm, &loader, &mut hud_cache, w, h)
    } else {
        Vec::new()
    };
    let mut quads = quads;
    quads.extend(hp3_engine::screen_flash(&mut vm, w, h));
    eprintln!("{} HUD quads", quads.len());
    let overlay = hp3_render::overlay::Overlay::new(&renderer);
    let images = |k: &hp3_assets::loader::ObjKey| hud_cache.image(&loader, k);
    let particle_renderer = hp3_render::particles::ParticleRenderer::new(&renderer);
    let mut particle_cache = hp3_engine::ParticleCache::default();
    let particle_quads = hp3_engine::particle_quads(&mut vm, &loader, &mut particle_cache, &camera);
    eprintln!("{} particle quads ({} particles alive)", particle_quads.len(), hp3_world::particles::live(&mut vm));
    let particle_images = |k: &hp3_assets::loader::ObjKey| loader.image_with_alpha(&loader.set.get(&k.0).ok()?, k.1);
    let particle_draw = particle_renderer.prepare(&renderer, &particle_quads, &particle_images);
    let rgba = renderer.render_frames_to_rgba(w, h, |color, depth| {
        vec![
            dynamic.draw_full(
                &renderer,
                &gpu_scene,
                &refs,
                particle_draw.as_ref().map(|d| (&particle_renderer, d)),
                &camera,
                color,
                depth,
                w as f32 / h as f32,
            ),
            overlay.draw(&renderer, &quads, &images, color, w, h),
        ]
    });
    write_png(out, w, h, &rgba)
}
