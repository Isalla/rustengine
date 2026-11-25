//! G-Buffer-Bausteine für die Deferred-Pipeline (M5, Box 1).
//!
//! Deferred Rendering schreibt Szeneeigenschaften in mehrere Targets
//! (G-Buffer) und beleuchtet sie später im Screen-Space. Der knausrigste
//! Kanal sind die Oberflächennormalen: drei floats wären Verschwendung.
//! Die oktaedrische Kodierung packt eine Einheitsrichtung verlustarm in
//! zwei — Standard in modernen Deferred-Engines.
//!
//! Diese Datei enthält die Kodierung als backend-neutrale Referenz-Mathe;
//! der WGSL-Post-/Lighting-Pass spiegelt sie Kanal-für-Kanal, damit CPU
//! (Picking, Editor) und GPU dieselbe Repräsentation sehen.

use glam::Vec3;

/// Richtungsvektor → oktaedrische UV in `0..=1` (zwei Kanäle statt drei).
///
/// Die Richtung wird auf den Oktaeder projiziert; die untere Hemisphäre
/// wird nach außen gefaltet. Der Nullvektor gilt als `+Z` (gerade von der
/// Kamera weg ist der harmloseste Fall).
pub fn octahedral_encode(n: Vec3) -> [f32; 2] {
    let sum = n.x.abs() + n.y.abs() + n.z.abs();
    if sum <= f32::EPSILON {
        return [0.5, 0.5]; // +Z
    }
    let p = n / sum;
    let mut e = [p.x, p.y];
    if p.z < 0.0 {
        let sx = if e[0] >= 0.0 { 1.0 } else { -1.0 };
        let sy = if e[1] >= 0.0 { 1.0 } else { -1.0 };
        e = [
            (1.0 - e[1].abs()) * sx,
            (1.0 - e[0].abs()) * sy,
        ];
    }
    [e[0] * 0.5 + 0.5, e[1] * 0.5 + 0.5]
}

/// Oktaedrische UV (`0..=1`) → normierter Richtungsvektor.
///
/// Umkehrung von [`octahedral_encode`]; Werte außerhalb `0..=1` werden
/// auf das Quadrat geklemmt (robust gegen Rundung/Bilinear-Filterung).
pub fn octahedral_decode(uv: [f32; 2]) -> Vec3 {
    let e = [uv[0].clamp(0.0, 1.0) * 2.0 - 1.0, uv[1].clamp(0.0, 1.0) * 2.0 - 1.0];
    let z = 1.0 - e[0].abs() - e[1].abs();
    let v = if z >= 0.0 {
        Vec3::new(e[0], e[1], z)
    } else {
        let sx = if e[0] >= 0.0 { 1.0 } else { -1.0 };
        let sy = if e[1] >= 0.0 { 1.0 } else { -1.0 };
        Vec3::new((1.0 - e[1].abs()) * sx, (1.0 - e[0].abs()) * sy, z)
    };
    // Auf dem Oktaeder-Rand (|x|+|y| = 1) ist die Länge 1; sonst > 0.
    let len = v.length();
    if len <= f32::EPSILON {
        Vec3::Z
    } else {
        v / len
    }
}

/// Quantisiere kodierte UV auf `bits` Bit pro Kanal (z. B. 8 Bit = ein
/// R8G8-Target) — hilft, Präzisionsbudgets durchzurechnen.
pub fn quantize_uv(uv: [f32; 2], bits: u32) -> [f32; 2] {
    let steps = (1u32 << bits) - 1;
    uv.map(|c| (c.clamp(0.0, 1.0) * steps as f32).round() / steps as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn angle_deg(a: Vec3, b: Vec3) -> f32 {
        a.normalize().dot(b.normalize()).clamp(-1.0, 1.0).acos().to_degrees()
    }

    #[test]
    fn achsen_runden_exakt() {
        let achsen = [
            Vec3::X,
            -Vec3::X,
            Vec3::Y,
            -Vec3::Y,
            Vec3::Z,
            -Vec3::Z,
        ];
        for a in achsen {
            let back = octahedral_decode(octahedral_encode(a));
            assert!(angle_deg(a, back) < 1e-3, "{a:?} -> {back:?}");
        }
    }

    #[test]
    fn diagonale_runden_dicht() {
        let richtungen = [
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(-1.0, 0.2, -0.7),
            Vec3::new(0.0, -1.0, -1.0),
            Vec3::new(0.3, 0.3, -0.05),
            Vec3::new(-0.9, 0.1, 0.42),
        ];
        for r in richtungen {
            let back = octahedral_decode(octahedral_encode(r));
            assert!(angle_deg(r, back) < 1e-3, "{r:?} -> {back:?}");
        }
    }

    #[test]
    fn kodierte_uv_in_null_eins() {
        for i in 0..64u32 {
            let f = i as f32 / 64.0;
            let n = Vec3::new((f * 7.0).sin(), (f * 11.0).cos(), (f * 3.0).sin() - 0.3);
            let uv = octahedral_encode(n);
            assert!(
                uv[0] >= 0.0 && uv[0] <= 1.0 && uv[1] >= 0.0 && uv[1] <= 1.0,
                "{n:?} -> {uv:?}"
            );
        }
    }

    #[test]
    fn nullvektor_ist_plus_z() {
        assert_eq!(octahedral_encode(Vec3::ZERO), [0.5, 0.5]);
        assert_eq!(octahedral_decode([0.5, 0.5]), Vec3::Z);
    }

    #[test]
    fn encode_decode_encode_idempotent() {
        let n = Vec3::new(0.4, -0.8, 0.35);
        let uv1 = octahedral_encode(n);
        let uv2 = octahedral_encode(octahedral_decode(uv1));
        assert!((uv1[0] - uv2[0]).abs() < 1e-5 && (uv1[1] - uv2[1]).abs() < 1e-5);
    }

    #[test]
    fn quantisierung_8bit_haelt_winkel_klein() {
        // Das ist die Kern-Aussage: 2×8 Bit reichen für Normalen.
        let mut worst = 0.0f32;
        for i in 0..64u32 {
            let f = i as f32 / 64.0;
            let n = Vec3::new((f * 13.0).sin(), (f * 5.0).cos(), (f * 9.0).sin());
            let q = quantize_uv(octahedral_encode(n), 8);
            worst = worst.max(angle_deg(n, octahedral_decode(q)));
        }
        assert!(worst < 3.0, "8-Bit-Quantisierung: max {worst}°");
    }

    #[test]
    fn decode_klemmt_ausserhalb() {
        // Bilinear gefilterte Werte können knapp über 1 liegen — kein Crash,
        // kein NaN.
        let v = octahedral_decode([1.02, -0.01]);
        assert!(v.is_finite());
        assert!((v.length() - 1.0).abs() < 1e-5);
    }
}
