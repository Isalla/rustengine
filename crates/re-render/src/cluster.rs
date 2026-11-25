//! Cluster-Grid für Licht-Culling und Clustered Shadows (M5, Box 2).
//!
//! „Shadow 2.0 = cascaded + clustered": CSM deckt die gerichtete Sonne ab,
//! für Punkt- und Spot-Lichter schneidet man den Sichtraum zusätzlich in
//! ein 3D-Raster — Bildschirm-Kacheln (x·y) mal logarithmische
//! Tiefenscheiben. Jede Zelle (ein *Cluster*) trägt die Lichter, die sie
//! schneiden; der Shader indiziert nur noch seinen eigenen Cluster.
//!
//! Dieser Kern liefert die Aufteilung, die Index-Mathematik und die
//! CPU-Referenz des Licht-Cullings ([`cull_lights`]) — backend-neutral
//! und headless testbar. Der GPU-Pass spiegelt [`cluster_view_aabb`] und
//! die Kugel-AABB-Prüfung Kanal-für-Kanal.

use glam::{Mat4, Vec3};

/// Logarithmische Tiefen-Grenzen von `near` bis `far` (Länge `count + 1`).
///
/// Log-Scheiben haben konstante *relative* Auflösung: nahe Cluster sind
/// dünn, ferne dick — passend zur perspektivischen Tiefenverteilung.
/// `count = 0` ergibt nur die beiden Anker.
pub fn log_z_bounds(near: f32, far: f32, count: usize) -> Vec<f32> {
    let near = near.max(1e-4);
    let far = far.max(near * 1.001);
    let n = count;
    let ratio = far / near;
    (0..=n)
        .map(|i| {
            if i == n {
                far
            } else {
                near * ratio.powf(i as f32 / n.max(1) as f32)
            }
        })
        .collect()
}

/// Bildschirm-Kachelraster × logarithmische Tiefenscheiben.
#[derive(Clone, Debug, PartialEq)]
pub struct ClusterGrid {
    /// Kacheln waagerecht.
    pub tiles_x: u32,
    /// Kacheln senkrecht.
    pub tiles_y: u32,
    /// Tiefen-Grenzen, Länge `slices + 1`, monoton `near..far`.
    z_bounds: Vec<f32>,
}

impl ClusterGrid {
    /// Raster mit `tiles_x·tiles_y` Kacheln und `slices` Scheiben über
    /// `near..far`. Alle Achsen mindestens 1; `far <= near` wird
    /// auf `near · 1.001` gehoben.
    pub fn new(tiles_x: u32, tiles_y: u32, slices: u32, near: f32, far: f32) -> Self {
        Self {
            tiles_x: tiles_x.max(1),
            tiles_y: tiles_y.max(1),
            z_bounds: log_z_bounds(near, far, slices.max(1) as usize),
        }
    }

    /// Kachelraster für eine Rendergröße: `tiles = ceil(size / tile_size)`,
    /// mindestens 1 je Achse.
    pub fn for_render_size(
        render_size: [u32; 2],
        tile_size: u32,
        slices: u32,
        near: f32,
        far: f32,
    ) -> Self {
        let ts = tile_size.max(1);
        Self::new(
            render_size[0].max(1).div_ceil(ts),
            render_size[1].max(1).div_ceil(ts),
            slices,
            near,
            far,
        )
    }

    /// Anzahl Tiefenscheiben.
    pub fn slice_count(&self) -> u32 {
        (self.z_bounds.len() - 1) as u32
    }

    /// Gesamtzahl Cluster.
    pub fn cluster_count(&self) -> u32 {
        self.tiles_x * self.tiles_y * self.slice_count()
    }

    /// Grenzen der Scheibe `slice` als `(near, far)` dieser Scheibe.
    pub fn slice_bounds(&self, slice: u32) -> Option<(f32, f32)> {
        let i = slice as usize;
        if i + 1 < self.z_bounds.len() {
            Some((self.z_bounds[i], self.z_bounds[i + 1]))
        } else {
            None
        }
    }

    /// Linearer Cluster-Index: x am schnellsten, dann y, dann Scheibe —
    /// benachbarte Kacheln liegen also beieinander (gutmütig für
    /// Speicherzugriff-Muster auf der GPU).
    pub fn index(&self, tile_x: u32, tile_y: u32, slice: u32) -> u32 {
        (slice * self.tiles_y + tile_y) * self.tiles_x + tile_x
    }

    /// Umkehrung von [`Self::index`]. `None` außerhalb des Rasters.
    pub fn coords(&self, index: u32) -> Option<(u32, u32, u32)> {
        if index >= self.cluster_count() {
            return None;
        }
        let x = index % self.tiles_x;
        let y = (index / self.tiles_x) % self.tiles_y;
        let z = index / (self.tiles_x * self.tiles_y);
        Some((x, y, z))
    }

    /// Scheibe einer Sichttiefe. `None` außerhalb `near..far`.
    /// Scheibengrenzen gehören zur *unteren* Scheibe (nur `near` selbst
    /// startet Scheibe 0) — so ist die Zuordnung eindeutig.
    pub fn slice_of(&self, view_depth: f32) -> Option<u32> {
        let last = self.z_bounds.len() - 1;
        if view_depth < self.z_bounds[0] || view_depth > self.z_bounds[last] {
            return None;
        }
        let pp = self.z_bounds.partition_point(|&b| b < view_depth);
        Some((pp.saturating_sub(1) as u32).min(self.slice_count() - 1))
    }

    /// Cluster für einen Pixel (`pixel_x`, `pixel_y`) bei gegebener
    /// Kachelgröße und Sichttiefe. Pixel außerhalb des Rasters werden auf
    /// die Randkachel geklemmt (Raster deckt die Rendergröße vollständig).
    pub fn locate(
        &self,
        pixel_x: u32,
        pixel_y: u32,
        tile_size: u32,
        view_depth: f32,
    ) -> Option<u32> {
        let ts = tile_size.max(1);
        let tx = (pixel_x / ts).min(self.tiles_x - 1);
        let ty = (pixel_y / ts).min(self.tiles_y - 1);
        self.slice_of(view_depth).map(|z| self.index(tx, ty, z))
    }
}

// ── Licht-Culling (CPU-Referenz des Compute-Passes) ───────────────────
//
// Der GPU-Compute-Pass tut genau zwei Dinge: pro Cluster sein View-Raum-
// AABB aufbauen (cluster_view_aabb) und jede Licht-Kugel dagegen testen
// (sphere_intersects_aabb). Diese Funktionen sind 1:1 die CPU-Referenz —
// der Compute-Shader muss dieselben Listen liefern (Index-für-Index).

/// Ein Licht als Culling-Kugel (Punktlicht: Radius = Reichweite; Spot:
/// Hüllkugel des Kegels).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SphereLight {
    pub position: Vec3,
    pub radius: f32,
}

impl SphereLight {
    pub fn new(position: Vec3, radius: f32) -> Self {
        Self { position, radius: radius.max(0.0) }
    }
}

/// Das View-Raum-AABB des Clusters `(tile_x, tile_y, slice)`.
///
/// Der Cluster ist ein Sichtkörper-Zellfach: die Kachel spannt ihren NDC-
/// x/y-Bereich (Kachel 0 = oben-links), die Scheibe die lineare Sichttiefe
/// `near..far` (Kamera blickt die −Z entlang). Da die x/y-Ausdehnung mit
/// der Tiefe wächst, wird das AABB über beide Scheiben-Tiefen genommen —
/// konservativ, nie ein False Negative gegen die echte Zelle.
///
/// `proj` muss eine Standard-Perspektivmatrix sein (x_axis.x = cot(fov_x/2),
/// y_axis.y = cot(fov_y/2)); entartete Werte → `None`.
pub fn cluster_view_aabb(
    grid: &ClusterGrid,
    tile_x: u32,
    tile_y: u32,
    slice: u32,
    proj: &Mat4,
) -> Option<(Vec3, Vec3)> {
    let a = proj.x_axis.x;
    let b = proj.y_axis.y;
    if a == 0.0 || b == 0.0 || !a.is_finite() || !b.is_finite() {
        return None;
    }
    if tile_x >= grid.tiles_x || tile_y >= grid.tiles_y {
        return None;
    }
    let (zn, zf) = grid.slice_bounds(slice)?;
    // NDC-Spannweite der Kachel; y von oben (Kachel 0) nach unten.
    let nx0 = 2.0 * tile_x as f32 / grid.tiles_x as f32 - 1.0;
    let nx1 = 2.0 * (tile_x + 1) as f32 / grid.tiles_x as f32 - 1.0;
    let ny1 = 1.0 - 2.0 * tile_y as f32 / grid.tiles_y as f32;
    let ny0 = 1.0 - 2.0 * (tile_y + 1) as f32 / grid.tiles_y as f32;
    // ndc_x = a·x/d ⇒ x = ndc_x·d/a (analog y); Sichttiefe d ⇒ z = −d.
    let corner = |ndc_x: f32, ndc_y: f32, d: f32| Vec3::new(ndc_x * d / a, ndc_y * d / b, -d);
    let mut mn = Vec3::splat(f32::MAX);
    let mut mx = Vec3::splat(f32::MIN);
    for (ex, ey) in [(nx0, ny0), (nx0, ny1), (nx1, ny0), (nx1, ny1)] {
        for d in [zn, zf] {
            let c = corner(ex, ey, d);
            mn = mn.min(c);
            mx = mx.max(c);
        }
    }
    Some((mn, mx))
}

/// Konservative Kugel-AABB-Prüfung (Distanz zum nächstgelegenen Punkt).
pub fn sphere_intersects_aabb(center: Vec3, radius: f32, min: Vec3, max: Vec3) -> bool {
    let closest = center.clamp(min, max);
    (closest - center).length_squared() <= radius * radius
}

/// CPU-Referenz des Licht-Culling-Passes: je Cluster die Indizes der
/// erreichenden Lichter, aufsteigend — deterministisch.
///
/// Die Licht-Kugeln werden in den View-Raum transformiert (View ist
/// starr ⇒ Radius bleibt) gegen [`cluster_view_aabb`] getestet — genau
/// die Reihenfolge des Compute-Shaders. Ergebnis-Länge:
/// `grid.cluster_count()`.
pub fn cull_lights(
    grid: &ClusterGrid,
    view: &Mat4,
    proj: &Mat4,
    lights: &[SphereLight],
) -> Vec<Vec<u32>> {
    let view_lights: Vec<(Vec3, f32)> = lights
        .iter()
        .map(|l| (view.transform_point3(l.position), l.radius))
        .collect();
    let mut out = vec![Vec::new(); grid.cluster_count() as usize];
    for z in 0..grid.slice_count() {
        for y in 0..grid.tiles_y {
            for x in 0..grid.tiles_x {
                let Some((mn, mx)) = cluster_view_aabb(grid, x, y, z, proj) else {
                    continue;
                };
                let cell = &mut out[grid.index(x, y, z) as usize];
                for (li, (c, r)) in view_lights.iter().enumerate() {
                    if sphere_intersects_aabb(*c, *r, mn, mx) {
                        cell.push(li as u32);
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn z_grenzen_verankert_monoton_und_log() {
        let b = log_z_bounds(0.5, 200.0, 4);
        assert_eq!(b.len(), 5);
        assert!(approx(b[0], 0.5, 1e-5) && approx(*b.last().unwrap(), 200.0, 1e-5));
        for w in b.windows(2) {
            assert!(w[1] > w[0], "monoton: {w:?}");
        }
        // Log: konstantes Verhältnis je Scheibe.
        let r0 = b[1] / b[0];
        let r1 = b[2] / b[1];
        let r3 = b[4] / b[3];
        assert!(approx(r0, r1, 1e-3) && approx(r1, r3, 1e-3), "log-Verhältnis {r0},{r1},{r3}");
        // Absolute Abstände wachsen nach hinten.
        assert!(b[1] - b[0] < b[4] - b[3]);
    }

    #[test]
    fn grid_baut_und_klemmt() {
        let g = ClusterGrid::new(8, 6, 4, 0.1, 100.0);
        assert_eq!(g.slice_count(), 4);
        assert_eq!(g.cluster_count(), 8 * 6 * 4);
        // Unsinnige Achsen -> mindestens 1.
        let e = ClusterGrid::new(0, 0, 0, 10.0, 5.0);
        assert_eq!((e.tiles_x, e.tiles_y, e.slice_count()), (1, 1, 1));
        // for_render_size deckt die Größe vollständig ab.
        let r = ClusterGrid::for_render_size([1920, 1080], 16, 4, 0.1, 400.0);
        assert_eq!((r.tiles_x, r.tiles_y), (120, 68)); // ceil(1920/16), ceil(1080/16)
    }

    #[test]
    fn index_und_coords_sind_invers() {
        let g = ClusterGrid::new(4, 3, 5, 0.1, 100.0);
        for z in 0..g.slice_count() {
            for y in 0..g.tiles_y {
                for x in 0..g.tiles_x {
                    let i = g.index(x, y, z);
                    assert!(i < g.cluster_count());
                    assert_eq!(g.coords(i), Some((x, y, z)), "Roundtrip {x},{y},{z}");
                }
            }
        }
        assert_eq!(g.coords(g.cluster_count()), None);
    }

    #[test]
    fn scheibenzuordnung_eindeutig() {
        let g = ClusterGrid::new(1, 1, 4, 1.0, 1000.0);
        // Anker: near -> Scheibe 0, far -> letzte Scheibe.
        assert_eq!(g.slice_of(1.0), Some(0));
        assert_eq!(g.slice_of(1000.0), Some(3));
        // Außerhalb: None.
        assert_eq!(g.slice_of(0.99), None);
        assert_eq!(g.slice_of(1000.1), None);
        // Jede Tiefe zwischen near und far trifft genau eine Scheibe,
        // und die Scheibengrenzen gehören nach unten.
        let b = log_z_bounds(1.0, 1000.0, 4);
        for k in 0..4 {
            let innen = (b[k] + b[k + 1]) * 0.5;
            assert_eq!(g.slice_of(innen), Some(k as u32), "Mitte Scheibe {k}");
            if k > 0 {
                assert_eq!(g.slice_of(b[k]), Some((k - 1) as u32), "Grenze {k} -> unten");
            }
        }
        // slice_bounds passt zu slice_of: Mitte der Scheibe liegt in ihr.
        let (n, f) = g.slice_bounds(2).unwrap();
        assert!(n < f && f <= 1000.0);
        assert_eq!(g.slice_of((n + f) * 0.5), Some(2));
        assert_eq!(g.slice_bounds(4), None);
    }

    #[test]
    fn locate_nutzt_raster_richtig() {
        let g = ClusterGrid::for_render_size([100, 50], 10, 4, 1.0, 1000.0);
        assert_eq!((g.tiles_x, g.tiles_y), (10, 5));
        let tiefe = 1.0; // Scheibe 0
        assert_eq!(g.locate(0, 0, 10, tiefe), Some(g.index(0, 0, 0)));        assert_eq!(g.locate(10, 0, 10, tiefe), Some(g.index(1, 0, 0)));
        assert_eq!(g.locate(0, 10, 10, tiefe), Some(g.index(0, 1, 0)));
        // Rand-Pixel (99,49) und darüber hinaus -> letzte Kachel.
        assert_eq!(g.locate(99, 49, 10, tiefe), Some(g.index(9, 4, 0)));
        assert_eq!(g.locate(500, 500, 10, tiefe), Some(g.index(9, 4, 0)));
        // Tiefe außerhalb -> None.
        assert_eq!(g.locate(5, 5, 10, 2000.0), None);
    }

    #[test]
    fn tiefe_und_index_haengen_konsistent_zusammen() {
        let g = ClusterGrid::new(2, 2, 4, 1.0, 1000.0);
        let b = log_z_bounds(1.0, 1000.0, 4);
        for k in 0..4 {
            let d = (b[k] + b[k + 1]) * 0.5;
            let idx = g.locate(15, 15, 10, d).unwrap();
            let (x, y, z) = g.coords(idx).unwrap();
            assert_eq!((x, y), (1, 1), "Pixel (15,15) bei tile 10");
            assert_eq!(z, k as u32, "Scheibe bei Tiefe {d}");
        }
    }

    // ── Licht-Culling ──────────────────────────────────────────────────

    /// Kamera: Identitäts-View, 90° fovy, Aspect 1, near 1, far 100.
    fn test_camera() -> (Mat4, Mat4) {
        (
            Mat4::IDENTITY,
            Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 1.0, 1.0, 100.0),
        )
    }

    #[test]
    fn cluster_aabb_waechst_mit_der_tiefe() {
        let (view, proj) = test_camera();
        let _ = view;
        let g = ClusterGrid::new(1, 1, 4, 1.0, 100.0);
        let (mn0, mx0) = cluster_view_aabb(&g, 0, 0, 0, &proj).unwrap();
        let (mn3, mx3) = cluster_view_aabb(&g, 0, 0, 3, &proj).unwrap();
        // Volle NDC-Breite: bei 90° fovy gilt x = ±d.
        assert!(approx(mx0.x, g.slice_bounds(0).unwrap().1, 1e-3));
        assert!(mn0.z < mx0.z && approx(mn0.z, -g.slice_bounds(0).unwrap().1, 1e-5));
        assert!(mn3.x < mn0.x, "ferne Zelle auch nach links breiter");
        assert!(mx3.x > mx0.x * 3.0, "ferne Zelle deutlich breiter");
        // Z-Achse = Scheiben-Tiefen (negativ, Blick −Z).
        let (zn, zf) = g.slice_bounds(2).unwrap();
        let (mn, mx) = cluster_view_aabb(&g, 0, 0, 2, &proj).unwrap();
        assert!(approx(mn.z, -zf, 1e-4) && approx(mx.z, -zn, 1e-4));
    }

    #[test]
    fn kugel_aabb_grundfaelle() {
        let (mn, mx) = (Vec3::splat(-1.0), Vec3::splat(1.0));
        assert!(sphere_intersects_aabb(Vec3::ZERO, 0.1, mn, mx), "Innen");
        assert!(sphere_intersects_aabb(Vec3::new(1.5, 0.0, 0.0), 0.5, mn, mx), "Berührt");
        assert!(!sphere_intersects_aabb(Vec3::new(1.6, 0.0, 0.0), 0.5, mn, mx), "Außerhalb");
        assert!(sphere_intersects_aabb(Vec3::new(3.0, 3.0, 0.0), 3.0, mn, mx), "Ecke aus der Ferne");
    }

    #[test]
    fn zentrales_licht_nur_in_seiner_scheibe() {
        let (view, proj) = test_camera();
        let g = ClusterGrid::new(2, 2, 4, 1.0, 100.0);
        // Tiefe 5: Scheiben [1, 3.16, 10, 31.6, 100] -> Scheibe 1.
        let lights = [SphereLight::new(Vec3::new(0.0, 0.0, -5.0), 0.1)];
        let lists = cull_lights(&g, &view, &proj, &lights);
        for z in 0..4 {
            for t in 0..4 {
                let cell = &lists[g.index(t % 2, t / 2, z) as usize];
                if z == 1 {
                    // x=0/y=0 liegt auf der Kachelgrenze -> alle 4 Kacheln.
                    assert!(cell.contains(&0), "Scheibe 1, Kachel {t} muss treffen");
                } else {
                    assert!(cell.is_empty(), "Scheibe {z}, Kachel {t} muss leer sein");
                }
            }
        }
    }

    #[test]
    fn licht_hinter_oder_neben_der_kamera_faellt_weg() {
        let (view, proj) = test_camera();
        let g = ClusterGrid::new(2, 2, 4, 1.0, 100.0);
        let lights = [
            SphereLight::new(Vec3::new(0.0, 0.0, 5.0), 0.5),  // dahinter
            SphereLight::new(Vec3::new(50.0, 0.0, -5.0), 0.5), // weit rechts vorbei
        ];
        let lists = cull_lights(&g, &view, &proj, &lights);
        assert!(lists.iter().all(|l| l.is_empty()), "beide müssen komplett weg sein");
    }

    #[test]
    fn riesiges_licht_trifft_alle_cluster() {
        let (view, proj) = test_camera();
        let g = ClusterGrid::new(3, 3, 4, 1.0, 100.0);
        let lights = [SphereLight::new(Vec3::new(0.0, 0.0, -30.0), 500.0)];
        let lists = cull_lights(&g, &view, &proj, &lights);
        assert_eq!(lists.len(), g.cluster_count() as usize);
        assert!(lists.iter().all(|l| l == &vec![0]), "riesige Kugel schneidet alles");
    }

    #[test]
    fn keine_false_negatives_konservativ_wie_die_gpu() {
        let (view, proj) = test_camera();
        let g = ClusterGrid::new(4, 4, 4, 1.0, 100.0);
        // Tiefe 6 (Scheibe 1), NDC (-0.4, 0.4) -> Kachel x=1, y=1 (y von oben).
        let d = 6.0;
        let pos = Vec3::new(-0.4 * d, 0.4 * d, -d);
        let lights = [SphereLight::new(pos, 0.05)];
        let lists = cull_lights(&g, &view, &proj, &lights);
        let hits: Vec<u32> = (0..g.cluster_count())
            .filter(|&i| !lists[i as usize].is_empty())
            .collect();
        // Muss den Ziel-Cluster enthalten (kein False Negative)…
        let target = g.index(1, 1, 1);
        assert!(hits.contains(&target), "Ziel-Cluster fehlt: {hits:?}");
        // …und darf wegen des konservativen AABBs (spannt beide Scheiben-
        // Tiefen) Nachbar-Kacheln derselben Scheibe streifen — niemals
        // aber eine andere Scheibe oder einen anderen Kachelblock.
        for h in &hits {
            let (x, y, z) = g.coords(*h).unwrap();
            assert_eq!(z, 1, "Treffer in falscher Scheibe: {h}");
            assert!((x as i32 - 1).abs() <= 1 && (y as i32 - 1).abs() <= 1, "Treffer zu weit weg: {h}");
        }
    }

    #[test]
    fn culling_ist_deterministisch_und_sortiert() {
        let (view, proj) = test_camera();
        let g = ClusterGrid::new(2, 2, 2, 1.0, 50.0);
        let lights = [
            SphereLight::new(Vec3::new(0.0, 0.0, -5.0), 2.0),
            SphereLight::new(Vec3::new(0.2, 0.0, -5.2), 1.0),
            SphereLight::new(Vec3::new(0.0, 0.0, -20.0), 3.0),
        ];
        let a = cull_lights(&g, &view, &proj, &lights);
        let b = cull_lights(&g, &view, &proj, &lights);
        assert_eq!(a, b, "zwei Läufe müssen identisch sein");
        for l in &a {
            let mut s = l.clone();
            s.sort();
            assert_eq!(l, &s, "Listen müssen aufsteigend sortiert sein");
        }
        // Licht 0 und 1 überschneiden sich -> gemeinsame Cluster mit [0,1].
        assert!(a.iter().any(|l| l == &vec![0u32, 1]), "Überschneidung muss Liste [0,1] erzeugen");
    }
}
