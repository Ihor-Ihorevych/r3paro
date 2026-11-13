//! The shader pieces every scene shader shares: the camera's uniform block and the last steps of a fragment (distance fog,
//! height fog, tone mapping), plus where each draw's depth goes in the depth buffer.
//!
//! The uniform block (`Globals`, written by `Renderer::draw_target`), as vec4 rows after the matrix:
//! - `fog_color`, `fog`: start, end, on (0/1), how many moving lights follow (the world shader's).
//! - `cam`: the eye's position and the far plane.
//! - `fx`: tone mapping on, rim light on, height fog on, how far (units) a soft particle fades into what is behind it.
//! - `hfog`: the height fog's base height, falloff (units), density at the base (per unit), most it covers (0..1).
//! - `dr`: depth range (offset, scale) this draw's depth is squeezed into, the projection's near plane, MSAA on (0/1).
//!
//! The depth range lets the sky zone and the level share one pass (on a phone's tiled GPU a second pass stores and reloads the
//! whole picture, four times over with MSAA): the sky is drawn into the last hundredth of the depth buffer, the level in front of it.

/// Floats in the shared part of the uniform block (the world shader's moving lights come after).
pub const GLOBALS_FLOATS: usize = 16 + 6 * 4;

/// The level's draws take depths `0..LEVEL_DEPTH`, the sky zone's the rest.
pub const LEVEL_DEPTH: f32 = 0.99;

/// The projection's near plane (`Camera::view_proj_far`).
pub const NEAR: f32 = 4.0;

/// The uniform block's fields up to (not including) the world shader's lights.
pub const GLOBALS_FIELDS: &str = "view_proj: mat4x4<f32>, fog_color: vec4<f32>, fog: vec4<f32>, cam: vec4<f32>, fx: vec4<f32>, hfog: vec4<f32>, dr: vec4<f32>";

/// The helper functions (they read `globals`).
pub const FX_WGSL: &str = r#"
// How much of the picture's own color is left at a fragment (1 outside the fog): linear fog by depth from `fog.x` to `fog.y`.
// `w` is the fragment position's w (one over the distance along the view).
fn fog_factor(w: f32) -> f32 {
    if (globals.fog.z < 0.5) { return 1.0; }
    return clamp((globals.fog.y - 1.0 / w) / max(globals.fog.y - globals.fog.x, 1.0), 0.0, 1.0);
}
// A clip position with its depth put in this draw's slice of the depth buffer (linear in clip space, so no division).
fn squeeze(p: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(p.x, p.y, globals.dr.x * p.w + globals.dr.y * p.z, p.w);
}
// How much of a fragment the height fog covers: thick below the base height (a little under the eye), thinning out above
// it, and growing with distance. Only where the zone has distance fog (its colour is the mist's).
fn height_fog(world: vec3<f32>) -> f32 {
    if (globals.fx.z < 0.5 || globals.fog.z < 0.5) { return 0.0; }
    let d = distance(world, globals.cam.xyz);
    let h = max(world.z - globals.hfog.x, 0.0);
    let density = globals.hfog.z * exp(-h / globals.hfog.y);
    return min(1.0 - exp(-d * density), globals.hfog.w);
}
// Tone mapping: colours a little richer, and above the knee the brightest channel rolls off toward 1 with the hue kept, so a
// strongly lit surface keeps its texture instead of clipping to white. Below the knee (most of the picture) nothing changes
// but the saturation.
fn tonemap(c: vec3<f32>) -> vec3<f32> {
    if (globals.fx.x < 0.5) { return c; }
    let l = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
    var v = max(mix(vec3<f32>(l), c, 1.08), vec3<f32>(0.0));
    let knee = 0.7;
    let m = max(max(v.r, v.g), v.b);
    if (m > knee) {
        let s = knee + (1.0 - knee) * (1.0 - exp(-(m - knee) / (1.0 - knee)));
        v = v * (s / m);
    }
    return v;
}
// A cut-out surface's coverage from its texture's alpha (call it before any discard: it takes the screen derivative). Without
// MSAA it is all or nothing at 0.5, as the original's alpha test; with MSAA the 0.5 edge is spread over about a pixel and
// handed to alpha to coverage, so leaves, hair and railings get smooth edges too. 0 means the fragment is thrown away.
fn coverage(a: f32) -> f32 {
    if (globals.dr.w < 0.5) { return select(0.0, 1.0, a >= 0.5); }
    return clamp((a - 0.5) / max(fwidth(a), 0.0001) + 0.5, 0.0, 1.0);
}
// The last step of an opaque surface: distance fog, height fog, tone mapping.
fn finish(rgb: vec3<f32>, world: vec3<f32>, w: f32) -> vec3<f32> {
    let c = mix(globals.fog_color.rgb, rgb, fog_factor(w));
    return tonemap(mix(c, globals.fog_color.rgb, height_fog(world)));
}
"#;

/// The uniform block declaration and helpers for a shader that reads only the shared part.
pub fn globals_wgsl() -> String {
    format!("struct Globals {{ {GLOBALS_FIELDS} }};\n@group(0) @binding(0) var<uniform> globals: Globals;\n{FX_WGSL}")
}

/// The shared part of the uniform block for one frame (`fog`: start, end, on; the light count is the world shader's to set).
pub struct Shared {
    pub view_proj: glam::Mat4,
    pub fog: Option<crate::scene::Fog>,
    pub eye: glam::Vec3,
    pub far: f32,
    /// The sky zone's camera: drawn behind the level, no fog of its own.
    pub sky: bool,
    /// The pass draws four samples a pixel: cut-out surfaces hand their edge to alpha to coverage (`dr.w`).
    pub msaa: bool,
}

impl Shared {
    pub fn write(&self, out: &mut [f32]) {
        use crate::quality;
        out[..16].copy_from_slice(&self.view_proj.to_cols_array());
        if let Some(f) = self.fog.filter(|_| !self.sky) {
            out[16..19].copy_from_slice(&f.color.to_array());
            out[20..23].copy_from_slice(&[f.start, f.end, 1.0]);
            // The height fog follows the zone's own fog: below the base it reaches 60% at the fog's end distance, capped at 40%.
            out[32..36].copy_from_slice(&[self.eye.z - 200.0, 260.0, 0.9 / f.end.max(256.0), 0.4]);
        }
        out[24..28].copy_from_slice(&[self.eye.x, self.eye.y, self.eye.z, self.far]);
        let on = |b: bool| if b { 1.0 } else { 0.0 };
        out[28..32].copy_from_slice(&[on(quality::tonemap()), on(quality::rim_light()), on(quality::height_fog() && !self.sky), 24.0]);
        let (offset, scale) = if self.sky { (LEVEL_DEPTH, 1.0 - LEVEL_DEPTH) } else { (0.0, LEVEL_DEPTH) };
        out[36..40].copy_from_slice(&[offset, scale, NEAR, on(self.msaa)]);
    }
}
