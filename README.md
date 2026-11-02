# hp3-engine

An open-source engine for *Harry Potter and the Prisoner of Azkaban* (PC, 2004), written in Rust.
It runs the game's own compiled UnrealScript on a new engine, so the levels, characters, spells, puzzles,
cutscenes, voices and music are the retail game's, not a remake.

**It ships no game data.** You need your own retail copy of the PC game. hp3-engine is written clean-room
(`CLEANROOM.md`) and licensed GPLv3. It supports Harry Potter 3 only.

Version **0.5.1**. It is the engine behind Harry Potter 3 in
[Flipendo Droid](https://github.com/Ihor-Ihorevych/flipendo-droid) (Android), and it also runs on Windows.

## What works

- All 34 levels load and run with no script errors. The front end (menus, New Game, save slots) works, and so do level travel and saving.
- **Play**: walking, jumping, climbing ledges, falling and flying (the hippogriff). The three heroes with their
  companions, their spells (Rictusempra, Depulso, Alohomora, Lumos, Carpe Retractum, Glacius,
  Draconifors, Lapifors, Spongify, the Patronus and more), spell lessons and challenges, jelly beans
  and chests, cauldrons, puzzles with movers and spinning platforms, creatures (Bundimuns, flying books, fire crabs,
  imps, Dementors) and their AI, and cutscenes (Enter skips any of them).
- **Look and sound**: static and baked lighting, terrain, sky, skeletal characters with layered animation,
  particle effects, the HUD and menus with the game's fonts (system fonts for other languages), Bink sound
  effects and dialogue, and Ogg music.
- **Platforms**: Windows (desktop window, 60 fps) and Android arm64. On Android it runs inside Flipendo with
  its touch controls and settings panel.

Known gaps are in `docs/status.md` and `docs/ROADMAP.md`: no movie playback (Bink video), parts of the GUI
(some list and slider widgets), and many values still need to be matched to the retail game by playing it.
Please report what you find.

## Play it on Windows

1. Install the Rust toolchain (rustup) and CMake. SDL3 is built from source, so CMake must be on the `PATH`
   (Visual Studio Build Tools ships one).
2. Build and run, giving the folder of your HP3 install (the one with `System` and `Maps` in it):

```sh
cargo build --release -p hp3
target/release/hp3.exe "C:/Games/HP3"              # the front end
target/release/hp3.exe "C:/Games/HP3" HP3_Adv1Express   # a level straight away
```

`hp3.exe` opens a window; Shift+Escape quits. `cargo build` without `--release` is too slow to play.

### Controls

They are the game's own key bindings (read from its ini files): **W A S D** walk, **Space** jumps, the **mouse**
looks and the **left button** casts, the **right button** jumps too, **Enter** uses things and skips cutscenes,
**Tab** is the map, **Escape** opens the menu. Engine keys:

| Key | |
|---|---|
| F1 | free camera (WASD, mouse, Space / Ctrl, Shift) |
| F5, F9 | quick save, quick load |
| F11 | list of levels |
| G, F, H | debug movement: ghost, fly, walk |

### Options

Environment variables (set them before starting):

| Variable | |
|---|---|
| `HP3_FPS=<n>` | frame rate limit (60 by default, `0` for none) |
| `HP3_FIRST_PERSON=1` | camera in the character's head |
| `HP3_HIT_LOG=1` | prints what blocks a character (for collision bugs) |
| `HP3_UNLIT=1` | lighting off |
| `HP3_PROFILE=1` | frame timing |

`docs/` lists the rest. Headless tools for development (`hp3-run`, `hp3-view`, `hp3-pkg`) are below.

## Play it on Android

Use Flipendo Droid: it carries this engine, Flipendo's touch controls and settings, and a launcher that takes
the HP3 data folder. The Android build of the library is made by Flipendo's `android/build-apk.sh`
(`cargo build --release --target aarch64-linux-android -p hp3 --lib`, NDK 27).

## Tools

| Tool | What it does |
|---|---|
| `hp3-pkg` | reads game packages: `check`, `info`, `exports`, `dump`, `disasm` (bytecode), `textures`, `sounds` |
| `hp3-run` | headless runner: `level <folder> <map> [ticks]` runs a level, `classes`, `instances`, `levels`; debug env vars to cast spells, trigger events, watch actors, probe collision |
| `hp3-view` | headless renderer: `shot` (static map), `play` (live actors), `pose` (one mesh) to a PNG |

## How it is built

A workspace of crates: `hp3-package` (the package format), `hp3-assets` (meshes, textures, sounds...),
`hp3-script` (the UnrealScript VM), `hp3-world` (physics, collision, touches, navigation, particles, cutscenes and the
game's natives), `hp3-anim` (skeletal animation), `hp3-audio`, `hp3-render` (wgpu), `hp3-gui`, `hp3-engine` and
the `hp3` app (SDL3 window and Android entry). Formats and findings are written down in `docs/formats/` and
`docs/status.md`.

## Clean room

The engine is written from observing the retail game's data and scripts, not from the game's engine source code.
`CLEANROOM.md` lists exactly which sources may be used. Contributions follow the same rule.

## Legal

hp3-engine is free software under the GNU General Public License, version 3 or later (`LICENSE`). It is an independent
fan project, not affiliated with or endorsed by Warner Bros., Electronic Arts or the game's developers. Harry Potter is a
trademark of Warner Bros. Entertainment. The game data is not included and not free: you need your own copy.
