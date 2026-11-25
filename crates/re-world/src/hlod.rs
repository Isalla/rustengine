//! HLOD: vorgebackene Fernfeld-Zusammenfassungen am Clipmap-Rand (M3d).
//!
//! Jenseits der äußersten Clipmap-Ebene endet das Zell-Streaming — ohne
//! Gegenmittel beginnt dort die schwarze Leere. **HLOD-Cluster** füllen
//! sie: grobe Rasterzellen, die der Backer aus vielen feinen Zellen zu
//! einer billigen Zusammenfassung verschmilzt (Terrain-Hügel, Silhouetten
//! ferner Gebäude). Diese Datei führt die Buchführung, *welcher* Cluster
//! gerade sichtbar sein soll — die Geometrie selbst erzeugt der Renderer
//! aus dem Bake, niemals live aus den Feinzellen.
//!
//! Zustandsmaschine pro Cluster (an [`crate::tier`] angelehnt, gleiche
//! Hysterese-Idee, damit eine Kamera an der Randzone nicht flackert):
//!
//! ```text
//!            Distanz > take_over ─────────────► GEZEIGT
//!   VERSTECKT                                  │
//!            Distanz < take_over·(1−hyst) ◄────┘
//!   (und: über fade_out hinaus immer VERSTECKT — zu weit für alles)
//! ```
//!
//! Die Distanz ist der Chebyshev-Abstand Kamera→Clusterquadrat (nicht
//! zum Mittelpunkt) — so schaltet ein Cluster, sobald *irgendein* Teil
//! von ihm in die Übernahmezone wandert. Determinismus: Cluster werden
//! über sortierte Schlüssel besucht, die Ereignislisten sind sortiert.

use std::collections::HashMap;

use glam::Vec2;

use crate::clipmap::chebyshev_near;
use crate::grid::{cell_center, floor_div};

/// Grober Rasterschlüssel eines HLOD-Clusters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HlodCell {
    pub x: i64,
    pub y: i64,
}

/// Das HLOD-Raster und seine Übernahmezonen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HlodConfig {
    /// Kantenlänge eines Cluster-Quadrats in Weltmetern. Sinnvoll: die
    /// Zellgröße der äußersten Clipmap-Ebene (`base_cell · 2^(levels−1)`)
    /// — dann übernimmt ein Cluster exakt dort, wo das Streaming endet.
    pub cell_size: f64,
    /// Ab diesem Chebyshev-Abstand zur Kamera übernimmt der Cluster
    /// (typisch: [`crate::clipmap::ClipmapConfig::outer_radius`]).
    pub take_over: f64,
    /// Jenseits davon ist selbst die Zusammenfassung zu weit — ausblenden.
    pub fade_out: f64,
    /// Hysterese-Anteil der Übernahmezone, z. B. 0.05 = 5 % breitere
    /// Rückkehrschwelle. Muss in `0..0.5` liegen.
    pub hysteresis: f64,
}

impl HlodConfig {
    /// Weltmittelpunkt eines Clusters (seine Rasterzellen-Mitte).
    pub fn cluster_center(&self, cell: HlodCell) -> Vec2 {
        Vec2::new(
            cell_center(cell.x, self.cell_size) as f32,
            cell_center(cell.y, self.cell_size) as f32,
        )
    }

    /// Cluster-Schlüssel, der die Weltkoordinaten enthält.
    pub fn cell_at(&self, world: Vec2) -> HlodCell {
        HlodCell {
            x: floor_div(world.x as f64, self.cell_size),
            y: floor_div(world.y as f64, self.cell_size),
        }
    }

    fn checked(&self) {
        assert!(self.cell_size > 0.0, "cell_size muss > 0 sein");
        assert!(self.take_over > 0.0, "take_over muss > 0 sein");
        assert!(
            self.fade_out >= self.take_over,
            "fade_out ({}) muss >= take_over ({}) sein",
            self.fade_out,
            self.take_over
        );
        assert!(
            (0.0..0.5).contains(&self.hysteresis),
            "hysteresis muss in 0..0.5 liegen"
        );
    }
}

/// Was ein [`HlodSet::update`]-Lauf an Sichtbarkeit bewegte.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HlodDelta {
    /// Cluster, die jetzt die Ferne füllen — der Renderer blendet ihre
    /// Bake-Geometrie ein (nach `(x, y)` sortiert).
    pub shown: Vec<HlodCell>,
    /// Cluster, die wieder den echten Zellen weichen (oder zu weit sind).
    pub hidden: Vec<HlodCell>,
}

impl HlodDelta {
    pub fn is_empty(&self) -> bool {
        self.shown.is_empty() && self.hidden.is_empty()
    }
}

/// Ein registrierter Cluster mit seinem Sichtbarkeitszustand.
struct Cluster {
    /// Wie viele Fein-Zellen der Backer in diesen Cluster verschmolz
    /// (nur Diagnose — die Szene baut sich nicht daraus um).
    source_cells: u32,
    shown: bool,
}

/// Die Cluster-Menge mit Zustandsmaschine (siehe Modul-Doku).
///
/// Eine leere Menge ist der Normalzustand, solange kein Bake vorhanden
/// ist: [`update`](Self::update) liefert dann immer `None` und die
/// Partition läuft exakt wie vor M3d.
pub struct HlodSet {
    cfg: HlodConfig,
    clusters: HashMap<HlodCell, Cluster>,
}

impl HlodSet {
    pub fn new(cfg: HlodConfig) -> Self {
        cfg.checked();
        Self {
            cfg,
            clusters: HashMap::new(),
        }
    }

    pub fn config(&self) -> &HlodConfig {
        &self.cfg
    }

    pub fn len(&self) -> usize {
        self.clusters.len()
    }

    pub fn is_empty(&self) -> bool {
        self.clusters.is_empty()
    }

    pub fn contains(&self, cell: HlodCell) -> bool {
        self.clusters.contains_key(&cell)
    }

    /// Alle registrierten Cluster, deterministisch sortiert nach `(x, y)`
    /// (für Abfragen und Agent-Antworten).
    pub fn cells(&self) -> Vec<HlodCell> {
        let mut out: Vec<HlodCell> = self.clusters.keys().copied().collect();
        out.sort_by_key(|c| (c.x, c.y));
        out
    }

    /// Cluster aus dem Bake eintragen; `source_cells` = Zahl der
    /// zusammengefassten Fein-Zellen. Erneutes Registrieren überschreibt
    /// nur die Zellzahl, nie den Sichtbarkeitszustand.
    pub fn register(&mut self, cell: HlodCell, source_cells: u32) {
        self.clusters
            .entry(cell)
            .and_modify(|c| c.source_cells = source_cells)
            .or_insert(Cluster {
                source_cells,
                shown: false,
            });
    }

    /// Cluster entfernen. War er sichtbar, meldet [`update`](Self::update)
    /// kein `hidden` mehr für ihn — der Aufrufer blendet ihn selbst aus.
    pub fn unregister(&mut self, cell: HlodCell) -> Option<u32> {
        self.clusters.remove(&cell).map(|c| c.source_cells)
    }

    /// Vom Backer zusammengefasste Fein-Zellen eines Clusters.
    pub fn source_cells(&self, cell: HlodCell) -> Option<u32> {
        self.clusters.get(&cell).map(|c| c.source_cells)
    }

    pub fn is_shown(&self, cell: HlodCell) -> bool {
        self.clusters.get(&cell).is_some_and(|c| c.shown)
    }

    /// Aktuell sichtbare Cluster, deterministisch nach `(x, y)` sortiert.
    pub fn shown(&self) -> Vec<HlodCell> {
        let mut out: Vec<HlodCell> = self
            .clusters
            .iter()
            .filter(|(_, c)| c.shown)
            .map(|(k, _)| *k)
            .collect();
        out.sort_unstable();
        out
    }

    /// Ein Frame: Übernahmezonen gegen die Kamera rechnen. Liefert
    /// `None`, wenn kein Cluster seinen Zustand wechselte.
    pub fn update(&mut self, camera: Vec2) -> Option<HlodDelta> {
        if self.clusters.is_empty() {
            return None;
        }
        let mut delta = HlodDelta::default();
        // Sortierte Schlüsselreihenfolge: gleiches Kamera-Skript ⇒
        // exakt dieselbe Ereignisliste.
        let mut keys: Vec<HlodCell> = self.clusters.keys().copied().collect();
        keys.sort_unstable();
        for key in keys {
            let c = self.clusters.get_mut(&key).expect("sortierter Schlüssel");
            let d = chebyshev_near(
                camera.x as f64,
                camera.y as f64,
                key.x,
                key.y,
                self.cfg.cell_size,
            );
            let next = if d > self.cfg.fade_out {
                false // zu weit weg — auch die Zusammenfassung endet hier
            } else if c.shown {
                d >= self.cfg.take_over * (1.0 - self.cfg.hysteresis)
            } else {
                d > self.cfg.take_over
            };
            if next != c.shown {
                c.shown = next;
                if next {
                    delta.shown.push(key);
                } else {
                    delta.hidden.push(key);
                }
            }
        }
        (!delta.is_empty()).then_some(delta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(take_over: f64, hysteresis: f64) -> HlodSet {
        HlodSet::new(HlodConfig {
            cell_size: 1000.0,
            take_over,
            fade_out: 10_000.0,
            hysteresis,
        })
    }

    fn cell(x: i64, y: i64) -> HlodCell {
        HlodCell { x, y }
    }

    #[test]
    fn leere_menge_ruht() {
        let mut s = set(1000.0, 0.1);
        assert!(s.update(Vec2::ZERO).is_none());
        assert!(s.shown().is_empty());
    }

    /// Cluster, in denen die Kamera steckt oder die dicht daneben liegen,
    /// bleiben versteckt; erst jenseits von `take_over` übernimmt der
    /// Cluster — und beide Ereignislisten kommen sortiert an.
    #[test]
    fn uebernahme_nach_distanz() {
        let mut s = set(1500.0, 0.0);
        s.register(cell(0, 0), 16); // Kamera steckt drin: Distanz 0
        s.register(cell(1, 0), 16); // [1000, 2000]: Distanz 1000 → nah
        s.register(cell(2, 0), 16); // [2000, 3000]: Distanz 2000 > 1500
        s.register(cell(5, 0), 16); // [5000, 6000]: Distanz 5000 > 1500
        let d = s.update(Vec2::ZERO).expect("zwei wechseln auf gezeigt");
        assert_eq!(d.shown, vec![cell(2, 0), cell(5, 0)]);
        assert!(d.hidden.is_empty());
        assert_eq!(s.shown(), vec![cell(2, 0), cell(5, 0)]);
        assert_eq!(s.source_cells(cell(5, 0)), Some(16));
        assert!(!s.is_shown(cell(1, 0)));
    }

    /// Wackeln innerhalb der Hysteresezone darf kein einziges Mal
    /// umschalten (versteckt→gezeigt will > 2000, zurück will < 1800).
    #[test]
    fn hysterese_verhindert_flackern() {
        let mut s = set(2000.0, 0.1);
        let edge = cell(2, 0); // Quadrat [2000, 3000]
        s.register(edge, 8);
        let mut flips = 0;
        for i in 0..240 {
            // Kamera pendelt bei x = 100 ↔ 200 → Distanz zu edge ist
            // 1900 ↔ 1800: genau die Zone (1800, 2000] ohne Wechsel.
            let x = if i % 2 == 0 { 100.0 } else { 200.0 };
            if let Some(d) = s.update(Vec2::new(x, 0.0)) {
                if d.shown.contains(&edge) || d.hidden.contains(&edge) {
                    flips += 1;
                }
            }
        }
        assert_eq!(flips, 0, "keine Umschaltung in der Hysteresezone");
        assert!(!s.is_shown(edge));
    }

    #[test]
    fn klar_jenseits_uebernommen_klar_innerhalb_zurueck() {
        let mut s = set(2000.0, 0.1);
        let c = cell(3, 0); // Quadrat [3000, 4000]
        s.register(c, 8);
        // Kamera weit weg (0): Distanz 3000 > 2000 → übernommen.
        let d = s.update(Vec2::ZERO).unwrap();
        assert_eq!(d.shown, vec![c]);
        // Kamera mitten rein (3500): Distanz 0 → weit unter 1800 → zurück.
        let d = s.update(Vec2::new(3500.0, 0.0)).unwrap();
        assert_eq!(d.hidden, vec![c]);
        // Und keine weiteren Ereignisse bei gleicher Kamera.
        assert!(s.update(Vec2::new(3500.0, 0.0)).is_none());
    }

    /// Über `fade_out` hinaus wird selbst ein gezeigter Cluster ausgeblendet —
    /// und beim Wiedereintauchen in Reichweite wieder eingeblendet.
    #[test]
    fn fade_out_blendet_aus_und_wieder_ein() {
        let mut s = HlodSet::new(HlodConfig {
            cell_size: 1000.0,
            take_over: 1000.0,
            fade_out: 2500.0,
            hysteresis: 0.0,
        });
        let c = cell(3, 0); // [3000, 4000]
        s.register(c, 4);
        // Distanz 3000 > fade_out 2500 → bleibt versteckt, kein Ereignis.
        assert!(s.update(Vec2::ZERO).is_none());
        // Kamera bei 800 → Distanz 2200: zwischen take_over und fade_out.
        let d = s.update(Vec2::new(800.0, 0.0)).unwrap();
        assert_eq!(d.shown, vec![c]);
        // Zurück über fade_out → wieder ausgeblendet.
        let d = s.update(Vec2::ZERO).unwrap();
        assert_eq!(d.hidden, vec![c]);
    }

    #[test]
    fn raster_rechen_hilfen() {
        let cfg = HlodConfig {
            cell_size: 1000.0,
            take_over: 1000.0,
            fade_out: 2000.0,
            hysteresis: 0.0,
        };
        assert_eq!(cfg.cell_at(Vec2::new(3500.0, -50.0)), cell(3, -1));
        assert_eq!(cfg.cluster_center(cell(3, -1)), Vec2::new(3500.0, -500.0));
    }

    #[test]
    fn unregister_meldet_entfernung() {
        let mut s = set(1000.0, 0.0);
        let c = cell(2, 0);
        s.register(c, 6);
        assert!(s.update(Vec2::ZERO).unwrap().shown.contains(&c));
        assert_eq!(s.unregister(c), Some(6));
        assert!(!s.contains(c));
        assert!(s.shown().is_empty());
    }

    /// Gleiches Kamera-Skript, zweimal gefahren → identische Ereignisfolgen.
    #[test]
    fn ereignisfolgen_sind_deterministisch() {
        fn run() -> Vec<(Vec<HlodCell>, Vec<HlodCell>)> {
            let mut s = set(2000.0, 0.05);
            for x in -3..=3 {
                for y in -3..=3 {
                    s.register(cell(x, y), 64);
                }
            }
            let mut log = Vec::new();
            for i in 0..200 {
                let t = i as f32 * 0.05;
                let cam = Vec2::new(t * 137.0, (t * 0.7).sin() * 900.0);
                match s.update(cam) {
                    Some(d) => log.push((d.shown, d.hidden)),
                    None => log.push((Vec::new(), Vec::new())),
                }
            }
            log
        }
        let a = run();
        assert_eq!(a, run());
        let flips: usize = a.iter().map(|(s, h)| s.len() + h.len()).sum();
        assert!(flips > 0, "das Skript bewegt wirklich Cluster");
    }
}
