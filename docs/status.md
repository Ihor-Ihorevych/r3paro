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
  (Entry has none, as it should), and 2 seconds of play raise no script errors. The natives still hit need
  the systems that come next: pathfinding (`FindPathToward`, `actorReachable`), traces, bone coordinates,
  `SetOpacity`, `FinishRotation`, `Volume.Encompasses`. Sound, animation, collision and movement natives are
  placeholders.
- Known issue: the interpreter runs at about 0.5 µs per statement; Adv1 needs about 9 ms per tick, mostly in
  the ambient creatures' scripts. A performance pass is planned (fewer value copies, faster property access).
- **M5 Playable render, started**: `hp3 <HP3 folder> [map]` opens an SDL3 window and draws the level with
  wgpu while its scripts run at 60 Hz. The camera flies freely (WASD, mouse, Space/Ctrl, Shift); the
  player's own view, movement and the dynamic actors are next. Building it needs CMake (Visual Studio Build
  Tools ship one) because SDL3 is compiled from source.
- M6 onwards: not started.

## Checking

```sh
cargo run --release -p hp3-pkg -- check <HP3 folder>
HP3_DIR=<HP3 folder> cargo test --release
cargo run --release -p hp3-run -- levels <HP3 folder>
```
