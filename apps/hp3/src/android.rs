//! The Android entry: SDL's activity loads this library and calls `SDL_main` with the game folder as the first
//! argument. What the game prints goes to logcat.

use std::ffi::{CStr, CString, c_char, c_int};
use std::io::{BufRead, BufReader};
use std::os::fd::FromRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

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

/// Whether a menu (rather than play) is on screen; the touch controls read it through JNI.
static IN_MENU: AtomicBool = AtomicBool::new(true);

pub fn set_in_menu(open: bool) {
    IN_MENU.store(open, Ordering::Relaxed);
}

/// `GameActivity.nativeInMenu()`: true while a menu takes the touches.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_ut2004_android_GameActivity_nativeInMenu(_env: *mut std::ffi::c_void, _class: *mut std::ffi::c_void) -> u8 {
    IN_MENU.load(Ordering::Relaxed) as u8
}

/// `GameActivity.nativeCommand(String)`: a command from the touch menus, for the game loop (see `commands`).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_ut2004_android_GameActivity_nativeCommand(env: *mut *const *const std::ffi::c_void, _class: *mut std::ffi::c_void, text: *mut std::ffi::c_void) {
    if env.is_null() || text.is_null() {
        return;
    }
    // JNI's function table: GetStringUTFChars is entry 169 and ReleaseStringUTFChars entry 170.
    type GetChars = unsafe extern "system" fn(*mut *const *const std::ffi::c_void, *mut std::ffi::c_void, *mut u8) -> *const c_char;
    type ReleaseChars = unsafe extern "system" fn(*mut *const *const std::ffi::c_void, *mut std::ffi::c_void, *const c_char);
    // SAFETY: `env` is the JNIEnv pointer the VM passes in; its table has these entries at these indices.
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
