//! Skeletal pose evaluation: a mesh's reference skeleton, an animation set's sequences, sampling a sequence at
//! a point in time, bone matrices, and skinning the mesh's points.
//!
//! The bone and key conventions (local = translate * rotate, every stored rotation conjugated) were chosen by
//! rendering HP3's characters in their run cycle and checking the pose; NOT yet verified against HP3's
//! Engine.dll.

use std::collections::HashMap;

use glam::{Mat4, Quat, Vec3};
use hp3_assets::anim::{MeshAnimation, Track};
use hp3_assets::skeletal::SkeletalMesh;
use hp3_package::Package;

fn quat(q: &hp3_assets::anim::Quat) -> Quat {
    Quat::from_xyzw(q.x, q.y, q.z, q.w).normalize()
}

fn v3(v: &hp3_assets::Vec3) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

/// A bone's transform relative to its parent.
#[derive(Clone, Copy, Debug)]
pub struct BonePose {
    pub rotation: Quat,
    pub position: Vec3,
}

impl BonePose {
    /// The stored quaternions rotate the other way from glam's convention.
    fn matrix(&self) -> Mat4 {
        Mat4::from_rotation_translation(self.rotation.conjugate(), self.position)
    }
}

pub struct Skeleton {
    /// Lower-case bone names.
    pub names: Vec<String>,
    pub parents: Vec<usize>,
    pub reference: Vec<BonePose>,
    inv_reference_world: Vec<Mat4>,
}

impl Skeleton {
    pub fn from_mesh(mesh: &SkeletalMesh, pkg: &Package) -> Skeleton {
        let names = mesh.ref_skeleton.iter().map(|b| pkg.name(b.name).to_ascii_lowercase()).collect();
        let parents: Vec<usize> = mesh.ref_skeleton.iter().map(|b| b.parent.max(0) as usize).collect();
        let reference: Vec<BonePose> = mesh
            .ref_skeleton
            .iter()
            .map(|b| BonePose { rotation: quat(&b.orientation), position: v3(&b.position) })
            .collect();
        let world = world_matrices(&parents, &reference);
        let inv_reference_world = world.iter().map(|m| m.inverse()).collect();
        Skeleton { names, parents, reference, inv_reference_world }
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

/// Bone-to-mesh matrices from bone-to-parent poses (parents come before children).
fn world_matrices(parents: &[usize], poses: &[BonePose]) -> Vec<Mat4> {
    let mut out: Vec<Mat4> = Vec::with_capacity(poses.len());
    for (i, p) in poses.iter().enumerate() {
        let local = p.matrix();
        let m = if i == 0 || parents[i] >= i { local } else { out[parents[i]] * local };
        out.push(m);
    }
    out
}

#[derive(Clone, Debug)]
pub struct Sequence {
    pub name: String,
    pub frames: i32,
    /// Frames per second.
    pub rate: f32,
    motion: usize,
}

pub struct AnimSet {
    bone_names: Vec<String>,
    pub sequences: Vec<Sequence>,
    by_name: HashMap<String, usize>,
    anim: MeshAnimation,
}

impl AnimSet {
    pub fn new(anim: MeshAnimation, pkg: &Package) -> AnimSet {
        let bone_names = anim.bones.iter().map(|b| pkg.name(b.name).to_ascii_lowercase()).collect();
        let sequences: Vec<Sequence> = anim
            .sequences
            .iter()
            .enumerate()
            .map(|(i, s)| Sequence { name: pkg.name(s.name).to_ascii_lowercase(), frames: s.num_frames.max(1), rate: s.rate, motion: i })
            .collect();
        let by_name = sequences.iter().enumerate().map(|(i, s)| (s.name.clone(), i)).collect();
        AnimSet { bone_names, sequences, by_name, anim }
    }

    pub fn find(&self, name: &str) -> Option<usize> {
        self.by_name.get(&name.to_ascii_lowercase()).copied()
    }

    /// For each skeleton bone, the animation bone driving it.
    pub fn bone_map(&self, skel: &Skeleton) -> Vec<Option<usize>> {
        skel.names.iter().map(|n| self.bone_names.iter().position(|b| b == n)).collect()
    }
}

/// Key `k` of an array, holding the last one when there are fewer keys.
fn key<T: Copy>(v: &[T], k: usize) -> Option<T> {
    v.get(k.min(v.len().saturating_sub(1))).copied()
}

/// A track's pose at `t` frames into a motion of `length` frames.
fn sample_track(track: &Track, t: f32, length: f32) -> Option<BonePose> {
    let rot0 = key(&track.rotations, 0)?;
    let pos0 = key(&track.positions, 0)?;
    if track.times.len() <= 1 {
        return Some(BonePose { rotation: quat(&rot0), position: v3(&pos0) });
    }
    let next = track.times.iter().position(|&kt| kt > t).unwrap_or(track.times.len());
    let (i0, i1, alpha) = if next == 0 {
        (0, 0, 0.0)
    } else if next >= track.times.len() {
        // Past the last key: blend toward the first for looping motions.
        let last = track.times.len() - 1;
        let span = (length - track.times[last]).max(1e-4);
        (last, 0, ((t - track.times[last]) / span).clamp(0.0, 1.0))
    } else {
        let (a, b) = (track.times[next - 1], track.times[next]);
        (next - 1, next, ((t - a) / (b - a).max(1e-4)).clamp(0.0, 1.0))
    };
    let r0 = quat(&key(&track.rotations, i0)?);
    let mut r1 = quat(&key(&track.rotations, i1)?);
    if r0.dot(r1) < 0.0 {
        r1 = -r1;
    }
    let p0 = v3(&key(&track.positions, i0)?);
    let p1 = v3(&key(&track.positions, i1)?);
    Some(BonePose { rotation: r0.slerp(r1, alpha), position: p0.lerp(p1, alpha) })
}

/// Unreal rotator (pitch, yaw, roll in 65536ths of a turn) to a rotation matrix; columns are the X, Y and Z axes.
pub fn rotator_matrix(rot: [i32; 3]) -> Mat4 {
    let k = std::f32::consts::TAU / 65536.0;
    let (sp, cp) = (rot[0] as f32 * k).sin_cos();
    let (sy, cy) = (rot[1] as f32 * k).sin_cos();
    let (sr, cr) = (rot[2] as f32 * k).sin_cos();
    let x = Vec3::new(cp * cy, cp * sy, sp);
    let y = Vec3::new(sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, -sr * cp);
    let z = Vec3::new(-(cr * sp * cy + sr * sy), cy * sr - cr * sp * sy, cr * cp);
    Mat4::from_cols(x.extend(0.0), y.extend(0.0), z.extend(0.0), glam::Vec4::W)
}

/// The rotator whose axes are `m`'s (normalized) X, Y and Z columns; inverse of `rotator_matrix`.
pub fn matrix_rotator(m: &Mat4) -> [i32; 3] {
    let x = m.x_axis.truncate().normalize_or_zero();
    let y = m.y_axis.truncate().normalize_or_zero();
    let z = m.z_axis.truncate().normalize_or_zero();
    let k = 65536.0 / std::f32::consts::TAU;
    let pitch = x.z.atan2((x.x * x.x + x.y * x.y).sqrt());
    let yaw = x.y.atan2(x.x);
    let roll = (-y.z).atan2(z.z);
    [(pitch * k).round() as i32, (yaw * k).round() as i32, (roll * k).round() as i32]
}

/// A skeletal mesh's own space to its actor's space (origin, scale and rotation offsets).
pub fn mesh_to_actor(mesh: &SkeletalMesh) -> Mat4 {
    let h = &mesh.header;
    let scale = Vec3::new(h.mesh_scale.x, h.mesh_scale.y, h.mesh_scale.z);
    let origin = Vec3::new(h.mesh_origin.x, h.mesh_origin.y, h.mesh_origin.z);
    rotator_matrix(h.rot_origin) * Mat4::from_scale(scale) * Mat4::from_translation(-origin)
}

/// Bone-to-mesh matrices for a sequence at `frame` (0 to its frame count); the reference pose without one.
pub fn bones(skel: &Skeleton, set: Option<(&AnimSet, &[Option<usize>])>, seq: Option<usize>, frame: f32) -> Vec<Mat4> {
    let mut poses = skel.reference.clone();
    if let (Some((set, map)), Some(seq)) = (set, seq)
        && let Some(s) = set.sequences.get(seq)
        && let Some(motion) = set.anim.motions.get(s.motion)
    {
        let length = motion.track_time.max(1.0);
        let t = (frame / s.frames as f32).clamp(0.0, 1.0) * length;
        for (bone, anim_bone) in map.iter().enumerate() {
            let Some(ab) = anim_bone else { continue };
            if let Some(track) = motion.tracks.get(*ab)
                && let Some(p) = sample_track(track, t, length)
            {
                poses[bone] = p;
            }
        }
    }
    world_matrices(&skel.parents, &poses)
}

impl Skeleton {
    pub fn find(&self, bone: &str) -> Option<usize> {
        let bone = bone.to_ascii_lowercase();
        self.names.iter().position(|n| *n == bone)
    }
}

/// Skin matrices (bind pose to posed, in mesh space) for a sequence at `frame` (0 to its frame count).
pub fn pose(skel: &Skeleton, set: Option<(&AnimSet, &[Option<usize>])>, seq: Option<usize>, frame: f32) -> Vec<Mat4> {
    let world = bones(skel, set, seq, frame);
    world.iter().zip(&skel.inv_reference_world).map(|(w, inv)| *w * *inv).collect()
}

/// Posed positions of the mesh's points (its reference vertices), blending the bones that influence each.
pub fn skin_points(mesh: &SkeletalMesh, skin: &[Mat4]) -> Vec<Vec3> {
    let mut out = vec![Vec3::ZERO; mesh.points.len()];
    let mut weight = vec![0.0f32; mesh.points.len()];
    for inf in &mesh.influences {
        let (v, b) = (inf.vertex as usize, inf.bone as usize);
        let (Some(p), Some(m)) = (mesh.points.get(v), skin.get(b)) else { continue };
        out[v] += m.transform_point3(v3(p)) * inf.weight;
        weight[v] += inf.weight;
    }
    for (i, p) in out.iter_mut().enumerate() {
        if weight[i] > 0.0 {
            *p /= weight[i];
        } else if let Some(r) = mesh.points.get(i) {
            *p = v3(r);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotator_round_trip() {
        for r in [[0, 0, 0], [0, 16384, 0], [4000, -12000, 2500], [-8000, 30000, -6000], [10000, 5000, 0]] {
            let back = matrix_rotator(&rotator_matrix(r));
            for i in 0..3 {
                let d = (back[i] - r[i]).rem_euclid(65536);
                assert!(d <= 2 || d >= 65534, "{r:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn yaw_turns_x_toward_y() {
        let m = rotator_matrix([0, 16384, 0]);
        let x = m.transform_vector3(Vec3::X);
        assert!((x - Vec3::Y).length() < 1e-4, "{x:?}");
    }
}
