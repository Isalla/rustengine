//! World Partition: die Fassade, die Clipmap, Streamer und Stufen hält.
//!
//! [`WorldPartition`] ist das, was Spiellogik und Renderer sehen:
//!
//! ```text
//! update(kamera, dt)
//!   ├─ Clipmap::set_center      → aktive Zellmenge nachziehen
//!   ├─ HLOD-Cluster             → Fernfeld-Übernahme jenseits der Reichweite
//!   ├─ Streamer::sync_demand    → Zellen mit Priorität anfordern
//!   ├─ Streamer::drive          → Loaded/Evicted/Failed-Ereignisse
//!   └─ Instanzen-Stufen         → TierChanged-Ereignisse (T0..T4)
//! ```
//!
//! [`PartitionDelta`] bündelt genau diese Ereignislisten — der Renderer
//! baut daraus seine Szenen-Anpassungen, ohne selbst zu entscheiden.
//! [`Instance`] ist ein Ding in der Welt (Einheit, Gebäude, Deko);
//! seine Stufe wird pro Frame gegen die Kameradistanz gerechnet.
//!
//! Die Fassade ist bewusst frei von `re-ecs`-Typen: wer die Integration
//! will, spiegelt [`WorldPartition::instances`] per System in eine
//! Stufen-Komponente — so bleibt das Crate kopflos testbar.

use glam::Vec2;

use crate::clipmap::{Clipmap, ClipmapConfig};
use crate::grid::CellKey;
use crate::hlod::{HlodConfig, HlodDelta, HlodSet};
use crate::stream::{CellSource, Priority, StreamConfig, Streamer};
use crate::tier::{Tier, TierPolicy, TierState};

/// Konfiguration der Partition — alles Werte aus docs/architecture.md.
#[derive(Clone, Debug)]
pub struct PartitionConfig {
    pub clipmap: ClipmapConfig,
    pub stream: StreamConfig,
    pub tiers: TierPolicy,
    /// Stufenregel an die gemessene IO-Lage anpassen (siehe
    /// [`crate::telemetry`]): langsame Platte → frühere Abwertung, mehr
    /// Hysterese, längere Verweilzeit. Aus = [`Self::tiers`] unverändert.
    pub tiers_adaptive: bool,
}

/// Eine Instanz in der Welt: Position + ihr Stufen-Zustand.
#[derive(Clone, Copy, Debug)]
pub struct Instance {
    pub position: Vec2,
    pub tier: TierState,
}

/// Ein Stufenwechsel einer Instanz (Index in [`WorldPartition::instances`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TierChange {
    pub instance: usize,
    pub from: Tier,
    pub to: Tier,
}

/// Was ein [`WorldPartition::update`]-Frame bewegt hat.
#[derive(Debug)]
pub struct PartitionDelta<P> {
    /// Zellen, die neu geladen wurden (Renderer: Ressourcen übernehmen).
    pub loaded: Vec<(CellKey, P)>,
    /// Zellen, die das Budget räumen musste (Renderer: Ressource
    /// freigeben — sie wurde zuvor über `loaded` übergeben).
    pub evicted: Vec<CellKey>,
    /// Ladfehler (Quelle kennt die Zelle nicht).
    pub failed: Vec<CellKey>,
    /// HLOD-Cluster, die neu die Ferne füllen oder ihr weichen — leer,
    /// solang kein [`WorldPartition::enable_hlod`] registriert wurde.
    pub hlod: HlodDelta,
    /// Stufenwechsel der Instanzen, sortiert nach Instanz-Index.
    pub tier_changed: Vec<TierChange>,
}

impl<P> Default for PartitionDelta<P> {
    fn default() -> Self {
        Self {
            loaded: Vec::new(),
            evicted: Vec::new(),
            failed: Vec::new(),
            hlod: HlodDelta::default(),
            tier_changed: Vec::new(),
        }
    }
}

/// Kopflose, deterministische Welt-Partition (siehe Modul-Doku).
pub struct WorldPartition<S: CellSource> {
    cfg: PartitionConfig,
    clipmap: Clipmap,
    streamer: Streamer<S>,
    hlod: HlodSet,
    instances: Vec<Instance>,
    time: f64,
}

impl<S: CellSource> WorldPartition<S> {
    pub fn new(cfg: PartitionConfig, source: S) -> Self {
        let clipmap = Clipmap::new(cfg.clipmap);
        let streamer = Streamer::new(source, cfg.stream);
        // Ohne Bake ist HLOD eine leere, unbeobachtete Nebenstrecke.
        // Default: Clusterkante = Zellgröße der äußersten Ebene, Übernahme
        // genau dort, wo die Clipmap-Reichweite endet.
        let outer = cfg.clipmap.cell_size(cfg.clipmap.levels - 1);
        let reach = cfg.clipmap.outer_radius();
        let hlod = HlodSet::new(HlodConfig {
            cell_size: outer,
            take_over: reach,
            fade_out: reach * 8.0,
            hysteresis: 0.05,
        });
        Self {
            cfg,
            clipmap,
            streamer,
            hlod,
            instances: Vec::new(),
            time: 0.0,
        }
    }

    /// Weltzeit in Sekunden (Start bei 0, wächst um jedes `dt`).
    pub fn time(&self) -> f64 {
        self.time
    }

    pub fn config(&self) -> &PartitionConfig {
        &self.cfg
    }

    pub fn clipmap(&self) -> &Clipmap {
        &self.clipmap
    }

    pub fn streamer(&self) -> &Streamer<S> {
        &self.streamer
    }

    /// Die HLOD-Cluster-Menge (Fernfeld am Clipmap-Rand, siehe [`hlod`](crate::hlod)).
    pub fn hlod(&self) -> &HlodSet {
        &self.hlod
    }

    /// HLOD mit anderer Übernahmezone/anderem Raster neu aufsetzen —
    /// der Bestand startet versteckt (der Bake wird danach eingetragen).
    pub fn enable_hlod(&mut self, cfg: HlodConfig) {
        self.hlod = HlodSet::new(cfg);
    }

    /// Bake-Nachschub: Cluster registrieren/entfernen, ohne die Zone zu ändern.
    pub fn hlod_mut(&mut self) -> &mut HlodSet {
        &mut self.hlod
    }

    pub fn instances(&self) -> &[Instance] {
        &self.instances
    }

    /// Instanz hinzufügen; die Startstufe kommt sofort aus der
    /// Distanz zu `camera` (ohne Kamera-Angabe startet sie bei `T4`).
    pub fn spawn(&mut self, position: Vec2, camera: Option<Vec2>) -> usize {
        let dist = camera.map(|c| (position - c).length()).unwrap_or(f32::MAX);
        let tier = TierState::new(&self.cfg.tiers, dist, self.time as f32);
        let id = self.instances.len();
        self.instances.push(Instance { position, tier });
        id
    }

    /// Instanz entfernen. Achtung: `remove` verschiebt den letzten
    /// Eintrag nach `id` — alle Indizes > `id` wandern um eins zurück.
    pub fn despawn(&mut self, id: usize) -> Option<Instance> {
        if id >= self.instances.len() {
            return None;
        }
        Some(self.instances.remove(id))
    }

    /// Stufe einer Instanz abfragen.
    pub fn tier_of(&self, id: usize) -> Option<Tier> {
        self.instances.get(id).map(|i| i.tier.tier)
    }

    /// Ein Frame: Clipmap nachziehen, Streaming treiben, Stufen rechnen.
    ///
    /// Die Reihenfolge ist Teil der Determinismus-Zusage: erst Zellen,
    /// dann Instanzen, und beide Ereignislisten in fester Sortierung.
    pub fn update(&mut self, camera: Vec2, dt: f32) -> PartitionDelta<S::Payload> {
        self.time += dt as f64;
        self.streamer.set_now(self.time);
        let mut delta = PartitionDelta::default();

        // 1) Clipmap — die aktive Zellmenge ist eine reine Funktion der
        //    Kamera-Basiszelle, also ohne eigenen Zustand zu diffen.
        self.clipmap.set_center(camera);

        // 1b) HLOD — die Ferne hinter dem Clipmap-Rand. Läuft vor der
        //    Zell-Nachfrage, damit der Renderer im selben Frame beides
        //    (Zellen und Cluster) in fester Reihenfolge bekommt.
        if let Some(hd) = self.hlod.update(camera) {
            delta.hlod = hd;
        }

        // 2) Nachfrage + Streaming.
        let cells: Vec<(CellKey, Priority)> = self
            .clipmap
            .cells()
            .iter()
            .map(|a| (a.key, Priority(a.key.level, a.dist_base)))
            .collect();
        self.streamer.sync_demand(cells);
        let sd = self.streamer.drive();
        delta.loaded = sd.loaded;
        delta.evicted = sd.evicted;
        delta.failed = sd.failed;

        // 3) Instanzen-Stufen — über den Instanz-Index aufsteigend,
        //    damit die Ereignisliste bei gleichem Input identisch ist.
        //    Bei tiers_adaptive ist die Regel IO-adaptiert: misst der
        //    Streamer eine langsame Platte, wird früher abgewertet.
        let tiers = if self.cfg.tiers_adaptive {
            self.streamer.telemetry().adapt(&self.cfg.tiers)
        } else {
            self.cfg.tiers.clone()
        };
        for (id, inst) in self.instances.iter_mut().enumerate() {
            let dist = (inst.position - camera).length();
            let from = inst.tier.tier;
            if let Some(to) = tiers.update(&mut inst.tier, dist, self.time as f32) {
                delta.tier_changed.push(TierChange { instance: id, from, to });
            }
        }
        delta
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::MockSource;

    fn partition(budget: u64) -> WorldPartition<MockSource> {
        partition_io(budget, 0.0, 100)
    }

    /// Partition mit wählbarer Quelle — `latency`/`bytes` bestimmen, was
    /// die Telemetrie als IO-Geschwindigkeit misst.
    fn partition_io(budget: u64, latency: f64, bytes: u64) -> WorldPartition<MockSource> {
        WorldPartition::new(
            PartitionConfig {
                clipmap: ClipmapConfig {
                    base_cell: 64.0,
                    radius: 2,
                    levels: 3,
                },
                stream: StreamConfig {
                    budget_bytes: budget,
                    max_inflight: 64,
                },
                tiers: TierPolicy::default(),
                tiers_adaptive: false,
            },
            MockSource::new(latency, bytes),
        )
    }

    #[test]
    fn frame_laedt_clipmap_zellen() {
        let mut p = partition(100_000);
        // Frame 1 startet die Loads (Latency 0), Frame 2 pollen fertig.
        let d1 = p.update(Vec2::ZERO, 1.0 / 60.0);
        assert!(d1.loaded.is_empty(), "Latency 0 heißt nicht sofort fertig");
        let d2 = p.update(Vec2::ZERO, 1.0 / 60.0);
        assert!(!d2.loaded.is_empty(), "zweiter Frame macht resident");
        assert!(p.streamer().resident_bytes() > 0);
    }

    /// Kernfall aus docs/architecture.md: der Kameraflug ist reine
    /// Stufenfolge. Weit weg gestartet muss eine Instanz auf dem Flug
    /// T4 → T3 → T2 → T1 → T0 durchlaufen, ohne Auslassung.
    #[test]
    fn kameraflug_hinein_wechselt_stufen_monoton() {
        let mut p = partition(100_000);
        // Verweilzeit aus, damit die Wechsel an den Hysterese-Grenzen
        // nicht zusätzlich zeitlich gebremst werden (dwell selbst ist
        // in tier.rs getestet).
        p.cfg.tiers = TierPolicy {
            min_dwell: 0.0,
            ..Default::default()
        };
        p.update(Vec2::ZERO, 0.1);
        let far = p.spawn(Vec2::new(5_000.0, 0.0), Some(Vec2::ZERO));
        assert_eq!(p.tier_of(far), Some(Tier::T4));

        let mut seq = Vec::new();
        // Kamera fliegt 25 s gerade auf die Instanz zu und bleibt dann stehen.
        for i in 0..1800 {
            let t = i as f32 / 60.0;
            let cam = Vec2::new((t * 200.0).min(5_000.0), 0.0);
            let d = p.update(cam, 1.0 / 60.0);
            for c in d.tier_changed {
                assert_eq!(c.instance, far);
                seq.push((c.from, c.to));
            }
        }
        assert_eq!(p.tier_of(far), Some(Tier::T0));
        assert_eq!(
            seq,
            vec![
                (Tier::T4, Tier::T3),
                (Tier::T3, Tier::T2),
                (Tier::T2, Tier::T1),
                (Tier::T1, Tier::T0),
            ]
        );
    }

    /// Der End-zu-End-Pfad der IO-Telemetrie: eine langsame Quelle drückt
    /// die gemessene Gesundheit, und die adaptive Regel wirft die Instanz
    /// **später** in die feine Stufe zurück als die feste Basis-Regel.
    #[test]
    fn io_adaptive_stufen_reagieren_auf_langsame_platte() {
        fn fliege(adaptive: bool) -> Tier {
            // Langsam: 0,6 s für 2 KiB → ~3,4 KiB/s.
            let mut p = partition_io(100_000, 0.6, 2 * 1024);
            p.cfg.tiers_adaptive = adaptive;
            let inst = p.spawn(Vec2::new(150.0, 0.0), Some(Vec2::ZERO));
            assert_eq!(p.tier_of(inst), Some(Tier::T2));
            // Kamera fährt 15 s mit 5 Einheiten/s auf die Instanz zu.
            for i in 0..900 {
                let cam = Vec2::new(i as f32 / 60.0 * 5.0, 0.0);
                p.update(cam, 1.0 / 60.0);
            }
            let health = p.streamer().telemetry().health();
            assert!(
                health < 0.4,
                "die langsame Quelle muss messbar sein: {health}"
            );
            p.tier_of(inst).unwrap()
        }
        assert_eq!(fliege(false), Tier::T1, "Basis-Regel: bei 75 m ist T1 frei");
        assert_eq!(fliege(true), Tier::T2, "langsame Platte: T1 bleibt gesperrt");
    }

    #[test]
    fn despawn_entfernt_und_schiebt_nach() {
        let mut p = partition(100_000);
        p.update(Vec2::ZERO, 0.1);
        let a = p.spawn(Vec2::new(1.0, 1.0), Some(Vec2::ZERO));
        let b = p.spawn(Vec2::new(2.0, 2.0), Some(Vec2::ZERO));
        assert_eq!((a, b), (0, 1));
        let gone = p.despawn(a).expect("Index 0 existiert");
        assert_eq!(gone.position, Vec2::new(1.0, 1.0));
        assert_eq!(p.instances().len(), 1);
        // Der Nachrücker liegt jetzt auf Index 0.
        assert_eq!(p.instances()[0].position, Vec2::new(2.0, 2.0));
        assert!(p.despawn(5).is_none(), "außerhalb ist None");
    }

    /// Gleiches Kamera-Skript auf zwei frischen Partitionen → identische
    /// Ereignisfolgen inkl. Stufenwechsel.
    #[test]
    fn fassade_ist_deterministisch() {
        fn run() -> Vec<(Vec<String>, Vec<TierChange>)> {
            let mut p = partition(4_000);
            let mut log = Vec::new();
            p.update(Vec2::ZERO, 0.5);
            p.spawn(Vec2::new(60.0, 0.0), Some(Vec2::ZERO));
            p.spawn(Vec2::new(0.0, 900.0), Some(Vec2::ZERO));
            for i in 0..240 {
                let t = i as f32 / 60.0;
                let cam = Vec2::new(t * 400.0, t * 50.0);
                let d = p.update(cam, 1.0 / 60.0);
                let mut ev: Vec<String> = Vec::new();
                for (k, _) in &d.loaded {
                    ev.push(format!("L {k:?}"));
                }
                for k in &d.evicted {
                    ev.push(format!("E {k:?}"));
                }
                for k in &d.failed {
                    ev.push(format!("F {k:?}"));
                }
                log.push((ev, d.tier_changed));
            }
            log
        }
        let a = run();
        let b = run();
        assert_eq!(a.len(), b.len());
        // Nicht nur Leere vergleichen: es müssen wirklich Ereignisse da sein.
        let events: usize = a.iter().map(|(e, t)| e.len() + t.len()).sum();
        assert!(events > 20, "Skript erzeugt messbar viel Betrieb ({events})");
        for (fa, fb) in a.iter().zip(&b) {
            assert_eq!(fa.0, fb.0, "Zell-Ereignisse pro Frame identisch");
            assert_eq!(fa.1, fb.1, "Stufenwechsel pro Frame identisch");
        }
    }

    /// HLOD hängt als Nebenstrecke an der Partition: Cluster fern der
    /// Kamera übernehmen, beim Hineinfliegen weichen sie wieder — und
    /// ohne registrierte Cluster bleibt das Delta leer (alter Pfad).
    #[test]
    fn partition_treibt_hlod_cluster() {
        let mut p = partition(1 << 30); // Reichweite 512, Clusterkante 256
        assert!(
            p.update(Vec2::ZERO, 0.1).hlod.is_empty(),
            "ohne Bake bleibt es leer"
        );

        let fern = crate::hlod::HlodCell { x: 4, y: 0 }; // [1024, 1280] → d = 1024
        p.hlod_mut().register(fern, 64);
        let d = p.update(Vec2::ZERO, 0.1);
        assert_eq!(d.hlod.shown, vec![fern]);

        // Hineinfliegen: der Cluster umgibt die Kamera → Distanz 0 → zurück.
        let d = p.update(Vec2::new(1_100.0, 0.0), 0.1);
        assert_eq!(d.hlod.hidden, vec![fern]);
        assert!(p.update(Vec2::new(1_100.0, 0.0), 0.1).hlod.is_empty());
    }
}
