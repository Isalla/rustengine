//! Billboard-Bausteine: Quad-Geometrie, kamera-zugewandte Transform und
//! UV-Rechtecke für Sheet-Texturen (SC1-Einheiten, Effekte, Icons).
//!
//! Ein Billboard ist hier bewusst kein eigener Draw-Typ, sondern ein
//! normales [`DrawBatch`](crate::backend::DrawBatch) mit `sprite:
//! Some(..)`: identischer Culling-/Sortier-/Backend-Pfad, nur eine
//! andere Pipeline (unlit + alpha-blend) im Backend.

use glam::{Mat4, Vec3};
use re_core::mesh::Mesh;

use crate::backend::Sprite;
use crate::camera::Camera;

/// Einheits-Quad (1×1) in der XY-Ebene, Front nach +Z, UV 0..1 mit
/// (0,0) oben links — wie die Pixelzeilen einer Sheet-Textur.
/// Alle Billboards einer Art teilen sich dieses Mesh (Backend-Cache).
pub fn unit_quad() -> Mesh {
    Mesh {
        name: "billboard".into(),
        positions: vec![
            [-0.5, 0.5, 0.0],
            [0.5, 0.5, 0.0],
            [0.5, -0.5, 0.0],
            [-0.5, -0.5, 0.0],
        ],
        normals: Some(vec![[0.0, 0.0, 1.0]; 4]),
        uvs: Some(vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]),
        indices: Some(vec![0, 1, 2, 0, 2, 3]),
        ..Default::default()
    }
}

/// Matrix, die das Einheits-Quad an `center` kamera-zugewandt aufstellt.
///
/// `yaw_only` hält das Quad senkrecht (nur um die Hochachse zur Kamera,
/// wie im Original und in VR üblich); sonst kippt es exakt mit ihr.
pub fn billboard_matrix(camera: &Camera, center: Vec3, width: f32, height: f32, yaw_only: bool) -> Mat4 {
    let c2w = camera.view.inverse();
    let (right, up, forward) = if yaw_only {
        // Horizontale Kamerarechte; Welt-Up; Front = rechtwinklig dazwischen.
        let mut flat = Vec3::new(c2w.x_axis.x, 0.0, c2w.x_axis.z);
        if flat.length_squared() < 1e-12 {
            flat = Vec3::X; // Kamera senkrecht nach unten: Rechte beliebig
        }
        let right = flat.normalize();
        (right, Vec3::Y, right.cross(Vec3::Y))
    } else {
        (
            c2w.x_axis.truncate(),
            c2w.y_axis.truncate(),
            c2w.z_axis.truncate(),
        )
    };
    Mat4::from_cols(
        (right * width).extend(0.0),
        (up * height).extend(0.0),
        forward.extend(0.0),
        center.extend(1.0),
    )
}

/// Pixelrechteck eines [`Sprite`] → normalisierter UV-Offset
/// `[u0, v0, du, dv]` (Textur-UVs: (0,0) = oben links).
pub fn uv_rect(sprite: &Sprite, tex_w: u32, tex_h: u32) -> [f32; 4] {
    let [x, y, w, h] = sprite.rect;
    [
        x as f32 / tex_w as f32,
        y as f32 / tex_h as f32,
        w as f32 / tex_w as f32,
        h as f32 / tex_h as f32,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> Camera {
        // MoBA-typisch: erhöht, schräg auf den Ursprung.
        Camera::perspective(Vec3::new(20.0, 30.0, 20.0), Vec3::ZERO, Vec3::Y, 0.9, 1.6)
    }

    #[test]
    fn quad_ist_einheitlich_und_uv_vollstaendig() {
        let q = unit_quad();
        assert_eq!(q.triangle_count(), 2);
        let (mn, mx) = q.aabb();
        assert_eq!((mn, mx), ([-0.5, -0.5, 0.0], [0.5, 0.5, 0.0]));
        let uvs = q.uvs.unwrap();
        assert_eq!(uvs[0], [0.0, 0.0]); // oben links (y>0)
        assert_eq!(uvs[2], [1.0, 1.0]);
    }

    #[test]
    fn billboard_zeigt_zur_kamera() {
        let cam = camera();
        let m = billboard_matrix(&cam, Vec3::new(4.0, 2.0, -6.0), 3.0, 6.0, true);
        // Mittelpunkt landet exakt auf center.
        let c = m.transform_point3(Vec3::new(0.0, 0.0, 0.0));
        assert!((c - Vec3::new(4.0, 2.0, -6.0)).length() < 1e-5);
        // yaw-only: Die Normale zeigt HORIZONTAL zur Kamera (Kamerahöhe
        // darf sie nicht kippen) — darum Vergleich in der XZ-Ebene.
        let n = m.transform_vector3(Vec3::Z);
        assert!(n.y.abs() < 1e-6, "yaw-only-Normale muss waagerecht sein");
        let eye = cam.view.inverse().w_axis.truncate();
        let to_cam = Vec3::new(eye.x - c.x, 0.0, eye.z - c.z).normalize();
        assert!(n.normalize().dot(to_cam) > 0.95);
        // Breite wirkt horizontal, Höhe vertikal (yaw-only: Up = Welt-Y).
        let w = m.transform_vector3(Vec3::X);
        let h = m.transform_vector3(Vec3::Y);
        assert!((h.length() - 6.0).abs() < 1e-5);
        assert!((h.y.abs() / h.length() - 1.0).abs() < 1e-5);
        assert!((w.length() - 3.0).abs() < 1e-5);
        assert!(w.y.abs() < 1e-5);
    }

    #[test]
    fn volle_kameraausrichtung_ohne_yaw_only() {
        let cam = camera();
        let m = billboard_matrix(&cam, Vec3::ZERO, 1.0, 1.0, false);
        let c2w = cam.view.inverse();
        // Ohne yaw_only sind die Spalten die Kamera-Achsen (skaliert).
        let d = m.transform_vector3(Vec3::Y).dot(c2w.y_axis.truncate()).abs();
        assert!((d - 1.0).abs() < 1e-6);
    }

    #[test]
    fn uv_rechteck_rechnet_prozentual() {
        let s = Sprite { texture: 0, rect: [64, 128, 32, 64] };
        let uv = uv_rect(&s, 2048, 640);
        let close = |a: f32, b: f32| (a - b).abs() < 1e-6;
        assert!(close(uv[0], 64.0 / 2048.0));
        assert!(close(uv[1], 128.0 / 640.0));
        assert!(close(uv[2], 32.0 / 2048.0));
        assert!(close(uv[3], 64.0 / 640.0));
    }
}
