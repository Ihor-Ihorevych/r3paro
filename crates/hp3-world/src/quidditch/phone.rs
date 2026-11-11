//! Where the phone meets the match: the touch layout's steering, and the frame budget.
//!
//! Both are the app's to measure and this module's to hand to the match, so neither `glue` nor the app has to
//! know the other. Nothing here does anything while no match is running, which is every level but a
//! Quidditch one.

use glam::Vec2;
use hp3_script::Vm;

use super::lod::Budget;
use super::touch::touch_input;

/// This frame's steering from the phone's touch layout: the steering stick's deflection (`None`: no finger on
/// it), a tap of the boost button this frame and the brake button held now. It is used instead of the
/// desktop's keys and mouse for the frame it is fed, and the boost toggle it flips is the match's own, so a
/// pause that stops the tick cannot reset it.
pub fn feed_touch(vm: &mut Vm<'_>, stick: Option<Vec2>, boost_tap: bool, brake_held: bool) {
    let Some(m) = crate::world(vm).quidditch.as_mut() else { return };
    let i = touch_input(stick, boost_tap, brake_held, &mut m.boost);
    m.pending_touch = Some(i);
}

/// How long the app's frames are taking on average, in milliseconds: the budget follows it (see
/// `quidditch::lod`). The budget now in force comes back, or `None` when no match is running.
///
/// `HP3_QUID_BUDGET=<players>` (debugging) pins it instead, to see a tight budget on a fast machine.
pub fn feed_frame_ms(vm: &mut Vm<'_>, avg_frame_ms: f32) -> Option<Budget> {
    let forced = forced_budget();
    let m = crate::world(vm).quidditch.as_mut()?;
    let before = m.budget;
    m.budget = match forced {
        Some(players) => {
            m.gov.force(players);
            m.gov.budget()
        }
        None => m.gov.frame(avg_frame_ms),
    };
    let now = m.budget;
    if now != before && crate::envc::var_os("HP3_QUID_DEBUG").is_some() {
        vm.log.push(format!(
            "quidditch: frames {avg_frame_ms:.1} ms, budget {} -> {} players steered, proxies from {:.0}",
            before.max_active_ai, now.max_active_ai, now.proxy_distance
        ));
    }
    Some(now)
}

/// The budget the match is keeping to, for whoever shows it (`None`: no match).
pub fn budget(vm: &mut Vm<'_>) -> Option<Budget> {
    Some(crate::world(vm).quidditch.as_ref()?.budget)
}

/// `HP3_QUID_BUDGET=<players>`: a budget pinned by hand, whatever the frames are doing.
fn forced_budget() -> Option<usize> {
    crate::envc::var_os("HP3_QUID_BUDGET")?.to_str()?.trim().parse().ok()
}
