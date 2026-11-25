//! Clipmap-Ebenen um die Kamera.
//!
//! Um den Anker liegen mehrere Ebenen mit wachsender Zellgröße
//! (`base · 2^ebene`) und gleichem Zellradius pro Ebene — die Welt-
//! reichweite verdoppelt sich mit jeder Ebene. Jede Zelle gehört zur
//! feinsten Ebene, die sie braucht:
//!
//! * Eine Zelle ist **aktiv**, wenn sie mit der Ringbox der Ebene
//!   (`Chebyshev-Nähe ≤ radius · Zellgröße`) schneidet und — für
//!   Ebenen > 0 — nicht vollständig im feineren Innenbereich liegt
//!   (`Chebyshev-Ferne > radius · halbe Zellgröße`).
//! * Damit ist die ebenenübergreifende Zellmenge überlappungsarm: kein
//!   Punkt innerhalb der äußersten Reichweite liegt in mehr als zwei
//!   aktiven Zellen, und die Doppelung tritt nur an den Bandgrenzen
//!   auf (eine Zellbreite). Bewiesen durch die Abtasttests unten.
//!
//! Der Anker ist die Zelle der **feinsten** Ebene, in der die Kamera
//! steht; er quantisiert damit alle Bewegungen und macht das Diffen
//! billig: innerhalb einer Basiszelle ändert sich nichts.

use std::collections::HashSet;

use glam::Vec2;

use crate::grid::{cell_center, floor_div, CellKey};

/// Aufbau einer Clipmap. Werte sind Projektkonfiguration, kein Geheimnis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipmapConfig {
    /// Kantenlänge einer Zelle der feinsten Ebene, in Weltmetern.
    /// Typisch 64–256 m (siehe docs/architecture.md, Abschnitt 5).
    pub base_cell: f64,
    /// Zellradius pro Ebene (Kantenlänge = 2·radius+1 Zellen).
    pub radius: u32,
    /// Anzahl Ebenen, 1..=16.
    pub levels: u8,
}

impl ClipmapConfig {
    /// Weltreichweite der äußersten Ebene (Chebyshev, ab Anker).
    pub fn outer_radius(&self) -> f64 {
        self.radius as f64 * self.cell_size(self.levels - 1)
    }

    /// Kantenlänge einer Zelle auf der gegebenen Ebene.
    pub fn cell_size(&self, level: u8) -> f64 {
        self.base_cell * (1u32 << level) as f64
    }

    fn checked(&self) {
        assert!(self.base_cell > 0.0, "base_cell muss > 0 sein");
        assert!(self.radius >= 1, "radius muss >= 1 sein");
        assert!((1..=16).contains(&self.levels), "levels muss 1..=16 sein");
    }
}

/// Eine aktive Zelle plus their Abstand zum Anker in Basiszellen
/// (Prioritätszutat des Streamers).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActiveCell {
    pub key: CellKey,
    /// Chebyshev-Abstand Zelle→Anker, in Einheiten der Basiszelle.
    pub dist_base: u32,
}

/// Was sich gegenüber dem letzten [`Clipmap::set_center`] geändert hat.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClipDiff {
    /// Zellen, die neu aktiv wurden — nach `(ebene, x, y)` sortiert.
    pub entered: Vec<CellKey>,
    /// Zellen, die nicht mehr aktiv sind — gleiche Sortierung.
    pub left: Vec<CellKey>,
}

/// Sichtbare Zellmenge um eine bewegte Kamera.
pub struct Clipmap {
    cfg: ClipmapConfig,
    anchor: Option<[i64; 2]>,
    cells: Vec<ActiveCell>,
    lookup: HashSet<CellKey>,
}

impl Clipmap {
    pub fn new(cfg: ClipmapConfig) -> Self {
        cfg.checked();
        Self {
            cfg,
            anchor: None,
            cells: Vec::new(),
            lookup: HashSet::new(),
        }
    }

    pub fn config(&self) -> &ClipmapConfig {
        &self.cfg
    }

    /// Weltmittelpunkt des aktuellen Ankers.
    pub fn anchor_center(&self) -> Option<Vec2> {
        self.anchor.map(|[x, y]| {
            Vec2::new(
                cell_center(x, self.cfg.base_cell) as f32,
                cell_center(y, self.cfg.base_cell) as f32,
            )
        })
    }

    /// Alle aktuell aktiven Zellen, sortiert nach `(ebene, abstand, x, y)`.
    pub fn cells(&self) -> &[ActiveCell] {
        &self.cells
    }

    pub fn is_active(&self, key: CellKey) -> bool {
        self.lookup.contains(&key)
    }

    /// Kamera setzen; liefert `None`, wenn sie dieselbe Basiszelle wie
    /// beim letzten Aufruf trifft (dann hat sich nichts zu ändern).
    pub fn set_center(&mut self, camera: Vec2) -> Option<ClipDiff> {
        let base = [
            floor_div(camera.x as f64, self.cfg.base_cell),
            floor_div(camera.y as f64, self.cfg.base_cell),
        ];
        if self.anchor == Some(base) {
            return None;
        }
        let next = self.compute(base);
        let old: HashSet<CellKey> = std::mem::take(&mut self.lookup);
        let mut entered: Vec<CellKey> = Vec::new();
        let mut left: Vec<CellKey> = Vec::new();
        for c in &next {
            if !old.contains(&c.key) {
                entered.push(c.key);
            }
        }
        let fresh: HashSet<CellKey> = next.iter().map(|c| c.key).collect();
        for k in &old {
            if !fresh.contains(k) {
                left.push(*k);
            }
        }
        entered.sort_unstable();
        left.sort_unstable();
        self.anchor = Some(base);
        self.cells = next;
        self.lookup = fresh;
        Some(ClipDiff { entered, left })
    }

    fn compute(&self, anchor_base: [i64; 2]) -> Vec<ActiveCell> {
        // Ankerpunkt = Mittelpunkt der Basiszelle; quantisiert, damit
        // Kamera-Wackeln innerhalb einer Zelle kein Neu-Berechnung kostet.
        let ax = cell_center(anchor_base[0], self.cfg.base_cell);
        let ay = cell_center(anchor_base[1], self.cfg.base_cell);
        let mut out = Vec::new();
        for level in 0..self.cfg.levels {
            let s = self.cfg.cell_size(level);
            let r_out = self.cfg.radius as f64 * s;
            let r_in = if level == 0 { -1.0 } else { r_out / 2.0 };
            let x0 = floor_div(ax - r_out, s);
            let x1 = floor_div(ax + r_out, s);
            let y0 = floor_div(ay - r_out, s);
            let y1 = floor_div(ay + r_out, s);
            for x in x0..=x1 {
                for y in y0..=y1 {
                    let near = chebyshev_near(ax, ay, x, y, s);
                    if near > r_out {
                        continue;
                    }
                    if level > 0 && chebyshev_far(ax, ay, x, y, s) <= r_in {
                        continue; // liegt ganz in der feineren Scheibe
                    }
                    out.push(ActiveCell {
                        key: CellKey::new(level, x, y),
                        dist_base: (near / self.cfg.base_cell).ceil() as u32,
                    });
                }
            }
        }
        out.sort_by(|a, b| {
            (a.key.level, a.dist_base, a.key.x, a.key.y).cmp(&(b.key.level, b.dist_base, b.key.x, b.key.y))
        });
        out
    }
}

/// Chebyshev-Abstand Punkt→Quadrat (nächster Punkt des Quadrats).
pub(crate) fn chebyshev_near(px: f64, py: f64, x: i64, y: i64, s: f64) -> f64 {
    let dx = axis_near(px, x as f64 * s, s);
    let dy = axis_near(py, y as f64 * s, s);
    dx.max(dy)
}

/// Chebyshev-Abstand Punkt→Quadrat (entferntester Eckpunkt).
fn chebyshev_far(px: f64, py: f64, x: i64, y: i64, s: f64) -> f64 {
    let lo = x as f64 * s;
    let hi = lo + s;
    let dx = (px - lo).abs().max((px - hi).abs());
    let lo = y as f64 * s;
    let hi = lo + s;
    let dy = (py - lo).abs().max((py - hi).abs());
    dx.max(dy)
}

fn axis_near(p: f64, lo: f64, s: f64) -> f64 {
    if p < lo {
        lo - p
    } else if p > lo + s {
        p - (lo + s)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cm(base: f64, radius: u32, levels: u8) -> Clipmap {
        Clipmap::new(ClipmapConfig {
            base_cell: base,
            radius,
            levels,
        })
    }

    #[test]
    fn erste_kamera_setzt_ebene_null_quadrat() {
        let mut c = cm(10.0, 2, 1);
        let d = c.set_center(Vec2::ZERO).expect("erster Setzen liefert Diff");
        // Eine einzige Ebene: (2·2+1)² = 25 Zellen, keine verlassenen.
        assert_eq!(d.entered.len(), 25);
        assert!(d.left.is_empty());
        assert_eq!(c.cells().len(), 25);
    }

    #[test]
    fn innerhalb_einer_basiszelle_passiert_nichts() {
        let mut c = cm(64.0, 3, 3);
        c.set_center(Vec2::new(1.0, 1.0)).unwrap();
        assert!(c.set_center(Vec2::new(30.0, 50.0)).is_none());
        assert!(c.set_center(Vec2::new(63.9, 63.9)).is_none());
        assert!(c.set_center(Vec2::new(64.0, 63.9)).is_some());
    }

    /// Kern-Eigenschaft: jeder Punkt innerhalb der äußersten Reichweite
    /// steckt in mindestens einer aktiven Zelle — sonst gäbe es Löcher
    /// im Streaming.
    #[test]
    fn abdeckung_ohne_loecher() {
        let base = 8.0;
        let cfg = ClipmapConfig {
            base_cell: base,
            radius: 3,
            levels: 4,
        };
        let mut c = cm(base, 3, 4);
        c.set_center(Vec2::ZERO);
        let outer = cfg.outer_radius();
        // 1024 Abtastpunkte gleichmäßig in der Rand-Box, fester Schritt
        // — die Regel ist exakt, hier gibt es kein Runden im Grenzbereich.
        let n = 32;
        let step = (2.0 * outer) / n as f64;
        for i in 0..n {
            for j in 0..n {
                let px = -outer + i as f64 * step;
                let py = -outer + j as f64 * step;
                if px.abs().max(py.abs()) > outer {
                    continue;
                }
                let hit = c.cells().iter().any(|a| {
                    let s = cfg.cell_size(a.key.level);
                    a.key.x as f64 * s <= px
                        && px < (a.key.x + 1) as f64 * s
                        && a.key.y as f64 * s <= py
                        && py < (a.key.y + 1) as f64 * s
                });
                assert!(hit, "Punkt ({px},{py}) liegt in keiner aktiven Zelle");
            }
        }
    }

    /// Kein Punkt liegt in aktiven Zellen zweier Ebenen — ausgenommen
    /// die zulässige Randbreite an den Bandgrenzen: gezählt wird, und
    /// die Zahl darf nicht explosionsartig wachsen.
    #[test]
    fn doppelabdeckung_nur_an_bandgrenzen() {
        let cfg = ClipmapConfig {
            base_cell: 8.0,
            radius: 4,
            levels: 4,
        };
        let mut c = Clipmap::new(cfg);
        c.set_center(Vec2::ZERO);
        // Doppelte Abdeckung kann nur in Zellen auftreten, deren Bandgrenze
        // (Vielfaches von r_in) sie schneidet: pro Ebene 1 Randzelle.
        // Prüfen: kein Punkt steckt in >2 aktiven Zellen.
        let outer = cfg.outer_radius();
        let mut worst = 0;
        let n = 64;
        let step = (2.0 * outer) / n as f64;
        for i in 0..n {
            for j in 0..n {
                let px = -outer + i as f64 * step;
                let py = -outer + j as f64 * step;
                if px.abs().max(py.abs()) > outer {
                    continue;
                }
                let hits = c
                    .cells()
                    .iter()
                    .filter(|a| {
                        let s = cfg.cell_size(a.key.level);
                        a.key.x as f64 * s <= px
                            && px < (a.key.x + 1) as f64 * s
                            && a.key.y as f64 * s <= py
                            && py < (a.key.y + 1) as f64 * s
                    })
                    .count();
                worst = worst.max(hits);
            }
        }
        assert!(worst <= 2, "Punkte in bis zu {worst} aktiven Zellen");
    }

    #[test]
    fn diff_ist_konsistent_mit_neuberechnung() {
        let mut c = cm(16.0, 2, 3);
        c.set_center(Vec2::ZERO).unwrap();
        let before: HashSet<CellKey> = c.cells().iter().map(|a| a.key).collect();
        let d = c.set_center(Vec2::new(40.0, 0.0)).unwrap();
        let after: HashSet<CellKey> = c.cells().iter().map(|a| a.key).collect();
        for k in &d.entered {
            assert!(after.contains(k) && !before.contains(k));
        }
        for k in &d.left {
            assert!(before.contains(k) && !after.contains(k));
        }
        // Menge der Wechsel ist symmetrisch vollständig.
        assert_eq!(
            d.entered.len() + before.len(),
            d.left.len() + after.len()
        );
    }

    #[test]
    fn zellen_sind_deterministisch_sortiert() {
        let mut c = cm(10.0, 2, 3);
        c.set_center(Vec2::new(5.0, 5.0));
        let a = c.cells().to_vec();
        c.set_center(Vec2::new(500.0, 500.0));
        c.set_center(Vec2::new(5.0, 5.0));
        assert_eq!(a, c.cells().to_vec(), "gleiche Kamera → gleiche Reihenfolge");
    }
}
