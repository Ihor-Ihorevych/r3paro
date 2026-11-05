# R3paro

A clean-room reimplementation of the engine of *Harry Potter and the Prisoner of Azkaban* (PC, 2004) in Rust.
It is a new, open-source engine that runs the game's own compiled UnrealScript, so the levels, characters, spells,
puzzles, cutscenes, voices and music are the retail game's, not a remake.

**It ships no game data.** You need your own retail copy of the PC game. R3paro is written clean-room
(`CLEANROOM.md`) and licensed GPLv3. It supports Harry Potter 3 only.

Version **0.5.5**. It is the engine behind Harry Potter 3 in
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

## Install and run

You need **your own copy of the PC game** (Harry Potter and the Prisoner of Azkaban, 2004). The engine reads its
data files and runs its scripts; nothing is downloaded or shipped with it.

### 1. Get the game's folder ready

Install the PC game (or copy an existing install). The folder you need is the one that has `System` and `Maps`
next to `Textures`, `Sounds` and `Music`, with `System\Core.u` inside (written `system/core.u` on a phone).
The engine only reads this folder; it writes its saved games to a `Save` folder inside it, so the folder must be
writable. The Windows programs of the game (`.exe`, `.dll`) are not used and can be missing.

### 2. Windows

**Easiest: the release zip.**

1. Download `r3paro-<version>-windows.zip` from the [Releases](../../releases) page and unpack it anywhere.
2. Open a terminal (PowerShell or cmd) in that folder and start the game, giving it the path of the game folder:

   ```
   .\hp3.exe "C:\Games\HP3"
   ```

   Or drag the game folder onto `hp3.exe`, or make a shortcut whose target is `hp3.exe "C:\Games\HP3"`.
3. A window opens and the game starts at its own menu: **New Game**, pick a slot, and play. To jump straight to a
   level, add its name: `.\hp3.exe "C:\Games\HP3" HP3_Adv1Express` (the names are the files in `Maps`, without `.unr`;
   **F11** in game shows the list). **Shift+Escape** quits.

The first start takes a few seconds while the scripts and the level load; a log scrolls in the terminal.

**Build it yourself** (the same exe, from this source): install [rustup](https://rustup.rs) (stable Rust) and
CMake (Visual Studio Build Tools ships one; it must be on the `PATH`, SDL3 is built from source), then:

```sh
git clone https://github.com/Ihor-Ihorevych/r3paro
cd r3paro
cargo build --release -p hp3
target
elease\hp3.exe "C:\Games\HP3"
```

A build without `--release` is far too slow to play. The first build takes several minutes.

### 3. Android

On Android the engine is part of [Flipendo Droid](https://github.com/Ihor-Ihorevych/flipendo-droid), which brings the touch
controls and settings. You need Android 10 or newer, 64-bit, and about 1 GB free for the game's data.

1. Install the APK from Flipendo's [Releases](https://github.com/Ihor-Ihorevych/flipendo-droid/releases). The app has no
   game data (steps 2 and 3 follow).
2. Put the game's folder on the phone: copy your `HP3` folder (the one from step 1 above, or the data zip of the release)
   to the phone's internal storage, for example as **FlipendoHP3**. Keep the `System`, `Maps`... folders directly inside.
3. Start Flipendo and allow **All files access** when it asks, then open the app again.
4. On the start screen pick **Harry Potter 3**. If it does not find the game by itself, tap **Choose folder** and select the
   folder from step 2. The launcher remembers it.
5. Play: the left half of the screen is a floating stick, drag the right half to look around, the big button casts, the
   other buttons jump and use. Settings (the gear, top right) has the control layout, render scale, FPS limit and a debug menu with
   cheats and a level list.

### If something goes wrong

| What you see | What to do |
|---|---|
| `error: ... Core.u` or "no such package" | the folder you gave is not the game's folder: it must hold `System\Core.u` and `Maps` |
| the window opens and stays black | wait for the level to load (a minute on a slow disk); the terminal prints progress. Update your graphics driver if it never ends |
| very slow | use the release build (`cargo build --release`); `HP3_FPS=0` lifts the 60 fps cap |
| no sound | check the system volume and the default audio device; the music is `Music\*.ogg` |
| a level crashes or a puzzle can't be solved | start it from the terminal, copy the last lines and open an issue with the level name |

## Controls (Windows)

They are the game's own key bindings (read from its ini files): **W A S D** walk, **Space** jumps, the **mouse**
looks and the **left button** casts, the **right button** jumps too, **Enter** uses things and skips cutscenes,
**Tab** is the map, **Escape** opens the menu. Engine keys:

| Key | |
|---|---|
| F1 | free camera (WASD, mouse, Space / Ctrl, Shift) |
| F5, F9 | quick save, quick load |
| F11 | list of levels |
| G, F, H | debug movement: ghost, fly, walk |

## Options

Environment variables (set them before starting, e.g. `$env:HP3_FPS = 0` in PowerShell):

| Variable | |
|---|---|
| `HP3_FPS=<n>` | frame rate limit (60 by default, `0` for none) |
| `HP3_FOV_OFFSET=<degrees>` | degrees added to the game's field of view (20 by default, as on the phone) |
| `HP3_FIRST_PERSON=1` | camera in the character's head |
| `HP3_LANGUAGE=<ext>` | interface language: `int` (English), `ukr`, `bel`, `geo`, `lav`; also read from a `.flipendo-language` file next to the game (the Android launcher writes it). Menus and loading tips only: subtitles stay in the edition's language |
| `HP3_HIT_LOG=1` | prints what blocks a character (for collision bugs) |
| `HP3_UNLIT=1` | lighting off |
| `HP3_BACKEND=<api>` | graphics API: `auto` (default: Vulkan, then DX12 on Windows, then OpenGL), `vulkan`, `gl` (OpenGL; OpenGL ES 3 on a phone), `dx12`, `metal`. The Android launcher sets it from its Renderer choice |
| `HP3_PROFILE=1` | frame timing |

`docs/` lists the rest. Headless tools for development (`hp3-run`, `hp3-view`, `hp3-pkg`) are below.

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

## Community

Questions, bug reports and results from your phone are welcome in the Telegram group: [flipendodroid.t.me](https://t.me/flipendodroid).

## Clean room

The engine is written from observing the retail game's data and scripts, not from the game's engine source code.
`CLEANROOM.md` lists exactly which sources may be used. Contributions follow the same rule.

## Legal

R3paro is free software under the GNU General Public License, version 3 or later (`LICENSE`). It is an independent
fan project, not affiliated with or endorsed by Warner Bros., Electronic Arts or the game's developers. Harry Potter is a
trademark of Warner Bros. Entertainment. The game data is not included and not free: you need your own copy.
