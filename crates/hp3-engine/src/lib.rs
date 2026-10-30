//! Ties the running world to the renderer: which actors are drawn and with which mesh and transform.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use glam::{Mat4, Vec3};
use hp3_assets::loader::{self, Loader, ObjKey};
use hp3_package::{ObjRef, Package};
use hp3_render::dynamic::{Dynamic, GpuMesh};
use hp3_render::gpu::Renderer;
use hp3_render::meshes::{self, MeshGeom};
use hp3_render::scene::{rotation_matrix, LightSource, Lighting};
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
        if pkg.as_ref() == map.name.to_ascii_lowercase() && get(vm, a, "bStatic").as_bool() {
            out.insert(index);
        }
    }
    out
}

/// `LightEffect` of sunlights in HP3 (the `Sunlight` class default).
const LE_SUNLIGHT: u8 = 19;

/// Unreal light color: `hue` around the color wheel (0..255), `saturation` 255 meaning white.
fn light_color(hue: u8, saturation: u8) -> Vec3 {
    let h = hue as f32 / 255.0 * 6.0;
    let x = 1.0 - ((h % 2.0) - 1.0).abs();
    let pure = match h as u32 {
        0 => Vec3::new(1.0, x, 0.0),
        1 => Vec3::new(x, 1.0, 0.0),
        2 => Vec3::new(0.0, 1.0, x),
        3 => Vec3::new(0.0, x, 1.0),
        4 => Vec3::new(x, 0.0, 1.0),
        _ => Vec3::new(1.0, 0.0, x),
    };
    pure.lerp(Vec3::ONE, saturation as f32 / 255.0)
}

/// The map's lights and ambient light, read from the running world (class defaults applied).
pub fn lighting(vm: &mut Vm<'_>, map: &Package) -> Lighting {
    let mut out = Lighting::default();
    let map_name = map.name.to_ascii_lowercase();
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
        let color = light_color(get(vm, a, "LightHue").as_byte(), get(vm, a, "LightSaturation").as_byte()) * brightness;
        let position = vec3(&get(vm, a, "Location"));
        let radius = 25.0 * (get(vm, a, "LightRadius").as_float() + 1.0);
        let direction = (get(vm, a, "LightEffect").as_byte() == LE_SUNLIGHT).then(|| {
            let r: Vec<i32> = get(vm, a, "Rotation").fields().iter().map(Value::as_int).collect();
            let m = rotation_matrix([r.first().copied().unwrap_or(0), r.get(1).copied().unwrap_or(0), 0]);
            m.transform_vector3(Vec3::X)
        });
        out.lights.insert(index, LightSource { position, color, radius, direction });
    }
    // Zones carry their ambient light, already computed, in `AmbientVector`.
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
        let ambient = vec3(&get(vm, a, "AmbientVector"));
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
    last: Option<(Option<usize>, u32)>,
}

/// Uploaded meshes, keyed by mesh and skins; animated actors' own skinned copies.
#[derive(Default)]
pub struct MeshCache {
    meshes: HashMap<(ObjKey, Vec<Option<ObjKey>>), Option<Rc<GpuMesh>>>,
    skinned: HashMap<ObjId, Skinned>,
    skeletons: HashMap<ObjKey, Rc<hp3_anim::Skeleton>>,
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
            self.skinned.insert(
                actor,
                Skinned { mesh, skins: skins.to_vec(), gpu, geom, asset: asset.clone(), skeleton, bone_maps: HashMap::new(), last: None },
            );
        }
        let state = hp3_world::world(vm)
            .anim
            .actors
            .get(&actor)
            .map(|a| (a.set.clone(), a.channels.first().cloned()));
        let s = self.skinned.get_mut(&actor)?;
        let (set, channel) = state.unwrap_or((None, None));
        let (seq, frame) = channel.map(|c| (c.sequence, c.frame)).unwrap_or((None, 0.0));
        let key = (seq, frame.to_bits());
        if s.last != Some(key) {
            let hp3_assets::Asset::SkeletalMesh(m) = &*s.asset else { return None };
            let skin = match &set {
                Some(set) => {
                    let map = s.bone_maps.entry(Rc::as_ptr(set) as usize).or_insert_with(|| set.bone_map(&s.skeleton));
                    hp3_anim::pose(&s.skeleton, Some((set, map)), seq, frame)
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

fn vec3(v: &Value) -> Vec3 {
    hp3_world::physics::vec(v)
}

/// The meshes to draw this frame with their actor-to-world transforms.
pub fn actor_instances(
    vm: &mut Vm<'_>,
    loader: &Loader<'_>,
    r: &Renderer,
    dynamic: &Dynamic,
    cache: &mut MeshCache,
    lighting: Option<&Lighting>,
) -> Vec<(Rc<GpuMesh>, Mat4, Vec3)> {
    let actors = world(vm).actors.clone();
    let mut out = Vec::new();
    for a in actors {
        if vm.db.objects.get(a).deleted || get(vm, a, "bHidden").as_bool() {
            continue;
        }
        let draw_type = get(vm, a, "DrawType").as_byte();
        let mesh = match draw_type {
            DT_MESH => get(vm, a, "Mesh").as_obj(),
            DT_STATIC_MESH if !get(vm, a, "bStatic").as_bool() => get(vm, a, "StaticMesh").as_obj(),
            _ => None,
        };
        let Some(mesh) = mesh else { continue };
        let skins: Vec<Option<ObjId>> = match get(vm, a, "Skins") {
            Value::Array(v) => v.iter().map(Value::as_obj).collect(),
            _ => Vec::new(),
        };
        let skinned = if draw_type == DT_MESH { cache.skinned(vm, loader, r, dynamic, a, mesh, &skins) } else { None };
        let Some(gpu) = skinned.or_else(|| cache.get(vm, loader, r, dynamic, mesh, &skins)) else { continue };
        let loc = vec3(&get(vm, a, "Location"));
        let rot: Vec<i32> = get(vm, a, "Rotation").fields().iter().map(Value::as_int).collect();
        let rot = [rot.first().copied().unwrap_or(0), rot.get(1).copied().unwrap_or(0), rot.get(2).copied().unwrap_or(0)];
        let scale = match get(vm, a, "DrawScale").as_float() {
            0.0 => 1.0,
            s => s,
        };
        let s3 = match get(vm, a, "DrawScale3D") {
            v @ Value::Struct(_) => vec3(&v),
            _ => Vec3::ONE,
        };
        let s3 = if s3 == Vec3::ZERO { Vec3::ONE } else { s3 };
        let prepivot = vec3(&get(vm, a, "PrePivot"));
        let transform = Mat4::from_translation(loc + prepivot) * rotation_matrix(rot) * Mat4::from_scale(s3 * scale);
        // Actors are lit as a whole by the lights around them (half: the shader doubles it).
        // Capped, so an actor next to a strong light is bright but not washed out.
        let light = lighting.map(|l| l.actor_light(loc).min(Vec3::splat(1.25)) * 0.5).unwrap_or(Vec3::splat(0.5));
        out.push((gpu, transform, light));
    }
    out
}
