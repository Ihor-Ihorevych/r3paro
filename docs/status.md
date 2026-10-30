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
  Not yet: 3D actors in the HUD.
- **Menus**: the player's viewport objects (`Player`, `InteractionMaster`, console, `HPGUIController`) are made
  at login and the controller initialized. The game starts on its local map with the front end
  (`HP3_MENU=<page>` opens any page). The GUI's native side is reimplemented: automated controls instanced
  from their templates (delegates rebound to the new owner), layout, drawing of pages, images, labels,
  buttons and GFX buttons through styles and fonts, mouse hover and clicks calling the scripts' delegates,
  keys to the page. Languages HP3's bitmap fonts do not cover (Russian...) use system TrueType fonts
  (`SAPFont.<language>`, rasterized with fontdue; `HP3_FONT` overrides the file). Masked palettised textures
  treat index 0 as transparent. The front end and the New Game page work. Not yet: lists, edit boxes,
  sliders, combo boxes, tabs, timers.
- **Level travel**: a level switch the game asks for (`Level.NextURL` once its countdown runs out) loads the
  new map in the same VM (objects outside the level persist), with the URL's options for `InitGame` and
  `Login` and the player pawn's `travel` properties. The desktop app, `hp3-view play` (with `HP3_CLICK`) and
  `hp3-run` follow it: front end, New Game, slot, and the game travels to HP3_Adv1Express.
- Fixed: auto states are flagged 0x2 (0x1 is "editable"); every actor now starts in its class's auto state.
- **Cutscenes (M7, started)**: HP3's cutscene system is script (`KWCutScene`, one `KWCutControllerII` per
  actor thread, commands from `System\Cutscenes\*.int`); the engine side it needed is in place, and the train
  intro plays through to its end and gives control back in the corridor, with Ron and Hermione following.
  That took: localized files in subfolders, text import that reads names as one token and numbers from the
  front (`Howl rate=.8`), `AnimEnd` reaching the pawn's controller, `bMoveToSuccess`, reachability against the
  level's geometry only, movers (keyframe interpolation, riders, moving collision) and the save request's level
  action being cleared. Saving itself is not implemented. No sound or dialogue timing yet: lines wait their
  default time.
  Harry now walks the corridor at full speed and the camera stays inside it. Known issue: the companions do not
  follow properly yet (Ron stands still while Harry runs, and `Run_Back` is looped for a pawn that is not
  moving), so their running animations look wrong.
- Skeletal meshes: a material slot past the mesh's textures takes the last texture (Hermione's hair).
- **Audio (M9, started)**: nearly all sounds are Bink audio (1126 effects, 2596 dialogue lines), a few are WAV,
  music is Ogg Vorbis. `hp3-audio` has a pure-Rust Bink audio decoder (the DCT variant every sound uses; the
  inverse-DFT variant is written but unused by this install; `hp3-pkg sounds <folder> <package> [out]` decodes a
  package and writes WAVs), a mixer (pitch, volume, 3D falloff at 25 units per radius unit, panning, slot
  overriding, music with fades) and Ogg music. The game's `PlaySound`/`PlayOwnedSound`/`StopSound`/`PlayMusic`/
  `StopMusic`/`FindCurrentMusic` natives queue events the app plays through an SDL3 stream; `GetSoundDuration`
  comes from the sound headers, so cutscene lines now last as long as their dialogue (the train intro ends at
  about 100 s). Checked by numbers only (the decoded audio has a speech-like spectrum, the overlap of blocks
  agrees much better with the DCT-III than with a DCT-IV, levels are sane); nobody has listened yet. The output
  gain is 1/frame length (calibrated, not derived). Ambient sounds (`AmbientSound`) loop at their actors while the listener is
  within reach, and the game's `[ALAudio.ALAudioSubsystem]` volumes are used. Not yet: reverb/occlusion, lip-sync from the sound envelopes, music on a thread (a long track delays its start by a
  fraction of a second), Android output.
- **Movies**: this install has no movie files and no Bink video decoder exists yet, so `Movie.Play` logs the
  skipped movie and reports it ended on the next tick; the game carries on (the studio logos of the preamble
  need a HUD that does not exist at that point and are skipped anyway).
- **Characters stand on the floor** (cutscene "flying" fixed): a skeletal mesh's feet are at its origin, drawn at
  `Location + PrePivot + SavedPrePivot`; actors with `bAlignBottom` (and `bCollideWorld`) or `bAlignBottomAlways`
  are lowered by their collision height, plus 5 while they have physics. Harry, Ron and Hermione do not align and
  use a `PrePivot` of -42 instead. Before this every NPC hovered 43 units up.
- **Layered animation**: channels above 0 blend over the base pose on the bone named by `AnimBlendParams`, with
  `AnimBlendToAlpha` fades (the casting pose); `Falling` is sent when a script sets falling physics (jumping);
  walk and run cycles keep their root bone's horizontal position (no side-to-side sway).
- **Particles**: `Emitter` actors' sprite emitters are simulated (rates, lifetimes, start location and velocity,
  acceleration, size and colour over life, fades, spin, texture frames, draw styles) and drawn as depth-tested
  quads. Rate: `InitialParticlesPerSecond` is the working rate when `AutomaticInitialSpawning` is on. Beams are
  drawn as stretched quads but their end points and noise are not done; no collision, attractors, revolution,
  mesh or trail emitters. Not seen yet: the spell cursor and the cast "ray" (script-driven; see below).
- **Spells reach their targets** (engine side, behaviour from the reference): touches are re-checked when an actor
  moves (`SetLocation`, `Move`, `MoveSmooth`), resizes, changes its collision flag, spawns or is destroyed, with a
  frame pass for the rest (`touch.rs`); movers are tested by their geometry and never against each other; an
  actor is stopped by blocking actors only if both have `bBlockActors`, and by the level only with `bCollideWorld`
  (a spell flies through creatures it touches); projectile physics sends `HitWall` with the stopping actor (the
  level for its geometry). `AddNotify` / `AddAnimNotifys` raise script events as sequences play (the cast pose
  calls `finalizeSpell` at its release frame), with `EnableChannelNotify` and the per-channel
  `GetAnimSequence/Rate/Time/Frame`. `SetPhysics` to a non-resting mode drops the base (a launched spell leaves the
  wand), and `SetBase` raises `Attach` / `Detach`. Config properties are read from the file a class names
  (`config(Spells)` is `Spells.ini`), and a class keeps its own default unless it inherited the value.
  `hp3-run` has a cast harness (`HP3_CAST=<Spell>:<TargetClass>:<n>@<tick>`, with `HP3_CAST_ACTIVATE`,
  `HP3_CAST_WAIT`, `HP3_CAST_DIRECT`) that places the player, aims the game's camera, holds fire, and reports the
  target's states and what spawned. Verified so far: Alohomora on chest spawners, Depulso on knight spawners,
  `KnightCollapse` and flying books, Lumos on gargoyles, Spongify on pads, Rictusempra on Poxies, Lapifors and
  Draconifors on their statues (the player takes over the dragon), Carpe Retractum on its targets.
- **Debug keys** in the app: F11 level list, G ghost (no collision), F fly (with collision), H back to walking.
- Open: the spell cursor emitter stays empty (its rate is set by script during targeting, not yet observed to
  happen); companions sometimes block the player (Ron standing in InsideHub).
- M8 onwards: not started.

## Checking

```sh
cargo run --release -p hp3-pkg -- check <HP3 folder>
HP3_DIR=<HP3 folder> cargo test --release
cargo run --release -p hp3-run -- levels <HP3 folder>
```
