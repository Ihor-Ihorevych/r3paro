//! Ties the running world to the renderer: which actors are drawn and with which mesh and transform.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use glam::{Mat4, Vec3};
use hp3_assets::loader::{self, Loader, ObjKey};
use hp3_package::{ObjRef, Package};
mod actor_light;
mod fog;
pub use fog::{debug_zone, still_fog, view_fog, FogState};

use hp3_render::dynamic::{ActorLight, Dynamic, GpuMesh};
use hp3_render::gpu::Renderer;
use hp3_render::meshes::{self, MeshGeom};
use hp3_render::scene::{rotation_matrix, ClassStatic, DecoLayer, FluidSurface, LightSource, Lighting, MapInputs, TerrainLayer};
use hp3_script::{ObjId, Value, Vm};
use hp3_world::{get, world};

const DT_MESH: u8 = 2;
const DT_STATIC_MESH: u8 = 8;

/// Export indices of the map's static actors: baked into the level scene instead of drawn per frame.
pub fn static_exports(vm: &mut Vm<'_>, map: &Package) -> HashSet<usize> {
    let mut out = HashSet::new();
    let actors = world(vm).actors.clone();
    for a in actors {
        let Some((pkg, index)) = vm.db.objects.get(a).key.clone() else { continue };
        if pkg.as_ref() == map.name.to_ascii_lowercase() && get(vm, a, "bStatic").as_bool() && hp3_world::rd(vm, a, "LumosType").as_int() == 0 {
            out.insert(index);
        }
    }
    out
}

/// `LightEffect` of sunlights in HP3 (the `Sunlight` class default).
pub(crate) const LE_SUNLIGHT: u8 = 19;

/// Unreal's byte hue, saturation and brightness as a color, the way the original's `FGetHSV` reads them: the hue runs
/// red -> green -> blue -> red in three straight blends (the channels of a pure hue add up to 1), saturation 255 means
/// white, and the brightness goes through a curve that gives about 0.82 at 255.
pub(crate) fn unreal_hsv(hue: u8, saturation: u8, value: u8) -> Vec3 {
    let h = hue as f32;
    let pure = if hue < 86 {
        Vec3::new((85.0 - h) / 85.0, h / 85.0, 0.0)
    } else if hue < 171 {
        Vec3::new(0.0, (170.0 - h) / 85.0, (h - 85.0) / 85.0)
    } else {
        Vec3::new((h - 170.0) / 85.0, 0.0, (255.0 - h) / 84.0)
    };
    let b = value as f32 * 1.4 / 255.0;
    let b = (b * 0.7 / (0.01 + b.sqrt())).clamp(0.0, 1.0);
    (pure + (Vec3::ONE - pure) * (saturation as f32 / 255.0)) * b
}

/// A light's color at full brightness (`LightBrightness` and the level's `Brightness` scale it further).
pub(crate) fn light_color(hue: u8, saturation: u8) -> Vec3 {
    unreal_hsv(hue, saturation, 255)
}

/// The level's overall light scale (`LevelInfo.Brightness`), which every light's color is multiplied by.
pub(crate) fn level_brightness(vm: &mut Vm<'_>) -> f32 {
    match world(vm).level_info {
        Some(li) => get(vm, li, "Brightness").as_float(),
        None => 1.0,
    }
}

#[cfg(test)]
mod hsv_tests {
    use super::unreal_hsv;

    fn close(a: glam::Vec3, b: [f32; 3]) -> bool {
        (a - glam::Vec3::from(b)).abs().max_element() < 0.005
    }

    #[test]
    fn full_value_is_about_0_82() {
        assert!(close(unreal_hsv(0, 0, 255), [0.8213, 0.0, 0.0]));
        assert!(close(unreal_hsv(0, 255, 255), [0.8213, 0.8213, 0.8213]));
        assert!(close(unreal_hsv(0, 0, 0), [0.0, 0.0, 0.0]));
    }

    #[test]
    fn hue_blends_in_three_parts() {
        // An orange torch: red 53/85, green 32/85 before saturation.
        let c = unreal_hsv(32, 128, 255) / 0.8213;
        let s = 128.0 / 255.0;
        assert!(close(c, [53.0 / 85.0 + s * 32.0 / 85.0, 32.0 / 85.0 + s * 53.0 / 85.0, s]));
        assert!(close(unreal_hsv(170, 0, 255) / 0.8213, [0.0, 0.0, 1.0]));
        assert!(close(unreal_hsv(255, 0, 255) / 0.8213, [1.0, 0.0, 0.0]));
    }
}

/// The map's lights and ambient light, read from the running world (class defaults applied).
pub fn lighting(vm: &mut Vm<'_>, map: &Package) -> Lighting {
    let mut out = Lighting::default();
    let map_name = map.name.to_ascii_lowercase();
    let level_scale = level_brightness(vm);
    let actors = world(vm).actors.clone();
    for a in actors {
        let Some((pkg, index)) = vm.db.objects.get(a).key.clone() else { continue };
        if pkg.as_ref() != map_name || get(vm, a, "LightType").as_byte() == 0 {
            continue;
        }
        let brightness = get(vm, a, "LightBrightness").as_float() / 255.0;
        if brightness <= 0.0 {
            continue;
        }
        let color = light_color(get(vm, a, "LightHue").as_byte(), get(vm, a, "LightSaturation").as_byte()) * brightness * level_scale;
        let position = vec3(&get(vm, a, "Location"));
        let radius = 25.0 * (get(vm, a, "LightRadius").as_float() + 1.0);
        let direction = (get(vm, a, "LightEffect").as_byte() == LE_SUNLIGHT).then(|| {
            let r: Vec<i32> = get(vm, a, "Rotation").fields().iter().map(Value::as_int).collect();
            let m = rotation_matrix([r.first().copied().unwrap_or(0), r.get(1).copied().unwrap_or(0), 0]);
            m.transform_vector3(Vec3::X)
        });
        if std::env::var_os("HP3_LIGHT_DEBUG").is_some() && get(vm, a, "bDynamicLight").as_bool() {
            eprintln!("dynamic light {} effect {} at {}", hp3_script::natives::object_path(vm, a), get(vm, a, "LightEffect").as_byte(), position);
        }
        let aim = {
            let r: Vec<i32> = get(vm, a, "Rotation").fields().iter().map(Value::as_int).collect();
            rotation_matrix([r.first().copied().unwrap_or(0), r.get(1).copied().unwrap_or(0), 0]).transform_vector3(Vec3::X)
        };
        let effect = get(vm, a, "LightEffect").as_byte();
        let cone = get(vm, a, "LightCone").as_float();
        out.lights.insert(index, LightSource { position, color, radius, direction, effect, cone, aim });
    }
    // A zone's ambient light comes from its hue, saturation and brightness, worked out when the level starts as the
    // original does (the `AmbientVector` saved in the map is the editor's copy and is missing on some zones).
    let (zone, sky) = (vm.db.find("ZoneInfo"), vm.db.find("SkyZoneInfo"));
    let actors = world(vm).actors.clone();
    let mut sum = (Vec3::ZERO, 0);
    for a in actors {
        let Some(class) = vm.db.objects.get(a).class else { continue };
        if !zone.is_some_and(|z| vm.is_child_of(class, z)) {
            continue;
        }
        let Some((pkg, index)) = vm.db.objects.get(a).key.clone() else { continue };
        if pkg.as_ref() != map_name {
            continue;
        }
        let ambient = unreal_hsv(get(vm, a, "AmbientHue").as_byte(), get(vm, a, "AmbientSaturation").as_byte(), get(vm, a, "AmbientBrightness").as_byte());
        if std::env::var_os("HP3_LIGHT_DEBUG").is_some() {
            eprintln!("zone {} ambient {ambient} (saved {})", hp3_script::natives::object_path(vm, a), vec3(&get(vm, a, "AmbientVector")));
        }
        out.zone_ambient.insert(index, ambient);
        if ambient != Vec3::ZERO && !sky.is_some_and(|s| vm.is_child_of(class, s)) {
            sum = (sum.0 + ambient, sum.1 + 1);
        }
    }
    if sum.1 > 0 {
        out.ambient = sum.0 / sum.1 as f32;
    }
    out
}

/// The texture a material object shows, as a loader key.
fn texture_key(vm: &Vm<'_>, loader: &Loader<'_>, material: ObjId) -> Option<ObjKey> {
    let (pkg, index) = export_of(vm, material)?;
    let (tp, ti) = loader.base_texture(&pkg, ObjRef::from_export(index))?;
    Some(loader::key(&tp, ti))
}

/// Everything the scene builder takes from the running world: lighting (unless `lit` is false) and the
/// layers of each terrain.
pub fn map_inputs(vm: &mut Vm<'_>, loader: &Loader<'_>, map: &Package, lit: bool) -> MapInputs {
    let mut out = MapInputs { lighting: lit.then(|| lighting(vm, map)), ..Default::default() };
    let map_name = map.name.to_ascii_lowercase();
    let actors = world(vm).actors.clone();
    if let Some(fluid_class) = vm.db.find("FluidSurfaceInfo") {
        for &a in &actors {
            let Some(class) = vm.db.objects.get(a).class else { continue };
            if !vm.is_child_of(class, fluid_class) || vm.db.objects.get(a).deleted || hp3_world::get(vm, a, "bHidden").as_bool() {
                continue;
            }
            let skin = match hp3_world::get(vm, a, "Skins") {
                Value::Array(v) => v.first().and_then(Value::as_obj),
                _ => None,
            };
            let Some(texture) = skin.and_then(|s| texture_key(vm, loader, s)) else { continue };
            let at = hp3_world::physics::vec(&hp3_world::get(vm, a, "Location"));
            let yaw = hp3_world::get(vm, a, "Rotation").fields().get(1).map_or(0, Value::as_int);
            let size = |vm: &mut Vm<'_>, n: &str| hp3_world::get(vm, a, n).as_int().max(0) as usize;
            let tile = |vm: &mut Vm<'_>, n: &str| match hp3_world::get(vm, a, n).as_float() {
                t if t > 0.0 => t,
                _ => 1.0,
            };
            out.fluids.push(FluidSurface {
                texture,
                at: at.to_array(),
                yaw,
                nx: size(vm, "FluidXSize"),
                ny: size(vm, "FluidYSize"),
                spacing: hp3_world::get(vm, a, "FluidGridSpacing").as_float(),
                tiles: [tile(vm, "UTiles"), tile(vm, "VTiles")],
            });
        }
    }
    // Static actors of other classes than StaticMeshActor (SmartDesk, ...) take their mesh from class defaults: the
    // map's export has no tag for it, so the scene builder is given the running world's values.
    let plain = vm.db.find("StaticMeshActor");
    for &a in &actors {
        let Some(class) = vm.db.objects.get(a).class else { continue };
        let Some((pkg, index)) = vm.db.objects.get(a).key.clone() else { continue };
        if pkg.as_ref() != map_name || vm.db.objects.get(a).deleted || plain.is_some_and(|p| vm.is_child_of(class, p)) {
            continue;
        }
        if hp3_world::get(vm, a, "DrawType").as_byte() != DT_STATIC_MESH
            || !hp3_world::get(vm, a, "bStatic").as_bool()
            || hp3_world::get(vm, a, "bHidden").as_bool()
            || hp3_world::get(vm, a, "bSpecialHidden").as_bool()
        {
            continue;
        }
        let Some(mesh) = hp3_world::get(vm, a, "StaticMesh").as_obj().and_then(|m| export_of(vm, m)) else { continue };
        let scale = match hp3_world::rd(vm, a, "DrawScale").as_float() {
            0.0 => 1.0,
            s => s,
        };
        let s3 = match hp3_world::rd(vm, a, "DrawScale3D") {
            v @ Value::Struct(_) => vec3(&v),
            _ => Vec3::ONE,
        };
        let s3 = if s3 == Vec3::ZERO { Vec3::ONE } else { s3 };
        let prepivot = hp3_world::rd_vec(vm, a, "PrePivot");
        out.class_statics.insert(index, ClassStatic { mesh: loader::key(&mesh.0, mesh.1), scale, scale3d: s3.to_array(), prepivot: prepivot.to_array() });
    }
    let Some(terrain_class) = vm.db.find("TerrainInfo") else { return out };
    for a in actors {
        let Some(class) = vm.db.objects.get(a).class else { continue };
        let Some((pkg, index)) = vm.db.objects.get(a).key.clone() else { continue };
        if !vm.is_child_of(class, terrain_class) || pkg.as_ref() != map_name {
            continue;
        }
        if std::env::var_os("HP3_TERRAIN_DEBUG").is_some()
            && let Some(ds) = hp3_world::slot(vm, a, "DecoLayers")
            && let Some(Value::Array(items)) = vm.db.objects.values(a).and_then(|v| v.get(ds)).cloned()
        {
            for (n, it) in items.iter().enumerate() {
                let desc: Vec<String> = it
                    .fields()
                    .iter()
                    .map(|v| match v {
                        Value::Object(Some(o)) => hp3_script::natives::object_path(vm, *o),
                        v => format!("{v:?}"),
                    })
                    .collect();
                eprintln!("terrain {index} deco {n}: {desc:?}");
            }
        }
        let Some(slot) = hp3_world::slot(vm, a, "Layers") else { continue };
        let mut layers = Vec::new();
        for i in 0..32 {
            let Some(layer) = vm.db.objects.values(a).and_then(|v| v.get(slot + i)).cloned() else { break };
            let f = layer.fields().to_vec();
            // The drawn layers end at the first slot that lacks a texture or an alpha map, as in the original's
            // terrain renderer: what comes after a gap is an editor helper (COMC's red-and-white deco layers).
            if f.first().and_then(Value::as_obj).is_none() || f.get(1).and_then(Value::as_obj).is_none() {
                break;
            }
            let Some(texture) = f.first().and_then(Value::as_obj).and_then(|t| texture_key(vm, loader, t)) else { continue };
            let alpha_map = f.get(1).and_then(Value::as_obj).and_then(|t| texture_key(vm, loader, t));
            if std::env::var_os("HP3_TERRAIN_DEBUG").is_some() {
                let name = |vm: &Vm<'_>, v: Option<&Value>| v.and_then(Value::as_obj).map(|o| hp3_script::natives::object_path(vm, o));
                eprintln!("terrain {index} layer {i}: texture {:?} alpha {:?} fields {:?}", name(vm, f.first()), name(vm, f.get(1)), &f[2..f.len().min(9)]);
            }
            let mut matrix = [[0.0f32; 4]; 4];
            if let Some(m) = f.get(9) {
                for (r, plane) in m.fields().iter().take(4).enumerate() {
                    for (c, v) in plane.fields().iter().take(4).enumerate() {
                        matrix[r][c] = v.as_float();
                    }
                }
            }
            layers.push(TerrainLayer { texture, alpha_map, matrix });
        }
        out.terrain_layers.insert(index, layers);
        if let Some(ds) = hp3_world::slot(vm, a, "DecoLayers")
            && let Some(Value::Array(items)) = vm.db.objects.values(a).and_then(|v| v.get(ds)).cloned()
        {
            let range = |v: Option<&Value>| v.map(|r| [r.fields().first().map_or(0.0, Value::as_float), r.fields().get(1).map_or(0.0, Value::as_float)]).unwrap_or_default();
            let mut deco = Vec::new();
            for it in &items {
                // DecorationLayer: ShowOnTerrain, ScaleMap, DensityMap, ColorMap, StaticMesh, ScaleMultiplier, FadeoutRadius,
                // DensityMultiplier, MaxPerQuad, Seed, AlignToTerrain, DrawOrder, ShowOnInvisibleTerrain, LitDirectional,
                // DisregardTerrainLighting, RandomYaw.
                let f = it.fields();
                if f.first().map_or(0, Value::as_int) == 0 {
                    continue;
                }
                let Some(density_map) = f.get(2).and_then(Value::as_obj).and_then(|t| texture_key(vm, loader, t)) else { continue };
                let Some(mesh) = f.get(4).and_then(Value::as_obj).and_then(|m| export_of(vm, m)) else { continue };
                let scale = f.get(5).map(|s| s.fields().iter().map(|r| range(Some(r))).collect::<Vec<_>>()).unwrap_or_default();
                if scale.len() < 3 {
                    continue;
                }
                let flag = |n: usize| f.get(n).is_some_and(|v| v.as_int() != 0);
                deco.push(DecoLayer {
                    mesh: loader::key(&mesh.0, mesh.1),
                    density_map,
                    scale: [scale[0], scale[1], scale[2]],
                    density: range(f.get(7)),
                    max_per_quad: f.get(8).map_or(0, Value::as_int),
                    seed: f.get(9).map_or(0, Value::as_int),
                    align_to_terrain: flag(10),
                    show_on_invisible: flag(12),
                    flat_light: flag(14),
                    random_yaw: flag(15),
                });
            }
            out.deco_layers.insert(index, deco);
        }
    }
    out
}

/// Textures of 2D materials with their sizes, by material object.
#[derive(Default)]
pub struct HudCache {
    textures: HashMap<ObjId, Option<(ObjKey, f32, f32)>>,
    /// Textures made at run time (native font pages), under keys of the `<runtime>` package.
    runtime: HashMap<ObjKey, Rc<hp3_assets::loader::Image>>,
}

impl HudCache {
    /// The image behind an overlay texture key: a runtime texture or a package texture with its alpha.
    pub fn image(&self, loader: &Loader<'_>, k: &ObjKey) -> Option<Rc<hp3_assets::loader::Image>> {
        if let Some(i) = self.runtime.get(k) {
            return Some(i.clone());
        }
        loader.image_with_alpha(&loader.set.get(&k.0).ok()?, k.1)
    }
}

/// The HUD of this frame as overlay quads (and the key-to-image lookup the overlay needs).
pub fn hud_quads(vm: &mut Vm<'_>, loader: &Loader<'_>, cache: &mut HudCache, width: u32, height: u32) -> Vec<hp3_render::overlay::Quad> {
    use hp3_render::overlay::{Blend, Quad};
    use hp3_world::canvas::{STY_ALPHA, STY_MODULATED, STY_TRANSLUCENT};
    let tiles = hp3_world::canvas::render(vm, width, height);
    let mut out = Vec::with_capacity(tiles.len());
    for t in tiles {
        // Tiles without a material (or one that does not resolve to a texture) draw nothing, as in Unreal.
        let Some(material) = t.material else { continue };
        if !cache.textures.contains_key(&material)
            && let Some(img) = world(vm).native_fonts.images.get(&material).cloned()
        {
            let k: ObjKey = ("<runtime>".to_string(), material.0 as usize);
            cache.textures.insert(material, Some((k.clone(), img.width as f32, img.height as f32)));
            cache.runtime.insert(k, img);
        }
        let tex = Some(material).and_then(|m| {
            cache
                .textures
                .entry(m)
                .or_insert_with(|| {
                    let k = texture_key(vm, loader, m)?;
                    let img = loader.image_with_alpha(&loader.set.get(&k.0).ok()?, k.1)?;
                    Some((k, img.width as f32, img.height as f32))
                })
                .clone()
        });
        let Some(tex) = tex else { continue };
        let (tw, th) = (tex.1, tex.2);
        // Negative sizes are scales of the texture's size (`DrawTileScaled`); negative extents the whole texture.
        let w = if t.w < 0.0 { tw * -t.w } else { t.w };
        let h = if t.h < 0.0 { th * -t.h } else { t.h };
        let (ul, vl) = if t.ul < 0.0 { (tw, th) } else { (t.ul, t.vl) };
        let blend = match t.style {
            STY_TRANSLUCENT => Blend::Additive,
            STY_MODULATED => Blend::Modulate,
            _ => Blend::Alpha,
        };
        let c = t.color;
        let alpha = if t.style == STY_ALPHA { c[3] as f32 / 255.0 } else { 1.0 };
        out.push(Quad {
            texture: Some(tex.0),
            rect: [t.x, t.y, w, h],
            uv: [t.u / tw, t.v / th, (t.u + ul) / tw, (t.v + vl) / th],
            color: [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, alpha],
            blend,
        });
    }
    out
}

/// The screen flash over everything: `FlashScale` darkens or tints the picture, `FlashFog` adds a color (fades
/// to black or white in cutscenes). Full-screen quads to append after the HUD.
pub fn screen_flash(vm: &mut Vm<'_>, width: u32, height: u32) -> Vec<hp3_render::overlay::Quad> {
    use hp3_render::overlay::{Blend, Quad};
    let Some(pc) = world(vm).player else { return Vec::new() };
    let scale = vec3(&get(vm, pc, "FlashScale"));
    let fog = vec3(&get(vm, pc, "FlashFog"));
    let rect = [0.0, 0.0, width as f32, height as f32];
    let mut out = Vec::new();
    if scale != Vec3::ONE && scale.max_element() >= 0.0 {
        // Modulated blending doubles: dst * src * 2.
        let c = (scale.clamp(Vec3::ZERO, Vec3::splat(2.0)) * 0.5).extend(1.0).to_array();
        out.push(Quad { texture: None, rect, uv: [0.0, 0.0, 1.0, 1.0], color: c, blend: Blend::Modulate });
    }
    if fog.max_element() > 0.0 {
        let c = fog.clamp(Vec3::ZERO, Vec3::ONE).extend(1.0).to_array();
        out.push(Quad { texture: None, rect, uv: [0.0, 0.0, 1.0, 1.0], color: c, blend: Blend::Additive });
    }
    out
}

/// The package export behind an object, if it came from a package.
fn export_of(vm: &Vm<'_>, o: ObjId) -> Option<(Rc<Package>, usize)> {
    let (pkg, index) = vm.db.objects.get(o).key.clone()?;
    Some((vm.db.set.get(&pkg).ok()?, index))
}

/// An animated actor's own copy of its skeletal mesh, re-posed when its animation moves.
struct Skinned {
    mesh: ObjId,
    skins: Vec<Option<ObjId>>,
    gpu: Rc<GpuMesh>,
    geom: MeshGeom,
    asset: Rc<hp3_assets::Asset>,
    skeleton: Rc<hp3_anim::Skeleton>,
    /// Bone maps per animation set (by the set's address).
    bone_maps: HashMap<usize, Vec<Option<usize>>>,
    /// The (sequence, frame) last written, to skip unchanged poses.
    last: Option<(Option<usize>, u32, u64)>,
}

/// Uploaded meshes, keyed by mesh and skins; animated actors' own skinned copies.
#[derive(Default)]
pub struct MeshCache {
    meshes: HashMap<(ObjKey, Vec<Option<ObjKey>>), Option<Rc<GpuMesh>>>,
    skinned: HashMap<ObjId, Skinned>,
    skeletons: HashMap<ObjKey, Rc<hp3_anim::Skeleton>>,
    /// What the actor lighting remembers between frames.
    light: actor_light::LightCache,
    /// Where each actor stood at the last two ticks (tick, place before, place now, turn before, turn now), to draw it between them.
    motion: HashMap<ObjId, (u64, Vec3, Vec3, glam::Quat, glam::Quat)>,
    /// Meshes uploaded to the GPU so far (a debugging counter).
    pub uploads: usize,
}

/// What `Skinned::last` holds for an actor's animation state: its first channel's sequence and frame (with the bone
/// controllers folded in), and a hash of the tween and the blended channels. The same value `skinned` works out when it poses.
fn pose_key(anim: Option<&hp3_world::anim::ActorAnim>) -> (Option<usize>, u32, u64) {
    let Some(a) = anim else { return (None, 0, 0) };
    let first = a.channels.first();
    let (seq, frame) = first.map(|c| (c.sequence, c.frame)).unwrap_or((None, 0.0));
    let tween_bits = first.map_or(0, |c| if c.from.filter(|_| c.tween_t < c.tween_total).is_some() { c.tween_weight().to_bits() as u64 + 1 } else { 0 });
    let mut layer_hash: u64 = 0;
    for c in a.channels.iter().skip(1) {
        if c.sequence.is_some() && c.alpha > 0.001 {
            layer_hash = layer_hash
                .wrapping_mul(1_000_003)
                .wrapping_add(c.sequence.unwrap_or(0) as u64 + 1)
                .wrapping_mul(31)
                .wrapping_add(c.frame.to_bits() as u64 ^ (((c.alpha * c.tween_weight()).to_bits() as u64) << 20));
        }
    }
    let turn_sum: i64 = a.bone_turns.iter().map(|t| t.1.iter().map(|&v| v as i64).sum::<i64>() + (t.2 * 1000.0) as i64).sum();
    (seq, frame.to_bits() ^ (turn_sum as u32), layer_hash ^ tween_bits.wrapping_mul(0x9E37_79B9))
}

fn skin_refs(vm: &Vm<'_>, mp: &Rc<Package>, skins: &[Option<ObjId>]) -> Vec<(Rc<Package>, ObjRef)> {
    skins
        .iter()
        .map(|s| match s.and_then(|s| export_of(vm, s)) {
            Some((p, i)) => (p, ObjRef::from_export(i)),
            None => (mp.clone(), ObjRef::NONE),
        })
        .collect()
}

impl MeshCache {
    /// An actor's place and turn `blend` of the way from the previous tick to this one (the whole way when `blend` is 1).
    fn smooth(&mut self, actor: ObjId, tick: u64, loc: Vec3, rot: [i32; 3], blend: f32) -> (Vec3, Mat4) {
        let turn = rotation_matrix(rot);
        if blend >= 1.0 {
            return (loc, turn);
        }
        let q = glam::Quat::from_mat4(&turn).normalize();
        let e = self.motion.entry(actor).or_insert((tick, loc, loc, q, q));
        if e.0 != tick {
            // A new tick: the last one is where the blend starts (an actor that jumped far is just put there).
            if e.0 + 1 == tick && e.2.distance(loc) < 400.0 {
                e.1 = e.2;
                e.3 = e.4;
            } else {
                e.1 = loc;
                e.3 = q;
            }
            e.0 = tick;
        }
        e.2 = loc;
        e.4 = q;
        (e.1.lerp(e.2, blend), Mat4::from_quat(e.3.slerp(e.4, blend)))
    }

    /// A skeletal mesh posed by the actor's animation channel 0; `None` for other meshes.
    #[allow(clippy::too_many_arguments)]
    fn skinned(
        &mut self,
        vm: &mut Vm<'_>,
        loader: &Loader<'_>,
        r: &Renderer,
        dynamic: &Dynamic,
        actor: ObjId,
        mesh: ObjId,
        skins: &[Option<ObjId>],
    ) -> Option<Rc<GpuMesh>> {
        let (mp, mi) = export_of(vm, mesh)?;
        let asset = loader.asset(&mp, mi)?;
        let hp3_assets::Asset::SkeletalMesh(m) = &*asset else { return None };
        let fresh = self.skinned.get(&actor).is_none_or(|s| s.mesh != mesh || s.skins != skins);
        if fresh {
            let skeleton = self
                .skeletons
                .entry(loader::key(&mp, mi))
                .or_insert_with(|| Rc::new(hp3_anim::Skeleton::from_mesh(m, &mp)))
                .clone();
            let geom = meshes::build(loader, &mp, mi, &skin_refs(vm, &mp, skins))?;
            let images = |k: &ObjKey| loader.image(&loader.set.get(&k.0).ok()?, k.1);
            let gpu = Rc::new(dynamic.upload(r, &geom, &images));
            self.uploads += 1;
            if std::env::var_os("HP3_UPLOAD_DEBUG").is_some() {
                let old = self.skinned.get(&actor).map(|s| (s.mesh == mesh, s.skins == skins));
                eprintln!("upload skinned actor {actor:?} mesh {mesh:?} (same mesh, same skins before: {old:?})");
            }
            self.skinned.insert(
                actor,
                Skinned { mesh, skins: skins.to_vec(), gpu, geom, asset: asset.clone(), skeleton, bone_maps: HashMap::new(), last: None },
            );
        }
        // Most actors stand as they did last frame: that is found from the animation state in place, without copying it.
        let key_now = pose_key(hp3_world::world(vm).anim.actors.get(&actor));
        if let Some(s) = self.skinned.get(&actor)
            && s.last == Some(key_now)
        {
            return Some(s.gpu.clone());
        }
        let state = hp3_world::world(vm)
            .anim
            .actors
            .get(&actor)
            .map(|a| (a.set.clone(), a.channels.clone(), a.bone_turns.clone()));
        let s = self.skinned.get_mut(&actor)?;
        let (set, channels, turns) = state.unwrap_or((None, Vec::new(), Vec::new()));
        let (seq, frame) = channels.first().map(|c| (c.sequence, c.frame)).unwrap_or((None, 0.0));
        let base = channels.first().map(|c| hp3_anim::Base {
            sequence: c.sequence,
            frame: c.frame,
            looping: c.looping,
            from: c.from.filter(|_| c.tween_t < c.tween_total),
            weight: c.tween_weight(),
        });
        let tween_bits = base.map_or(0, |b| if b.from.is_some() { b.weight.to_bits() as u64 + 1 } else { 0 });
        // Other channels with a sequence and some strength are blended over the base, limited to their bone.
        let mut layers: Vec<hp3_anim::Layer> = Vec::new();
        let mut layer_hash: u64 = 0;
        for c in channels.iter().skip(1) {
            if c.sequence.is_some() && c.alpha > 0.001 {
                let root = c.bone.as_deref().and_then(|b| s.skeleton.find(b));
                layers.push(hp3_anim::Layer { sequence: c.sequence, frame: c.frame, alpha: c.alpha * c.tween_weight(), root, looping: c.looping });
                layer_hash = layer_hash
                    .wrapping_mul(1_000_003)
                    .wrapping_add(c.sequence.unwrap_or(0) as u64 + 1)
                    .wrapping_mul(31)
                    .wrapping_add(c.frame.to_bits() as u64 ^ (((c.alpha * c.tween_weight()).to_bits() as u64) << 20));
            }
        }
        // Bone controllers change the pose too: they are part of what was last written.
        let turn_sum: i64 = turns.iter().map(|t| t.1.iter().map(|&v| v as i64).sum::<i64>() + (t.2 * 1000.0) as i64).sum();
        let key = (seq, frame.to_bits() ^ (turn_sum as u32), layer_hash ^ tween_bits.wrapping_mul(0x9E37_79B9));
        if s.last != Some(key) {
            let hp3_assets::Asset::SkeletalMesh(m) = &*s.asset else { return None };
            let skin = match &set {
                Some(set) => {
                    let map = s.bone_maps.entry(Rc::as_ptr(set) as usize).or_insert_with(|| set.bone_map(&s.skeleton));
                    let mut t = hp3_world::anim::turns_for(&s.skeleton, &turns);
                    t.extend(hp3_world::anim::strafe_turns(vm, actor));
                    hp3_anim::pose_blended(&s.skeleton, Some((set, map)), &base.unwrap_or_default(), &layers, &t)
                }
                None => hp3_anim::pose(&s.skeleton, None, None, 0.0),
            };
            let points = hp3_anim::skin_points(m, &skin);
            dynamic.write_points(r, &s.gpu, &s.geom, &points);
            s.last = Some(key);
        }
        Some(s.gpu.clone())
    }

    fn get(
        &mut self,
        vm: &Vm<'_>,
        loader: &Loader<'_>,
        r: &Renderer,
        dynamic: &Dynamic,
        mesh: ObjId,
        skins: &[Option<ObjId>],
    ) -> Option<Rc<GpuMesh>> {
        let (mp, mi) = export_of(vm, mesh)?;
        let skin_exports: Vec<Option<(Rc<Package>, usize)>> = skins.iter().map(|s| s.and_then(|s| export_of(vm, s))).collect();
        let key = (loader::key(&mp, mi), skin_exports.iter().map(|s| s.as_ref().map(|(p, i)| loader::key(p, *i))).collect());
        if let Some(m) = self.meshes.get(&key) {
            return m.clone();
        }
        let skin_refs: Vec<(Rc<Package>, ObjRef)> = skin_exports
            .into_iter()
            .map(|s| match s {
                Some((p, i)) => (p, ObjRef::from_export(i)),
                None => (mp.clone(), ObjRef::NONE),
            })
            .collect();
        let gpu = meshes::build(loader, &mp, mi, &skin_refs).map(|geom| {
            let images = |k: &ObjKey| {
                let p = loader.set.get(&k.0).ok()?;
                loader.image(&p, k.1)
            };
            Rc::new(dynamic.upload(r, &geom, &images))
        });
        self.meshes.insert(key, gpu.clone());
        gpu
    }
}

pub(crate) fn vec3(v: &Value) -> Vec3 {
    hp3_world::physics::vec(v)
}

/// Time spent this second making skinned meshes and plain ones for the frame, and how many skinned ones there were
/// (nanoseconds, nanoseconds, count): read and cleared by `take_instance_times`.
static INSTANCE_NS: [std::sync::atomic::AtomicU64; 5] = [const { std::sync::atomic::AtomicU64::new(0) }; 5];

/// (skinned ms, plain ms, skinned actors drawn, placing the drawn actors ms, the whole function ms) since the last call.
pub fn take_instance_times() -> (f32, f32, u64, f32, f32) {
    use std::sync::atomic::Ordering::Relaxed;
    let ms = |i: usize| INSTANCE_NS[i].swap(0, Relaxed) as f32 / 1e6;
    (ms(0), ms(1), INSTANCE_NS[2].swap(0, Relaxed), ms(3), ms(4))
}

/// What the camera sees, to leave out skeletal meshes that are surely out of view.
pub struct View {
    pub eye: Vec3,
    pub forward: Vec3,
    /// Horizontal field of view in degrees.
    pub fov_degrees: f32,
}

/// The largest radius (Unreal units) a character is taken to have around its location.
const CULL_RADIUS: f32 = 400.0;

impl View {
    /// True when something with `CULL_RADIUS` around `at` lies outside a cone a little wider than the view.
    fn rules_out(&self, at: Vec3) -> bool {
        let d = at - self.eye;
        let dist = d.length();
        if dist <= CULL_RADIUS {
            return false;
        }
        let angle = (d.dot(self.forward) / dist).clamp(-1.0, 1.0).acos().to_degrees();
        // The cone reaches past the corners of the screen (the field of view is not the diagonal) and past the radius.
        let cone = self.fov_degrees * 0.5 + 25.0 + (CULL_RADIUS / dist).asin().to_degrees();
        angle > cone
    }
}

/// The meshes to draw this frame with their actor-to-world transforms.
pub fn actor_instances(
    vm: &mut Vm<'_>,
    loader: &Loader<'_>,
    r: &Renderer,
    dynamic: &Dynamic,
    cache: &mut MeshCache,
    lighting: Option<&Lighting>,
    view: Option<View>,
    hidden_pawn: Option<ObjId>,
    blend: f32,
) -> Vec<(Rc<GpuMesh>, Mat4, ActorLight)> {
    let t_total = std::time::Instant::now();
    let actors = world(vm).actors.clone();
    let mut out = Vec::new();
    let mut shadows: Vec<hp3_render::dynamic::BlobShadow> = Vec::new();
    let shadows_on = std::env::var_os("HP3_NO_SHADOWS").is_none();
    let pawn_class = vm.db.find("Pawn");
    let time = if lighting.is_some() {
        cache.light.begin_frame(vm);
        let t = actor_light::level_time(vm);
        // The dynamic lights fall on the level's surfaces too, not only on actors.
        let eye = view.as_ref().map_or(Vec3::ZERO, |v| v.eye);
        let mut moving = cache.light.moving_lights(vm, eye, t);
        // HP3_MOVING_LIGHTS=1 prints them, =0 leaves them off the level's surfaces (debugging).
        match std::env::var("HP3_MOVING_LIGHTS").as_deref() {
            Ok("0") => moving.clear(),
            Ok(_) if !moving.is_empty() => eprintln!("moving lights at tick {}: {moving:?}", world(vm).frame),
            _ => {}
        }
        r.set_moving_lights(&moving);
        t
    } else {
        r.set_moving_lights(&[]);
        0.0
    };
    // Most of a level's actors draw nothing (volumes, triggers, baked static meshes): they are told apart by reading
    // a few values through slots found once, before anything is looked up by name.
    let (hidden, special_hidden, draw_slot, static_slot) = (
        hp3_world::actor_slot(vm, "bHidden"),
        hp3_world::actor_slot(vm, "bSpecialHidden"),
        hp3_world::actor_slot(vm, "DrawType"),
        hp3_world::actor_slot(vm, "bStatic"),
    );
    let (owner_slot, base_slot) = (hp3_world::actor_slot(vm, "Owner"), hp3_world::actor_slot(vm, "Base"));
    let skin_mod_slot = hp3_world::actor_slot(vm, "bUseSkinColorMod");
    let lumos_slot = hp3_world::actor_slot(vm, "LumosType");
    // The debugging switches, read once (looking the environment up takes a lock, and this loop sees every actor).
    static DRAW_DEBUG: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    static HIDE_ACTOR: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    let draw_debug = DRAW_DEBUG.get_or_init(|| std::env::var("HP3_DRAW_DEBUG").ok().map(|n| format!(".{n}"))).as_deref();
    let hide_actor = HIDE_ACTOR.get_or_init(|| std::env::var("HP3_HIDE_ACTOR").ok().map(|n| format!(".{n}"))).as_deref();
    let named = |vm: &Vm<'_>, a: ObjId, suffix: Option<&str>| suffix.is_some_and(|s| { let p = hp3_script::natives::object_path(vm, a); p.ends_with(s) || p == s.trim_start_matches('.') });
    for a in actors {
        if vm.db.objects.get(a).deleted {
            continue;
        }
        // What cannot draw is ruled out first, by plain reads: most of a level's actors are such, and the pawn check below
        // would otherwise look two properties up by name for each.
        let draw_type = hp3_world::read(vm, a, draw_slot).as_byte();
        if named(vm, a, draw_debug) {
            eprintln!(
                "draw {}: type {draw_type} static {:?} hidden {:?} special {:?}",
                hp3_script::natives::object_path(vm, a),
                hp3_world::read(vm, a, static_slot),
                hp3_world::read(vm, a, hidden),
                hp3_world::read(vm, a, special_hidden)
            );
        }
        if draw_type != DT_MESH && (draw_type != DT_STATIC_MESH || (hp3_world::read(vm, a, static_slot).as_bool() && hp3_world::read(vm, a, lumos_slot).as_int() == 0)) {
            continue;
        }
        // `bSpecialHidden` keeps an actor out of the normal view (the animation helper of a ledge climb has it).
        if hp3_world::read(vm, a, hidden).as_bool() || hp3_world::read(vm, a, special_hidden).as_bool() {
            continue;
        }
        // Something on a bone is drawn as part of the actor whose bone it is, as in the original: when that actor is
        // hidden (an armoured Peeves waiting to be woken up), its attachments are not drawn either.
        if get(vm, a, "AttachmentBone").as_name() != hp3_script::Name::NONE
            && let Some(parent) = hp3_world::read(vm, a, base_slot).as_obj()
            && (hp3_world::read(vm, parent, hidden).as_bool() || hp3_world::read(vm, parent, special_hidden).as_bool())
        {
            continue;
        }
        if let Some(pawn) = hidden_pawn
            && (a == pawn || hp3_world::read(vm, a, owner_slot).as_obj() == Some(pawn) || hp3_world::read(vm, a, base_slot).as_obj() == Some(pawn))
        {
            continue;
        }
        let mesh = if draw_type == DT_MESH { get(vm, a, "Mesh").as_obj() } else { get(vm, a, "StaticMesh").as_obj() };
        let Some(mesh) = mesh else { continue };
        // HP3_HIDE_ACTOR=<name>: leaves that actor out of the picture (to see what on screen it is).
        if named(vm, a, hide_actor) {
            continue;
        }
        // A skinned character that cannot be in view is not skinned at all.
        if draw_type == DT_MESH
            && let Some(v) = &view
            && v.rules_out(vec3(&get(vm, a, "Location")))
        {
            continue;
        }
        let skins: Vec<Option<ObjId>> = match get(vm, a, "Skins") {
            Value::Array(v) => v.iter().map(Value::as_obj).collect(),
            _ => Vec::new(),
        };
        let t0 = std::time::Instant::now();
        let skinned = if draw_type == DT_MESH { cache.skinned(vm, loader, r, dynamic, a, mesh, &skins) } else { None };
        let t1 = std::time::Instant::now();
        let was_skinned = skinned.is_some();
        let found = skinned.or_else(|| cache.get(vm, loader, r, dynamic, mesh, &skins));
        {
            use std::sync::atomic::Ordering::Relaxed;
            INSTANCE_NS[0].fetch_add((t1 - t0).as_nanos() as u64, Relaxed);
            INSTANCE_NS[1].fetch_add(t1.elapsed().as_nanos() as u64, Relaxed);
            INSTANCE_NS[2].fetch_add(was_skinned as u64, Relaxed);
        }
        if named(vm, a, draw_debug) {
            eprintln!("draw {}: mesh {} found {}", hp3_script::natives::object_path(vm, a), hp3_script::natives::object_path(vm, mesh), found.is_some());
        }
        let Some(gpu) = found else { continue };
        let t_rest = std::time::Instant::now();
        let loc = hp3_world::rd_vec(vm, a, "Location") + Vec3::Z * hp3_world::physics::view_offset(vm, a);
        let rot = hp3_world::rd_rot(vm, a, "Rotation");
        let scale = match hp3_world::rd(vm, a, "DrawScale").as_float() {
            0.0 => 1.0,
            s => s,
        };
        let s3 = match hp3_world::rd(vm, a, "DrawScale3D") {
            v @ Value::Struct(_) => vec3(v),
            _ => Vec3::ONE,
        };
        let s3 = if s3 == Vec3::ZERO { Vec3::ONE } else { s3 };
        // Skeletal meshes stand on their feet; static meshes are placed at the actor's location.
        let prepivot = if draw_type == DT_MESH { hp3_world::mesh_offset(vm, a) } else { hp3_world::rd_vec(vm, a, "PrePivot") };
        let tick = world(vm).frame;
        let (shown, turn) = cache.smooth(a, tick, loc, rot, blend);
        let transform = hp3_world::anim::hang_correction(vm, a) * Mat4::from_translation(shown + prepivot) * turn * Mat4::from_scale(s3 * scale);
        // Lit as the original lights an actor: per vertex, by the lights that reach its bounding sphere.
        let mut light = match lighting {
            Some(l) => {
                let sphere = cache.light.sphere_of(vm, a);
                // A static mesh is lit from the middle of its bounds, as the original does from its picture sphere: an origin at a corner
                // (a block set into the floor) would have every light blocked by the floor around it.
                let lit_at = if draw_type == DT_MESH { loc } else { transform.transform_point3(gpu.local.transform_point3(gpu.bounds_centre)) };
                let result = actor_light::actor_light_for(vm, &mut cache.light, l, a, lit_at, sphere, time);
                actor_light::debug_print(vm, a, &result);
                result
            }
            None => ActorLight { ambient: Vec3::splat(0.5), lights: Vec::new(), fade: 0.0 },
        };
        // HP3 gives a static mesh half of its zone ambient (and glow), drawn alone or batched; characters keep all of it.
        if lighting.is_some() && draw_type != DT_MESH && !hp3_world::rd(vm, a, "bUnlit").as_bool() {
            light.ambient *= 0.5;
        }
        if named(vm, a, draw_debug) && let Some(extra) = std::env::var("HP3_DEBUG_AMBIENT").ok().and_then(|v| v.parse::<f32>().ok()) {
            light.ambient += Vec3::splat(extra);
        }
        if named(vm, a, draw_debug) {
            eprintln!("draw {}: light ambient {} lights {} transform {:?}", hp3_script::natives::object_path(vm, a), light.ambient, light.lights.len(), transform.w_axis);
            for l in &light.lights {
                eprintln!("   light kind {} at {} dir {} colour {} range {} atten {:?}", l.kind, l.position, l.direction, l.color, l.range, l.atten);
            }
        }
        // `Actor.SkinColorModifier` (set by the fade delegates: the frozen salamander goes ice blue) tints the skin: each channel
        // scales the light on it, 128 being no change.
        if hp3_world::read(vm, a, skin_mod_slot).as_bool() {
            let c = hp3_world::get(vm, a, "SkinColorModifier");
            let ch = |i: usize| c.fields().get(i).map_or(128, Value::as_int) as f32 / 128.0;
            // A `Color` is declared B, G, R, A.
            let tint = Vec3::new(ch(2), ch(1), ch(0));
            light.ambient *= tint;
            for l in &mut light.lights {
                l.color *= tint;
            }
        }
        // A soft shadow on the ground under a pawn (not in the original): from the strongest light on it, stretched away from it.
        if draw_type == DT_MESH && shadows_on && pawn_class.is_some_and(|p| vm.db.objects.get(a).class.is_some_and(|c| vm.is_child_of(c, p))) {
            if let Some(s) = pawn_shadow(vm, a, shown, &light) {
                shadows.push(s);
            }
        }
        // A wall the Lumos spell works on: while the hero's light is on, it turns see-through near the hero (a vanishing wall) or
        // shows up near the hero (an appearing one). The original fades by distance from the light, down to about a fifth.
        let lumos = hp3_world::read(vm, a, lumos_slot).as_int();
        if lumos == 1 || lumos == 2 {
            let hero = world(vm).level_info.and_then(|li| get(vm, li, "PlayerHeroActor").as_obj());
            let (on, radius, at) = match hero {
                Some(h) => (get(vm, h, "bLumosOn").as_bool(), get(vm, h, "fLumosRadius").as_float(), vec3(&get(vm, h, "Location"))),
                None => (false, 0.0, Vec3::ZERO),
            };
            let radius = if radius > 0.0 { radius } else { 512.0 };
            let middle = transform.transform_point3(gpu.local.transform_point3(gpu.bounds_centre));
            // A wall is big and its middle far from the part near the light: allow for that.
            let near = ((at.distance(middle) - 128.0).max(0.0) / (radius * 0.8)).min(1.0);
            if lumos == 2 {
                if on {
                    light.fade = 0.78 * (1.0 - near);
                }
            } else if !on || near >= 0.98 {
                continue;
            } else {
                light.fade = near;
            }
        }
        out.push((gpu, transform, light));
        INSTANCE_NS[3].fetch_add(t_rest.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
    }
    // Pieces thrown by mesh emitters (the ice of a shattering salamander) are drawn like any other static mesh.
    for piece in hp3_world::particles::collect_meshes(vm) {
        let Some(gpu) = cache.get(vm, loader, r, dynamic, piece.mesh, &[]) else { continue };
        let transform = Mat4::from_translation(piece.pos) * Mat4::from_mat3(piece.rot) * Mat4::from_scale(piece.scale);
        out.push((gpu, transform, ActorLight { ambient: Vec3::splat(0.8), lights: Vec::new(), fade: 0.0 }));
    }
    if std::env::var_os("HP3_SHADOW_DEBUG").is_some() {
        eprintln!("shadows: {} {:?}", shadows.len(), shadows.first());
    }
    dynamic.set_shadows(r, &shadows);
    INSTANCE_NS[4].fetch_add(t_total.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
    out
}

/// Remembers which texture each particle material shows.
#[derive(Default)]
pub struct ParticleCache {
    textures: HashMap<ObjId, Option<ObjKey>>,
}

/// The frame's particles as renderer quads facing `camera`.
pub fn particle_quads(vm: &mut Vm<'_>, loader: &Loader<'_>, cache: &mut ParticleCache, camera: &hp3_render::scene::Camera) -> Vec<hp3_render::particles::ParticleQuad> {
    use hp3_render::particles::{ParticleBlend, ParticleQuad};
    let fwd = camera.forward();
    let right = Vec3::new(-fwd.y, fwd.x, 0.0).normalize_or_zero();
    let up = fwd.cross(right).normalize_or_zero();
    let quads = hp3_world::particles::collect(vm, right, up);
    let mut out = Vec::with_capacity(quads.len());
    for q in quads {
        let texture = match q.material {
            Some(m) => cache.textures.entry(m).or_insert_with(|| texture_key(vm, loader, m)).clone(),
            None => None,
        };
        let blend = match q.blend {
            hp3_world::particles::Blend::Alpha => ParticleBlend::Alpha,
            hp3_world::particles::Blend::Additive => ParticleBlend::Additive,
            hp3_world::particles::Blend::Modulate => ParticleBlend::Modulate,
        };
        out.push(ParticleQuad { texture, blend, corners: q.corners.map(|c| c.to_array()), uv: q.uv, color: q.color });
    }
    // Character shadows: a procedural round decal (see `fs_decal`) on the floor under each pawn.
    {
        for b in hp3_world::shadows::collect(vm, camera.position) {
            let t = if b.normal.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
            let side = b.normal.cross(t).normalize_or_zero() * b.radius;
            let fwd = b.normal.cross(side).normalize_or_zero() * b.radius;
            let corners = [b.center - side + fwd, b.center + side + fwd, b.center + side - fwd, b.center - side - fwd];
            out.push(ParticleQuad {
                texture: None,
                blend: ParticleBlend::Decal,
                corners: corners.map(|c| c.to_array()),
                uv: [0.0, 0.0, 1.0, 1.0],
                color: [1.0, 1.0, 1.0, b.alpha],
            });
        }
    }
    out
}

/// The soft shadow of a pawn standing at `at`: found by looking for the ground below, darkest when the pawn is close to it
/// and lit from one side, and stretched away from the strongest light.
fn pawn_shadow(vm: &mut Vm<'_>, a: ObjId, at: Vec3, light: &ActorLight) -> Option<hp3_render::dynamic::BlobShadow> {
    let half_height = hp3_world::rd(vm, a, "CollisionHeight").as_float().max(1.0);
    let radius = (hp3_world::rd(vm, a, "CollisionRadius").as_float() * 1.9).clamp(16.0, 56.0);
    let hit = world(vm).collision.trace_static(at, at - Vec3::Z * (half_height + 220.0), Vec3::ZERO)?;
    if hit.normal.z < 0.5 {
        return None;
    }
    let feet_above = (at.z - half_height - hit.location.z).max(0.0);
    let closeness = 1.0 - (feet_above / 180.0).clamp(0.0, 1.0);
    if closeness <= 0.0 {
        return None;
    }
    // The strongest light, as a direction along the ground away from it and how much it lights the pawn.
    let luma = |c: Vec3| c.dot(Vec3::new(0.3, 0.59, 0.11));
    let mut best = (0.0f32, Vec3::ZERO);
    for l in &light.lights {
        let (strength, away) = if l.kind == 1 {
            (luma(l.color), l.direction)
        } else {
            let to = at - l.position;
            let d = to.length();
            if d < 1.0 || d > l.range.max(1.0) {
                continue;
            }
            (luma(l.color) / (l.atten[0] + l.atten[1] * d + l.atten[2] * d * d).max(0.2), to / d)
        };
        if strength > best.0 {
            best = (strength, away);
        }
    }
    let lit = (best.0 + luma(light.ambient)).clamp(0.0, 1.0);
    let along = Vec3::new(best.1.x, best.1.y, 0.0);
    let reach = (along.length() / best.1.length().max(0.001)).clamp(0.0, 1.0);
    let dir = if along.length() > 0.01 { along.normalize() } else { Vec3::X };
    // On a slope the axes lie in the ground plane.
    let u = (dir - hit.normal * dir.dot(hit.normal)).normalize_or_zero();
    let u = if u == Vec3::ZERO { Vec3::X } else { u };
    let v = hit.normal.cross(u);
    let stretch = 1.0 + 0.7 * reach * (best.0 / (best.0 + 0.25));
    let center = hit.location + hit.normal * 0.6 + u * (radius * 0.45 * reach);
    Some(hp3_render::dynamic::BlobShadow {
        center,
        axis_u: u * radius * stretch * (1.0 + 0.4 * (1.0 - closeness)),
        axis_v: v * radius * (1.0 + 0.4 * (1.0 - closeness)),
        opacity: closeness * (0.75 + 0.25 * lit),
    })
}
