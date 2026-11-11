//! The goal hoops and what counts as a goal. Pure: nothing here touches the script VM.
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

/// Whether the ball's path from `prev` to `now` went through `h` the right way: it must start on the front
/// (the side the normal points to), end behind the disc, and cross the plane inside the radius.
///
/// Rubbish in (a NaN position, a zero normal, a radius of zero) scores nothing.
pub fn goal_scored(prev: Vec3, now: Vec3, h: &Hoop) -> bool {
    if !prev.is_finite() || !now.is_finite() || !h.center.is_finite() || !(h.radius > 0.0) {
        return false;
    }
    let n = h.normal.normalize_or_zero();
    if n == Vec3::ZERO {
        return false;
    }
    // Signed distance from the disc's plane: positive is in front of it (where the normal points).
    let d0 = (prev - h.center).dot(n);
    let d1 = (now - h.center).dot(n);
    if d0 <= 0.0 || d1 > 0.0 {
        return false;
    }
    // `d0 - d1 >= d0 > 0`, so the crossing fraction is finite and in 0..1.
    let t = d0 / (d0 - d1);
    let hit = prev + (now - prev) * t;
    let off = hit - h.center;
    // How far the crossing point is from the hoop's axis.
    let radial = off - n * off.dot(n);
    radial.length() <= h.radius
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
}
