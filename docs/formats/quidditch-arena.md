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

Harry's broom is **not** in this package and is **not** called `QuidBroom_SM`: that string is in the name table of
`StaticMeshes/HP3_InteriorAccents_SM.usx` but no static mesh carries it. The mesh is
`HP3_InteriorAccents_SM.miscDetails.QuiddichBroom_SM`, and the retail `HarryQuid` skeleton has a bone named
`body_broom` (its last bone, after the hand and finger bones) to hang it from.

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

Because there is no ring mesh at all (and the retail game's own rings are flat black outlines over the pole
quads), `quidditch::ball` draws each ring as **twelve small round static meshes of radius 32 around the hoop's
rim** -- 72 in all, spawned `bStatic` so they are never ticked, and destroyed with the rest of the match's
actors. The retail quads are left exactly where they are. The Quaffle is the same stand-in mesh at radius 60.
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

**Speed is untouched**: cruise 600, boost 1200, brake 150, brake beats boost (the mod changes no speed either;
our halved numbers are the playtest's).

The soft boundary (`flight::boundary_turn`) now steers the *look* back over the pitch, not the broom's
heading, and `flight::confine` levels the look with the broom when it slides along the floor or the ceiling.

**The camera is an orbit camera, and the broom follows it.** `camera::chase_camera` aims along the rider's
look direction, not along the broom's heading (so steering is camera-relative, as in the mod), with its own
lagged rotation in both axes: 8/s towards the look, at most 1.917 rad of yaw and 0.959 rad of pitch in one
step, pitch clamped to +-1.342 rad. It sits on a 128 uu arm (`ARM`) from a point 55 uu over the rider
(`LOOK_UP`) -- the mod's numbers, and much closer than the 420/140 rig it replaces -- with no positional lag
at all. Looking up from below pulls the arm in by `1 - pitch/0.9587`, immediately, and it eases back out at
2.5/s. The arm is also shortened so the camera stops 5 uu short of the arena wall instead of hanging in the
stands (the mod traces the world; the flight volume is all the collision a match has).

Two deliberate differences from the mod, both because its numbers put the rider out of the picture at our
scale: the pull-in scales the **whole rig** (the 55 uu look-at offset shrinks with the arm), so the rider
keeps his place in the frame however far the camera is pulled in, and the pull-in floor is **0.3**, not the
mod's 0.15, which would end with the camera inside him. The field of view still widens with speed (90 -> 100
deg); that is ours, neither HP1 package has it.

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
  play every 120 ticks. And every 120 ticks, a sample of the match: phase, the Snitch's pure and actor positions,
  the catch progress, the Bludger bumps, the HUD item list, the Bludgers in flight, and the broom's position,
  speed, heading, look direction and camera (with its arm). Set it to a number (`HP3_QUID_DEBUG=5`) to sample
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
