//! Procedural motion so the world does not feel frozen: water ripples,
//! foliage swaying in the wind, floating crystals.
//!
//! Motion is a per-node and per-tile flag evaluated at draw time (viewer) —
//! no keyframes, no skinning involved. The offset is computed in WORLD space
//! from the moving vertex position, so it works for meshes, tilemap cells and
//! skinned rigs alike. The sprite baker (`iso::bake`) bakes at rest per
//! default; with `anim_frames` > 1 it renders one motion cycle per view
//! using [`Motion::offset_loop`], which guarantees a seamless loop.

use serde::{Deserialize, Serialize};

/// Loopable wind/water oscillation attached to a node or tile.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Motion {
    /// Static (the default — serialized maps stay compatible).
    None,
    /// Water surface: vertical ripple travelling across XZ.
    Waves { amp: f32, speed: f32 },
    /// Foliage: horizontal sway, zero at the base, strongest at the top.
    Sway { amp: f32, speed: f32 },
    /// Floating object: whole mesh bobs up and down.
    Bob { amp: f32, speed: f32 },
}

impl Default for Motion {
    fn default() -> Self {
        Motion::None
    }
}

impl Motion {
    /// True when nothing moves (used to skip serialization and per-vertex work).
    pub fn is_none(&self) -> bool {
        matches!(self, Motion::None)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Motion::None => "keine",
            Motion::Waves { .. } => "Wellen",
            Motion::Sway { .. } => "Schwingen",
            Motion::Bob { .. } => "Schweben",
        }
    }

    pub fn amp(&self) -> f32 {
        match self {
            Motion::None => 0.0,
            Motion::Waves { amp, .. } | Motion::Sway { amp, .. } | Motion::Bob { amp, .. } => *amp,
        }
    }

    pub fn speed(&self) -> f32 {
        match self {
            Motion::None => 0.0,
            Motion::Waves { speed, .. } | Motion::Sway { speed, .. } | Motion::Bob { speed, .. } => {
                *speed
            }
        }
    }

    /// World-space displacement of point `p` at time `t`. `y_floor`/`y_ceil`
    /// bound the object so [`Motion::Sway`] can taper with height; `phase`
    /// decorrelates neighbours (see [`cell_phase`], [`node_phase`]).
    pub fn offset(&self, p: glam::Vec3, y_floor: f32, y_ceil: f32, phase: f32, t: f32) -> glam::Vec3 {
        use glam::Vec3;
        match self {
            Motion::None => Vec3::ZERO,
            Motion::Waves { amp, speed } => {
                let w = speed.max(0.0);
                let ripple = (p.x * 1.6 + w * t + phase).sin()
                    + (p.z * 1.2 - w * 0.8 * t + phase * 1.7).sin();
                Vec3::new(0.0, amp * 0.5 * ripple, 0.0)
            }
            Motion::Sway { amp, speed } => {
                let span = (y_ceil - y_floor).max(1e-4);
                let hn = ((p.y - y_floor) / span).clamp(0.0, 1.0);
                let a = *amp * hn * hn;
                let w = speed.max(0.0);
                Vec3::new(
                    a * (w * t + phase).sin(),
                    0.0,
                    a * 0.6 * (w * 0.9 * t + phase * 1.3).cos(),
                )
            }
            Motion::Bob { amp, speed } => {
                Vec3::new(0.0, amp * (speed.max(0.0) * t + phase).sin(), 0.0)
            }
        }
    }

    /// Displacement for sprite-sequence baking: identical shape to
    /// [`Motion::offset`], but every time term repeats exactly when `t`
    /// advances by [`std::f32::consts::TAU`]. Baking N frames at
    /// `t = TAU·k/N` therefore yields a seamless loop.
    pub fn offset_loop(&self, p: glam::Vec3, y_floor: f32, y_ceil: f32, phase: f32, t: f32) -> glam::Vec3 {
        use glam::Vec3;
        match self {
            Motion::None => Vec3::ZERO,
            Motion::Waves { amp, speed } => {
                let _ = speed; // loop period is one cycle; amp scales it
                let ripple = (p.x * 1.6 + t + phase).sin()
                    + (p.z * 1.2 - t + phase * 1.7).sin();
                Vec3::new(0.0, amp * 0.5 * ripple, 0.0)
            }
            Motion::Sway { amp, speed } => {
                let _ = speed;
                let span = (y_ceil - y_floor).max(1e-4);
                let hn = ((p.y - y_floor) / span).clamp(0.0, 1.0);
                let a = *amp * hn * hn;
                Vec3::new(
                    a * (t + phase).sin(),
                    0.0,
                    a * 0.6 * (t + phase * 1.3).cos(),
                )
            }
            Motion::Bob { amp, speed } => {
                let _ = speed;
                Vec3::new(0.0, amp * (t + phase).sin(), 0.0)
            }
        }
    }
}

/// Stable phase per tile cell so a lake does not wave in lockstep.
pub fn cell_phase(x: i32, z: i32) -> f32 {
    let s = (x as f32 * 12.9898 + z as f32 * 78.233).sin() * 43758.5453;
    (s - s.floor()) * std::f32::consts::TAU
}

/// Stable phase per node id.
pub fn node_phase(id: u32) -> f32 {
    let s = (id as f32 * 0.6180339887) % 1.0;
    s * std::f32::consts::TAU
}

/// Does the document contain any moving object (nodes or kit tiles)? The
/// viewer uses this to decide whether to keep repainting.
pub fn any_motion(p: &crate::project::Project) -> bool {
    p.scene
        .nodes
        .values()
        .any(|n| !n.motion.is_none() && n.visible)
        || p.library
            .tilesets
            .values()
            .any(|ts| ts.tiles.iter().any(|t| !t.motion.is_none()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use crate::project::Project;

    #[test]
    fn waves_stay_within_amplitude_and_move() {
        let m = Motion::Waves { amp: 0.1, speed: 1.5 };
        let mut saw_peak = 0.0f32;
        let mut saw_trough = 0.0f32;
        for k in 0..64 {
            let t = k as f32 * 0.11;
            let d = m.offset(Vec3::new(2.0, 0.0, -1.0), -1.0, 1.0, 0.4, t);
            assert_eq!(d.x, 0.0);
            assert_eq!(d.z, 0.0);
            assert!(d.y.abs() <= 0.1 + 1e-5, "wave exceeds amp: {d:?}");
            saw_peak = saw_peak.max(d.y);
            saw_trough = saw_trough.min(d.y);
        }
        assert!(saw_peak > 0.02 && saw_trough < -0.02, "waves must oscillate");
    }

    #[test]
    fn sway_tapers_with_height() {
        let m = Motion::Sway { amp: 0.2, speed: 1.0 };
        let base = m.offset(Vec3::new(0.0, 0.0, 0.0), 0.0, 2.0, 0.0, 0.785_398_2);
        assert_eq!(base, Vec3::ZERO, "the trunk foot must not move");
        let top = m.offset(Vec3::new(0.0, 2.0, 0.0), 0.0, 2.0, std::f32::consts::FRAC_PI_2, 0.0);
        assert!((top.x - 0.2).abs() < 1e-5, "crown sways at full amp: {top:?}");
        assert!(top.length() <= 0.2 * 1.2 + 1e-4, "sway bounded by amp");
    }

    #[test]
    fn bob_is_pure_vertical_sin() {
        let m = Motion::Bob { amp: 0.1, speed: 2.0 };
        assert!(m.offset(Vec3::X * 7.0, 0.0, 1.0, 0.0, 0.0).length() < 1e-6);
        let up = m.offset(Vec3::X * 7.0, 0.0, 1.0, std::f32::consts::FRAC_PI_2, 0.0);
        assert!((up.y - 0.1).abs() < 1e-5);
        assert_eq!(up.x, 0.0);
    }

    #[test]
    fn none_never_moves_phases_are_stable() {
        assert_eq!(
            Motion::None.offset(Vec3::ONE, 0.0, 1.0, 0.0, 9.9),
            Vec3::ZERO
        );
        assert_eq!(cell_phase(3, -2), cell_phase(3, -2));
        assert_eq!(node_phase(5), node_phase(5));
        // neighbouring cells differ
        assert!((cell_phase(3, -2) - cell_phase(4, -2)).abs() > 1e-6);
    }

    #[test]
    fn loop_offset_repeats_after_full_cycle() {
        use std::f32::consts::TAU;
        let ms = [
            Motion::Waves { amp: 0.1, speed: 1.3 },
            Motion::Sway { amp: 0.2, speed: 0.7 },
            Motion::Bob { amp: 0.05, speed: 2.5 },
        ];
        for m in ms {
            for &(x, z) in &[(1.5, -2.0), (0.3, 0.9)] {
                let p = Vec3::new(x, 0.7, z);
                let a = m.offset_loop(p, 0.0, 2.0, 1.1, 0.0);
                let b = m.offset_loop(p, 0.0, 2.0, 1.1, TAU);
                assert!(
                    (a - b).length() < 1e-5,
                    "{m:?} must close its loop: {a:?} vs {b:?}"
                );
            }
            // sampling k/N frames hits every extremum at least twice over
            let mut spread = 0.0f32;
            for k in 0..8 {
                let d = m.offset_loop(Vec3::new(1.0, 1.5, -1.0), 0.0, 2.0, 0.5, TAU * k as f32 / 8.0);
                spread = spread.max(d.length());
            }
            assert!(spread > 1e-4, "{m:?} loop must actually move");
        }
    }

    #[test]
    fn any_motion_scans_nodes_and_kits() {
        let mut p = Project::new("welt");
        assert!(!any_motion(&p));
        let id = p.scene.alloc_node_id();
        let mut n = crate::scene::Node::default();
        n.motion = Motion::Bob { amp: 0.1, speed: 1.0 };
        p.scene.nodes.insert(id, n);
        assert!(any_motion(&p));
    }
}
