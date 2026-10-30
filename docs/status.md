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
- M4 onwards: not started.

## Checking

```sh
cargo run --release -p hp3-pkg -- check <HP3 folder>
HP3_DIR=<HP3 folder> cargo test --release -p hp3-pkg
```
