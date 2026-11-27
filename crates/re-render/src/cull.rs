//! Culling-Mathematik für den GPU-getriebenen Draw-Pfad (M5).
//!
//! Der GPU-Driven-Pfad (Roadmap M5) lässt einen Compute-Shader entscheiden,
//! was gezeichnet wird: erst Frustum-Test, dann Hi-Z-Occlusion-Test gegen
//! die Depth-Pyramide des Vorschaubilds, Sichtbares wandert in den
//! Indirect-Draw-Buffer. Damit die GPU-Fassung prüfbar bleibt, lebt die
//! Referenz-Mathe hier — backend-neutral, deterministisch, headless testbar:
//!
//! 1. [`Frustum::from_view_proj`] — Ebenen aus der View-Proj-Matrix
//!    (Gribb/Hartmann, in der Konvention `clip = M·v`).
//! 2. [`Frustum::intersects_sphere`] / [`Frustum::intersects_aabb`] —
//!    konservative Tests ohne False Negatives.
//! 3. [`HiZPyramid`] — Min-Reduktion der Tiefe (0 = nah, 1 = fern) und
//!    der Okklusionstest: Ein Objekt ist verdeckt, wenn die nächste
//!    Szenentiefe des Pyramidensamples nicht weiter entfernt ist als die
//!    naheste Objekt-Ebene.

use glam::{Mat4, Vec3};

/// Eine Frustum-Ebene in Normalform: `n·p + d >= 0` heißt „innen".
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub n: Vec3,
    pub d: f32,
}

impl Plane {
    /// Signed Distance: > 0 = innen (vor der Ebene), < 0 = außen.
    pub fn signed_distance(&self, p: Vec3) -> f32 {
        self.n.dot(p) + self.d
    }
}

/// Der Sichtkörper als sechs Ebenen (links, rechts, unten, oben, nah, fern).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frustum {
    pub planes: [Plane; 6],
}

/// Zeile `i` einer Matrix (Spalten-Vektoren von glam zusammenlesen).
fn row(m: &Mat4, i: usize) -> glam::Vec4 {
    glam::Vec4::new(m.x_axis[i], m.y_axis[i], m.z_axis[i], m.w_axis[i])
}

impl Frustum {
    /// Ebenen aus `view_proj` extrahieren (Konvention `clip = M·v`,
    /// NDC-Bereich -1..1). Jede Ebene wird normalisiert, damit
    /// [`Plane::signed_distance`] Welt-Einheiten liefert.
    pub fn from_view_proj(view_proj: &Mat4) -> Self {
        let r0 = row(view_proj, 0);
        let r1 = row(view_proj, 1);
        let r2 = row(view_proj, 2);
        let r3 = row(view_proj, 3);
        // (plus, minus) Paare: links/rechts, unten/oben, nah/fern.
        let raw = [
            r3 + r0,
            r3 - r0,
            r3 + r1,
            r3 - r1,
            r3 + r2,
            r3 - r2,
        ];
        let planes = raw.map(|v| {
            let n = Vec3::new(v.x, v.y, v.z);
            let len = n.length().max(1e-9);
            Plane { n: n / len, d: v.w / len }
        });
        Self { planes }
    }

    /// Konservative Kugel-Prüfung: schneidet die Kugel alle Ebenen
    /// (Abstand >= -radius)? Liefert nie ein False Negative.
    pub fn intersects_sphere(&self, center: Vec3, radius: f32) -> bool {
        self.planes
            .iter()
            .all(|p| p.signed_distance(center) >= -radius)
    }

    /// Konservative AABB-Prüfung (Zentrum + halbe Ausdehnung). Pro Ebene
    /// exakt gegen die Kugel mit Radius `|n|·half` — damit ebenfalls ohne
    /// False Negatives.
    pub fn intersects_aabb(&self, center: Vec3, half: Vec3) -> bool {
        self.planes.iter().all(|p| {
            let abs_n = Vec3::new(p.n.x.abs(), p.n.y.abs(), p.n.z.abs());
            p.signed_distance(center) >= -abs_n.dot(half)
        })
    }
}

/// Pyramide aus minimierten Tiefen (0 = nah, 1 = fern).
///
/// Level 0 ist die Auflösung des Depth-Buffers; jedes folgende Level ist
/// die 2×2-Min-Reduktion des vorigen („nächste Tiefe der Region"). Die
/// GPU-Fassung macht dasselbe per Compute/Mip-Generation mit Min-Filter.
#[derive(Clone, Debug, PartialEq)]
pub struct HiZPyramid {
    pub width: u32,
    pub height: u32,
    /// Level 0 zuerst; jedes Level `l` hat `ceil(width/2^l) × ceil(height/2^l)`.
    pub levels: Vec<Vec<f32>>,
}

impl HiZPyramid {
    /// Pyramide der gegebenen Größe; alle Level initial `1.0` (= fern,
    /// verdeckt nichts).
    pub fn new(width: u32, height: u32) -> Self {
        let mut levels = Vec::new();
        let (mut w, mut h) = (width.max(1), height.max(1));
        loop {
            levels.push(vec![1.0; (w as usize) * (h as usize)]);
            if w == 1 && h == 1 {
                break;
            }
            w = w.div_ceil(2).max(1);
            h = h.div_ceil(2).max(1);
        }
        Self { width: width.max(1), height: height.max(1), levels }
    }

    /// Anzahl Level.
    pub fn level_count(&self) -> u32 {
        self.levels.len() as u32
    }

    /// Pyramide aus einem Depth-Buffer (Level-0-Größe) aufbauen.
    /// Falsche Datenlänge ist ein Aufrufer-Fehler → Panic mit Ansage.
    pub fn build_from_depth(&mut self, depth: &[f32]) {
        assert_eq!(
            depth.len(),
            (self.width * self.height) as usize,
            "Depth-Buffer passt nicht zur Pyramiden-Grundfläche"
        );
        self.levels[0].copy_from_slice(depth);
        for l in 1..self.levels.len() {
            let prev_w = self.level_width((l - 1) as u32) as usize;
            let prev_h = self.level_height((l - 1) as u32) as usize;
            let w = self.level_width(l as u32) as usize;
            let h = self.level_height(l as u32) as usize;
            for y in 0..h {
                for x in 0..w {
                    let mut m = f32::MAX;
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let (sx, sy) = (x * 2 + dx, y * 2 + dy);
                            if sx < prev_w && sy < prev_h {
                                m = m.min(self.levels[l - 1][sy * prev_w + sx]);
                            }
                        }
                    }
                    self.levels[l][y * w + x] = m;
                }
            }
        }
    }

    pub fn level_width(&self, l: u32) -> u32 {
        // ceil, not floor: level l really holds ceil(width / 2^l) texels
        // (div_ceil chain in `new`) — floor here would read past row ends.
        self.width.div_ceil(1u32 << l.min(31)).max(1)
    }

    pub fn level_height(&self, l: u32) -> u32 {
        self.height.div_ceil(1u32 << l.min(31)).max(1)
    }

    /// Level, auf dem ein `w×h` großes Bildschirm-Fenster in genau ein
    /// Texel passt (log2 der Kantenlänge, aufwärts).
    pub fn level_for_size(w: u32, h: u32) -> u32 {
        let m = w.max(h).max(1);
        (u32::BITS - m.leading_zeros() - 1) + (m.count_ones() > 1) as u32
    }

    /// Nächste Szenentiefe (Minimum) im Fenster `[x0..x1) × [y0..y1)`
    /// eines Levels, geclampt auf die Level-Grenzen.
    pub fn min_depth_in(&self, level: u32, x0: u32, y0: u32, x1: u32, y1: u32) -> f32 {
        let l = (level as usize).min(self.levels.len() - 1);
        let lx = self.level_width(l as u32);
        let ly = self.level_height(l as u32);
        let w = lx as usize;
        let (xa, xb) = ((x0.min(lx)) as usize, (x1.max(x0 + 1).min(lx)) as usize);
        let (ya, yb) = ((y0.min(ly)) as usize, (y1.max(y0 + 1).min(ly)) as usize);
        let mut m = f32::MAX;
        for y in ya..yb {
            for x in xa..xb {
                m = m.min(self.levels[l][y * w + x]);
            }
        }
        m
    }

    /// Konservativer Okklusionstest: Ist ein Objekt, dessen Bildschirm-\
    /// fenster auf Level `level` in das Texel `(tx, ty)` fällt und dessen
    /// **naheste** Tiefe `near_depth` ist, vollständig verdeckt?
    ///
    /// Verdeckt, wenn die nächste Szenentiefe im Region-Minimum
    /// (`<= near_depth + epsilon`) nicht hinter dem Objektnahrand liegt.
    pub fn occludes(&self, level: u32, tx: u32, ty: u32, near_depth: f32, epsilon: f32) -> bool {
        self.min_depth_in(level, tx, ty, tx + 1, ty + 1) <= near_depth + epsilon
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn persp(aspect: f32) -> Mat4 {
        // 90° fovy, RH, Blick die -Z entlang.
        Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, aspect, 0.1, 100.0)
    }

    #[test]
    fn frustum_punkte_richtig() {
        let f = Frustum::from_view_proj(&persp(1.0));
        assert!(f.intersects_sphere(Vec3::new(0.0, 0.0, -5.0), 0.0), "vorne Mitte");
        assert!(!f.intersects_sphere(Vec3::new(0.0, 0.0, 5.0), 0.0), "hinter Kamera");
        assert!(!f.intersects_sphere(Vec3::new(0.0, 0.0, -200.0), 0.5), "hinter fern");
        assert!(!f.intersects_sphere(Vec3::new(0.0, 0.0, -0.02), 0.0), "vor nah");
        assert!(!f.intersects_sphere(Vec3::new(50.0, 0.0, -5.0), 0.5), "rechts vorbei");
        // Bei 90° und -5 Entfernung ragt x=±4, y=±4 gerade noch hinein.
        assert!(f.intersects_sphere(Vec3::new(3.9, 0.0, -5.0), 0.0));
        assert!(f.intersects_sphere(Vec3::new(0.0, -3.9, -5.0), 0.0));
    }

    #[test]
    fn kugel_am_rand_schneidet() {
        let f = Frustum::from_view_proj(&persp(1.0));
        // Mittelpunkt knapp außerhalb der rechten Ebene, Radius erreicht sie.
        let center = Vec3::new(5.2, 0.0, -5.0);
        assert!(!f.intersects_sphere(center, 0.0));
        assert!(f.intersects_sphere(center, 1.5), "Radius muss reinschneiden");
    }

    #[test]
    fn aabb_test_konservativ() {
        let f = Frustum::from_view_proj(&persp(1.0));
        // Box, die von außen in den Frustum ragt: muss getroffen werden.
        assert!(f.intersects_aabb(Vec3::new(6.0, 0.0, -5.0), Vec3::new(3.0, 1.0, 1.0)));
        // Box weit rechts außerhalb: darf verworfen werden.
        assert!(!f.intersects_aabb(Vec3::new(50.0, 0.0, -5.0), Vec3::new(1.0, 1.0, 1.0)));
        // Box um die Kamera: immer sichtbar.
        assert!(f.intersects_aabb(Vec3::ZERO, Vec3::new(0.5, 0.5, 0.5)));
        // Enthaltene Kugel ⇒ enthaltene Box (keine False Negatives):
        assert!(f.intersects_aabb(Vec3::new(0.0, 0.0, -10.0), Vec3::new(1.0, 1.0, 1.0)));
    }

    #[test]
    fn pyramiden_geometrie() {
        let p = HiZPyramid::new(8, 4);
        assert_eq!(p.level_count(), 4); // 8x4, 4x2, 2x1, 1x1
        assert_eq!(p.level_width(0), 8);
        assert_eq!(p.level_height(2), 1);
        assert_eq!(p.levels[3].len(), 1);
        // Level für Fenstergrößen.
        assert_eq!(HiZPyramid::level_for_size(1, 1), 0);
        assert_eq!(HiZPyramid::level_for_size(8, 8), 3);
        assert_eq!(HiZPyramid::level_for_size(1024, 1024), 10);
        assert_eq!(HiZPyramid::level_for_size(5, 3), 3); // ceil(log2(5))
    }

    #[test]
    fn pyramide_nicht_pot_konsistent_mit_level_geometrie() {
        // 1280×720 (Pi-Profil): die Level schrumpfen per div_ceil-Kette;
        // floor-basierte Zugriffswerte lasen über Zeilenenden hinaus.
        let mut p = HiZPyramid::new(1280, 720);
        for (l, lv) in p.levels.iter().enumerate() {
            assert_eq!(
                lv.len(),
                (p.level_width(l as u32) * p.level_height(l as u32)) as usize,
                "Level {l} Geometrie"
            );
        }
        // Einheitliche Tiefe muss bis ins oberste Level durchschlagen,
        // ohne veraltete 1.0-Ränder zu hinterlassen.
        let depth = vec![0.5; 1280 * 720];
        p.build_from_depth(&depth);
        for lv in &p.levels {
            assert!(lv.iter().all(|&d| d == 0.5), "veraltete Texel nach build");
        }
    }

    #[test]
    fn min_reduktion_findet_naechste_tiefe() {
        let mut p = HiZPyramid::new(4, 4);
        let mut depth = vec![1.0; 16];
        depth[2 * 4 + 3] = 0.25; // Texel (3,2) ist nah
        p.build_from_depth(&depth);
        assert!(approx_eq(p.levels[1][1 * 2 + 1], 0.25)); // Region (2..4, 2..4)
        assert!(approx_eq(p.min_depth_in(2, 0, 0, 1, 1), 0.25)); // ganz oben
        assert!(approx_eq(p.min_depth_in(1, 0, 0, 1, 1), 1.0)); // linke obere Ecke: fern
    }

    #[test]
    fn okklusion_wand_verdeckt_dahinter() {
        // Wand mit Tiefe 0.5 links im Bild, rechts offen (1.0 = fern).
        let mut p = HiZPyramid::new(8, 8);
        let mut depth = vec![1.0; 64];
        for y in 0..8 {
            for x in 0..4 {
                depth[y * 8 + x] = 0.5;
            }
        }
        p.build_from_depth(&depth);
        // Objekt NAH-ebene 0.6 hinter der Wand (0.5 <= 0.6) -> verdeckt.
        assert!(p.occludes(1, 0, 0, 0.6, 1e-4), "hinter Wand verdeckt");
        // Objekt mit Nah-ebene 0.4 VOR der Wand -> sichtbar.
        assert!(!p.occludes(1, 0, 0, 0.4, 1e-4), "vor Wand sichtbar");
        // Gleiches Objekt rechts neben der Wand (Level-1-Texel 2 deckt
        // die Pixel 4..6 ab — offen) -> sichtbar.
        assert!(!p.occludes(1, 2, 0, 0.6, 1e-4), "neben Wand sichtbar");
    }

    #[test]
    fn leere_pyramide_verdeckt_nichts() {
        let p = HiZPyramid::new(16, 16);
        assert!(!p.occludes(2, 1, 1, 0.9, 1e-4), "1.0 fern > 0.9");
        assert!(p.occludes(2, 1, 1, 1.0, 1e-4), "Grenzfall 1.0 <= 1.0");
    }

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }
}
