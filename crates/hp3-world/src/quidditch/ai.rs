//! The team AI: everyone on the pitch who is not Harry, and the Quaffle play between them. Pure: nothing here
//! touches the script VM (`quidditch::team` is what puts these agents on actors).
//!
//! Harry is the home Seeker and is flown by the player, so he is not an agent: his side fields six agents (3
//! Chasers, 2 Beaters, 1 Keeper) and the opponent seven (the same plus a Seeker). Each agent is a `BroomState`
//! flown by `flight::step` -- the same flight model the player gets -- steered by a small state machine per
//! role. A broom can never stop, so every "hold a position" behaviour is a circle flown around it.
//!
//! **Every number here is a placeholder, tuned by feel like the rest of the match.**

use glam::Vec3;

use super::arena::{self, ArenaBounds};
use super::flight::{self, BroomInput, BroomState, BroomTuning};
use super::goals::Hoop;
use super::lod::Budget;
use super::quaffle::{self, PlayerId, Quaffle};
use super::rng::Rng;

/// Harry's own id. He is the player, never an agent, but the AI has to be able to name him: the Bludgers fly
/// at him, so his own Beaters defend him.
pub const HARRY: PlayerId = PlayerId(0);

/// How near a Chaser has to be to a free Quaffle to take it (uu).
pub const CATCH_RADIUS: f32 = 250.0;
/// Nearest and furthest a shot at a hoop is taken from (uu).
pub const SHOOT_MIN: f32 = 250.0;
pub const SHOOT_RANGE: f32 = 2600.0;
/// How far in front of the hoop's disc the shooter must be (uu), and how straight on: the cosine of the angle
/// between the way the ball is thrown and the hoop's own axis, so a ball thrown from the side is not tried.
const SHOOT_FRONT: f32 = 300.0;
const SHOOT_CONE: f32 = 0.6;
/// Where a Chaser carrying the ball aims while the goal is still too far to shoot at (uu in front of it).
const APPROACH: f32 = 1600.0;
/// An opponent inside this (uu) marks a team-mate, who is then not passed to.
pub const OPEN_RADIUS: f32 = 900.0;
/// A pass is only worth it to someone this much nearer the goal (uu), and only over this distance (uu).
const PASS_GAIN: f32 = 600.0;
const PASS_MIN: f32 = 400.0;
const PASS_RANGE: f32 = 4500.0;
/// How far from his own goal's hoops a Keeper flies (uu), and the band he is never outside.
pub const KEEPER_STANDOFF: f32 = 700.0;
pub const KEEPER_BAND: f32 = 1800.0;
/// A Quaffle this near his goal (uu) has the Keeper blocking.
const BLOCK_RANGE: f32 = 3500.0;
/// How near a Beater has to be to a Bludger to swat it (uu), how far off a Bludger's line a team-mate still
/// counts as in its way (uu), and how far down that line it is worth looking (uu).
pub const BEAT_RADIUS: f32 = 400.0;
const THREAT_RADIUS: f32 = 700.0;
const THREAT_LENGTH: f32 = 5000.0;
/// How fast a beaten Bludger leaves the bat (uu/s).
pub const BEAT_SPEED: f32 = 1100.0;
/// How long the opposing Seeker leaves the snitch alone after the kickoff (s).
pub const SEEKER_DELAY: f32 = 8.0;
/// How far an agent's look may be swung in one frame (rad): the broom's own rate caps fly the rest of the turn.
const LOOK_STEP: f32 = 0.25;
/// An opponent this near a hoop (uu) counts as defending it, which is the hoop a shot avoids.
const DEFENDED: f32 = 900.0;
/// How far in front of a Chaser the ball he carries rides (uu).
const CARRY_AHEAD: f32 = 60.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Chaser,
    Beater,
    Keeper,
    Seeker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Team {
    Home,
    Away,
}

impl Team {
    pub fn other(self) -> Team {
        match self {
            Team::Home => Team::Away,
            Team::Away => Team::Home,
        }
    }

    /// True for Harry's side (Gryffindor), which `goals::Hoop::home` is about.
    pub fn is_home(self) -> bool {
        self == Team::Home
    }
}

/// One AI flyer: who he is, which side he is on, what he does and where his broom is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Agent {
    pub id: PlayerId,
    pub team: Team,
    pub role: Role,
    pub body: BroomState,
    /// Seconds he has been playing. Only the opposing Seeker's delay reads it.
    pub age: f32,
}

/// What one agent looks like to the others: enough to pass to him, mark him or defend him, and no borrow of
/// the agent list itself (`step_agent` holds one agent mutably while it reads all of them).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AgentView {
    pub id: PlayerId,
    pub team: Team,
    pub role: Role,
    pub pos: Vec3,
}

/// The snapshot of a frame an agent decides from.
#[derive(Debug, Clone, Copy)]
pub struct AiWorld<'a> {
    pub bounds: ArenaBounds,
    /// The six scoring hoops (`arena::hoops`).
    pub hoops: &'a [Hoop],
    /// Where the Quaffle is, and who has it (`None` while it is loose).
    pub quaffle: Vec3,
    pub holder: Option<PlayerId>,
    /// Every Bludger in flight: where it is and where it is going.
    pub bludgers: &'a [(Vec3, Vec3)],
    /// Harry, the home Seeker: the player, so not an agent.
    pub harry: Vec3,
    pub snitch: Vec3,
    /// Everyone the AI flies, as they were at the top of the frame.
    pub players: &'a [AgentView],
}

/// What an agent asks the match to do this frame. The agent flies itself; these are the things only the match
/// can do (the Quaffle is one ball, and the Bludgers are not the agents').
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AiAction {
    Pass(PlayerId),
    Shoot,
    /// Swat the Bludger he is on at `dir`.
    HitBludger(Vec3),
    Block,
    ChaseSnitch,
}

/// What a role wants of this frame: where to fly and what to ask for.
struct Plan {
    target: Vec3,
    actions: Vec<AiAction>,
    boost: bool,
    brake: bool,
}

impl Plan {
    fn fly_to(target: Vec3, boost: bool) -> Plan {
        Plan { target, actions: Vec::new(), boost, brake: false }
    }
}

/// Whether a goal through `h` is `team`'s own ten points.
fn is_attack(h: &Hoop, team: Team) -> bool {
    h.home == team.is_home()
}

/// The middle of the goal `team` defends: the average of its three hoops.
pub fn goal_center(hoops: &[Hoop], team: Team) -> Vec3 {
    let mut sum = Vec3::ZERO;
    let mut n = 0.0;
    for h in hoops.iter().filter(|h| !is_attack(h, team) && h.center.is_finite()) {
        sum += h.center;
        n += 1.0;
    }
    if n == 0.0 {
        return Vec3::ZERO;
    }
    sum / n
}

/// Which way the goal `team` defends looks: into the pitch, the side a shot at it comes from.
fn own_normal(hoops: &[Hoop], team: Team) -> Vec3 {
    hoops
        .iter()
        .find(|h| !is_attack(h, team))
        .and_then(|h| h.normal.try_normalize())
        .unwrap_or(Vec3::Y)
}

/// +1 for the side that attacks towards +y (Gryffindor, the north goal), -1 for the other.
fn attack_sign(team: Team) -> f32 {
    if team.is_home() { 1.0 } else { -1.0 }
}

/// Which of the three lanes across the pitch an agent keeps to: -1, 0 or +1 from his id.
fn lane(id: PlayerId) -> f32 {
    (id.0 % 3) as f32 - 1.0
}

/// Whether a shot from `from` at `h` is on: in front of the disc, inside range, and straight enough on that
/// the ball would fit through the ring.
pub fn shot_is_on(from: Vec3, h: &Hoop) -> bool {
    let to = h.center - from;
    if !from.is_finite() || !to.is_finite() {
        return false;
    }
    let d = to.length();
    if !(d > SHOOT_MIN) || d > SHOOT_RANGE {
        return false;
    }
    let Some(n) = h.normal.try_normalize() else { return false };
    (from - h.center).dot(n) > SHOOT_FRONT && (to / d).dot(-n) >= SHOOT_CONE
}

/// Where an agent waits when there is nothing better to do, which is also where he starts: Chasers up the
/// pitch they attack, Beaters in the middle where the Bludgers come from, the Keeper in front of his posts and
/// the Seeker high over his own half.
pub fn zone(team: Team, role: Role, id: PlayerId, b: &ArenaBounds, hoops: &[Hoop]) -> Vec3 {
    let sign = attack_sign(team);
    let x = b.center.x + lane(id) * b.half.x * 0.5;
    let mid = b.center.z;
    let p = match role {
        Role::Chaser => Vec3::new(x, sign * b.half.y * 0.35, mid),
        Role::Beater => Vec3::new(x, sign * b.half.y * 0.1, mid + 300.0),
        Role::Keeper => goal_center(hoops, team) + own_normal(hoops, team) * KEEPER_STANDOFF,
        Role::Seeker => Vec3::new(x, -sign * b.half.y * 0.2, mid + 600.0),
    };
    arena::clamp_into(b, p)
}

/// `zone`, for an agent that already exists.
pub fn zone_of(a: &Agent, w: &AiWorld) -> Vec3 {
    zone(a.team, a.role, a.id, &w.bounds, w.hoops)
}

/// The standard seven a side, less Harry: 3 Chasers, 2 Beaters and 1 Keeper for the home side (whose Seeker is
/// the player) and the same plus a Seeker for the opponent. Ids run from 1, because `HARRY` is 0.
pub fn standard_teams() -> Vec<Agent> {
    let b = arena::bounds();
    let hoops = arena::hoops();
    let mut out: Vec<Agent> = Vec::with_capacity(13);
    let mut id = 1u32;
    for team in [Team::Home, Team::Away] {
        let roles: &[Role] = match team {
            // Harry is the home Seeker, so his side has no Seeker agent.
            Team::Home => &[Role::Chaser, Role::Chaser, Role::Chaser, Role::Beater, Role::Beater, Role::Keeper],
            Team::Away => &[Role::Chaser, Role::Chaser, Role::Chaser, Role::Beater, Role::Beater, Role::Keeper, Role::Seeker],
        };
        for &role in roles {
            let pid = PlayerId(id);
            id += 1;
            let pos = zone(team, role, pid, &b, &hoops);
            // Facing the middle of the pitch, as everyone lines up for the kickoff.
            let look = b.center - pos;
            let yaw = if look.truncate().length() > 1e-3 { look.y.atan2(look.x) } else { 0.0 };
            out.push(Agent { id: pid, team, role, body: BroomState::new(pos, Vec3::ZERO, yaw, 0.0), age: 0.0 });
        }
    }
    out
}

/// Which agents get a full `step_agent` this frame; the rest are flown by `drift_agent`. The ball comes first
/// when the frame will not pay for everyone, then the goal, then the Bludgers, and last the opposing Seeker,
/// who only flies about.
pub fn active_agents(all: &[Agent], budget: &Budget) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..all.len()).collect();
    idx.sort_by_key(|&i| match all[i].role {
        Role::Chaser => 0,
        Role::Keeper => 1,
        Role::Beater => 2,
        Role::Seeker => 3,
    });
    idx.truncate(budget.max_active_ai.min(all.len()));
    idx
}

/// The steering that points `s` at `target`: the look direction is nudged towards it, no further than a hand
/// could shove a mouse in one frame, and the broom's own rate caps fly the turn. The headless pilot steers
/// Harry the same way.
fn steer_to(s: &BroomState, target: Vec3, boost: bool, brake: bool) -> BroomInput {
    let plain = BroomInput { boost, brake, ..BroomInput::default() };
    let d = target - s.pos;
    if !d.is_finite() || d.length_squared() < 1.0 {
        return plain;
    }
    let flat = d.truncate().length();
    let yaw = d.y.atan2(d.x);
    let pitch = d.z.atan2(flat.max(1e-3));
    BroomInput {
        look_yaw: flight::wrap_pi(yaw - s.want_yaw).clamp(-LOOK_STEP, LOOK_STEP),
        look_pitch: (pitch - s.want_pitch).clamp(-LOOK_STEP, LOOK_STEP),
        ..plain
    }
}

/// Flies one agent for a frame: the soft boundary that turns him back from the walls, the flight model, and
/// the hard boundary that keeps him in the volume whatever he was told to do.
fn fly(a: &mut Agent, w: &AiWorld, i: &BroomInput, dt: f32) {
    let mut i = *i;
    i.yaw = (i.yaw + flight::boundary_turn(a.body.pos, a.body.want_yaw, &w.bounds) * 0.8).clamp(-1.0, 1.0);
    let flown = flight::step(&a.body, &i, &BroomTuning::default(), dt);
    a.body = flight::confine(&flown, &w.bounds);
}

/// One frame of one agent: his role's state machine, then the flight. What he asks the match for comes back.
pub fn step_agent(a: &mut Agent, w: &AiWorld, dt: f32) -> Vec<AiAction> {
    let dt = if dt.is_finite() { dt.clamp(0.0, 0.25) } else { 0.0 };
    a.age += dt;
    let plan = match a.role {
        Role::Chaser => chaser(a, w),
        Role::Beater => beater(a, w),
        Role::Keeper => keeper(a, w),
        Role::Seeker => seeker(a, w),
    };
    let i = steer_to(&a.body, arena::clamp_into(&w.bounds, plan.target), plan.boost, plan.brake);
    fly(a, w, &i, dt);
    plan.actions
}

/// The cheap half of the budget: an agent nobody is looking closely at flies to his zone and asks for
/// nothing. No searching of the ball, the hoops, the Bludgers or the other players.
pub fn drift_agent(a: &mut Agent, w: &AiWorld, dt: f32) {
    let dt = if dt.is_finite() { dt.clamp(0.0, 0.25) } else { 0.0 };
    a.age += dt;
    let i = steer_to(&a.body, zone_of(a, w), false, false);
    fly(a, w, &i, dt);
}

/// The nearest Chaser of either side to `p`, who is the one that goes for a loose ball.
fn nearest_chaser(w: &AiWorld, p: Vec3) -> Option<PlayerId> {
    let mut best: Option<(f32, PlayerId)> = None;
    for v in w.players.iter().filter(|v| v.role == Role::Chaser) {
        let d = v.pos.distance(p);
        if !d.is_finite() {
            continue;
        }
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, v.id));
        }
    }
    best.map(|(_, id)| id)
}

/// The hoop of the far goal a shot from `from` is on at, nearest first. `None` means no shot from here.
fn shot_from(from: Vec3, w: &AiWorld, team: Team) -> Option<Hoop> {
    let mut best: Option<(f32, Hoop)> = None;
    for h in w.hoops.iter().filter(|h| is_attack(h, team) && shot_is_on(from, h)) {
        let d = from.distance(h.center);
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, *h));
        }
    }
    best.map(|(_, h)| h)
}

/// The team-mate a Chaser passes to: a Chaser of his own side, well clear of him, nearer the goal than he is
/// and with nobody marking him.
fn open_mate(a: &Agent, w: &AiWorld) -> Option<AgentView> {
    let goal = goal_center(w.hoops, a.team.other());
    let mine = a.body.pos.distance(goal);
    let mut best: Option<(f32, AgentView)> = None;
    for v in w.players.iter().filter(|v| v.team == a.team && v.role == Role::Chaser && v.id != a.id) {
        let d = v.pos.distance(goal);
        let reach = a.body.pos.distance(v.pos);
        if !d.is_finite() || !reach.is_finite() || d > mine - PASS_GAIN || reach < PASS_MIN || reach > PASS_RANGE {
            continue;
        }
        if w.players.iter().any(|o| o.team != a.team && o.pos.distance(v.pos) < OPEN_RADIUS) {
            continue;
        }
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, *v));
        }
    }
    best.map(|(_, v)| v)
}

/// A Chaser: shoot when a hoop of the far goal is on, else pass to someone better placed, else carry the ball
/// at the goal. Without the ball he goes for a loose one if he is nearest, presses whoever of the other side
/// has it, and otherwise takes up his position.
fn chaser(a: &Agent, w: &AiWorld) -> Plan {
    if w.holder == Some(a.id) {
        if let Some(h) = shot_from(a.body.pos, w, a.team) {
            return Plan { target: h.center, actions: vec![AiAction::Shoot], boost: true, brake: false };
        }
        if let Some(v) = open_mate(a, w) {
            return Plan { target: v.pos, actions: vec![AiAction::Pass(v.id)], boost: false, brake: false };
        }
        // Not yet in range: fly at the point in front of the nearest hoop a shot is taken from.
        let goal = goal_center(w.hoops, a.team.other());
        let at = w
            .hoops
            .iter()
            .filter(|h| is_attack(h, a.team))
            .min_by(|x, y| a.body.pos.distance(x.center).total_cmp(&a.body.pos.distance(y.center)))
            .map(|h| h.center + h.normal * APPROACH)
            .unwrap_or(goal);
        return Plan::fly_to(at, true);
    }
    match w.holder {
        None => {
            if nearest_chaser(w, w.quaffle) == Some(a.id) {
                Plan::fly_to(w.quaffle, true)
            } else {
                Plan::fly_to(zone_of(a, w), false)
            }
        }
        Some(h) => {
            let holder = w.players.iter().find(|v| v.id == h);
            match holder.filter(|v| v.team != a.team) {
                Some(v) => Plan::fly_to(v.pos, true),
                None => Plan::fly_to(zone_of(a, w), true),
            }
        }
    }
}

/// Whether `p` is in the way of the Bludger at `pos` flying `vel`.
fn in_the_way(pos: Vec3, vel: Vec3, p: Vec3) -> bool {
    let Some(d) = vel.try_normalize() else { return false };
    let to = p - pos;
    let along = to.dot(d);
    along > 0.0 && along < THREAT_LENGTH && (to - d * along).length() < THREAT_RADIUS
}

/// A Beater: the Bludger heading for one of his own, swatted at the nearest opponent. With nothing to defend
/// he shadows the nearest Bludger, and with no Bludger in the air he holds the middle.
fn beater(a: &Agent, w: &AiWorld) -> Plan {
    // His own side, Harry included: the Bludgers fly at Harry, so his Beaters are the ones with work.
    let threat = w.bludgers.iter().find(|(p, v)| {
        (a.team.is_home() && in_the_way(*p, *v, w.harry))
            || w.players.iter().any(|m| m.team == a.team && m.id != a.id && in_the_way(*p, *v, m.pos))
    });
    if let Some(&(p, _)) = threat {
        if a.body.pos.distance(p) <= BEAT_RADIUS {
            // At the nearest opponent, or, with none to be found, back down the pitch he attacks.
            let goal = goal_center(w.hoops, a.team.other());
            let at = w
                .players
                .iter()
                .filter(|v| v.team != a.team)
                .min_by(|x, y| p.distance(x.pos).total_cmp(&p.distance(y.pos)))
                .map(|v| v.pos)
                .unwrap_or(goal);
            let dir = (at - p).try_normalize().unwrap_or_else(|| (goal - p).try_normalize().unwrap_or(Vec3::Y));
            return Plan { target: p, actions: vec![AiAction::HitBludger(dir)], boost: true, brake: false };
        }
        return Plan::fly_to(p, true);
    }
    match w.bludgers.iter().min_by(|x, y| a.body.pos.distance(x.0).total_cmp(&a.body.pos.distance(y.0))) {
        Some(&(p, _)) => Plan::fly_to(p, true),
        None => Plan::fly_to(zone_of(a, w), false),
    }
}

/// A Keeper: on the line from the Quaffle to his own hoops, a fixed stand-off in front of them, and never
/// outside `KEEPER_BAND` of them. A broom cannot stop, so once he is on his line he flies a tight circle in
/// front of the posts instead of trying to hover.
fn keeper(a: &Agent, w: &AiWorld) -> Plan {
    let goal = goal_center(w.hoops, a.team);
    let into = own_normal(w.hoops, a.team);
    let ball = if w.quaffle.is_finite() { w.quaffle } else { goal + into * 1000.0 };
    let line = (ball - goal).try_normalize().unwrap_or(into);
    let post = goal + line * KEEPER_STANDOFF;
    let off = a.body.pos - goal;
    let home_d = off.length();
    let target = if a.body.pos.distance(post) < KEEPER_STANDOFF * 0.6 {
        let side = Vec3::new(-off.y, off.x, 0.0).try_normalize().unwrap_or(Vec3::X);
        goal + off.try_normalize().unwrap_or(into) * KEEPER_STANDOFF + side * (KEEPER_STANDOFF * 0.8)
    } else {
        post
    };
    let actions = if ball.distance(goal) < BLOCK_RANGE { vec![AiAction::Block] } else { Vec::new() };
    // Out of position he hurries back; on his line he flies as slowly as a broom can.
    Plan { target, actions, boost: home_d > KEEPER_BAND * 0.5, brake: home_d < KEEPER_STANDOFF }
}

/// The opposing Seeker: he leaves the snitch alone for `SEEKER_DELAY` seconds (the match has to get going
/// first), then chases it. He can never win it -- Harry's catch is what ends the match -- so this is for show.
fn seeker(a: &Agent, w: &AiWorld) -> Plan {
    if a.age < SEEKER_DELAY {
        return Plan::fly_to(zone_of(a, w), false);
    }
    Plan { target: w.snitch, actions: vec![AiAction::ChaseSnitch], boost: true, brake: false }
}

/// Which of the far goal's hoops a shot from `from` goes at: of the ones that are on, the least defended, and
/// a coin toss between equals. `None` when no shot from there is on at all.
fn pick_hoop(from: Vec3, w: &AiWorld, team: Team, rng: &mut Rng) -> Option<Hoop> {
    let mut best: Option<(usize, Hoop)> = None;
    let mut seen = 0u32;
    for h in w.hoops.iter().filter(|h| is_attack(h, team) && shot_is_on(from, h)) {
        let guards = w.players.iter().filter(|v| v.team != team && v.pos.distance(h.center) < DEFENDED).count();
        match best {
            Some((bg, _)) if guards > bg => continue,
            Some((bg, _)) if guards == bg => {
                // Reservoir of one over the equals, so the three hoops share the shots.
                seen += 1;
                if rng.unit() * seen as f32 >= 1.0 {
                    continue;
                }
            }
            _ => seen = 1,
        }
        best = Some((guards, *h));
    }
    best.map(|(_, h)| h)
}

/// What the match did this frame, for the log, the commentary and the Bludgers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TeamEvent {
    Caught { by: PlayerId },
    Passed { from: PlayerId, to: PlayerId },
    Shot { by: PlayerId, home: bool },
    /// The Beater swatted Bludger `index` off along `dir`; only `bludger::Bludgers` can do it.
    Beat { index: usize, dir: Vec3 },
}

/// Both teams and the Quaffle play between them: the one pure thing a match ticks for its AI.
pub struct TeamPlay {
    pub agents: Vec<Agent>,
    rng: Rng,
    views: Vec<AgentView>,
}

impl TeamPlay {
    pub fn new(seed: u64) -> TeamPlay {
        TeamPlay { agents: standard_teams(), rng: Rng::new(seed ^ 0x7a11_1deb_eef0_1234), views: Vec::new() }
    }

    /// The agent of `team` nearest `p`, in whatever role.
    fn nearest(&self, team: Team, role: Role, p: Vec3) -> Option<usize> {
        let mut best: Option<(f32, usize)> = None;
        for (i, a) in self.agents.iter().enumerate().filter(|(_, a)| a.team == team && a.role == role) {
            let d = a.body.pos.distance(p);
            if !d.is_finite() {
                continue;
            }
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, i));
            }
        }
        best.map(|(_, i)| i)
    }

    /// Hands the ball to the Chaser of `team` nearest the middle of the pitch: the kickoff, and the restart
    /// the conceding side takes after a goal. Without it a symmetric pitch would give every loose ball to the
    /// same side for the whole match.
    pub fn hand_to(&mut self, q: &mut Quaffle, team: Team, at: Vec3) {
        let Some(i) = self.nearest(team, Role::Chaser, at) else { return };
        q.holder = Some(self.agents[i].id);
        q.carry(self.agents[i].body.pos);
    }

    /// The kickoff: a toss for which side starts with the Quaffle, which is then in that side's hands.
    pub fn kickoff(&mut self, q: &mut Quaffle, at: Vec3) -> Team {
        let team = if self.rng.unit() < 0.5 { Team::Home } else { Team::Away };
        self.hand_to(q, team, at);
        team
    }

    /// One frame of both teams. `bludgers` is every Bludger in flight (`bludger::Bludgers::flying`), and the
    /// `Beat` events that come back are the swats the caller applies with `Bludgers::redirect`.
    #[allow(clippy::too_many_arguments)]
    pub fn step(
        &mut self,
        bounds: &ArenaBounds,
        hoops: &[Hoop],
        q: &mut Quaffle,
        bludgers: &[(Vec3, Vec3)],
        harry: Vec3,
        snitch: Vec3,
        budget: &Budget,
        dt: f32,
    ) -> Vec<TeamEvent> {
        // The frame's snapshot: every agent as the others see him, kept in the struct so the tick allocates
        // nothing.
        self.views.clear();
        self.views.extend(self.agents.iter().map(|a| AgentView { id: a.id, team: a.team, role: a.role, pos: a.body.pos }));
        let w = AiWorld {
            bounds: *bounds,
            hoops,
            quaffle: q.pos,
            holder: q.holder,
            bludgers,
            harry,
            snitch,
            players: &self.views,
        };
        let active = active_agents(&self.agents, budget);
        let mut asked: Vec<(usize, AiAction)> = Vec::new();
        for i in 0..self.agents.len() {
            if active.contains(&i) {
                for act in step_agent(&mut self.agents[i], &w, dt) {
                    asked.push((i, act));
                }
            } else {
                drift_agent(&mut self.agents[i], &w, dt);
            }
        }
        let mut out: Vec<TeamEvent> = Vec::new();
        for (i, act) in asked {
            let (id, team, from) = (self.agents[i].id, self.agents[i].team, self.agents[i].body.pos);
            match act {
                AiAction::Pass(to) if q.holder == Some(id) => {
                    let Some(v) = self.views.iter().find(|v| v.id == to) else { continue };
                    q.pass(from, v.pos, quaffle::PASS_SPEED);
                    out.push(TeamEvent::Passed { from: id, to });
                }
                AiAction::Shoot if q.holder == Some(id) => {
                    let Some(h) = pick_hoop(from, &w, team, &mut self.rng) else { continue };
                    q.shoot(from, h.center, quaffle::SHOOT_SPEED);
                    out.push(TeamEvent::Shot { by: id, home: h.home });
                }
                AiAction::HitBludger(dir) => {
                    // Which ball he swatted: the one he is on.
                    let Some((index, _)) = bludgers
                        .iter()
                        .enumerate()
                        .filter(|(_, (p, _))| p.distance(from) <= BEAT_RADIUS)
                        .min_by(|(_, (x, _)), (_, (y, _))| x.distance(from).total_cmp(&y.distance(from)))
                    else {
                        continue;
                    };
                    out.push(TeamEvent::Beat { index, dir });
                }
                _ => {}
            }
        }
        // A loose ball goes to the nearest Chaser that can reach it, and whoever has it carries it.
        if q.catchable() {
            let hands: Vec<(PlayerId, Vec3)> =
                self.agents.iter().filter(|a| a.role == Role::Chaser).map(|a| (a.id, a.body.pos)).collect();
            if let Some(by) = q.try_catch(&hands, CATCH_RADIUS) {
                out.push(TeamEvent::Caught { by });
            }
        }
        if let Some(h) = q.holder
            && let Some(a) = self.agents.iter().find(|a| a.id == h)
        {
            let ahead = a.body.vel.try_normalize().unwrap_or(Vec3::Y) * CARRY_AHEAD;
            q.carry(arena::clamp_into(bounds, a.body.pos + ahead));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    fn views(all: &[Agent]) -> Vec<AgentView> {
        all.iter().map(|a| AgentView { id: a.id, team: a.team, role: a.role, pos: a.body.pos }).collect()
    }

    /// An `AiWorld` over `all` with the ball at `quaffle` and nothing else going on.
    fn world<'a>(b: &ArenaBounds, hoops: &'a [Hoop], players: &'a [AgentView], quaffle: Vec3, holder: Option<PlayerId>, bludgers: &'a [(Vec3, Vec3)]) -> AiWorld<'a> {
        AiWorld {
            bounds: *b,
            hoops,
            quaffle,
            holder,
            bludgers,
            harry: arena::harry_start(b),
            snitch: b.center,
            players,
        }
    }

    fn agent(id: u32, team: Team, role: Role, pos: Vec3) -> Agent {
        Agent { id: PlayerId(id), team, role, body: BroomState::new(pos, Vec3::ZERO, 0.0, 0.0), age: 0.0 }
    }

    #[test]
    fn chaser_with_quaffle_shoots_when_in_range() {
        let (b, hoops) = (arena::bounds(), arena::hoops());
        // In front of a north hoop (the one Gryffindor attacks), a thousand units out.
        let h = *hoops.iter().find(|h| h.home).expect("a north hoop");
        let me = agent(1, Team::Home, Role::Chaser, h.center + h.normal * 1000.0);
        let all = [me];
        let v = views(&all);
        let w = world(&b, &hoops, &v, me.body.pos, Some(me.id), &[]);
        let mut me = me;
        assert_eq!(step_agent(&mut me, &w, DT), vec![AiAction::Shoot]);
        // From his own half it is far too far: he carries it instead.
        let mut far = agent(1, Team::Home, Role::Chaser, Vec3::new(b.center.x, -2000.0, b.center.z));
        let all = [far];
        let v = views(&all);
        let w = world(&b, &hoops, &v, far.body.pos, Some(far.id), &[]);
        assert!(step_agent(&mut far, &w, DT).is_empty(), "no shot from the far half");
    }

    #[test]
    fn chaser_passes_to_open_teammate_when_far() {
        let (b, hoops) = (arena::bounds(), arena::hoops());
        // He has the ball in his own half; a team-mate is well up the pitch with nobody near him.
        let me = agent(1, Team::Home, Role::Chaser, Vec3::new(b.center.x, -2000.0, b.center.z));
        let mate = agent(2, Team::Home, Role::Chaser, Vec3::new(b.center.x, 1000.0, b.center.z));
        let all = [me, mate];
        let v = views(&all);
        let w = world(&b, &hoops, &v, me.body.pos, Some(me.id), &[]);
        let mut me = me;
        assert_eq!(step_agent(&mut me, &w, DT), vec![AiAction::Pass(mate.id)]);
        // Marked, he is not passed to.
        let marker = agent(9, Team::Away, Role::Chaser, mate.body.pos + Vec3::new(100.0, 0.0, 0.0));
        let all = [me, mate, marker];
        let v = views(&all);
        let w = world(&b, &hoops, &v, me.body.pos, Some(me.id), &[]);
        let mut me = me;
        assert!(step_agent(&mut me, &w, DT).is_empty(), "a marked team-mate is not passed to");
    }

    #[test]
    fn keeper_stays_within_hoop_radius_band() {
        let (b, hoops) = (arena::bounds(), arena::hoops());
        for team in [Team::Home, Team::Away] {
            let goal = goal_center(&hoops, team);
            let mut k = agent(6, team, Role::Keeper, zone(team, Role::Keeper, PlayerId(6), &b, &hoops));
            assert!(k.body.pos.distance(goal) <= KEEPER_BAND, "he does not even start in the band");
            // The ball runs up and down the pitch; the Keeper must stay in front of his own posts.
            for n in 0..1800 {
                let t = n as f32 * DT;
                let ball = Vec3::new(b.center.x + 2000.0 * (t * 0.7).sin(), 4000.0 * (t * 0.4).cos(), b.center.z + 500.0 * (t).sin());
                let all = [k];
                let v = views(&all);
                let w = world(&b, &hoops, &v, ball, None, &[]);
                step_agent(&mut k, &w, DT);
                assert!(k.body.pos.distance(goal) <= KEEPER_BAND, "frame {n}: {:?} is {} from his goal", k.body.pos, k.body.pos.distance(goal));
            }
        }
    }

    #[test]
    fn beater_hits_bludger_heading_for_teammate() {
        let (b, hoops) = (arena::bounds(), arena::hoops());
        let mate = agent(2, Team::Away, Role::Chaser, Vec3::new(b.center.x, 2000.0, b.center.z));
        let foe = agent(9, Team::Home, Role::Chaser, Vec3::new(b.center.x + 1500.0, -1000.0, b.center.z));
        // The Bludger is a hundred units from the Beater, flying straight at his team-mate.
        let bl = Vec3::new(b.center.x, 0.0, b.center.z);
        let vel = (mate.body.pos - bl).normalize() * 750.0;
        let me = agent(5, Team::Away, Role::Beater, bl + Vec3::new(100.0, 0.0, 0.0));
        let all = [me, mate, foe];
        let v = views(&all);
        let air = [(bl, vel)];
        let w = world(&b, &hoops, &v, b.center, None, &air);
        let mut me = me;
        let acts = step_agent(&mut me, &w, DT);
        let dir = match acts.as_slice() {
            [AiAction::HitBludger(d)] => *d,
            other => panic!("expected a swat, got {other:?}"),
        };
        // Swatted at the opponent, and it is a direction.
        assert!((dir.length() - 1.0).abs() < 1e-3, "{dir:?}");
        assert!(dir.dot((foe.body.pos - bl).normalize()) > 0.99, "{dir:?} is not at the opponent");
        // With nothing of his own in the way he only shadows the ball.
        let lone = [me];
        let v = views(&lone);
        let away = [(bl, Vec3::new(0.0, -750.0, 0.0))];
        let w = world(&b, &hoops, &v, b.center, None, &away);
        let mut me = me;
        assert!(step_agent(&mut me, &w, DT).is_empty(), "nothing to defend, nothing to swat");
    }

    #[test]
    fn opposing_seeker_chases_snitch_after_delay() {
        let (b, hoops) = (arena::bounds(), arena::hoops());
        let mut s = agent(13, Team::Away, Role::Seeker, zone(Team::Away, Role::Seeker, PlayerId(13), &b, &hoops));
        let all = [s];
        let v = views(&all);
        let w = world(&b, &hoops, &v, b.center, None, &[]);
        // Before the delay he keeps out of it.
        for _ in 0..((SEEKER_DELAY / DT) as usize - 2) {
            assert!(step_agent(&mut s, &w, DT).is_empty(), "he waits his turn");
        }
        // And after it he is on the snitch, every frame.
        let mut chasing = false;
        for _ in 0..120 {
            chasing |= step_agent(&mut s, &w, DT) == vec![AiAction::ChaseSnitch];
        }
        assert!(chasing, "the opposing Seeker never went for the snitch");
        assert!(s.age >= SEEKER_DELAY);
    }

    #[test]
    fn agents_stay_inside_arena_bounds() {
        let b = arena::bounds();
        let hoops = arena::hoops();
        let mut play = TeamPlay::new(7);
        let mut q = Quaffle::new(arena::bludger_home(&b));
        let budget = Budget::default();
        let lo = b.center - b.half;
        for n in 0..3000 {
            let t = n as f32 * DT;
            let harry = Vec3::new(b.center.x + 3000.0 * (t * 0.5).sin(), 3000.0 * (t * 0.3).cos(), b.center.z);
            let bludgers = [(b.center, Vec3::new(0.0, 700.0, 0.0))];
            play.step(&b, &hoops, &mut q, &bludgers, harry, b.center, &budget, DT);
            q.step(DT);
            q.confine(&b);
            for a in &play.agents {
                let p = a.body.pos;
                assert!(p.is_finite() && a.body.vel.is_finite(), "frame {n}: {a:?}");
                assert!((p.x - b.center.x).abs() <= b.half.x + 1e-2, "frame {n}: {p:?}");
                assert!((p.y - b.center.y).abs() <= b.half.y + 1e-2, "frame {n}: {p:?}");
                assert!(p.z >= lo.z - 1e-2 && p.z <= b.ceiling + 1e-2, "frame {n}: {p:?}");
            }
        }
    }

    #[test]
    fn active_agents_respects_budget() {
        let all = standard_teams();
        assert_eq!(all.len(), 13, "six for Harry's side and seven for the opponent's");
        // The whole budget steers everyone.
        let whole = active_agents(&all, &Budget::default());
        assert_eq!(whole.len(), all.len());
        // A tight one steers only as many as it says, and they are the ones nearest the ball.
        for n in [0usize, 1, 4, 7, 13, 50] {
            let got = active_agents(&all, &super::super::lod::budget_for(n));
            assert_eq!(got.len(), n.min(all.len()), "budget {n}");
            assert!(got.iter().all(|&i| i < all.len()), "budget {n}: {got:?}");
            let mut sorted = got.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(sorted.len(), got.len(), "budget {n}: repeated an agent");
            // The Seeker, who does nothing but fly, is the first to be dropped.
            if n < all.len() {
                assert!(got.iter().all(|&i| all[i].role != Role::Seeker), "budget {n} should not steer the Seeker");
            }
        }
    }

    /// Odd input never panics and never puts a NaN on an agent.
    #[test]
    fn odd_input_is_survived() {
        let b = arena::bounds();
        let hoops = arena::hoops();
        let mut play = TeamPlay::new(1);
        let mut q = Quaffle::new(b.center);
        for dt in [0.0, -1.0, f32::NAN, f32::INFINITY, 1e6, DT] {
            let bludgers = [(Vec3::splat(f32::NAN), Vec3::ZERO), (b.center, Vec3::ZERO)];
            play.step(&b, &hoops, &mut q, &bludgers, Vec3::splat(f32::NAN), Vec3::splat(f32::INFINITY), &Budget::default(), dt);
            for a in &play.agents {
                assert!(a.body.pos.is_finite() && a.body.vel.is_finite(), "dt {dt}: {a:?}");
            }
        }
        // No hoops and no players at all: every role still flies.
        let mut play = TeamPlay::new(2);
        let mut q = Quaffle::new(b.center);
        for _ in 0..60 {
            play.step(&b, &[], &mut q, &[], b.center, b.center, &super::super::lod::budget_for(0), DT);
        }
        assert!(play.agents.iter().all(|a| a.body.pos.is_finite()));
    }

    /// The restart that keeps one side from taking every loose ball: the conceding team gets it back.
    #[test]
    fn hand_to_gives_the_ball_to_that_sides_chaser() {
        let b = arena::bounds();
        let mut play = TeamPlay::new(3);
        let mut q = Quaffle::new(b.center);
        for team in [Team::Away, Team::Home] {
            play.hand_to(&mut q, team, b.center);
            let holder = q.holder.expect("someone has it");
            let who = play.agents.iter().find(|a| a.id == holder).expect("an agent of ours");
            assert_eq!(who.team, team);
            assert_eq!(who.role, Role::Chaser);
            assert_eq!(q.pos, who.body.pos);
        }
    }
}
