//! What a match looks, sounds and reads like: the Snitch's mesh, the Bludger balls, the match sounds, the
//! commentary subtitle and the HUD.
//!
//! This touches the VM, like `glue`, but it is called only from `glue` (and `canvas::render`, for the HUD),
//! so the match's own logic stays in one file and the presentation in this one. Everything it decides -- what
//! the HUD shows, which line is said -- comes out of the pure `hud` and `commentary` modules; what is here is
//! the spawning, the property writes, the sound events and the canvas tiles.
//!
//! Missing art: HP3 ships no Bludger (or Quaffle) mesh, so a Bludger is an existing round static mesh scaled
//! to the right size, darkened and skinned with the retail `Bludger_Halo` glow texture. The renderer draws
//! neither sprites nor translucent actors, so the halo is the ball's skin rather than a sprite over it; the
//! Snitch keeps its own retail mesh and goes without a halo for the same reason.

use glam::Vec3;

use hp3_script::{ObjId, Value, Vm};

use super::commentary::{self, CommentId};
use super::glue::QuidditchMatch;
use super::hud::{self, HudInput, HudItem, HudKind};
use super::rules::MatchEvent;
use crate::canvas;

/// The looping and one-shot sounds of a match, all in `soundeffects.Quidditch`.
const SND_BROOM_LOOP: &str = "soundeffects.Quidditch.flying_broom_loop";
const SND_SNITCH_LOOP: &str = "soundeffects.Quidditch.Q_snitch_loop";
const SND_SNITCH_FLYBY: &str = "soundeffects.Quidditch.snitch_flyby01";
const SND_BLUDGER_LOOP: &str = "soundeffects.Quidditch.bludger_loop";
const SND_BLUDGER_FLYBY: &str = "soundeffects.Quidditch.bludger_flyby01";
const SND_CROWD_LOOP: &str = "soundeffects.Quidditch.AMB_crowd_loop1";
/// Commentary sounds (`commentary::sound_for`) are named without their group.
const SND_GROUP: &str = "soundeffects.Quidditch.";
/// The one-shots the match fires later, and the crowd reactions `commentary::sound_for` asks for. They are
/// looked up again when they are wanted; this list is only so a name that is not in the package shows up in
/// the log at the start of the match rather than as a silence nobody notices.
const SND_PROBE: [&str; 7] = [
    SND_SNITCH_FLYBY,
    SND_BLUDGER_LOOP,
    SND_BLUDGER_FLYBY,
    "soundeffects.Quidditch.SS_QUID_Bigcheers_0001",
    "soundeffects.Quidditch.SS_QUID_Crowd_roar_01",
    "soundeffects.Quidditch.SS_QUID_Big_cheer_01",
    "soundeffects.Quidditch.SS_QUID_boo_long_01",
];
/// The match's music, from the game's own `music` folder.
const MUSIC: &str = "SM_QUD_SnitchOrch_01";

/// The retail Golden Snitch: a skeletal mesh in the props package. `HGame.Snitch` carries it as its `Mesh`
/// default, but an actor's `DrawType` has to say so before anything is drawn.
const SNITCH_MESH: &str = "HProps.Snitch";
/// What a Bludger is made of, best first: HP3 has no Bludger mesh, so a round one stands in. It is scaled by
/// its own bounds to `BLUDGER_RADIUS`, so swapping the mesh needs no new numbers.
const BLUDGER_MESHES: [&str; 3] = [
    "HP3_InteriorAccents_SM.miscDetails.MagicalOrb_SM",
    "HP3_InteriorAccents_SM.miscDetails.Globe_SM",
    "HP3_InteriorAccents_SM.miscDetails.FloorGlobe_SM",
];
/// The retail glow textures. `Bludger_Halo` skins the ball (see the note at the top).
const BLUDGER_HALO: &str = "HP_FX.General.Bludger_Halo";
/// What the stadium pieces, the broom and the Bludgers are spawned as.
const MESH_ACTOR_CLASS: &str = "Engine.StaticMeshActor";
/// A plain white tile the HUD's bars and panels are drawn from (a canvas tile with no material draws nothing).
const WHITE: &str = "Engine.WhiteSquareTexture";

/// How big a Bludger is drawn (uu, radius): a 30 uu iron ball next to Harry's 76 uu tall box. The pure
/// `BludgerTuning::hit_radius` is 55, this plus Harry's body.
const BLUDGER_RADIUS: f32 = 15.0;
/// How big the Snitch is drawn (uu): a walnut would be invisible from the chase camera.
const SNITCH_RADIUS: f32 = 55.0;
/// `DrawType`: 2 is a skeletal mesh, 8 a static one.
const DT_MESH: u8 = 2;
const DT_STATIC_MESH: u8 = 8;

/// Harry's broom loop follows his speed between these volumes (0..255) and pitches (1 = as recorded).
const BROOM_VOLUME: (f32, f32) = (90.0, 230.0);
const BROOM_PITCH: (f32, f32) = (0.75, 1.45);

/// How close the Snitch has to pass Harry for a flyby (uu), and how long before another one may play.
const FLYBY_RADIUS: f32 = 700.0;
const FLYBY_COOLDOWN: f32 = 4.0;

/// How long a commentary line stays on screen (s).
const SUBTITLE_TIME: f32 = 4.0;

/// Everything the presentation owns on top of the match's own state.
#[derive(Default)]
pub struct Presentation {
    /// One static mesh actor per Bludger slot, hidden while its slot is empty. They are in the match's
    /// `actors`, so `glue::stop` destroys them with the rest.
    bludgers: Vec<ObjId>,
    /// The silent actor at the middle of the pitch that carries the crowd loop.
    crowd: Option<ObjId>,
    /// The music handle, to stop it again.
    music: Option<i32>,
    /// The commentary line on screen and how long it has left.
    subtitle: Option<(String, f32)>,
    /// What was said last and how long ago (`commentary::comment_for`).
    last: Option<(CommentId, f32)>,
    /// Seconds until another Snitch flyby may play.
    flyby_cooldown: f32,
    /// The match's own stream for picking commentary lines.
    rng: u64,
    /// Looked up once: the HUD's font and the white tile its bars are drawn from.
    font: Option<ObjId>,
    white: Option<ObjId>,
    /// Whether `Started` has been reported to the commentary.
    kicked_off: bool,
    /// The HUD's SVG art for the current screen size, and the size it failed to be made at (not retried).
    art: Option<super::hud_art::HudArt>,
    art_failed: Option<i32>,
}

/// SplitMix64, the same shape as `rng::Rng`: the commentary's own stream, so picking a line never moves the
/// snitch's or the Bludgers' randomness.
fn split(state: &mut u64) -> u32 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    ((z ^ (z >> 31)) >> 32) as u32
}

fn v3(v: Vec3) -> Value {
    Value::Struct(vec![Value::Float(v.x), Value::Float(v.y), Value::Float(v.z)].into())
}

/// A line that goes to the console and into the level's log, once.
fn note(vm: &mut Vm<'_>, line: String) {
    println!("{line}");
    vm.log.push(line);
}

/// The sound object of a full package path, or `None` with a line saying which one is missing.
fn sound(vm: &mut Vm<'_>, path: &str) -> Option<ObjId> {
    let o = crate::find_object_of(vm, path, Some("Sound"));
    if o.is_none() {
        note(vm, format!("quidditch: sound {path} not found"));
    }
    o
}

/// Hangs a looping sound on an actor (the engine's `AmbientSound` path, which the mixer picks up every frame).
/// `volume` and `pitch` are the actor's own, before the game's ambient setting.
fn ambient(vm: &mut Vm<'_>, actor: ObjId, s: Option<ObjId>, volume: f32, pitch: f32, radius: f32) {
    crate::set(vm, actor, "AmbientSound", Value::Object(s));
    crate::set(vm, actor, "SoundVolume", Value::Byte(volume.clamp(0.0, 255.0) as u8));
    crate::set(vm, actor, "SoundPitch", Value::Byte((pitch * 64.0).clamp(1.0, 255.0) as u8));
    crate::set(vm, actor, "SoundRadius", Value::Float(radius));
}

/// Queues one sound at `at`, straight into the audio events the platform's mixer drains (the match has no
/// script to call `PlaySound` for it).
pub(super) fn play_once(vm: &mut Vm<'_>, s: ObjId, at: Vec3, volume: f32, pitch: f32, radius: f32) {
    let Some(key) = vm.db.objects.get(s).key.as_ref().map(|(p, i)| (p.to_string(), *i)) else { return };
    crate::world(vm).audio.events.push(crate::audio::Event::Play {
        sound: key,
        emitter: None,
        location: at,
        slot: 1,
        volume,
        pitch,
        radius,
        no_override: false,
        attenuate: true,
        no_3d: false,
        who: "quidditch".into(),
    });
}

/// The radius of a mesh's bounding box, to scale a stand-in mesh to the size the game wants it at.
fn mesh_radius(vm: &mut Vm<'_>, mesh: ObjId) -> Option<f32> {
    let (pkg, index) = vm.db.objects.get(mesh).key.clone()?;
    let p = vm.db.set.get(&pkg).ok()?;
    let chain = vm.db.set.export_chain(&p, index);
    let r = match crate::native_asset(&p, index, &chain)? {
        hp3_assets::Asset::StaticMesh(m) => {
            let (lo, hi) = (m.bounds.bounding_box.min, m.bounds.bounding_box.max);
            (Vec3::new(hi.x, hi.y, hi.z) - Vec3::new(lo.x, lo.y, lo.z)).max_element() * 0.5
        }
        hp3_assets::Asset::SkeletalMesh(m) => {
            let mut lo = Vec3::splat(f32::MAX);
            let mut hi = Vec3::splat(f32::MIN);
            for v in &m.points {
                let v = Vec3::new(v.x, v.y, v.z);
                lo = lo.min(v);
                hi = hi.max(v);
            }
            if m.points.is_empty() { return None } else { (hi - lo).max_element() * 0.5 }
        }
        _ => return None,
    };
    (r.is_finite() && r > 1e-3).then_some(r)
}

/// Gives the Snitch actor its retail mesh and makes it big enough to chase. The `HGame.Snitch` class carries
/// the mesh as a default, but nothing is drawn until `DrawType` says it is a skeletal mesh.
fn dress_snitch(vm: &mut Vm<'_>, snitch: ObjId) {
    let had = crate::get(vm, snitch, "Mesh").as_obj();
    let mesh = had.or_else(|| crate::find_object_of(vm, SNITCH_MESH, Some("SkeletalMesh")));
    let Some(mesh) = mesh else {
        note(vm, format!("quidditch: no Snitch mesh ({SNITCH_MESH}), the snitch is invisible"));
        return;
    };
    crate::set(vm, snitch, "Mesh", Value::Object(Some(mesh)));
    crate::set(vm, snitch, "DrawType", Value::Byte(DT_MESH));
    crate::set(vm, snitch, "bHidden", Value::Bool(false));
    // Gold, and lit by nothing out in the open: the snitch glows by itself.
    crate::set(vm, snitch, "bUnlit", Value::Bool(true));
    let scale = mesh_radius(vm, mesh).map(|r| SNITCH_RADIUS / r).unwrap_or(1.0).clamp(0.05, 200.0);
    crate::set(vm, snitch, "DrawScale", Value::Float(scale));
    let path = hp3_script::natives::object_path(vm, mesh);
    note(vm, format!("quidditch: snitch mesh {path} (from the class {}) at scale {scale:.2}", if had.is_some() { "default" } else { "package" }));
}

/// Spawns one round static mesh actor of `radius` uu at the origin, born hidden: the shape every ball of the
/// match is made of, since HP3 ships neither a Bludger nor a Quaffle mesh. `skin`, where it is given, is the
/// material the ball is wrapped in, and `unlit` makes it bright on its own. The caller owns the actor (it
/// belongs in the match's `actors`) and shows it when it has somewhere to be. `what` names it in the log;
/// `None` spawns it silently, which is what a hoop ring's dozenth bead wants.
pub(super) fn spawn_ball(vm: &mut Vm<'_>, what: Option<&str>, radius: f32, skin: Option<&str>, unlit: bool) -> Option<ObjId> {
    let class = crate::find_object(vm, MESH_ACTOR_CLASS)?;
    let mesh = BLUDGER_MESHES.iter().find_map(|p| crate::find_object_of(vm, p, Some("StaticMesh")));
    let Some(mesh) = mesh else {
        note(vm, format!("quidditch: no round mesh of {BLUDGER_MESHES:?}, the {} is invisible", what.unwrap_or("ball")));
        return None;
    };
    let args = [Value::Object(Some(class)), Value::Object(None), Value::Empty, v3(Vec3::ZERO), Value::Struct(vec![Value::Int(0); 3].into())];
    let a = crate::natives::spawn(vm, None, &args).ok()?.as_obj()?;
    crate::set(vm, a, "StaticMesh", Value::Object(Some(mesh)));
    crate::set(vm, a, "DrawType", Value::Byte(DT_STATIC_MESH));
    // Scenery is born `bStatic`, which would freeze it where it was spawned and refuse to destroy it -- and
    // would also make it invisible: `hp3_engine::actor_instances` draws no `bStatic` static mesh, because
    // the map's own are baked into the level scene instead, and nothing bakes one that was spawned.
    crate::set(vm, a, "bStatic", Value::Bool(false));
    crate::set(vm, a, "bNoDelete", Value::Bool(false));
    // A Bludger is lit by the stadium (an iron ball is dark, and the halo skin alone is a lamp); a hoop ring
    // has to be seen from across the pitch, so it carries its own light.
    crate::set(vm, a, "bUnlit", Value::Bool(unlit));
    crate::set(vm, a, "bHidden", Value::Bool(true));
    let scale = mesh_radius(vm, mesh).map(|r| radius / r).unwrap_or(1.0).clamp(0.01, 200.0);
    crate::set(vm, a, "DrawScale", Value::Float(scale));
    if let Some(mat) = skin.and_then(|s| crate::find_object(vm, s)) {
        crate::set(vm, a, "Skins", Value::Array(vec![Value::Object(Some(mat))]));
    }
    if let Some(what) = what {
        let path = hp3_script::natives::object_path(vm, mesh);
        note(vm, format!("quidditch: {what} from {path} at scale {scale:.3}"));
    }
    Some(a)
}

/// Spawns one Bludger ball: a round static mesh scaled to `BLUDGER_RADIUS`, darkened and skinned with the
/// retail halo glow. Born hidden; `tick` shows it while its slot has a Bludger in it.
fn spawn_bludger(vm: &mut Vm<'_>) -> Option<ObjId> {
    spawn_ball(vm, Some("Bludger ball"), BLUDGER_RADIUS, Some(BLUDGER_HALO), false)
}

/// Starts everything the player sees and hears: the Snitch's mesh, the crowd, the broom and snitch loops and
/// the match music. Called at the end of `glue::start`, with the match still in hand.
pub fn start(vm: &mut Vm<'_>, m: &mut QuidditchMatch, seed: u64) {
    m.show.rng = seed ^ 0x5eed_c0de_1234_abcd;
    m.show.font = crate::fonts::create(vm, "Arial", 18);
    m.show.white = crate::find_object_of(vm, WHITE, Some("Texture"));
    if let Some(s) = m.snitch_actor {
        dress_snitch(vm, s);
        if let Some(loop_sound) = sound(vm, SND_SNITCH_LOOP) {
            ambient(vm, s, Some(loop_sound), 150.0, 1.0, 2500.0);
            note(vm, format!("quidditch: sound started {SND_SNITCH_LOOP} on the snitch"));
        }
    }
    if let Some(h) = m.harry
        && let Some(loop_sound) = sound(vm, SND_BROOM_LOOP)
    {
        ambient(vm, h, Some(loop_sound), BROOM_VOLUME.0, BROOM_PITCH.0, 2500.0);
        note(vm, format!("quidditch: sound started {SND_BROOM_LOOP} on the broom (volume and pitch follow the speed)"));
    }
    // The crowd comes from the middle of the pitch, off an actor with no mesh of its own.
    if let Some(class) = crate::find_object(vm, MESH_ACTOR_CLASS) {
        let at = m.arena.center;
        let args = [Value::Object(Some(class)), Value::Object(None), Value::Empty, v3(at), Value::Struct(vec![Value::Int(0); 3].into())];
        if let Some(a) = crate::natives::spawn(vm, None, &args).ok().and_then(|v| v.as_obj()) {
            crate::set(vm, a, "bStatic", Value::Bool(false));
            crate::set(vm, a, "bNoDelete", Value::Bool(false));
            crate::set(vm, a, "bHidden", Value::Bool(true));
            m.actors.push(a);
            m.show.crowd = Some(a);
            if let Some(s) = sound(vm, SND_CROWD_LOOP) {
                ambient(vm, a, Some(s), 170.0, 1.0, 20000.0);
                crate::set(vm, a, "bFullVolume", Value::Bool(false));
                note(vm, format!("quidditch: sound started {SND_CROWD_LOOP} over the pitch"));
            }
        }
    }
    let ready: Vec<&str> = SND_PROBE.iter().copied().filter(|p| sound(vm, p).is_some()).collect();
    note(vm, format!("quidditch: {} of {} one-shot sounds found: {ready:?}", ready.len(), SND_PROBE.len()));
    let handle = crate::audio::play_music(vm, MUSIC, 2.0, true);
    m.show.music = Some(handle);
    note(vm, format!("quidditch: music started {MUSIC} (handle {handle})"));
}

/// Fades out whatever screen flash the level left behind. The script does this in `ViewFlash`, which runs
/// inside `PlayerCalcView` -- and a match is watched from its own chase camera, so `PlayerCalcView` never runs
/// (`player_view`). Without this, the level's fade-in from black hangs where it stood when the match started
/// and tints the whole picture for the rest of the match.
fn fade_out_the_flash(vm: &mut Vm<'_>, dt: f32) {
    let Some(pc) = crate::world(vm).player else { return };
    // `FlashScale` is a multiplier (1 1 1 is the plain picture) and `FlashFog` a colour added on top.
    let scale = crate::rd_vec(vm, pc, "FlashScale");
    let fog = crate::rd_vec(vm, pc, "FlashFog");
    if scale == Vec3::ONE && fog == Vec3::ZERO {
        return;
    }
    // The script's own rate: the gap to the resting value is closed over `FADE` seconds.
    const FADE: f32 = 0.4;
    let k = (dt / FADE).clamp(0.0, 1.0);
    let step = |v: Vec3, goal: Vec3| {
        let n = v + (goal - v) * k;
        if n.is_finite() { n } else { goal }
    };
    let scale = step(scale, Vec3::ONE);
    let fog = step(fog, Vec3::ZERO);
    // Close enough is done, so the flash never sits a hair off and keeps the picture a shade dark.
    let scale = if (scale - Vec3::ONE).abs().max_element() < 0.01 { Vec3::ONE } else { scale };
    let fog = if fog.abs().max_element() < 0.01 { Vec3::ZERO } else { fog };
    crate::set(vm, pc, "FlashScale", v3(scale));
    crate::set(vm, pc, "FlashFog", v3(fog));
}

/// One frame of the presentation: the Bludger balls follow the pure Bludgers, the broom loop follows the
/// speed, a flyby plays when the Snitch passes, and each match event may get a commentary line.
pub fn tick(vm: &mut Vm<'_>, m: &mut QuidditchMatch, dt: f32, events: &[MatchEvent]) {
    let dt = if dt.is_finite() { dt.clamp(0.0, 1.0) } else { 0.0 };
    fade_out_the_flash(vm, dt);
    commentary::age(&mut m.show.last, dt);
    m.show.flyby_cooldown = (m.show.flyby_cooldown - dt).max(0.0);
    if let Some((_, left)) = m.show.subtitle.as_mut() {
        *left -= dt;
    }
    if m.show.subtitle.as_ref().is_some_and(|(_, left)| *left <= 0.0) {
        m.show.subtitle = None;
    }
    // The broom's loop rides the speed, as HP1's does.
    if let Some(h) = m.harry.filter(|&h| !vm.db.objects.get(h).deleted) {
        let k = (m.broom.vel.length() / m.momentum.tuning().max_speed.max(1.0)).clamp(0.0, 1.0);
        let lerp = |(a, b): (f32, f32)| a + (b - a) * k;
        crate::set(vm, h, "SoundVolume", Value::Byte(lerp(BROOM_VOLUME).clamp(0.0, 255.0) as u8));
        crate::set(vm, h, "SoundPitch", Value::Byte((lerp(BROOM_PITCH) * 64.0).clamp(1.0, 255.0) as u8));
    }
    place_bludgers(vm, m);
    // A Snitch that whips past Harry is heard going.
    if let Some(s) = m.snitch_actor.filter(|_| m.show.flyby_cooldown <= 0.0)
        && m.snitch.pos().distance(m.broom.pos) < FLYBY_RADIUS
        && !vm.db.objects.get(s).deleted
        && let Some(snd) = crate::find_object_of(vm, SND_SNITCH_FLYBY, Some("Sound"))
    {
        let at = m.snitch.pos();
        play_once(vm, snd, at, 0.6, 1.0, 2500.0);
        m.show.flyby_cooldown = FLYBY_COOLDOWN;
    }
    for e in events {
        if let MatchEvent::Bludger(super::bludger::BludgerEvent::Launched) = e {
            let at = super::arena::bludger_home(&m.arena);
            note(vm, format!("quidditch: Bludger launched from ({:.0}, {:.0}, {:.0}), {} in flight", at.x, at.y, at.z, m.bludgers.positions().len()));
            if let Some(snd) = crate::find_object_of(vm, SND_BLUDGER_FLYBY, Some("Sound")) {
                play_once(vm, snd, at, 0.8, 1.0, 4000.0);
            }
        }
        say(vm, m, *e);
    }
}

/// Gives each Bludger in flight a ball and hides the rest.
fn place_bludgers(vm: &mut Vm<'_>, m: &mut QuidditchMatch) {
    // The pure Bludgers stop being stepped once the match is over, so their last positions would hang in the
    // air over the result screen: the balls go away with the match.
    let positions = if matches!(m.rules.phase(), super::rules::MatchPhase::Ended(_)) { Vec::new() } else { m.bludgers.positions() };
    while m.show.bludgers.len() < positions.len() {
        match spawn_bludger(vm) {
            Some(a) => {
                m.actors.push(a);
                m.show.bludgers.push(a);
            }
            // No mesh: stop trying, the Bludgers are heard but not seen.
            None => break,
        }
    }
    for (i, a) in m.show.bludgers.clone().into_iter().enumerate() {
        if vm.db.objects.get(a).deleted {
            continue;
        }
        match positions.get(i) {
            Some(&p) => {
                crate::set(vm, a, "Location", v3(super::arena::clamp_into(&m.arena, p)));
                crate::set(vm, a, "bHidden", Value::Bool(false));
                // The frame budget's one rule the match already keeps (`lod::use_proxy`): a ball further off
                // than the proxy distance is only seen, not heard. Its loop is inaudible that far away
                // anyway, and the sound costs a voice and an update a frame.
                if super::lod::use_proxy((p - m.cam.pos).length(), &m.budget) {
                    crate::set(vm, a, "AmbientSound", Value::Object(None));
                } else if crate::get(vm, a, "AmbientSound").as_obj().is_none() {
                    // Looked up only when a ball needs it, not every frame of the match.
                    let loop_sound = crate::find_object_of(vm, SND_BLUDGER_LOOP, Some("Sound"));
                    ambient(vm, a, loop_sound, 180.0, 1.0, 4000.0);
                }
            }
            None => {
                crate::set(vm, a, "bHidden", Value::Bool(true));
                crate::set(vm, a, "AmbientSound", Value::Object(None));
            }
        }
    }
}

/// Picks a commentary line for `e`, puts it on screen and plays the crowd under it, if there is one.
fn say(vm: &mut Vm<'_>, m: &mut QuidditchMatch, e: MatchEvent) {
    // The stream and the last line are taken out, so the closure `comment_for` wants can hold the one while
    // it writes the other, and put back whatever it did with them.
    let mut state = m.show.rng;
    let mut last = m.show.last;
    let picked = {
        let mut rng = || split(&mut state);
        commentary::comment_for(&e, &mut rng, &mut last)
    };
    m.show.rng = state;
    m.show.last = last;
    let Some(id) = picked else { return };
    let text = commentary::line(id).to_string();
    note(vm, format!("quidditch: commentary {id:?}: {text}"));
    m.show.subtitle = Some((text, SUBTITLE_TIME));
    if let Some(name) = commentary::sound_for(id)
        && let Some(snd) = crate::find_object_of(vm, &format!("{SND_GROUP}{name}"), Some("Sound"))
    {
        let at = m.broom.pos;
        play_once(vm, snd, at, 0.7, 1.0, 20000.0);
    }
}

/// The match has begun: the kickoff line, once.
pub fn kickoff(vm: &mut Vm<'_>, m: &mut QuidditchMatch) {
    if m.show.kicked_off {
        return;
    }
    m.show.kicked_off = true;
    say(vm, m, MatchEvent::Started);
}

/// Ends the sound of the match. The actors (and with them every ambient loop) go with `glue::stop`.
pub fn stop(vm: &mut Vm<'_>, m: &QuidditchMatch) {
    if let Some(h) = m.show.music {
        crate::audio::stop_music(vm, h, 2.0);
    }
}

/// What the HUD needs out of the match.
pub fn hud_input(m: &QuidditchMatch) -> HudInput {
    HudInput {
        phase: m.rules.phase(),
        progress: m.snitch.progress(),
        tries_left: m.snitch.tries_left(),
        score_home: m.rules.score().home,
        score_away: m.rules.score().away,
        speed: m.broom.vel.length(),
        top_speed: m.momentum.tuning().max_speed,
        bumps: m.bludgers.bumps(),
        bumps_to_lose: m.bludgers.bumps_to_lose(),
        ring_streak: m.rings.trail.streak(),
        rings_needed: m.rings.trail.tuning().catch_rings,
        charge: m.momentum.charge(),
        charge_glow: m.charge_glow,
        window_left: m.snitch.catch_left(),
        boost: m.momentum.boost_state(),
    }
}

/// The HUD item list as one line, for the headless logs.
pub fn hud_line(items: &[HudItem]) -> String {
    let parts: Vec<String> = items.iter().map(|i| format!("{:?}={:.2}", i.kind, i.value)).collect();
    format!("quidditch: HUD [{}]", parts.join(" "))
}

fn value_of(items: &[HudItem], kind: HudKind) -> Option<f32> {
    items.iter().find(|i| i.kind == kind).map(|i| i.value)
}

/// A solid rectangle in screen pixels.
fn bar(vm: &mut Vm<'_>, white: Option<ObjId>, r: [f32; 4], color: [u8; 4]) {
    canvas::tile(vm, white, r, [0.0, 0.0, -1.0, -1.0], color, canvas::STY_ALPHA);
}

/// One picture of the HUD's art, `x y w h` in screen pixels, at `alpha` (0..1).
fn pic(vm: &mut Vm<'_>, tex: ObjId, r: [f32; 4], alpha: f32) {
    let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    canvas::tile(vm, Some(tex), r, [0.0, 0.0, -1.0, -1.0], [255, 255, 255, a], canvas::STY_ALPHA);
}

/// Text centred on `cx`, its top at `y`.
fn centered(vm: &mut Vm<'_>, font: ObjId, line: &str, cx: f32, y: f32, color: [u8; 4]) {
    let tw = canvas::measure(vm, font, line).0;
    canvas::text(vm, font, line, cx - tw * 0.5, y, cx + tw + 400.0, color);
}

/// The HUD drawn from the SVG art (`hud_art`): the catch dial with the Snitch in it, the wings that open with
/// the broom's charge, the run of rings, the tries, the Bludger brooms, the score shields, the catch prompt,
/// the hints and the result banner. Laid out against a 720-high screen and scaled, like the plain HUD.
#[allow(clippy::too_many_arguments)]
fn draw_art(
    vm: &mut Vm<'_>,
    art: &super::hud_art::HudArt,
    items: &[HudItem],
    input: &HudInput,
    font: Option<ObjId>,
    opponent: &str,
    subtitle: Option<String>,
    result: Option<String>,
    (w, h): (f32, f32),
    frames: u64,
    touch: bool,
    caught: bool,
) {
    use super::hud_art::{ARC_FRAMES, DIAL, EMBLEM, FEATHERS, HAND, ICON, KEYCAP, PIP, RIBBON, SHIELD, WING, BROOM};
    let s = (h / 720.0).clamp(0.5, 4.0);
    let t = frames as f32 / 60.0;
    let (gold, white_c, dim) = ([255, 214, 110, 255], [255, 255, 255, 255], [215, 205, 180, 255]);
    let ended = matches!(input.phase, super::rules::MatchPhase::Ended(_));
    let cx = w * 0.5;
    // The dial and what hangs on it live in the bottom left corner, at three quarters of their old size.
    let ds = s * 0.75;
    let d = DIAL * ds;
    let (dcx, dcy) = (24.0 * s + d * 0.5, h - 24.0 * s - d * 0.5 - 10.0 * ds);

    // The speed wings, either side of the dial: dim, then lit up to the charge, the feather in between fading
    // in, with a little breath of light for a ring just taken.
    let charge = value_of(items, HudKind::Charge).unwrap_or(0.0);
    let glow = value_of(items, HudKind::ChargeGlow).unwrap_or(0.0);
    let lit = charge * FEATHERS as f32;
    let (full, part) = (lit.floor() as usize, lit.fract());
    // At the top of the screen, in the middle, small (0.6) and a little see-through (0.8).
    const WING_SCALE: f32 = 0.6;
    const WING_ALPHA: f32 = 0.8;
    let (ww, wh) = (WING.0 * s * WING_SCALE, WING.1 * s * WING_SCALE);
    let wing_y = 14.0 * s;
    let gap = 44.0 * s;
    // Between the wings, the boost: a gold ring when it is ready (breathing while it runs), a dark one that
    // fills in while it cools down.
    let boost = value_of(items, HudKind::Boost).unwrap_or(1.0);
    let bs = 28.0 * s;
    let (bx, by) = (cx - bs * 0.5, wing_y + (wh - bs) * 0.5);
    if boost >= 2.0 {
        let k = 1.0 + 0.15 * (t * 18.0).sin().abs();
        pic(vm, art.hoop_on, [cx - bs * k * 0.5, by - bs * (k - 1.0) * 0.5, bs * k, bs * k], 1.0);
    } else if boost >= 1.0 {
        pic(vm, art.hoop_on, [bx, by, bs, bs], WING_ALPHA);
    } else {
        pic(vm, art.hoop_off, [bx, by, bs, bs], WING_ALPHA);
        pic(vm, art.hoop_on, [bx, by, bs, bs], boost * 0.6);
    }
    for (side, set) in [(0, &art.wing_l), (1, &art.wing_r)] {
        let x = if side == 0 { cx - gap * 0.5 - ww } else { cx + gap * 0.5 };
        pic(vm, set[0], [x, wing_y, ww, wh], WING_ALPHA);
        if full > 0 {
            pic(vm, set[full.min(FEATHERS)], [x, wing_y, ww, wh], WING_ALPHA);
        }
        if full < FEATHERS && part > 0.0 {
            pic(vm, set[full + 1], [x, wing_y, ww, wh], part * WING_ALPHA);
        }
        if glow > 0.0 && full > 0 {
            // The lit wing drawn again, a hair bigger and thin, is the flash.
            let grow = 3.0 * s * glow;
            pic(vm, set[full.min(FEATHERS)], [x - grow, wing_y - grow, ww + 2.0 * grow, wh + 2.0 * grow], glow * 0.3);
        }
    }

    // The dial: backing, the catch bar's arc, the Snitch (which beats when the catch is on).
    let (dx, dy) = (dcx - d * 0.5, dcy - d * 0.5);
    pic(vm, art.dial, [dx, dy, d, d], 1.0);
    let progress = value_of(items, HudKind::SnitchProgress).unwrap_or(0.0);
    let frame = (progress * ARC_FRAMES as f32).round() as usize;
    pic(vm, art.arc[frame.min(ARC_FRAMES)], [dx, dy, d, d], 1.0);
    let window = value_of(items, HudKind::CatchWindow);
    let beat = if window.is_some() { 1.0 + 0.12 * (t * 14.0).sin().abs() } else { 1.0 };
    let (ew, eh) = (EMBLEM.0 * ds * beat, EMBLEM.1 * ds * beat);
    pic(vm, art.emblem, [dcx - ew * 0.5, dcy - eh * 0.5, ew, eh], if progress > 0.0 || window.is_some() { 1.0 } else { 0.7 });

    // (No ring-run indicator on screen: the owner's word. The run still decides when the catch opens.)
    // One little Snitch under the dial for each catch left.
    let tries = value_of(items, HudKind::CatchPips).unwrap_or(0.0) as i32;
    let pw = PIP.0 * ds;
    let x0 = dcx - (tries as f32 * (pw + 6.0 * ds) - 6.0 * ds) * 0.5;
    for i in 0..tries.max(0) {
        pic(vm, art.pip, [x0 + i as f32 * (pw + 6.0 * ds), dcy + d * 0.5 - 6.0 * ds, pw, PIP.1 * ds], 1.0);
    }

    // Top left: the two shields with the score, and the brooms the Bludgers have left him.
    let (sw, sh) = (SHIELD.0 * s, SHIELD.1 * s);
    let home = value_of(items, HudKind::ScoreHome).unwrap_or(0.0) as i32;
    let away = value_of(items, HudKind::ScoreAway).unwrap_or(0.0) as i32;
    pic(vm, art.shield_home, [24.0 * s, 18.0 * s, sw, sh], 1.0);
    pic(vm, art.shield_away, [24.0 * s + sw + 10.0 * s, 18.0 * s, sw, sh], 1.0);
    let left = ((value_of(items, HudKind::Control).unwrap_or(1.0)) * input.bumps_to_lose.max(1) as f32).round() as u32;
    for i in 0..input.bumps_to_lose.clamp(1, 6) {
        let tex = if i < left { art.broom_ok } else { art.broom_lost };
        pic(vm, tex, [24.0 * s + i as f32 * (BROOM.0 + 4.0) * s, 18.0 * s + sh + 10.0 * s, BROOM.0 * s, BROOM.1 * s], 1.0);
    }

    let Some(f) = font else { return };
    centered(vm, f, &home.to_string(), 24.0 * s + sw * 0.5, 18.0 * s + sh * 0.27, white_c);
    centered(vm, f, &away.to_string(), 24.0 * s + sw * 1.5 + 10.0 * s, 18.0 * s + sh * 0.27, white_c);
    canvas::text(vm, f, &format!("Gryffindor - {opponent}"), 24.0 * s + 2.0 * sw + 24.0 * s, 18.0 * s + sh * 0.3, w, dim);

    // The catch prompt, while the window is open: a hand, pulsing, and the key (or the button on a phone).
    if let Some(left) = window {
        let pulse = 0.65 + 0.35 * (t * 10.0).sin().abs();
        let hs = HAND * s * (1.0 + 0.1 * pulse);
        let y = h * 0.52;
        pic(vm, art.hand, [cx - hs - 14.0 * s, y - hs * 0.5, hs, hs], pulse);
        let (kw, kh) = (KEYCAP.0 * s, KEYCAP.1 * s);
        pic(vm, art.keycap, [cx + 6.0 * s, y - kh * 0.5, kw, kh], 1.0);
        centered(vm, f, if touch { "JUMP" } else { "SPACE" }, cx + 6.0 * s + kw * 0.5, y - 11.0 * s, gold);        centered(vm, f, &format!("{left:.1}"), cx, y + hs * 0.5 + 4.0 * s, dim);
    }

    // The first seconds of a match: what the controls are, as pictures with a few words.
    if !ended && t < 11.0 {
        let a = ((11.0 - t) / 2.0).clamp(0.0, 1.0);
        let c = |base: [u8; 4]| [base[0], base[1], base[2], (base[3] as f32 * a) as u8];
        let rows: [(ObjId, &str); 3] = [
            (art.fly, if touch { "STICK: FLY" } else { "MOUSE / ARROWS: FLY" }),
            (art.hoop_on, "FLY THROUGH RINGS TO SPEED UP"),
            (art.hand, if touch { "JUMP: BOOST + CATCH" } else { "RIGHT CLICK / A: BOOST + CATCH" }),
        ];
        let is = ICON * s;
        let mut y = 20.0 * s;
        let x = w - 24.0 * s - 360.0 * s;
        for (tex, label) in rows {
            pic(vm, tex, [x, y, is, is], a);
            canvas::text(vm, f, label, x + is + 10.0 * s, y + is * 0.28, w, c(white_c));
            y += is + 6.0 * s;
        }
    }

    if let Some(line) = subtitle {
        centered(vm, f, &line, cx, h - 282.0 * s, gold);
    }
    if let Some(line) = result {
        let won = value_of(items, HudKind::Result) == Some(1.0);
        let rw = (RIBBON.0 * s).min(w * 0.86);
        let rh = rw * RIBBON.1 / RIBBON.0;
        let (rx, ry) = (cx - rw * 0.5, h * 0.42 - rh * 0.5);
        pic(vm, if won { art.ribbon_win } else { art.ribbon_lose }, [rx, ry, rw, rh], 1.0);
        centered(vm, f, &line, cx, ry + rh * 0.4, if won { gold } else { white_c });
        if caught {
            let (ew, eh) = (EMBLEM.0 * s * 1.3, EMBLEM.1 * s * 1.3);
            pic(vm, art.emblem, [cx - ew * 0.5, ry - eh - 4.0 * s, ew, eh], 1.0);
        }
    }
}

/// Draws the match's HUD over the game's own, in screen pixels. Called from `canvas::render`; a no-op on every
/// level that is not playing a match, which is every level but a Quidditch one.
pub fn draw_hud(vm: &mut Vm<'_>, width: u32, height: u32) {
    let Some(m) = crate::world(vm).quidditch.as_ref() else { return };
    // The SVG art is made for this screen size the first time it is wanted (and again if the size changes).
    let bucket = super::hud_art::bucket_for(height as f32);
    if m.show.art.as_ref().map(|a| a.bucket) != Some(bucket) && m.show.art_failed != Some(bucket) {
        let opponent = m.opponent.clone();
        let art = super::hud_art::HudArt::build(vm, bucket, &opponent);
        if art.is_none() {
            note(vm, format!("quidditch: the HUD's SVG art could not be made at size {bucket}, the plain HUD is used"));
        }
        if let Some(m) = crate::world(vm).quidditch.as_mut() {
            m.show.art_failed = art.is_none().then_some(bucket);
            m.show.art = art;
        }
    }
    let Some(m) = crate::world(vm).quidditch.as_ref() else { return };
    let (input, opponent) = (hud_input(m), m.opponent.clone());
    let (font, white) = (m.show.font, m.show.white);
    // The commentator's line is not put on screen (the owner's word); it is still said and logged.
    let subtitle: Option<String> = None;
    let items = hud::hud_items(&input);
    // No result banner or "won" text over the finish: the camera orbit is the whole of it (the owner's word).
    // The line is still logged by `glue` through `result_of`.
    let result: Option<String> = None;
    let (w, h) = (width as f32, height as f32);
    if let Some(art) = m.show.art.clone().filter(|a| a.bucket == bucket) {
        let touch = cfg!(target_os = "android") || m.touch_seen;
        let frames = m.frames;
        let caught = m.caught;
        draw_art(vm, &art, &items, &input, font, &opponent, subtitle, result, (w, h), frames, touch, caught);
        return;
    }
    // Everything is laid out against a 720-high screen and scaled, so the HUD keeps its share of any window.
    let s = (h / 720.0).clamp(0.5, 4.0);
    let gold = [255, 205, 70, 255];
    let white_c = [255, 255, 255, 255];
    let dim = [0, 0, 0, 150];

    // The catch bar, in the middle at the bottom: how close Harry is to the Snitch.
    let (bw, bh) = (w * 0.34, 16.0 * s);
    let (bx, by) = ((w - bw) * 0.5, h - 78.0 * s);
    bar(vm, white, [bx - 3.0 * s, by - 3.0 * s, bw + 6.0 * s, bh + 6.0 * s], dim);
    let fill = value_of(&items, HudKind::SnitchProgress).unwrap_or(0.0);
    bar(vm, white, [bx, by, bw * fill, bh], gold);
    if let Some(f) = font {
        canvas::text(vm, f, "SNITCH", bx, by - 20.0 * s, bx + bw, white_c);
        // The run of the Snitch's flight rings, at the other end of the catch bar: without it the catch window
        // never opens, so it belongs where the player is already looking.
        if let Some(line) = None::<String> {
            let tw = canvas::measure(vm, f, &line).0;
            canvas::text(vm, f, &line, bx + bw - tw, by - 20.0 * s, bx + bw, gold);
        }
    }
    // One pip per catch attempt left, under the bar.
    let pips = value_of(&items, HudKind::CatchPips).unwrap_or(0.0) as i32;
    for i in 0..pips.max(0) {
        bar(vm, white, [bx + i as f32 * 18.0 * s, by + bh + 6.0 * s, 12.0 * s, 8.0 * s], gold);
    }
    // Speed, as a thin bar along the right of the catch bar.
    let speed = value_of(&items, HudKind::Speed).unwrap_or(0.0);
    bar(vm, white, [bx, by + bh + 20.0 * s, bw, 5.0 * s], dim);
    bar(vm, white, [bx, by + bh + 20.0 * s, bw * speed, 5.0 * s], [120, 200, 255, 255]);
    // How much the Bludgers have left him.
    let control = value_of(&items, HudKind::Control).unwrap_or(1.0);
    bar(vm, white, [24.0 * s, 54.0 * s, 160.0 * s, 10.0 * s], dim);
    bar(vm, white, [24.0 * s, 54.0 * s, 160.0 * s * control, 10.0 * s], [220, 70, 60, 255]);

    let Some(f) = font else { return };
    let home = value_of(&items, HudKind::ScoreHome).unwrap_or(0.0) as i32;
    let away = value_of(&items, HudKind::ScoreAway).unwrap_or(0.0) as i32;
    canvas::text(vm, f, &format!("Gryffindor {home} - {away} {opponent}"), 24.0 * s, 24.0 * s, w, white_c);
    if value_of(&items, HudKind::Hints).is_some() {
        for (i, hint) in hud::HINTS.iter().enumerate() {
            canvas::text(vm, f, hint, 24.0 * s, h - 110.0 * s + i as f32 * 20.0 * s, w, [200, 200, 200, 255]);
        }
    }
    if let Some(line) = subtitle {
        let tw = canvas::measure(vm, f, &line).0;
        canvas::text(vm, f, &line, (w - tw) * 0.5, h - 130.0 * s, w, gold);
    }
    if let Some(line) = result {
        let tw = canvas::measure(vm, f, &line).0;
        bar(vm, white, [0.0, h * 0.42 - 14.0 * s, w, 44.0 * s], dim);
        canvas::text(vm, f, &line, (w - tw) * 0.5, h * 0.42, w, gold);
    }
}

/// The result line, once there is one. `glue` prints it through this, so the HUD's words and the log's are
/// the same.
pub fn result_of(m: &QuidditchMatch) -> Option<String> {
    hud::result_line(&hud_input(m), &m.opponent)
}
