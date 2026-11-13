//! The game runner: `hp3 <HP3 folder> [map]` plays a level in a window. The desktop binary and the Android library both
//! start `run`.
//!
//! Keys and mouse go to the game through its key bindings, and the view is the game's own camera
//! (`PlayerCalcView`). F1 switches to a free camera (WASD, mouse, Space/Ctrl, Shift) and back.
/// The scripts build and drop small boxes and vectors all the time (every vector read is an allocation): mimalloc serves them
/// several times faster than the platform allocator.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;


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
pub mod broom;
mod captaintalk;
mod cheatmenu;
pub mod commands;
mod gamelog;
mod gamepad;
mod levelmenu;
mod present;
mod quidbutton;
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
    /// The world ticks at a fixed 60 Hz and what is drawn is blended between ticks (a faster display shows smooth motion).
    pub fn interpolate() -> bool {
        true
    }
    /// The desktop game is not limited (the display paces it); `HP3_FPS=<n>` limits it.
    pub fn frame_budget() -> Option<f32> {
        let fps = FPS_LIMIT.load(std::sync::atomic::Ordering::Relaxed);
        (fps > 0).then(|| 1.0 / fps as f32)
    }
    /// The frame rate limit (0 for none), set from `HP3_FPS` and the F10 key.
    pub static FPS_LIMIT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    pub fn set_fps_limit(fps: u32) {
        FPS_LIMIT.store(fps, std::sync::atomic::Ordering::Relaxed);
    }
    pub fn fps_limit() -> u32 {
        FPS_LIMIT.load(std::sync::atomic::Ordering::Relaxed)
    }
    /// Degrees added to the game's field of view: -10 by default on the desktop (the phone uses 20), `HP3_FOV_OFFSET=<degrees>` changes it.
    pub static FOV_OFFSET: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(u32::MAX);
    pub fn fov_offset() -> f32 {
        use std::sync::atomic::Ordering::Relaxed;
        let bits = FOV_OFFSET.load(Relaxed);
        if bits != u32::MAX {
            return f32::from_bits(bits);
        }
        let v = std::env::var("HP3_FOV_OFFSET").ok().and_then(|v| v.parse().ok()).unwrap_or(-10.0);
        FOV_OFFSET.store(f32::to_bits(v), Relaxed);
        v
    }
    /// Changes the field of view offset by `delta` degrees (the `[` and `]` keys) and returns the new value.
    pub fn nudge_fov(delta: f32) -> f32 {
        let v = (fov_offset() + delta).clamp(-60.0, 60.0);
        FOV_OFFSET.store(f32::to_bits(v), std::sync::atomic::Ordering::Relaxed);
        v
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
    /// Buckbeak's flight (the phone's touch layout can swap its halves; the desktop has nothing to swap).
    pub fn set_flight(_on: bool) {}
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
/// The first-person eye sits at this fraction of the eye height, so Harry does not look a head taller than everyone else.
const FIRST_PERSON_EYE_SCALE: f32 = 0.5;
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

/// Whether the player rides Buckbeak (the controller is in `PlayerFlappingHippo`, which a flying creature like the chicken does not use).
fn player_on_hippogriff(vm: &mut Vm<'_>) -> bool {
    let Some(pc) = hp3_world::world(vm).player else { return false };
    vm.state_of(pc).is_some_and(|st| vm.db.names.get(vm.db.get(st).name).starts_with("PlayerFlappingHippo"))
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

thread_local! {
    /// The extra field of view the slide has opened so far (degrees) and when it was last moved.
    static DYN_FOV: std::cell::Cell<(f32, Option<Instant>)> = const { std::cell::Cell::new((0.0, None)) };
}

/// Dynamic field of view: the faster the hero slides, the wider the view (up to `HP3_DYN_FOV_MAX` degrees, 16 by default, 0 turns
/// it off), eased in and out so it neither jumps nor lags.
///
/// The *slide* is all this is for. A Quidditch match opens the view with speed too, and does it in the chase
/// camera itself (`quidditch::camera`, which has the broom's speed to hand and smooths it there); while one
/// runs this adds nothing, so the two never stack.
fn dynamic_fov(vm: &mut Vm<'_>) -> f32 {
    let max: f32 = std::env::var("HP3_DYN_FOV_MAX").ok().and_then(|v| v.parse().ok()).unwrap_or(16.0);
    let playing_quidditch = hp3_world::world(vm).quidditch.is_some();
    let pawn = hp3_world::world(vm).player.and_then(|pc| hp3_world::get(vm, pc, "Pawn").as_obj());
    let target = match pawn {
        Some(p) if max > 0.0 && !playing_quidditch && hp3_world::get(vm, p, "bIsSliding").as_bool() => {
            let speed = hp3_world::physics::vec(&hp3_world::get(vm, p, "Velocity")).length();
            ((speed - 150.0) / 450.0).clamp(0.0, 1.0) * max
        }
        _ => 0.0,
    };
    let now = Instant::now();
    let (cur, last) = DYN_FOV.with(|c| c.get());
    let dt = last.map_or(0.0, |l| (now - l).as_secs_f32().min(0.1));
    // Opens over about a second, closes a little quicker.
    let rate = if target > cur { 1.2 } else { 2.0 };
    let next = cur + (target - cur) * (1.0 - (-rate * dt * 3.0).exp());
    DYN_FOV.with(|c| c.set((next, Some(now))));
    next
}

fn game_camera(vm: &mut Vm<'_>) -> Option<Camera> {
    let extra_fov = dynamic_fov(vm);
    if let Some((mut position, rotation, fov)) = hp3_world::player_view(vm) {
        // The eye height the game's camera leaves out of its view; a match's chase camera is a whole view of
        // its own and needs none of it.
        if let Some(pawn) = hp3_world::world(vm).player.and_then(|pc| hp3_world::get(vm, pc, "Pawn").as_obj())
            && hp3_world::world(vm).quidditch.is_none()
        {
            position.z += hp3_world::physics::view_offset(vm, pawn);
        }
        // In play (not in a cutscene) the camera stands a little further back from the character. A Quidditch
        // match brings its own chase camera, already placed behind the broom: nothing is added to it.
        let playing = hp3_world::world(vm).quidditch.is_none()
            && hp3_world::world(vm).player.is_some_and(|pc| vm.state_of(pc).is_some_and(|st| vm.db.names.get(vm.db.get(st).name).starts_with("Player")));
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
                let head = hp3_world::physics::vec(&hp3_world::get(vm, pawn, "Location")) + Vec3::Z * (eye * FIRST_PERSON_EYE_SCALE);
                return Some(Camera { position: head + forward * FIRST_PERSON_AHEAD, rotation, fov_degrees: fov + platform::fov_offset() + extra_fov });
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
        return Some(Camera { position, rotation, fov_degrees: fov + platform::fov_offset() + extra_fov });
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

/// The GPU for the window: the first graphics API of `HP3_BACKEND` (see `hp3_render::gpu::backend_order`) whose adapter
/// can show the window and give a device. Vulkan is tried first; a phone whose Vulkan driver fails falls back to
/// OpenGL ES.
fn open_gpu(
    window: &sdl3::video::Window,
) -> Result<(wgpu::Instance, wgpu::Surface<'static>, wgpu::SurfaceCapabilities, wgpu::TextureFormat, Renderer), String> {
    let mut failed = Vec::new();
    for backends in hp3_render::gpu::backend_order() {
        let instance = hp3_render::gpu::instance_for(backends);
        // SAFETY: the window outlives the surface; both live until the end of `run`.
        let surface = match unsafe { wgpu::SurfaceTargetUnsafe::from_window(window).map_err(|e| e.to_string()).and_then(|t| instance.create_surface_unsafe(t).map_err(|e| e.to_string())) } {
            Ok(s) => s,
            Err(e) => {
                failed.push(format!("{backends:?}: surface: {e}"));
                continue;
            }
        };
        let adapter = match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        })) {
            Ok(a) => a,
            Err(e) => {
                failed.push(format!("{backends:?}: no adapter: {e}"));
                continue;
            }
        };
        let caps = surface.get_capabilities(&adapter);
        let Some(&first) = caps.formats.first() else {
            failed.push(format!("{backends:?}: the surface has no formats"));
            continue;
        };
        let format = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(first);
        match Renderer::with_adapter(&adapter, format) {
            Ok(renderer) => {
                for f in &failed {
                    eprintln!("GPU: skipped {f}");
                }
                return Ok((instance, surface, caps, format, renderer));
            }
            Err(e) => failed.push(format!("{backends:?}: {e}")),
        }
    }
    Err(format!("no usable GPU: {}", failed.join("; ")))
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
    vm.profile = std::env::var_os("HP3_PROFILE").is_some();
    vm.trace_states = std::env::var_os("HP3_TRACE_STATES").is_some();
    let t = Instant::now();
    hp3_world::load(&mut vm, &pkg, config)?;
    eprintln!("world loaded in {:?}", t.elapsed());
    commands::load_graphics(&vm);
    // Static actors are baked into the level's geometry; everything else is drawn per frame.
    let statics = hp3_engine::static_exports(&mut vm, &pkg);
    let t = Instant::now();
    // HP3_UNLIT=1 draws everything at full brightness.
    hp3_engine::sync_material_switches(&mut vm, &loader);
    let mut level_inputs = hp3_engine::map_inputs(&mut vm, &loader, &pkg, std::env::var_os("HP3_UNLIT").is_none());
    let scene = scene::build_map(&loader, &pkg, &|i| statics.contains(&i), &level_inputs);
    let mut lighting = level_inputs.lighting.take();
    let mut level_pkg = pkg.clone();
    let mut level_statics = statics;
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

    let (instance, surface, caps, format, renderer) = open_gpu(&window)?;
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
        .ok_or("the presenting thread could not be started")?;
    let mut depth = renderer.depth_target(width.max(1), height.max(1));

    let (start, start_rot) = scene.player_starts.first().copied().unwrap_or((Vec3::ZERO, [0, 0, 0]));
    // The level's geometry and pictures are on the GPU: the CPU copies (tens of megabytes) are not kept.
    drop(scene);
    loader.trim();
    let mut camera = Camera { position: start + Vec3::Z * 50.0, rotation: [start_rot[0], start_rot[1], 0], fov_degrees: 90.0 };
    sdl.mouse().set_relative_mouse_mode(&window, true);
    let mut events = sdl.event_pump()?;
    // No gamepad subsystem is not fatal: the keyboard and the touch controls still work.
    let mut pad = sdl.gamepad().map(gamepad::Pad::new).map_err(|e| eprintln!("gamepad: off ({e})")).ok();
    let mut pad_clock = Instant::now();
    let mut last = Instant::now();
    // HP3_CAST=<Spell>:<TargetClass>[:n[:distance]]@<tick> casts a spell at a creature, as in hp3-run (with HP3_SHOT to see it).
    let mut cast = hp3_world::casttest::CastTest::from_env();
    let mut cast_ticks = 0u32;
    let mut accumulator = 0.0f32;
    let mut switch_check = 0u32;
    let (mut cam_from, mut cam_to, mut cam_tick) = (camera, camera, 0u64);
    let mut fog_state = hp3_engine::FogState::default();
    let fixed_step = std::env::var_os("HP3_FIXED_STEP").is_some();
    // The frame rate limit: `HP3_FPS` wins, then the one chosen with F10 last time, else none.
    let saved_fps = hp3_world::settings::float(&vm, "hp3.app", "FpsLimit", 0.0) as u32;
    platform::set_fps_limit(std::env::var("HP3_FPS").ok().and_then(|v| v.parse().ok()).unwrap_or(saved_fps));
    let mut frames = 0u32;
    // When the limit was last changed (the window title says so for a few seconds).
    let mut fps_note: Option<(Instant, u32)> = None;
    let mut perf = PerfStats::default();
    // How long a frame's work is taking, in milliseconds, smoothed over the last frames (0 before the first
    // one). A Quidditch match's frame budget is chosen from it (`quidditch::lod`).
    let mut avg_frame_ms = 0.0f32;
    let mut fps_timer = Instant::now();
    let mut free_camera = false;
    let mut in_menu = false;
    // F11 level list; G ghost, F fly, H walk (debug movement).
    let mut levels = levelmenu::LevelMenu::new(root);
    // F12 cheat list (the phone debug menu's entries).
    let mut cheats = cheatmenu::CheatMenu::new();
    let mut captain_talk = captaintalk::CaptainTalk::new();
    let mut captain_offer = captaintalk::Offer::SeasonOver;
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
    let mut audio_reopen_at: Option<Instant> = None;
    let mut audio_check_at = Instant::now();
    'main: loop {
        if audio_check_at.elapsed() >= std::time::Duration::from_secs(1) {
            audio_check_at = Instant::now();
            if let Some(a) = &mut audio {
                a.check_device(&sdl);
            }
        }
        if audio_reopen_at.is_some_and(|t| Instant::now() >= t) {
            audio_reopen_at = None;
            match &mut audio {
                Some(a) => {
                    a.reopen(&sdl);
                }
                // The game started with no output at all: a device that shows up later is used.
                None if std::env::var_os("HP3_NO_AUDIO").is_none() => audio = audio::AudioSystem::new(&sdl, &root),
                None => {}
            }
        }
        // Menus take the mouse as a visible cursor; play takes it relative.
        let menu = hp3_world::gui::menu_open(&mut vm);
        if menu != in_menu {
            in_menu = menu;
            platform::set_in_menu(menu);
            sdl.mouse().set_relative_mouse_mode(&window, !menu);
        }
        // The Quidditch captain in the Grounds hub: a prompt while the player stands near, a speech box once they talk.
        let match_running = hp3_world::world(&mut vm).quidditch.is_some();
        if captain_talk.open && (in_menu || match_running) {
            captain_talk.open = false;
        }
        let captain_near = !in_menu && !cheats.open && !levels.open && !captain_talk.open && !match_running && hp3_world::quidditch::captain::near(&mut vm);
        // The QUIDDITCH button is on the front-end menu and on the in-game menu (its tabbed pages), while no match runs and no debug
        // list is open.
        let quid_button = in_menu && !levels.open && !cheats.open && hp3_world::world(&mut vm).quidditch.is_none() && {
            let front = hp3_world::quidditch::menu::front_end_map(&mut vm);
            front.eq_ignore_ascii_case(&current_map) || hp3_world::gui::top_menu_class(&mut vm).is_some_and(|c| c.starts_with("Tab_"))
        };
        for event in events.poll_iter() {
            if let Some(p) = &mut pad
                && matches!(
                    event,
                    Event::GamepadAdded { .. }
                        | Event::GamepadRemoved { .. }
                        | Event::GamepadAxisMotion { .. }
                        | Event::GamepadButtonDown { .. }
                        | Event::GamepadButtonUp { .. }
                )
            {
                for action in p.event(&event, in_menu) {
                    apply_pad_action(&mut vm, action);
                }
                continue;
            }
            match event {
                Event::Quit { .. } => break 'main,
                // An output device came or went (a Bluetooth headset connected): the sound moves to the new default once the
                // burst of events a connection makes has passed.
                Event::AudioDeviceAdded { iscapture: false, .. } | Event::AudioDeviceRemoved { iscapture: false, .. } => {
                    audio_reopen_at = Some(Instant::now() + std::time::Duration::from_millis(700));
                }
                Event::MouseButtonDown { mouse_btn: MouseButton::Left, x, y, .. } if quid_button && {
                    let (ww, _) = window.size();
                    let scale = width as f32 / ww.max(1) as f32;
                    quidbutton::hit(x * scale, y * scale, width, height)
                } => commands::push("quidditch"),
                Event::MouseButtonUp { mouse_btn: MouseButton::Left, x, y, .. } if quid_button && {
                    let (ww, _) = window.size();
                    let scale = width as f32 / ww.max(1) as f32;
                    quidbutton::hit(x * scale, y * scale, width, height)
                } => {}
                Event::KeyDown { scancode: Some(Scancode::F11), repeat: false, .. } if !cheats.open => levels.toggle(&current_map),
                Event::KeyDown { scancode: Some(Scancode::F12), repeat: false, .. } if !levels.open => cheats.toggle(),
                // F7: gamma, brightness and contrast back to the defaults (also while a menu is open).
                Event::KeyDown { scancode: Some(Scancode::F7), repeat: false, .. } => commands::push("resetpicture"),
                // F4: a screenshot with where the player is burned into it, plus the full F8 report beside it, in <root>/screenshots.
                #[cfg(not(target_os = "android"))]
                Event::KeyDown { scancode: Some(Scancode::F4), repeat: false, .. } => {
                    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                    let path = root.join("screenshots").join(format!("shot-{stamp}.png"));
                    let _ = std::fs::create_dir_all(root.join("screenshots"));
                    let lines = hp3_world::physics::caption(&mut vm, camera.position, camera.rotation);
                    let _ = std::fs::write(path.with_extension("txt"), format!("{}
{}", lines.join("
"), hp3_world::physics::mark(&mut vm)));
                    eprintln!("screenshot: {} ({})", path.display(), lines.join(" | "));
                    if let Ok(mut r) = present::SHOT_REQUEST.lock() {
                        *r = Some((path, lines));
                    }
                }
                // F6: the actors around the player go to the log (what a pickup that cannot be taken looks like to the engine).
                Event::KeyDown { scancode: Some(Scancode::F6), repeat: false, .. } if !in_menu => commands::push("dumpnear"),
                // [ and ] (or numpad - and +): the field of view narrows or widens by 5 degrees.
                Event::KeyDown { scancode: Some(sc @ (Scancode::LeftBracket | Scancode::RightBracket | Scancode::KpMinus | Scancode::KpPlus)), .. } if !in_menu && !cfg!(target_os = "android") => {
                    let delta = if matches!(sc, Scancode::LeftBracket | Scancode::KpMinus) { -5.0 } else { 5.0 };
                    let v = platform::nudge_fov(delta);
                    eprintln!("fov offset: {v:+}");
                    let _ = window.set_title(&format!("hp3 - FOV offset {v:+}"));
                }
                // F10: the frame rate limit steps through the usual refresh rates, then none.
                Event::KeyDown { scancode: Some(Scancode::F10), repeat: false, .. } => {
                    const STEPS: [u32; 8] = [30, 60, 90, 120, 144, 165, 240, 0];
                    let now = platform::fps_limit();
                    let next = STEPS.iter().position(|&s| s == now).map_or(STEPS[0], |i| STEPS[(i + 1) % STEPS.len()]);
                    platform::set_fps_limit(next);
                    hp3_world::settings::store(&mut vm, "hp3.app", "FpsLimit", &next.to_string());
                    fps_note = Some((Instant::now(), next));
                    eprintln!("fps limit: {}", if next == 0 { "none".to_string() } else { next.to_string() });
                }
                // The cheat list takes the keyboard and the mouse the same way.
                Event::KeyDown { scancode: Some(sc), .. } if cheats.open => match sc {
                    Scancode::Up => cheats.step(-1),
                    Scancode::Down => cheats.step(1),
                    Scancode::Home => cheats.home(),
                    Scancode::End => cheats.end(),
                    Scancode::Return | Scancode::KpEnter => match cheats.choose() {
                        cheatmenu::Pick::Command(c) => commands::push(c),
                        cheatmenu::Pick::Levels => levels.toggle(&current_map),
                    },
                    Scancode::Escape => cheats.open = false,
                    _ => {}
                },
                Event::MouseMotion { .. } | Event::MouseButtonDown { .. } if cheats.open => {}
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
                // E next to the captain starts the talk; while it is open it takes the keyboard (Y or Enter: play, N or Escape: not now).
                Event::KeyDown { scancode: Some(Scancode::E), repeat: false, .. } if captain_near => {
                    captain_offer = match hp3_world::quidditch::menu::next_opponent(&mut vm) {
                        Some(o) => captaintalk::Offer::Match(o.name().to_ascii_uppercase()),
                        None => captaintalk::Offer::SeasonOver,
                    };
                    captain_talk.open = true;
                }
                Event::KeyDown { scancode: Some(sc), .. } if captain_talk.open => match sc {
                    Scancode::Y | Scancode::Return | Scancode::KpEnter => {
                        captain_talk.open = false;
                        if matches!(captain_offer, captaintalk::Offer::Match(_)) {
                            commands::push("quidditch");
                        }
                    }
                    Scancode::N | Scancode::Escape => captain_talk.open = false,
                    _ => {}
                },
                Event::MouseMotion { .. } | Event::MouseButtonDown { .. } if captain_talk.open => {}
                Event::KeyDown { scancode: Some(sc @ (Scancode::G | Scancode::F | Scancode::H)), repeat: false, .. } if !in_menu => {
                    let mode = match sc {
                        Scancode::G => hp3_world::debug_move::Mode::Ghost,
                        Scancode::F => hp3_world::debug_move::Mode::Fly,
                        _ => hp3_world::debug_move::Mode::Walk,
                    };
                    eprintln!("movement: {mode:?} ({})", if hp3_world::debug_move::set_mode(&mut vm, mode) { "ok" } else { "no pawn" });
                }
                // Character switch: 1, 2, 3 pick Harry, Ron, Hermione; C goes to the next one (the phone's switch button sends C).
                // F9: writes where the player is and what is around to logs/marks.log (for bug reports).
                Event::KeyDown { scancode: Some(Scancode::F8), repeat: false, .. } if !in_menu => {
                    let text = hp3_world::physics::mark(&mut vm);
                    eprint!("{text}");
                    let path = root.join("logs").join("marks.log");
                    let _ = std::fs::create_dir_all(root.join("logs"));
                    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                        use std::io::Write;
                        let _ = f.write_all(text.as_bytes());
                    }
                }
                Event::KeyDown { scancode: Some(sc @ (Scancode::_1 | Scancode::_2 | Scancode::_3 | Scancode::C)), repeat: false, .. } if !in_menu => {
                    let which = match sc {
                        Scancode::_1 => Some(0),
                        Scancode::_2 => Some(1),
                        Scancode::_3 => Some(2),
                        _ => None,
                    };
                    eprintln!("switch character: {}", hp3_world::character_switch::switch(&mut vm, which));
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
                // Q: Carpe or the crab's own spell for the next lock-on (only with "Carpe pulls crabs" on).
                Event::KeyDown { scancode: Some(Scancode::Q), repeat: false, .. } if !in_menu && !cheats.open && !levels.open && hp3_world::natives::carpe_crab_on(&vm) => {
                    let on = !hp3_world::natives::CARPE_PICK.load(std::sync::atomic::Ordering::Relaxed);
                    hp3_world::natives::CARPE_PICK.store(on, std::sync::atomic::Ordering::Relaxed);
                    eprintln!("carpe for crabs: {}", if on { "on" } else { "off" });
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
        if let Some(p) = &mut pad {
            let dt = pad_clock.elapsed().as_secs_f32().min(0.1);
            pad_clock = Instant::now();
            for action in p.frame(&mut hp3_world::world(&mut vm).input, in_menu, dt) {
                apply_pad_action(&mut vm, action);
            }
        }
        // HP3_COMMANDS=<command>@<tick>[;<command>@<tick>...] (debugging): runs those commands at those world ticks (`quicksave@100;quickload@200`).
        if let Ok(list) = std::env::var("HP3_COMMANDS") {
            let tick = hp3_world::world(&mut vm).frame;
            for item in list.split(';') {
                if let Some((command, at)) = item.rsplit_once('@')
                    && at.trim().parse::<u64>() == Ok(tick as u64)
                {
                    commands::push(command.trim());
                }
            }
        }
        for command in commands::take() {
            commands::run(&mut vm, &command, &mut pending_level);
        }
        // A Quidditch match: the phone's touch layout flies the broom (its stick and buttons come in through
        // `broom`), and the frames it is taking set the match's budget. Both are nothing on every other
        // level, where no match is running.
        if hp3_world::world(&mut vm).quidditch.is_some() {
            if let Some(t) = broom::take() {
                hp3_world::quidditch::phone::feed_touch(&mut vm, t.stick, t.reach_tap);
            }
            hp3_world::quidditch::phone::feed_frame_ms(&mut vm, avg_frame_ms);
        }
        if hp3_world::world(&mut vm).frame % 6 == 0 {
            broom::set_on_broom(hp3_world::world(&mut vm).quidditch.is_some());
            platform::set_cutscene(player_in_cutscene(&mut vm, in_menu));
            platform::set_creature(player_controls_creature(&mut vm));
            FLYING.store(player_flying(&mut vm), std::sync::atomic::Ordering::Relaxed);
            platform::set_flight(player_on_hippogriff(&mut vm));
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
        // The world ticks at 60 Hz: scripts (the imp cutscene's among them) advance by whole ticks. On the desktop each tick
        // is a fixed 1/60 second and a faster display draws the state blended between the last two ticks (`blend`).
        // HP3_FIXED_STEP=1 keeps the plain fixed step without blending.
        let mut blend = 1.0f32;
        if platform::interpolate() && !fixed_step {
            accumulator += dt;
            let mut ticks = 0;
            while accumulator >= 1.0 / 60.0 && ticks < 6 {
                tick_world(&mut vm, 1.0 / 60.0, &mut cast, &mut cast_ticks);
                accumulator -= 1.0 / 60.0;
                ticks += 1;
            }
            if accumulator >= 1.0 / 60.0 {
                // Too far behind: the rest of the lag is dropped, not played later in a rush.
                accumulator = 0.0;
            }
            blend = (accumulator * 60.0).clamp(0.0, 1.0);
        } else if fixed_step {
            accumulator += dt;
            while accumulator >= 1.0 / 60.0 {
                tick_world(&mut vm, 1.0 / 60.0, &mut cast, &mut cast_ticks);
                accumulator -= 1.0 / 60.0;
            }
        } else {
            // The world never ticks faster than 60 Hz: scripts (the imp cutscene's among them) advance by whole
            // ticks, so a faster monitor would play them too fast. A slower frame ticks with the time it took.
            accumulator += dt;
            if accumulator >= 1.0 / 60.0 - 0.0005 {
                tick_world(&mut vm, accumulator.min(1.0 / 30.0), &mut cast, &mut cast_ticks);
                accumulator = 0.0;
            }
        }
        stages.push(("ticks", frame_start.elapsed().as_secs_f32() * 1000.0));
        // A script flipped a material switch (Glacius froze the water): what was baked from it is built again.
        switch_check += 1;
        if switch_check % 6 == 0 && hp3_engine::sync_material_switches(&mut vm, &loader) {
            let t = Instant::now();
            level_inputs.lighting = lighting.take();
            let scene = scene::build_map(&loader, &level_pkg, &|i| level_statics.contains(&i), &level_inputs);
            lighting = level_inputs.lighting.take();
            gpu_scene = renderer.upload_scene(&scene);
            drop(scene);
            loader.trim();
            mesh_cache = hp3_engine::MeshCache::default();
            eprintln!("material switch changed: scene rebuilt in {:?}", t.elapsed());
        }
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
        // A level picked from a menu (a plain map name) starts in its first story state, so what leads out of it lands where the
        // next level expects the story to be.
        if let Some(menu_url) = pending_level.take_if(|u| !u.contains('?')) {
            let state = hp3_world::menu_start_state(&mut vm, &menu_url);
            pending_level = Some(match state {
                Some(s) if std::env::var_os("HP3_MENU_STATE0").is_none() => format!("{menu_url}?GameState={s}"),
                _ => menu_url,
            });
        }
        if let Some(url) = hp3_world::take_travel(&mut vm).or_else(|| pending_level.take()) {
            let t = Instant::now();
            let pkg = hp3_world::travel(&mut vm, &url)?;
            let statics = hp3_engine::static_exports(&mut vm, &pkg);
            loader.clear_switch_current();
            hp3_engine::sync_material_switches(&mut vm, &loader);
            let mut inputs = hp3_engine::map_inputs(&mut vm, &loader, &pkg, std::env::var_os("HP3_UNLIT").is_none());
            let scene = scene::build_map(&loader, &pkg, &|i| statics.contains(&i), &inputs);
            gpu_scene = renderer.upload_scene(&scene);
            drop(scene);
            loader.trim();
            lighting = inputs.lighting.take();
            level_inputs = inputs;
            level_statics = statics;
            level_pkg = pkg.clone();
            mesh_cache = hp3_engine::MeshCache::default();
            fog_state = hp3_engine::FogState::default();
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
            let tick = hp3_world::world(&mut vm).frame;
            if tick != cam_tick {
                // A new tick: the camera of the last one is where the blend starts (a cut or a jump starts at the new place).
                cam_from = if cam_tick + 1 == tick && cam_to.position.distance(c.position) < 400.0 { cam_to } else { c };
                cam_tick = tick;
            }
            cam_to = c;
            camera = blend_cameras(&cam_from, &cam_to, blend);
        }
        if let Some((position, rotation)) = *hp3_world::casttest::CAMERA.lock().unwrap() {
            camera.position = position;
            camera.rotation = rotation;
        }
        if let Some(a) = &mut audio {
            a.update(&mut vm, camera.position, camera.rotation[1]);
        }
        game_log.update(&mut vm);
        stages.push(("camera", frame_start.elapsed().as_secs_f32() * 1000.0));
        // In first person the player's own body and what it carries are not drawn.
        renderer.set_fog(hp3_engine::view_fog(&mut vm, &mut fog_state, &current_map, camera.position, dt));
        let hidden_pawn = if platform::first_person() && !platform::in_cutscene() {
            hp3_world::world(&mut vm).player.and_then(|pc| hp3_world::get(&mut vm, pc, "Pawn").as_obj())
        } else {
            None
        };
        let instances = hp3_engine::actor_instances(&mut vm, &loader, &renderer, &dynamic, &mut mesh_cache, lighting.as_ref(), Some(hp3_engine::View { eye: camera.position, forward: camera.forward(), fov_degrees: camera.fov_degrees }), hidden_pawn, blend);
        let refs: Vec<_> = instances.iter().map(|(m, t, l)| (m.as_ref(), *t, l.clone())).collect();
        stages.push(("instances", frame_start.elapsed().as_secs_f32() * 1000.0));
        let particle_quads = hp3_engine::particle_quads(&mut vm, &loader, &mut particle_cache, &camera);
        stages.push(("particles", frame_start.elapsed().as_secs_f32() * 1000.0));
        let particle_images = |k: &hp3_assets::loader::ObjKey| loader.image_with_alpha(&loader.set.get(&k.0).ok()?, k.1);
        let particle_draw = particle_renderer.prepare(&renderer, &particle_quads, &particle_images);
        stages.push(("particle_upload", frame_start.elapsed().as_secs_f32() * 1000.0));
        renderer.animate_scene(&gpu_scene, hp3_world::world(&mut vm).frame as f32 / 60.0);
        let fluids = renderer.fluid_actors(&gpu_scene);
        if !fluids.is_empty() {
            renderer.move_fluids(&gpu_scene, &hp3_engine::fluid_places(&mut vm, &fluids));
        }
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
        quads.extend(cheats.quads(width, height));
        if captain_talk.open {
            quads.extend(captain_talk.quads(width, height, &captain_offer));
        } else if captain_near {
            quads.extend(captain_talk.prompt(width, height));
        }
        // The spell indicator: with "Carpe pulls crabs" on, or while a spell picked by hand (the phone's spell button) is in the wand,
        // and for a moment after the button was pressed.
        let crab_option = hp3_world::natives::carpe_crab_on(&vm);
        let hand_pick = hp3_world::natives::spell_pick();
        let toast = commands::spell_toast();
        // (Not in a Quidditch match: spells are not cast there, and the box sits over the match's score banner.)
        if (crab_option || hand_pick.is_some() || toast.is_some()) && hp3_world::world(&mut vm).quidditch.is_none() {
            let spell = hp3_world::natives::wand_spell_name(&mut vm).unwrap_or_else(|| "-".to_string());
            let picked = hp3_world::natives::CARPE_PICK.load(std::sync::atomic::Ordering::Relaxed);
            let mode = match (&hand_pick, &toast) {
                (Some(_), _) => "PICKED",
                (None, Some(_)) => "AUTO",
                _ => "",
            };
            quads.extend(cheatmenu::spell_indicator(&spell, mode, crab_option.then_some(picked), width, height));
        }
        if quid_button {
            quads.extend(quidbutton::quads(width, height));
        }
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

        // Pictures decoded since (spawned actors, menus) that nothing holds any more are let go every half minute or so.
        if hp3_world::world(&mut vm).frame % 1800 == 0 {
            loader.trim();
        }
        frames += 1;
        let took = frame_start.elapsed().as_secs_f32() * 1000.0;
        perf.add(&stages, took);
        // What a frame is costing, smoothed: the frame rate limiter's own wait is not in it (it is measured
        // from `frame_start`), so this is the work a frame takes. A Quidditch match's budget follows it.
        avg_frame_ms += (took - avg_frame_ms) * if avg_frame_ms > 0.0 { 0.1 } else { 1.0 };
        if fps_timer.elapsed().as_secs_f32() >= 1.0 {
            let fps = frames as f32 / fps_timer.elapsed().as_secs_f32();
            if std::env::var_os("HP3_PERF").is_some() || cfg!(target_os = "android") {
                eprintln!("perf: {fps:.0} fps, {}", perf.report());
                let (skinned, plain, drawn, transform, total) = hp3_engine::take_instance_times();
                eprintln!("perf: instances: skinned meshes {skinned:.0} ms/s ({} per frame), other meshes {plain:.0} ms/s, placing drawn actors {transform:.0} ms/s, whole function {total:.0} ms/s", drawn / perf.frames.max(1) as u64);
                let (posed, draws, upload_ns) = hp3_render::dynamic::take_draw_counts();
                let n = perf.frames.max(1) as u64;
                eprintln!(
                    "perf: draw: posed vertices {} KB per frame ({:.1} ms to upload), actor draw calls {} per frame",
                    posed / n / 1024,
                    upload_ns as f64 / n as f64 / 1e6,
                    draws / n
                );
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
                // HP3_PROFILE=1: the natives that took the most this second, and the script functions that ran the most statements.
                if vm.profile {
                    let mut n: Vec<_> = std::mem::take(&mut vm.profile_natives).into_iter().map(|(f, (c, t))| (t, c, f)).collect();
                    n.sort_by(|a, b| b.0.cmp(&a.0));
                    let text: Vec<String> = n.iter().take(8).map(|(t, c, f)| format!("{} {:.1}ms/{c}", vm.function_path(*f), t.as_secs_f32() * 1000.0)).collect();
                    eprintln!("perf: natives: {}", text.join(", "));
                    let mut p: Vec<_> = std::mem::take(&mut vm.profile_counts).into_iter().map(|(f, n)| (n, f)).collect();
                    p.sort_by(|a, b| b.0.cmp(&a.0));
                    let text: Vec<String> = p.iter().take(8).map(|(n, f)| format!("{} {n}", vm.function_path(*f))).collect();
                    eprintln!("perf: statements: {}", text.join(", "));
                }
            }
            perf = PerfStats::default();
            let limit = match fps_note {
                Some((t, n)) if t.elapsed().as_secs() < 6 => format!(" (limit {}, F10 changes)", if n == 0 { "none".to_string() } else { n.to_string() }),
                _ => String::new(),
            };
            let _ = window.set_title(&format!("hp3 - {current_map} - {fps:.0} fps{limit}"));
            frames = 0;
            fps_timer = Instant::now();
        }
    }
    Ok(())
}

/// The camera `t` of the way from `a` to `b` (angles the short way round).
/// Carries out what a gamepad button asked for.
fn apply_pad_action(vm: &mut Vm<'_>, action: gamepad::Action) {
    use gamepad::Action;
    match action {
        Action::Key(k, true) => hp3_world::world(vm).input.key_down(k),
        Action::Key(k, false) => hp3_world::world(vm).input.key_up(k),
        Action::Notch(k) => {
            hp3_world::world(vm).input.key_down(k);
            hp3_world::world(vm).input.key_up(k);
        }
        Action::SwitchCharacter => eprintln!("switch character: {}", hp3_world::character_switch::switch(vm, None)),
        Action::Menu(k, state) => {
            hp3_world::gui::key(vm, k, state, 0.0);
        }
    }
}

fn blend_cameras(a: &Camera, b: &Camera, t: f32) -> Camera {
    let turn = |x: i32, y: i32| {
        let d = ((y - x) as i64).rem_euclid(65536) as i32;
        let d = if d > 32768 { d - 65536 } else { d };
        x + (d as f32 * t) as i32
    };
    if t >= 1.0 {
        return *b;
    }
    Camera {
        position: a.position.lerp(b.position, t),
        rotation: [turn(a.rotation[0], b.rotation[0]), turn(a.rotation[1], b.rotation[1]), turn(a.rotation[2], b.rotation[2])],
        fov_degrees: a.fov_degrees + (b.fov_degrees - a.fov_degrees) * t,
    }
}

/// One tick of the world, first stepping the scripted spell cast (`HP3_CAST`) when there is one.
fn tick_world(vm: &mut hp3_script::Vm<'_>, dt: f32, cast: &mut Option<hp3_world::casttest::CastTest>, cast_ticks: &mut u32) {
    present::TICKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // HP3_SPAWN=<Class>[,<Class>...]: once the player has control, spawns these at the player's pawn (to look at an effect on its own).
    if let Ok(list) = std::env::var("HP3_SPAWN") {
        static SPAWNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !SPAWNED.load(std::sync::atomic::Ordering::Relaxed)
            && let Some(pawn) = hero_actor(vm)
            && present::TICKS.load(std::sync::atomic::Ordering::Relaxed) > std::env::var("HP3_SPAWN_AFTER").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(60)
        {
            SPAWNED.store(true, std::sync::atomic::Ordering::Relaxed);
            for name in list.split(',') {
                let class = vm.db.find(name).map(|s| vm.db.get(s).object);
                let made = class.map(|c| hp3_world::natives::spawn(vm, Some(pawn), &[hp3_script::Value::Object(Some(c))]));
                let path = made.as_ref().and_then(|r| r.as_ref().ok()).and_then(|v| v.as_obj()).map(|o| hp3_script::natives::object_path(vm, o));
                eprintln!("spawn: {name} -> {path:?}");
            }
        }
    }
    // HP3_SPAWN_CAM=<distance>,<height>: a close-up camera on the player's pawn (with HP3_SPAWN).
    if let Ok(spec) = std::env::var("HP3_SPAWN_CAM") {
        let mut it = spec.split(',').filter_map(|v| v.parse::<f32>().ok());
        let (dist, height) = (it.next().unwrap_or(120.0), it.next().unwrap_or(30.0));
        if let Some(pawn) = hero_actor(vm) {
            let at = hp3_world::physics::vec(&hp3_world::get(vm, pawn, "Location"));
            let yaw = hp3_world::get(vm, pawn, "Rotation").fields().get(1).map_or(0, hp3_script::Value::as_int) as f32 * std::f32::consts::TAU / 65536.0;
            let eye = at + glam::Vec3::new(yaw.cos() * dist, yaw.sin() * dist, height);
            let d = at + glam::Vec3::Z * (height * 0.9) - eye;
            let k = 65536.0 / std::f32::consts::TAU;
            *hp3_world::casttest::CAMERA.lock().unwrap() = Some((eye, [(d.z.atan2(d.truncate().length()) * k).round() as i32, (d.y.atan2(d.x) * k).round() as i32, 0]));
        }
    }
    // HP3_TRACK=<every>: prints the player's pawn (state, place, speed, floor) every that many ticks.
    if let Ok(every) = std::env::var("HP3_TRACK") {
        let tick = present::TICKS.load(std::sync::atomic::Ordering::Relaxed);
        if let Some(pc) = hp3_world::world(vm).player
            && tick % every.parse::<u64>().unwrap_or(10).max(1) == 0
            && let Some(pawn) = hp3_world::get(vm, pc, "Pawn").as_obj()
        {
            let st = vm.state_of(pc).map(|s| vm.db.names.get(vm.db.get(s).name).to_string()).unwrap_or_default();
            let (at, vel, floor) = (hp3_world::physics::vec(&hp3_world::get(vm, pawn, "Location")), hp3_world::physics::vec(&hp3_world::get(vm, pawn, "Velocity")), hp3_world::physics::vec(&hp3_world::get(vm, pawn, "Floor")));
            let phys = hp3_world::get(vm, pawn, "Physics").as_byte();
            let vol = hp3_world::get(vm, pawn, "PhysicsVolume").as_obj().map(|v| hp3_script::natives::object_path(vm, v)).unwrap_or_default();
            eprintln!("track: tick {tick} {st} phys {phys} at {at:.1} vel {vel:.1} speed {:.0} floor {floor:.4} vol {vol}", vel.length());
        }
    }
    // HP3_TELEPORT=<x>,<y>,<z>@<tick>[;...]: puts the player's pawn there at that tick (as in hp3-run), HP3_TELEPORT_YAW turns it.
    if let Ok(list) = std::env::var("HP3_TELEPORT") {
        static DONE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let tick = present::TICKS.load(std::sync::atomic::Ordering::Relaxed);
        for (i, spec) in list.split(';').enumerate() {
            let bit = 1u32 << i.min(31);
            let Some((at, when)) = spec.split_once('@') else { continue };
            let c: Vec<f32> = at.split(',').filter_map(|v| v.parse().ok()).collect();
            if DONE.load(std::sync::atomic::Ordering::Relaxed) & bit != 0 || when.parse::<u64>().is_ok_and(|w| tick < w) {
                continue;
            }
            // Waits for the player to have control (the opening cutscene is over).
            let Some(pc) = hp3_world::world(vm).player else { continue };
            if tick % 20 == 0 && tick <= 400 {
                let st = vm.state_of(pc).map(|s| vm.db.names.get(vm.db.get(s).name).to_string());
                let pw = hp3_world::get(vm, pc, "Pawn").as_obj();
                eprintln!("teleport: waiting at tick {tick}: controller state {st:?}, pawn {pw:?}");
            }
            let Some(pawn) = hp3_world::get(vm, pc, "Pawn").as_obj() else { continue };
            let walking = vm.state_of(pc).is_some_and(|s| vm.db.names.get(vm.db.get(s).name) == "PlayerWalking");
            // A fourth number is the heading (65536 to a turn).
            if let (&[x, y, z, ..], true) = (c.as_slice(), walking) {
                hp3_world::physics::far_move(vm, pawn, glam::Vec3::new(x, y, z), false, false);
                let yaw = c.get(3).map(|y| *y as i32).or_else(|| std::env::var("HP3_TELEPORT_YAW").ok().and_then(|v| v.parse::<i32>().ok()));
                if let Some(yaw) = yaw {
                    let r = hp3_script::Value::Struct(vec![hp3_script::Value::Int(0), hp3_script::Value::Int(yaw), hp3_script::Value::Int(0)].into());
                    hp3_world::set(vm, pawn, "Rotation", r.clone());
                    hp3_world::set(vm, pawn, "DesiredRotation", r.clone());
                    hp3_world::set(vm, pc, "Rotation", r);
                }
                DONE.fetch_or(bit, std::sync::atomic::Ordering::Relaxed);
                eprintln!("teleport: player to {x},{y},{z} at tick {tick}");
            }
        }
    }
    if let Some(t) = cast.as_mut() {
        *cast_ticks += 1;
        hp3_world::casttest::step(vm, t, *cast_ticks);
    }
    hp3_world::tick(vm, dt);
}

/// The player's pawn, or else the first hero pawn in the level (an opening cutscene has no player pawn).
fn hero_actor(vm: &mut hp3_script::Vm<'_>) -> Option<hp3_script::ObjId> {
    if let Some(s) = hp3_world::lipsync::speaker(vm) {
        return Some(s);
    }
    if let Some(pc) = hp3_world::world(vm).player
        && let Some(p) = hp3_world::get(vm, pc, "Pawn").as_obj()
    {
        return Some(p);
    }
    let hero = vm.db.find("HPHeroPawn")?;
    let actors = hp3_world::world(vm).actors.clone();
    actors.into_iter().find(|&a| !vm.db.objects.get(a).deleted && vm.db.objects.get(a).class.is_some_and(|c| vm.is_child_of(c, hero)))
}
