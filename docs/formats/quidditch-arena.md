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
