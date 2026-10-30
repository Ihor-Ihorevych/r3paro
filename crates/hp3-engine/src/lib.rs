//! Ties the running world to the renderer: which actors are drawn and with which mesh and transform.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use glam::{Mat4, Vec3};
use hp3_assets::loader::{self, Loader, ObjKey};
use hp3_package::{ObjRef, Package};
use hp3_render::dynamic::{Dynamic, GpuMesh};
use hp3_render::gpu::Renderer;
use hp3_render::meshes;
use hp3_render::scene::rotation_matrix;
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

/// The package export behind an object, if it came from a package.
fn export_of(vm: &Vm<'_>, o: ObjId) -> Option<(Rc<Package>, usize)> {
    let (pkg, index) = vm.db.objects.get(o).key.clone()?;
    Some((vm.db.set.get(&pkg).ok()?, index))
}

/// Uploaded meshes, keyed by mesh and skins.
#[derive(Default)]
pub struct MeshCache {
    meshes: HashMap<(ObjKey, Vec<Option<ObjKey>>), Option<Rc<GpuMesh>>>,
}

impl MeshCache {
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
) -> Vec<(Rc<GpuMesh>, Mat4)> {
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
        let Some(gpu) = cache.get(vm, loader, r, dynamic, mesh, &skins) else { continue };
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
        out.push((gpu, transform));
    }
    out
}
