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
  wgpu while its scripts run at 60 Hz (Vulkan 1.0+ or OpenGL / OpenGL ES 3.0+: the device asks only for the
  adapter's own limits, and `HP3_BACKEND` picks the API, Vulkan first and OpenGL when that fails; OpenGL surfaces
  cannot be copied into, so the presenter draws frames into them). Keyboard and mouse go through the game's key bindings; the view is the
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
  (the whole level for now, to be culled to the sky zone), then the scene over it. Terrain decoration layers (the Adv1Express pines) are scattered with the original's random generator and baked in, with no distance fade-out yet. Not yet: emitters,
  translucency, fog, dynamic lights, HUD.
- **M6 Movement, started**: swept-box collision against the level BSP, static mesh and terrain triangles and
  blocking actors; walking (friction, step-up of 24, sliding, floor snapping; going down a stair the floor sweep's
  graze of the tread corner counts as the tread, not a wall, so pawns step down instead of falling off each tread;
  `HP3_FALL_LOG=1` prints why a walker lost its floor), falling and landing, flying,
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
  within reach, and the game's `[ALAudio.ALAudioSubsystem]` volumes are used. Not yet: reverb/occlusion, music on a thread (a long track delays its start by a
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

## Languages

An install can hold several languages at once. The player's choice (`HP3_LANGUAGE`, or `.flipendo-language` next to the game, which the
launcher writes; `int` is English) picks both the texts and the voices; with no choice the install's own language (`Default.ini`) stands.

- **Texts:** `System/<Package>.<lang>` (`HpMenu.int`, `HpMenu.rus`...), read by `Config::localize`; a missing text falls back to `int`, then to the
  edition's own language.
- **Voices and localised art:** packages named `<name>.<lang>_<ext>` (`alldialog.rus_uax`, `HP3_Menu.rus_utx`) beat the plain `<name>.<ext>` (English)
  when that language is chosen (`PackageSet::open_with_language`). A language with no package of its own (the text-only translations) keeps what
  the install offers; a package only another language provides is used all the same.
- **What differs between the retail English and the Russian edition:** the five text files (`BumpDialog`, `HpDialog`, `HpMenu`, `SaveTips`,
  `ScriptTimes`), `sounds/alldialog` (the dub), `HP3_Menu.utx` and `HP3_Gameplay.utx` (the title logo and some menu art). `kwGame.u` differs in
  20 bytes of the copy-protection digest only. Everything else is identical.
- **Subtitle fonts:** Russian voices need system fonts for the subtitles whatever the menus speak; English voices use HP3's own fonts.
- **Phone:** the launcher's own English `HpMenu.int` / `SaveTips.int` (made for a Russian-only install) are not copied when `HpDialog.int` is there.

## Matching the original: 2026-11-05/06 pass

Gaps found by comparing against `flipendo-droid-2004/Source` and the game's own scripts, and what was done.

Done:
- **Falling** follows the original's step: half-step speed for the move, then the speed it really moved at; volume `TerminalVelocity` (total
  speed) and `ZoneVelocity`; `ModifyVelocity` is raised each slice (the hippogriff's track steering lives there); the controller hears
  `NotifyJumpApex` and `NotifyLanded` (the hero's slide takes its landing speed there) before the pawn's `Landed`.
- **Flying pawns** (`PHYS_FLYING`) use the volume's fluid friction, `AirSpeed` (times `DesiredSpeed` for AI), slide with `HitWall`, take their
  speed from the distance moved.
- **Wall hits** for AI pawns follow `processHitWall`: no event without acceleration, only head-on (`MinHitWall` against the direction to the
  controller's `Destination`), the controller's `NotifyHitWall` may take it. The player's pawn still hears every hit.
- **Pawn lean**: pawns with `RotationRate.Roll > 0` (all HP3 characters) bank into their acceleration and settle level when slow.
- **Rotating physics** (`PHYS_ROTATING`): one angle at a time, `bFixedRotationDir`, `EndedRotation`; a controlled pawn is not turned twice.
- **Enable / Disable** of events (the probe mask) work; the mask resets on every `GotoState`, so a `Disable` in `PostBeginPlay` is lost to
  `SetInitialState`, as in the original.
- **Terrain**: layers stop at the first slot without texture or alpha map; holes (`QuadVisibilityBitmap`) are holes for collision too.
- **Editor-only map objects** (cameras, brush builders: exports flagged not-for-client and not-for-server) are not loaded.
- **Bottom-aligned meshes** drop by the collision height and keep their mesh origin height (no lowest-bone rule).
- **LevelEnterText** is filled with the map name on load (the game reads the level it leaves from it for the next level's SmartStart).

Open (found, not done):
- **Dynamic shadows**: characters have `ShadowProjector`s (a `ShadowBitmapMaterial` projected on the floor); `Projector.AttachActor` /
  `DetachActor` are unimplemented and nothing draws projectors on actors.
- `FluidSurfaceInfo.Pling` (water ripples; water is a flat grid).
- `Controller.AdjustFromWall` (nudges an AI mover off a wall that belongs to its target).
- Pawn lean and fly/swim physics are checked only by headless runs; swimming (`PHYS_SWIMMING`) still uses the simple flight step.

## Lighting matched to the original (2026-11-06)

Found by comparing with `flipendo-droid-2004/Source/Engine` (`UnTex.cpp FGetHSV`, `UnActor.cpp UpdateRenderData`, `UnRenderLight.cpp`,
`UnRenderBatch.cpp`, `UnRenderVisibility.cpp`):
- **Colors**: light and zone-ambient colors come from `FGetHSV` (three-sector hue blend, brightness curve, about 0.82 at 255), not a
  plain HSV; light colors are scaled by `LevelInfo.Brightness`. The computed zone ambient equals the map's saved `AmbientVector` to
  1e-7, which confirms the function. Warm lights stay orange instead of clipping to white.
- **Ambient per actor**: an actor takes the strongest per channel of the zones its bounding sphere reaches into (plus `AmbientGlow`),
  worked out again when it moves; static meshes and terrain take the ambient of the zone they stand in, instead of one average for the map.
- **Same as the original already** (checked, not changed): lightmap math, actor light picking, fades and the D3D attenuation fit, the
  fixed-function vertex shader. The maps ship no baked lightmap pixels, so the original builds them at load from the same shadow bitmaps.
- **Gamma**: the original's Windows client defaults (Brightness 0.5, Contrast 0.5, Gamma 1.0) make the ramp the identity; nothing to apply.
- ~~Not done: the dynamic-light pass~~: done 2026-11-07, see below.

## HP3's own lighting changes, read from its binaries (2026-11-07)

UT2004's source is not HP3's: `spike-hp3/System/Engine.dll` and `D3DDrv.dll` were disassembled (`dumpbin /disasm`; Unreal exports
its C++ symbols, e.g. `?SampleIntensity@FDynamicLight@@...`) and compared function by function with the UT2004 code:
- **Lightmap palette**: HP3's `MergeLightMap` shifts by the light's factor, UT2004's by one less, so a light adds `level x color`,
  half of UT2004's. Our BSP lights were twice too bright (`HP3_LIGHTMAP_SCALE` in `scene.rs`). The ambient fill (half the zone's
  HSV) and `FDynamicLight::SampleIntensity` / `Update` are the same as UT2004's.
- **Static meshes**: HP3's `CalculateStaticMeshLighting` scales every light by `ScaleGlow * 0.5`, which is why the maps' saved
  `StaticMeshInstance` colors are half of what UT2004's formula gives (they are what HP3 computes, so we keep using them). Both the
  batched and the single static-mesh paths add the zone ambient **halved** (UT2004: whole); characters keep all of it.
- **D3D**: the lightmap stage is 2x unless the draw also uses vertex colors (HP3 added that condition), which only the dynamic-light
  pass does: its attenuation stage is 1x, its vertex color 2x.
- **Dynamic lights** (`bDynamicLight`; a wand's glow while charging, `WandEmitter`, `LE_QuadraticNonIncidence`, radius 125): the
  level's surfaces now take up to four of them per frame (`MovingLight`, `Renderer::set_moving_lights`), added in the world shader.
  BSP: the original's projected disc, `color * sqrt(1 - h^2/R^2) * (1 - along/disc)`; static meshes: the D3D point light
  (`1 / (8 d^2 / R^2)` for the quadratic kind) by the cosine, doubled. `HP3_MOVING_LIGHTS=1` prints them, `=0` turns them off;
  `HP3_DYN_LIGHTS=<every>` in hp3-run lists the live dynamic lights.
- Against a frame of the original (Carpe Retractum font room, `original_light.jpg`): median luminance of the room's sides 59 -> 48
  (original 22). The wall lit by the windows now matches (64 vs 54); ambient-lit walls are still about twice the original's,
  with nothing left in the code paths compared that explains it (capture gamma of that frame unknown).

Phone performance (POCO F6, OpenGL ES), InsideHub: 30 -> 55 fps. Posed character vertices are drawn from a ring of three buffers
(writing into a buffer the GPU still read made the driver wait about 10 ms a frame); the default physics volume is found once;
actor lights are looked up through a ground grid. `perf:` lines on Android now include posed KB, upload ms and draw calls;
`env.flag` in the game folder sets debug switches (`NAME=value` lines).

## Fixes from the 2026-11-07 bug list (written unattended, not yet seen on screen)

- **Beans stuck in a chest** (Adv1Express `HPGenericSpawner1`, a chest on a `BlockingVolume` slab): a bean spawns 10 below the chest, inside the
  slab's footprint and a few units into its top; the slab's side face then acted as a wall and every `HitWall` halved the bean's speed to
  zero. `Collision::starts_in_volume` now skips a volume whose footprint holds the mover's centre and which the box already sinks into
  (not only a centre strictly inside), and `tri_trace` shrinks the edge tolerance of a flat box (a bean) to its half height. All five
  Adv1 chests now scatter their beans. Still open: Adv5Shack skull-shelf beans bounce off the edge of their floor and fall to a floor 2000
  units lower (`SkullShelfSpwn*`, `KnightSpawner1` in Adv3Library were not looked at).
- **Mouth**: the jaw was rolled about its own long axis (only the mouth corners moved). Its X runs to the chin, Z across the face, so it now
  yaws, -25 degrees (`HP3_JAW_AXIS`/`HP3_JAW_DEG` still override). Not checked by eye. Real HP3 lip-sync is 9 viseme pose sequences on channels
  25-33 (rooted at `Body__Mouth`) driven by the 26 curves of each dialog sound; only curve 19 is used for the jaw so far.
- **Lumos walls**: `LumosSparklesTrigger` walls (`Actor.LumosType` 1 appear / 2 disappear) are no longer baked into the level; they are drawn live
  and, while `PlayerHeroActor.bLumosOn`, fade by distance within `fLumosRadius` (the HP2 curve: down to about 22% opacity). Support:
  `ActorLight.fade` goes to the dynamic shader (`extra.x`), a faded actor's sections all go through the blended pass. Not checked by eye;
  walls now take the live (vertex) lighting instead of the baked one.
- **Harry beside Ron before the control switch** (end of Adv1Express): fixed by `HPCharacter.SwitchToFightStanceAnims` (a native override in `natives.rs`) leaving a pawn in `DementorEncounterCollapse` as he is; the cause as found: At the end of the cutscene Harry's own script finishes and
  the hero controller gets him back; `DementorEncounter0` then starts `StateHWE_DementorEncounter`, whose `SwitchToFightStanceAnims` runs on
  its pawn (Harry) and replaces his collapsed idle with `BattleIdle`; only afterwards `SwitchControlToMe` moves control to Ron. The original
  presumably runs these in another order; where it differs is unknown.

## Dialog lip-sync as the original, and soft shadows (2026-11-07)

- **Lip-sync** (`crates/hp3-world/src/lipsync.rs`), read from HP3's own `Engine.dll` (`FLipGenLipSync`): a dialog line's envelope has 26 floats per
  frame; only the first 15 are read. They drive nine mouth-shape pose sequences, each on its own animation channel (25 to 33) limited to
  `Pawn.MouthBone` (`Body__Mouth`: jaw, lips, tongue): `ai` (curve 2), `e` (0, gain 0.6), `o` (3), `u` (4), `cdgknrsthyz` (1 6 7 9 12 14), `l` (10),
  `wq` (5 13), `mbp` (11), `fv` (8). Every channel plays its sequence frozen at half of it (with two frames, the full shape; frame 0 is the rest
  pose) and its alpha follows `0.8 * mean(curve * gain over the shape's curves above 0.001)` with a 0.2 s blend, updated every 0.033 s. The row
  read is `frame - header[2]` (the first 9 rows are lead-in), clamped to the last row. Curves 15 to 25 (brows, blinks, head and eye angles of
  the analysis tool) are not read by the retail engine. The older single-jaw-bone turn remains for speakers without the nine sequences, the
  amplitude bytes of creature sounds still drive `mouth_anim`. Tools: `HP3_LIPSYNC_DEBUG=1`, `HP3_LIPSYNC_DUMP=1` (envelope rows),
  `hp3-view pose` with `HP3_LIST_SEQS`, `HP3_FACE_POSES`, `HP3_FACE_REF`.
- **Soft shadows** (new, not in the original): `actor_instances` traces down from every pawn (`pawn_shadow`) and gives the renderer a quad
  lying on the ground (`Dynamic::set_shadows`, `BlobShadow`): a soft bell, stretched away from the strongest light on the pawn, weaker when
  the pawn is high above the ground or poorly lit. `HP3_NO_SHADOWS=1` turns them off, `HP3_SHADOW_DEBUG=1` prints the count.
