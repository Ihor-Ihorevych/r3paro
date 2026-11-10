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
face along Y (towards the pitch centre). World hoop centres, Z = -960 + local z:

| goal | hoop | centre (x, y, z) |
|---|---|---|
| north (y = +4096, yaw +90) | low | (-354, 4096, 158) |
| north | high | (256, 4096, 1445) |
| north | mid | (866, 4096, 745) |
| south (y = -4096, yaw -90) | low | (866, -4096, 158) |
| south | high | (256, -4096, 1445) |
| south | mid | (-354, -4096, 745) |

Hoop radius 120. Posts' tops: world z 1565 at most.

## Chosen arena constants (for Task 9)

- Ground level z = -960 (goal post base and player starts at -963..-835).
- Pitch centre (x, y) = (272, 116).
- Playfield half-extents: x 3900 (matches the flat `Quttish_Pitch_SM` half-width 3905), y 4600 (goals at +-4096 plus 500
  behind them). The pitch mesh itself reaches x +-4283, y +-6630 (stands included), so the playfield sits inside it.
- Ceiling z = 2300 (stand top, world 2308.7; tallest hoop top is 1565).
- Hoops: the six world centres above, radius 120 (150 forgiving).
- Towers: not in the retail map. Provisional tower slots at the stand perimeter, on the stairs ring: the seven
  `Q_stairs_SM` placements (z 824.8) at (4026, 2925), (2364, 5881), (3329, -4334), (-1813, 5889), (-2787, -4327),
  (4286, -809) and (-3733, 1038) mark where the stands sit; `Quid_Tower_SM` (footprint 532 x 563, height 2963, local
  origin near its centre) can be placed at the four pitch corners (+-4400, +-6000) relative to the centre if wanted.
