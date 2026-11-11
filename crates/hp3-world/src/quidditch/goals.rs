//! Discs in space: the goal hoops, what counts as a goal, and the points around a ring's rim that a ring is
//! drawn with. Pure: nothing here touches the script VM. The Snitch's flight rings (`quidditch::trail`) are
//! discs too, and take their crossing test and their rim from here.
//!
//! A hoop is a disc in space -- a centre, a radius and a normal that points into the pitch, which is the side
//! a shot has to come from. There is no ring mesh in HP3 (`docs/formats/quidditch-arena.md`: the retail
//! `Quid_GoalPosts_SM` carries only a flat quad on each pole), so the ring the player sees is built by
//! `quidditch::ball` and the scoring is this disc test.

use glam::Vec3;

/// One scoring hoop: a disc of `radius` at `center`, facing `normal` (into the pitch). `home` says which side
/// scores by putting the Quaffle through it -- see `arena::hoops`, where the sides are decided.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hoop {
    pub center: Vec3,
    pub radius: f32,
    /// Unit normal, pointing into the pitch: the side a shot must come from.
    pub normal: Vec3,
    /// True when a ball through this hoop scores for the home side (Gryffindor).
    pub home: bool,
}

/// Whether the segment `prev` -> `now` goes through the disc at `center` of `radius`, facing `normal`, **in
/// either direction**, and how far along the segment it did (0..1). The disc test on its own: a goal adds the
/// direction to it, and the Snitch's ring trail (`quidditch::trail`) takes it as it is, because a flight ring
/// counts whichever way it is flown through.
///
/// Rubbish in (a NaN position, a zero normal, a radius of zero) is no crossing.
pub fn disc_crossing(prev: Vec3, now: Vec3, center: Vec3, normal: Vec3, radius: f32) -> Option<f32> {
    if !prev.is_finite() || !now.is_finite() || !center.is_finite() || !(radius > 0.0) {
        return None;
    }
    let n = normal.normalize_or_zero();
    if n == Vec3::ZERO {
        return None;
    }
    // Signed distance from the disc's plane: positive is in front of it (where the normal points).
    let d0 = (prev - center).dot(n);
    let d1 = (now - center).dot(n);
    // Front to back, or back to front; a segment that stays on one side crosses nothing.
    if !(d0 > 0.0 && d1 <= 0.0) && !(d0 < 0.0 && d1 >= 0.0) {
        return None;
    }
    // The ends are strictly on opposite sides of the plane, so `d0 - d1` has `d0`'s sign and is never 0: the
    // crossing fraction is finite and in 0..1.
    let t = d0 / (d0 - d1);
    let hit = prev + (now - prev) * t;
    let off = hit - center;
    // How far the crossing point is from the disc's axis.
    let radial = off - n * off.dot(n);
    (radial.length() <= radius).then_some(t)
}

/// Two unit vectors that span the disc whose axis is `n`.
pub fn disc_axes(n: Vec3) -> (Vec3, Vec3) {
    let n = n.normalize_or_zero();
    if n == Vec3::ZERO {
        return (Vec3::X, Vec3::Z);
    }
    // Anything not along the axis will do to start from; the goal hoops' axes lie along y, so z is the one.
    let seed = if n.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
    let u = n.cross(seed).normalize_or_zero();
    if u == Vec3::ZERO {
        return (Vec3::X, Vec3::Z);
    }
    (u, n.cross(u))
}

/// `beads` points evenly around the rim of the disc at `center` of `radius` facing `normal`, in its own plane:
/// the little balls a ring is drawn with, since the retail data has no ring mesh at all
/// (`docs/formats/quidditch-arena.md`). Both the goal hoops (`quidditch::ball`) and the Snitch's flight rings
/// (`quidditch::rings`) are built from it.
pub fn ring_points(center: Vec3, normal: Vec3, radius: f32, beads: usize) -> Vec<Vec3> {
    let (u, v) = disc_axes(normal);
    (0..beads)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / beads.max(1) as f32;
            center + (u * a.cos() + v * a.sin()) * radius
        })
        .collect()
}

/// Whether the ball's path from `prev` to `now` went through `h` the right way: it must start on the front
/// (the side the normal points to), end behind the disc, and cross the plane inside the radius.
///
/// Rubbish in (a NaN position, a zero normal, a radius of zero) scores nothing.
pub fn goal_scored(prev: Vec3, now: Vec3, h: &Hoop) -> bool {
    // Only from the front. A NaN here compares false and is caught by the disc test below.
    if (prev - h.center).dot(h.normal.normalize_or_zero()) <= 0.0 {
        return false;
    }
    disc_crossing(prev, now, h.center, h.normal, h.radius).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hoop at the origin facing -y (as the north goal's do: into the pitch).
    fn hoop() -> Hoop {
        Hoop { center: Vec3::ZERO, radius: 120.0, normal: Vec3::new(0.0, -1.0, 0.0), home: true }
    }

    #[test]
    fn goal_needs_the_front_side_and_the_ring() {
        let h = hoop();
        // Through the middle, from the front (the pitch side, -y) towards the back.
        assert!(goal_scored(Vec3::new(0.0, -50.0, 0.0), Vec3::new(0.0, 50.0, 0.0), &h));
        // Both ends on the same side: no crossing.
        assert!(!goal_scored(Vec3::new(0.0, -50.0, 0.0), Vec3::new(0.0, -10.0, 0.0), &h));
        assert!(!goal_scored(Vec3::new(0.0, 10.0, 0.0), Vec3::new(0.0, 50.0, 0.0), &h));
    }

    #[test]
    fn rubbish_never_scores() {
        let h = hoop();
        let good = Vec3::new(0.0, -50.0, 0.0);
        assert!(!goal_scored(Vec3::new(f32::NAN, 0.0, 0.0), Vec3::ZERO, &h));
        assert!(!goal_scored(good, Vec3::splat(f32::INFINITY), &h));
        assert!(!goal_scored(good, Vec3::new(0.0, 50.0, 0.0), &Hoop { radius: 0.0, ..h }));
        assert!(!goal_scored(good, Vec3::new(0.0, 50.0, 0.0), &Hoop { normal: Vec3::ZERO, ..h }));
    }

    /// The bare disc test counts a crossing either way round (a flight ring is flown through from both sides),
    /// and only inside the radius. A goal is that test plus the side the shot came from.
    #[test]
    fn a_disc_is_crossed_from_either_side_but_only_inside_its_radius() {
        let (c, n, r) = (Vec3::ZERO, Vec3::Y, 120.0);
        let front = disc_crossing(Vec3::new(0.0, 50.0, 0.0), Vec3::new(0.0, -50.0, 0.0), c, n, r);
        let back = disc_crossing(Vec3::new(0.0, -50.0, 0.0), Vec3::new(0.0, 50.0, 0.0), c, n, r);
        assert!(front.is_some_and(|t| (t - 0.5).abs() < 1e-6), "{front:?}");
        assert!(back.is_some_and(|t| (t - 0.5).abs() < 1e-6), "{back:?}");
        // Past the rim, and never reaching the plane at all.
        assert_eq!(disc_crossing(Vec3::new(r + 1.0, 50.0, 0.0), Vec3::new(r + 1.0, -50.0, 0.0), c, n, r), None);
        assert_eq!(disc_crossing(Vec3::new(0.0, 50.0, 0.0), Vec3::new(0.0, 10.0, 0.0), c, n, r), None);
        // Rubbish in, nothing out.
        assert_eq!(disc_crossing(Vec3::splat(f32::NAN), Vec3::ZERO, c, n, r), None);
        assert_eq!(disc_crossing(Vec3::new(0.0, 50.0, 0.0), Vec3::new(0.0, -50.0, 0.0), c, Vec3::ZERO, r), None);
        assert_eq!(disc_crossing(Vec3::new(0.0, 50.0, 0.0), Vec3::new(0.0, -50.0, 0.0), c, n, 0.0), None);
    }
}
