//! Arena geometry shared by the Quidditch systems (a later task extends this).

use glam::Vec3;

/// Axis-aligned flight volume: x/y within `center +/- half`, z from
/// `center.z - half.z` up to `ceiling`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArenaBounds {
    pub center: Vec3,
    pub half: Vec3,
    pub ceiling: f32,
}
