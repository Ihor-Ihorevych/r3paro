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
        Scancode::Delete => "Delete".into(),
        _ if name.len() == 1 => name.to_ascii_uppercase(),
        _ if name.starts_with('F') && name[1..].parse::<u8>().is_ok() => name.to_string(),
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
    let pc = hp3_world::world(vm).player?;
    let mut args = [Value::Object(None), Value::Empty, Value::Empty];
    vm.event(pc, "PlayerCalcView", &mut args).ok()?;
    let loc = args[1].fields();
    let rot = args[2].fields();
    let at_origin = loc.iter().all(|v| v.as_float() == 0.0);
    if loc.len() != 3 || rot.len() != 3 || at_origin {
        // The game's camera is not running (a map loaded directly skips the flow that starts it): follow the
        // pawn from behind instead.
        let pawn = hp3_world::get(vm, pc, "Pawn").as_obj()?;
        let p = hp3_world::physics::vec(&hp3_world::get(vm, pawn, "Location"));
        let r = hp3_world::get(vm, pc, "Rotation");
        let r: Vec<i32> = r.fields().iter().map(Value::as_int).collect();
        let yaw = r.get(1).copied().unwrap_or(0);
        let k = std::f32::consts::TAU / 65536.0;
        let back = Vec3::new((yaw as f32 * k).cos(), (yaw as f32 * k).sin(), 0.0);
        return Some(Camera { position: p - back * 220.0 + Vec3::Z * 90.0, rotation: [-2500, yaw, 0], fov_degrees: 90.0 });
    }
    Some(Camera {
        position: Vec3::new(loc[0].as_float(), loc[1].as_float(), loc[2].as_float()),
        rotation: [rot[0].as_int(), rot[1].as_int(), rot[2].as_int()],
        fov_degrees: 90.0,
    })
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
    let t = Instant::now();
    let scene = scene::build(&loader, &pkg);
    eprintln!("{map}: scene built in {:?} ({:?})", t.elapsed(), scene.stats);

    let mut db = ClassDb::new(&set);
    db.load_all_classes();
    let mut vm = Vm::new(db);
    let t = Instant::now();
    hp3_world::load(&mut vm, &pkg, config)?;
    eprintln!("world loaded in {:?}", t.elapsed());

    let sdl = sdl3::init()?;
    let video = sdl.video()?;
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
    let gpu_scene = renderer.upload_scene(&scene);

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
    let mut frames = 0u32;
    let mut fps_timer = Instant::now();
    let mut free_camera = false;
    'main: loop {
        for event in events.poll_iter() {
            match event {
                Event::Quit { .. } | Event::KeyDown { keycode: Some(Keycode::Escape), .. } => break 'main,
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

        // The level's scripts run at a fixed 60 Hz.
        accumulator += dt;
        while accumulator >= 1.0 / 60.0 {
            hp3_world::tick(&mut vm, 1.0 / 60.0);
            accumulator -= 1.0 / 60.0;
        }

        if !free_camera && let Some(c) = game_camera(&mut vm) {
            camera = c;
        }
        let frame = match surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                configure(width, height);
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        let view = frame.texture.create_view(&Default::default());
        let cmd = renderer.draw(&gpu_scene, &camera, &view, &depth, width as f32 / height as f32);
        renderer.queue.submit([cmd]);
        frame.present();

        frames += 1;
        if fps_timer.elapsed().as_secs_f32() >= 1.0 {
            let fps = frames as f32 / fps_timer.elapsed().as_secs_f32();
            let _ = window.set_title(&format!("hp3 - {map} - {fps:.0} fps"));
            frames = 0;
            fps_timer = Instant::now();
        }
    }
    Ok(())
}
