# Quidditch match in r3paro: design

Status: draft for review, 2026-11-11. Nothing is built yet.

## Intent

Add a playable, full Quidditch match to r3paro (the clean-room HP3 engine, `hp3-engine`). The player flies Harry on a broom in an
HP3 stadium against an AI team. HP1's Quidditch is the model for the *feel* (broom controls, snitch chase, Bludger
bumps, Lee Jordan-style commentary, league structure); HP3 supplies the stadium, models, sounds and music.

Why: HP3 has no playable Quidditch (its `HP3_Quidditch` map is only the Dementor cutscene and its Quidditch classes have empty
scripts), and the owner wants a match on the HP3 map.

## What the owner decided

- The mode is a **true full match**: Quaffle, goal hoops, Chasers, Beaters, Keepers and the Seeker chase, not just HP1's
  seeker-only chase. (Offered the faithful seeker-only option; chose the full match.)
- It plays **on an HP3 map** with HP1's rules and feel as the reference.
- r3paro stays clean-room (`CLEANROOM.md`): HP1 and Flipendo-Droid are read for behaviour, then our own code is written.

## Goals

1. A match that starts, plays and ends: kickoff, Quaffle play, goals, Bludgers, snitch release and catch, final score, win or loss.
2. Harry flies a broom with the HP1 controls (steer, boost toggle, brake) on desktop and with touch controls on the phone.
3. AI teammates and opponents that play recognisable roles (Chasers pass and shoot, Keepers block, Beaters hit Bludgers, Seekers chase the snitch).
4. HUD (score, snitch progress, controls hints), crowd, commentary and music that make it read as Quidditch.
5. Runs on the phone at a playable frame rate (the owner measures fps on the phone).

## Non-goals

- Network play, replays, a team editor, other characters as the player.
- Matching HP1's unknown numeric tunables exactly (they are not readable from compiled data).
- Changing the existing story maps or the HP3 cutscene.

## Anti-goals (failure even if it "works")

- Ships or depends on HP1 data or Epic code. (HP3 assets are loaded from the user's own install, as with the rest of r3paro.)
- Breaks any of the 34 maps' current 0-script-error status.
- Needs a fix to the VM that regresses existing gameplay (Carpe, Glacius, etc.).

## Known facts (from recon)

HP1 (reference only; compiled, no numbers available):
- It is a seeker-only snitch chase. `QuidditchReferee` runs: intro cutscene, snitch visible, opposing seeker joins the
  pursuit, a progress bar that rises near the snitch and falls when far, timed catch attempts with a try limit, then a score and
  commentary. Bludgers launch at Harry inside a proximity radius at random intervals and bump him; enough bumps make him
  lose control and spiral. Story matches: Slytherin, Ravenclaw. League: Slytherin, Ravenclaw, Hufflepuff, twice each, then a final.
- Broom inputs: mouse or keys for yaw and pitch (pitch invertible), boost (held or toggled), brake. Boost only applies when
  brake is off. Broom sound pitch and volume follow speed.
- Quaffle exists only as a prop in HP1. **Quaffle play, hoop scoring, Chaser, Beater and Keeper behaviour are new design here**;
  HP2's script source (`flipendo-droid/reference/hp2/ScriptSource/hgame/Classes`, `BEATER.uc`, `BlackboardQuidditch.uc`) is a
  closer reference for team roles, subject to the clean-room rules (read for behaviour, do not copy).

HP3 (retail data in `spike-hp3`):
- Present: pitch and stands (`Quid_Stadium` static meshes, `Quid_GoalPosts_SM`, `Quid_GoldHoop`), brooms (`QuidBroom_SM`, skeletal
  `HP2QUID_Broom`, `HP3Broom_*`), `HarryQuid` and `QuidPlayer{Griff,Sly}{Male,Female}_KW` skeletal meshes with broom animations,
  the Snitch mesh, halo glow textures, rain and cloud emitters, broom, snitch, Bludger, crowd and weather sounds, and three
  Quidditch music tracks.
- Missing: Quaffle and Bludger meshes, any Quidditch script behaviour (the classes in `HGame.u` have empty bodies), and
  possibly Quidditch HUD textures (not exhaustively searched). Hoop meshes beyond `Quid_GoldHoop` and `Quid_GoalPost` are unconfirmed.
- Unconfirmed: exact pitch bounds, flight ceiling, whether `Quid_GoalPosts_SM` already includes hoops.

Engine (`hp3-engine`):
- Flying physics exists (`hp3-world/src/physics.rs`, `PHYS_FLYING`), but a human pawn is capped near walking speed, so the broom
  needs its own movement. Camera comes from the script's `PlayerCalcView` (`hp3-world/src/lib.rs::player_view`).
- No game-mode registry. Rust-side gameplay has precedent (`carpe_tick` in `hp3-world/src/natives.rs`, `interpolation.rs`,
  `character_switch.rs`); `hp3-game` is an empty crate. Natives and overrides register through `vm.register` and `vm.register_override`.
- The only HP3 Quidditch map is the cutscene map. Using a different (or modified) map is a builder decision, see below.

## Constraints

- Rust, `hp3-engine` conventions; do not run `cargo fmt --all` (the code is hand-formatted).
- Android arm64 build must keep working; the phone is the performance target.
- Retail data is read-only; new content must be engine-generated or loaded from the user's own HP3 install.

## Decided after the builder check (owner's answers)

- **Player role:** Seeker only, shipped in stages. The first playable is the snitch chase plus Bludgers. Quaffle, goals and
  Chaser, Beater and Keeper AI follow as later stages; the match counts as complete once they are in.
- **Phone frame rate:** target 60 fps on the owner's phone. That means a small default team on the phone and cheap proxy
  models for distant players; desktop can run the full 7 per side. Call 4 below (team size) is superseded by this: the phone's default
  active team size is whatever holds 60 fps, chosen by measurement.

## My calls (the owner did not decide these; change any of them)

1. **Where the code lives:** a new Rust module in `hp3-world` (or the empty `hp3-game` crate) that owns a `QuidditchMatch`
   state machine, ticked like `carpe_tick`, plus overrides on the existing `Snitch`, `HarryQuid` and `QuidPlayer*` classes. No script.
2. **Map:** build the arena from the HP3 `Quid_Stadium` static meshes and terrain in a new engine-side arena (spawned from
   the stadium parts), because the retail map is a cutscene stage with no usable pitch layout. Exact approach is the builder's.
3. **Missing balls:** Quaffle and Bludger render as simple engine-built meshes (a sphere, with the existing halo sprites for the
   Bludger glow) with no new art needed. Replaceable later.
4. **Team size:** start with 3 Chasers, 2 Beaters, 1 Keeper and 1 Seeker per side (the standard 7), degraded to fewer active AI
   if the phone cannot keep up.
5. **Scoring:** goal 10 points, snitch catch 150 points, ends on catch (standard rules). Match length is untimed, as in HP1.
6. **Player role:** the player is the Seeker only (as in HP1), so the Chaser, Beater and Keeper roles are AI only. This keeps
   controls to the HP1 broom scheme.
7. **Order of work:** broom flight and camera first, then the snitch chase (HP1 core), then Bludgers, then Quaffle and goals,
   then team AI, then HUD, commentary and league wrapper.
8. **Numbers:** all speeds, radii and timings are tuned by feel; HP1's exact values are unknown and not a goal.

9. **Arena feasibility (my call):** step 0 of the plan is a spike that inventories the `Quid_Stadium` meshes (assembled or loose
   parts, pitch bounds, ceiling, hoops). We are free to choose pitch dimensions; if hoops are missing, primitive rings are acceptable.
10. **Launch and exit (my call):** a debug entry first (`HP3_MATCH=<opponent>` env var and a command), then a menu entry
    in the existing front end. Exit returns to the menu. The league (6 rounds plus final, persisted like the existing saves)
    is optional and comes last.
11. **Teams (my call):** Harry plays Gryffindor. The first opponent is Slytherin, because both team meshes exist. Ravenclaw and
    Hufflepuff are recolors of existing skins, added with the league.
12. **Acceptance (my call):** a headless `hp3-run` smoke match that runs to a result with 0 script errors, the 34-map
    regression, and the owner's manual playtest on desktop and phone.
13. **Existing classes (my call):** overrides on `Snitch`, `HarryQuid` and `QuidPlayer*` apply only while a match is active,
    so the Dementor cutscene map is untouched.
14. **Touch controls (my call):** reuse the steering stick and boost-toggle scheme already built for the HP1 broom
    in Flipendo-Droid (`docs/android.md`, "Flying the broom") on top of `apps/hp3/src/touch.rs`.

## Left to the builder

- Arena construction method, collision details, AI implementation (behaviour trees, steering, or simple state machines).
- Data structures, module boundaries, tests, and the touch-control layout.
- Commentary line selection (HP3's Quidditch commentary audio availability is unconfirmed).

## Open risks

- No Quaffle or Bludger assets, and unknown HUD assets.
- Team AI at 14 flyers plus balls on a phone may need a reduced-count mode.
- Flight physics for a human pawn needs its own speed handling in `hp3-world`.
