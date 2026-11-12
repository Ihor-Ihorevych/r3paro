# HP1 broom flight and broom camera: AdamJD's "Movement mod" vs retail

Behaviour notes for a clean-room reimplementation of the mod's *feel* in `crates/hp3-world/src/quidditch`. No mod
script or bytecode is reproduced here; everything below is a description of observed behaviour with the numbers the
two package sets carry.

## How this was established

Both package sets were decompiled with Flipendo-Droid's bundled `Eliot.UELib` (built into the session scratchpad, not
into the repo) and compared class by class, function by function:

- retail: `C:\Games\Game Files HP1 German\System\` (German retail, package version 76, unpatched)
- mod: `movement.zip` (`HPBase.u` 2020-03-08, `HarryPotter.u`/`Hub2-5.u`/`Tut1.u` 2019-12-20), extracted to its own
  scratchpad directory and only ever read

Native function numbers were resolved from the retail `Engine.u` itself (so `MoveSmooth` = 3969, `SetPhysics` = 3970,
`TurnToward` = 510, `SetRotation` = 299, and the arithmetic/comparison operators), which makes the readings below
exact rather than guessed.

**Short answer: yes, the mod changes the broom.** It changes one function on `BroomHarry`, two of its state scripts
and one default, and it replaces the camera that the broom levels run under. It does *not* touch broom speed, boost,
brake, sound, animation or any input variable.

Units throughout: Unreal rotation units (65536 = one turn, 182.044 units = 1 degree), Unreal units (uu) for length,
and seconds. Positive pitch is nose-up; roll is banking.

## 1. Retail baseline (what the mod starts from)

The flying player pawn is `HarryPotter.BroomHarry` (extends `Harry` extends `HPBase.baseHarry` extends
`Engine.PlayerPawn`). All flight lives in its `PlayerWalking` state (its `PlayerTick` just calls `PlayerMove`).

### Tuning values

| value | where | retail |
|---|---|---|
| `AirSpeedNormal` | defaults | 400 uu/s |
| `AirSpeedBoost` | defaults | 800 uu/s |
| `PitchLimitUp` / `PitchLimitDown` | defaults | 60 / 60 (degrees) |
| `RotationRate` | defaults | pitch 24000, yaw 50000, roll 6000 (units/s) |
| `RotationRate` | overwritten in `PostBeginPlay` | pitch 24000, yaw 50000, roll 6000 — i.e. the same |
| `fRotationRateYaw` (the steering yaw rate) | `PostBeginPlay` | 20000 units/s = 109.9 deg/s |
| `fBroomSensitivity` | `PostBeginPlay` | 1/20000 = 0.00005 per mouse-axis unit |
| `WallDamage` | defaults | 1 |
| reversal statistics | consts | 20 samples over a 2.5 s window; wall-avoidance direction held at most 1.0 s |

Retail key bindings (`DefUser.ini`, unchanged by the mod, which ships no `.ini`): arrow keys are
`bBroomYawLeft/Right` and `bBroomPitchUp/Down`; `RightMouse` and `A` are `bBroomBoost`; `LeftMouse` and `Z` are
`bBroomBrake`; `Jump` is `bBroomAction`; `MouseX` drives `aBroomYaw` at speed 6 and `MouseY` drives `aBroomPitch` at
speed **-6** (inverted at the binding); `JoyX`/`JoyY` the same at speed 6/-6. `bInvertBroomPitch=False`,
`bAllowBroomMouse` is a global config flag.

### Steering (retail)

Input is turned into two virtual stick values, `fPitchControl` and `fYawControl`, once per frame:

- Axis dead zone 0.0005 everywhere.
- **Mode latch.** While no yaw key is held and `bAllowBroomMouse` is set and the yaw axis moves, yaw goes "under
  mouse"; the moment a yaw key is held, mouse yaw is dropped and its accumulator zeroed. Pitch latches the same way
  independently.
- **Mouse mode is a virtual spring-less stick, not a rate.** The mouse delta times `fBroomSensitivity` is
  *accumulated* into a stick position clamped to +-1.5. Yaw then gets a large dead zone with an offset: below 0.5 of
  accumulated deflection the yaw stick is 0, above it the stick is the accumulation minus 0.3 (so it jumps to 0.2 and
  rises to 1.2). Pitch keeps the raw accumulation.
- **Key mode** is digital: yaw stick = right minus left, pitch stick = up minus down (negated by
  `bInvertBroomPitch`).

Rotation is then integrated by the pawn itself:

- **Yaw** is a rate. The yaw stick is clamped to +-1 and integrated at `fRotationRateYaw` (20000 units/s = 110 deg/s)
  into the player's own `ViewRotation.Yaw`, and the pawn's rotation is set to it. **While the broom is coasting
  (acceleration exactly zero) the yaw rate is multiplied by 4/3** (26667 units/s = 146.6 deg/s).
- **Pitch with the mouse is absolute, not a rate**: the accumulated pitch stick, after a 0.15 dead zone and
  saturating at 1.0, maps linearly onto the pitch limit, so stick position = pitch angle within +-60 deg.
- **Pitch with the keys is a rate with auto-levelling**: held, pitch changes at `RotationRate.Pitch`
  (24000 units/s = 131.8 deg/s); released, the broom returns to level at that same rate (the code converts the
  current pitch into exactly the stick value that cancels it in one `RotationRate.Pitch * dt` step, clamped to 1).
  Pitch is clamped to +-60 deg and snapped to the limit when it would cross it.
- **Wall avoidance**: with no yaw input and the wall flag set by `HitWall`, a yaw stick value is synthesised from
  `WallAvoidanceYaw * fWallAvoidanceRate` so the broom steers itself off the wall; the direction is held for at most
  1 s.
- **Reversal statistics**: every sign flip of the pitch or yaw input is timestamped (20 slots, 2.5 s window) and
  `GetReversalsPerSecond` reports it — the referee uses it for "flying like a maniac" feedback.
- **View shake** is applied to the view every frame.
- `DesiredRotation` is set to the rotation just applied, so the engine's pawn rotation step has nothing left to do
  for yaw/pitch. Roll is never written by script: the engine banks it from lateral acceleration up to
  `RotationRate.Roll` (6000 units = 33 deg) and eases it back to level, which is where all broom banking comes from.

### Speed, boost and brake (retail)

- Thrust is constant and always forward: acceleration = forward axis times 200000, i.e. the broom is always at full
  throttle along its nose. The destination each frame is the nose direction times `AirSpeed * dt + 0.1`.
- **Boost**: while the boost button (or the scripted `bAuxBoost`) is down **and the brake is not**, `AirSpeed` is
  `AirSpeedBoost` (800). The brake therefore cancels the boost outright.
- **Brake and hard turns are the same thing.** If the brake is down **or** the magnitude of the yaw stick exceeds
  0.2, a `Deceleration` value ramps up at `(AirSpeedNormal/2)/2.5` = 80 uu/s per second, capped at
  `AirSpeedNormal/2` = 200. Released, it decays at 1000 uu/s per second while above 160, then at 80 uu/s per second,
  down to 0. `AirSpeed` is `AirSpeedNormal - Deceleration`, so cruise is 400 and a sustained brake or hard turn
  settles at 200.
- `UpdateBroomSound`, `DeterminePrimaryAnim` (lean/turn animation chosen from roll and speed) and the snitch
  look-around timer run after that.

### Camera (retail)

The camera is a `HPBase.BaseCam` pawn placed in the map. The broom maps (`Lev2_Quid1`, `Lev3_Quid2`,
`Lev2_RemChase`, `Quid_*`) each carry a `BaseCam` with its `CameraType` property overridden in the map and no
`CameraTrigger` actors, and camera type 1 is `QuidditchState`; retail `StartState` applies the map's type, so the
broom runs under `QuidditchState`.

`QuidditchState` on entry: the aim target in front of Harry is offset (100, 0, 50); **camera height 60,
camera distance 150**. Each tick:

- The camera's yaw chases the player's `ViewRotation.Yaw` proportionally: the gap times `cameraRotSpeed` (10) times
  `dt`, clamped to +-1024 units per tick (5.6 deg, i.e. at most ~337 deg/s at 60 fps).
- Camera pitch (`campitch`) is mouse-orbited only while the player is *stationary*; on a broom the player is always
  moving, so camera pitch is forced to 0 every tick.
- The goal point is `CameraDistance` (150) away from Harry along the direction from Harry's head (eye height +
  `CameraHeight`) away from the aim target in front of him, i.e. behind and above, never below Harry's own Z. Up to
  6 alternative positions are tried when Harry cannot be seen from it (a 1.5 s grace counter).
- The camera then moves a fraction `min(CameraSpeed * dt, 1)` of the remaining distance each tick, with
  `CameraSpeed` = 2, i.e. an exponential position chase at 2/s (tripled while backing up or in a boss/reverse
  camera).
- A latent loop in the state also runs `TurnToward(aim target)` continuously, so the camera pawn turns to face the
  point in front of Harry at its own `RotationRate` (20000 units/s = 110 deg/s).
- There is no field-of-view change with speed anywhere; `ShakeCamera` is already an empty function in retail.

On the broom, retail also switches camera state for two events: a death goes to the top-down camera, and catching
the snitch goes to the reverse camera with the aim target moved to (-100, 10, 50).

## 2. What the mod changes

### 2.1 `BroomHarry` — steering becomes free mouse-look, slaved to the camera

Exactly one function changed: the flight state's rotation update. The whole retail input pipeline above
(`PlayerMove`, the latch, the accumulators, the dead zones, the reversal counting, the wall avoidance, the pitch
auto-levelling, the pitch limit) is **still computed, and then discarded** for the parts that used to steer. In its
place:

1. **Steering is now raw smoothed mouse delta, both axes.** The player's `SmoothMouseX`/`SmoothMouseY` (UE1's own
   smoothed mouse axes: raw counts times the binding speed 6 times `MouseSensitivity`, per tick) are divided by
   **1.5**, multiplied by `dt`, and *added* to the player's `ViewRotation.Yaw` and `ViewRotation.Pitch`. So pitch
   stops being an absolute stick position (retail mouse mode) or an auto-levelling rate (retail key mode) and
   becomes a free integrating mouse-look axis, like an FPS.
2. **There is no pitch limit on the broom any more.** `PitchLimitUp/Down` (+-60 deg) is still computed and the
   clamped value is still produced, but it is overwritten before use. The only pitch limit left in the system is the
   camera's (see 2.2): +-14000 units = +-76.9 deg.
3. **A clamp that never fires.** The mod clamps the per-frame yaw step to +-20000 units and the pitch step to
   +-10000 units, but each clamp is guarded by a condition that is only true when that mouse axis is exactly zero,
   so in practice neither clamp ever applies. (The same +-20000/+-10000 clamps *do* apply in the camera, where they
   are unguarded — see 2.2. Treat +-20000 yaw / +-10000 pitch as the intended per-step limit.)
4. **The broom's own rotation is taken from the camera.** Instead of applying its integrated yaw, the pawn sets its
   yaw and pitch to the *camera actor's* yaw and pitch. The relationship is inverted compared with retail: the
   camera no longer follows the broom, the broom follows the camera.
5. **`DesiredRotation` is set to the mouse-accumulated view rotation** instead of to the rotation just applied. The
   engine's pawn rotation step then turns the broom on from the camera's rotation towards the view rotation at
   `RotationRate` — yaw 50000 units/s = 274.7 deg/s, pitch 24000 units/s = 131.8 deg/s. So the broom's heading each
   frame is "the camera's heading, stepped towards where the mouse is pointing, at most 275 deg/s in yaw and
   132 deg/s in pitch". This rate limit is the one hard cap on how fast the mod's broom can turn.
6. **View shake on the broom is gone.**
7. **The 4/3 coasting turn bonus is gone** (it was applied to the keyboard yaw rate, which no longer steers).
8. **Wall self-avoidance no longer steers.** The value is still computed and the timestamp still written, but
   nothing applies it.
9. `RotationRate.Yaw` in the defaults was raised **50000 -> 70000**, but `PostBeginPlay` (unchanged) writes 50000
   back at spawn, so **this change has no effect at runtime**. The effective yaw rate limit stays 274.7 deg/s.

Side effects of leaving the old pipeline in place:

- The arrow keys still produce a yaw stick value, which no longer turns the broom but **still triggers the
  hard-turn deceleration** (|yaw stick| > 0.2 drops cruise speed towards 200). The old turn keys are effectively
  brake keys.
- Moving the mouse sideways also still feeds the yaw stick through the retail mouse accumulator (dead zone 0.5,
  offset 0.3), so **steering with the mouse also brakes** once the accumulator passes the dead zone. Together with
  the brake cancelling the boost, this is the already-documented Android finding that with the mod "every turn
  braked and dropped the boost" (`flipendo-droid/docs/android.md`).
- Reversal statistics are still recorded, but now from the key/accumulator values rather than from what actually
  steers, so the referee's "maniac" measure reads something the player no longer does.

### 2.2 The camera — the broom gets the mod's new orbit camera

The mod rewrites `HPBase.BaseCam` around a new generic camera (new helper functions for mouse-to-destination-
rotation, rotation update, position update and a distance scalar; a new look-at helper actor; a new idle state), and
crucially:

**`StartState` now forces camera type 0 (`Standardstate`) and ignores the type the map set.** Every level, the broom
levels included, therefore runs the new orbit camera; `QuidditchState` is left in the package but is only reachable
through the debug camera-cycling key or a state restore.

New camera defaults (these are the numbers Flipendo's own notes already recorded for this mod):

| value | mod |
|---|---|
| arm length (`fLookAtDistance`) | 128 uu |
| look-at offset above Harry (`vLookAtOffset.Z`) | 55 uu |
| rotation speed (mouse gain, `fRotSpeed`) | 4 |
| rotation tightness (`fRotTightness`) | 8 /s |
| distance recovery tightness (`fMoveBackTightness`) | 2.5 /s |
| position tightness (`fMoveTightness`) | 0 — i.e. the position snaps, no positional lag |
| camera pitch limits | -14000 .. +14000 units (+-76.9 deg) |
| per-tick mouse clamp | yaw +-20000, pitch +-10000 units |
| pitch pull-in threshold / spread | 0 / 10000 units |
| minimum distance scalar | 0.15 (so the arm never goes below 19.2 uu) |
| position/height carried over from spawn | distance 80, height 120, camera speed 2 (only used by the states that still run the old positioner) |

Per tick, in order:

1. `destination yaw += SmoothMouseX * dt * 4`, clamped to a +-20000-unit step; `destination pitch += SmoothMouseY *
   dt * 4`, clamped to a +-10000-unit step, then the destination pitch itself is clamped to +-14000 units.
2. The current camera rotation closes the gap to the destination by `min(1, 8 * dt)` each tick (an exponential
   approach with a time constant of 1/8 s, clamped so a long frame cannot overshoot). The camera's rotation is then
   applied and also stored as its desired rotation. Roll is held at 0.
3. The camera position is the look-at point (Harry + 55 uu up, no smoothing of its own) minus the current arm length
   along the camera's own forward direction. With position tightness 0, the camera is placed there exactly — all the
   softness in the mod's camera is in the rotation, none in the position.
4. A trace from the look-at point to the camera pulls the camera in to the first world hit plus a 5 uu cushion, and
   shortens the arm to that distance for the frame.
5. Distance scalar: when the camera's pitch is above 0 (camera low, looking up), the arm shrinks by
   `1 - pitch/10000`, floored at 0.15 — so looking up from below pulls the camera in towards Harry. Shrinking is
   immediate; growing back is an exponential approach at 2.5/s.

No field-of-view change with speed, and no camera shake, in either version.

### 2.3 Other broom-adjacent camera changes (only reachable in the leftover states)

For completeness, in the old positioner that `QuidditchState`, `FarState`, `Highstate` etc. still use:

- The "try six alternative camera positions when Harry cannot be seen" loop is now dead: the loop always declares
  the first position good, so the camera never cycles to an alternative any more.
- A new rule lifts the goal point when it comes within 50 uu of Harry (raise by the shortfall), instead of letting
  it clip in.
- A 50 uu dead zone: the camera only moves while its goal is more than 50 uu away and then stops 50 uu short. In
  retail that behaviour existed only while aiming.
- The camera shake offset was removed from the positioner.
- `QuidditchState` no longer sets camera height 60 / distance 150 on entry (it inherits whatever the previous state
  left, 80/120 from spawn), and its latent `TurnToward(aim target)` loop was removed — which, since the state no
  longer writes a desired rotation either, would leave that camera's rotation frozen. This is further evidence that
  the state is not meant to be used any more.

### 2.4 Event camera switches removed on the broom

- **Dying on the broom** no longer switches the camera to the top-down state.
- **Catching the snitch** no longer switches the camera to the reverse state, and no longer moves the aim target to
  (-100, 10, 50).
- Catching also no longer clears the look-at target when the caught actor is the one Harry was looking at.

## 3. What is unchanged

- **All broom speed behaviour.** `AirSpeedNormal` 400, `AirSpeedBoost` 800, the constant 200000 forward thrust, the
  boost/brake rule (brake beats boost), the hard-turn deceleration (threshold 0.2, ramp 80 uu/s per second, cap 200,
  release 1000 then 80), the destination formula. `PlayerMove` is byte-identical apart from the rotation call.
- **All broom input variables.** `HPBase.baseHarry`'s variable list is unchanged: `aBroomYaw`, `aBroomPitch`,
  `bBroomYawLeft/Right`, `bBroomPitchUp/Down`, `bBroomBoost`, `bBroomBrake`, `bBroomAction`, `bAllowBroomMouse`,
  `bInvertBroomPitch`, `bScreenRelativeMovement`. Nothing added, nothing removed, no default changed. The mod ships
  no `.ini`, so the retail bindings above still apply.
- `BroomHarry.PlayerInput` (the action/boost/brake edge tracking), `UpdateBroomSound`, `DeterminePrimaryAnim`,
  `PlayFastWhooshSound`/`PlaySlowWhooshSound`, `HitWall`, `TakeDamage`, `PostBeginPlay` (including every runtime
  tuning value), `FlyOnPath`/`GetOnPath`/`StopFlyingOnPath`, and the `Pursue`, `FlyingOnPath`, `Hit` and
  `CutIdleing` states.
- Every `BroomHarry` default except `RotationRate.Yaw` (and that one is overwritten at spawn).
- **`HarryPotter.QuidCam` is byte-identical** (follow distance 60, height 60, camera speed 4, camera rotation speed
  30, lock bias 1, rotation rate 60000/60000/20000) and **`HarryPotter.QuidPlayer` is byte-identical** (damage 2,
  rotation rate pitch 18000, yaw 25000, roll 5000). Neither the Quidditch spectator camera nor the AI flyers were
  touched.
- `baseChar`'s path-following movement (the AI flyers' spline movement) differs only in local-variable letter case,
  a recompile artefact.
- The level script packages (`Hub2-5`, `Tut1`) carry no broom or camera change; they were only recompiled.

## 4. What could not be determined

- **`MouseSensitivity`.** The mod's whole steering gain is `SmoothMouseX/Y`, which is the raw mouse delta times the
  binding speed 6 times `MouseSensitivity * FOVScale`. `MouseSensitivity` is in the player's own `User.ini`/`HP.ini`
  and is not in the shipped `DefUser.ini` or `Default.ini`, so no absolute degrees-per-inch figure can be given
  from the packages. Everything below is therefore expressed relative to a gain constant to be fitted by feel.
- **Frame-rate dependence.** `SmoothMouseX` is a per-tick delta, and the mod multiplies it by `dt` again, in both
  the pawn and the camera. That makes the effective sensitivity proportional to the frame time: at 30 fps the same
  physical mouse motion turns the broom about twice as far as at 60 fps. Whether this was intended, and which frame
  rate the author tuned at, cannot be established from the packages. A port should not copy it.
- **Which value actually dominates the broom's facing in practice.** The broom takes the camera's rotation and then
  the engine steps it towards the mouse-accumulated view rotation at `RotationRate`. Both terms are driven by the
  same mouse, with the camera lagging at 1/8 s, so without running the game it cannot be said how much of the
  broom's visible heading is the camera's smoothed rotation and how much is the engine's rate-limited step. The
  behaviour is bracketed, though: no faster than 274.7 deg/s yaw and 131.8 deg/s pitch, and no snappier than the
  camera's 1/8 s rotation approach.
- **Whether the mod author intended `QuidditchState` to be dead.** The forced camera type in `StartState` makes it
  unreachable in normal play, but the state was still edited (the aiming branch cleanups), so it may be an
  oversight rather than a decision.
- **Banking/roll.** Neither version writes roll on the broom; it comes from the engine's pawn rotation step (bank
  from lateral acceleration, capped by `RotationRate.Roll` = 6000 units = 33 deg, eased in at 5x the frame time, per
  `flipendo-droid/docs/re/engine/physics.md`). The mod changes nothing here, but the exact engine curve was not
  re-derived for this note.
- The German retail packages are not the build the mod was made from (the mod's `Harry` and `HPBase` show a reworked
  spell-casting and HUD path, a removed `playeraiming` state and added casting states). Those differences are
  outside the broom and were not separated into "mod change" vs "build difference"; the broom findings above are
  unaffected, because every broom-side difference is a coherent, deliberate edit.

## 5. Recommended mapping onto our pure model

Current state of our code: `flight.rs` has `BroomInput { yaw, pitch, boost, brake }` as -1..1 stick deflections and
`step` integrates yaw at `yaw_rate`, pitch at `pitch_rate` clamped to `max_pitch`, and moves speed towards a target
at `accel`. `camera.rs::chase_camera` sits `DIST` = 420 behind and `HEIGHT` = 140 above the broom, lags only yaw
(exponential `TURN_LAG` = 6/s, capped at `MAX_TURN` = 3.2 rad/s), follows pitch and height immediately, and widens
the FOV with speed. `input.rs::desktop_input` turns keys plus mouse pixels into stick deflections with
`MOUSE_FULL` = 12 px for full deflection.

Our model is already the *retail* shape (sticks integrate the broom's own heading, the camera follows). The mod's
shape is the opposite: the mouse integrates a free look direction and the broom chases it. The changes below adopt
the mod's feel without copying its quirks.

### 5.1 `input.rs` — give the mouse a look direction instead of a stick

- Add a mod-style path that treats mouse motion as an **angular delta** rather than a stick deflection: a new field
  pair on the input struct (e.g. `look_yaw` / `look_pitch`, radians this frame) filled as
  `pixels * MOUSE_GAIN`, with `MOUSE_GAIN` a per-pixel radian constant and **no `dt` factor** (mouse pixels are
  already a per-frame quantity — this is the mod bug from section 4 that we must not reproduce).
- Keep the keys on the existing stick fields: they are what the original used and they still work.
- Keep the boost toggle and the held brake exactly as they are (retail and mod agree, and the toggle is our own
  phone-friendly addition).
- **Do not** feed mouse yaw into the brake. Our `BroomInput.brake` should stay the brake button only. The mod's
  "every turn brakes" is a side effect of dead code, not a design.
- `invert_pitch` keeps applying to both paths; retail inverts mouse pitch at the binding (speed -6) and our sign
  convention already matches that.

### 5.2 `flight.rs` — add a rate-limited heading chase

- Add to `BroomState` a target heading: `want_yaw`, `want_pitch` (radians), the mouse-integrated look direction.
- In `step`, when look input is present: `want_yaw += look_yaw`, `want_pitch = (want_pitch +
  look_pitch).clamp(-max_look_pitch, max_look_pitch)`. Use **`max_look_pitch` = 76.9 deg = 1.342 rad** (the mod's
  camera clamp of 14000 units), which replaces retail's 60 deg for the mod path. Keep `max_pitch` = 1.2 rad for the
  key path if we keep it.
- Then turn the broom's actual `yaw`/`pitch` towards `want_*` with **both** a tightness and a rate cap, which is
  what the mod's double path (camera smoothing plus engine `RotationRate`) adds up to:
  - tightness: close `min(1, chase_tightness * dt)` of the gap, `chase_tightness` = **8.0 /s** (the mod's
    `fRotTightness`; a 1/8 s time constant),
  - cap: never more than **`yaw_rate_max` = 4.80 rad/s (274.7 deg/s)** and **`pitch_rate_max` = 2.30 rad/s
    (131.8 deg/s)** — the mod's effective `RotationRate` yaw 50000 / pitch 24000. Note these are *much* faster than
    our current `yaw_rate` 2.0 / `pitch_rate` 1.5 rad/s, which is the retail 110 deg/s / 132 deg/s stick rate;
    keep the old numbers for the key path and use the caps only for the look path.
  - use `wrap_pi` for the yaw gap so it always takes the short way round.
- Keep the key path exactly as it is (`yaw_rate` 2.0 rad/s = 114.6 deg/s is retail's 110 deg/s, close enough) and
  **do not** add retail's 4/3 coasting bonus: the mod removed it.
- **Do not** add retail's pitch auto-levelling to the look path — the mod's pitch is free and holds where the mouse
  left it. Keep auto-levelling (or its absence) for the key path as it is today.
- Speed: nothing to change for the mod. If we ever want retail-accurate speed, the numbers are cruise 400,
  boost 800, floor 200, brake ramp 80 uu/s per second with a 1000-then-80 release — but our 600/1200/150 half-speed
  tuning was a deliberate playtest decision and the mod does not touch speed at all, so leave it.
- `BroomTuning` additions: `chase_tightness`, `yaw_rate_max`, `pitch_rate_max`, `max_look_pitch`.
- `confine`'s "level out against the floor and ceiling" should also clear `want_pitch`, or the broom will keep
  fighting the wall.

### 5.3 `camera.rs` — much closer, and lag the whole rotation, not just yaw

The mod's camera is the single biggest change to how flying feels, and ours is shaped quite differently.

- **Arm length 128 uu, not 420.** Add a pull-in: when the camera pitch is above level, scale the arm by
  `1 - pitch_units/10000` floored at **0.15**; in radians that is `1 - pitch/0.9587`, i.e. the arm reaches its floor
  of 19.2 uu at 8500 units = 46.7 deg of camera pitch. Shrink immediately, grow back with an exponential approach at
  **2.5 /s**.
- **Look-at point: the broom plus 55 uu up** (we scale it with our longer arm, and now also stand the camera
  off the arm: `AIM_UP` = 116 with a `RISE` of 30, so the camera is 146 uu over the rider and looks 8.5 deg
  down at him -- see `docs/formats/quidditch-arena.md`, which is where our rig is written down).
- **Camera rotation is its own state, lagging the look direction in both axes**, not derived from the broom's
  heading: keep a `CamState` rotation that closes `min(1, 8.0 * dt)` of the gap to the *look* direction
  (`want_yaw`/`want_pitch`), with the per-step clamps +-20000 units yaw (1.917 rad) and +-10000 units pitch
  (0.959 rad). That replaces `TURN_LAG`/`MAX_TURN` and the "pitch follows at once" rule; `PITCH_FOLLOW` and
  `MIN_ABOVE` go away, since the arm direction now carries the pitch.
- **No positional lag.** The mod's position tightness is 0 — the camera is placed exactly on the arm every frame.
  Our camera already derives position from the lagged yaw, which is equivalent in spirit; just make sure the
  position is computed from the *camera's* rotation, not the broom's.
- **Keep the arm trace.** The mod traces look-at -> camera against the world and pulls in to the hit plus a 5 uu
  cushion. We have the arena bounds in `arena.rs`; at minimum clamp the camera position into the flight volume with
  a 5 uu cushion so it does not end up inside the stands.
- **Clamp camera pitch to +-14000 units (+-76.9 deg)**, which is what keeps the mod's camera out of the ground and
  off the top of Harry's head.
- **FOV: the mod has none.** Our speed-widening FOV (90 -> 100 deg at 1200 uu/s) is our own; it is not in either
  package. Keep it if the playtest likes it, but know it is not "mod-accurate".
- **Drop the event camera switches we do not have**, and do not add them: the mod removed the top-down death camera
  and the reverse snitch-catch camera. If `presentation.rs` ever grows them, make them opt-in.

### 5.4 Deliberately not ported

- The `dt`-squared mouse sensitivity (section 4).
- Yaw input feeding the brake, and the arrow keys braking without steering.
- The `RotationRate.Yaw` 70000 default (dead code).
- The two clamps in the pawn that never fire.
- Wall self-avoidance and the reversal statistics being computed but unused — if we want wall avoidance, our
  `boundary_turn` already does the job properly, on the value that actually steers.
