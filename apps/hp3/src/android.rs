//! The Android entry: SDL's activity loads this library and calls `SDL_main` with the game folder as the first
//! argument. What the game prints goes to logcat.

use std::ffi::{CStr, CString, c_char, c_int};
use std::io::{BufRead, BufReader};
use std::os::fd::FromRawFd;
use std::path::PathBuf;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};

unsafe extern "C" {
    fn __android_log_write(priority: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

const ANDROID_LOG_INFO: c_int = 4;

fn log_line(text: &str) {
    if let (Ok(tag), Ok(text)) = (CString::new("hp3"), CString::new(text.replace('\0', " "))) {
        // SAFETY: both strings are NUL-terminated and outlive the call.
        unsafe { __android_log_write(ANDROID_LOG_INFO, tag.as_ptr(), text.as_ptr()) };
    }
}

/// Sends everything written to stdout and stderr to logcat, a line at a time.
fn forward_output_to_logcat() {
    let mut fds = [0 as c_int; 2];
    // SAFETY: `fds` has room for the two descriptors `pipe` writes.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return;
    }
    // SAFETY: both descriptors are open; 1 and 2 are stdout and stderr, now pointing at the pipe's write end.
    unsafe {
        libc::dup2(fds[1], 1);
        libc::dup2(fds[1], 2);
        libc::close(fds[1]);
    }
    // SAFETY: `fds[0]` is the read end of the pipe made above and nothing else owns it.
    let reader = unsafe { std::fs::File::from_raw_fd(fds[0]) };
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            log_line(&line);
        }
    });
}

/// SDL's activity runs this on its game thread: `argv[1]` is the game folder, `argv[2]` an optional map.
#[unsafe(no_mangle)]
pub extern "C" fn SDL_main(argc: c_int, argv: *mut *mut c_char) -> c_int {
    forward_output_to_logcat();
    let args: Vec<String> = (0..argc.max(0) as usize)
        .filter_map(|i| {
            // SAFETY: SDL passes `argc` valid NUL-terminated strings.
            let p = unsafe { *argv.add(i) };
            (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
        })
        .collect();
    log_line(&format!("hp3 starts with {args:?}"));
    // A file named `profile.flag` in the game folder asks for the world's tick timings in the log, once a second (they
    // slow the game down by a few milliseconds a tick, so they are off otherwise).
    if args.get(1).is_some_and(|root| std::path::Path::new(root).join("profile.flag").exists()) {
        // SAFETY: nothing else is running yet; this is the first thing the game thread does.
        unsafe { std::env::set_var("HP3_TICK_PROFILE", "1") };
    }
    // A file named `states.flag` asks for every script state change in the log (who went from which state to which).
    if args.get(1).is_some_and(|root| std::path::Path::new(root).join("states.flag").exists()) {
        // SAFETY: as above.
        unsafe { std::env::set_var("HP3_TRACE_STATES", "1") };
    }
    // A file named `env.flag` sets debugging switches: one `NAME=value` a line (lines starting with `#` are skipped).
    if let Some(text) = args.get(1).and_then(|root| std::fs::read_to_string(std::path::Path::new(root).join("env.flag")).ok()) {
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            if let Some((k, v)) = line.split_once('=') {
                log_line(&format!("env.flag: {}={}", k.trim(), v.trim()));
                // SAFETY: as above.
                unsafe { std::env::set_var(k.trim(), v.trim()) };
            }
        }
    }
    let Some(root) = args.get(1).map(PathBuf::from) else {
        log_line("no game folder given");
        return 2;
    };
    match crate::run(&root, args.get(2).map(String::as_str)) {
        Ok(()) => 0,
        Err(e) => {
            log_line(&format!("error: {e}"));
            1
        }
    }
}

// What the Java side asks about the game and tells it: Flipendo's touch controls (FlipendoActivity.Overlay) poll these
// through JNI natives declared in `io.github.flipendo.spike.FlipendoActivity`, which this library answers.

static IN_MENU: AtomicBool = AtomicBool::new(true);
static CUTSCENE: AtomicBool = AtomicBool::new(false);
static CREATURE: AtomicBool = AtomicBool::new(false);
static SURFACE_ALIVE: AtomicBool = AtomicBool::new(true);
static FRAMES: AtomicI32 = AtomicI32::new(0);
static FPS_LIMIT: AtomicI32 = AtomicI32::new(60);
static FIRST_PERSON: AtomicBool = AtomicBool::new(false);
static FOV_OFFSET: AtomicU32 = AtomicU32::new(0);
static UI_SCALE: AtomicU32 = AtomicU32::new(0x3F80_0000); // 1.0
static SAVED: AtomicI32 = AtomicI32::new(0);
static LEVEL: Mutex<String> = Mutex::new(String::new());

/// A menu (rather than play) is on screen: the touch controls hide their buttons.
pub fn set_in_menu(open: bool) {
    IN_MENU.store(open, Ordering::Relaxed);
}

/// A cutscene holds the player: the touch controls offer Skip and stop steering.
/// True while the game runs a cutscene.
pub fn in_cutscene() -> bool {
    CUTSCENE.load(Ordering::Relaxed)
}

pub fn set_cutscene(on: bool) {
    CUTSCENE.store(on, Ordering::Relaxed);
}

/// The player steers a creature (rabbit, dragon): the touch controls offer an Exit button (the Enter key).
pub fn set_creature(on: bool) {
    CREATURE.store(on, Ordering::Relaxed);
}

pub fn set_level(name: &str) {
    if let Ok(mut l) = LEVEL.lock()
        && *l != name
    {
        *l = name.to_string();
    }
}

/// Whether the window's surface exists (the activity says when it is destroyed or made).
pub fn surface_alive() -> bool {
    SURFACE_ALIVE.load(Ordering::Relaxed)
}

/// One more frame drawn (the FPS counter).
pub fn tick_frame() {
    FRAMES.fetch_add(1, Ordering::Relaxed);
}

/// The longest a frame may take, from Settings > FPS limit; None when the limit is the display's own.
pub fn frame_budget() -> Option<f32> {
    let fps = FPS_LIMIT.load(Ordering::Relaxed);
    (fps > 0 && fps < 120).then(|| 1.0 / fps as f32)
}

/// Degrees the player added to the field of view (Settings > Field of view).
pub fn fov_offset() -> f32 {
    f32::from_bits(FOV_OFFSET.load(Ordering::Relaxed))
}

/// How much bigger than the game lays them out the HUD and subtitles are drawn (Settings > Interface size).
pub fn ui_scale() -> f32 {
    f32::from_bits(UI_SCALE.load(Ordering::Relaxed))
}

/// A quick save was written: the activity shows a toast.
pub fn note_saved() {
    SAVED.fetch_add(1, Ordering::Relaxed);
}

type Env = *mut *const *const c_void;

macro_rules! native_bool {
    ($name:ident, $value:expr) => {
        #[unsafe(no_mangle)]
        pub extern "system" fn $name(_env: Env, _class: *mut c_void) -> u8 {
            $value as u8
        }
    };
}

macro_rules! native_unit {
    ($name:ident $(, $arg:ident : $ty:ty)*) => {
        #[unsafe(no_mangle)]
        pub extern "system" fn $name(_env: Env, _class: *mut c_void $(, $arg: $ty)*) {}
    };
}

native_bool!(Java_io_github_flipendo_spike_FlipendoActivity_nativeMenuActive, IN_MENU.load(Ordering::Relaxed));
native_bool!(Java_io_github_flipendo_spike_FlipendoActivity_nativeCutsceneActive, CUTSCENE.load(Ordering::Relaxed));
native_bool!(Java_io_github_flipendo_spike_FlipendoActivity_nativeCreatureActive, CREATURE.load(Ordering::Relaxed));
native_bool!(Java_io_github_flipendo_spike_FlipendoActivity_nativeLessonDrawing, false);
native_bool!(Java_io_github_flipendo_spike_FlipendoActivity_nativeVendorActive, false);
// A Quidditch match is the broom the overlay knows: it shows its broom layout (a steering stick that presses
// nothing, a boost button that toggles and a brake that is held; see `crate::broom`).
native_bool!(Java_io_github_flipendo_spike_FlipendoActivity_nativeOnBroom, crate::broom::on_broom());

native_unit!(Java_io_github_flipendo_spike_FlipendoActivity_nativeSetDebugMode, _on: u8);
native_unit!(Java_io_github_flipendo_spike_FlipendoActivity_nativeSetAutoJump, _on: u8);
native_unit!(Java_io_github_flipendo_spike_FlipendoActivity_nativeSetPerfLog, _on: u8);
native_unit!(Java_io_github_flipendo_spike_FlipendoActivity_nativeSetTouch, _x: f32, _y: f32, _down: u8);
native_unit!(Java_io_github_flipendo_spike_FlipendoActivity_nativeMenuAdjust, _index: i32, _dir: i32);
native_unit!(Java_io_github_flipendo_spike_FlipendoActivity_nativeSetLessonTimeScale, _scale: f32);

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_flipendo_spike_FlipendoActivity_nativeSetUiScale(_env: Env, _class: *mut c_void, scale: f32) {
    UI_SCALE.store(scale.clamp(0.3, 3.0).to_bits(), Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_flipendo_spike_FlipendoActivity_nativeSetFirstPerson(_env: Env, _class: *mut c_void, on: u8) {
    FIRST_PERSON.store(on != 0, Ordering::Relaxed);
}

/// Settings > First person: the camera sits in the player's head.
pub fn first_person() -> bool {
    FIRST_PERSON.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_flipendo_spike_FlipendoActivity_nativeSetFovOffset(_env: Env, _class: *mut c_void, degrees: f32) {
    FOV_OFFSET.store(degrees.to_bits(), Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_flipendo_spike_FlipendoActivity_nativeSetFpsLimit(_env: Env, _class: *mut c_void, fps: i32) {
    FPS_LIMIT.store(fps, Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_flipendo_spike_FlipendoActivity_nativeSetSurfaceAlive(_env: Env, _class: *mut c_void, alive: u8) {
    SURFACE_ALIVE.store(alive != 0, Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_flipendo_spike_FlipendoActivity_nativeFrameCount(_env: Env, _class: *mut c_void) -> i32 {
    FRAMES.load(Ordering::Relaxed)
}

/// The game's own menu items for the gamepad: none (the engine draws its menus itself).
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_flipendo_spike_FlipendoActivity_nativeMenuItems(_env: Env, _class: *mut c_void) -> *mut c_void {
    std::ptr::null_mut()
}

/// The quick save the touch controls' Save button asks for.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_flipendo_spike_FlipendoActivity_nativeRequestSave(_env: Env, _class: *mut c_void) {
    crate::commands::push("quicksave");
}

/// The map being played, for the controls' prompts.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_flipendo_spike_FlipendoActivity_nativeLevelName(env: Env, _class: *mut c_void) -> *mut c_void {
    if env.is_null() {
        return std::ptr::null_mut();
    }
    let name = LEVEL.lock().map(|l| l.clone()).unwrap_or_default();
    let Ok(name) = CString::new(name) else { return std::ptr::null_mut() };
    // JNI's function table: NewStringUTF is entry 167.
    type NewString = unsafe extern "system" fn(Env, *const c_char) -> *mut c_void;
    // SAFETY: `env` is the JNIEnv the VM passed in; its table has this entry at this index.
    unsafe {
        let new_string: NewString = std::mem::transmute(*(*env).add(167));
        new_string(env, name.as_ptr())
    }
}

/// A debug command from the settings panel's debug menu: `level <map>`, `cheat <name>` (see `commands`).
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_flipendo_spike_FlipendoActivity_nativeDebugCommand(env: Env, _class: *mut c_void, text: *mut c_void) {
    if env.is_null() || text.is_null() {
        return;
    }
    // JNI's function table: GetStringUTFChars is entry 169 and ReleaseStringUTFChars entry 170.
    type GetChars = unsafe extern "system" fn(Env, *mut c_void, *mut u8) -> *const c_char;
    type ReleaseChars = unsafe extern "system" fn(Env, *mut c_void, *const c_char);
    // SAFETY: as above.
    unsafe {
        let table = *env;
        let get: GetChars = std::mem::transmute(*table.add(169));
        let release: ReleaseChars = std::mem::transmute(*table.add(170));
        let chars = get(env, text, std::ptr::null_mut());
        if chars.is_null() {
            return;
        }
        let command = CStr::from_ptr(chars).to_string_lossy().into_owned();
        release(env, text, chars);
        crate::commands::push(command);
    }
}

/// `GameActivity.nativeSavedCount()`: how many quick saves have been written.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_ut2004_android_GameActivity_nativeSavedCount(_env: Env, _class: *mut c_void) -> i32 {
    SAVED.load(Ordering::Relaxed)
}

/// The phone's display runs at the game's own rate: no blending between ticks.
pub fn interpolate() -> bool {
    false
}

pub fn set_fps_limit(_fps: u32) {}

pub fn fps_limit() -> u32 {
    FPS_LIMIT.load(Ordering::Relaxed) as u32
}

/// The desktop's field of view keys do nothing on the phone (its slider sets the offset).
pub fn nudge_fov(_delta: f32) -> f32 {
    fov_offset()
}
