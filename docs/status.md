# Status

Milestones are from the plan (M0 to M12).

- **M0 Foundations**: workspace, crate skeletons, clean-room rules, Windows MSVC and Android arm64 toolchains.
- **M1 Packages: done.** `hp3-pkg check <HP3 folder>` reads all 159,093 exports of the 110 packages of a retail
  install exactly (0 errors, 0 leftover bytes, under a second), including all 10,452 scripts. Formats:
  `docs/formats/packages.md`.
- **M2 Assets**: decoders for every native asset class exist (`hp3-assets`). `hp3-pkg textures` converts
  textures (DXT1/3/5, P8, RGB/RGBA, L8, G16) to PNG. `hp3-view shot <HP3 folder> <map> <out.png>` renders a map
  headless with wgpu: BSP render sections and static mesh actors, fullbright. Not yet: lightmaps, terrain,
  skyboxes, skeletal meshes, emitters, translucency.
- **M3 Script VM**: `hp3-pkg natives` lists the native functions the scripts declare: 499 in all (Core 179,
  Engine 279, GUI 24, Editor 14, kwGame 3: `KWBeamSegment.UpdateChain`, `PersistentStore.FindSlot`,
  `PersistentStore.CreateSlot`). HGame and the other game packages declare none. `hp3-pkg disasm` prints a
  class's bytecode. `hp3-run classes` loads all 1,685 classes with their properties and builds every default
  object (12,446 default tags, all decoded); `hp3-run instances` builds the property values of all 68,554
  scripted objects placed in the 34 maps (986,581 tags, all decoded, every object reference resolved).
  Next: the interpreter.
- **M3 Script VM**: an interpreter over compiled expression trees (locals, out parameters, switch,
  short-circuit operators, struct and array access, casts, virtual/final/native/delegate calls, `new`,
  `foreach`, states with labels, latent `Sleep`). Every one of the 67,804 statements in the game's scripts
  compiles; no opcode is unsupported. Core's natives are implemented (operators, math, strings, vectors,
  rotators, states).
- **M4 World boot, mostly**: `hp3-run level <HP3 folder> <map> [ticks]` loads a map into the VM, spawns the
  game class from the config (`HGame.HGame`), runs begin-play and the player login, and ticks events, timers
  and state code. `hp3-run levels <HP3 folder>` does this for every map: all 34 load, 33 log in a player
  (Entry has none, as it should), and play raises no script errors. Sound natives are still placeholders.
- Performance: the interpreter runs at about 0.2 to 0.5 µs per statement; on a laptop Adv1 needs about 3 ms
  per tick and HP3_InsideHub, the largest map, about 5.5 ms (it was 160 ms: radius queries and touch
  tracking use spatial grids, calls cache their function information, per-tick loops read Actor slots
  directly, walkers at rest skip their floor probes). `HP3_TICK_PROFILE=1` times each tick phase. Runs are deterministic: hash maps use a fixed hash, so
  timers, latent actions and events are processed in the same order every time.
- **M5 Playable render, in progress**: `hp3 <HP3 folder> [map]` opens an SDL3 window and draws the level with
  wgpu while its scripts run at 60 Hz. Keyboard and mouse go through the game's key bindings; the view is the
  game's own `PlayerCalcView`, or a camera behind the pawn while the game's camera is not running (a map loaded
  directly skips the menu and New Game flow that starts it). F1 toggles a free camera. Characters and other
  live actors are drawn with their meshes and skins, skinned on the CPU each frame from their animation;
  static actors are baked into the level geometry. `hp3-view play <HP3 folder> <map> <out.png>` renders the
  same headless. Static lighting: BSP lightmaps are built at load from the map's lights and their stored shadow
  bitmaps plus each zone's ambient light (`AmbientVector`); static meshes use their editor-baked vertex colors
  (`StaticMeshInstance`, shadows included) plus ambient, or computed vertex light when they have none,
  and live actors as a whole by the lights around them. The light model is an approximation, not yet matched
  to HP3's renderer; `HP3_UNLIT=1` turns lighting off. Terrain: every layer of each terrain, textured by its
  stored `TerrainMatrix` and blended over the ones below by its alpha map, lit per vertex, with holes from
  `QuadVisibilityBitmap`. Sky: the level is first drawn from the `SkyZoneInfo` with the camera's rotation
  (the whole level for now, to be culled to the sky zone), then the scene over it. Not yet: emitters,
  translucency, fog, dynamic lights, deco layers, HUD.
- **M6 Movement, started**: swept-box collision against the level BSP, static mesh and terrain triangles and
  blocking actors; walking (friction, step-up of 24, sliding, floor snapping), falling and landing, flying,
  projectiles; touch events from collision cylinders; `Trace`, `FastTrace`, `TraceActors`. Holding W walks
  Harry until the wall stops him at the distance a trace measures. Movement values are not yet matched to the
  retail game.
- **Animation (Phase 8), started**: skeletal poses sampled from HP3's animation sets (`hp3-anim`), one channel
  per actor with looping, rate, `AnimEnd` and `FinishAnim`; pawns pick run/walk/idle animations from their
  velocity and call the scripts' `PlayWaiting`/`PlayFalling`; bone attachments (`AttachToBone`, used for
  students' hair and props), `GetBoneCoords`, `GetBoneRotation`, `LinkMesh`. Not yet: blending between
  channels, tweening, notifies, root motion, bone controllers. `hp3-view pose` renders one mesh at a sequence
  and frame.
- Script objects: package objects (meshes, materials, sounds, notifies) are now loaded the first time a script
  uses them, and native-only classes (`Mesh`, `SkeletalMesh`, `Sound`...) exist as classes, so casts and
  `DynamicLoadObject(..., class'Mesh')` work.
- **Navigation, started**: the map's path network (navigation points and reach specs), route finding
  (`FindPathToward`, `FindPathTo`, `RouteCache`), reachability (`actorReachable`, `pointReachable`) and latent
  AI moves (`MoveTo`, `MoveToward`). Reachability is a straight line test for now (no jumps or drops).
- `Volume.Encompasses` tests the point against the volume brush's BSP. `SetOpacity` is recorded for the
  renderer; emitters are not simulated yet (`ParticleEmitter.Trigger` does nothing).
- All 34 maps run 600 ticks with no script errors and no unimplemented natives reached.
- **Config and arrival**: `config` class properties are read from the ini files (each class's own section,
  parents first). A map loaded directly arrives as if from a neighbouring level: the pawn takes the
  `PreviousLevelName` of the map's first `SmartStart` (or `HP3_FROM=<level>`; `HP3_FROM=none` for a bare
  start), so the game places Harry, attaches Ron and Hermione as followers and starts its own camera, which
  takes the pawn's camera settings when none were carried over. Ticked actors flip `bTicked` each frame
  (HP3's camera computes its view once per frame on that).
- **HUD**: each frame the HUD's `PostRender` draws through the `Canvas` natives (tiles, text with HP3's fonts,
  sizes, wrapping) into 2D quads drawn over the scene with Unreal's blend styles. HP3's HUD items rest hidden
  in their `Idle` state until the game fades them in (`HP3_SHOW_HUD=1` in `hp3-view play` shows them all).
  Not yet: menus (GUI), native TTF fonts, 3D actors in the HUD.
- Fixed: auto states are flagged 0x2 (0x1 is "editable"); every actor now starts in its class's auto state.
- M7 onwards: not started.

## Checking

```sh
cargo run --release -p hp3-pkg -- check <HP3 folder>
HP3_DIR=<HP3 folder> cargo test --release
cargo run --release -p hp3-run -- levels <HP3 folder>
```
