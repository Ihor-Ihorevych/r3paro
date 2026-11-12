# Quidditch arena (retail `HP3_Quidditch`, package `Quid_Stadium`)

Produced by `hp3-run quid-arena <HP3 folder>`. All units are Unreal units; Z is up. Mesh bounds are in mesh-local
space; world values below apply the placement of the retail map `HP3_Quidditch.unr`.

## Meshes (`StaticMeshes/HP3_ExteriorArch_SM.usx`, group `Quid_Stadium`)

| Mesh | local min | local max | tris | placed in retail map |
|---|---|---|---|---|
| `QUid_Pitch_SM` | (-4282.6, -6.4, 0.0) | (3.1, 6629.7, 3318.6) | 1080 | 4 copies (one quadrant each) |
| `Quttish_Pitch_SM` | (-3905.3, 0.0, 0.0) | (3905.3, 6464.5, 0.0) | 146 | no (flat plane, a half pitch) |
| `Quid_GoalPosts_SM` | (-5.0, -729.1, 0.0) | (5.0, 730.4, 2524.6) | 12 | 2 copies |
| `Q_stairs_SM` | (-175.6, -188.2, -2000.4) | (175.6, 188.2, 5.0) | 440 | 7 copies |
| `Quid_Tower_SM` | (-50.7, -341.0, 0.0) | (481.0, 222.0, 2963.1) | 499 | no |
| `Quid_Wallbase_tower_SM` | (-1832.6, -256.2, 0.0) | (74.8, 667.3, 1150.0) | 111 | no |
| `Quid_EndWall_SM` | (-1067.7, 0.0, 0.0) | (408.5, 1762.2, 1150.0) | 364 | no |
| `Quid_SideWall_SM` | (-1288.8, -246.2, 0.0) | (25.5, 499.3, 1678.8) | 183 | no |

There is no `Quid_GoldHoop` mesh in any static mesh package (a scan of every `.usx` and the map for names containing
`hoop` finds only `Quid_GoalPosts_SM`).

**Harry rides no separate broom actor.** The retail `HarryQuid` skeletal mesh already has a broom modelled into
it, posed by its `hover` animation; nothing has to be attached. There *is* a loose broom static mesh --
`HP3_InteriorAccents_SM.miscDetails.QuiddichBroom_SM` (not `QuidBroom_SM`: that string is in the name table of
`StaticMeshes/HP3_InteriorAccents_SM.usx` but no static mesh carries it) -- and the `HarryQuid` skeleton has a
last bone named `body_broom`. An earlier pass hung that mesh off that bone, which gave Harry **two brooms**: the
modelled one under him and the static one sticking out sideways across his body, because the bone's rest
orientation is not the broom's. `glue::start` now attaches nothing, and `stop` therefore has one actor fewer to
destroy. The AI riders (`QuidPlayer*`) were never given one: their meshes carry their brooms the same way.

## Assembled or loose

Loose parts. The pitch is four copies of one quadrant mesh (`QUid_Pitch_SM`, which spans x -4283..3, y 0..6630) around
the pitch centre `(271.9, 116.0, -1009.9)`: identity, `DrawScale3D.x = -1`, yaw 180 deg, and yaw 180 deg with
`DrawScale3D.x = -1`. Together they cover x +-4283, y +-6630 around the centre (stands included; z up to 3318.6 above the
origin, world 2308.7). The map places only pitch, goal posts and stairs; towers and walls are not placed in
`HP3_Quidditch`. Terrain `TerrainInfo1`: 4x4 sectors, 64x64 heightmap, 4096 vertices, vertex bounds
(-16335, -16566, -1246) .. (15921, 15690, 3864) in terrain space (a backdrop far outside the pitch).

The retail placements, exactly as the map stores them (`Rotation` is pitch, yaw, roll in 65536ths of a turn; all
locations are world, not relative to the pitch centre). `quidditch::arena::arena_spawn_list` spawns this list.

| mesh | location | rotation | DrawScale3D |
|---|---|---|---|
| `QUid_Pitch_SM` | (271.9, 116.0, -1009.9) | [0, 0, 0] | (1, 1, 1) |
| `QUid_Pitch_SM` | (271.9, 116.0, -1009.9) | [0, 0, 0] | (-1, 1, 1) |
| `QUid_Pitch_SM` | (271.9, 116.0, -1009.9) | [0, -32768, 0] | (1, 1, 1) |
| `QUid_Pitch_SM` | (271.9, 116.0, -1009.9) | [0, -32768, 0] | (-1, 1, 1) |
| `Quid_GoalPosts_SM` | (256, 4096, -960) | [0, 16384, 0] | (1, 1, 1) |
| `Quid_GoalPosts_SM` | (256, -4096, -960) | [0, -16384, 0] | (1, 1, 1) |
| `Q_stairs_SM` | (4026.4, 2924.6, 824.8) | [0, 3504, 0] | (1, 1, 1) |
| `Q_stairs_SM` | (2364.0, 5880.7, 824.8) | [0, 7456, 0] | (1, 1, 1) |
| `Q_stairs_SM` | (3329.3, -4333.8, 824.8) | [0, -5344, 0] | (1, 1, 1) |
| `Q_stairs_SM` | (-1812.9, 5889.4, 824.8) | [0, 25112, 0] | (1, 1, 1) |
| `Q_stairs_SM` | (-2786.9, -4326.7, 824.8) | [0, 38128, 0] | (1, 1, 1) |
| `Q_stairs_SM` | (4285.9, -809.1, 824.8) | [0, -848, 0] | (1, 1, 1) |
| `Q_stairs_SM` | (-3733.3, 1038.2, 824.8) | [0, 31968, 0] | (1, 1, 1) |

## Goal posts

`Quid_GoalPosts_SM` is three thin poles (x thickness 10) in the local YZ plane. Each pole carries a flat quad on top
that is the hoop face (240 wide, about 237 tall, normal along local X). There is no ring mesh, so hoops are primitive
rings centred on those quads. Local hoop centres (y, z) and radius:

- y = +610, z = 1118 (pole top 1007)
- y = 0, z = 2405 (pole top 2292)
- y = -610, z = 1705 (pole top 1594)
- radius 120 (half the quad width; use about 150 for forgiving scoring)

Placement: `StaticMeshActor78` at (256, 4096, -960) yaw +90 deg (16384 uu) and `StaticMeshActor79` at (256, -4096, -960)
yaw -90 deg. After rotation the local Y axis lies along world X, so each goal's three hoops are spread along X and
face along Y (towards the pitch centre). World hoop centres, Z = -960 + local z. **Verified in-engine** (Task 6): the
goals were spawned into a loaded level and the local hoop offsets turned with the engine's own rotator maths
(`hp3_script::natives::rot_axes`, `HP3_QUID_DEBUG=1`), which printed exactly the table below, so the yaw sign
convention used here is right (yaw turns +X towards +Y, so yaw +90 deg sends local +Y to world -X):

| goal | hoop | centre (x, y, z) |
|---|---|---|
| north (y = +4096, yaw +90) | low | (-354, 4096, 158) |
| north | high | (256, 4096, 1445) |
| north | mid | (866, 4096, 745) |
| south (y = -4096, yaw -90) | low | (866, -4096, 158) |
| south | high | (256, -4096, 1445) |
| south | mid | (-354, -4096, 745) |

Hoop radius 120. Posts' tops: world z 1565 at most.

`quidditch::arena::hoops()` returns those six as `goals::Hoop`s, each with its radius and a unit normal
pointing into the pitch (the north goal's look down -y, the south goal's up +y): that normal is the side a
shot has to come from, and `goals::goal_scored` is the disc crossing test. **Which end is whose:** Harry's
team (Gryffindor, the home side) defends the south goal (`y = -4096`, where `harry_start` puts him) and
attacks the north, so a ball through a north hoop is a home goal and one through a south hoop is the
opponent's. Nothing in the retail map says so; it is this engine's choice, and `harry_start` is the only
thing that has to agree with it.

### Hoop rings: what there is to draw them with

**There is no ring mesh anywhere in the retail data.** The whole of `StaticMeshes/`, `Animations/` and the maps
was searched for one (Task 17), and this is everything that turned up:

| candidate | what it really is |
|---|---|
| `HGame.HippoRing` (the Buckbeak flight rings, `HP3_COMC` places ~110 of them) | a `Keypoint` with **no mesh at all**: its visual is `ringFXClass = HGame.HippoRingEmitter`, a particle emitter whose three emitters are `HP3_FX.Bats` flappers (not `Particles.Ring`), 12 sprites each in a polar circle of about 200 uu. Its size is only a collision box, `CollisionWidth`/`Height` 192 at `DrawScale` 5. Nothing to spawn -- and spawning the emitter itself was tried for the Snitch's ring trail and dropped; see "The Snitch's ring trail" below. |
| `HP3_Particles_SM.Character_Shapes.DementorSpawnRing_SM` | the only mesh in the game with a hole through it, but a **capless cylinder band**: 50 vertices, all at radius 1500, at z 0 or 64, outward normals, one section, 48 faces. Bounding box 3000 x 3000 x 64, axis +z. Looked at down its own axis -- which is how a goal hoop is seen -- it projects to nothing, and in a render only two thin arcs of it survive. |
| `HP3_ArchiExterior.Quid_Stadium.Quid_GoldHoop` | a **texture**, not a mesh: the gold ring the retail goal-post quad wears. It draws correctly (masked, the sky shows through the middle), but the level's own lighting leaves the goal posts near black for the whole match, so in play the retail ring reads as a thin black outline. |
| `post_ring_SM`, `FogRing`, `WaterRing`, `WispyRing` | a hitching post (bounds 72 x 40 x 126) and three textures. |

So a ring is **generated**: `quidditch::ball` draws each one as **twelve round static meshes of radius 32
evenly around the hoop's rim** (`120 * sin(180/12) = 31`, so the beads touch), 72 in all, skinned with
`HP3_ArchiExterior.Quid_Stadium.Quid_GoalPost` -- the goal posts' own gold -- and `bUnlit`, so a hoop is a
gold ring from across the pitch whatever the level's lighting does. A ball is round from every side, so
unlike a flat ring it never vanishes when the hoop is flown at head on. The beads are spawned
`bCollideActors = false` (a hoop is scored through; `goals::goal_scored` is the only test that matters) and
are destroyed with the rest of the match's actors.

Two traps, both found the hard way:

- **A spawned static mesh must not be `bStatic`.** `hp3_engine::actor_instances` skips `bStatic` static-mesh
  actors because the map's own are baked into the level scene instead -- and nothing bakes a spawned one, so
  it is drawn by nobody. The beads were `bStatic` until Task 17 and had never once been on screen.
- **`HP_FX.General.Bludger_Halo` is a `Fire.IceTexture`**, a procedural material nothing in this engine
  generates, so anything skinned with it comes out black.

The retail quads are left exactly where they are. The Quaffle is the same stand-in mesh at `quaffle::RADIUS`.
A free Quaffle falls at 980 uu/s^2 and comes to rest on the turf (`GROUND_Z` + its radius), not on the flight
volume's floor, which is 150 uu of broom clearance over the grass.

## Chosen arena constants

These are what `quidditch::arena` holds (`bounds()`, and the `pub const`s beside it).

- Ground level z = -960 (goal post base and player starts at -963..-835).
- Pitch centre x = 272. **The flight volume is centred on the goals, not on the pitch mesh.** The mesh's own centre is
  y = 116 while the goals stand at y = +-4096, so bounds centred on the mesh would leave 384 more playfield behind the
  south goal than behind the north one. The volume's centre y is therefore 0 and its half-extent 4596 (4096 + a 500
  margin), which gives both ends the same margin. (Changed in Task 6; earlier revisions of this file centred on y = 116
  with half-extent 4600.)
- Playfield half-extents: x 3900 (matches the flat `Quttish_Pitch_SM` half-width 3905), y 4596. The pitch mesh itself
  reaches x +-4283, y +-6630 (stands included), so the playfield sits inside it.
- Flight volume in z: floor -810 (ground -960 plus a 150 clearance, so nothing flies inside the pitch) up to the
  ceiling 2300 (stand top, world 2308.7; the tallest hoop top is 1565). `ArenaBounds` stores that as
  centre z 745, half z 1555, ceiling 2300.
- Harry's start: over his own (south) goal line, (272, -3296, 227) - 800 uu in front of the goal, a third of the way up
  the volume, facing north (yaw 16384).
- Bludger home (where they are launched from): the middle of the pitch at mid height, (272, 0, 745), as the real game
  releases the balls at the centre circle. Harry's start is 3336 uu away, inside the default launch band 800..3500, so
  Bludgers engage from the kickoff.
- Hoops: the six world centres above, radius 120 (150 forgiving).
- Towers and walls: not in the retail map, so nothing places them. The seven `Q_stairs_SM` placements in the table
  above (z 824.8) mark where the stands sit; `Quid_Tower_SM` (footprint 532 x 563, height 2963, local origin near its
  centre) could go at the four pitch corners (+-4400, +-6000) relative to the centre, but those positions would be
  invented, so Task 6 leaves them out. Note that three of the seven stairs sit inside the playfield box in x and y:
  they are stand pieces at z 824.8, above the pitch, and `arena_spawn_list`'s test treats them as such.

## Broom steering and the chase camera

Since the broom rework the broom is flown the way AdamJD's HP1 "Movement mod" flies it (the behaviour is
written up in `docs/formats/hp1-broom-movement-mod.md`, from which this was implemented clean-room). The
retail scheme -- the stick *is* the turn rate, the camera follows the broom -- is gone.

**Steering is free mouse-look.** `BroomState` carries a look direction (`want_yaw`, `want_pitch`) as well as
the broom's own heading. The mouse's motion this frame is an angular delta that moves the look
(`BroomInput::look_yaw` / `look_pitch`, radians, `input::MOUSE_GAIN` = 0.003 rad per pixel, **never scaled by
`dt`** -- pixels are already per-frame; the mod's own `dt` factor makes its sensitivity depend on the frame
rate and is deliberately not reproduced). The arrow keys and the phone's stick move the same look at a
constant rate instead (`BroomTuning::yaw_rate` 2.0 rad/s, `pitch_rate` 1.5 rad/s), so keyboard-only and
thumb-only flying still work. The look's pitch is clamped to `max_look_pitch` = 1.342 rad (76.9 deg, the mod's
camera limit).

**The broom then turns into the look**, closing `chase_tightness` = 8/s of the gap each frame (a 1/8 s time
constant) and never faster than `yaw_rate_max` = 4.80 rad/s (274.7 deg/s) or `pitch_rate_max` = 2.30 rad/s
(131.8 deg/s) -- the mod's effective `RotationRate`. Its own pitch is clamped to `max_pitch` = 1.4 rad as a
backstop. A mouse flick of half a turn is therefore a turn the broom takes about a third of a second to fly,
which is what traces the long arcs the mod is liked for. There is no pitch auto-levelling and no hard 60 deg
limit on the steering input, and -- unlike the mod -- a turn never brakes: the brake is the brake button.

**Speed**: cruise **450**, boost **900**, brake **112.5**, `accel` **562.5**, brake beats boost. The mod
changes no speed; the halving was the playtest's, and the further x0.75 is the owner's after playing ("make
Harry slower"). The turn rates were left alone both times, so the slower broom turns the more tightly.

The soft boundary (`flight::boundary_turn`) now steers the *look* back over the pitch, not the broom's
heading, and `flight::confine` levels the look with the broom when it slides along the floor or the ceiling.

**The camera is an orbit camera, and the broom follows it.** `camera::chase_camera` aims along the rider's
look direction, not along the broom's heading (so steering is camera-relative, as in the mod), with its own
lagged rotation in both axes: 8/s towards the look, at most 1.917 rad of yaw and 0.959 rad of pitch in one
step, pitch clamped to +-1.342 rad. It sits on a 200 uu arm (`ARM`) from a point 116 uu over the rider
(`AIM_UP`), 30 uu off that arm square to it (`RISE`), which puts it **146 uu over the rider in level flight**
(`CAM_UP`) and tips its own facing 8.5 deg below the look (`TILT`, and its distance from the look-at point is
`SLANT` = 1.011 arms) -- still much closer than the 420/140 rig it replaces -- with no positional lag at all.
Standing square to the arm rather than straight up is what keeps the framing the same at every pitch: the
whole rig simply tips with the look. Harry sits 27.6 deg under the middle of the picture (it was 23.3 with the
old 86 uu rig and no tilt), so he is a little lower in the frame and more of the pitch ahead is in it: the
owner asked for the camera "a little bit up" after playing. Looking up
from below pulls the arm in by `1 - pitch/0.9587`, immediately, and it eases back out at 2.5/s. The arm is also
shortened so the camera stops 5 uu short of the arena wall instead of hanging in the stands (the mod traces the
world; the flight volume is all the collision a match has).

**The arm is 200 uu, not the mod's 128.** `docs/formats/hp1-broom-movement-mod.md` records `fLookAtDistance` =
128 and `vLookAtOffset.Z` = 55; ours is deliberately farther, because at 128 uu Harry fills the middle of the
screen and blocks the view ahead. The whole rig is scaled by the same 200/128 (the look-at offset with it), so
the only thing that changes is how big he is in the frame -- he keeps his place in the picture, and the
pull-in, the wall shortening and the camera volume all follow `ARM` and need no separate tuning.

Two further deliberate differences from the mod, both because its numbers put the rider out of the picture at
our scale: the pull-in scales the **whole rig** (the look-at offset shrinks with the arm), so the rider keeps
his place in the frame however far the camera is pulled in, and the pull-in floor is **0.3**, not the mod's
0.15, which would end with the camera inside him.

**The field of view widens with speed, smoothly** (ours; neither HP1 package has it). It is the one field-of-
view mechanism a match has -- the slide's `dynamic_fov` in `apps/hp3` adds nothing while a match runs, so the
two never stack. The rule, all of it in `camera::chase_camera` so it is unit-tested with the rest of the rig:

* target = **90** deg (`FOV`) + **15** deg (`FOV_BOOST`) x `smoothstep` of the speed between the broom's own
  cruise and boost speeds, **450 and 900 uu/s**, read from `BroomTuning::default()` rather than written down
  a second time (so retuning the broom retunes the field of view with it);
* the field of view carried in `CamState.fov` is low-passed towards that target every frame,
  `fov += (target - fov) * (1 - exp(-dt / 0.35))` -- a time constant of **0.35 s** (`FOV_TAU`), frame-rate
  independent, so boost on, boost off and braking all ease in and out and nothing ever jumps;
* clamped to 90..105 deg, and a non-finite `dt` or a rubbish frame before it starts again from 90.

A boost toggle traced with `HP3_QUID_DEBUG` (the tick line carries `fov`) ramps 90 -> 105 over about a second
and back down again the same way.

## The level's own scripts during a match

`HP3_Quidditch` is not a playable pitch in the retail game: it is the stage of the Dementor cutscene
`7010QuidDem_ALL`. Left running, that cutscene takes the camera (`HPCam` goes to `StateCutSceneCam`), flies
four `Dementor`s and two `BlackDog`s over the pitch, and finishes by switching the level to
`HP3_GroundsDada?GameState=GState070` -- about 160 seconds in, which is what used to end a match.

So `quidditch::stage::clear` (from `glue::start`, and from nowhere else) destroys the level's cutscene
machinery -- `KWCutScene`, `KWCutController*`, `KWCutTextController`, `CutFactory`, `CutSceneTrigger` -- and
its cutscene-only cast -- `Dementor`, `BlackDog`, `DementorsConverging` -- with the controller of each pawn it
removes. The factory is gone before its state code ever builds the scenes, so no cutscene starts at all. The
crowd in the stands (`HPGenericStudent`) stays, and so do the two teams' placed players -- the match borrows
those (below) rather than destroying them.

## The two teams

`HP3_Quidditch` stands fifteen Quidditch players on the turf: eight Gryffindor (`QuidPlayerGriffMale0..7`,
with `QuidPlayerGriffFem1` among them) and seven Slytherin (`QuidPlayerSlyMale0..7`). A match **borrows**
them: `quidditch::team::start` records each one's `Location`, `Rotation`, `Physics` and `bHidden`, puts it in
the air on `PHYS_FLYING` with no world collision, plays its `fly_forward` animation, and from then on writes
its `Location` and `Rotation` from an agent every frame (in `glue::tick`, which runs after the actors have
moved, so the match's writes are the ones the frame is drawn from). `quidditch::team::stop` sets them all
back down exactly as they stood. The thirteen agents (see below) do not match the fifteen placed players in
mix, so a side that runs short has the rest spawned as `HGame.QuidPlayer{Griff,Sly}{Male,Fem}` and those
belong to the match; whoever is left over on the turf is hidden for the match and shown again by `stop`.

**The agents** (`quidditch::ai`, pure) are the standard seven a side less Harry, who is the home Seeker and is
the player: three Chasers, two Beaters and a Keeper for Gryffindor, the same plus a Seeker for the opponent --
thirteen in all. Each one is a `BroomState` flown by the same `flight::step` the player gets, steered by a
small state machine: a Chaser shoots when a hoop of the far goal is in front of him, within 2600 uu and
straight enough on, else passes to an unmarked Chaser nearer the goal, else carries the ball at the goal;
without it he goes for a loose Quaffle if he is the nearest Chaser, presses whoever of the other side has it,
or takes up his lane. A Keeper flies the line from the Quaffle to his own hoops at a 700 uu stand-off and
never leaves a 1800 uu band around them (a broom cannot stop, so on his line he flies a tight circle instead
of hovering). A Beater swats the Bludger that is on its way to one of his own at the nearest opponent
(`Bludgers::redirect`). The opposing Seeker chases the snitch after eight seconds, for show: only Harry's
catch ends a match.

**Possession.** A free Quaffle inside 250 uu of a Chaser is caught by the nearest one, but never while the
1.5 s goal latch runs or within 0.4 s of a throw (`THROW_GRACE`, so a ball gets out of the thrower's hands).
Its holder carries it; a pass flies it at a team-mate and a shot at one of the three hoops -- the least
defended, with a toss between equals. After a goal the ball goes back to the middle of the pitch and **the
side that conceded restarts with it**, as the real game does: the pitch is symmetric, so without that rule
every loose ball would go to whichever side happened to be a hair nearer and one team could take them all.

**What a frame costs.** `lod::Budget::max_active_ai` is how many agents get the full `step_agent`; the rest
are steered cheaply (`drift_agent`: fly to your zone, and look at nothing else). The default budget is 14, so
all thirteen are fully steered on the desktop, and `BudgetGovernor` lowers it when frames go over 16.6 ms.
Measured headless on the owner's PC, thirteen fully steered agents plus the thirteen riders' `Location`
writes cost **0.007 ms a tick** (`HP3_QUID_DEBUG` prints the running mean every 120 ticks).

## The Golden Snitch

`quidditch::snitch` is pure and deterministic from the match seed. The Snitch flies toward random waypoints
inside the flight volume at 375 uu/s, 875 uu/s when Harry is right on it (the owner asked for "snitch a little
bit faster", x1.25 on both, against a broom that lost a quarter), and **corkscrews as it goes**, the way HP1's
Snitch does so the player can pick it out of the sky: a circle of `SnitchTuning::circle_radius` = 150 uu around
the path point, in the plane across the direction of travel, turning at `circle_rate` = 4.5 rad/s (about 0.7 of
a loop a second). The path speed is untouched -- the loop is an offset around the path, not extra ground
covered -- and the path is flown inside the flight volume brought in by the radius, so a looping Snitch never
leaves the arena and the loop is never cut short.

`Snitch::pos()` is the real, circling position, and that is what the VM's Snitch actor follows, what the catch
progress is measured from and what the catch radius is tested against: nothing works off the hidden path. The
one thing that moved with the circling is `catch_radius`, from 350 to **500 uu** = 350 + `circle_radius`, so a
Snitch looping 150 uu away from where it would otherwise be is exactly as hard to catch as before. Nothing else
was widened: the progress radii are 600/1500/4000 uu and 150 uu of wobble in them is noise.

### The flight path: a heading that turns, not a line that bends

The path the Snitch flies -- the line the corkscrew is wound around and the line the flight rings are dropped
on -- is **not** the straight run from waypoint to waypoint it was at first. It used to reach a waypoint and
set off for the next one in the same frame, which put a corner of up to 180 degrees in the path, and the ring
chain dropped on it was unreadable (the owner: "the path is NOT OBVIOUS and can be jerky").

It is now a **heading-limited pursuit**: the Snitch carries a heading, the heading turns toward where the
steering wants to go at a bounded rate, and the path walks `speed * dt` along the heading. Nothing else
changed: the speed rules, the corkscrew (whose axis now follows the smoothed heading), `pos()`, the catch and
the progress are all as they were.

A steering model, not a spline through the waypoints, because the turn limit is then a property of the path
itself rather than of how the spline happens to be fitted: whatever the waypoints do -- including one chosen
directly behind -- the heading cannot move faster than the limit, so **no input can put a kink in the path**.
A Catmull-Rom through random waypoints would need its own arc-length parameterisation and still bends as
tightly as three close waypoints ask it to.

| number (`SnitchTuning`) | value | why |
|---|---|---|
| `path_turn_rate` | 0.58 rad/s at `SPEED_NEAR` | really a **curvature**, `path_turn_rate / SPEED_NEAR` = rad per uu flown, so the bend is the same at either speed and the turn between two rings a fixed distance apart does not depend on how fast the Snitch is going. The turn radius is `SPEED_NEAR / path_turn_rate` = **1508 uu**, and 600 uu of ring `spacing` over it is 0.40 rad = 23 degrees. Not wider: the arena is 7060 uu across the short way once the margins are off, and much over 1500 is a Snitch that cannot cross it without hugging a wall |
| `max_pitch` | 0.35 rad (20 degrees) | a Snitch standing on its tail needs the whole height of the arena to level off, and the chase camera loses it |
| `path_margin` | 220 uu | room kept between the path and the arena wall on top of `circle_radius`, so a flight ring dropped on the path has its whole rim inside the arena |
| `path_capture` | 600 uu | how near a waypoint counts as reached. A waypoint is also given up on the moment it falls behind, which is what stops a Snitch whose turn circle is wider than this from orbiting it for ever |
| `path_min_hop` | 2500 uu | how far off the next waypoint must be, so each leg is a long arc and not a jink |
| `path_cone` | pi/3 (+-60 degrees) | the forward cone the next waypoint is looked for in. Eight candidates are drawn (always all eight, so the randomness a waypoint costs never depends on which was taken) and the first both far enough off and inside the cone is taken; with nothing in the cone, the farthest-forward one |

**Walls.** The path keeps itself off the arena wall by steering, never by a clamp -- the clamp is still there
as a backstop but a test asserts it never binds, because a clamped frame covers less than `speed * dt` and so
drops rings closer together in space and further apart in heading, which is a kink. Each of the four side
walls gets a *hurry*: the ground the path will make good toward it before the nose comes across it
(`R (1 - sin phi)` turning away from it, `R (1 + sin phi)` turning toward it, for a nose `phi` off the wall)
against the room that is left. The worst of the four, blended in from a fifth of the room to nearly half, is
how far the aim leans off the waypoint; the lean is 40 degrees short of straight out, so the path runs along
the arena instead of back across the middle of it. The way out is the four hurries summed, so a corner is
left by its diagonal. The floor and the ceiling do the same to the pitch. Waypoints are drawn from the box
brought in by `wall_hold` (a fifth of a turn radius) so the steering is not forever pulled at a wall it then
has to lean out of, and a new Snitch starts in the middle of the box with two turn radii of room, because a
heading-limited path dropped next to a wall pointing at it cannot get away.

Three things here are not the obvious thing and each was a bug first:

* the turn is **in yaw and pitch**, not a rotation about `heading x aim`. The shortest great circle between
  two headings far apart in yaw and both near the pitch cap runs up over the pole, so nearly the whole of a
  step went into a climb the cap took straight back out, and the Snitch flew into the wall it was turning
  away from with its nose swinging a degree a second.
* **which way round** the yaw turns is chosen by comparing the two hurries, not by which way is nearer. A
  wall-at-a-time rule cannot turn out of a corner (each of its walls says to turn into the other), and the
  summed way out flips sides from frame to frame when the Snitch points nearly straight at a wall -- a
  rate-limited heading handed first one and then the other averages them and goes nowhere.
* the way round is **kept** unless the other is clearly better (within 1.3x), for the same reason.

### The Snitch's ring trail (HP1's hoop trail)

HP1's Snitch optionally leaves a trail of flight hoops behind it (`BroomHoopTrail`, with `HoopTrailLen` and
`fHoopSpacing`, the hoops visible or invisible by stage). Its Seeker game has three modes: plain proximity;
proximity with the hoops shown as guides; and `PM_Hoops`, in which the Snitch is caught only by flying
through N hoops **in a row**. Ours is the guide mode: the rings pay into the catch bar and are never a
requirement (`catch_rings` = 0, the owner's word after playing). The `PM_Hoops` rule is still in the code and
is one number away -- set `catch_rings` and the catch window waits for the run.

`quidditch::trail` is the pure half (no VM) and `quidditch::rings` the actors' half.

| number (`TrailTuning`, all placeholders) | value | why |
|---|---|---|
| `spacing` | 600 uu | close enough that six of them read as one road rather than six separate targets (it was 900), and short enough against the path's 1508 uu turn radius that the turn from one ring to the next is 0.40 rad = 23 degrees, inside `CHAIN_MAX_TURN` |
| `max_rings` | 6 | 3600 uu of chain; enough for a run of three, few enough not to fill the sky (or the phone's frame) -- and the particle cost is `max_rings` x 96 sprites |
| `radius` | 220 uu | nearly twice a goal hoop's 120: it has to be fair to fly through at 450 (900 boosted) uu/s |
| `life` | 6 s | a ring that nobody went for fades away without a word. 6 s x the chased Snitch's 875 uu/s over a 600 uu `spacing` is 7.5 rings, so in a chase it is `max_rings` that decides and the full six are always up; a Snitch left alone (375 uu/s) shows about three |
| `miss_behind` / `miss_near` | 300 / 900 uu | a ring counts as gone past only when Harry is behind its plane **and** was near it; one on the far side of the pitch is one he never went for |
| `ring_bonus` | +0.06 of the bar | about four rings' worth of the proximity gain (`gain_rate` 0.25/s) |
| `streak_len` / `streak_bonus` | 3 / +0.15 | the bigger bonus on every third ring in a row, on top of the ring's own |
| `catch_rings` | 0 | the rings are a guide and a bonus by default (the owner's word); set it to 3 for HP1's `PM_Hoops` rule, no catch window until three in a row |

A ring is dropped **on the Snitch's flight path** (`Snitch::path_pos`), not where the Snitch is really seen.
Dropping them at the circling position was tried and rendered: the loop is 150 uu wide, so consecutive rings
sit on alternate sides of the path and the trail reads as a cloud of beads instead of a row of hoops. The
ring's normal is the **tangent of the smoothed path** there (`Snitch::travel_dir`), so each ring faces along
the curve and stands a bounded angle from the one before it. Every centre comes out of the Snitch, which
flies inside the flight volume, so the trail is bounded by it. A ring is dropped every `spacing` of path
*length*, and the overshoot is carried over rather than thrown away, so the gaps stay a `spacing` apart to
within a frame's flying.

**The chain is smooth, and it is checked.** `trail::chain_is_smooth` (and `chain_metrics`) is the measure:
over consecutive rings, the turn from one ring's normal to the next must be at most `CHAIN_MAX_TURN` (0.436
rad, 25 degrees) and the gap between centres must be within 0.7..1.3 of a `spacing`. It is asserted every
step of a 20 000-step chase for each of fifty seeds. The guarantee comes from the path, not from the trail:
the Snitch's flight path bends at no more than `path_turn_rate / SPEED_NEAR` rad per uu flown (see the next
section), so `spacing` uu of path can turn by at most `spacing / 1508` = 0.40 rad. Measured over fifty seeds:
worst turn **23.2 degrees**, gaps **585..610 uu** of 600.

Before the smoothed path -- when the Snitch flew straight lines between random waypoints and turned on a
sixpence at each one -- the same measure over the same fifty seeds gave a worst turn of **179.1 degrees** and
gaps of **0..913 uu** of 900: rings landing on top of each other and the next one pointing back the way it
came. That is what the owner saw as "not obvious and jerky".

A ring is flown through **either way round** -- `goals::disc_crossing`, the same disc test a goal uses without
the direction -- and counts once. A run is reset by going past a ring, and costs nothing else. A full bar with
no run waits instead of opening the catch window, so a missing run never spends one of the three tries; the
HUD's `RINGS n/3` next to the catch bar says what is still wanted, and the commentary says so when the run is
in. A pass-through plays one of seven broom swishes (`Q_BRM_Swish_04`/`_06`, `broomwhoosh01..05`): HP3 ships no
hoop sound at all, where HP1 picks one of fifteen `Q_Through_Hoop` samples.

**How a ring is drawn: the real Buckbeak flight ring.** A trail ring is a real `HGame.HippoRing`, the ring the
hippogriff ride is flown through (about 110 of them in `HP3_COMC`, more in `HP3_Adv6Paddock`).

What the retail ring actually is, read out of the packages (`hp3-pkg dump`/`disasm`), off a live level
(`HP3_PARTICLE_DEBUG=1 HP3_PARTICLE_DEBUG_NAME=HippoRingEmitter hp3-run level ... HP3_COMC`) and off a render
of `HP3_COMC` (`hp3-view play` with `HP3_VIEW_CAM` parked in front of `HippoRing87`):

* the actor **carries no mesh**. `HippoRing.TurnOn(path)` spawns its `ringFXClass` -- which is
  `HGame.HippoRingEmitter` -- at its own `Location`, then calls `SetRadius(CollisionWidth)` and
  `SetColor(ringColorStart, ringColorEnd)` on it and goes to `StateActive`. **Nothing is drawn before
  `TurnOn`**; the map's rings are turned on by their `HippoPathManager`.
* `HippoRingEmitter` is three `SpriteEmitter`s of 32 particles each, 96 in all, textured
  `HP3_FX.Bats.BatFlap1_A00` / `2` / `3` -- the three flap frames of the bat sprite, which is what makes a ring
  shimmer instead of sitting still. Alpha-blended, 18..28 uu a sprite, 2 s each and respawned, drifting only
  +-8 uu/s, opacity 0.9, `CoordinateSystem` **relative**, so the sprites ride the actor.
* the circle itself is `StartLocationShape` = polar with `StartLocationPolarRange`, whose **radius is 0 in the
  defaults**: `SetRadius` is the thing that opens it out, and the actor's rotation turns the circle so it
  stands across the flight. Retail rings are 192 uu (`CollisionWidth` = `CollisionHeight`) and white
  (255, 255, 255); `fRingLength` is 0, so the ring is flat and not a tube.

**Why the first attempt came out as a swarm of bats**: it spawned a bare `HGame.HippoRingEmitter` and left it
alone. With no `HippoRing` to call `SetRadius`, the polar radius stays 0, so all 96 sprites are born on top of
each other at the actor and only the +-8 uu/s drift moves them -- a cloud, not a ring. The bats were never the
mistake; the retail ring really is made of bat flappers. The missing `SetRadius` (and a rotation) was.

So `quidditch::rings` spawns one `HGame.HippoRing` per ring slot (six), hidden and off, with
`CollisionWidth` = `CollisionHeight` = `TrailTuning::radius` (220 uu, a little wider than retail's 192 so it
stays fair to fly through) and `fRingLength` 0. Re-aiming a slot is a `Location` write, a `Rotation` whose
forward axis is the ring's normal, `Setup` (the ring re-reads its own axes) and another `TurnOn`, which
destroys the slot's old emitter and spawns a fresh one in the new place -- about one spawn a second over the
whole trail, which is what the retail ride itself does when a path is switched. Collision is switched off
after every `TurnOn` (`TurnOn` turns it on, and the retail `Touch` wants a hippogriff rider to hand a speed
boost to); the disc test in `trail` is the only thing that decides a pass-through. `rings::stop`, called from
`glue::stop`, `TurnOff`s each ring and destroys the emitter it is holding -- the emitter is spawned by the
ring's own script and is not in the match's `actors`, so nothing else would.

**Ours are gold, not white** (`rings::RING_RGB` = 255, 220, 40; the owner's word after playing: a white ring
is lost in the pitch's fog). It is written twice over, because the two paths reach the sprites differently:

* the ring actor's `ringColorStart` and `ringColorEnd`, which is what `TurnOn` hands the emitter's
  `SetColor`; and
* each of the emitter's three `SpriteEmitter`s' **`ColorMultiplierRange`**, written by `rings::tint` right
  after the `TurnOn`. That is the one the renderer reads: `particles::collect` multiplies the white bat-flap
  texture by it per channel, so (1.0, 0.863, 0.157) is a plain tint. It must be written in the same frame as
  the `TurnOn` -- `particles::tick` runs **before** `quidditch::glue::tick`, so an emitter spawned here has
  its config read, and its first particles born, a frame later. Set it late and every ring would flash white
  for a frame.

A multiplier must stay in 0..1: the particle code reads a channel above 1 as byte values for a green glow
(see `HP3_RAW_COLOR_MULT`), which would turn the ring green.

**What it costs**: six rings x 96 sprites = **576 particles** at most, all single quads. That is the number to
cut first if the phone's frame goes over (fewer rings, or a smaller `max_rings`). The sprites were not made
bigger or more numerous for the fog: the colour alone was enough in a render.

Still open, if the rule is ever wanted in its HP1 form: `PM_Hoops` proper, where the rings are the *only* way
to the Snitch and the proximity bar goes away.

As a backstop, `count_down_travel` refuses `Level.NextURL` for as long as `World::quidditch` is `Some`, clears
it and logs `quidditch: level travel blocked (<url>)` once per level. A level with no match is untouched by
either: both are gated on a running match.

## Environment variables (debugging only; nothing in the game sets them)

- `HP3_MATCH=<house>` — start a Quidditch match against that house as soon as the level is up.
- `HP3_MATCH_SEED=<n>` — the match's randomness. Without it the seed comes from the opponent's name, so a
  given opponent always plays the same match.
- `HP3_MATCH_RESTART=<tick>` — stop and start the match again at that tick (to exercise the hand-over).
- `HP3_QUID_DEBUG=1` — also: which of the level's players each side's riders are, the first rider's whole
  animation set, every pass, shot, catch and Bludger swat as it happens, and the mean cost of a tick of team
  play every 120 ticks. And every 120 ticks, a sample of the match: phase, the Snitch's pure, actor, path and
  loop-offset positions (the last two show the corkscrew),
  the catch progress, the Bludger bumps, the HUD item list, the Bludgers in flight, and the broom's position,
  speed, heading, look direction and camera (with its arm and its field of view). Set it to a number (`HP3_QUID_DEBUG=5`) to sample
  every that many ticks instead, which is how a headless run watches the steering turn frame by frame.
- `HP3_QUID_SHOOT=<tick>` — **off by default.** At that tick of the match the Quaffle is shot from the middle
  of the pitch at a hoop, from outside the Chasers' play. It is how a headless run proves that goals are
  detected and counted by themselves: every goal also prints a
  `quidditch: GOAL for <side> through the hoop at (...)` line. Without it the Chasers are the only ones who
  throw the ball.
- `HP3_QUID_SHOOT_HOOP=<0..5>` — which hoop that shot goes through, in the order of `arena::hoops()`: 0, 1, 2
  are the north goal's low, high and mid hoops (a goal for Gryffindor, 10-0) and 3, 4, 5 the south goal's (a
  goal for the opponent, 0-10). Default 0.
- `HP3_QUID_START=<x,y,z>` — **off by default.** Where Harry's broom starts instead of his own goal line,
  brought inside the flight volume. It exists so a headless frame can be rendered from somewhere other than
  the kickoff -- `HP3_QUID_START=256,2300,760` puts the chase camera in front of the north goal's three hoops.
- `HP3_QUID_RING_AUTOFLY=1` — **off by default.** The broom flies itself from flight ring to flight ring of
  the Snitch's trail (`flight::look_at`, the same scripted pilot `quid-smoke` uses), so a headless run proves
  a pass-through, its streak and its bonus with nobody at the keyboard. It chases rings and not the Snitch, so
  such a run ends by running out of catch tries, not by a catch.
- `HP3_QUID_EASY=1` — **off by default.** The Snitch's catch bar fills from anywhere on the pitch, the catch
  window is wide open and the catch key is held for Harry, so a headless run reaches a caught Snitch (and the
  result line) in about 12 seconds with nobody at the keyboard. Only for tests; it makes the game trivial.

## The match's sounds and music

All in `soundeffects.Quidditch` (verified present in the retail packages, Task 8): `flying_broom_loop` (on
Harry, its volume and pitch following his speed), `Q_snitch_loop` (on the Snitch), `AMB_crowd_loop1` (on a
silent actor at the pitch centre), `bludger_loop` (on each Bludger ball in flight), the one-shots
`snitch_flyby01` and `bludger_flyby01`, and the crowd reactions under the commentary lines
(`SS_QUID_Bigcheers_0001`, `SS_QUID_Crowd_roar_01`, `SS_QUID_Big_cheer_01`, `SS_QUID_boo_long_01`). The music
is `SM_QUD_SnitchOrch_01`. HP3 ships no commentary speech we could confirm, so the commentary is on-screen
text with those crowd reactions under it.

HP3 has no Bludger (or Quaffle) mesh, so a Bludger is `HP3_InteriorAccents_SM.miscDetails.MagicalOrb_SM`
scaled by its own bounds to a 15 uu radius (30 uu across, hit radius 55 with Harry's body; the Quaffle is the same size) and skinned with the retail `HP_FX.General.Bludger_Halo` glow.
