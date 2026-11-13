//! Graphics options the player can change while the game runs (the shared Settings view, `gfx` commands): the characters'
//! shadow and its quality tier, anti-aliasing, tone mapping, the characters' rim light, height fog and soft particles. Plain
//! global switches the render passes read each frame; the first read takes its starting values from the environment
//! (`HP3_ACTOR_SHADOWS=0`, `HP3_SHADOW_QUALITY=low|high`, `HP3_AA=off|fxaa|msaa`, `HP3_TONEMAP=0|1`, `HP3_RIM=0|1`,
//! `HP3_HEIGHT_FOG=0|1`, `HP3_SOFT_PARTICLES=0|1`).
//!
//! Two shadow tiers: Low is the phone's single shadow map; High (the PC's default) nests several maps of growing reach around the player.

use std::sync::Once;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering::Relaxed};

static START: Once = Once::new();
static SHADOWS: AtomicBool = AtomicBool::new(true);
static HIGH: AtomicBool = AtomicBool::new(!cfg!(target_os = "android"));
static AA: AtomicU8 = AtomicU8::new(Aa::Msaa4 as u8);
static TONEMAP: AtomicBool = AtomicBool::new(true);
static RIM: AtomicBool = AtomicBool::new(true);
static HEIGHT_FOG: AtomicBool = AtomicBool::new(true);
// The one effect that costs an extra pass (the particles are drawn after the scene, reading its depth): off on the phone by default.
static SOFT: AtomicBool = AtomicBool::new(!cfg!(target_os = "android"));

/// How edges are smoothed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Aa {
    Off = 0,
    /// A blur along contrast edges when the frame is shown (one full-screen pass).
    Fxaa = 1,
    /// Four samples a pixel, resolved at the end of the scene pass (nearly free on a phone's tiled GPU).
    Msaa4 = 2,
}

impl Aa {
    pub fn label(self) -> &'static str {
        match self {
            Aa::Off => "OFF",
            Aa::Fxaa => "FXAA",
            Aa::Msaa4 => "MSAA 4X",
        }
    }

    /// The next one in the Settings view's cycle.
    pub fn next(self) -> Aa {
        match self {
            Aa::Off => Aa::Fxaa,
            Aa::Fxaa => Aa::Msaa4,
            Aa::Msaa4 => Aa::Off,
        }
    }

    pub fn parse(text: &str) -> Option<Aa> {
        match text.to_ascii_lowercase().as_str() {
            "off" | "0" | "none" | "false" => Some(Aa::Off),
            "fxaa" => Some(Aa::Fxaa),
            "msaa" | "msaa4" | "4" | "on" | "true" => Some(Aa::Msaa4),
            _ => None,
        }
    }
}

fn env_flag(name: &str, flag: &AtomicBool) {
    match std::env::var(name).as_deref() {
        Ok("0" | "off" | "false") => flag.store(false, Relaxed),
        Ok(_) => flag.store(true, Relaxed),
        Err(_) => {}
    }
}

fn start() {
    START.call_once(|| {
        if std::env::var("HP3_ACTOR_SHADOWS").is_ok_and(|v| v == "0") {
            SHADOWS.store(false, Relaxed);
        }
        match std::env::var("HP3_SHADOW_QUALITY").as_deref() {
            Ok("low") => HIGH.store(false, Relaxed),
            Ok("high") => HIGH.store(true, Relaxed),
            _ => {}
        }
        // The older switch: HP3_FXAA=1 asks for FXAA, HP3_FXAA=0 for none.
        match std::env::var("HP3_FXAA").as_deref() {
            Ok("0" | "off" | "false") => AA.store(Aa::Off as u8, Relaxed),
            Ok(_) => AA.store(Aa::Fxaa as u8, Relaxed),
            Err(_) => {}
        }
        if let Some(a) = std::env::var("HP3_AA").ok().as_deref().and_then(Aa::parse) {
            AA.store(a as u8, Relaxed);
        }
        env_flag("HP3_TONEMAP", &TONEMAP);
        env_flag("HP3_RIM", &RIM);
        env_flag("HP3_HEIGHT_FOG", &HEIGHT_FOG);
        env_flag("HP3_SOFT_PARTICLES", &SOFT);
    });
}

/// Whether the characters cast real shadows (otherwise only their blob).
pub fn actor_shadows() -> bool {
    start();
    SHADOWS.load(Relaxed)
}

pub fn set_actor_shadows(on: bool) {
    start();
    SHADOWS.store(on, Relaxed);
}

/// Whether the shadow takes the high tier (nested maps). The phone starts on Low (it costs two more shadow passes a frame) but
/// the Settings view can pick High, as the PC can.
pub fn high_shadows() -> bool {
    start();
    HIGH.load(Relaxed)
}

pub fn set_high_shadows(on: bool) {
    start();
    HIGH.store(on, Relaxed);
}

/// How many nested maps the shadow uses now.
pub fn cascades() -> usize {
    if high_shadows() { crate::shadow::ACTOR_LAYERS as usize } else { 1 }
}

/// The anti-aliasing asked for (MSAA falls back to FXAA where the GPU cannot do it: see `Renderer::msaa_supported`).
pub fn aa() -> Aa {
    start();
    match AA.load(Relaxed) {
        0 => Aa::Off,
        1 => Aa::Fxaa,
        _ => Aa::Msaa4,
    }
}

pub fn set_aa(a: Aa) {
    start();
    AA.store(a as u8, Relaxed);
}

static MSAA_OK: AtomicBool = AtomicBool::new(false);

/// Whether the GPU can draw the frame's colour and depth with four samples (the renderer says so when it opens the device).
pub fn set_msaa_supported(ok: bool) {
    MSAA_OK.store(ok, Relaxed);
}

pub fn msaa_supported() -> bool {
    MSAA_OK.load(Relaxed)
}

static GL: AtomicBool = AtomicBool::new(false);

/// The renderer runs on OpenGL (ES): there, with MSAA on, soft particles cannot read the depth and are drawn hard.
pub fn set_backend_gl(on: bool) {
    GL.store(on, Relaxed);
}

/// Whether the particles are soft right now (asked for, and possible with this anti-aliasing and graphics API).
pub fn soft_particles_active() -> bool {
    soft_particles() && !(GL.load(Relaxed) && msaa_samples() > 1)
}

/// The samples a pixel the scene is drawn with now: 4 for MSAA where the GPU has it, else 1.
pub fn msaa_samples() -> u32 {
    if aa() == Aa::Msaa4 && msaa_supported() { 4 } else { 1 }
}

/// Whether the shown frame goes through FXAA now: asked for, or MSAA asked for on a GPU without it.
pub fn fxaa_now() -> bool {
    match aa() {
        Aa::Fxaa => true,
        Aa::Msaa4 => !msaa_supported(),
        Aa::Off => false,
    }
}

/// Highlights roll off instead of clipping to white, and colours are a little richer (in the shaders' last step, no extra pass).
pub fn tonemap() -> bool {
    start();
    TONEMAP.load(Relaxed)
}

pub fn set_tonemap(on: bool) {
    start();
    TONEMAP.store(on, Relaxed);
}

/// A soft light on the characters' edges turned away from the viewer, which lifts them off dark backgrounds.
pub fn rim_light() -> bool {
    start();
    RIM.load(Relaxed)
}

pub fn set_rim_light(on: bool) {
    start();
    RIM.store(on, Relaxed);
}

/// In zones that have distance fog, a mist that thickens below the viewer's height.
pub fn height_fog() -> bool {
    start();
    HEIGHT_FOG.load(Relaxed)
}

pub fn set_height_fog(on: bool) {
    start();
    HEIGHT_FOG.store(on, Relaxed);
}

/// Particles fade where they meet a surface instead of cutting into it along a hard line (an extra pass that reads the depth).
pub fn soft_particles() -> bool {
    start();
    SOFT.load(Relaxed)
}

pub fn set_soft_particles(on: bool) {
    start();
    SOFT.store(on, Relaxed);
}

/// The options packed for the phone's Java side: bit 0 the characters' shadow (bits 1 and 2 belong to the Java side, bit 3 is the
/// shadow's high tier).
pub fn packed() -> i32 {
    actor_shadows() as i32 | (high_shadows() as i32) << 3
}
