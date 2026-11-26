//! Kameramathematik — bewusst GPU-API-frei, damit jedes Backend sie nutzt.

use glam::{Mat4, Vec3};

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub view: Mat4,
    pub proj: Mat4,
    /// RGBA, sRGB-0..1 wie im Szenen-Editor.
    pub clear_color: [f32; 4],
}

const NEAR: f32 = 0.1;
const FAR: f32 = 4000.0;

impl Camera {
    /// Perspektive (rechtshändig, Blick von `eye` nach `target`).
    pub fn perspective(eye: Vec3, target: Vec3, up: Vec3, fovy_rad: f32, aspect: f32) -> Self {
        Self {
            view: Mat4::look_at_rh(eye, target, up),
            proj: Mat4::perspective_rh(fovy_rad, aspect.max(1e-6), NEAR, FAR),
            clear_color: [0.02, 0.03, 0.05, 1.0],
        }
    }

    /// Orthografisch — Draufsicht fuer Maps und Minimap-Renderer.
    pub fn orthographic(eye: Vec3, target: Vec3, up: Vec3, height: f32, aspect: f32) -> Self {
        let h = height * 0.5;
        let w = h * aspect.max(1e-6);
        Self {
            view: Mat4::look_at_rh(eye, target, up),
            proj: Mat4::orthographic_rh(-w, w, -h, h, NEAR, FAR),
            clear_color: [0.0, 0.0, 0.0, 1.0],
        }
    }

    /// View*Proj — die einzige Matrix, die die meisten Shader sehen.
    pub fn view_proj(&self) -> Mat4 {
        self.proj * self.view
    }

    /// Lineare Tiefen-Spanne `(near, far)` einer Perspektiv-Projektion,
    /// aus der Matrix zurueckgewonnen — die Cluster-Scheiben brauchen
    /// sie, und der Renderer kennt nur die Matrix, nicht die Konstanten.
    ///
    /// fuer `perspective_rh` (Depth 0..1) gilt: `z_axis.z = f/(n-f)`,
    /// `w_axis.z = n*f/(n-f)`, `z_axis.w = -1`. Daraus folgt
    /// `n = w_axis.z / z_axis.z` und `f = w_axis.z / (z_axis.z + 1)`.
    /// `near` ist damit auf die Rundung der Division genau; `far`
    /// traegt eine Ausloeschung (nahe -1 plus 1) und erreicht nur
    /// ~1e-3 relative Genauigkeit — fuer logarithmische Cluster-
    /// Scheiben und konservatives Culling genuegend, weil CPU und GPU
    /// dieselben Werte teilen. Orthografische oder entartete Matrigen
    /// ergeben `None`.
    pub fn depth_range(&self) -> Option<(f32, f32)> {
        let zz = self.proj.z_axis.z;
        let wz = self.proj.w_axis.z;
        if self.proj.z_axis.w != -1.0 || !zz.is_finite() || !wz.is_finite() || zz >= 0.0 {
            return None;
        }
        let n = wz / zz;
        let f = wz / (zz + 1.0);
        if n > 0.0 && f > n {
            Some((n, f))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perspektiv_matrix_ist_nicht_entartet() {
        let cam = Camera::perspective(Vec3::Z * 10.0, Vec3::ZERO, Vec3::Y, std::f32::consts::FRAC_PI_3, 1.6);
        let p = cam.view_proj().transform_point3(Vec3::ZERO);
        // Ursprung vor der Kamera landet im sichtbaren Clip-Bereich (-1..1 in NDC-xy).
        assert!(p.x.abs() < 1e-3 && p.y.abs() < 1e-3);
    }

    #[test]
    fn depth_range_gibt_perspektive_zurueck_ortho_ist_none() {
        let cam = Camera::perspective(Vec3::Z * 2.0, Vec3::ZERO, Vec3::Y, std::f32::consts::FRAC_PI_3, 1.0);
        let (n, f) = cam.depth_range().expect("Perspektive hat eine Tiefen-Spanne");
        // Die Konstanten des Konstruktors (0.1 .. 4000). near ist auf
        // Divisionsrundung genau; far traegt die dokumentierte
        // Ausloeschung der Rueckgewinnung (~1e-3 relativ, gemessen
        // 3994.7) — Scheibengrenzen und Culling bleiben konsistent,
        // weil beide Seiten denselben Wert nutzen.
        assert!((n - 0.1).abs() < 1e-6, "near {n}");
        assert!((f - 4000.0).abs() / 4000.0 < 5e-3, "far {f}");
        let ortho = Camera::orthographic(Vec3::Z * 2.0, Vec3::ZERO, Vec3::Y, 2.0, 1.0);
        assert_eq!(ortho.depth_range(), None, "orthografisch hat keine");
    }
}
