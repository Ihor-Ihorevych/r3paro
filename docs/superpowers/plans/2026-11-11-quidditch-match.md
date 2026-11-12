# Quidditch Match Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A playable Quidditch match in r3paro: Harry (Seeker) flies a broom in an HP3 stadium; stage 1 is the snitch chase plus Bludgers, stage 2 adds Quaffle, goals and team AI.

**Architecture:** Match rules, flight, snitch, Bludger and AI are pure Rust in a new `hp3-world/src/quidditch/` module with no VM dependency, so each is unit-tested. One thin glue file (`quidditch/glue.rs`) maps that state onto VM actors and is ticked from `hp3-world/src/lib.rs::tick` right after `natives::carpe_tick`. The arena is spawned engine-side from the `Quid_Stadium` static meshes; the Quaffle and Bludger are generated meshes. No UnrealScript is written (the retail Quidditch classes have empty bodies).

**Tech Stack:** Rust 2024, workspace `hp3-engine` (`hp3-world`, `hp3-render`, `apps/hp3`, `tools/hp3-run`), glam, wgpu, SDL3.

**Spec:** `docs/superpowers/specs/2026-11-11-quidditch-design.md`

## Global Constraints

- Clean-room (`CLEANROOM.md`): HP1 and Flipendo-Droid are read for behaviour only; write our own code; ship no HP1 data or Epic code.
- Never run `cargo fmt --all`; keep hand formatting. Commit with explicit `git add <paths>`: the working tree already has unrelated uncommitted changes.
- Player is the Seeker only. Harry plays Gryffindor; first opponent Slytherin.
- Scoring: goal 10, snitch catch 150, match ends on the catch, no clock.
- Teams of 7 (3 Chasers, 2 Beaters, 1 Keeper, 1 Seeker); the phone's default active team size is whatever holds **60 fps** on the owner's phone, found by measurement. Desktop may run the full 7.
- Broom controls: yaw, pitch (invertible), boost toggled by one tap, brake; boost applies only while brake is off.
- Overrides on `Snitch`, `HarryQuid`, `QuidPlayer*` apply only while a match is active; the `HP3_Quidditch` cutscene map must be unchanged.
- Launch: `HP3_MATCH=<opponent>` env var and a command first, menu entry later. League is optional and last.
- Acceptance: headless smoke match with 0 script errors, `hp3-run levels` over all 34 maps still 0 script errors, owner's playtest on desktop and phone.
- Android arm64 build must keep working. Retail data is read-only.

## Review Focus

- Harry hits the pitch, stands or terrain while flying: he must bump and slide, not tunnel through or get stuck.
- Harry leaves the stadium area: soft boundary turns him back; no `FellOutOfWorld` kill.
- Snitch is caught or lost at the moment a Bludger hits: outcome is deterministic and the match never double-ends.
- The match is entered twice in a session, or quit mid-match: all match actors are gone and state resets.
- Touch input with no finger down, two fingers, or the app pausing: Harry holds course, boost state survives a pause.

## File Structure

- `crates/hp3-world/src/quidditch/mod.rs`: `QuidditchMatch` (owns everything below), `MatchPhase`, tick entry.
- `quidditch/flight.rs`: broom movement model (pure).
- `quidditch/rules.rs`: phases, score, end conditions (pure).
- `quidditch/snitch.rs`: snitch path/AI, progress bar, catch attempts (pure).
- `quidditch/bludger.rs`: Bludger launch, pursuit, hit (pure).
- `quidditch/quaffle.rs`, `quidditch/goals.rs`: stage 2 (pure).
- `quidditch/ai.rs`: team roles (pure steering plus per-role state machines).
- `quidditch/arena.rs`: arena layout constants and the spawn list built from the spike.
- `quidditch/glue.rs`: the only file that touches the VM; maps state to actors, camera and sounds.
- `quidditch/commentary.rs`, `quidditch/hud.rs`: event-to-line selection; HUD draw list.
- Modify: `hp3-world/src/lib.rs` (tick hook near line 1112, `player_view` near 919), `hp3-world/src/input.rs` (`apply`), `hp3-world/src/touch.rs`, `apps/hp3/src/touch.rs`.
- Create: `tools/hp3-run/src/quidarena.rs` and `quidsmoke.rs`; `docs/formats/quidditch-arena.md`.

---

## Stage 1: the snitch chase

### Task 1: Arena spike (inventory)

**Files:** Create `tools/hp3-run/src/quidarena.rs`, `docs/formats/quidditch-arena.md`; Modify `tools/hp3-run/src/main.rs` (new subcommand).

**Interfaces:**
- Produces: `hp3-run quid-arena <HP3 folder>` prints, for each static mesh in package `Quid_Stadium`, its name, bounding box and triangle count; and for `HP3_Quidditch.unr` the terrain extents. The doc records: whether parts are assembled or loose, pitch bounds, ceiling, whether hoops are in `Quid_GoalPosts_SM`, and the chosen arena constants.

- [ ] **Step 1:** Implement `fn run(args: &[String]) -> Result<(), String>` in `quidarena.rs` using `hp3-assets` mesh decoding; print one line per mesh `name  min  max  tris`.
- [ ] **Step 2:** Run `cargo run -p hp3-run --release -- quid-arena ../spike-hp3`. Expected: lists `Quid_GoalPosts_SM`, `QUid_Pitch_SM`, `Quid_Tower_SM`, `Quid_GoldHoop` with finite boxes.
- [ ] **Step 3:** Write `docs/formats/quidditch-arena.md` with the findings and the constants Task 9 needs (pitch centre, half-extents, ceiling, hoop positions and radius, tower positions). If no hoop mesh sits on the goal posts, record "primitive rings at height H".
- [ ] **Step 4:** Commit `git add tools/hp3-run/src docs/formats/quidditch-arena.md && git commit -m "spike: Quid_Stadium inventory"`.

### Task 2: Broom flight model

**Files:** Create `quidditch/flight.rs`, `quidditch/mod.rs`; Modify `hp3-world/src/lib.rs` (`mod quidditch;`).

**Interfaces:**
- Produces: `pub struct BroomInput { pub yaw: f32, pub pitch: f32, pub boost: bool, pub brake: bool }` (yaw, pitch in -1..1); `pub struct BroomState { pub pos: Vec3, pub vel: Vec3, pub yaw: f32, pub pitch: f32 }`; `pub struct BroomTuning { pub speed: f32, pub boost_speed: f32, pub accel: f32, pub decel: f32, pub yaw_rate: f32, pub pitch_rate: f32, pub max_pitch: f32 }` with `Default`; `pub fn step(s: &BroomState, i: &BroomInput, t: &BroomTuning, dt: f32) -> BroomState`.

- [ ] **Step 1:** Write failing tests in `flight.rs`: `boost_raises_target_speed` (steady speed with boost > without, equal accel window), `brake_overrides_boost` (boost and brake together gives non-boosted-or-lower speed), `pitch_is_clamped_to_max_pitch`, `no_input_holds_heading`, `step_is_deterministic` (same inputs, same output).
- [ ] **Step 2:** `cargo test -p hp3-world quidditch::flight` Expected: FAIL (not defined).
- [ ] **Step 3:** Implement `step`: speed eases toward `speed`/`boost_speed`/`decel` target by `accel`; yaw and pitch integrate rate times input; velocity = forward direction times speed. Tuning defaults are placeholders tuned by feel in Task 11.
- [ ] **Step 4:** `cargo test -p hp3-world quidditch::flight` Expected: PASS.
- [ ] **Step 5:** Commit `git add crates/hp3-world/src/quidditch crates/hp3-world/src/lib.rs`.

### Task 3: Snitch AI, progress and catch

**Files:** Create `quidditch/snitch.rs`.

**Interfaces:**
- Consumes: `BroomState` from Task 2.
- Produces: `pub struct SnitchTuning { pub neutral_radius: f32, pub max_gain_radius: f32, pub max_loss_radius: f32, pub gain_rate: f32, pub loss_rate: f32, pub max_catch_time: f32, pub max_tries: u32, pub catch_radius: f32 }` with `Default`; `pub enum SnitchEvent { ClosingIn, Lost, CatchStart, Caught, Missed, OutOfTries }`; `pub struct Snitch` with `pub fn new(seed: u64, arena: &ArenaBounds) -> Snitch`, `pub fn step(&mut self, harry: &BroomState, wants_catch: bool, dt: f32) -> Vec<SnitchEvent>`, `pub fn pos(&self) -> Vec3`, `pub fn progress(&self) -> f32` (0..1). `ArenaBounds { pub center: Vec3, pub half: Vec3, pub ceiling: f32 }` is defined in `arena.rs` (Task 9 fills the values; tests use a literal).

- [ ] **Step 1:** Failing tests: `progress_rises_when_close`, `progress_falls_when_far`, `progress_holds_in_neutral_band`, `losing_snitch_zeroes_progress` (event `Lost`), `catch_succeeds_inside_catch_radius_when_wants_catch`, `catch_fails_after_max_catch_time` (event `Missed`), `out_of_tries_after_max_tries`, `snitch_stays_inside_bounds_for_10000_steps`, `same_seed_same_path`.
- [ ] **Step 2:** Run `cargo test -p hp3-world quidditch::snitch`. Expected: FAIL.
- [ ] **Step 3:** Implement: snitch flies between random in-bounds waypoints, faster when Harry is near, deterministic from `seed`; progress and catch as in the tests.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit.

### Task 4: Bludgers

**Files:** Create `quidditch/bludger.rs`.

**Interfaces:**
- Consumes: `BroomState`, `ArenaBounds`.
- Produces: `pub struct BludgerTuning { pub launch_inner: f32, pub launch_outer: f32, pub launch_min: f32, pub launch_max: f32, pub speed: f32, pub velocity_compensation: f32, pub hit_radius: f32, pub life: f32, pub bumps_to_lose_control: u32 }`; `pub enum BludgerEvent { Launched, Hit, Missed }`; `pub struct Bludgers` with `pub fn new(seed: u64) -> Bludgers`, `pub fn step(&mut self, harry: &BroomState, dt: f32) -> Vec<BludgerEvent>`, `pub fn positions(&self) -> Vec<Vec3>`, `pub fn bumps(&self) -> u32`, `pub fn control_lost(&self) -> bool`.

- [ ] **Step 1:** Failing tests: `no_launch_outside_outer_radius`, `launches_inside_proximity_at_random_interval_within_min_max`, `bludger_expires_after_life_as_missed`, `contact_inside_hit_radius_is_hit_and_adds_a_bump`, `control_lost_after_bumps_to_lose_control`, `hit_and_expire_same_step_counts_once`.
- [ ] **Step 2:** Run `cargo test -p hp3-world quidditch::bludger`. Expected: FAIL.
- [ ] **Step 3:** Implement launch aimed at Harry's position plus `velocity_compensation` of his velocity.
- [ ] **Step 4:** Run. Expected: PASS.
- [ ] **Step 5:** Commit.

### Task 5: Match rules and phases

**Files:** Create `quidditch/rules.rs`; Modify `quidditch/mod.rs`.

**Interfaces:**
- Consumes: `SnitchEvent`, `BludgerEvent`.
- Produces: `pub enum MatchPhase { Intro, Seeking, Catching, Ended(Outcome) }`; `pub enum Outcome { Won, Lost }`; `pub struct Score { pub home: u32, pub away: u32 }` with `pub const GOAL: u32 = 10; pub const SNITCH: u32 = 150;` `pub struct Rules` with `pub fn new() -> Rules`, `pub fn on_snitch(&mut self, e: SnitchEvent)`, `pub fn on_bludger(&mut self, e: BludgerEvent, control_lost: bool)`, `pub fn add_goal(&mut self, home: bool)`, `pub fn phase(&self) -> MatchPhase`, `pub fn score(&self) -> Score`, `pub fn start(&mut self)`.

- [ ] **Step 1:** Failing tests: `snitch_catch_adds_150_and_ends_won`, `out_of_tries_ends_lost`, `ended_match_ignores_later_events` (no double end, no further score), `goal_adds_10_to_the_scoring_side`, `winner_is_decided_by_total_score_after_catch` (catch while behind by >150 is Lost, otherwise Won), `phase_order_intro_seeking_catching`.
- [ ] **Step 2:** Run `cargo test -p hp3-world quidditch::rules`. Expected: FAIL.
- [ ] **Step 3:** Implement as a small state machine.
- [ ] **Step 4:** Run. Expected: PASS.
- [ ] **Step 5:** Commit.

### Task 6: Arena and match spawn in the engine

**Files:** Create `quidditch/arena.rs`, `quidditch/glue.rs`; Modify `hp3-world/src/lib.rs` (tick hook after line 1112 `natives::carpe_tick(vm, dt);`, load path near line 261), `hp3-world/src/natives.rs` (`spawn` at line 462 is reused).

**Interfaces:**
- Consumes: constants from `docs/formats/quidditch-arena.md`; `QuidditchMatch` pieces from Tasks 2 to 5.
- Produces: `pub struct ArenaBounds { pub center: Vec3, pub half: Vec3, pub ceiling: f32 }`; `pub fn arena_spawn_list() -> Vec<ArenaPiece>` with `ArenaPiece { pub mesh: &'static str, pub pos: Vec3, pub rot: [i32; 3] }`; `pub struct QuidditchMatch` (owns `BroomState`, `Snitch`, `Bludgers`, `Rules`); `pub fn start(vm: &mut Vm<'_>, opponent: &str) -> Result<(), VmError>`, `pub fn stop(vm: &mut Vm<'_>)`, `pub fn tick(vm: &mut Vm<'_>, dt: f32)` (no-op when no match is active). Env `HP3_MATCH=<opponent>` calls `start` after the map loads.

- [ ] **Step 1:** Failing test (pure, in `arena.rs`): `arena_pieces_inside_bounds_or_stands` and `arena_has_goal_posts_at_both_ends`.
- [ ] **Step 2:** Run `cargo test -p hp3-world quidditch::arena`. Expected: FAIL.
- [ ] **Step 3:** Implement arena layout from the spike constants and `start`/`stop`/`tick`; `start` spawns the stadium pieces, a `HarryQuid` pawn with `QuidBroom_SM` attached, the Snitch, and sets the match active; `stop` destroys every actor `start` created and clears state (Review Focus: re-entry).
- [ ] **Step 4:** `cargo test -p hp3-world quidditch::` Expected: PASS. Then `cargo run -p hp3-run --release -- level ../spike-hp3 HP3_Quidditch 600` with `HP3_MATCH=Slytherin`: 0 script errors, match phase `Intro`.
- [ ] **Step 5:** Commit.

### Task 7: Flight, collision, camera, input (desktop)

**Files:** Modify `quidditch/glue.rs`, `hp3-world/src/input.rs` (`apply`, line 146), `hp3-world/src/lib.rs::player_view` (line 919); Create `quidditch/camera.rs`.

**Interfaces:**
- Consumes: `flight::step`, existing swept-box collision in `hp3-world/src/collision.rs`.
- Produces: `pub fn chase_camera(s: &BroomState, prev: &CamState, dt: f32) -> CamState` with `CamState { pub pos: Vec3, pub rot: [i32; 3], pub fov: f32 }`; `pub fn desktop_input(keys: &KeyState, mouse: (f32, f32), invert_pitch: bool, boost_toggle: &mut bool) -> BroomInput`. While a match is active `player_view` returns the chase camera; `input::apply` fills `BroomInput` (mouse and keys yaw/pitch, RightMouse or A toggles boost, LeftMouse or Z brake).

- [ ] **Step 1:** Failing tests: `chase_camera_stays_behind_and_above_harry`, `chase_camera_smooths_a_sudden_turn` (angle change per step bounded), `boost_toggles_on_one_tap_and_off_on_second`, `brake_and_boost_together_report_brake`, `invert_pitch_flips_pitch_sign`.
- [ ] **Step 2:** Run `cargo test -p hp3-world quidditch::camera quidditch::glue`. Expected: FAIL.
- [ ] **Step 3:** Implement. In `glue.rs` sweep Harry's move against world collision; on contact slide along the surface and kill velocity into it (Review Focus: no tunnelling); a soft boundary adds a turn-back force beyond `ArenaBounds` and never calls the out-of-world kill.
- [ ] **Step 4:** Run tests. Expected: PASS. Then play on desktop: `hp3.exe ../spike-hp3 HP3_Quidditch` with `HP3_MATCH=Slytherin`: Harry flies, camera follows, cannot leave the arena or clip the stands.
- [ ] **Step 5:** Commit.

### Task 8: Snitch and Bludger in the world (render, sound, HUD)

**Files:** Create `quidditch/hud.rs`, `quidditch/commentary.rs`; Modify `quidditch/glue.rs`.

**Interfaces:**
- Consumes: `Snitch`, `Bludgers`, `Rules` state.
- Produces: `pub fn hud_items(m: &QuidditchMatch) -> Vec<HudItem>` (snitch progress bar, score, catch-attempt pips, control hints) where `HudItem { pub kind: HudKind, pub value: f32 }`; `pub fn comment_for(e: &MatchEvent, rng: &mut impl FnMut() -> u32, last: &mut Option<(CommentId, f32)>) -> Option<CommentId>` that avoids repeating a line within a minimum gap; sound names from the `soundeffects.Quidditch` package: `flying_broom_loop` (pitch and volume follow speed), `Q_snitch_loop`, `snitch_flyby01`, `bludger_loop`, `bludger_flyby01`, `AMB_crowd_loop1`, plus music `SM_QUD_SnitchOrch_01`. The Bludger is a generated sphere mesh with the `Bludger_Halo` sprite; the HUD draws through the existing `Canvas` path (`hp3-world/src/canvas.rs`).

- [ ] **Step 1:** Failing tests: `hud_progress_item_matches_snitch_progress`, `comment_not_repeated_within_gap`, `comment_chosen_for_each_event_kind`.
- [ ] **Step 2:** Run `cargo test -p hp3-world quidditch::hud quidditch::commentary`. Expected: FAIL.
- [ ] **Step 3:** Implement the HUD list, comment selection and the glue that spawns and moves the visuals and starts the sounds.
- [ ] **Step 4:** Run tests. Expected: PASS. Then play: snitch and Bludgers are visible and audible, a catch ends the match with a result screen line.
- [ ] **Step 5:** Commit.

### Task 9: Phone controls and 60 fps budget (first playable)

**Files:** Modify `hp3-world/src/touch.rs`, `apps/hp3/src/touch.rs`, `quidditch/glue.rs`; Create `quidditch/lod.rs`.

**Interfaces:**
- Produces: touch layout while a match is active: left-side steering stick (yaw and pitch), right-side boost toggle button and brake button; `pub fn touch_input(stick: Option<Vec2>, boost_tap: bool, brake_held: bool, boost: &mut bool) -> BroomInput` (no finger gives zero yaw and pitch, boost state unchanged); `pub struct Budget { pub max_active_ai: usize, pub proxy_distance: f32 }`; `pub fn choose_budget(avg_frame_ms: f32, current: Budget) -> Budget` that lowers `max_active_ai` when frames exceed 16.6 ms and raises it only after 120 stable frames; `pub fn use_proxy(distance: f32, b: &Budget) -> bool`.

- [ ] **Step 1:** Failing tests: `no_finger_holds_course`, `two_fingers_use_the_stick_finger_only`, `boost_survives_pause_resume`, `budget_drops_ai_when_over_16_6ms`, `budget_recovers_only_after_120_stable_frames`, `far_players_use_proxy`.
- [ ] **Step 2:** Run `cargo test -p hp3-world quidditch::lod touch`. Expected: FAIL.
- [ ] **Step 3:** Implement controls and budget. Build the APK per the memory recipe (copy to `Desktop\FlipendoHP`) and measure fps on the phone with the existing perf flags.
- [ ] **Step 4:** Run tests. Expected: PASS. On the phone: the stage 1 match holds 60 fps; record the measured fps in the PR notes. Stage 1 is the first playable.
- [ ] **Step 5:** Commit.

### Task 10: Smoke match and regression

**Files:** Create `tools/hp3-run/src/quidsmoke.rs`; Modify `tools/hp3-run/src/main.rs`.

**Interfaces:**
- Produces: `hp3-run quid-smoke <HP3 folder> [seed]`: runs a headless match with a scripted Harry (always steers toward the snitch, requests a catch when inside twice the catch radius) until `MatchPhase::Ended`, prints outcome, score, tick count and script error count; exit code 0 only when the match ended and there were 0 script errors.

- [ ] **Step 1:** Failing test (in `quidsmoke.rs`): `smoke_match_ends_within_20000_ticks_for_seeds_1_to_5`.
- [ ] **Step 2:** Run `cargo test -p hp3-run quid`. Expected: FAIL.
- [ ] **Step 3:** Implement the scripted driver on top of `quidditch::start/tick`.
- [ ] **Step 4:** Run `cargo test -p hp3-run quid` (Expected: PASS), then `hp3-run levels ../spike-hp3` (Expected: 34 maps, 0 script errors, unchanged).
- [ ] **Step 5:** Commit.

---

## Stage 2: the full match

### Task 11: Tuning pass

**Files:** Modify `BroomTuning`, `SnitchTuning`, `BludgerTuning` defaults.

- [ ] **Step 1:** Owner plays desktop and phone with `HP3_QUID_TUNE=1` printing the active tuning; adjust the three `Default` impls by feel; record the final values in `docs/formats/quidditch-arena.md`.
- [ ] **Step 2:** Re-run `cargo test -p hp3-world quidditch::` Expected: PASS (tests assert relations, not values). Commit.

### Task 12: Quaffle, hoops and goals

**Files:** Create `quidditch/quaffle.rs`, `quidditch/goals.rs`; Modify `quidditch/arena.rs`, `glue.rs`.

**Interfaces:**
- Produces: `pub struct Quaffle { pub pos: Vec3, pub vel: Vec3, pub holder: Option<PlayerId> }` with `pub fn step(&mut self, dt: f32)`, `pub fn pass(&mut self, from: Vec3, to: Vec3, speed: f32)`, `pub fn shoot(&mut self, from: Vec3, hoop: Vec3, speed: f32)`; `pub struct Hoop { pub center: Vec3, pub radius: f32, pub normal: Vec3, pub home: bool }`; `pub fn goal_scored(prev: Vec3, now: Vec3, h: &Hoop) -> bool` (segment crosses the hoop disc along its normal from the front); wired to `Rules::add_goal`.

- [ ] **Step 1:** Failing tests: `quaffle_through_hoop_front_scores`, `quaffle_through_hoop_from_behind_does_not_score`, `quaffle_missing_the_ring_does_not_score`, `quaffle_obeys_gravity_when_free`, `pass_arrives_near_target`, `one_goal_scores_once` (stays in the hoop for several steps).
- [ ] **Step 2:** Run `cargo test -p hp3-world quidditch::quaffle quidditch::goals`. Expected: FAIL.
- [ ] **Step 3:** Implement; hoops at the positions recorded in Task 1 (primitive rings if none).
- [ ] **Step 4:** Run. Expected: PASS. Commit.

### Task 13: Team AI (Chaser, Beater, Keeper, opposing Seeker)

**Files:** Create `quidditch/ai.rs`; Modify `glue.rs`, `lod.rs`.

**Interfaces:**
- Consumes: `Quaffle`, `Hoop`, `Bludgers`, `Budget`.
- Produces: `pub enum Role { Chaser, Beater, Keeper, Seeker }`; `pub struct Agent { pub id: PlayerId, pub team: Team, pub role: Role, pub body: BroomState }`; `pub fn step_agent(a: &mut Agent, w: &AiWorld, dt: f32) -> Vec<AiAction>` with `AiAction { Pass(PlayerId), Shoot, HitBludger(Vec3), Block, ChaseSnitch }`; `pub fn active_agents(all: &[Agent], budget: &Budget) -> Vec<usize>` (indices that get a full `step_agent`; the rest are steered cheaply).

- [ ] **Step 1:** Failing tests: `chaser_with_quaffle_shoots_when_in_range`, `chaser_passes_to_open_teammate_when_far`, `keeper_stays_within_hoop_radius_band`, `beater_hits_bludger_heading_for_teammate`, `opposing_seeker_chases_snitch_after_delay`, `agents_stay_inside_arena_bounds`, `active_agents_respects_budget`.
- [ ] **Step 2:** Run `cargo test -p hp3-world quidditch::ai`. Expected: FAIL.
- [ ] **Step 3:** Implement a state machine per role on top of `flight::step` steering.
- [ ] **Step 4:** Run. Expected: PASS. Extend `quid-smoke` to assert that both teams scored at least once over seeds 1 to 5 before the catch, then `hp3-run quid-smoke ../spike-hp3 1` exits 0.
- [ ] **Step 5:** Commit.

### Task 14: Menu entry, league, recolors

**Files:** Modify front-end menu glue in `hp3-world/src/gui.rs`, `hp3-world/src/save.rs`; Create `quidditch/league.rs`.

**Interfaces:**
- Produces: `pub struct League { pub rounds: Vec<Round>, pub points: [u32; 4] }` with `pub fn next_opponent(&self) -> Option<Opponent>`, `pub fn record(&mut self, r: Outcome, score: Score)`, `pub fn champion(&self) -> Option<Team>`; rounds are Slytherin, Ravenclaw, Hufflepuff, twice each, then a final; saved with the existing profile saves; Ravenclaw and Hufflepuff use recolored `QuidPlayer*` skins.

- [ ] **Step 1:** Failing tests: `league_has_six_rounds_then_a_final`, `losing_every_round_gives_no_final`, `league_round_trips_through_save`, `menu_entry_starts_match_with_next_opponent`.
- [ ] **Step 2:** Run `cargo test -p hp3-world quidditch::league`. Expected: FAIL.
- [ ] **Step 3:** Implement and wire the menu entry; exit returns to the menu.
- [ ] **Step 4:** Run. Expected: PASS. `hp3-run levels ../spike-hp3` still 0 script errors. Commit.

---

## Self-Review

- Spec coverage: goals 1 to 5 map to Tasks 5/12/13 (match, goals), 7/9 (controls), 13 (AI), 8 (HUD, crowd, commentary, music), 9 (60 fps). Calls 1 to 14: arena spike (Task 1), generated balls (Task 8), scoring (Task 5), launch (Tasks 6 and 14), classes gated (Task 6 `start`/`stop`), acceptance (Task 10).
- Types: `ArenaBounds`, `BroomState`, `BroomInput`, `SnitchEvent`, `BludgerEvent`, `Budget` are defined once and used with the same names later.
- Proportion: the plan lists signatures, test names and values; bodies are left to the implementer.
