//! Baked ambient occlusion: how much of the sky-like hemisphere above a surface point is open, found by casting rays
//! against the level's static triangles when the scene is built. It darkens the ambient part of the lightmaps and of the
//! static meshes' vertex colours in corners, under arches and behind pillars (screen-space AO done once, on the surfaces,
//! instead of every frame).
//!
//! Results are kept on disk (`set_cache_dir`), keyed by a hash of the geometry and the query points, so a level pays the
//! rays only the first time it loads.
//!
//! Switches: `HP3_AO_RAYS` (16), `HP3_AO_RANGE` (128 units), `HP3_AO_STRENGTH` (1.0), `HP3_AO_DIRECT` (0.5: the share of the
//! occlusion that also falls on direct light). The on/off switch is `quality::baked_ao`.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use glam::Vec3;

/// Bumped when the meaning of the stored values changes.
const VERSION: u64 = 1;
/// Stored for a point found inside solid geometry (its rays mostly meet back faces).
pub const INVALID: u8 = 255;

static CACHE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Where baked occlusion is kept between runs (created when first written). Without it every load bakes again.
pub fn set_cache_dir(dir: PathBuf) {
    let _ = CACHE_DIR.set(dir);
}

/// How the occlusion is baked and applied.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub rays: usize,
    pub range: f32,
    pub strength: f32,
    pub direct: f32,
}

impl Params {
    pub fn from_env() -> Params {
        let get = |name: &str, default: f32| std::env::var(name).ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(default);
        Params {
            rays: (get("HP3_AO_RAYS", 16.0) as usize).clamp(4, 256),
            range: get("HP3_AO_RANGE", 128.0).max(8.0),
            strength: get("HP3_AO_STRENGTH", 1.0).clamp(0.0, 2.0),
            direct: get("HP3_AO_DIRECT", 0.5).clamp(0.0, 1.0),
        }
    }

    /// The occlusion as it is applied to ambient light: `strength` scales how far it darkens.
    pub fn ambient_factor(&self, ao: f32) -> f32 {
        (1.0 - self.strength * (1.0 - ao)).clamp(0.0, 1.0)
    }

    /// The same for direct light, which already has its shadows: only the `direct` share of it.
    pub fn direct_factor(&self, ao: f32) -> f32 {
        1.0 - self.direct * (1.0 - self.ambient_factor(ao))
    }
}

#[derive(Clone, Copy)]
struct Tri {
    a: Vec3,
    e1: Vec3,
    e2: Vec3,
}

/// A BVH node: an inner node (`count` 0) has its children at `first` and `first + 1`; a leaf holds `count` triangles from `first`.
#[derive(Clone, Copy)]
struct Node {
    min: Vec3,
    max: Vec3,
    first: u32,
    count: u32,
}

/// The triangles rays can meet, in a bounding volume hierarchy. A triangle's front is the side its corners turn
/// anticlockwise towards (`(b - a) x (c - a)`).
pub struct Occluders {
    tris: Vec<Tri>,
    nodes: Vec<Node>,
}

const LEAF: usize = 4;

impl Occluders {
    pub fn new(input: &[[Vec3; 3]]) -> Occluders {
        let tris: Vec<Tri> = input
            .iter()
            .filter(|t| (t[1] - t[0]).cross(t[2] - t[0]).length_squared() > 1e-8)
            .map(|t| Tri { a: t[0], e1: t[1] - t[0], e2: t[2] - t[0] })
            .collect();
        let centroids: Vec<Vec3> = tris.iter().map(|t| t.a + (t.e1 + t.e2) / 3.0).collect();
        let mut index: Vec<u32> = (0..tris.len() as u32).collect();
        let mut nodes = vec![Node { min: Vec3::ZERO, max: Vec3::ZERO, first: 0, count: tris.len() as u32 }];
        if !tris.is_empty() {
            subdivide(&mut nodes, 0, &tris, &centroids, &mut index);
        }
        let tris = index.iter().map(|&i| tris[i as usize]).collect();
        Occluders { tris, nodes }
    }

    pub fn len(&self) -> usize {
        self.tris.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    /// The nearest triangle the ray from `o` along `d` meets before `tmax`: its distance and whether its front faces the ray.
    pub fn closest(&self, o: Vec3, d: Vec3, tmax: f32) -> Option<(f32, bool)> {
        if self.tris.is_empty() {
            return None;
        }
        let safe = |v: f32| if v.abs() < 1e-12 { 1e-12f32.copysign(v) } else { v };
        let inv = Vec3::new(1.0 / safe(d.x), 1.0 / safe(d.y), 1.0 / safe(d.z));
        let mut best = tmax;
        let mut front = false;
        let mut hit = false;
        let mut stack = [0u32; 64];
        let mut sp = 0usize;
        let mut node = 0usize;
        loop {
            let n = &self.nodes[node];
            if n.count > 0 {
                for t in &self.tris[n.first as usize..(n.first + n.count) as usize] {
                    if let Some((dist, f)) = intersect(t, o, d)
                        && dist < best
                    {
                        best = dist;
                        front = f;
                        hit = true;
                    }
                }
            } else {
                let (l, r) = (n.first as usize, n.first as usize + 1);
                let (tl, tr) = (slab(&self.nodes[l], o, inv, best), slab(&self.nodes[r], o, inv, best));
                match (tl, tr) {
                    (Some(a), Some(b)) => {
                        let (near, far) = if a <= b { (l, r) } else { (r, l) };
                        if sp < stack.len() {
                            stack[sp] = far as u32;
                            sp += 1;
                        }
                        node = near;
                        continue;
                    }
                    (Some(_), None) => {
                        node = l;
                        continue;
                    }
                    (None, Some(_)) => {
                        node = r;
                        continue;
                    }
                    (None, None) => {}
                }
            }
            // Next node from the stack (skipping any now farther than the best hit).
            loop {
                if sp == 0 {
                    return hit.then_some((best, front));
                }
                sp -= 1;
                let next = stack[sp] as usize;
                if slab(&self.nodes[next], o, inv, best).is_some() {
                    node = next;
                    break;
                }
            }
        }
    }

    /// How open the hemisphere over `p` (facing `normal`) is, 1 fully open, and whether `p` is outside solid geometry.
    pub fn occlusion(&self, p: Vec3, normal: Vec3, params: &Params) -> (f32, bool) {
        let n = normal.normalize_or(Vec3::Z);
        let (t1, t2) = n.any_orthonormal_pair();
        // Each point turns the sample pattern by its own angle, so that neighbouring texels' noise differs (the blur evens it).
        let spin = hash3(p) * std::f32::consts::TAU;
        let (ss, cs) = spin.sin_cos();
        let origin = p + n * 2.0;
        let rays = params.rays;
        let (mut occluded, mut back) = (0.0f32, 0usize);
        for i in 0..rays {
            // Cosine-weighted directions: a Fibonacci spiral over the unit disc, lifted onto the hemisphere.
            let r = ((i as f32 + 0.5) / rays as f32).sqrt();
            let phi = i as f32 * 2.399_963_1;
            let (sp, cp) = phi.sin_cos();
            let (x, y) = (r * cp, r * sp);
            let (x, y) = (x * cs - y * ss, x * ss + y * cs);
            let z = (1.0 - r * r).max(0.0).sqrt();
            let d = (t1 * x + t2 * y + n * z).normalize_or(n);
            if let Some((t, front)) = self.closest(origin, d, params.range) {
                occluded += 1.0 - t / params.range;
                if !front {
                    back += 1;
                }
            }
        }
        (1.0 - occluded / rays as f32, back * 4 < rays)
    }
}

fn subdivide(nodes: &mut Vec<Node>, at: usize, tris: &[Tri], centroids: &[Vec3], index: &mut [u32]) {
    let (first, count) = (nodes[at].first as usize, nodes[at].count as usize);
    let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    let (mut cmin, mut cmax) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for &i in &index[first..first + count] {
        let t = &tris[i as usize];
        for v in [t.a, t.a + t.e1, t.a + t.e2] {
            min = min.min(v);
            max = max.max(v);
        }
        cmin = cmin.min(centroids[i as usize]);
        cmax = cmax.max(centroids[i as usize]);
    }
    nodes[at].min = min;
    nodes[at].max = max;
    let extent = cmax - cmin;
    if count <= LEAF || extent.max_element() < 1e-3 {
        return;
    }
    let axis = if extent.x >= extent.y && extent.x >= extent.z { 0 } else if extent.y >= extent.z { 1 } else { 2 };
    let half = count / 2;
    index[first..first + count].select_nth_unstable_by(half, |a, b| centroids[*a as usize][axis].total_cmp(&centroids[*b as usize][axis]));
    let left = nodes.len();
    nodes.push(Node { min: Vec3::ZERO, max: Vec3::ZERO, first: first as u32, count: half as u32 });
    nodes.push(Node { min: Vec3::ZERO, max: Vec3::ZERO, first: (first + half) as u32, count: (count - half) as u32 });
    nodes[at].first = left as u32;
    nodes[at].count = 0;
    subdivide(nodes, left, tris, centroids, index);
    subdivide(nodes, left + 1, tris, centroids, index);
}

/// Where the ray enters the node's box, if before `tmax`.
fn slab(n: &Node, o: Vec3, inv: Vec3, tmax: f32) -> Option<f32> {
    let t0 = (n.min - o) * inv;
    let t1 = (n.max - o) * inv;
    let near = t0.min(t1).max_element().max(0.0);
    let far = t0.max(t1).min_element().min(tmax);
    (near <= far).then_some(near)
}

/// MÃ¶ller-Trumbore: the distance to the triangle and whether the ray meets its front.
fn intersect(t: &Tri, o: Vec3, d: Vec3) -> Option<(f32, bool)> {
    let p = d.cross(t.e2);
    let det = t.e1.dot(p);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - t.a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(t.e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let dist = t.e2.dot(q) * inv;
    // det = -d . (e1 x e2): positive when the ray runs against the front normal.
    (dist > 1e-3).then_some((dist, det > 0.0))
}

fn hash3(p: Vec3) -> f32 {
    let mut h = (p.x.to_bits()).wrapping_mul(0x9E37_79B1) ^ (p.y.to_bits()).wrapping_mul(0x85EB_CA77) ^ (p.z.to_bits()).wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// Bakes the occlusion of every query point (position, normal): 0..254 for closed..open, `INVALID` inside solid geometry.
/// Read from the cache when the same geometry and points were baked before.
pub fn bake_cached(tris: &[[Vec3; 3]], queries: &[(Vec3, Vec3)], params: &Params) -> Vec<u8> {
    let key = fingerprint(tris, queries, params);
    let path = CACHE_DIR.get().map(|d| d.join(format!("ao-{key:016x}.bin")));
    if let Some(data) = path.as_ref().and_then(|p| std::fs::read(p).ok())
        && data.len() == 16 + queries.len()
        && data[..8] == *b"HP3AO\0\0\x01"
    {
        log::info!("baked AO: {} points from the cache", queries.len());
        eprintln!("baked AO: {} points from the cache", queries.len());
        return data[16..].to_vec();
    }
    let t = std::time::Instant::now();
    let occ = Occluders::new(tris);
    let built = t.elapsed();
    let out = bake(&occ, queries, params);
    eprintln!(
        "baked AO: {} points x {} rays against {} triangles in {:?} (BVH {:?})",
        queries.len(),
        params.rays,
        occ.len(),
        t.elapsed(),
        built
    );
    if let Some(p) = path {
        let mut data = Vec::with_capacity(16 + out.len());
        data.extend_from_slice(b"HP3AO\0\0\x01");
        data.extend_from_slice(&(out.len() as u64).to_le_bytes());
        data.extend_from_slice(&out);
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&p, data) {
            eprintln!("baked AO: cannot write {}: {e}", p.display());
        }
    }
    out
}

/// Bakes without the cache, on all cores.
pub fn bake(occ: &Occluders, queries: &[(Vec3, Vec3)], params: &Params) -> Vec<u8> {
    let mut out = vec![INVALID; queries.len()];
    const CHUNK: usize = 2048;
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).max(1);
    let work = Mutex::new(out.chunks_mut(CHUNK).enumerate());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let Some((ci, chunk)) = work.lock().ok().and_then(|mut w| w.next()) else { break };
                    for (k, slot) in chunk.iter_mut().enumerate() {
                        let (p, n) = queries[ci * CHUNK + k];
                        let (ao, valid) = occ.occlusion(p, n, params);
                        *slot = if valid { (ao.clamp(0.0, 1.0) * 254.0).round() as u8 } else { INVALID };
                    }
                }
            });
        }
    });
    out
}

/// Decodes a stored value: `None` inside solid geometry.
pub fn decode(v: u8) -> Option<f32> {
    (v != INVALID).then(|| v as f32 / 254.0)
}

/// FNV-1a over everything the result depends on.
fn fingerprint(tris: &[[Vec3; 3]], queries: &[(Vec3, Vec3)], params: &Params) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |x: u64| {
        for b in x.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    eat(VERSION);
    eat(params.rays as u64);
    eat(params.range.to_bits() as u64);
    eat(tris.len() as u64);
    for t in tris {
        for v in t {
            eat((v.x.to_bits() as u64) << 32 | v.y.to_bits() as u64);
            eat(v.z.to_bits() as u64);
        }
    }
    eat(queries.len() as u64);
    for (p, n) in queries {
        eat((p.x.to_bits() as u64) << 32 | p.y.to_bits() as u64);
        eat((p.z.to_bits() as u64) << 32 | n.x.to_bits() as u64);
        eat((n.y.to_bits() as u64) << 32 | n.z.to_bits() as u64);
    }
    h
}

/// Fills the cells of a `w` x `h` grid that have no value from their neighbours (a few rings outward; the rest open), then
/// softens the grid with a 3x3 box blur. Used per lightmap, whose texels outside its polygon may sit inside walls.
pub fn fill_and_blur(cells: &[Option<f32>], w: usize, h: usize) -> Vec<f32> {
    let mut cur: Vec<Option<f32>> = cells.to_vec();
    for _ in 0..4 {
        if cur.iter().all(Option::is_some) {
            break;
        }
        let prev = cur.clone();
        for y in 0..h {
            for x in 0..w {
                if prev[y * w + x].is_some() {
                    continue;
                }
                let (mut sum, mut n) = (0.0, 0);
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                        if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                            continue;
                        }
                        if let Some(v) = prev[ny as usize * w + nx as usize] {
                            sum += v;
                            n += 1;
                        }
                    }
                }
                if n > 0 {
                    cur[y * w + x] = Some(sum / n as f32);
                }
            }
        }
    }
    let filled: Vec<f32> = cur.iter().map(|v| v.unwrap_or(1.0)).collect();
    let mut out = vec![1.0; w * h];
    for y in 0..h {
        for x in 0..w {
            let (mut sum, mut n) = (0.0, 0);
            for ny in y.saturating_sub(1)..(y + 2).min(h) {
                for nx in x.saturating_sub(1)..(x + 2).min(w) {
                    sum += filled[ny * w + nx];
                    n += 1;
                }
            }
            out[y * w + x] = sum / n as f32;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(a: Vec3, b: Vec3, c: Vec3, d: Vec3) -> [[Vec3; 3]; 2] {
        [[a, b, c], [a, c, d]]
    }

    /// A floor facing up (anticlockwise seen from above) and a wall along x = 0 facing +x.
    fn floor_and_wall() -> Vec<[Vec3; 3]> {
        let s = 1000.0;
        let mut t = Vec::new();
        t.extend(quad(Vec3::new(-s, -s, 0.0), Vec3::new(s, -s, 0.0), Vec3::new(s, s, 0.0), Vec3::new(-s, s, 0.0)));
        t.extend(quad(Vec3::new(0.0, -s, 0.0), Vec3::new(0.0, s, 0.0), Vec3::new(0.0, s, s), Vec3::new(0.0, -s, s)));
        t
    }

    fn params() -> Params {
        Params { rays: 64, range: 128.0, strength: 1.0, direct: 0.3 }
    }

    #[test]
    fn open_floor_is_open() {
        let occ = Occluders::new(&floor_and_wall());
        let (ao, valid) = occ.occlusion(Vec3::new(500.0, 0.0, 0.0), Vec3::Z, &params());
        assert!(valid);
        assert!(ao > 0.99, "{ao}");
    }

    #[test]
    fn crease_is_darker() {
        let occ = Occluders::new(&floor_and_wall());
        let (ao, valid) = occ.occlusion(Vec3::new(4.0, 0.0, 0.0), Vec3::Z, &params());
        assert!(valid);
        assert!(ao < 0.8, "{ao}");
    }

    #[test]
    fn behind_the_wall_is_inside() {
        // The wall faces +x: from x < 0, facing it, every ray meets its back.
        let occ = Occluders::new(&floor_and_wall());
        let (_, valid) = occ.occlusion(Vec3::new(-4.0, 0.0, 50.0), Vec3::X, &params());
        assert!(!valid);
    }

    #[test]
    fn bvh_matches_brute_force() {
        let mut seed = 7u32;
        let mut next = |range: f32| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 * range - range / 2.0
        };
        let mut tris = Vec::new();
        for _ in 0..500 {
            let a = Vec3::new(next(1000.0), next(1000.0), next(1000.0));
            tris.push([a, a + Vec3::new(next(80.0), next(80.0), next(80.0)), a + Vec3::new(next(80.0), next(80.0), next(80.0))]);
        }
        let occ = Occluders::new(&tris);
        let all: Vec<Tri> = tris.iter().map(|t| Tri { a: t[0], e1: t[1] - t[0], e2: t[2] - t[0] }).collect();
        for _ in 0..2000 {
            let o = Vec3::new(next(1200.0), next(1200.0), next(1200.0));
            let d = Vec3::new(next(2.0), next(2.0), next(2.0)).normalize_or(Vec3::X);
            let brute = all.iter().filter_map(|t| intersect(t, o, d)).filter(|h| h.0 < 2000.0).min_by(|a, b| a.0.total_cmp(&b.0));
            let fast = occ.closest(o, d, 2000.0);
            match (brute, fast) {
                (None, None) => {}
                (Some(a), Some(b)) => assert!((a.0 - b.0).abs() < 1e-3 && a.1 == b.1),
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn fill_takes_neighbours_and_blur_evens() {
        let cells = vec![Some(0.5), None, Some(0.5), Some(0.5)];
        let out = fill_and_blur(&cells, 2, 2);
        assert!(out.iter().all(|v| (v - 0.5).abs() < 1e-6), "{out:?}");
    }
}
