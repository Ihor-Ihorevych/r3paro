//! `hp3 <HP3 folder> [map]`: plays a level in a window.
//!
//! Keys and mouse go to the game through its key bindings, and the view is the game's own camera
//! (`PlayerCalcView`). F1 switches to a free camera (WASD, mouse, Space/Ctrl, Shift) and back.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use glam::Vec3;
use hp3_assets::loader::Loader;
use hp3_package::PackageSet;
use hp3_render::gpu::Renderer;
use hp3_render::scene::{self, Camera};
use hp3_script::{ClassDb, Vm};
use sdl3::event::{Event, WindowEvent};
use sdl3::keyboard::{Keycode, Scancode};
use sdl3::mouse::MouseButton;

mod audio;
mod gamelog;
mod levelmenu;
use hp3_script::Value;

/// The ini's name for a key.
fn key_name(sc: Scancode) -> Option<String> {
    let name = sc.name();
    Some(match sc {
        Scancode::Space => "Space".into(),
        Scancode::Return => "Enter".into(),
        Scancode::Escape => "Escape".into(),
        Scancode::Tab => "Tab".into(),
        Scancode::Backspace => "Backspace".into(),
        Scancode::LShift | Scancode::RShift => "Shift".into(),
        Scancode::LCtrl | Scancode::RCtrl => "Ctrl".into(),
        Scancode::LAlt | Scancode::RAlt => "Alt".into(),
        Scancode::Up => "Up".into(),
        Scancode::Down => "Down".into(),
        Scancode::Left => "Left".into(),
        Scancode::Right => "Right".into(),
        Scancode::PageUp => "PageUp".into(),
        Scancode::PageDown => "PageDown".into(),
        Scancode::Home => "Home".into(),
        Scancode::End => "End".into(),
        Scancode::Insert => "Insert".into(),
        Scancode::Kp0 => "NumPad0".into(),
        Scancode::Kp1 => "NumPad1".into(),
        Scancode::Kp2 => "NumPad2".into(),
        Scancode::Kp3 => "NumPad3".into(),
        Scancode::Kp4 => "NumPad4".into(),
        Scancode::Kp5 => "NumPad5".into(),
        Scancode::Kp6 => "NumPad6".into(),
        Scancode::Kp7 => "NumPad7".into(),
        Scancode::Kp8 => "NumPad8".into(),
        Scancode::Kp9 => "NumPad9".into(),
        Scancode::Delete => "Delete".into(),
        _ if name.len() == 1 => name.to_ascii_uppercase(),
        _ if name.starts_with('F') && name[1..].parse::<u8>().is_ok() => name.to_string(),
        _ => return None,
    })
}

/// Unreal key codes of the keys menus use.
fn menu_key(sc: Scancode) -> Option<u8> {
    Some(match sc {
        Scancode::Return | Scancode::KpEnter => 0x0d,
        Scancode::Escape => 0x1b,
        Scancode::Space => 0x20,
        Scancode::Left => 0x25,
        Scancode::Up => 0x26,
        Scancode::Right => 0x27,
        Scancode::Down => 0x28,
        Scancode::Tab => 0x09,
        Scancode::Backspace => 0x08,
        _ => return None,
    })
}

fn mouse_name(b: MouseButton) -> Option<&'static str> {
    match b {
        MouseButton::Left => Some("LeftMouse"),
        MouseButton::Right => Some("RightMouse"),
        MouseButton::Middle => Some("MiddleMouse"),
        _ => None,
    }
}

/// The game's view: the player controller's `PlayerCalcView`.
fn game_camera(vm: &mut Vm<'_>) -> Option<Camera> {
    if let Some((position, rotation, fov)) = hp3_world::player_view(vm) {
        return Some(Camera { position, rotation, fov_degrees: fov });
    }
    // The game's camera is not running: follow the pawn from behind instead.
    let pc = hp3_world::world(vm).player?;
    let pawn = hp3_world::get(vm, pc, "Pawn").as_obj()?;
    let p = hp3_world::physics::vec(&hp3_world::get(vm, pawn, "Location"));
    let r = hp3_world::get(vm, pc, "Rotation");
    let r: Vec<i32> = r.fields().iter().map(Value::as_int).collect();
    let yaw = r.get(1).copied().unwrap_or(0);
    let k = std::f32::consts::TAU / 65536.0;
    let back = Vec3::new((yaw as f32 * k).cos(), (yaw as f32 * k).sin(), 0.0);
    Some(Camera { position: p - back * 220.0 + Vec3::Z * 90.0, rotation: [-2500, yaw, 0], fov_degrees: 90.0 })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(root) = args.first().map(PathBuf::from) else {
        eprintln!("usage: hp3 <HP3 folder> [map]");
        return ExitCode::from(2);
    };
    match run(&root, args.get(1).map(String::as_str)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(root: &Path, map: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let config = hp3_core::Config::load(root);
    let map = map
        .map(str::to_string)
        .or_else(|| config.get("URL", "LocalMap").map(|m| m.trim_end_matches(".unr").to_string()))
        .unwrap_or_else(|| "HP3_Adv1Express".to_string());
    let set = PackageSet::open(root)?;
    let pkg = set.get(&map)?;

    let loader = Loader::new(&set);
    let mut db = ClassDb::new(&set);
    db.load_all_classes();
    let mut vm = Vm::new(db);
    let t = Instant::now();
    hp3_world::load(&mut vm, &pkg, config)?;
    eprintln!("world loaded in {:?}", t.elapsed());
    // Static actors are baked into the level's geometry; everything else is drawn per frame.
    let statics = hp3_engine::static_exports(&mut vm, &pkg);
    let t = Instant::now();
    // HP3_UNLIT=1 draws everything at full brightness.
    let inputs = hp3_engine::map_inputs(&mut vm, &loader, &pkg, std::env::var_os("HP3_UNLIT").is_none());
    let scene = scene::build_map(&loader, &pkg, &|i| statics.contains(&i), &inputs);
    let mut lighting = inputs.lighting;
    eprintln!("{map}: scene built in {:?} ({:?})", t.elapsed(), scene.stats);

    let sdl = sdl3::init()?;
    let video = sdl.video()?;
    // No audio device is not fatal: the game runs silent.
    let mut audio = if std::env::var_os("HP3_NO_AUDIO").is_none() { audio::AudioSystem::new(&sdl, &root) } else { None };
    if audio.is_none() {
        eprintln!("audio: off");
    }
    let mut window = video.window(&format!("hp3 - {map}"), 1280, 720).resizable().position_centered().build()?;

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    // SAFETY: the window outlives the surface; both live until the end of this function.
    let surface = unsafe { instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::from_window(&window)?)? };
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: Some(&surface),
        force_fallback_adapter: false,
    }))?;
    let caps = surface.get_capabilities(&adapter);
    let format = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(caps.formats[0]);
    let renderer = Renderer::with_adapter(&adapter, format)?;
    eprintln!("GPU: {}", renderer.adapter_info);
    let mut gpu_scene = renderer.upload_scene(&scene);
    let dynamic = hp3_render::dynamic::Dynamic::new(&renderer);
    let mut mesh_cache = hp3_engine::MeshCache::default();
    let overlay = hp3_render::overlay::Overlay::new(&renderer);
    let mut hud_cache = hp3_engine::HudCache::default();

    let (mut width, mut height) = window.size_in_pixels();
    let configure = |w: u32, h: u32| {
        surface.configure(
            &renderer.device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                width: w.max(1),
                height: h.max(1),
                present_mode: wgpu::PresentMode::AutoVsync,
                desired_maximum_frame_latency: 2,
                alpha_mode: caps.alpha_modes[0],
                view_formats: vec![],
            },
        );
    };
    configure(width, height);
    let mut depth = renderer.depth_target(width.max(1), height.max(1));

    let (start, start_rot) = scene.player_starts.first().copied().unwrap_or((Vec3::ZERO, [0, 0, 0]));
    let mut camera = Camera { position: start + Vec3::Z * 50.0, rotation: [start_rot[0], start_rot[1], 0], fov_degrees: 90.0 };
    sdl.mouse().set_relative_mouse_mode(&window, true);
    let mut events = sdl.event_pump()?;
    let mut last = Instant::now();
    let mut accumulator = 0.0f32;
    let fixed_step = std::env::var_os("HP3_FIXED_STEP").is_some();
    let mut frames = 0u32;
    let mut fps_timer = Instant::now();
    let mut free_camera = false;
    let mut in_menu = false;
    // F11 level list; G ghost, F fly, H walk (debug movement).
    let mut levels = levelmenu::LevelMenu::new(root);
    let mut current_map = map.clone();
    // HP3_OPEN_LEVELS=1 starts with the list open (for screenshots).
    if std::env::var_os("HP3_OPEN_LEVELS").is_some() {
        levels.toggle(&current_map);
    }
    let mut pending_level: Option<String> = None;
    let mut game_log = gamelog::GameLog::new(root);
    let particle_renderer = hp3_render::particles::ParticleRenderer::new(&renderer);
    let mut particle_cache = hp3_engine::ParticleCache::default();
    'main: loop {
        // Menus take the mouse as a visible cursor; play takes it relative.
        let menu = hp3_world::gui::menu_open(&mut vm);
        if menu != in_menu {
            in_menu = menu;
            sdl.mouse().set_relative_mouse_mode(&window, !menu);
        }
        for event in events.poll_iter() {
            match event {
                Event::Quit { .. } => break 'main,
                Event::KeyDown { scancode: Some(Scancode::F11), repeat: false, .. } => levels.toggle(&current_map),
                // While the level list is open it takes the keyboard and the mouse.
                Event::KeyDown { scancode: Some(sc), .. } if levels.open => match sc {
                    Scancode::Up => levels.step(-1),
                    Scancode::Down => levels.step(1),
                    Scancode::PageUp => levels.step(-10),
                    Scancode::PageDown => levels.step(10),
                    Scancode::Home => levels.home(),
                    Scancode::End => levels.end(),
                    Scancode::Return | Scancode::KpEnter => pending_level = levels.choose(),
                    Scancode::Escape => levels.open = false,
                    _ => {}
                },
                Event::MouseMotion { .. } | Event::MouseButtonDown { .. } if levels.open => {}
                Event::KeyDown { scancode: Some(sc @ (Scancode::G | Scancode::F | Scancode::H)), repeat: false, .. } if !in_menu => {
                    let mode = match sc {
                        Scancode::G => hp3_world::debug_move::Mode::Ghost,
                        Scancode::F => hp3_world::debug_move::Mode::Fly,
                        _ => hp3_world::debug_move::Mode::Walk,
                    };
                    eprintln!("movement: {mode:?} ({})", if hp3_world::debug_move::set_mode(&mut vm, mode) { "ok" } else { "no pawn" });
                }
                Event::KeyDown { scancode: Some(sc), repeat, .. } if in_menu => {
                    if let Some(k) = menu_key(sc) {
                        hp3_world::gui::key(&mut vm, k, if repeat { 2 } else { 1 }, 0.0);
                    }
                }
                Event::KeyUp { scancode: Some(sc), .. } if in_menu => {
                    if let Some(k) = menu_key(sc) {
                        hp3_world::gui::key(&mut vm, k, 3, 0.0);
                    }
                }
                Event::MouseMotion { x, y, .. } if in_menu => {
                    // Window coordinates to the render target's pixels.
                    let (ww, _) = window.size();
                    let scale = width as f32 / ww.max(1) as f32;
                    hp3_world::gui::mouse_move(&mut vm, x * scale, y * scale);
                }
                Event::MouseButtonDown { mouse_btn: MouseButton::Left, .. } if in_menu => hp3_world::gui::mouse_button(&mut vm, true),
                Event::MouseButtonUp { mouse_btn: MouseButton::Left, .. } if in_menu => hp3_world::gui::mouse_button(&mut vm, false),
                // Escape opens the game's menu; Shift+Escape leaves the program.
                Event::KeyDown { keycode: Some(Keycode::Escape), keymod, .. }
                    if keymod.intersects(sdl3::keyboard::Mod::LSHIFTMOD | sdl3::keyboard::Mod::RSHIFTMOD) =>
                {
                    break 'main
                }
                Event::MouseWheel { y, .. } if !free_camera && y != 0.0 => {
                    // A wheel notch is a key pressed and let go at once (it cycles the spells).
                    let key = if y > 0.0 { "MouseWheelUp" } else { "MouseWheelDown" };
                    hp3_world::world(&mut vm).input.key_down(key);
                    hp3_world::world(&mut vm).input.key_up(key);
                }
                Event::KeyDown { scancode: Some(Scancode::F1), repeat: false, .. } => free_camera = !free_camera,
                Event::KeyDown { scancode: Some(sc), repeat: false, .. } if !free_camera => {
                    if let Some(k) = key_name(sc) {
                        hp3_world::world(&mut vm).input.key_down(&k);
                    }
                }
                Event::KeyUp { scancode: Some(sc), .. } => {
                    if let Some(k) = key_name(sc) {
                        hp3_world::world(&mut vm).input.key_up(&k);
                    }
                }
                Event::MouseButtonDown { mouse_btn, .. } if !free_camera => {
                    if let Some(k) = mouse_name(mouse_btn) {
                        hp3_world::world(&mut vm).input.key_down(k);
                    }
                }
                Event::MouseButtonUp { mouse_btn, .. } => {
                    if let Some(k) = mouse_name(mouse_btn) {
                        hp3_world::world(&mut vm).input.key_up(k);
                    }
                }
                Event::Window { win_event: WindowEvent::PixelSizeChanged(w, h), .. } => {
                    (width, height) = (w.max(1) as u32, h.max(1) as u32);
                    configure(width, height);
                    depth = renderer.depth_target(width, height);
                }
                Event::MouseMotion { xrel, yrel, .. } => {
                    if free_camera {
                        camera.rotation[1] = camera.rotation[1].wrapping_add((xrel * 40.0) as i32);
                        camera.rotation[0] = (camera.rotation[0] - (yrel * 40.0) as i32).clamp(-16000, 16000);
                    } else {
                        hp3_world::world(&mut vm).input.mouse_move(xrel, yrel);
                    }
                }
                _ => {}
            }
        }
        let now = Instant::now();
        let dt = (now - last).as_secs_f32().min(0.1);
        last = now;

        let keys = events.keyboard_state();
        let free = free_camera;
        let speed = if keys.is_scancode_pressed(Scancode::LShift) { 1600.0 } else { 400.0 } * dt;
        let fwd = camera.forward();
        let right = Vec3::new(-fwd.y, fwd.x, 0.0).normalize_or_zero();
        let mut mv = Vec3::ZERO;
        if keys.is_scancode_pressed(Scancode::W) {
            mv += fwd;
        }
        if keys.is_scancode_pressed(Scancode::S) {
            mv -= fwd;
        }
        if keys.is_scancode_pressed(Scancode::D) {
            mv += right;
        }
        if keys.is_scancode_pressed(Scancode::A) {
            mv -= right;
        }
        if keys.is_scancode_pressed(Scancode::Space) {
            mv += Vec3::Z;
        }
        if keys.is_scancode_pressed(Scancode::LCtrl) {
            mv -= Vec3::Z;
        }
        if free {
            camera.position += mv * speed;
        }

        let frame_start = Instant::now();
        let mut stages: Vec<(&str, f32)> = Vec::new();
        // One world tick per drawn frame, with the real time since the last one (at most a thirtieth of a second,
        // so a stall does not make the world jump). HP3_FIXED_STEP=1 keeps a fixed 60 Hz instead.
        if fixed_step {
            accumulator += dt;
            while accumulator >= 1.0 / 60.0 {
                hp3_world::tick(&mut vm, 1.0 / 60.0);
                accumulator -= 1.0 / 60.0;
            }
        } else {
            hp3_world::tick(&mut vm, dt.min(1.0 / 30.0));
        }
        stages.push(("ticks", frame_start.elapsed().as_secs_f32() * 1000.0));
        // A level switch the game asked for: the new level is loaded and its scene built.
        if let Some(url) = hp3_world::take_travel(&mut vm).or_else(|| pending_level.take()) {
            let t = Instant::now();
            let pkg = hp3_world::travel(&mut vm, &url)?;
            let statics = hp3_engine::static_exports(&mut vm, &pkg);
            let inputs = hp3_engine::map_inputs(&mut vm, &loader, &pkg, std::env::var_os("HP3_UNLIT").is_none());
            let scene = scene::build_map(&loader, &pkg, &|i| statics.contains(&i), &inputs);
            gpu_scene = renderer.upload_scene(&scene);
            lighting = inputs.lighting;
            mesh_cache = hp3_engine::MeshCache::default();
            particle_cache = hp3_engine::ParticleCache::default();
            current_map = pkg.name.to_string();
            let _ = window.set_title(&format!("hp3 - {}", pkg.name));
            eprintln!("travelled to {} in {:?}", pkg.name, t.elapsed());
            accumulator = 0.0;
            last = Instant::now();
            if let Some(a) = &mut audio {
                a.stop_all();
            }
        }

        if !free_camera && let Some(c) = game_camera(&mut vm) {
            camera = c;
        }
        if let Some(a) = &mut audio {
            a.update(&mut vm, camera.position, camera.rotation[1]);
        }
        game_log.update(&mut vm);
        stages.push(("camera", frame_start.elapsed().as_secs_f32() * 1000.0));
        let frame = match surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                configure(width, height);
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        let view = frame.texture.create_view(&Default::default());
        stages.push(("acquire", frame_start.elapsed().as_secs_f32() * 1000.0));
        let instances = hp3_engine::actor_instances(&mut vm, &loader, &renderer, &dynamic, &mut mesh_cache, lighting.as_ref());
        let refs: Vec<_> = instances.iter().map(|(m, t, l)| (m.as_ref(), *t, *l)).collect();
        stages.push(("instances", frame_start.elapsed().as_secs_f32() * 1000.0));
        let particle_quads = hp3_engine::particle_quads(&mut vm, &loader, &mut particle_cache, &camera);
        let particle_images = |k: &hp3_assets::loader::ObjKey| loader.image_with_alpha(&loader.set.get(&k.0).ok()?, k.1);
        let particle_draw = particle_renderer.prepare(&renderer, &particle_quads, &particle_images);
        let cmd = dynamic.draw_full(
            &renderer,
            &gpu_scene,
            &refs,
            particle_draw.as_ref().map(|d| (&particle_renderer, d)),
            &camera,
            &view,
            &depth,
            width as f32 / height as f32,
        );
        stages.push(("draw", frame_start.elapsed().as_secs_f32() * 1000.0));
        let mut quads = hp3_engine::hud_quads(&mut vm, &loader, &mut hud_cache, width, height);
        quads.extend(hp3_engine::screen_flash(&mut vm, width, height));
        quads.extend(levels.quads(width, height, &current_map));
        let images = |k: &hp3_assets::loader::ObjKey| hud_cache.image(&loader, k);
        let hud = overlay.draw(&renderer, &quads, &images, &view, width, height);
        stages.push(("hud", frame_start.elapsed().as_secs_f32() * 1000.0));
        renderer.queue.submit([cmd, hud]);
        stages.push(("submit", frame_start.elapsed().as_secs_f32() * 1000.0));
        frame.present();
        stages.push(("present", frame_start.elapsed().as_secs_f32() * 1000.0));
        if stages.last().is_some_and(|s| s.1 > 200.0) {
            eprintln!("slow frame (cumulative ms): {stages:?}");
        }

        frames += 1;
        if fps_timer.elapsed().as_secs_f32() >= 1.0 {
            let fps = frames as f32 / fps_timer.elapsed().as_secs_f32();
            let _ = window.set_title(&format!("hp3 - {current_map} - {fps:.0} fps"));
            frames = 0;
            fps_timer = Instant::now();
        }
    }
    Ok(())
}
