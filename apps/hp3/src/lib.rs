//! The game runner: `hp3 <HP3 folder> [map]` plays a level in a window. The desktop binary and the Android library both
//! start `run`.
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

#[cfg(target_os = "android")]
pub mod android;
mod audio;
pub mod commands;
mod gamelog;
mod levelmenu;
mod present;
use hp3_script::Value;

/// What the platform layer needs to know about the game and tells it: on Android Flipendo's touch controls ask through JNI
/// (`android`), elsewhere nothing happens.
#[cfg(target_os = "android")]
use android as platform;

#[cfg(not(target_os = "android"))]
mod platform {
    pub fn set_in_menu(_open: bool) {}
    pub fn set_cutscene(_on: bool) {}
    pub fn set_creature(_on: bool) {}
    pub fn set_level(_name: &str) {}
    pub fn surface_alive() -> bool {
        true
    }
    pub fn tick_frame() {}
    /// The desktop game runs at 60 frames a second; `HP3_FPS=<n>` changes it and 0 lifts the limit.
    pub fn frame_budget() -> Option<f32> {
        let fps = std::env::var("HP3_FPS").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(60.0);
        (fps > 0.0).then(|| 1.0 / fps)
    }
    /// Degrees added to the game's field of view: 20 by default (as on the phone), `HP3_FOV_OFFSET=<degrees>` changes it.
    pub fn fov_offset() -> f32 {
        std::env::var("HP3_FOV_OFFSET").ok().and_then(|v| v.parse().ok()).unwrap_or(20.0)
    }
    pub fn in_cutscene() -> bool {
        false
    }
    pub fn first_person() -> bool {
        std::env::var_os("HP3_FIRST_PERSON").is_some()
    }
    pub fn ui_scale() -> f32 {
        1.0
    }
    pub fn note_saved() {}
}

/// The ini's name for a key.
fn key_name(sc: Scancode) -> Option<String> {
    let name = sc.name();
    Some(match sc {
        Scancode::Space => "Space".into(),
        Scancode::Return | Scancode::KpEnter => "Enter".into(),
        Scancode::Escape => "Escape".into(),
        Scancode::Tab => "Tab".into(),
        Scancode::Backspace => "Backspace".into(),
        Scancode::LShift | Scancode::RShift => "Shift".into(),
        Scancode::LCtrl | Scancode::RCtrl => "Ctrl".into(),
        Scancode::LAlt | Scancode::RAlt => "Alt".into(),
        Scancode::Up if cfg!(target_os = "android") && !FLYING.load(std::sync::atomic::Ordering::Relaxed) => "W".into(),
        Scancode::Down if cfg!(target_os = "android") && !FLYING.load(std::sync::atomic::Ordering::Relaxed) => "S".into(),
        Scancode::Up => "Up".into(),
        Scancode::Down => "Down".into(),
        // On the phone the movement stick sends the arrow keys: they act as W, A, S and D, so forward and sideways go
        // together (the arrow keys' own forward is a separate axis that does not mix with strafing).
        Scancode::Left if cfg!(target_os = "android") && !FLYING.load(std::sync::atomic::Ordering::Relaxed) => "A".into(),
        Scancode::Right if cfg!(target_os = "android") && !FLYING.load(std::sync::atomic::Ordering::Relaxed) => "D".into(),
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
        Scancode::Escape | Scancode::AcBack => 0x1b,
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

/// How far (in units) the camera is moved back from the view the game computes while the player is in control.
const CAMERA_PULL_BACK: f32 = 24.0;
/// How far in front of the head's centre the first-person camera sits, so it is outside the head's mesh.
const FIRST_PERSON_AHEAD: f32 = 12.0;
/// How high above the pawn's location the camera's line to the player starts, and how far (Unreal units) the camera keeps
/// from a wall; the near plane is 4, so this is enough for it never to see through.
const CAMERA_HEAD_HEIGHT: f32 = 40.0;
const CAMERA_CLEARANCE: f32 = 10.0;

/// The game's view: the player controller's `PlayerCalcView`.
/// Per-second frame timing: for each stage of a frame, its average and its worst time over the second.
#[derive(Default)]
struct PerfStats {
    names: Vec<&'static str>,
    sum: Vec<f32>,
    worst: Vec<f32>,
    frames: u32,
    slowest_frame: f32,
}

impl PerfStats {
    /// A frame's stages (name, cumulative ms) and its total time.
    fn add(&mut self, stages: &[(&'static str, f32)], total: f32) {
        if self.names.is_empty() {
            self.names = stages.iter().map(|s| s.0).collect();
            self.sum = vec![0.0; stages.len()];
            self.worst = vec![0.0; stages.len()];
        }
        let mut before = 0.0;
        for (i, (_, at)) in stages.iter().enumerate().take(self.sum.len()) {
            let took = at - before;
            before = *at;
            self.sum[i] += took;
            self.worst[i] = self.worst[i].max(took);
        }
        self.frames += 1;
        self.slowest_frame = self.slowest_frame.max(total);
    }

    fn report(&self) -> String {
        let n = self.frames.max(1) as f32;
        let stages: Vec<String> = (0..self.sum.len()).map(|i| format!("{} {:.1}/{:.0}", self.names[i], self.sum[i] / n, self.worst[i])).collect();
        format!("slowest {:.0} ms; stage avg/worst ms: {}", self.slowest_frame, stages.join(" "))
    }
}

/// True while the player rides the hippogriff: the stick then steers (turns and banks) instead of walking and strafing.
static FLYING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// True while the player controls a creature (the rabbit, the dragon): on the phone the jump button then also ends it,
/// as the Enter key does on the desktop.
static CREATURE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn player_is_creature(vm: &mut Vm<'_>) -> bool {
    let Some(pc) = hp3_world::world(vm).player else { return false };
    hp3_world::get(vm, pc, "Pawn").as_obj().is_some_and(|p| !hp3_world::get(vm, p, "bIsMainPlayer").as_bool())
}

/// Whether the key is the one the phone's jump button sends.
fn is_jump_key(name: &str) -> bool {
    name == "Space" || name == "RightMouse"
}

fn player_flying(vm: &mut Vm<'_>) -> bool {
    let Some(pc) = hp3_world::world(vm).player else { return false };
    vm.state_of(pc).is_some_and(|st| vm.db.names.get(vm.db.get(st).name).starts_with("PlayerFlapping"))
}

/// Whether the player is in a cutscene: something other than the player's own controller holds the pawn (a cutscene's
/// controller), or the player has no pawn at all while no menu is open (the opening of a new game is played that way).
fn player_in_cutscene(vm: &mut Vm<'_>, menu_open: bool) -> bool {
    let Some(pc) = hp3_world::world(vm).player else { return false };
    let Some(pawn) = hp3_world::get(vm, pc, "Pawn").as_obj() else { return !menu_open };
    hp3_world::get(vm, pawn, "Controller").as_obj().is_some_and(|c| c != pc)
}

/// Whether the player steers a creature the game gave them (the rabbit of Lapifors, the dragon of Draconifors...): Enter
/// ends it (`Enter=Use | BypassCutscene | EndCreaturePossess`), which a phone has no key for.
fn player_controls_creature(vm: &mut Vm<'_>) -> bool {
    let Some(pc) = hp3_world::world(vm).player else { return false };
    let Some(class) = hp3_world::get(vm, pc, "Pawn").as_obj().and_then(|p| vm.db.objects.get(p).class) else { return false };
    ["LapiforPawn", "AviforPawn", "SuperChickenPawn"].iter().filter_map(|n| vm.db.find(n)).any(|c| vm.is_child_of(class, c))
}

fn game_camera(vm: &mut Vm<'_>) -> Option<Camera> {
    if let Some((mut position, rotation, fov)) = hp3_world::player_view(vm) {
        if let Some(pawn) = hp3_world::world(vm).player.and_then(|pc| hp3_world::get(vm, pc, "Pawn").as_obj()) {
            position.z += hp3_world::physics::view_offset(vm, pawn);
        }
        // In play (not in a cutscene) the camera stands a little further back from the character.
        let playing = hp3_world::world(vm).player.is_some_and(|pc| vm.state_of(pc).is_some_and(|st| vm.db.names.get(vm.db.get(st).name).starts_with("Player")));
        if playing {
            let k = std::f32::consts::TAU / 65536.0;
            let (pitch, yaw) = (rotation[0] as f32 * k, rotation[1] as f32 * k);
            let forward = Vec3::new(pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), pitch.sin());
            position -= forward * CAMERA_PULL_BACK;
            if platform::first_person()
                && let Some(pawn) = hp3_world::world(vm).player.and_then(|pc| hp3_world::get(vm, pc, "Pawn").as_obj())
            {
                // First person: the camera is in the head, a little ahead of it, looking where the game's camera looks.
                let eye = match hp3_world::get(vm, pawn, "BaseEyeHeight").as_float() {
                    h if h > 1.0 => h,
                    _ => hp3_world::get(vm, pawn, "CollisionHeight").as_float() * 0.8,
                };
                let head = hp3_world::physics::vec(&hp3_world::get(vm, pawn, "Location")) + Vec3::Z * eye;
                return Some(Camera { position: head + forward * FIRST_PERSON_AHEAD, rotation, fov_degrees: fov + platform::fov_offset() });
            }
            // The camera stays on the player's side of a wall: it comes in along the line from the head to where it
            // would be, as far as the level's geometry lets it.
            if let Some(pawn) = hp3_world::world(vm).player.and_then(|pc| hp3_world::get(vm, pc, "Pawn").as_obj()) {
                let head = hp3_world::physics::vec(&hp3_world::get(vm, pawn, "Location")) + Vec3::Z * CAMERA_HEAD_HEIGHT;
                if let Some(hit) = hp3_world::world(vm).collision.trace_static(head, position, Vec3::splat(CAMERA_CLEARANCE)) {
                    position = head + (position - head) * hit.time;
                }
            }
        }
        return Some(Camera { position, rotation, fov_degrees: fov + platform::fov_offset() });
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

/// The desktop entry: the folder and map come from the command line.
pub fn desktop_main() -> ExitCode {
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

pub fn run(root: &Path, map: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let config = hp3_core::Config::load(root);
    let map = map
        .map(str::to_string)
        .or_else(|| config.get("URL", "LocalMap").map(|m| m.trim_end_matches(".unr").to_string()))
        .unwrap_or_else(|| "HP3_Adv1Express".to_string());
    let set = PackageSet::open(root)?;
    eprintln!("language: texts {}, voices {}", config.language, set.language);
    let pkg = set.get(&map)?;

    let loader = Loader::new(&set);
    let mut db = ClassDb::new(&set);
    db.load_all_classes();
    let mut vm = Vm::new(db);
    vm.trace_states = std::env::var_os("HP3_TRACE_STATES").is_some();
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

    // On a phone the game is played with the screen on its side, whichever way it is held.
    #[cfg(target_os = "android")]
    sdl3::hint::set("SDL_ORIENTATIONS", "LandscapeLeft LandscapeRight");
    let sdl = sdl3::init()?;
    let video = sdl.video()?;
    // No audio device is not fatal: the game runs silent.
    let mut settings_seen = u32::MAX;
    let mut audio = if std::env::var_os("HP3_NO_AUDIO").is_none() { audio::AudioSystem::new(&sdl, &root) } else { None };
    if audio.is_none() {
        eprintln!("audio: off");
    }
    let mut builder = video.window(&format!("hp3 - {map}"), 1280, 720);
    builder.resizable().position_centered();
    // On the phone the game fills the whole screen at the display's own size and shape, not a 16:9 window.
    if cfg!(target_os = "android") {
        builder.fullscreen().high_pixel_density();
    }
    let mut window = builder.build()?;

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
    eprintln!("window: {width}x{height} pixels, {:?} points", window.size());
    // Frames are drawn off-screen and shown by a thread of their own, so waiting for the display does not hold up the game.
    let mut presenter = present::Presenter::new(surface, &caps, renderer.device.clone(), renderer.queue.clone(), format, width, height)
        .ok_or("the display surface cannot be copied into")?;
    let mut depth = renderer.depth_target(width.max(1), height.max(1));

    let (start, start_rot) = scene.player_starts.first().copied().unwrap_or((Vec3::ZERO, [0, 0, 0]));
    let mut camera = Camera { position: start + Vec3::Z * 50.0, rotation: [start_rot[0], start_rot[1], 0], fov_degrees: 90.0 };
    sdl.mouse().set_relative_mouse_mode(&window, true);
    let mut events = sdl.event_pump()?;
    let mut last = Instant::now();
    let mut accumulator = 0.0f32;
    let fixed_step = std::env::var_os("HP3_FIXED_STEP").is_some();
    let mut frames = 0u32;
    let mut perf = PerfStats::default();
    let mut fps_timer = Instant::now();
    let mut free_camera = false;
    let mut in_menu = false;
    // F11 level list; G ghost, F fly, H walk (debug movement).
    let mut levels = levelmenu::LevelMenu::new(root);
    let mut current_map = map.clone();
    platform::set_level(&current_map);
    // HP3_OPEN_LEVELS=1 starts with the list open (for screenshots).
    if std::env::var_os("HP3_OPEN_LEVELS").is_some() {
        levels.toggle(&current_map);
    }
    let mut pending_level: Option<String> = None;
    let mut surface_was_gone = false;
    let mut game_log = gamelog::GameLog::new(root);
    let particle_renderer = hp3_render::particles::ParticleRenderer::new(&renderer);
    let mut particle_cache = hp3_engine::ParticleCache::default();
    'main: loop {
        // Menus take the mouse as a visible cursor; play takes it relative.
        let menu = hp3_world::gui::menu_open(&mut vm);
        if menu != in_menu {
            in_menu = menu;
            platform::set_in_menu(menu);
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
                Event::KeyDown { scancode: Some(Scancode::F5), repeat: false, .. } if !in_menu => commands::push("quicksave"),
                Event::KeyDown { scancode: Some(Scancode::F9), repeat: false, .. } if !in_menu => commands::push("quickload"),
                Event::KeyDown { scancode: Some(Scancode::F1), repeat: false, .. } => free_camera = !free_camera,
                Event::KeyDown { scancode: Some(sc), repeat: false, .. } if !free_camera => {
                    if let Some(k) = key_name(sc) {
                        if cfg!(target_os = "android") && std::path::Path::new("/sdcard/FlipendoHP3/keys.flag").exists() {
                            eprintln!("key down {sc:?} -> {k}");
                        }
                        hp3_world::world(&mut vm).input.key_down(&k);
                        if cfg!(target_os = "android") && is_jump_key(&k) && CREATURE.load(std::sync::atomic::Ordering::Relaxed) {
                            hp3_world::world(&mut vm).input.key_down("Enter");
                        }
                    }
                }
                Event::KeyUp { scancode: Some(sc), .. } => {
                    // The stick's arrow keys mean different keys on foot and in flight: letting go ends both, so a key held
                    // while the mode changes is not left pressed.
                    if cfg!(target_os = "android") {
                        let both: &[&str] = match sc {
                            Scancode::Up => &["Up", "W"],
                            Scancode::Down => &["Down", "S"],
                            Scancode::Left => &["Left", "A"],
                            Scancode::Right => &["Right", "D"],
                            _ => &[],
                        };
                        for k in both {
                            hp3_world::world(&mut vm).input.key_up(k);
                        }
                    }
                    if let Some(k) = key_name(sc) {
                        if cfg!(target_os = "android") && std::path::Path::new("/sdcard/FlipendoHP3/keys.flag").exists() {
                            eprintln!("key up {sc:?} -> {k}");
                        }
                        hp3_world::world(&mut vm).input.key_up(&k);
                        if cfg!(target_os = "android") && is_jump_key(&k) {
                            hp3_world::world(&mut vm).input.key_up("Enter");
                        }
                    }
                }
                Event::MouseButtonDown { mouse_btn, .. } if !free_camera => {
                    if let Some(k) = mouse_name(mouse_btn) {
                        hp3_world::world(&mut vm).input.key_down(k);
                        if cfg!(target_os = "android") && is_jump_key(k) && CREATURE.load(std::sync::atomic::Ordering::Relaxed) {
                            hp3_world::world(&mut vm).input.key_down("Enter");
                        }
                    }
                }
                Event::MouseButtonUp { mouse_btn, .. } => {
                    if let Some(k) = mouse_name(mouse_btn) {
                        hp3_world::world(&mut vm).input.key_up(k);
                        if cfg!(target_os = "android") && is_jump_key(k) {
                            hp3_world::world(&mut vm).input.key_up("Enter");
                        }
                    }
                }
                Event::Window { win_event: WindowEvent::PixelSizeChanged(w, h), .. } => {
                    (width, height) = (w.max(1) as u32, h.max(1) as u32);
                    presenter.resize(width, height);
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
        for command in commands::take() {
            commands::run(&mut vm, &command, &mut pending_level);
        }
        if hp3_world::world(&mut vm).frame % 6 == 0 {
            platform::set_cutscene(player_in_cutscene(&mut vm, in_menu));
            platform::set_creature(player_controls_creature(&mut vm));
            FLYING.store(player_flying(&mut vm), std::sync::atomic::Ordering::Relaxed);
            CREATURE.store(player_is_creature(&mut vm), std::sync::atomic::Ordering::Relaxed);
        }
        // The window's surface is gone (the app is in the background): nothing is drawn until it is back.
        if !platform::surface_alive() {
            std::thread::sleep(std::time::Duration::from_millis(50));
            surface_was_gone = true;
            last = Instant::now();
            continue;
        }
        // The presenter could not set its surface up: try again with a new one.
        if presenter.take_broken() {
            surface_was_gone = true;
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        if surface_was_gone {
            surface_was_gone = false;
            (width, height) = window.size_in_pixels();
            // The native window is a new one: the old surface is dead, so the presenter gets a surface made from it.
            // SAFETY: the window outlives the presenter, which is declared after it and dropped first.
            match unsafe { wgpu::SurfaceTargetUnsafe::from_window(&window).map_err(|e| e.to_string()).and_then(|t| instance.create_surface_unsafe(t).map_err(|e| e.to_string())) } {
                Ok(s) => {
                    eprintln!("surface: new one for the returned window ({width}x{height})");
                    presenter.replace_surface(s)
                }
                Err(e) => eprintln!("surface: {e}"),
            }
            presenter.resize(width, height);
            depth = renderer.depth_target(width.max(1), height.max(1));
        }
        // Settings > FPS limit: a frame takes at least that long.
        if let Some(budget) = platform::frame_budget() {
            // Sleep for most of the wait and spin the rest: a sleep alone overshoots by a few milliseconds.
            let spent = last.elapsed().as_secs_f32();
            if spent < budget - 0.002 {
                std::thread::sleep(std::time::Duration::from_secs_f32(budget - 0.002 - spent));
            }
            while last.elapsed().as_secs_f32() < budget {
                std::hint::spin_loop();
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
        // so a stall does not make the world jump), but not more than 60 times a second. HP3_FIXED_STEP=1 keeps a fixed 60 Hz instead.
        if fixed_step {
            accumulator += dt;
            while accumulator >= 1.0 / 60.0 {
                hp3_world::tick(&mut vm, 1.0 / 60.0);
                accumulator -= 1.0 / 60.0;
            }
        } else {
            // The world never ticks faster than 60 Hz: scripts (the imp cutscene's among them) advance by whole
            // ticks, so a faster monitor would play them too fast. A slower frame ticks with the time it took.
            accumulator += dt;
            if accumulator >= 1.0 / 60.0 - 0.0005 {
                hp3_world::tick(&mut vm, accumulator.min(1.0 / 30.0));
                accumulator = 0.0;
            }
        }
        stages.push(("ticks", frame_start.elapsed().as_secs_f32() * 1000.0));
        // A level switch the game asked for: the new level is loaded and its scene built.
        if hp3_world::world(&mut vm).quit_requested {
            break 'main;
        }
        // The option menus changed something (or a level just made a new world): the picture and the sound follow.
        let settings = hp3_world::world(&mut vm).settings_version;
        if settings != settings_seen {
            settings_seen = settings;
            presenter.set_look(hp3_world::settings::look(&vm));
            if let Some(a) = &mut audio {
                a.apply_settings(&vm);
            }
        }
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
            platform::set_level(&current_map);
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
        // In first person the player's own body and what it carries are not drawn.
        let hidden_pawn = if platform::first_person() && !platform::in_cutscene() {
            hp3_world::world(&mut vm).player.and_then(|pc| hp3_world::get(&mut vm, pc, "Pawn").as_obj())
        } else {
            None
        };
        let instances = hp3_engine::actor_instances(&mut vm, &loader, &renderer, &dynamic, &mut mesh_cache, lighting.as_ref(), Some(hp3_engine::View { eye: camera.position, forward: camera.forward(), fov_degrees: camera.fov_degrees }), hidden_pawn);
        let refs: Vec<_> = instances.iter().map(|(m, t, l)| (m.as_ref(), *t, *l)).collect();
        stages.push(("instances", frame_start.elapsed().as_secs_f32() * 1000.0));
        let particle_quads = hp3_engine::particle_quads(&mut vm, &loader, &mut particle_cache, &camera);
        stages.push(("particles", frame_start.elapsed().as_secs_f32() * 1000.0));
        let particle_images = |k: &hp3_assets::loader::ObjKey| loader.image_with_alpha(&loader.set.get(&k.0).ok()?, k.1);
        let particle_draw = particle_renderer.prepare(&renderer, &particle_quads, &particle_images);
        stages.push(("particle_upload", frame_start.elapsed().as_secs_f32() * 1000.0));
        renderer.animate_scene(&gpu_scene, hp3_world::world(&mut vm).frame as f32 / 60.0);
        // The off-screen image to draw into: waits only if the display is two frames behind.
        let slot = presenter.take();
        stages.push(("wait", frame_start.elapsed().as_secs_f32() * 1000.0));
        let view = presenter.view(slot).clone();
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
        hp3_world::world(&mut vm).canvas.ui_scale = platform::ui_scale();
        let mut quads = hp3_engine::hud_quads(&mut vm, &loader, &mut hud_cache, width, height);
        quads.extend(hp3_engine::screen_flash(&mut vm, width, height));
        quads.extend(levels.quads(width, height, &current_map));
        let images = |k: &hp3_assets::loader::ObjKey| hud_cache.image(&loader, k);
        let hud = overlay.draw(&renderer, &quads, &images, &view, width, height);
        stages.push(("hud", frame_start.elapsed().as_secs_f32() * 1000.0));
        renderer.queue.submit([cmd, hud]);
        stages.push(("submit", frame_start.elapsed().as_secs_f32() * 1000.0));
        presenter.show(slot);
        platform::tick_frame();
        stages.push(("present", frame_start.elapsed().as_secs_f32() * 1000.0));
        if stages.last().is_some_and(|s| s.1 > 200.0) {
            eprintln!("slow frame (cumulative ms): {stages:?}");
        }

        frames += 1;
        perf.add(&stages, frame_start.elapsed().as_secs_f32() * 1000.0);
        if fps_timer.elapsed().as_secs_f32() >= 1.0 {
            let fps = frames as f32 / fps_timer.elapsed().as_secs_f32();
            if std::env::var_os("HP3_PERF").is_some() || cfg!(target_os = "android") {
                eprintln!("perf: {fps:.0} fps, {}", perf.report());
                let (skinned, plain, drawn, transform, total) = hp3_engine::take_instance_times();
                eprintln!("perf: instances: skinned meshes {skinned:.0} ms/s ({} per frame), other meshes {plain:.0} ms/s, placing drawn actors {transform:.0} ms/s, whole function {total:.0} ms/s", drawn / perf.frames.max(1) as u64);
                // Where the world's ticks spent their time this second (ms per second), biggest first.
                let mut phases: Vec<_> = std::mem::take(&mut hp3_world::world(&mut vm).tick_profile).into_iter().collect();
                phases.sort_by(|a, b| b.1.cmp(&a.1));
                let text: Vec<String> = phases.iter().take(7).map(|(n, t)| format!("{n} {:.0}", t.as_secs_f32() * 1000.0)).collect();
                if !text.is_empty() {
                    eprintln!("perf: tick phases (ms this second): {}", text.join(", "));
                }
                let mut classes: Vec<_> = std::mem::take(&mut hp3_world::world(&mut vm).tick_classes).into_iter().collect();
                classes.sort_by(|a, b| b.1.1.cmp(&a.1.1));
                let text: Vec<String> = classes
                    .iter()
                    .take(8)
                    .map(|((phase, c), (n, t))| format!("{phase}:{} {:.0}ms/{n}", vm.db.names.get(vm.db.get(*c).name), t.as_secs_f32() * 1000.0))
                    .collect();
                if !text.is_empty() {
                    eprintln!("perf: top classes: {}", text.join(", "));
                }
            }
            perf = PerfStats::default();
            let _ = window.set_title(&format!("hp3 - {current_map} - {fps:.0} fps"));
            frames = 0;
            fps_timer = Instant::now();
        }
    }
    Ok(())
}
