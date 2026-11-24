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
}
