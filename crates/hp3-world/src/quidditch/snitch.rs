//! Golden Snitch AI, catch progress bar and catch window.

use glam::Vec3;

use super::arena::ArenaBounds;
use super::flight::BroomState;
use super::rng::Rng;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnitchTuning {
    pub neutral_radius: f32,
    pub max_gain_radius: f32,
    pub max_loss_radius: f32,
    pub gain_rate: f32,
    pub loss_rate: f32,
    pub max_catch_time: f32,
    pub max_tries: u32,
    pub catch_radius: f32,
    /// How wide the Snitch loops around its own flight path (uu), and how fast it goes round (rad/s). HP1's
    /// Snitch visibly corkscrews as it travels, which is how the player picks it out of the sky; ours does the
    /// same. 0 for either is a Snitch that flies the bare path.
    pub circle_radius: f32,
    pub circle_rate: f32,
    /// **How tightly the flight path may bend (rad/s at `SPEED_NEAR`).** The path is not a run of straight
    /// lines between waypoints any more -- it is a heading that turns *toward* the next waypoint at a bounded
    /// rate, so the path is made of gentle arcs and the ring chain dropped on it reads as one road.
    ///
    /// The limit is really a **curvature**: `path_turn_rate / SPEED_NEAR` rad per uu flown, which is the same
    /// at either speed, so the turn between two rings a `spacing` apart does not depend on how fast the Snitch
    /// happens to be going. 0.58 rad/s at 875 uu/s is a turn radius of **1508 uu**, and 600 uu of `spacing`
    /// over it is 0.40 rad = 23 degrees, inside `trail::CHAIN_MAX_TURN`. It is not wider than that for a
    /// reason: the arena is 7060 uu across the short way once the margins are off it, and a turn radius much
    /// over 1500 leaves a Snitch that cannot cross it without hugging a wall.
    pub path_turn_rate: f32,
    /// How steeply the path may climb or dive (rad). Capped well below vertical: a Snitch standing on its tail
    /// would need the whole height of the arena to level off again, and the chase camera loses it.
    pub max_pitch: f32,
    /// Room kept between the flight path and the arena wall, on top of `circle_radius` (uu): the flight rings
    /// are dropped on the path and are `trail::TrailTuning::radius` wide, so this is that radius.
    pub path_margin: f32,
    /// How near a waypoint counts as reached (uu). A waypoint is also given up on the moment it falls behind,
    /// which is what stops a Snitch whose turn circle is wider than this from orbiting it for ever.
    pub path_capture: f32,
    /// How far off the next waypoint must be (uu), so the Snitch commits to a long gentle arc instead of
    /// picking its way between nearby points.
    pub path_min_hop: f32,
    /// Half the forward cone the next waypoint is looked for in (rad): a point behind the Snitch would be a
    /// U-turn, which the turn limit makes into a long slow loop. Preferred, not required -- with nothing in
    /// the cone the farthest-forward candidate is taken.
    pub path_cone: f32,
    /// The **dodge**: the moment the catch window opens the Snitch jinks sideways and bursts away at
    /// `dodge_speed` (uu/s) for `dodge_time` seconds, turning `dodge_turn` times as tightly as its path
    /// otherwise may. Harry has to have the speed from his rings to stay on it. 0 time is no dodge.
    pub dodge_speed: f32,
    pub dodge_time: f32,
    pub dodge_turn: f32,
}

impl Default for SnitchTuning {
    /// Values in Unreal units (a walking human is ~440 uu/s, a broom cruises
    /// ~450 uu/s). The radii are distances, so rescaling the speeds left them alone.
    fn default() -> Self {
        SnitchTuning {
            // Full gain at or inside this distance.
            max_gain_radius: 600.0,
            // Progress neither rises nor falls at exactly this distance.
            neutral_radius: 1500.0,
            // Full loss at or beyond this distance; progress is zeroed (Lost).
            max_loss_radius: 4000.0,
            // Progress per second (the bar spans 0..1).
            gain_rate: 0.25,
            loss_rate: 0.15,
            // Seconds the catch window stays open once the bar is full.
            // Two seconds, and a tighter reach: the catch is a moment, not a stretch.
            max_catch_time: 2.0,
            max_tries: 3,
            // Harry must be this close with the catch key held. **350 + `circle_radius`**: every distance
            // here is measured to the Snitch's real, circling position, so a Snitch looping 150 uu around its
            // path is up to 150 uu farther away than one flying the bare path. Widening the catch by exactly
            // the loop leaves the catch as hard as it was before the circling; nothing else was widened (the
            // progress radii are thousands of uu, and 150 uu of wobble in them is noise).
            catch_radius: 320.0,
            circle_radius: 150.0,
            circle_rate: 4.5,
            // 0.4868 rad/s at `SPEED_NEAR` (734): the same 1508 uu turn radius as 0.58 at the old 875.
            path_turn_rate: 0.4868,
            max_pitch: 0.35,
            path_margin: 220.0,
            path_capture: 600.0,
            path_min_hop: 2500.0,
            path_cone: std::f32::consts::FRAC_PI_3,
            dodge_speed: 955.0,
            dodge_time: 0.9,
            dodge_turn: 2.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnitchEvent {
    ClosingIn,
    Lost,
    CatchStart,
    Caught,
    Missed,
    OutOfTries,
}

/// Snitch speeds (uu/s): idle, and when Harry is right on top of it. Half of what the first pass flew at and
/// then a quarter *faster* again at the owner's word (x1.25), against a broom that lost a quarter: the near
/// speed 875 sits just under the boost's 900, so a boosting Harry still closes on it, slowly.
///
/// Slowed again for the ring-driven broom: Harry now cruises at 240 uu/s and only the charge of a run of rings
/// (up to 760) brings him up to the Snitch, which flies 260 idle and 600 with him close.
const SPEED_IDLE: f32 = 318.0;
const SPEED_NEAR: f32 = 734.0;
/// Longest dt a single step integrates; larger values are clamped.
const MAX_DT: f32 = 1.0;

/// `a` folded into -pi..pi: how far round, and which way round, is nearer.
fn short_angle(a: f32) -> f32 {
    if !a.is_finite() {
        return 0.0;
    }
    let a = a.rem_euclid(std::f32::consts::TAU);
    if a > std::f32::consts::PI { a - std::f32::consts::TAU } else { a }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Chasing,
    Catching { elapsed: f32 },
    Done,
}

pub struct Snitch {
    tuning: SnitchTuning,
    rng: Rng,
    /// The arena, and the same box brought in by `circle_radius`: the path is flown inside the inner box, so
    /// the loops around it never leave the arena and the radius never has to be cut short.
    lo: Vec3,
    hi: Vec3,
    path_lo: Vec3,
    path_hi: Vec3,
    /// Where the Snitch would be with no circling (the point that walks the waypoints), and where it really
    /// is: the path point plus this frame's loop offset. `pos` is what everything else sees -- the catch, the
    /// progress and the actor the VM moves.
    path: Vec3,
    pos: Vec3,
    waypoint: Vec3,
    /// Which way the path is going now, as a unit vector: the thing the steering turns and the path follows.
    /// It is what the rings' normals are taken from, so the chain faces along the curve.
    heading: Vec3,
    /// Which way round the yaw is turning to get out of a wall (+1, -1), or 0 for a path with no wall to
    /// worry about. Held until the heading points out of the wall: see `steer`.
    escape: f32,
    /// How far round the loop it is (rad).
    angle: f32,
    progress: f32,
    tries: u32,
    phase: Phase,
    /// Whether Harry has the run of flight rings the catch needs (HP1's `PM_Hoops` rule, which the owner asked
    /// for): the bar may fill without it, but the catch window will not open. Set every frame from the ring
    /// trail's streak (`quidditch::rings`); true for a Snitch nobody tells, which is every unit test and any
    /// caller with no trail, so the rule costs nothing where there are no rings.
    rings_ready: bool,
    /// Seconds left of the dodge the opening of the catch window set off (0: not dodging).
    dodge: f32,
}

impl Snitch {
    pub fn new(seed: u64, arena: &ArenaBounds) -> Snitch {
        Snitch::with_tuning(seed, arena, SnitchTuning::default())
    }

    pub fn with_tuning(seed: u64, arena: &ArenaBounds, tuning: SnitchTuning) -> Snitch {
        let lo = arena.center - arena.half;
        let hi_z = arena.ceiling.min(arena.center.z + arena.half.z).max(lo.z);
        let hi = Vec3::new(arena.center.x + arena.half.x, arena.center.y + arena.half.y, hi_z);
        // Room for the loop on every axis, and never more than half the box (a flat or tiny arena just gets a
        // smaller circle rather than an inside-out one).
        let span = (hi - lo).min_element().max(0.0);
        let r = if tuning.circle_radius.is_finite() { tuning.circle_radius.clamp(0.0, span * 0.5) } else { 0.0 };
        // And room for the flight rings dropped on the path on top of that, so a ring of the trail is inside
        // the arena with its rim and not just its centre. Both margins together never eat more than the box.
        let m = if tuning.path_margin.is_finite() { tuning.path_margin.clamp(0.0, (span * 0.5 - r).max(0.0)) } else { 0.0 };
        let mut s = Snitch {
            tuning: SnitchTuning { circle_radius: r, path_margin: m, ..tuning },
            rng: Rng::new(seed),
            lo,
            hi,
            path_lo: lo + Vec3::splat(r + m),
            path_hi: hi - Vec3::splat(r + m),
            path: Vec3::ZERO,
            pos: Vec3::ZERO,
            waypoint: Vec3::ZERO,
            heading: Vec3::X,
            escape: 0.0,
            angle: 0.0,
            progress: 0.0,
            tries: 0,
            phase: Phase::Chasing,
            rings_ready: true,
            dodge: 0.0,
        };
        // The loop starts somewhere of its own, so two Snitches of different seeds are not in step.
        s.angle = s.rng.unit() * std::f32::consts::TAU;
        // Where it starts: in the **middle** of the flight box, not anywhere in it. A heading-limited path
        // cannot be dropped a few hundred uu off a wall pointing at it -- turning away takes up to a turn
        // radius of room -- and it would spend its first seconds pressed against the arena. A turn radius of
        // margin (and never more than a quarter of the box, for the short way up) is room to turn.
        let turn = (s.turn_radius() * 2.0).min(1e6);
        let margin = (s.path_hi - s.path_lo) * 0.3;
        let keep = Vec3::new(turn.min(margin.x), turn.min(margin.y), turn.min(margin.z)).max(Vec3::ZERO);
        let r3 = Vec3::new(s.rng.unit(), s.rng.unit(), s.rng.unit());
        s.path = (s.path_lo + keep) + ((s.path_hi - keep) - (s.path_lo + keep)) * r3;
        // A heading to start on (level, so the first arc is a readable one), and the first waypoint picked in
        // front of it like every one after.
        let yaw = s.rng.unit() * std::f32::consts::TAU;
        s.heading = Vec3::new(yaw.cos(), yaw.sin(), 0.0);
        s.waypoint = s.pick_waypoint();
        s.pos = s.circling();
        s
    }

    /// A random point of the box waypoints are drawn from: the flight box held off the wall by `wall_hold`,
    /// so the steering is not forever being pulled into a wall it then has to lean out of.
    fn random_point(&mut self) -> Vec3 {
        let r = Vec3::new(self.rng.unit(), self.rng.unit(), self.rng.unit());
        let (lo, hi) = self.aim_box();
        lo + (hi - lo) * r
    }

    /// How close to the wall the path is allowed to settle (uu): a fifth of a turn radius. Inside it the
    /// steering leans away whatever the waypoint says, which is what keeps the path clear of the hard clamp --
    /// and the clamp is the one thing that *would* put a corner in the path, because a clamped frame covers
    /// less than `speed * dt` and so leaves the turn limit behind.
    fn wall_hold(&self) -> f32 {
        (self.turn_radius().min(1e6) * 0.2).max(0.0)
    }

    /// The box waypoints are drawn from and the path is steered back into: the flight box minus `wall_hold`,
    /// or the flight box itself where there is not the room for it.
    fn aim_box(&self) -> (Vec3, Vec3) {
        let room = (self.path_hi - self.path_lo).min_element() * 0.25;
        let h = Vec3::splat(self.wall_hold().min(room.max(0.0)));
        (self.path_lo + h, self.path_hi - h)
    }

    /// The turn limit as a curvature: rad per uu flown. Taken from `path_turn_rate` at `SPEED_NEAR` so the
    /// bend of the path -- and so the turn between two rings a fixed distance apart -- is the same however
    /// fast the Snitch is going. Its reciprocal is the tightest turn radius the path will ever fly.
    fn curvature(&self) -> f32 {
        let rate = if self.tuning.path_turn_rate.is_finite() { self.tuning.path_turn_rate.max(0.0) } else { 0.0 };
        rate / SPEED_NEAR
    }

    /// The tightest turn radius the path can fly (uu); `f32::MAX` for a Snitch tuned not to turn at all.
    pub(super) fn turn_radius(&self) -> f32 {
        let k = self.curvature();
        if k > 0.0 { 1.0 / k } else { f32::MAX }
    }

    /// Picks the next waypoint: `CANDIDATES` random points of the inner box, of which the first one both far
    /// enough off (`path_min_hop`) and inside the forward cone (`path_cone`) is taken. The cone is what keeps
    /// the path from ever being asked to double back, and the distance is what makes each leg a long arc
    /// rather than a jink. With nothing in the cone the farthest-forward candidate is taken, and the turn
    /// limit then makes that into a wide loop rather than a reversal.
    fn pick_waypoint(&mut self) -> Vec3 {
        /// How many points are looked at. All of them are drawn every time, so the randomness a waypoint costs
        /// never depends on which one was taken: same seed, same path.
        const CANDIDATES: usize = 8;
        let mut pts = [Vec3::ZERO; CANDIDATES];
        for p in pts.iter_mut() {
            *p = self.random_point();
        }
        let hop = if self.tuning.path_min_hop.is_finite() { self.tuning.path_min_hop.max(0.0) } else { 0.0 };
        let cone = if self.tuning.path_cone.is_finite() { self.tuning.path_cone.clamp(0.0, std::f32::consts::PI) } else { 0.0 };
        let want = cone.cos();
        let (mut best, mut best_dot) = (pts[0], f32::MIN);
        for p in pts {
            let to = p - self.path;
            let d = to.length();
            let fwd = if d > 1e-3 { to.dot(self.heading) / d } else { -1.0 };
            if d >= hop && fwd >= want {
                return p;
            }
            if fwd > best_dot {
                best_dot = fwd;
                best = p;
            }
        }
        best
    }

    /// `v` with its climb or dive brought inside `max_pitch`, keeping its heading on the ground. Both the aim
    /// and the heading are held to it: an aim outside the cap is the one thing that can send the turn the long
    /// way round the sphere (the shortest great circle to a steep dive runs through the yaw behind you), which
    /// is a reversal and exactly what the owner complained of.
    fn pitch_capped(&self, v: Vec3) -> Vec3 {
        let v = v.normalize_or_zero();
        let v = if v == Vec3::ZERO { Vec3::X } else { v };
        let cap = if self.tuning.max_pitch.is_finite() { self.tuning.max_pitch.clamp(0.0, std::f32::consts::FRAC_PI_2) } else { 0.0 };
        let pitch = v.z.clamp(-1.0, 1.0).asin();
        if pitch.abs() <= cap {
            return v;
        }
        let flat = Vec3::new(v.x, v.y, 0.0).normalize_or_zero();
        let flat = if flat == Vec3::ZERO { Vec3::X } else { flat };
        let out = flat * cap.cos() + Vec3::Z * cap.sin() * pitch.signum();
        out.normalize_or_zero()
    }

    /// The four side walls as the steering sees them: how much room is left to each (uu), which way it lies
    /// as a yaw, and the unit direction away from it. `yaw` is the heading's.
    fn side_walls(&self, yaw: f32) -> [(f32, f32, Vec3); 4] {
        let mut out = [(0.0f32, 0.0f32, Vec3::ZERO); 4];
        let mut n = 0;
        for (i, at) in [(0usize, 0.0f32), (1, std::f32::consts::FRAC_PI_2)] {
            for (side, limit) in [(1.0f32, self.path_lo[i]), (-1.0, self.path_hi[i])] {
                // Which way the wall lies, as a yaw: the low wall of an axis lies behind that axis.
                let at = if side > 0.0 { at + std::f32::consts::PI } else { at };
                let mut away = Vec3::ZERO;
                away[i] = side;
                out[n] = ((self.path[i] - limit) * side, short_angle(yaw - at), away);
                n += 1;
            }
        }
        out
    }

    /// **How much of a hurry the walls are in**, for a yaw turning the way `s` says, and the way out.
    ///
    /// For each wall the heading is closing on, the ground it will make good toward it before the nose comes
    /// across it is `R (1 - sin phi)` when the turn is away from that wall and `R (1 + sin phi)` when it is
    /// toward it, for a nose `phi` off the wall. Against the room left that is a *ratio*: under 1 and the
    /// turn fits, over 1 and the path is going to meet the wall. The worst ratio of the four is the hurry,
    /// and the lean out is the sum of the ways out weighted by their own ratios -- so a corner is left by its
    /// diagonal, which is the thing a wall-at-a-time rule cannot do: each of a corner's two walls says to
    /// turn into the other.
    ///
    /// Being inside `wall_hold` of a wall counts as a ratio of its own however the nose points, which is what
    /// keeps the path off the hard clamp -- and the clamp is the one thing that *would* put a corner in the
    /// path, since a clamped frame covers less than `speed * dt` and so leaves the turn limit behind.
    fn wall_hurry(&self, yaw: f32, s: f32) -> (f32, Vec3) {
        let r = self.turn_radius().min(1e6);
        let hold = self.wall_hold().max(1.0);
        let (mut worst, mut out) = (0.0f32, Vec3::ZERO);
        for (room, phi, away) in self.side_walls(yaw) {
            let room = room.max(1.0);
            let need = if phi.abs() < std::f32::consts::FRAC_PI_2 {
                // Turning away from this wall is `s` having the same sign as `phi`.
                if s * phi >= 0.0 { r * (1.0 - phi.sin().abs()) } else { r * (1.0 + phi.sin().abs()) }
            } else {
                0.0
            };
            let ratio = (need / room).max(hold / room);
            worst = worst.max(ratio);
            out += away * ratio;
        }
        (worst, out)
    }

    /// One step of the steering: where the path would like to go, and the heading turned that far toward it.
    /// `max` is the most the heading may move this step (rad).
    ///
    /// The aim is the waypoint, blended by wall urgency toward flying out of the arena wall, **in yaw and
    /// pitch separately** -- the yaw out of the four side walls (summed, so a corner is left by its diagonal)
    /// and the pitch out of the floor or the ceiling. The heading then turns toward the aim in yaw and pitch
    /// at a bounded rate, which is the aeroplane the Snitch is pretending to be.
    ///
    /// Two things here are not the obvious thing, and both were bugs first:
    ///
    /// * the turn is **not** a rotation about `heading x aim`. The shortest great circle between two headings
    ///   far apart in yaw and both near the pitch cap runs up over the pole, so nearly the whole of a step
    ///   went into a climb the cap then took straight back out, and the Snitch flew into the wall it was
    ///   turning away from with its nose swinging a degree a second.
    /// * while a wall is urgent the **way round is committed to** (`escape`) and held until the heading no
    ///   longer closes on it. Whichever way round is nearer changes hands when the Snitch points nearly
    ///   straight at a wall, and the aim then flipped from one side to the other every frame: the
    ///   rate-limited heading averaged the two and went nowhere. Either way round is equally safe (the
    ///   quarter turn costs `R` of ground going round either way), so committing costs nothing.
    fn steer(&mut self, max: f32) {
        /// How far short of straight-out-of-the-wall the aim stops (rad, 40 degrees): enough that the path
        /// leaves, little enough that it runs along the arena instead of back across the middle of it.
        const LEAN: f32 = 0.7;
        let cap = if self.tuning.max_pitch.is_finite() { self.tuning.max_pitch.clamp(0.0, std::f32::consts::FRAC_PI_2) } else { 0.0 };
        let (yaw, pitch) = (self.heading.y.atan2(self.heading.x), self.heading.z.clamp(-1.0, 1.0).asin());
        let to = (self.waypoint - self.path).normalize_or_zero();
        let to = self.pitch_capped(if to == Vec3::ZERO { self.heading } else { to });
        let (mut aim_yaw, mut aim_pitch) = (to.y.atan2(to.x), to.z.clamp(-1.0, 1.0).asin());

        // Which way round the yaw turns is the way that leaves the walls the most room -- the lower of the
        // two hurries -- with the way already being turned kept unless the other is clearly better. Both are
        // needed: deciding it off the worst single wall cannot turn out of a corner (each of its walls says
        // to turn into the other), and deciding it off the summed way out flips from frame to frame when the
        // Snitch points nearly straight at a wall, which a rate-limited heading averages into going nowhere.
        let (hurry_pos, out_pos) = self.wall_hurry(yaw, 1.0);
        let (hurry_neg, out_neg) = self.wall_hurry(yaw, -1.0);
        let keep = self.escape != 0.0 && {
            let (mine, other) = if self.escape > 0.0 { (hurry_pos, hurry_neg) } else { (hurry_neg, hurry_pos) };
            mine <= other * 1.3
        };
        if !keep {
            self.escape = if hurry_pos <= hurry_neg { 1.0 } else { -1.0 };
        }
        let (hurry, out) = if self.escape > 0.0 { (hurry_pos, out_pos) } else { (hurry_neg, out_neg) };
        // The hurry as a weight: nothing until the turn wants a fifth of the room there is, everything by the
        // time it wants nearly half, so the lean comes on early and gently and is never a jerk -- and is
        // fully on with better than twice the room the turn needs, which is the slack the ramp itself eats.
        let lean = ((hurry - 0.2) / 0.25).clamp(0.0, 1.0);
        match Vec3::new(out.x, out.y, 0.0).try_normalize().filter(|_| lean > 0.0) {
            Some(out) => {
                let flee = out.y.atan2(out.x);
                aim_yaw += short_angle((flee - self.escape * LEAN) - aim_yaw) * lean;
            }
            // Clear of the walls: the way round is up for grabs again.
            None => self.escape = 0.0,
        }

        // The floor and the ceiling have no yaw of their own, only a climb or a dive, and the climb limit is
        // as steep as any aim is allowed to be anyway. The same hurry: levelling off from a pitch `p` drops
        // `R (1 - cos p)` on the way, which inside `max_pitch` is a hundred uu or so -- it is `wall_hold`
        // that does the work here.
        let r = self.turn_radius().min(1e6);
        let hold = self.wall_hold().max(1.0);
        let mut vert = 0.0f32;
        for (side, limit) in [(1.0f32, self.path_lo[2]), (-1.0, self.path_hi[2])] {
            let room = ((self.path.z - limit) * side).max(1.0);
            let need = if pitch * side < 0.0 { r * (1.0 - pitch.cos()) } else { 0.0 };
            let ratio = (need / room).max(hold / room);
            if ratio > vert.abs() {
                vert = ratio * side;
            }
        }
        let lean_v = ((vert.abs() - 0.2) / 0.25).clamp(0.0, 1.0);
        aim_pitch += (cap * vert.signum() - aim_pitch) * lean_v;

        // And the turn. `escape` forces the way round; otherwise it is whichever is nearer, which at this
        // rate is a wide arc even for a U-turn and never a reversal.
        let mut dy = short_angle(aim_yaw - yaw);
        if self.escape != 0.0 && dy * self.escape < 0.0 {
            dy += self.escape * std::f32::consts::TAU;
        }
        let dp = aim_pitch.clamp(-cap, cap) - pitch;
        // What that pair of turns costs the heading in radians (yaw counts for less the steeper the climb),
        // and the two scaled together so the heading never moves more than `max` in a step.
        let cost = ((dy * pitch.cos()).powi(2) + dp.powi(2)).sqrt();
        let k = if cost > max.max(0.0) && cost > 1e-9 { max.max(0.0) / cost } else { 1.0 };
        let (yaw, pitch) = (yaw + dy * k, (pitch + dp * k).clamp(-cap, cap));
        let h = Vec3::new(pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), pitch.sin());
        self.heading = h.try_normalize().unwrap_or(Vec3::X);
    }

    /// This frame's visible position: the path point with the loop offset added, in the plane across the
    /// direction of travel, so the Snitch corkscrews along its path instead of sliding down it.
    fn circling(&self) -> Vec3 {
        let r = self.tuning.circle_radius;
        if !(r > 0.0) {
            return self.path;
        }
        // A travel direction, and a pair of unit vectors across it. Up is the axis to cross with unless the
        // travel is itself nearly vertical, in which case X is. The axis of the corkscrew is the **smoothed**
        // heading, so the loop follows the arc the path is flying rather than the line to the waypoint.
        let dir = if self.heading.length_squared() > 1e-6 { self.heading.normalize() } else { Vec3::X };
        let away = if dir.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
        let u = dir.cross(away);
        let u = if u.length_squared() > 1e-6 { u.normalize() } else { Vec3::Y };
        let v = dir.cross(u);
        let p = self.path + (u * self.angle.cos() + v * self.angle.sin()) * r;
        if p.is_finite() {
            p.clamp(self.lo, self.hi)
        } else {
            self.path
        }
    }

    pub fn step(&mut self, harry: &BroomState, wants_catch: bool, dt: f32) -> Vec<SnitchEvent> {
        let dt = if dt.is_finite() { dt.clamp(0.0, MAX_DT) } else { 0.0 };
        let mut events = Vec::new();
        if self.phase == Phase::Done {
            return events;
        }
        let dist = (harry.pos - self.pos).length();
        let dist = if dist.is_finite() { dist } else { f32::MAX };
        let t = self.tuning;

        match self.phase {
            Phase::Chasing => self.update_progress(dist, dt, &mut events),
            Phase::Catching { elapsed } => {
                if wants_catch && dist <= t.catch_radius {
                    self.phase = Phase::Done;
                    events.push(SnitchEvent::Caught);
                } else if elapsed + dt > t.max_catch_time {
                    self.progress = 0.0;
                    self.tries += 1;
                    events.push(SnitchEvent::Missed);
                    if self.tries >= t.max_tries {
                        self.phase = Phase::Done;
                        events.push(SnitchEvent::OutOfTries);
                    } else {
                        self.phase = Phase::Chasing;
                    }
                } else {
                    self.phase = Phase::Catching { elapsed: elapsed + dt };
                }
            }
            Phase::Done => {}
        }

        self.fly(dist, dt);
        events
    }

    fn update_progress(&mut self, dist: f32, dt: f32, events: &mut Vec<SnitchEvent>) {
        let t = self.tuning;
        if dist >= t.max_loss_radius {
            if self.progress > 0.0 {
                self.progress = 0.0;
                events.push(SnitchEvent::Lost);
            }
            return;
        }
        let before = self.progress;
        if dist < t.neutral_radius {
            let span = (t.neutral_radius - t.max_gain_radius).max(f32::EPSILON);
            let k = ((t.neutral_radius - dist) / span).clamp(0.0, 1.0);
            self.progress += t.gain_rate * k * dt;
        } else if dist > t.neutral_radius {
            let span = (t.max_loss_radius - t.neutral_radius).max(f32::EPSILON);
            let k = ((dist - t.neutral_radius) / span).clamp(0.0, 1.0);
            self.progress -= t.loss_rate * k * dt;
        }
        self.progress = self.progress.clamp(0.0, 1.0);
        if before <= 0.0 && self.progress > 0.0 {
            events.push(SnitchEvent::ClosingIn);
        }
        // A full bar opens the catch window -- but only with the rings in (`rings_ready`). Without them the
        // bar simply sits full and nothing is lost; the HUD's ring counter says what is still wanted.
        if self.progress >= 1.0 && self.rings_ready {
            self.phase = Phase::Catching { elapsed: 0.0 };
            events.push(SnitchEvent::CatchStart);
            self.start_dodge();
        }
    }

    /// The Snitch sees the catch coming: it jinks to one side (which one is its own coin toss) and bursts away.
    fn start_dodge(&mut self) {
        let t = self.tuning;
        if !(t.dodge_time > 0.0) {
            return;
        }
        let side = if self.rng.unit() < 0.5 { -1.0 } else { 1.0 };
        let across = self.heading.cross(Vec3::Z).normalize_or_zero();
        let across = if across == Vec3::ZERO { Vec3::Y } else { across };
        let (lo, hi) = self.aim_box();
        self.waypoint = (self.path + across * side * 1800.0 + self.heading * 900.0).clamp(lo, hi);
        self.dodge = t.dodge_time;
    }

    /// Whether the Snitch is in the middle of its dodge (the ring trail drops no rings meanwhile: the chain
    /// stays one smooth road).
    pub fn dodging(&self) -> bool {
        self.dodge > 0.0
    }

    /// One step of the flight path: **steer, then move**. The heading turns toward where the steering wants to
    /// go by at most `curvature * ds` radians, and the path then walks `ds = speed * dt` along it, so the path
    /// is a run of arcs no tighter than `turn_radius` and never has a corner in it. The speed rules are
    /// untouched (idle, up to `SPEED_NEAR` with Harry on it) and so is the circling: the loop is an offset
    /// around the path, not extra ground covered.
    fn fly(&mut self, dist: f32, dt: f32) {
        let near = (1.0 - dist / self.tuning.max_loss_radius.max(1.0)).clamp(0.0, 1.0);
        let speed = SPEED_IDLE + (SPEED_NEAR - SPEED_IDLE) * near;
        // The dodge: a burst, and a tighter turn (the waypoint was already set sideways when it began).
        let (speed, turn_gain) = if self.dodge > 0.0 {
            self.dodge = (self.dodge - dt).max(0.0);
            (speed.max(self.tuning.dodge_speed), self.tuning.dodge_turn.max(1.0))
        } else {
            (speed, 1.0)
        };
        let ds = speed * dt;
        // Reached, or gone past: a waypoint is given up on the moment it falls behind, which is what stops a
        // Snitch whose turn circle is wider than `path_capture` circling it for ever.
        let to = self.waypoint - self.path;
        let capture = if self.tuning.path_capture.is_finite() { self.tuning.path_capture.max(0.0) } else { 0.0 };
        if to.length() <= capture.max(ds) || to.dot(self.heading) <= 0.0 {
            self.waypoint = self.pick_waypoint();
        }
        // The turn a step may make: the curvature over the ground covered, and never more than 0.3 rad even
        // so. The cap only bites at a `dt` of over half a second, where a frame covers more than a ring's
        // spacing on its own and the chain could not be smooth whatever the path did.
        self.steer((self.curvature() * turn_gain * ds).min(0.3));
        self.path += self.heading * ds;
        if !self.path.is_finite() {
            self.path = (self.path_lo + self.path_hi) * 0.5;
        }
        // The wall look-ahead is what really keeps the path inside; this is the backstop that makes it true
        // whatever the tuning (a Snitch told not to turn at all would otherwise fly out of the arena).
        self.path = self.path.clamp(self.path_lo, self.path_hi);
        let rate = if self.tuning.circle_rate.is_finite() { self.tuning.circle_rate } else { 0.0 };
        self.angle = (self.angle + rate * dt).rem_euclid(std::f32::consts::TAU);
        self.pos = self.circling();
    }

    pub fn pos(&self) -> Vec3 {
        self.pos
    }

    /// Where the Snitch would be with no circling: the point on its flight path that `pos` loops around.
    /// Nothing in the game uses it -- it is here for the `HP3_QUID_DEBUG` trace, which prints the loop offset
    /// so the helix can be read off a headless run.
    pub fn path_pos(&self) -> Vec3 {
        self.path
    }

    /// Where the Snitch is heading along its path, as a unit vector: the **tangent of the smoothed path**, not
    /// the line to the next waypoint. It is the normal of the rings its trail drops (`quidditch::trail`), so
    /// each ring faces along the curve and stands a bounded angle from the one before it; and it is the path's
    /// direction, not the loop's, so a ring faces the chase rather than the corkscrew.
    pub fn travel_dir(&self) -> Vec3 {
        self.heading.normalize_or_zero()
    }

    pub fn progress(&self) -> f32 {
        self.progress
    }

    /// Adds `amount` to the catch bar outright, for something Harry did other than closing in -- today, a
    /// flight ring of the Snitch's trail flown through (`quidditch::trail`). Capped at 1.0 like the proximity
    /// gain, and only while the chase is on: a bonus during the catch window (or after the match) would move
    /// a bar that is already full and is not what decides a catch. Rubbish in does nothing.
    pub fn bonus_progress(&mut self, amount: f32) {
        if self.phase != Phase::Chasing || !amount.is_finite() || amount <= 0.0 {
            return;
        }
        self.progress = (self.progress + amount).clamp(0.0, 1.0);
    }

    /// Catch attempts still to come, for the HUD's pips.
    pub fn tries_left(&self) -> u32 {
        self.tuning.max_tries.saturating_sub(self.tries)
    }

    /// True while the catch window is open (the bar is full and Harry has `max_catch_time` to grab it).
    pub fn in_catch_window(&self) -> bool {
        matches!(self.phase, Phase::Catching { .. })
    }

    /// Seconds the catch window has left (0 when it is not open): for the HUD's prompt.
    pub fn catch_left(&self) -> f32 {
        match self.phase {
            Phase::Catching { elapsed } => (self.tuning.max_catch_time - elapsed).max(0.0),
            _ => 0.0,
        }
    }

    /// Says whether Harry has the run of flight rings the catch needs (`quidditch::trail`'s
    /// `TrailTuning::catch_rings`). While this is false a full bar waits instead of opening the catch window.
    pub fn set_rings_ready(&mut self, ready: bool) {
        self.rings_ready = ready;
    }

    /// Whether the rings let the catch happen at all (for the HUD and the logs).
    pub fn rings_ready(&self) -> bool {
        self.rings_ready
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arena() -> ArenaBounds {
        ArenaBounds {
            center: Vec3::new(0.0, 0.0, 2000.0),
            half: Vec3::new(5000.0, 5000.0, 2000.0),
            ceiling: 3500.0,
        }
    }

    fn harry_at(pos: Vec3) -> BroomState {
        BroomState::new(pos, Vec3::ZERO, 0.0, 0.0)
    }

    /// Harry glued to the snitch, offset along +x by `d`.
    fn near(s: &Snitch, d: f32) -> BroomState {
        harry_at(s.pos() + Vec3::new(d, 0.0, 0.0))
    }

    /// Step with Harry at the snitch until `CatchStart` appears.
    fn raise_to_catch(s: &mut Snitch) {
        for _ in 0..1000 {
            let h = near(s, 0.0);
            if s.step(&h, false, 0.25).contains(&SnitchEvent::CatchStart) {
                return;
            }
        }
        panic!("never reached CatchStart");
    }

    fn raise_some(s: &mut Snitch) {
        let h = near(s, 100.0);
        s.step(&h, false, 0.5);
        let h = near(s, 100.0);
        s.step(&h, false, 0.5);
        assert!(s.progress() > 0.0 && s.progress() < 1.0);
    }

    #[test]
    fn progress_rises_when_close() {
        let mut s = Snitch::new(1, &arena());
        let h = near(&s, 100.0);
        let ev = s.step(&h, false, 0.1);
        assert!(s.progress() > 0.0);
        assert!(ev.contains(&SnitchEvent::ClosingIn));
    }

    #[test]
    fn progress_falls_when_far() {
        let mut s = Snitch::new(1, &arena());
        raise_some(&mut s);
        let before = s.progress();
        let far = SnitchTuning::default().neutral_radius + 1000.0;
        let h = near(&s, far);
        s.step(&h, false, 0.1);
        assert!(s.progress() < before);
        assert!(s.progress() > 0.0);
    }

    #[test]
    fn progress_holds_in_neutral_band() {
        let mut s = Snitch::new(1, &arena());
        raise_some(&mut s);
        let before = s.progress();
        let h = near(&s, SnitchTuning::default().neutral_radius);
        s.step(&h, false, 0.1);
        assert!((s.progress() - before).abs() < 1e-3, "{} vs {}", s.progress(), before);
    }

    #[test]
    fn losing_snitch_zeroes_progress() {
        let mut s = Snitch::new(1, &arena());
        raise_some(&mut s);
        let h = near(&s, SnitchTuning::default().max_loss_radius + 500.0);
        let ev = s.step(&h, false, 0.1);
        assert!(ev.contains(&SnitchEvent::Lost));
        assert_eq!(s.progress(), 0.0);
    }

    #[test]
    fn catch_succeeds_inside_catch_radius_when_wants_catch() {
        let mut s = Snitch::new(2, &arena());
        raise_to_catch(&mut s);
        let h = near(&s, 0.0);
        let ev = s.step(&h, true, 0.01);
        assert!(ev.contains(&SnitchEvent::Caught));
    }

    #[test]
    fn catch_fails_after_max_catch_time() {
        let mut s = Snitch::new(2, &arena());
        raise_to_catch(&mut s);
        let mut missed = false;
        for _ in 0..100 {
            let h = near(&s, SnitchTuning::default().catch_radius + 400.0);
            if s.step(&h, true, 0.25).contains(&SnitchEvent::Missed) {
                missed = true;
                break;
            }
        }
        assert!(missed);
        assert_eq!(s.progress(), 0.0);
    }

    #[test]
    fn out_of_tries_after_max_tries() {
        let tuning = SnitchTuning::default();
        let mut s = Snitch::new(3, &arena());
        let mut misses = 0;
        'outer: for _ in 0..tuning.max_tries + 2 {
            raise_to_catch(&mut s);
            for _ in 0..100 {
                let h = near(&s, tuning.catch_radius + 400.0);
                let ev = s.step(&h, true, 0.25);
                if ev.contains(&SnitchEvent::Missed) {
                    misses += 1;
                }
                if ev.contains(&SnitchEvent::OutOfTries) {
                    break 'outer;
                }
                if ev.contains(&SnitchEvent::Missed) {
                    break;
                }
            }
        }
        assert_eq!(misses, tuning.max_tries);
        assert!(s.step(&near(&s, 0.0), true, 0.25).is_empty());
    }

    #[test]
    fn snitch_stays_inside_bounds_for_10000_steps() {
        let a = arena();
        let mut s = Snitch::new(42, &a);
        let zmax = a.ceiling.min(a.center.z + a.half.z);
        for i in 0..10000 {
            let t = i as f32 * 0.05;
            let h = harry_at(Vec3::new(t.cos() * 3000.0, t.sin() * 3000.0, 2000.0));
            s.step(&h, i % 7 == 0, 1.0 / 60.0);
            let p = s.pos();
            assert!(p.is_finite());
            assert!((p.x - a.center.x).abs() <= a.half.x);
            assert!((p.y - a.center.y).abs() <= a.half.y);
            assert!(p.z >= a.center.z - a.half.z && p.z <= zmax);
        }
    }

    #[test]
    fn same_seed_same_path() {
        let a = arena();
        let mut s1 = Snitch::new(7, &a);
        let mut s2 = Snitch::new(7, &a);
        let mut s3 = Snitch::new(8, &a);
        let h = harry_at(Vec3::new(500.0, 0.0, 1500.0));
        for _ in 0..500 {
            s1.step(&h, false, 1.0 / 60.0);
            s2.step(&h, false, 1.0 / 60.0);
            s3.step(&h, false, 1.0 / 60.0);
            assert_eq!(s1.pos(), s2.pos());
        }
        assert_ne!(s1.pos(), s3.pos());
    }

    /// The Snitch really loops: its visible position sits a full `circle_radius` off the path it is flying,
    /// in the plane across the travel, and it goes round at `circle_rate`.
    #[test]
    fn circling_keeps_its_radius_around_the_path() {
        let t = SnitchTuning::default();
        let mut s = Snitch::new(11, &arena());
        let h = harry_at(Vec3::new(4000.0, 4000.0, 1000.0));
        let mut seen = [false; 4];
        let mut last = s.angle;
        for _ in 0..600 {
            s.step(&h, false, 1.0 / 60.0);
            let off = s.pos() - s.path;
            assert!((off.length() - t.circle_radius).abs() < 1e-2, "offset {} is not the radius", off.length());
            // Across the travel, so the loop shows from the side rather than being flown through. The travel
            // is the smoothed heading, which is what the corkscrew's axis follows.
            let dir = s.heading.normalize();
            assert!(off.normalize().dot(dir).abs() < 1e-2, "the loop is not across the travel");
            // It turns at the tuned rate, and goes right round.
            let step = (s.angle - last).rem_euclid(std::f32::consts::TAU);
            assert!((step - t.circle_rate / 60.0).abs() < 1e-3, "turned {step} in a frame");
            last = s.angle;
            seen[(s.angle / std::f32::consts::FRAC_PI_2) as usize % 4] = true;
        }
        assert!(seen.iter().all(|&q| q), "the loop never went all the way round: {seen:?}");
    }

    /// The loop is an offset, not a detour: the path under it still covers exactly `speed * dt` a frame, and
    /// a Snitch tuned with no circling flies that bare path.
    #[test]
    fn circling_leaves_the_path_and_its_speed_alone() {
        let a = arena();
        let dt = 1.0 / 60.0;
        // Harry out past `max_loss_radius`, so the speed is the idle one and known.
        let h = harry_at(Vec3::new(50_000.0, 0.0, 2000.0));
        let mut s = Snitch::new(13, &a);
        let mut steps = 0;
        for _ in 0..600 {
            let was = s.path;
            s.step(&h, false, dt);
            let moved = s.path.distance(was);
            // Either a full step along the path, or the frame it reached a waypoint and turned.
            if (moved - SPEED_IDLE * dt).abs() < 1e-2 {
                steps += 1;
            } else {
                assert!(moved <= SPEED_IDLE * dt + 1e-2, "the loop sped the path up: {moved}");
            }
        }
        assert!(steps > 580, "only {steps} of 600 frames flew a clean path step");
        let bare = SnitchTuning { circle_radius: 0.0, ..SnitchTuning::default() };
        let mut straight = Snitch::with_tuning(13, &a, bare);
        for _ in 0..600 {
            straight.step(&h, false, dt);
        }
        assert_eq!(straight.pos(), straight.path, "a Snitch with no circling flies the bare path");
        // Same seed, same loop: the whole thing is deterministic, offset and all.
        let mut again = Snitch::new(13, &a);
        for _ in 0..600 {
            again.step(&h, false, dt);
        }
        assert_eq!(s.pos(), again.pos());
    }

    /// Circling never puts the Snitch outside the arena, however long it flies: the path is flown inside the
    /// arena brought in by the radius, so the loop around it still fits.
    #[test]
    fn circling_snitch_stays_inside_bounds_for_10000_steps() {
        let a = arena();
        let mut s = Snitch::new(21, &a);
        let zmax = a.ceiling.min(a.center.z + a.half.z);
        let mut off_path = 0.0f32;
        for i in 0..10000 {
            let t = i as f32 * 0.05;
            let h = harry_at(Vec3::new(t.cos() * 4800.0, t.sin() * 4800.0, 3400.0));
            s.step(&h, i % 7 == 0, 1.0 / 60.0);
            let p = s.pos();
            assert!(p.is_finite());
            assert!((p.x - a.center.x).abs() <= a.half.x, "{p:?}");
            assert!((p.y - a.center.y).abs() <= a.half.y, "{p:?}");
            assert!(p.z >= a.center.z - a.half.z && p.z <= zmax, "{p:?}");
            off_path = off_path.max(p.distance(s.path));
        }
        assert!((off_path - SnitchTuning::default().circle_radius).abs() < 1e-2, "the loop was cut short: {off_path}");
    }

    /// A circling Snitch is still catchable: the catch radius covers the loop, so flying onto the path point
    /// and reaching still gets it.
    #[test]
    fn a_circling_snitch_is_still_caught_from_its_path() {
        let mut s = Snitch::new(4, &arena());
        for _ in 0..1000 {
            let h = harry_at(s.path);
            if s.step(&h, false, 0.25).contains(&SnitchEvent::CatchStart) {
                break;
            }
        }
        assert!(s.in_catch_window(), "the bar never filled with Harry on the path");
        let h = harry_at(s.path);
        assert!(s.step(&h, true, 0.01).contains(&SnitchEvent::Caught), "a looping Snitch could not be caught");
    }

    /// The ring trail's bonus: it fills the bar and never overfills it, it is ignored once the catch window is
    /// open or the match is over, and the catch still works exactly as it did (a full bar opens the window on
    /// the next step near the Snitch, and the catch key then takes it).
    #[test]
    fn a_ring_bonus_fills_the_bar_without_breaking_the_catch() {
        let mut s = Snitch::new(6, &arena());
        s.bonus_progress(0.3);
        assert!((s.progress() - 0.3).abs() < 1e-6, "{}", s.progress());
        // Rubbish and nothing are both nothing.
        for bad in [f32::NAN, f32::INFINITY, 0.0, -1.0] {
            s.bonus_progress(bad);
            assert!((s.progress() - 0.3).abs() < 1e-6, "{bad} moved the bar to {}", s.progress());
        }
        // Never past the top of the bar.
        s.bonus_progress(5.0);
        assert_eq!(s.progress(), 1.0);
        assert!(!s.in_catch_window(), "the bonus must not open the catch window by itself");
        // A full bar opens the window next step, and the catch is unchanged.
        let h = near(&s, 0.0);
        assert!(s.step(&h, false, 1.0 / 60.0).contains(&SnitchEvent::CatchStart));
        // The window is no place for a bonus.
        s.bonus_progress(1.0);
        assert_eq!(s.progress(), 1.0);
        let h = near(&s, 0.0);
        assert!(s.step(&h, true, 0.01).contains(&SnitchEvent::Caught));
        // And neither is a finished match.
        s.bonus_progress(1.0);
        assert_eq!(s.progress(), 1.0);
    }

    /// The owner's rule: without the run of flight rings the catch window never opens, however full the bar
    /// is, and nothing is lost by the wait -- the moment the rings are in, it opens.
    #[test]
    fn without_the_rings_the_catch_window_stays_shut() {
        let mut s = Snitch::new(12, &arena());
        s.set_rings_ready(false);
        for _ in 0..400 {
            let h = near(&s, 0.0);
            let ev = s.step(&h, true, 0.25);
            assert!(!ev.contains(&SnitchEvent::CatchStart), "the window opened with no rings");
            assert!(!ev.contains(&SnitchEvent::Caught), "the Snitch was caught with no rings");
        }
        // The bar is full and waiting, and no try has been spent.
        assert_eq!(s.progress(), 1.0);
        assert_eq!(s.tries_left(), SnitchTuning::default().max_tries);
        assert!(!s.in_catch_window());
        // The rings come in: the window opens on the next step and the catch is the usual one.
        s.set_rings_ready(true);
        let h = near(&s, 0.0);
        assert!(s.step(&h, false, 1.0 / 60.0).contains(&SnitchEvent::CatchStart));
        let h = near(&s, 0.0);
        assert!(s.step(&h, true, 0.01).contains(&SnitchEvent::Caught));
    }

    /// The travel direction the ring trail takes its normals from: a unit vector down the path, towards the
    /// waypoint the Snitch is flying at.
    #[test]
    fn travel_dir_points_down_the_path() {
        let mut s = Snitch::new(8, &arena());
        let h = harry_at(Vec3::new(40_000.0, 0.0, 2000.0));
        let mut agreed = 0;
        for _ in 0..300 {
            let (was, d) = (s.path_pos(), s.travel_dir());
            assert!((d.length() - 1.0).abs() < 1e-3, "{d:?} is not a unit vector");
            s.step(&h, false, 1.0 / 60.0);
            // The step really went that way, bar the frame the path turned at its waypoint.
            let moved = s.path_pos() - was;
            if moved.length() > 1.0 {
                agreed += u32::from(moved.normalize().dot(d) > 0.999);
            }
        }
        assert!(agreed > 290, "the travel direction matched the step in only {agreed} of 300 frames");
    }

    /// The smoothed path keeps itself inside the flight box, with room for the loop *and* the flight rings
    /// dropped on it, and clear of the floor -- by **steering**, never by the clamp. The clamp matters: a
    /// clamped frame covers less ground than `speed * dt`, so the rings dropped by path length come closer
    /// together in space and further apart in heading, and the chain kinks. Fifty seeds, 20 000 steps each.
    #[test]
    fn the_path_stays_off_the_walls_by_steering_for_every_seed() {
        let a = arena();
        let margin = SnitchTuning::default().circle_radius + SnitchTuning::default().path_margin;
        let mut nearest = f32::MAX;
        for seed in 1..=50u64 {
            let mut s = Snitch::new(seed, &a);
            for i in 0..20_000 {
                let th = i as f32 * 0.013;
                let harry = Vec3::new(th.cos() * 3000.0, th.sin() * 3500.0, 1500.0 + (th * 0.7).sin() * 1200.0);
                s.step(&harry_at(harry), i % 11 == 0, 1.0 / 60.0);
                let p = s.path_pos();
                assert!(p.is_finite(), "seed {seed} step {i}: {p:?}");
                for k in 0..3 {
                    let room = (p[k] - s.path_lo[k]).min(s.path_hi[k] - p[k]);
                    assert!(room > 0.5, "seed {seed} step {i}: the path rode the clamp on axis {k} at {p:?}");
                    nearest = nearest.min(room);
                }
                // The ring dropped here has its whole rim inside the arena, floor included.
                assert!(p.z - margin >= a.center.z - a.half.z, "seed {seed} step {i}: {p:?} is too low");
                assert!(p.z + margin <= a.ceiling.min(a.center.z + a.half.z), "seed {seed} step {i}: {p:?} is too high");
            }
        }
        println!("the path came within {nearest:.0} uu of the flight box over 50 seeds");
    }

    /// A waypoint put **directly behind** the Snitch is a U-turn, and a U-turn here is a wide arc: the path
    /// never reverses, every step turns by no more than the curvature allows, and the turn it flies is no
    /// tighter than `turn_radius`. (The waypoint chooser prefers points in front for exactly this reason, so
    /// this is the case it is not allowed to pick -- forced on it here.)
    #[test]
    fn a_waypoint_behind_makes_a_wide_arc_and_never_a_reversal() {
        let a = arena();
        let dt = 1.0 / 60.0;
        let mut s = Snitch::new(17, &a);
        // Out in the middle, flying +x, with the waypoint hard behind: no wall has anything to say about it.
        s.path = a.center;
        s.heading = Vec3::X;
        s.waypoint = a.center - Vec3::X * 4000.0;
        let r = s.turn_radius();
        // Harry well out of it, so the speed is the idle one and the arc is the tightest the path can fly.
        let h = harry_at(Vec3::new(50_000.0, 0.0, 2000.0));
        let (mut turned, mut steps) = (0.0f32, 0);
        let mut was = s.heading;
        while turned < std::f32::consts::PI * 0.95 && steps < 20_000 {
            let before = s.path_pos();
            // The waypoint is kept behind the *current* heading, so the U-turn is never let off the hook.
            s.waypoint = s.path_pos() - s.travel_dir() * 4000.0;
            s.step(&h, false, dt);
            let moved = s.path_pos() - before;
            let step_turn = was.dot(s.heading).clamp(-1.0, 1.0).acos();
            // Never a reversal: a frame's step still goes roughly the way the path was already going.
            assert!(moved.normalize().dot(was) > 0.99, "step {steps} reversed: {moved:?} against {was:?}");
            // And never tighter than the turn radius: the turn a frame makes against the ground it covers.
            let radius = moved.length() / step_turn.max(1e-9);
            assert!(radius >= r * 0.99, "step {steps}: flew a radius of {radius:.0}, tighter than {r:.0}");
            turned += step_turn;
            was = s.heading;
            steps += 1;
        }
        assert!(turned >= std::f32::consts::PI * 0.95, "the U-turn never came round: {turned} rad in {steps} steps");
        // A half turn at that radius takes a half circumference of flying, give or take the pitch.
        let flown = steps as f32 * SPEED_IDLE * dt;
        assert!(flown > r * 2.8, "the U-turn was flown in {flown:.0} uu, less than a half circle of {r:.0}");
    }

    #[test]
    fn odd_dt_never_panics_or_nans() {
        let mut s = Snitch::new(5, &arena());
        let h = harry_at(Vec3::new(10.0, 0.0, 1500.0));
        for dt in [0.0, 1e-9, 1e-3, 10.0, 1e9, f32::NAN, f32::INFINITY, -1.0] {
            s.step(&h, true, dt);
            assert!(s.pos().is_finite());
            assert!(s.progress().is_finite());
        }
    }
}
