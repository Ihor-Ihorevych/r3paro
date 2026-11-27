# r3paro launcher: design

## Intent

Players start r3paro today by typing `hp3.exe "C:\Games\HP3"` in a terminal or dragging the game folder onto the exe.
The launcher replaces that: the player chooses the game folder, the launcher checks it is a usable HP3 install, and
Play starts the engine on it. Windows, Linux and macOS.

## Goals

- Choose the HP3 game folder with a folder picker.
- Validate it with clear, specific messages.
- Remember the folder between runs.
- Start `hp3` on it, and show the error if the game fails right away.

## Non-goals (v1)

- Choosing a map, `HP3_*` environment-variable settings, auto-detecting installs, installers or packaging.
- Supporting games other than HP3.

## Constraints

- .NET 11, Avalonia (latest stable), MVVM (CommunityToolkit.Mvvm), DI (Microsoft.Extensions.DependencyInjection).
- `hp3` takes `<game folder> [map]` (`apps/hp3/src/lib.rs`); the launcher passes only the folder.
- A valid folder (README): holds `System/Core.u` and `Maps`; must be writable because saves go to `Save/`.
- Linux/macOS installs are often lowercase (`system/core.u`): name matching is case-insensitive.

## Approaches

- **Avalonia app in `apps/launcher` of the r3paro repo** (chosen): what the user asked for, lives next to the binary it starts.
- Rust GUI (egui/SDL) inside `hp3`: no .NET, but not Avalonia/MVVM as asked.
- Separate repo: decouples it from the engine it launches for no gain.

## Design

Project `apps/launcher/R3paro.Launcher` (Avalonia app) and `apps/launcher/R3paro.Launcher.Tests` (xUnit).

**Window**: game folder path (read-only text), **Browse…**, a status line (valid / the reason it is not), **Play**
(enabled only when valid), and an error area showing the tail of stderr when the game fails at start.

**Services** (interfaces, constructor-injected, registered in `App`):

- `IGameFolderValidator` → `ValidationResult { IsValid, Message }`. Checks in order: folder exists; `System` dir;
  `System/Core.u`; `Maps` dir; writable (create `Save/` if missing, write and delete a temp file). Case-insensitive lookup.
- `IGameLauncher` → resolves the engine (`hp3.exe` on Windows, `hp3` elsewhere) next to the launcher's base directory,
  or `EnginePath` from settings; starts it with the folder as the single argument, working directory = engine dir,
  stderr redirected. If it exits non-zero within ~5 s, returns the last ~20 stderr lines as a failure.
- `ISettingsStore` → `LauncherSettings { GameFolder, EnginePath }` in JSON under the per-user app-data folder
  (`Environment.SpecialFolder.ApplicationData`/r3paro/launcher.json).
- `IFolderPicker` → wraps Avalonia's `StorageProvider.OpenFolderPickerAsync` so the view model stays UI-free.

**MainViewModel**: loads settings on start and validates the saved folder; `BrowseCommand`, `PlayCommand`
(CanExecute = valid and not launching); saves the folder whenever it becomes valid.

**Tests**: validator against temp directories (missing folder, missing Core.u, missing Maps, lowercase layout, valid);
settings round-trip; view model with fake services.

## Left to the builder

Exact Avalonia/package versions, layout styling, window size, log wording.
