//! Mouths that move with speech. A sound that carries a face envelope (a dialog line's curves, or a creature's amplitude bytes)
//! played on an actor with a jaw makes the jaw follow it: every tick the lower jaw bone is turned by how far open the envelope
//! says the mouth is at that moment in the sound, and when the sound is over it is let go.
//!
//! A dialog line's envelope holds 26 curves per frame at the rate in its header (30 a second). The layout of the 26 is not
//! documented anywhere; column 19 is the large smooth one that follows the speech (loudness and pauses), so it drives the
//! opening here, scaled by its own peak. Creature sounds carry one amplitude byte per 0.02 s.
//!
//! An actor whose animation set has a `mouth_anim` sequence (a mouth rig of its own) is moved through an animation channel
//! limited to the `mouth_root` bone instead, which plays `mouth_anim` at rate 0 with its frame set from the opening, and goes
//! back to `mouth_rest` when the sound is over.

use hp3_script::{ObjId, Vm};

use crate::anim::{channel_mut, play, set_of, skeleton_of};
use crate::{get, world};

/// The animation channel the mouth rig uses (the scripts keep 25 to 33 for lip sync).
const MOUTH_CHANNEL: usize = 25;
/// The curve (of 26 per frame) that follows the jaw.
const JAW_COLUMN: usize = 19;
const CURVES_PER_FRAME: usize = 26;
/// How far open the mouth rig goes at the loudest point, as a share of `mouth_anim`.
const MOST_OPEN: f32 = 0.63;
/// Seconds between the amplitude bytes of a creature's sound.
const BYTE_STEP: f32 = 0.02;

/// One mouth shape of a dialog line: the pose sequence played on its own animation channel, the factor its weight is
/// scaled by, and the curves of the envelope that drive it. HP3's own table has these nine, on channels 25 to 33.
struct Viseme {
    name: &'static str,
    gain: f32,
    columns: &'static [usize],
}

const VISEMES: [Viseme; 9] = [
    Viseme { name: "ai", gain: 1.0, columns: &[2] },
    Viseme { name: "e", gain: 0.6, columns: &[0] },
    Viseme { name: "o", gain: 1.0, columns: &[3] },
    Viseme { name: "u", gain: 1.0, columns: &[4] },
    Viseme { name: "cdgknrsthyz", gain: 1.0, columns: &[1, 6, 7, 9, 12, 14] },
    Viseme { name: "l", gain: 1.0, columns: &[10] },
    Viseme { name: "wq", gain: 1.0, columns: &[5, 13] },
    Viseme { name: "mbp", gain: 1.0, columns: &[11] },
    Viseme { name: "fv", gain: 1.0, columns: &[8] },
];
/// The channel of the first viseme (the scripts keep 25 to 33 for lip sync).
const VISEME_CHANNEL: usize = 25;
/// How much of a pose a weight of 1 gives, and how long a weight takes to follow a new value (seconds).
const VISEME_SCALE: f32 = 0.8;
const VISEME_BLEND: f32 = 0.2;
/// Seconds between two updates of the weights.
const VISEME_STEP: f32 = 0.033;
/// Weights at or under this do not count.
const VISEME_THRESHOLD: f32 = 0.001;

/// How a mouth is moved.
enum Mouth {
    /// A rig with `mouth_anim`: the animation channel's frame.
    Rig,
    /// The dialog mouth: nine pose channels limited to the mouth bone, weighted from the envelope's curves.
    Visemes { bone: String, header: [i32; 4], rows: Vec<f32>, last: f32 },
    /// A jaw bone (lower case name) turned about one axis.
    Jaw(String),
}

/// A mouth following a sound.
pub struct LipSync {
    actor: ObjId,
    start: f32,
    /// The mouth opening (0 to 1) at each step of the sound.
    opening: Vec<f32>,
    step: f32,
    mouth: Mouth,
    started: bool,
}

/// The mouth openings of a sound's envelope: one per step, and the step in seconds.
fn openings(envelope: &hp3_assets::sound::Envelope) -> Option<(Vec<f32>, f32)> {
    use hp3_assets::sound::Envelope;
    match envelope {
        Envelope::None => None,
        Envelope::Bytes(bytes) => {
            let peak = bytes.iter().copied().max().unwrap_or(0);
            (peak > 0).then(|| (bytes.iter().map(|&b| b as f32 / peak as f32).collect(), BYTE_STEP))
        }
        Envelope::Curves { header, values } => {
            let jaw: Vec<f32> = values.chunks_exact(CURVES_PER_FRAME).map(|row| row[JAW_COLUMN].max(0.0)).collect();
            let peak = jaw.iter().copied().fold(0.0f32, f32::max);
            let fps = if header[1] > 0 { header[1] as f32 } else { 30.0 };
            (peak > 0.0).then(|| (jaw.iter().map(|v| v / peak).collect(), 1.0 / fps))
        }
    }
}

/// The lower jaw bone of an actor's skeleton, if it has one.
fn jaw_bone(vm: &mut Vm<'_>, actor: ObjId) -> Option<String> {
    if crate::envc::var_os("HP3_LIPSYNC_WEIGHTS").is_some()
        && let Some(mesh) = get(vm, actor, "Mesh").as_obj()
        && let Some((pkg, index)) = vm.db.objects.get(mesh).key.clone()
        && let Ok(p) = vm.db.set.get(&pkg)
    {
        let chain = vm.db.set.export_chain(&p, index);
        if let Some(hp3_assets::Asset::SkeletalMesh(m)) = crate::native_asset(&p, index, &chain) {
            for (bi, b) in m.ref_skeleton.iter().enumerate() {
                let name = p.name(b.name).to_ascii_lowercase();
                if name.contains("jaw") || name.contains("mouth") || name.contains("head") || name.contains("tongue") {
                    let n = m.influences.iter().filter(|i| i.bone as usize == bi && i.weight > 0.01).count();
                    let pts: Vec<_> = m.influences.iter().filter(|i| i.bone as usize == bi && i.weight > 0.01).filter_map(|i| m.points.get(i.vertex as usize)).collect();
                    let c = pts.iter().fold((0.0f32, 0.0f32, 0.0f32), |a, p| (a.0 + p.x, a.1 + p.y, a.2 + p.z));
                    let k = pts.len().max(1) as f32;
                    let parent = m.ref_skeleton.get(bi).map(|b| b.parent as usize).and_then(|pi| m.ref_skeleton.get(pi)).map(|pb| p.name(pb.name).to_string());
                    eprintln!("lipsync: bone {bi} {name} (parent {parent:?}): {n} weighted vertices, centre ({:.1}, {:.1}, {:.1})", c.0 / k, c.1 / k, c.2 / k);
                }
            }
        }
    }
    if let Ok(b) = crate::envc::var("HP3_JAW_BONE") {
        return Some(b);
    }
    let mesh = get(vm, actor, "Mesh").as_obj()?;
    let sk = skeleton_of(vm, mesh)?;
    let names = &sk.skeleton.names;
    // The hinge of the lower jaw: head, mouth, jaw (this one), then the chin and tongue below it. Other characters name it alike.
    ["body__jaw", "jaw_bone"].iter().find_map(|want| names.iter().find(|n| n.as_str() == *want).cloned()).or_else(|| names.iter().find(|n| n.ends_with("__jaw") || n.ends_with("_jaw")).cloned())
}

/// The bone the mouth shapes are limited to: the pawn's `MouthBone` (the mouth, with the jaw, lips and tongue below it).
fn mouth_bone(vm: &mut Vm<'_>, actor: ObjId) -> Option<String> {
    let mesh = get(vm, actor, "Mesh").as_obj()?;
    let sk = skeleton_of(vm, mesh)?;
    let named = get(vm, actor, "MouthBone").as_name();
    let want = if named == hp3_script::Name::NONE { "body__mouth".to_string() } else { vm.db.names.get(named).to_ascii_lowercase() };
    sk.skeleton.names.iter().find(|n| n.eq_ignore_ascii_case(&want)).cloned()
}

/// The weight of each mouth shape at `t` seconds into a line: the line's curves at the frame for that time (the envelope's
/// own offset moves the first frame), the mean of the shape's curves that are on, times the shape's gain.
fn viseme_weights(header: &[i32; 4], rows: &[f32], t: f32) -> [f32; 9] {
    let fps = if header[1] > 0 { header[1] as f32 } else { 30.0 };
    let frame = ((fps * t) as i32).clamp(0, header[3].max(0));
    let count = rows.len() / CURVES_PER_FRAME;
    let row = ((frame - header[2]).max(0) as usize).min(count.saturating_sub(1));
    let mut out = [0.0; 9];
    for (w, v) in out.iter_mut().zip(VISEMES.iter()) {
        let (mut sum, mut n) = (0.0, 0);
        for &c in v.columns {
            let x = rows[row * CURVES_PER_FRAME + c];
            if x > VISEME_THRESHOLD {
                sum += x * v.gain;
                n += 1;
            }
        }
        *w = if n == 0 { 0.0 } else { sum / n as f32 };
    }
    out
}

/// One step of a dialog mouth; false when the line is over.
fn tick_visemes(vm: &mut Vm<'_>, l: &mut LipSync, now: f32) -> bool {
    let Mouth::Visemes { bone, header, rows, last } = &mut l.mouth else { return false };
    let t = (now - l.start).max(0.0);
    if !l.started {
        l.started = true;
        for (i, v) in VISEMES.iter().enumerate() {
            let name = vm.db.names.intern(v.name);
            if !play(vm, l.actor, name, 0.0, 0.0, VISEME_CHANNEL + i, false) {
                return false;
            }
            let frames = set_of(vm, l.actor).and_then(|s| s.find(v.name).map(|si| s.sequences[si].frames)).unwrap_or(2);
            let ch = channel_mut(vm, l.actor, VISEME_CHANNEL + i);
            ch.bone = Some(bone.clone());
            ch.scripted_blend = true;
            ch.alpha = 0.0;
            ch.target_alpha = 0.0;
            ch.rate = 0.0;
            ch.playing = true;
            // Held at the half of the sequence the original asks for: with two frames, the full shape.
            ch.frame = 0.5 * frames as f32;
        }
    }
    let fps = if header[1] > 0 { header[1] as f32 } else { 30.0 };
    // After the last frame the weights fade out and the mouth is let go.
    if t * fps > header[3] as f32 + 6.0 {
        release_visemes(vm, l.actor);
        return false;
    }
    if t - *last >= VISEME_STEP {
        *last = t;
        let weights = viseme_weights(header, rows, t);
        if crate::envc::var_os("HP3_LIPSYNC_DEBUG").is_some() && world(vm).frame % 15 == 0 {
            let frame = world(vm).frame;
            eprintln!("lipsync: tick {frame} t {t:.2} weights {}", weights.iter().map(|w| format!("{w:.2}")).collect::<Vec<_>>().join(" "));
        }
        for (i, w) in weights.iter().enumerate() {
            let ch = channel_mut(vm, l.actor, VISEME_CHANNEL + i);
            ch.target_alpha = VISEME_SCALE * w;
            ch.blend_rate = 1.0 / VISEME_BLEND;
        }
    }
    true
}

/// The mouth shapes fade out.
fn release_visemes(vm: &mut Vm<'_>, actor: ObjId) {
    for i in 0..VISEMES.len() {
        let ch = channel_mut(vm, actor, VISEME_CHANNEL + i);
        ch.target_alpha = 0.0;
        ch.blend_rate = 1.0 / VISEME_BLEND;
    }
}

/// A sound has just been asked for on `actor`: its mouth starts to follow it, if the sound has an envelope and the actor a mouth.
pub fn start(vm: &mut Vm<'_>, actor: ObjId, sound: &(String, usize)) {
    let Ok(pkg) = vm.db.set.get(&sound.0) else { return };
    let chain = vm.db.set.export_chain(&pkg, sound.1);
    let Some(hp3_assets::Asset::Sound(s)) = crate::native_asset(&pkg, sound.1, &chain) else { return };
    if crate::envc::var_os("HP3_LIPSYNC_DUMP").is_some()
        && let hp3_assets::sound::Envelope::Curves { header, values } = &s.envelope
    {
        eprintln!("envelope {} header {header:?}", crate::audio::sound_name(vm, sound));
        for (i, row) in values.chunks_exact(CURVES_PER_FRAME).enumerate() {
            eprintln!("env {i} {}", row.iter().map(|v| format!("{v:.2}")).collect::<Vec<_>>().join(" "));
        }
    }
    // A dialog line on a speaker whose animation set holds the mouth shapes: HP3's own way, one pose channel per shape.
    if let hp3_assets::sound::Envelope::Curves { header, values } = &s.envelope
        && values.len() >= CURVES_PER_FRAME
        && let Some(set) = set_of(vm, actor)
        && VISEMES.iter().all(|v| set.find(v.name).is_some())
        && let Some(bone) = mouth_bone(vm, actor)
    {
        let now = world(vm).time;
        if let Some(old) = world(vm).lip_syncs.iter().position(|l| l.actor == actor) {
            let old = world(vm).lip_syncs.remove(old);
            release(vm, &old);
        }
        let mouth = Mouth::Visemes { bone, header: *header, rows: values.clone(), last: f32::MIN };
        world(vm).lip_syncs.push(LipSync { actor, start: now, opening: Vec::new(), step: 1.0 / 30.0, mouth, started: false });
        return;
    }
    let Some((opening, step)) = openings(&s.envelope) else { return };
    let mouth = if set_of(vm, actor).is_some_and(|set| set.find("mouth_anim").is_some()) {
        Mouth::Rig
    } else if let Some(bone) = jaw_bone(vm, actor) {
        Mouth::Jaw(bone)
    } else {
        return;
    };
    if crate::envc::var_os("HP3_LIPSYNC_DEBUG").is_some() {
        let name = crate::audio::sound_name(vm, sound);
        eprintln!("lipsync: {} starts {name} ({} steps of {:.3} s)", hp3_script::natives::object_path(vm, actor), opening.len(), step);
    }
    let now = world(vm).time;
    if let Some(old) = world(vm).lip_syncs.iter().position(|l| l.actor == actor) {
        let old = world(vm).lip_syncs.remove(old);
        release(vm, &old);
    }
    world(vm).lip_syncs.push(LipSync { actor, start: now, opening, step, mouth, started: false });
}

/// The jaw's turn for an opening of 0 to 1. The jaw bone's own X runs forward to the chin and its Z across the face, so the hinge is yaw
/// (axis 1) and a negative angle swings the chin down; roll (the old choice) only twists the mouth corners. `HP3_JAW_AXIS` picks another.
fn jaw_turn(open: f32) -> [i32; 3] {
    static AXIS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let mut axis = *AXIS.get_or_init(|| crate::envc::var("HP3_JAW_AXIS").ok().and_then(|v| v.parse().ok()).unwrap_or(1));
    if let Ok(cycle) = crate::envc::var("HP3_JAW_CYCLE") {
        // HP3_JAW_CYCLE=<ticks>: the axis changes every that many ticks (0, 1, 2), counted from tick 300; every fourth stretch has no turn, as a control.
        let n = cycle.parse::<u64>().unwrap_or(60).max(1);
        let stage = (CYCLE_TICK.load(std::sync::atomic::Ordering::Relaxed).saturating_sub(300) / n) % 4;
        if stage == 3 {
            return [0; 3];
        }
        axis = stage as usize;
    }
    // HP3_JAW_DEG=<degrees> sets the opening angle and HP3_JAW_HOLD keeps the jaw fully open (to find the right axis).
    let deg = crate::envc::var("HP3_JAW_DEG").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(-25.0);
    let open = if crate::envc::var_os("HP3_JAW_HOLD").is_some() { 1.0 } else { open };
    let mut turn = [0; 3];
    turn[axis.min(2)] = (open * 65536.0 * deg / 360.0) as i32;
    turn
}

static CYCLE_TICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn set_jaw(vm: &mut Vm<'_>, actor: ObjId, bone: &str, turn: [i32; 3]) {
    let a = world(vm).anim.actors.entry(actor).or_default();
    match a.bone_turns.iter_mut().find(|t| t.0 == bone) {
        Some(t) => {
            t.1 = turn;
            t.2 = 1.0;
        }
        None => a.bone_turns.push((bone.to_string(), turn, 1.0)),
    }
}

/// A mouth lets go: the jaw bone is released, the rig goes back to its rest pose.
fn release(vm: &mut Vm<'_>, l: &LipSync) {
    match &l.mouth {
        Mouth::Visemes { .. } => release_visemes(vm, l.actor),
        Mouth::Jaw(bone) => {
            if let Some(a) = world(vm).anim.actors.get_mut(&l.actor) {
                a.bone_turns.retain(|t| &t.0 != bone);
            }
        }
        Mouth::Rig => {
            let rest = vm.db.names.intern("mouth_rest");
            if play(vm, l.actor, rest, 1.0, 1.0, MOUTH_CHANNEL, false) {
                let ch = channel_mut(vm, l.actor, MOUTH_CHANNEL);
                ch.bone = Some("mouth_root".to_string());
                ch.scripted_blend = true;
            }
        }
    }
}

/// Moves every speaking mouth; called once a tick.
pub fn tick(vm: &mut Vm<'_>) {
    if world(vm).lip_syncs.is_empty() {
        return;
    }
    let now = world(vm).time;
    let mut list = std::mem::take(&mut world(vm).lip_syncs);
    list.retain_mut(|l| {
        if vm.db.objects.get(l.actor).deleted {
            return false;
        }
        if matches!(l.mouth, Mouth::Visemes { .. }) {
            return tick_visemes(vm, l, now);
        }
        let t = (now - l.start).max(0.0);
        let at = t / l.step;
        CYCLE_TICK.store(world(vm).frame, std::sync::atomic::Ordering::Relaxed);
        let hold = crate::envc::var_os("HP3_JAW_HOLD").is_some();
        if at >= l.opening.len() as f32 && !hold {
            release(vm, l);
            return false;
        }
        let at = at.min(l.opening.len() as f32 - 1.0);
        let (i, f) = (at.floor() as usize, at.fract());
        let (a, b) = (l.opening[i], l.opening[(i + 1).min(l.opening.len() - 1)]);
        let open = a + (b - a) * f;
        match &l.mouth {
            Mouth::Visemes { .. } => {}
            Mouth::Jaw(bone) => {
                set_jaw(vm, l.actor, bone, jaw_turn(open));
                if crate::envc::var_os("HP3_LIPSYNC_DEBUG").is_some() && world(vm).frame % 30 == 0 {
                    let n = world(vm).anim.actors.get(&l.actor).map_or(0, |a| a.bone_turns.len());
                    let frame = world(vm).frame;
                    let path = hp3_script::natives::object_path(vm, l.actor);
                    eprintln!("lipsync: tick {frame} {path} jaw {bone} open {open:.2} turn {:?} (bone turns held: {n})", jaw_turn(open));
                }
            }
            Mouth::Rig => {
                let Some(set) = set_of(vm, l.actor) else { return false };
                if !l.started {
                    l.started = true;
                    let anim = vm.db.names.intern("mouth_anim");
                    if !play(vm, l.actor, anim, 0.0, 0.0, MOUTH_CHANNEL, false) {
                        return false;
                    }
                    let ch = channel_mut(vm, l.actor, MOUTH_CHANNEL);
                    ch.bone = Some("mouth_root".to_string());
                    ch.scripted_blend = true;
                    ch.alpha = 1.0;
                    ch.target_alpha = 1.0;
                    ch.rate = 0.0;
                    ch.playing = true;
                }
                let frames = set.find("mouth_anim").map_or(1, |s| set.sequences[s].frames);
                channel_mut(vm, l.actor, MOUTH_CHANNEL).frame = open * MOST_OPEN * (frames as f32 - 1.0).max(0.0);
            }
        }
        true
    });
    // New trackers may have been added while these were stepped (a line starting inside a script); keep both.
    let added = std::mem::take(&mut world(vm).lip_syncs);
    list.extend(added);
    world(vm).lip_syncs = list;
}

/// The actor of the line that started first and is still going (for the test camera).
pub fn speaker(vm: &mut Vm<'_>) -> Option<ObjId> {
    world(vm).lip_syncs.first().map(|l| l.actor)
}
