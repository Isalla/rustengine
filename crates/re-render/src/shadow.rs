//! Cascaded Shadow Maps — Kaskaden-Aufteilung (M5, Shadow 2.0).
//!
//! Nahe Schatten brauchen hohe Auflösung, ferne geringe. CSM schneidet die
//! Sichttiefe in mehrere Kaskaden und legt pro Kaskade eine eigene
//! Shadow-Map. Dieser Kern berechnet die Tiefen-Aufteilung **und** den
//! Light-Space-Fit pro Kaskade ([`fit_cascade`], Sphere-Fit mit
//! Texel-Snapping) — backend-neutral und headless testbar. Der GPU-Pass
//! muss dieselben Matrizen verwenden; die Map-Textur selbst folgt mit
//! der Pipeline.
//!
//! Aufteilung nach dem verbreiteten „practical split"-Schema: eine
//! Mischung aus logarithmischer und gleichmäßiger Verteilung.
//!
//! ```text
//! split(i) = lerp( log(i), uniform(i), lambda )
//!   lambda = 0  -> rein logarithmisch (packt Kaskaden nah an die Kamera)
//!   lambda = 1  -> gleichmäßig
//!   typisch     -> 0.5 .. 0.8
//! ```

/// Kaskaden-Konfiguration für die Tiefen-Aufteilung.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CascadeSetup {
    /// Anzahl Kaskaden (1..=4).
    pub count: usize,
    /// Kamera-Nahgrenze (> 0).
    pub near: f32,
    /// Reichweite der Schatten (= Kamera-Ferngrenze für den Schattenwurf).
    pub far: f32,
    /// Mischung Log↔Uniform, 0..=1.
    pub lambda: f32,
}

impl CascadeSetup {
    /// Konfiguration mit sinnvollen Klemmen: `count` auf 1..=4,
    /// `lambda` auf 0..=1, `far` strikt über `near`.
    pub fn new(count: usize, near: f32, far: f32, lambda: f32) -> Self {
        let near = near.max(1e-4);
        let far = far.max(near * 1.001);
        Self {
            count: count.clamp(1, 4),
            near,
            far,
            lambda: lambda.clamp(0.0, 1.0),
        }
    }

    /// Die `count + 1` Grenzen, monoton steigend, von `near` bis `far`.
    ///
    /// `boundaries()[0] == near`, `boundaries().last() == far`.
    pub fn boundaries(&self) -> Vec<f32> {
        let n = self.count;
        let mut out = Vec::with_capacity(n + 1);
        let ratio = self.far / self.near;
        for i in 0..=n {
            let f = i as f32 / n as f32;
            let uniform = self.near + (self.far - self.near) * f;
            let log = self.near * ratio.powf(f);
            // lerp(log, uniform, lambda): lambda=0 -> log, lambda=1 -> uniform.
            let split = log + (uniform - log) * self.lambda;
            out.push(if i == n { self.far } else { split });
        }
        out
    }

    /// Die `[near, far]`-Tiefenbereiche je Kaskade, aneinandergereiht.
    /// Länge == `count`; lückenlos und überlappungsfrei.
    pub fn cascade_ranges(&self) -> Vec<(f32, f32)> {
        let b = self.boundaries();
        b.windows(2).map(|w| (w[0], w[1])).collect()
    }
}

// ── Light-Space-Fit (Kaskade → Shadow-Map) ─────────────────────────────
//
// Der Standard-Weg (sphere fit): Die 8 Ecken des Kaskaden-Pyramidenstumpfs
// fassen eine Umkugel ein; die Licht-Kamera blickt entlang der
// Lichtrichtung auf ihren Mittelpunkt, eine Ortho-Matrix der Kantenlänge
// 2r projiziert sie lückenlos auf die Shadow-Map. Texel-Snapping quantisiert
// den Mittelpunkt auf das Texel-Raster der Map — sonst wimmelt die
// Schattenkante bei Kamerabewegung (berühmtes CSM-Flackern).

use glam::{Mat4, Vec3, Vec4};

/// Ergebnis des Light-Space-Fits einer Kaskade.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CascadeFit {
    /// Welt → Licht-Raum.
    pub light_view: Mat4,
    /// Licht-Raum → NDC (Ortho, Tiefe 0..1).
    pub light_proj: Mat4,
    /// Kombinierte Matrix — der Shadow-Pass benutzt direkt diese.
    pub light_view_proj: Mat4,
    /// (Gesnappter) Mittelpunkt der Umkugel in Weltkoordinaten.
    pub sphere_center: Vec3,
    /// Radius der Umkugel.
    pub radius: f32,
}

/// Die 8 Welt-Ecken des Kamera-Sichtkörpers zwischen den Sichttiefen
/// `near` und `far` (Konvention `clip = M·v`, Blick die −Z entlang).
fn cascade_corners(view: &Mat4, proj: &Mat4, near: f32, far: f32) -> [Vec3; 8] {
    let inv = (*proj * *view).inverse();
    let ndc_z = |d: f32| {
        let clip = *proj * Vec4::new(0.0, 0.0, -d, 1.0);
        clip.z / clip.w
    };
    let mut out = [Vec3::ZERO; 8];
    let mut i = 0;
    for &d in &[near, far] {
        let z = ndc_z(d);
        for &(x, y) in &[(-1.0, -1.0), (-1.0, 1.0), (1.0, -1.0), (1.0, 1.0)] {
            let h = inv * Vec4::new(x, y, z, 1.0);
            out[i] = h.truncate() / h.w;
            i += 1;
        }
    }
    out
}

/// Passt die Kaskade `(near, far)` in eine Shadow-Map `map_size × map_size`.
///
/// `light_dir` ist die Richtung, in die das Licht fällt (Sonnenuntergang:
/// y negativ); entartete Richtungen kippen auf −Z. Mit `snap` liegt der
/// Kugel-Mittelpunkt exakt auf dem Texel-Raster — die Map bleibt bei
/// Kamerabewegung stabil.
pub fn fit_cascade(
    cascade: (f32, f32),
    view: &Mat4,
    proj: &Mat4,
    light_dir: Vec3,
    map_size: u32,
    snap: bool,
) -> CascadeFit {
    let dir = if light_dir.length_squared() < 1e-12 {
        Vec3::NEG_Z
    } else {
        light_dir.normalize()
    };
    // Up-Wahl gegen degenerale look-at (dir parallel zu Y).
    let up = if dir.abs().dot(Vec3::Y) > 0.98 { Vec3::X } else { Vec3::Y };

    let corners = cascade_corners(view, proj, cascade.0, cascade.1);
    let center = corners.iter().fold(Vec3::ZERO, |a, c| a + *c) / 8.0;
    let radius = corners
        .iter()
        .map(|c| c.distance(center))
        .fold(0.0f32, f32::max)
        .max(1e-4);

    // Licht-Basis (Zeilen 0/1 = right/up) für das Snapping am Weltursprung.
    let view0 = Mat4::look_at_rh(center - dir * radius, center, up);
    let center_final = if snap && map_size > 0 {
        let texel = (2.0 * radius / map_size as f32).max(1e-6);
        let right = Vec3::new(view0.x_axis.x, view0.y_axis.x, view0.z_axis.x);
        let upv = Vec3::new(view0.x_axis.y, view0.y_axis.y, view0.z_axis.y);
        let u = (center.dot(right) / texel).round() * texel;
        let v = (center.dot(upv) / texel).round() * texel;
        right * u + upv * v + dir * center.dot(dir)
    } else {
        center
    };

    let light_view = Mat4::look_at_rh(center_final - dir * radius, center_final, up);
    let light_proj =
        Mat4::orthographic_rh(-radius, radius, -radius, radius, 0.0, 2.0 * radius);
    let light_view_proj = light_proj * light_view;
    CascadeFit {
        light_view,
        light_proj,
        light_view_proj,
        sphere_center: center_final,
        radius,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn grenzen_verankern_bei_near_und_far() {
        let s = CascadeSetup::new(4, 0.5, 200.0, 0.6);
        let b = s.boundaries();
        assert_eq!(b.len(), 5);
        assert!(approx(b[0], 0.5, 1e-5));
        assert!(approx(*b.last().unwrap(), 200.0, 1e-5));
    }

    #[test]
    fn grenzen_streng_monoton() {
        for lambda in [0.0, 0.3, 0.6, 1.0] {
            let s = CascadeSetup::new(4, 1.0, 500.0, lambda);
            let b = s.boundaries();
            for w in b.windows(2) {
                assert!(w[1] > w[0], "lambda={lambda}: {w:?} nicht steigend");
            }
        }
    }

    #[test]
    fn uniform_bei_lambda_eins() {
        // lambda = 1 -> gleichmäßige Abstände.
        let s = CascadeSetup::new(4, 0.0, 100.0, 1.0);
        let b = s.boundaries();
        let step = 25.0;
        for (i, &v) in b.iter().enumerate() {
            assert!(approx(v, step * i as f32, 1e-3), "uniform {i}: {v}");
        }
    }

    #[test]
    fn log_packt_nahe_bei_lambda_null() {
        // lambda = 0 -> logarithmisch: der erste Abstand ist der kleinste,
        // der letzte der größte (Kaskaden drängen sich nah an der Kamera).
        let s = CascadeSetup::new(4, 1.0, 256.0, 0.0);
        let b = s.boundaries();
        let gaps: Vec<f32> = b.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(gaps[0] < gaps[1] && gaps[1] < gaps[2] && gaps[2] < gaps[3], "log-Gaps {gaps:?}");
    }

    #[test]
    fn kaskaden_bereiche_lueckenlos() {
        let s = CascadeSetup::new(3, 2.0, 120.0, 0.7);
        let r = s.cascade_ranges();
        assert_eq!(r.len(), 3);
        assert!(approx(r[0].0, 2.0, 1e-5));
        assert!(approx(r[2].1, 120.0, 1e-5));
        for w in r.windows(2) {
            assert!(approx(w[0].1, w[1].0, 1e-5), "Lücke/Überlappung {w:?}");
        }
    }

    #[test]
    fn eine_kaskade_ist_ganzer_bereich() {
        let s = CascadeSetup::new(1, 0.5, 80.0, 0.5);
        let r = s.cascade_ranges();
        assert_eq!(r.len(), 1);
        assert!(approx(r[0].0, 0.5, 1e-5) && approx(r[0].1, 80.0, 1e-5));
    }

    #[test]
    fn klemmt_unsinnige_eingaben() {
        // count > 4 -> 4, count 0 -> 1.
        assert_eq!(CascadeSetup::new(9, 1.0, 100.0, 0.5).count, 4);
        assert_eq!(CascadeSetup::new(0, 1.0, 100.0, 0.5).count, 1);
        // far <= near wird auf near * 1.001 gehoben -> bleibt monoton.
        let s = CascadeSetup::new(2, 10.0, 5.0, 0.5);
        let b = s.boundaries();
        assert!(b.windows(2).all(|w| w[1] > w[0]), "far<=near: {b:?}");
        // lambda außerhalb 0..=1 wird geklemmt.
        assert_eq!(CascadeSetup::new(2, 1.0, 10.0, 3.0).lambda, 1.0);
        assert_eq!(CascadeSetup::new(2, 1.0, 10.0, -2.0).lambda, 0.0);
    }

    // ── Light-Space-Fit ────────────────────────────────────────────────

    fn test_cam() -> (Mat4, Mat4) {
        let view = Mat4::from_translation(Vec3::new(0.0, 1.5, 0.0)).inverse();
        let proj = Mat4::perspective_rh(std::f32::consts::FRAC_PI_3, 16.0 / 9.0, 0.5, 120.0);
        (view, proj)
    }

    #[test]
    fn ecken_lückenlos_in_der_ortho_box() {
        let (view, proj) = test_cam();
        let dir = Vec3::new(-0.4, -1.0, -0.3);
        for cascade in [(0.5f32, 10.0), (10.0, 40.0), (40.0, 120.0)] {
            let fit = fit_cascade(cascade, &view, &proj, dir, 1024, true);
            assert!(fit.radius > 0.0 && fit.radius.is_finite());
            for c in cascade_corners(&view, &proj, cascade.0, cascade.1) {
                let h = fit.light_view_proj * Vec4::new(c.x, c.y, c.z, 1.0);
                let n = h.truncate() / h.w;
                assert!(
                    n.x.abs() <= 1.0 + 1e-3 && n.y.abs() <= 1.0 + 1e-3
                        && n.z >= -1e-3 && n.z <= 1.0 + 1e-3,
                    "Kaskade {cascade:?}: Ecke {n:?} außerhalb NDC"
                );
            }
        }
    }

    #[test]
    fn snapping_legt_mittelpunkt_aufs_texelraster() {
        let (view, proj) = test_cam();
        let dir = Vec3::new(0.3, -1.0, 0.2);
        let map = 512;
        let fit = fit_cascade((1.0, 48.0), &view, &proj, dir, map, true);
        let c_ls = fit.light_view.transform_point3(fit.sphere_center);
        let texel = 2.0 * fit.radius / map as f32;
        // In Licht-Raum-x/y muss der Mittelpunkt auf einem Ganzzahl-Texel sitzen.
        for v in [c_ls.x / texel, c_ls.y / texel] {
            let f = (v - v.round()).abs();
            assert!(f < 1e-3, "Mittelpunkt nicht gesnappt: {v}");
        }
    }

    #[test]
    fn snapping_stabilisiert_gegen_kamerabewegung() {
        let (view, proj) = test_cam();
        let dir = Vec3::new(0.3, -1.0, 0.2).normalize();
        let a = fit_cascade((0.5, 40.0), &view, &proj, dir, 512, true);
        // Kamera um einen kleinen Bruchteil eines Texels verschieben.
        let shift = a.radius / 512.0 * 0.1;
        let view2 = Mat4::from_translation(Vec3::new(-shift, 0.0, -shift * 0.5)) * view;
        let b = fit_cascade((0.5, 40.0), &view2, &proj, dir, 512, true);
        let ta = a.light_view.transform_point3(a.sphere_center);
        let tb = b.light_view.transform_point3(b.sphere_center);
        let ra = 2.0 * a.radius / 512.0;
        let rb = 2.0 * b.radius / 512.0;
        // Gleiche Texel-Indizes -> die Map-Inhalte wandern nicht mit der Kamera.
        assert_eq!((ta.x / ra).round(), (tb.x / rb).round(), "x-Texel index kippt");
        assert_eq!((ta.y / ra).round(), (tb.y / rb).round(), "y-Texel index kippt");
        // Ohne Snapping würde sich die Licht-Kamera sofort mit bewegen.
        let c = fit_cascade((0.5, 40.0), &view2, &proj, dir, 512, false);
        assert_ne!(a.light_view.transform_point3(a.sphere_center).x, c.light_view.transform_point3(c.sphere_center).x);
    }

    #[test]
    fn entartete_richtung_und_blickachse_nan_frei() {
        let (view, proj) = test_cam();
        // dir = Nullvektor -> Fallback −Z; dir senkrecht (reines Y) -> up-Kip.
        for dir in [Vec3::ZERO, Vec3::Y, Vec3::new(0.0, -1.0, 0.0)] {
            let fit = fit_cascade((1.0, 20.0), &view, &proj, dir, 256, true);
            assert!(fit.light_view_proj.to_cols_array_2d().iter().flatten().all(|v| v.is_finite()), "NaN bei {dir:?}");
        }
    }
}
