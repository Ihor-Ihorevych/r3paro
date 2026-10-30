# Roadmap and working method

Live status is in `status.md`. The long task list the project started from is the master plan kept with the
maintainer; this file holds the rules for how work is done and the next steps.

## Working method (every engine feature, every time)

1. **Investigate first.** Before writing an engine behaviour (camera, input, physics, animation, movement,
   collision, cutscenes, particles, audio...), read how the reference engine does it: `flipendo-droid-2004/Source`
   (the maintainer's own repo; its HP3 changes sit in `UE_HP3_LAYOUT` blocks), and the game's own scripts and
   data. Note the behaviour: inputs, order of steps, thresholds, which script events are sent.
2. **Then reimplement it your own way.** Never transcribe or paraphrase line by line. Write the behaviour fresh
   in this codebase's structure (Rust, the VM's natives, `hp3-world` modules), with its own names, helpers and
   comments. Leave a one-line note of which behaviour it re-expresses.
3. **Check it against the game.** Headless probes (`hp3-run` with `HP3_HOLD`, `HP3_MOUSE`, `HP3_PAWN_LOG`,
   `HP3_WALKABOUT`, `HP3_TRACE_CALLS`...), renders (`hp3-view play`, `HP3_SOAK`), the all-maps smoke test, and the
   app itself.
4. Commit small steps; keep all 34 maps at 0 errors and the train intro ending.

This is permitted by `CLEANROOM.md` (item 5) and confirmed by the maintainer. It is about behaviour, not code.

## In progress: match the original's camera, movement and cutscenes

- Phase A (done): per-actor tick order, time dilation and pause, the original's input maths, mouse look through
  the game's camera script, Escape menu, wheel, numpad.
- C1/D1 (done): pawn turning toward desired rotation, controllers face their focus, MoveTo focus and walk,
  FinishRotation waits.
- Phase B (done): tweened animation changes, one-shots hold their last pose, TweenAnim, FinishAnim by channel,
  AnimEnd to one receiver.
- Movement animation (done): the engine keeps the walk/run/strafe channels (4-7) and turn-in-place channels (2-3)
  playing and fades them by acceleration direction at one shared pace; it sends the script `ChangeAnimation`,
  `PlayJump`, `PlayFalling` and `PlayLandingAnimation`. The invented root lock and the `SetPhysics` hack are gone.
- Next: animation notifies and the HP3 channel natives, AnimEnd on every loop of a channel; walking rules (accel cap, friction, sub-steps, ledges, floor `Base`); falling and landing
  events; collision rules (`bBlockActors` both ways, cylinders, `Bump`, `Attach`/`Detach`/`BaseChange`); movers
  (OldPos interpolation, push/carry, `KeyFrameReached`); `Mount` ledge climbing; MoveTo arrival and slow-down.

## Later

Saves, Bink video, remaining GUI widgets, companions, spell effects and the cast ray, Android integration.
