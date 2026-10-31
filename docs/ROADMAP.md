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

## Spells: everything enchantable reacts like the original

Done: touch rules, animation notifies, spell stats from `Spells.ini`, blocking rules, projectile `HitWall`,
trailer physics, `ConstantAcceleration`, `SetPhysics` dropping the base, `SetBase` events. Checked with the cast
harness (`HP3_CAST=<Spell>:<TargetClass>:<n>@<tick>` in `hp3-run`, see `tools/hp3-run/src/casttest.rs`):
Alohomora (chests in four maps, padlock), Depulso (knight spawners, `KnightCollapse`, pumpkin spawner, flying
books), Lumos (gargoyles), Spongify (pads), Rictusempra (Poxie, crate spawner, FireCrab, Bundimun), Glacius
(salamander base), Lapifors and Draconifors (statues; the player takes over the dragon), Carpe Retractum (hook
swing, pull-and-drop with hang and release, item pull).

Also done and checked: blocked moves send `Bump` to both actors; `Trace`, `FastTrace`, `TraceActors` and the four
radius iterators follow the reference's filters; Patronum (charged on a bell target, charge level decides freeze or
knock-back on a Dementor, overcharge fails the cast); the dragon's fire (pick up a fire powerup, fire at a
`SpellTrigger`: `HP3_CAST_FLAME`); a character bouncing on an enabled Spongify pad (`HP3_CAST_DROP`).
Verdimillous and Incendio have no target anywhere in the game data (stubs / sound-table entries only).

Open, in the order they matter:
- Static meshes with a collision model answer box traces with it (`UseSimpleBoxCollision`, on unless the mesh turns
  it off) and line traces when `UseSimpleLineCollision` is set; otherwise the collision triangles are used. Still
  missing: `bUseCylinderCollision` actors (a cylinder instead of the mesh).
- Creatures whose spell target is a companion-cast proxy (`CompanionSpellTrigger`: skeletons, knight collapses)
  need Ron or Hermione nearby to react; companions in the hubs still follow imperfectly.
- Peeves reacts only inside his scripted encounter (he starts hidden and disabled), so his spell handling is not
  covered by the harness yet.

## Later

Saves, Bink video, remaining GUI widgets, companions, spell effects and the cast ray, Android integration.
