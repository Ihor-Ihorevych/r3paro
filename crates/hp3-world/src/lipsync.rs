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

/// How a mouth is moved.
enum Mouth {
    /// A rig with `mouth_anim`: the animation channel's frame.
    Rig,
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

/// A sound has just been asked for on `actor`: its mouth starts to follow it, if the sound has an envelope and the actor a mouth.
pub fn start(vm: &mut Vm<'_>, actor: ObjId, sound: &(String, usize)) {
    let Ok(pkg) = vm.db.set.get(&sound.0) else { return };
    let chain = vm.db.set.export_chain(&pkg, sound.1);
    let Some(hp3_assets::Asset::Sound(s)) = crate::native_asset(&pkg, sound.1, &chain) else { return };
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

/// The jaw's turn: roll (axis 2) opens the mouth on Harry's rig (found by trying the three; `HP3_JAW_AXIS` picks another), for an opening of 0 to 1.
fn jaw_turn(open: f32) -> [i32; 3] {
    static AXIS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let mut axis = *AXIS.get_or_init(|| crate::envc::var("HP3_JAW_AXIS").ok().and_then(|v| v.parse().ok()).unwrap_or(2));
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
    let deg = crate::envc::var("HP3_JAW_DEG").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(25.0);
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
