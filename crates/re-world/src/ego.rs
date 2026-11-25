//! Ego-Übergabe (M3e): Steuerungswechsel von der Strategiekamera in die
//! Ego-Perspektive — **mitten im Flug**, ohne den Szenenaufbau anzufassen.
//!
//! Das ist der Kern des Genre-Wechsels: Die Welt (Clipmap, Streamer,
//! HLOD, Instanzen) läuft unverändert weiter; nur der *Kamerapunkt* und
//! der *Eingabebesitzer* wandern. Diese Kopfstütze ist bewusst rein
//! numerisch — sie kennt keine Entities, keine Renderer, keine Eingabe-
//! geräte. Sie liefert pro Frame eine Position und sagt, wer gerade das
//! Sagen hat:
//!
//! * [`ControlMode::Strategy`] — freier Fluganker, Eingaben an die Kamera.
//! * [`ControlMode::Ego`] — Kamera hängt an einer Einheitenposition,
//!   Eingaben an die Einheit.
//! * Während der Überblendung gehört die Kamera dem System; rohe Eingaben
//!   bleiben bei der Kamera ([`InputOwner::Camera`]).
//!
//! Die Überblendung ist eine geglättete Linie (Smoothstep) zwischen
//! Ankerpunkt und Einheitenposition — damit ist der Kamerapfad *stetig*,
//! was die Streaming-Nachfrage gegen Ruckler und Total-Austausch schützt:
//! Kein Frame wirft die Welt weg, jede Zelle bleibt, solange sie gebraucht
//! wird. Determinismus wie überall in `re-world`: gleiche `dt`-Folge →
//! gleiche Kamerapfade.

use glam::Vec2;

/// Wer die Kamera gerade steuert (nach abgeschlossener Überblendung).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlMode {
    /// Strategiekamera: freier Ankerpunkt, Eingaben bewegen die Kamera.
    Strategy,
    /// Ego-Kamera: hängt an einer Einheit, Eingaben bewegen die Einheit.
    Ego,
}

/// Wer in diesem Frame die Eingaben bekommt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputOwner {
    /// Strategiekamera (frei, oder überblendungssteuernde Kamera).
    Camera,
    /// Die Ego-Einheit.
    Unit,
}

/// Ein Ereignis des Übergabeautomaten (aus [`EgoHandoff::update`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandoffEvent {
    /// Überblendung fertig: Kamera ist jetzt an der Einheit.
    Attached,
    /// Rückblendung fertig: Kamera ist wieder frei (Strategie).
    Released,
}

/// Parameter der Übergabe.
#[derive(Clone, Copy, Debug)]
pub struct EgoConfig {
    /// Dauer der Überblendung in Sekunden. 0 heißt harter Schnitt
    /// (für Tests und Teleport-Fälle).
    pub duration: f64,
}

impl Default for EgoConfig {
    fn default() -> Self {
        Self { duration: 1.2 }
    }
}

fn smoothstep(t: f64) -> f64 {
    let x = t.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Weich normalisierte Blendenkurve `0..=1` (Renderer-Fades nutzen dieselbe).
pub fn fade_curve(t: f32) -> f32 {
    smoothstep(t as f64) as f32
}

/// Der Übergabe-Automat. `attach`/`release` starten eine Blende,
/// `update(dt)` schiebt sie, `camera()` liefert den Kamerapunkt,
/// `input_owner()` sagt, wohin Eingaben gehören.
///
/// Die Einheitenposition wird von außen nachgeführt
/// ([`set_unit_position`](Self::set_unit_position)) — hier lebt nur die
/// Kamera-Logik, nicht die Spiellogik.
#[derive(Clone, Debug)]
pub struct EgoHandoff {
    cfg: EgoConfig,
    mode: ControlMode,
    /// Strategie-Anker: wohin die Kamera gehört, wenn sie frei ist.
    anchor: Vec2,
    /// Letzte bekannte Einheitenposition (Ego-Ziel).
    unit: Vec2,
    /// Startpunkt der laufenden Blende.
    from: Vec2,
    /// Fortschritt der laufenden Blende (0..=1); 1 = fertig.
    progress: f64,
    /// Läuft eine Blende *zur* Einheit (attach) oder *weg* (release)?
    to_unit: bool,
}

impl EgoHandoff {
    pub fn new(cfg: EgoConfig, start: Vec2) -> Self {
        Self {
            cfg,
            mode: ControlMode::Strategy,
            anchor: start,
            unit: start,
            from: start,
            progress: 1.0,
            to_unit: true,
        }
    }

    pub fn config(&self) -> &EgoConfig {
        &self.cfg
    }

    pub fn mode(&self) -> ControlMode {
        self.mode
    }

    /// Läuft gerade eine Überblendung?
    pub fn is_transitioning(&self) -> bool {
        self.progress < 1.0
    }

    /// Zielmodus der laufenden (oder letzten) Blende — der Renderer mixt
    /// damit Kamerahöhe und Übersicht-Deckkraft.
    pub fn heading(&self) -> ControlMode {
        if self.to_unit {
            ControlMode::Ego
        } else {
            ControlMode::Strategy
        }
    }

    /// Überblendungsanteil `0..=1` Richtung Ego (1 = Ego fest, 0 = frei).
    pub fn ego_blend(&self) -> f32 {
        match (self.mode, self.is_transitioning()) {
            (ControlMode::Ego, false) => 1.0,
            (_, true) if self.heading() == ControlMode::Ego => fade_curve(self.progress as f32),
            _ => 0.0,
        }
    }

    /// Fortschritt der laufenden Blende (0..=1); 1 wenn ruhig.
    pub fn progress(&self) -> f64 {
        self.progress
    }

    /// Kamerapunkt dieses Frames.
    pub fn camera(&self) -> Vec2 {
        if self.progress >= 1.0 {
            return match self.mode {
                ControlMode::Strategy => self.anchor,
                ControlMode::Ego => self.unit,
            };
        }
        let target = if self.to_unit { self.unit } else { self.anchor };
        self.from.lerp(target, smoothstep(self.progress) as f32)
    }

    /// Wohin gehören die Eingaben dieses Frames? Während der Blende bleibt
    /// alles bei der Kamera — die Einheit bekommt erst nach `Attached`.
    pub fn input_owner(&self) -> InputOwner {
        if !self.is_transitioning() && self.mode == ControlMode::Ego {
            InputOwner::Unit
        } else {
            InputOwner::Camera
        }
    }

    /// Strategiekamera bewegen (nur wirksam, wenn die Kamera frei ist;
    /// während einer Blende wird der Anker für danach gesetzt).
    pub fn move_anchor(&mut self, delta: Vec2) {
        self.anchor += delta;
    }

    pub fn anchor(&self) -> Vec2 {
        self.anchor
    }

    /// Ego-Einheit bewegen — aufrufen, wenn [`input_owner`](Self::input_owner)
    /// [`InputOwner::Unit`] meldet.
    pub fn move_unit(&mut self, delta: Vec2) {
        self.unit += delta;
    }

    /// Einheitenposition von außen nachführen (Physik/Spiellogik).
    /// Läuft keine Blende zur Einheit, folgt die Kamera sofort.
    pub fn set_unit_position(&mut self, pos: Vec2) {
        self.unit = pos;
    }

    pub fn unit_position(&self) -> Vec2 {
        self.unit
    }

    /// Übergabe zur Einheit: Kamera gleitet von ihrer aktuellen Position
    /// an die Einheitenposition. Bei `duration = 0` sofortiger Wechsel.
    /// `true` liefert, wenn eine Blende gestartet (oder gewechselt) wurde.
    pub fn attach(&mut self) -> bool {
        let started = self.cfg.duration > 0.0
            && (self.mode != ControlMode::Ego || self.is_transitioning());
        self.from = self.camera();
        self.progress = if self.cfg.duration <= 0.0 {
            1.0
        } else {
            0.0
        };
        self.to_unit = true;
        if self.progress >= 1.0 {
            self.mode = ControlMode::Ego;
        }
        started
    }

    /// Rückgabe an die Strategie: Anker wird die aktuelle Kameraposition
    /// (kein Sprung des Strategieblicks), Kamera gleitet hin — bei
    /// `attach` ist Ziel die Einheit, hier der Anker.
    pub fn release(&mut self) -> bool {
        let started = self.cfg.duration > 0.0
            && (self.mode != ControlMode::Strategy || self.is_transitioning());
        self.anchor = self.camera();
        self.from = self.anchor;
        self.progress = if self.cfg.duration <= 0.0 {
            1.0
        } else {
            0.0
        };
        self.to_unit = false;
        if self.progress >= 1.0 {
            self.mode = ControlMode::Strategy;
        }
        started
    }

    /// Blende weitertreiben; liefert das Ereignis beim Abschließen.
    pub fn update(&mut self, dt: f64) -> Option<HandoffEvent> {
        if self.progress >= 1.0 {
            return None;
        }
        self.progress = (self.progress + dt / self.cfg.duration.max(1e-6)).min(1.0);
        if self.progress < 1.0 {
            return None;
        }
        if self.to_unit {
            self.mode = ControlMode::Ego;
            Some(HandoffEvent::Attached)
        } else {
            self.mode = ControlMode::Strategy;
            Some(HandoffEvent::Released)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rig() -> EgoHandoff {
        EgoHandoff::new(
            EgoConfig { duration: 1.0 },
            Vec2::new(0.0, 0.0),
        )
    }

    #[test]
    fn start_ist_strategie_mit_freier_kamera() {
        let r = rig();
        assert_eq!(r.mode(), ControlMode::Strategy);
        assert_eq!(r.camera(), Vec2::new(0.0, 0.0));
        assert_eq!(r.input_owner(), InputOwner::Camera);
        assert!(!r.is_transitioning());
    }

    #[test]
    fn attach_blendet_stetig_und_endet_bei_der_einheit() {
        let mut r = rig();
        r.set_unit_position(Vec2::new(100.0, 0.0));
        assert!(r.attach());
        let mut prev = r.camera();
        let mut evs = Vec::new();
        for _ in 0..60 {
            let ev = r.update(1.0 / 60.0);
            evs.extend(ev);
            let c = r.camera();
            // Stetigkeit: kein Frame springt mehr als einen kleinen Schritt.
            assert!(
                (c - prev).length() <= 100.0 / 30.0,
                "Sprung von {prev} nach {c}"
            );
            prev = c;
        }
        assert_eq!(evs, vec![HandoffEvent::Attached], "ein Abschluss, kein früheres");
        assert_eq!(r.mode(), ControlMode::Ego);
        assert_eq!(r.camera(), Vec2::new(100.0, 0.0));
        assert_eq!(r.input_owner(), InputOwner::Unit);
    }

    #[test]
    fn waehrend_der_blende_bleibt_die_eingabe_bei_der_kamera() {
        let mut r = rig();
        r.set_unit_position(Vec2::new(100.0, 0.0));
        r.attach();
        r.update(0.5);
        assert_eq!(r.input_owner(), InputOwner::Camera, "Einheit noch gesperrt");
        assert!(r.is_transitioning());
    }

    #[test]
    fn release_gibt_frei_ohne_blicksprung() {
        let mut r = rig();
        r.set_unit_position(Vec2::new(100.0, 0.0));
        r.attach();
        for _ in 0..60 {
            r.update(1.0 / 60.0);
        }
        // Ego-Cam bei 100 → Release: Anker übernimmt die Position,
        // die Kamera steht in Frame 0 der Rückblende exakt dort.
        assert!(r.release());
        assert_eq!(r.camera(), Vec2::new(100.0, 0.0));
        for _ in 0..60 {
            r.update(1.0 / 60.0);
        }
        assert_eq!(r.mode(), ControlMode::Strategy);
        assert_eq!(r.anchor(), Vec2::new(100.0, 0.0));
        assert_eq!(r.input_owner(), InputOwner::Camera);
    }

    #[test]
    fn dauer_null_ist_harter_schnitt() {
        let mut r = EgoHandoff::new(EgoConfig { duration: 0.0 }, Vec2::ZERO);
        r.set_unit_position(Vec2::new(50.0, 50.0));
        assert!(!r.attach(), "kein Blendenstart nötig");
        assert_eq!(r.mode(), ControlMode::Ego);
        assert_eq!(r.camera(), Vec2::new(50.0, 50.0));
    }

    #[test]
    fn einheit_begleiten_ohne_blende() {
        let mut r = rig();
        r.set_unit_position(Vec2::new(10.0, 0.0));
        r.attach();
        for _ in 0..60 {
            r.update(1.0 / 60.0);
        }
        // Im Ego-Modus folgt die Kamera der Einheit auf der Stelle.
        r.move_unit(Vec2::new(0.0, 2.0));
        assert_eq!(r.camera(), Vec2::new(10.0, 2.0));
    }

    #[test]
    fn umhaengen_waehrend_der_blende_startet_neu() {
        let mut r = rig();
        r.set_unit_position(Vec2::new(100.0, 0.0));
        r.attach();
        r.update(0.5);
        r.set_unit_position(Vec2::new(100.0, 100.0));
        // Zielwechsel vor dem Neustart: die neue Blende setzt genau auf
        // dem aktuellen Kamerapunkt auf — kein Sprung.
        let mid = r.camera();
        assert!(r.attach(), "Zielwechsel ist ein neuer Blendenstart");
        assert_eq!(r.camera(), mid, "neue Blende beginnt an alter Position");
        for _ in 0..60 {
            r.update(1.0 / 60.0);
        }
        assert_eq!(r.camera(), Vec2::new(100.0, 100.0));
    }

    /// Integration M3e: Der Kamerapunkt kommt aus dem Rig, die Partition
    /// läuft unverändert weiter — die Welt wird **nicht** neu aufgebaut.
    #[test]
    fn handoff_mitten_im_flug_ohne_szenen_neuaufbau() {
        use crate::partition::{PartitionConfig, WorldPartition};
        use crate::clipmap::ClipmapConfig;
        use crate::stream::{MockSource, StreamConfig};
        use crate::tier::TierPolicy;

        let mut p = WorldPartition::new(
            PartitionConfig {
                clipmap: ClipmapConfig {
                    base_cell: 64.0,
                    radius: 2,
                    levels: 3,
                },
                stream: StreamConfig {
                    budget_bytes: 100_000,
                    max_inflight: 64,
                },
                tiers: TierPolicy::default(),
                tiers_adaptive: false,
            },
            MockSource::new(0.0, 100),
        );
        let mut r = EgoHandoff::new(EgoConfig { duration: 0.5 }, Vec2::ZERO);
        let avatar = p.spawn(Vec2::new(120.0, 0.0), Some(Vec2::ZERO));

        let mut events = Vec::new();
        let mut min_resident = u64::MAX;
        let mut frame_of_attach = 0;
        let dt = 1.0 / 60.0;
        for i in 0..300 {
            // 0..120: Strategieflug Richtung Avatar. Ab 120: Ego-Übergabe,
            // danach bewegt die Eingabe die Einheit.
            if i == 120 {
                frame_of_attach = i;
                r.attach();
            }
            if i > frame_of_attach && r.input_owner() == InputOwner::Unit {
                r.move_unit(Vec2::new(2.0, 0.0));
                // Die Avatar-Instanz folgt der Ego-Einheit.
                p.set_instance_position(avatar, r.unit_position());
            } else {
                r.move_anchor(Vec2::new(2.0, 0.0));
            }
            let cam = r.camera();
            if let Some(ev) = r.update(dt) {
                events.push(ev);
            }
            p.update(cam, dt as f32);
            // Resident-Bestand nie leer — kein Reload der ganzen Welt
            // (die ersten Frames füllen erst auf, die zählen nicht).
            if i > 5 {
                min_resident = min_resident.min(p.streamer().resident_bytes());
            }
        }
        assert_eq!(r.mode(), ControlMode::Ego);
        assert_eq!(events, vec![HandoffEvent::Attached]);
        assert!(min_resident > 0, "Zellen blieben durchgehend resident");
        // Der Avatar ist jetzt die T0-Instanz: Kamera hängt an ihm.
        assert_eq!(p.tier_of(avatar), Some(crate::tier::Tier::T0));
    }

    #[test]
    fn folge_ist_deterministisch() {
        fn run() -> Vec<(Vec2, InputOwner)> {
            let mut r = rig();
            r.set_unit_position(Vec2::new(100.0, 0.0));
            let mut log = Vec::new();
            r.attach();
            for _ in 0..120 {
                r.update(1.0 / 60.0);
                log.push((r.camera(), r.input_owner()));
            }
            r.release();
            for _ in 0..120 {
                r.update(1.0 / 60.0);
                log.push((r.camera(), r.input_owner()));
            }
            log
        }
        let a = run();
        let b = run();
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.0.x.to_bits(), y.0.x.to_bits());
            assert_eq!(x.1, y.1);
        }
    }
}
