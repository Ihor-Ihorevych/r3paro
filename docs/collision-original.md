# How HP3 decides collision (read from the game's own Engine.dll)

This is the spec the collision rework follows. Every rule below was read from HP3's `System/Engine.dll`. The DLL is
disassembled with dumpbin; exports give each function's thunk, and the `jmp` gives its body. Addresses are VAs with the
DLL's base at 0x10300000. The UT2004 source in `flipendo-droid-2004` is only a map for reading the DLL. Where HP3 differs
from it, this file says so. Reimplement the rules; do not transcribe them.

Field offsets were worked out from `hp3-headers/EngineClasses.h` and confirmed by the code that reads them.

- **Actor flags word at `+0x2EC`**, bits in order: SavedBlocksCamera, SavedProjTarget, **CollideActors (b2), CollideWorld
  (b3), BlockActors (b4), BlockPlayers (b5), ProjTarget (b6)**, BlockZeroExtentTraces (b7), BlockNonZeroExtentTraces (b8),
  AutoAlignToTerrain, **UseCylinderCollision (b10)**, BlockKarma, … JustTeleported (b18).
- **`+0x70`**: b27 WorldGeometry, b28 AcceptsProjectors, b30 OnlyAffectPawns. **`+0x78`**: b0 IgnoreEncroachers.
- **Other actor fields**: `+0x41` Physics, `+0x44` StaticMesh, `+0x4C` Base, `+0xB8` Mesh, `+0x264` Brush,
  `+0x2DC/2E0/2E4` CollisionRadius/Width/Height, `+0x2E8` CollideType.
- **Pawn fields**: `+0x3F8` Controller, `+0x470` GroundSpeed, `+0x480` AccelRate, `+0x488` AirControl, **`+0x4A4`
  fMinFloorZ**, `+0x628/62C` StepHeight/StepWidth, `+0x630` MaxMountHeight, `+0x634` bUseStaticMeshSizeForMount,
  `+0x638/63C` GroundFrictionOverride/bOverride…, **`+0x648` MaxStepHeight**.

## 1. Who blocks whom: `AActor::IsBlockedBy(Other)`, 0x103E2910

"Player" below means `GetPlayerPawn()` is non-null **or** the actor is a projectile (`IsAProjectile`).
`APawn::GetPlayerPawn` (0x10470930) returns the pawn when its Controller is a PlayerController that has a viewport
(`Player`). Any other actor returns null.

In order:

1. If Other is world geometry, the result is `this.bCollideWorld`.
2. If this is an encroacher and Other ignores encroachers, nothing blocks. The same applies in the mirror direction.
3. **If Other is a brush** (`Brush != None && IsABrush`, which includes every volume) **or an encroacher**, the rules are:
   - A moving brush (StaticMesh mover) never blocks a PlayerController itself. Pawns are not affected by this rule.
   - Otherwise it blocks iff `this.bCollideWorld` holds and the matching Other flag is set: `bBlockPlayers` if this is a
     "player", `bBlockActors` if not.
4. The mirror of 3 applies when this is the brush or encroacher.
5. **Otherwise both sides must agree.** The first condition is `this is player ? Other.bBlockPlayers : Other.bBlockActors`.
   The second is `Other is player ? this.bBlockPlayers : this.bBlockActors`.

**HP3 vs UT2004:** UT2004 never reads `bBlockPlayers`. Its default rule is `bBlockActors && bBlockActors`, and it has
vehicle and projectile special cases. HP3 picks Players/Actors per side, and **projectiles (spells) count as players**.

Encroacher (0x1033B550) means `bCollideActors && (StaticMesh mover || Physics == Karma(13, not a constraint) ||
KarmaRagdoll(14))`. Moving brush (0x1033B4F0) means `StaticMesh != None && IsAMover`.

## 2. What a sweep looks at: `ShouldTrace` and the trace flags

The trace flag bits are UT2004's: Pawns 0x1, Movers 0x2, Level 0x4, Volumes 0x8, Others 0x10, OnlyProjActor 0x20,
Blocking 0x40, LevelGeometry 0x80, ShadowCast 0x100.

- **Pawn** (0x10470A10) returns `flags & Pawns`, after the shadow/projector/OnlyAffectPawns early-outs.
- **Mover** (0x10324230) returns `flags & Movers`.
- **Actor** (0x103E2D80):
  - A WorldGeometry actor returns `flags & LevelGeometry` and nothing else.
  - Otherwise Others is required. With OnlyProjActor, the actor needs `bProjTarget || (bBlockActors && bBlockPlayers)`.
  - With Blocking, the result is `Source.IsBlockedBy(this)`. Otherwise it is true.
- **Volume** (0x103E2FB0) follows the Actor rule, keyed on Volumes. WorldGeometry with LevelGeometry is true. There is no
  BlockingVolume override: HP3 has no `bClassBlocker`/`BlockedClasses`.
- **HP3 vs UT2004:** the OnlyProjActor test includes `bBlockActors && bBlockPlayers`. UT2004 uses `bBlockActors` alone.

## 3. Moving an actor: `ULevel::MoveActor`, 0x10431230 (same as UT2004)

- **Sweep conditions.** The sweep runs only when the actor collides, is not an encroacher, and moves.
- **Flags.** `bCollideActors` adds Others | Volumes, plus Pawns unless the caller ignores pawns. `bCollideWorld` adds
  Movers | Level | LevelGeometry.
- **Extent** is `GetCylinderExtent` (0x103218C0): `(R, R, H)`, or `(R, Width (or R when 0), H)` when `CollideType ==
  CT_Box`. The sweep box is always axis-aligned.
- **Probe length.** The probe runs 2 units past the move. The hit loop, which skips bases both ways, takes the first hit
  that `IsBlockedBy` accepts. It runs only when the mover has CollideWorld, BlockActors or **BlockPlayers** (mask 0x38).
- **Stop distance.** The actor stops 2 units short of the contact. Contact closer than 2 units is a zero move, and the
  call fails if the rotation did not change either.
- **After the move**, in order: Bump (not for world geometry, not for the base), touches for non-blocking hits before
  the block, untouch, zone.

## 4. Primitives: what a hit is tested against

`AActor::GetPrimitive` (0x103E27C0) chooses the primitive in this priority order:

1. **The engine's `UCollBox` when `CollideType == CT_Box`.** This beats Mesh and StaticMesh.
2. Mesh.
3. StaticMesh.
4. AntiPortal.
5. The engine's cylinder.

Brushes (volumes and brush movers) use their Brush model.

- **`UCollBox`** (`LineCheck` body at 0x1048DBE0, `PointCheck` at 0x1048D410) is a box with half-sizes
  `(R, Width or R, H)` in the actor's own frame. The ray goes into local space through the actor's world-to-local
  transform, with a slab test against the six faces. **A sweep that starts inside is blocked (time 0) only when the end
  is deeper than the start**, so an actor caught in a box can walk out.
- **Cylinder** (`UPrimitive::LineCheck`, 0x1048CBF0) is UT2004's. The extent is the summed cylinder extent, centred on
  Location (CollisionWidth does not raise it). A sweep that starts inside blocks only when moving toward the axis
  (`dir < -0.1`). Time is pulled back by 0.001.
- **Skeletal mesh** collides as the cylinder unless it has collision boxes, or is a ragdoll (`UseCylinderCollision`,
  0x104FA500).
- **Static mesh** (0x10515C20) follows UT2004:
  - `bUseCylinderCollision` gives the cylinder.
  - Otherwise the simple collision model is used when `(UseSimpleBoxCollision && extent ≠ 0) || (UseSimpleLineCollision &&
    extent == 0)`.
  - Otherwise the collision triangles are used. Time is pulled back by `clamp(0.1, 0.1/len, 4/len)`.
- **Brush model** (`UModel::LineCheck`, 0x10542D60) uses the same constants as UT2004: 0.5 pull-back for rays, 0.1…4 for
  boxes. The BSP solid with leaf hulls is solid from inside. **A box that starts inside a brush is blocked only when moving
  deeper**, and leaving or sliding is allowed.
- **Hit list** (`MultiLineCheck`, 0x10434600): the level first, then terrain. Actors are considered only up to 5 units
  past the world hit (20 past terrain). Hits are sorted by time.

## 5. Pawn movement

- **`physWalking`** (0x10486910), `stepUp` (0x104816B0), `physFalling` (0x1048A930) and `CheckForLedges` (0x1047B930)
  read the pawn's own **`MaxStepHeight`** (`+0x648`) and **`fMinFloorZ`** (`+0x4A4`). No 24/35 or 0.7 constant appears in
  them. Walking also honours `bOverrideGroundFriction`/`GroundFrictionOverride`.
  - Other constants match UT2004: MINFLOORDIST 1.9, MAXFLOORDIST 2.4, 0.05 s sub-steps, friction slide 3.3, step-side Z
    0.08, step retry 144, pushable-decoration -0.9, landing 0.1 / 0.003, air-control slow speed 10.
  - The retail defaults are MaxStepHeight 24 and fMinFloorZ 0.7 for every pawn in Adv6Paddock. Scripts may change them.
- **`APawn::Mount`** (0x10481090) reads MaxMountHeight and bUseStaticMeshSizeForMount. It accepts a wall when
  `-0.1 < normal.z < 0.17`. Its rise must be at least `MaxStepHeight` while walking, and 0 while falling.

## 6. What `hp3-world` follows now (`HP3_COLLISION=old` restores the earlier approximations)

- **Blocking volumes with a convex brush are solid.** That is about 95% of volumes across the 34 maps.
  - A box that starts inside is held only when it would go deeper (`Hull` in `collision.rs`).
  - `find_spot` first pushes out through the nearest face.
  - Non-convex brushes still collide by their faces.
- **Every `BlockingVolume` is baked**, including one that starts switched off, so a trigger can switch it on.
- **Triangles are met by the true box**, through a separating-axis sweep (`sat_sweep`).
  - A box that already overlaps a triangle is retried from 5 units back, as the original's kDOP check does.
  - The old circle footprint let a tall box slip under or over a wall edge it overlapped by less than its radius.
- **A `CollideType` box beats the static mesh** (luggage racks, portrait blockers): the actor collides as its box.

## 7. Still different (the work list)

| Original | `hp3-world` today |
|---|---|
| Non-convex volumes and brush movers are BSP solids | Non-convex volumes are still two-sided faces with the `starts_in_volume` heuristic; brush movers are triangles |
| Static meshes and volumes are in a live octree and follow moves | Baked once at load; only on/off and who-they-block update |
| `IsBlockedBy`: per-side Players/Actors, projectiles count as players, a moving brush never blocks a PlayerController | Only the mover's side is checked; no projectile rule; movers checked with a separate flag |
| `ShouldTrace`: WorldGeometry is only LevelGeometry; BlockZero/NonZeroExtent gate actor sweeps | Movement ignores those flags |
| MoveActor: probe +2, stop 2 short, first blocking hit, touches before it | Shrink/creep/push ladders, 8-direction unwedge, `trace_static` without actors |
| Per-pawn MaxStepHeight and fMinFloorZ | Constants 24 / 0.7 (equal to the retail defaults) |
| One cylinder rule (start inside blocks only when moving inward) | Two different rules (`box_trace` lets every start-inside sweep through) |
