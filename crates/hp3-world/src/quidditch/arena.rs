//! Arena geometry shared by the Quidditch systems: the flight volume, and the list of retail
//! `Quid_Stadium` static meshes that build the pitch.
//!
//! Every number here comes from `docs/formats/quidditch-arena.md` (the Task 1 spike), which read the meshes and
//! the placements out of `HP3_Quidditch.unr`. Units are Unreal units; Z is up. Nothing in this file touches the
//! script VM: `quidditch::glue` spawns what `arena_spawn_list` describes.

use glam::Vec3;

use super::goals::Hoop;

/// Axis-aligned flight volume: x/y within `center +/- half`, z from
/// `center.z - half.z` up to `ceiling`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArenaBounds {
    pub center: Vec3,
    pub half: Vec3,
    pub ceiling: f32,
}

/// Where the retail map places the four pitch quadrants (their shared pivot).
pub const PITCH_CENTER: Vec3 = Vec3::new(271.9, 116.0, -1009.9);
/// Pitch floor: the goal posts' base, and where the map's Quidditch players stand.
pub const GROUND_Z: f32 = -960.0;
/// Stand top (the pitch mesh reaches world 2308.7; the tallest hoop top is 1565).
pub const CEILING_Z: f32 = 2300.0;
/// How far above the ground the flight volume starts, so nothing flies inside the pitch.
pub const FLY_CLEARANCE: f32 = 150.0;
/// Both goals stand at `y = +-GOAL_Y`; the flight volume is centred between them.
pub const GOAL_Y: f32 = 4096.0;
/// Playfield kept behind each goal (so both ends are symmetric).
pub const GOAL_MARGIN: f32 = 500.0;
/// Playfield half-width, matching the flat `Quttish_Pitch_SM` half-width 3905.
pub const PLAYFIELD_HALF_X: f32 = 3900.0;

/// Local (y, z) of the three hoop faces of `Quid_GoalPosts_SM`: the flat quads on top of its poles. The
/// world centres below were turned out of these with the engine's own rotator maths (Task 6); they stay here
/// because `glue::log_hoops` checks the one against the other.
pub const HOOPS_LOCAL: [(f32, f32); 3] = [(610.0, 1118.0), (0.0, 2405.0), (-610.0, 1705.0)];

/// World (x, z) of the three hoop centres of the **north** goal (`y = +GOAL_Y`), from the verified table in
/// `docs/formats/quidditch-arena.md`: the goal's local +Y lies along world -X after its quarter turn.
pub const HOOPS_NORTH_XZ: [(f32, f32); 3] = [(-354.0, 158.0), (256.0, 1445.0), (866.0, 745.0)];
/// The same for the **south** goal (`y = -GOAL_Y`), whose opposite yaw mirrors the three poles in x.
pub const HOOPS_SOUTH_XZ: [(f32, f32); 3] = [(866.0, 158.0), (256.0, 1445.0), (-354.0, 745.0)];
/// Half the width of a hoop face's quad: what a shot has to fit through.
pub const HOOP_RADIUS: f32 = 120.0;

/// The six scoring hoops, the north goal's three first, each facing into the pitch (the side a shot has to
/// come from).
///
/// **Which end is whose.** Harry's team (Gryffindor, the home side) **defends the south goal** (`y =
/// -GOAL_Y`, where `harry_start` puts him) and **attacks the north** (`y = +GOAL_Y`). So a ball through a
/// north hoop is a home goal (`home: true`) and one through a south hoop is the opponent's. Nothing in the
/// retail map says which end belongs to whom -- this is the choice this engine makes, and `harry_start` is
/// the only thing that has to agree with it.
pub fn hoops() -> Vec<Hoop> {
    let mut out = Vec::with_capacity(6);
    for (y, table, home) in [(GOAL_Y, HOOPS_NORTH_XZ, true), (-GOAL_Y, HOOPS_SOUTH_XZ, false)] {
        // Into the pitch: the north goal's hoops look down -y, the south goal's up +y.
        let normal = Vec3::new(0.0, -y.signum(), 0.0);
        for (x, z) in table {
            out.push(Hoop { center: Vec3::new(x, y, z), radius: HOOP_RADIUS, normal, home });
        }
    }
    out
}

/// The stadium meshes, by their full package path.
pub const PITCH: &str = "HP3_ExteriorArch_SM.Quid_Stadium.QUid_Pitch_SM";
pub const GOAL_POSTS: &str = "HP3_ExteriorArch_SM.Quid_Stadium.Quid_GoalPosts_SM";
pub const STAIRS: &str = "HP3_ExteriorArch_SM.Quid_Stadium.Q_stairs_SM";

/// The flight volume. It is centred on the goals (`y = 0`), not on the pitch mesh's own centre
/// (`y = 116`), so the same margin lies behind both ends.
pub fn bounds() -> ArenaBounds {
    let floor = GROUND_Z + FLY_CLEARANCE;
    ArenaBounds {
        center: Vec3::new(PITCH_CENTER.x, 0.0, (floor + CEILING_Z) * 0.5),
        half: Vec3::new(PLAYFIELD_HALF_X, GOAL_Y + GOAL_MARGIN, (CEILING_Z - floor) * 0.5),
        ceiling: CEILING_Z,
    }
}

/// One static mesh of the stadium, as the retail map places it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArenaPiece {
    /// Full package path of the static mesh.
    pub mesh: &'static str,
    pub pos: Vec3,
    /// Rotation as Unreal's (pitch, yaw, roll) in 65536ths of a turn.
    pub rot: [i32; 3],
    /// `DrawScale3D`: the pitch is one quadrant mirrored along x, which needs `x = -1`.
    pub scale: Vec3,
}

fn piece(mesh: &'static str, pos: Vec3, yaw: i32) -> ArenaPiece {
    ArenaPiece { mesh, pos, rot: [0, yaw, 0], scale: Vec3::ONE }
}

/// Every stadium mesh to spawn, with the placement the retail `HP3_Quidditch` uses: the four pitch quadrants,
/// the two goals and the seven stand stairs. Towers and walls exist in the package but the map never places
/// them, so they are left out (their positions would be invented).
pub fn arena_spawn_list() -> Vec<ArenaPiece> {
    let mirror = Vec3::new(-1.0, 1.0, 1.0);
    let mut out = Vec::with_capacity(13);
    // The pitch is one quadrant mesh four times around the same pivot: as placed, mirrored along x, turned
    // half round, and both.
    for (yaw, scale) in [(0, Vec3::ONE), (0, mirror), (-32768, Vec3::ONE), (-32768, mirror)] {
        out.push(ArenaPiece { mesh: PITCH, pos: PITCH_CENTER, rot: [0, yaw, 0], scale });
    }
    // The goals face each other: after a quarter turn the mesh's local Y (its three poles) lies along world X
    // and the hoop faces look along Y, towards the pitch centre.
    out.push(piece(GOAL_POSTS, Vec3::new(256.0, GOAL_Y, GROUND_Z), 16384));
    out.push(piece(GOAL_POSTS, Vec3::new(256.0, -GOAL_Y, GROUND_Z), -16384));
    // The stand stairs, each turned to face the pitch.
    const STAIRS_Z: f32 = 824.8;
    for (x, y, yaw) in [
        (4026.4, 2924.6, 3504),
        (2364.0, 5880.7, 7456),
        (3329.3, -4333.8, -5344),
        (-1812.9, 5889.4, 25112),
        (-2786.9, -4326.7, 38128),
        (4285.9, -809.1, -848),
        (-3733.3, 1038.2, 31968),
    ] {
        out.push(piece(STAIRS, Vec3::new(x, y, STAIRS_Z), yaw));
    }
    out
}

/// Where Harry's broom pawn starts: over his own (south) goal line, a third of the way up the flight volume,
/// so the pitch and the far goal are both ahead of him.
pub fn harry_start(b: &ArenaBounds) -> Vec3 {
    let floor = b.center.z - b.half.z;
    Vec3::new(b.center.x, -(GOAL_Y - 800.0), floor + (b.ceiling - floor) / 3.0)
}

/// Where Bludgers are launched from: the middle of the pitch at mid height, as the real game releases the
/// balls at the centre circle. Harry's start lies inside the launch band around it.
pub fn bludger_home(b: &ArenaBounds) -> Vec3 {
    b.center
}

/// `p` brought inside the flight volume (`x`/`y` within `center +- half`, `z` from the volume's floor up to
/// its ceiling), and the arena centre when it is not a finite vector. It is both what `flight::confine` holds
/// the broom to and what makes a pure-side position safe to write to an actor's `Location`.
pub fn clamp_into(b: &ArenaBounds, p: Vec3) -> Vec3 {
    if !p.is_finite() {
        return b.center;
    }
    let lo = Vec3::new(b.center.x - b.half.x, b.center.y - b.half.y, b.center.z - b.half.z);
    let hi = Vec3::new(b.center.x + b.half.x, b.center.y + b.half.y, b.ceiling.max(lo.z));
    p.clamp(lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stadium footprint: the four `QUid_Pitch_SM` quadrants around the retail pitch centre cover
    /// x +-4283 and y +-6630 of it, stands included.
    fn footprint() -> (Vec3, Vec3) {
        let c = Vec3::new(PITCH_CENTER.x, PITCH_CENTER.y, 0.0);
        (c - Vec3::new(4283.0, 6630.0, 0.0), c + Vec3::new(4283.0, 6630.0, 0.0))
    }

    #[test]
    fn arena_pieces_inside_bounds_or_stands() {
        let b = bounds();
        let (lo, hi) = footprint();
        let pieces = arena_spawn_list();
        assert_eq!(pieces.len(), 13, "the retail pitch (4), goals (2) and stairs (7)");
        assert_eq!(pieces.iter().filter(|p| p.mesh == PITCH).count(), 4);
        assert_eq!(pieces.iter().filter(|p| p.mesh == STAIRS).count(), 7);
        for p in &pieces {
            assert!(p.pos.is_finite(), "{}: {:?}", p.mesh, p.pos);
            // Everything belongs to the stadium: inside the footprint the pitch quadrants cover.
            assert!(p.pos.x >= lo.x && p.pos.x <= hi.x, "{} x {} outside the stadium", p.mesh, p.pos.x);
            assert!(p.pos.y >= lo.y && p.pos.y <= hi.y, "{} y {} outside the stadium", p.mesh, p.pos.y);
            // The stairs are stand pieces and may sit outside the playfield; everything else is inside it.
            if p.mesh == STAIRS {
                // Stand pieces all the same: above the turf, below the stand tops, and not on the pitch.
                assert!(p.pos.z > GROUND_Z && p.pos.z < CEILING_Z, "{} z {} is not in the stands", p.mesh, p.pos.z);
                assert!((p.pos.x - b.center.x).abs() > 1500.0 || p.pos.y.abs() > GOAL_Y, "stairs at {:?} stand on the pitch", p.pos);
                continue;
            }
            assert!((p.pos.x - b.center.x).abs() <= b.half.x, "{} x {} outside the playfield", p.mesh, p.pos.x);
            assert!((p.pos.y - b.center.y).abs() <= b.half.y, "{} y {} outside the playfield", p.mesh, p.pos.y);
            assert!(p.pos.z >= GROUND_Z - 100.0 && p.pos.z <= b.ceiling, "{} z {} outside the volume", p.mesh, p.pos.z);
        }
    }

    #[test]
    fn arena_has_goal_posts_at_both_ends() {
        let b = bounds();
        let goals: Vec<_> = arena_spawn_list().into_iter().filter(|p| p.mesh == GOAL_POSTS).collect();
        assert_eq!(goals.len(), 2, "two goals");
        let north = goals.iter().find(|p| p.pos.y > 0.0).expect("a goal at the north end");
        let south = goals.iter().find(|p| p.pos.y < 0.0).expect("a goal at the south end");
        assert_eq!(north.pos.y, GOAL_Y);
        assert_eq!(south.pos.y, -GOAL_Y);
        // Symmetric about the bounds centre, which the goals define.
        assert_eq!(b.center.y, 0.0);
        assert_eq!(north.pos.x, south.pos.x);
        assert_eq!(north.pos.z, south.pos.z);
        // The two ends face each other: opposite yaw, so each goal's hoops look at the pitch centre.
        assert_eq!(north.rot[1], -south.rot[1]);
        assert_ne!(north.rot[1], 0);
        // The same margin lies behind both goals.
        assert_eq!(b.half.y - GOAL_Y, GOAL_MARGIN);
    }

    #[test]
    fn six_hoops_three_per_goal_facing_the_pitch() {
        let b = bounds();
        let hoops = hoops();
        assert_eq!(hoops.len(), 6, "three hoops on each of the two goals");
        for (side, home) in [(GOAL_Y, true), (-GOAL_Y, false)] {
            let mine: Vec<_> = hoops.iter().filter(|h| h.center.y == side).collect();
            assert_eq!(mine.len(), 3, "three hoops at y {side}");
            for h in mine {
                assert_eq!(h.radius, HOOP_RADIUS);
                assert_eq!(h.home, home, "{h:?}");
                // Facing into the pitch: a unit normal straight at y = 0.
                assert!((h.normal.length() - 1.0).abs() < 1e-6, "{h:?}");
                assert_eq!(h.normal, Vec3::new(0.0, -side.signum(), 0.0), "{h:?}");
                assert!((h.center + h.normal).y.abs() < side.abs(), "{h:?} does not face the pitch");
                // And inside the flight volume, or nothing could ever be shot through it.
                assert!((h.center.x - b.center.x).abs() <= b.half.x, "{h:?}");
                assert!(h.center.z >= b.center.z - b.half.z && h.center.z <= b.ceiling, "{h:?}");
            }
            // Three different poles at three different heights.
            let zs: Vec<f32> = hoops.iter().filter(|h| h.center.y == side).map(|h| h.center.z).collect();
            assert!(zs.iter().all(|z| zs.iter().filter(|o| *o == z).count() == 1), "{zs:?} repeat a height");
        }
        // Harry defends the south goal, so the home hoops are the ones he is flying at.
        let harry = harry_start(&b);
        let home: Vec<_> = hoops.iter().filter(|h| h.home).collect();
        assert!(home.iter().all(|h| h.center.y.signum() != harry.y.signum()), "home scores at the far goal");
    }

    #[test]
    fn bounds_sit_between_the_ground_and_the_stand_tops() {
        let b = bounds();
        assert!(b.half.x > 0.0 && b.half.y > 0.0 && b.half.z > 0.0);
        assert_eq!(b.center.z - b.half.z, GROUND_Z + FLY_CLEARANCE);
        assert_eq!(b.center.z + b.half.z, b.ceiling);
        assert_eq!(b.ceiling, CEILING_Z);
    }

    #[test]
    fn harry_start_and_bludger_home_are_in_the_air_over_the_pitch() {
        let b = bounds();
        for p in [harry_start(&b), bludger_home(&b)] {
            assert!((p.x - b.center.x).abs() <= b.half.x, "{p:?}");
            assert!((p.y - b.center.y).abs() <= b.half.y, "{p:?}");
            assert!(p.z >= b.center.z - b.half.z && p.z <= b.ceiling, "{p:?}");
            assert!(p.z > GROUND_Z + 100.0, "{p:?} is on the ground");
        }
        // Harry must start inside the band in which Bludgers are launched at him, or none ever would be.
        let t = crate::quidditch::bludger::BludgerTuning::default();
        let d = harry_start(&b).distance(bludger_home(&b));
        assert!(d >= t.launch_inner && d <= t.launch_outer, "kickoff distance {d} outside {}..{}", t.launch_inner, t.launch_outer);
    }

    #[test]
    fn clamp_into_brings_a_point_into_the_volume_and_is_always_finite() {
        let b = bounds();
        let inside = Vec3::new(b.center.x, 1000.0, 500.0);
        assert_eq!(clamp_into(&b, inside), inside);
        let far = clamp_into(&b, Vec3::new(1e9, -1e9, 1e9));
        assert_eq!(far, Vec3::new(b.center.x + b.half.x, b.center.y - b.half.y, b.ceiling));
        for bad in [Vec3::new(f32::NAN, 0.0, 0.0), Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)] {
            let p = clamp_into(&b, bad);
            assert!(p.is_finite(), "{bad:?} -> {p:?}");
            assert!((p.x - b.center.x).abs() <= b.half.x && (p.y - b.center.y).abs() <= b.half.y);
        }
    }
}
