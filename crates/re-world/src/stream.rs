//! Streaming-Tabelle: Zellen anfordern, laden, im Budget halten.
//!
//! Der [`Streamer`] entscheidet über **Reihenfolge und Budget**, besitzt
//! aber selbst keine Payloads. Die eigentliche Beschaffung liegt hinter
//! dem [`CellSource`]-Trait (M3b: [`MockSource`] für Tests und Demos;
//! M3c: IO-Thread-Pool). Ein fertiger Payload wandert über
//! [`StreamDelta::loaded`] an den Aufrufer — der Renderer ist damit
//! Eigentümer seiner Ressourcen, der Streamer führt nur die Buchführung
//! (Status, Bytes, LRU) und meldet über [`StreamDelta::evicted`], welche
//! Zelle er nicht mehr tragen kann.
//!
//! * `Queued → Loading → Ready`: jede Zelle genau einmal pro
//!   Lebenszyklus, auch wenn die Nachfrage mehrfach anklopft.
//! * **Priorität** = (Ebene, Abstand in Basiszellen) — nahe, feine
//!   Zellen zuerst; total sortiert, also deterministisch.
//! * **Pooling statt Entladen**: verlassene Zellen bleiben als Resident
//!   gebucht und räumen erst, wenn das Budget es verlangt (zuerst
//!   ungeliebte, dann grobe Ebenen, dann LRU). Ein Wackler an der
//!   Clipmap-Kante kostet deshalb keinen Reload.

use std::collections::HashMap;

use crate::grid::CellKey;

/// Lade-Priorität: kleinere Werte zuerst — (Ebene, Abstand in
/// Basiszellen). Total sortierbar; bei Gleichstand treten immer die
/// Zellkoordinaten als Stabilisator ein.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Priority(pub u8, pub u32);

/// Eine angeforderte Zelle wurde von der Quelle abgelehnt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// Die Quelle hat für diese Zelle nichts — sie wird als Fehler
    /// gemeldet und erst bei nächster Nachfrage erneut versucht.
    Unsupported,
}

/// Rückmeldung einer laufenden Ladung bei [`CellSource::poll`].
pub enum PollResult<P> {
    /// Noch nicht fertig — beim nächsten Lauf wieder anfragen.
    Pending,
    /// Fertig; `bytes` zählt gegen das Resident-Budget des Streamers.
    Ready { payload: P, bytes: u64 },
    /// Endgültig fehlgeschlagen (Zelle entfernt, Fehler gemeldet).
    Failed,
}

/// Die Beschaffungsseite: Datei-IO, HTTP, GPU-Upload — was auch immer
/// eine Zell-Payload erzeugt. Reihenfolge: erst [`start`](Self::start),
/// dann [`poll`](Self::poll) bis `Ready`/`Failed`; will der Streamer die
/// Zelle nicht mehr, ruft er [`cancel`](Self::cancel) auf.
///
/// Beide Zeit-Parameter sind die **Weltzeit des Streamers** — die Quelle
/// führt keine eigene Uhr, damit ein Mock ohne Threads deterministisch
/// bleibt und der echte IO-Pool seine Zeitgeber ebenfalls daran koppelt.
pub trait CellSource {
    /// Payload-Typ einer Zelle (z. B. Handles auf Mesh-/Batch-Bündel).
    type Payload;

    /// Ladung aufnehmen. `Err` heißt: sofortiger Fehlschlag, kein `poll`.
    fn start(&mut self, cell: CellKey, now: f64) -> Result<(), LoadError>;

    /// Fortschritt abfragen.
    fn poll(&mut self, cell: CellKey, now: f64) -> PollResult<Self::Payload>;

    /// Der Streamer braucht die Zelle nicht mehr (nur Höflichkeit — eine
    /// bereits fertige Payload darf trotzdem geliefert werden).
    fn cancel(&mut self, _cell: CellKey) {}
}

/// Grenzen des Streamers.
#[derive(Clone, Copy, Debug)]
pub struct StreamConfig {
    /// Obergrenze für die Summe aller Resident-Bytes — der Teil des
    /// Speicher-Budgets, der fürs Welt-Streaming reserviert ist.
    pub budget_bytes: u64,
    /// Wie viele Zellen gleichzeitig laden dürfen.
    pub max_inflight: usize,
}

/// Was ein [`Streamer::drive`]-Lauf bewegt hat.
#[derive(Debug)]
pub struct StreamDelta<P> {
    /// Frisch geladene Payloads — ab jetzt Eigentum des Aufrufers.
    /// Der Streamer hat sie als Resident gebucht (`bytes` siehe
    /// [`Streamer::resident_bytes`]).
    pub loaded: Vec<(CellKey, P)>,
    /// Zellen, die das Budget räumen musste — der Aufrufer gibt die
    /// zugehörige Ressource frei (sie wurde zuvor aus `loaded` geliefert).
    pub evicted: Vec<CellKey>,
    /// Abgelehnte, fehlgeschlagene oder zu große Ladversuche.
    pub failed: Vec<CellKey>,
}

impl<P> Default for StreamDelta<P> {
    fn default() -> Self {
        Self {
            loaded: Vec::new(),
            evicted: Vec::new(),
            failed: Vec::new(),
        }
    }
}

/// Zustand einer Zelle in der Tabelle (für Assertions und Diagnose).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotState {
    Queued,
    Loading,
    Ready,
}

/// Reine Buchführung — kein Payload, nur Status und Größe.
#[derive(Clone, Copy, Debug)]
struct Slot {
    state: SlotState,
    /// Resident-Größe (nur für `Ready` gesetzt).
    bytes: u64,
    /// Weltzeit der letzten Nutzung als nachgefragte Zelle (LRU).
    last_use: f64,
}

/// Verwaltung aller Zell-Zustände (siehe Modul-Doku).
pub struct Streamer<S: CellSource> {
    source: S,
    cfg: StreamConfig,
    slots: HashMap<CellKey, Slot>,
    demand: HashMap<CellKey, Priority>,
    resident_bytes: u64,
    now: f64,
}

impl<S: CellSource> Streamer<S> {
    pub fn new(source: S, cfg: StreamConfig) -> Self {
        Self {
            source,
            cfg,
            slots: HashMap::new(),
            demand: HashMap::new(),
            resident_bytes: 0,
            now: 0.0,
        }
    }

    /// Die Quelle entnehmen (z. B. um sie einem anderen Besitzer zu
    /// geben); der Streamer ist danach unbrauchbar.
    pub fn into_source(self) -> S {
        self.source
    }

    /// Auf die Quelle zugreifen (Laden, Puffern — der Streamer bleibt
    /// zuständig für die Zustandsübergänge).
    pub fn source(&self) -> &S {
        &self.source
    }

    pub fn source_mut(&mut self) -> &mut S {
        &mut self.source
    }

    /// Weltzeit setzen — vor [`sync_demand`](Self::sync_demand) und
    /// [`drive`](Self::drive) desselben Frames.
    pub fn set_now(&mut self, now: f64) {
        self.now = now;
    }

    pub fn config(&self) -> &StreamConfig {
        &self.cfg
    }

    pub fn resident_bytes(&self) -> u64 {
        self.resident_bytes
    }

    /// Residente Zellen, deterministisch sortiert nach Zellschlüssel —
    /// Schlüssel und Größe (der Payload liegt beim Aufrufer).
    pub fn resident(&self) -> Vec<(CellKey, u64)> {
        let mut out: Vec<_> = self
            .slots
            .iter()
            .filter(|(_, s)| s.state == SlotState::Ready)
            .map(|(k, s)| (*k, s.bytes))
            .collect();
        out.sort_unstable_by_key(|(k, _)| *k);
        out
    }

    pub fn state_of(&self, cell: CellKey) -> Option<SlotState> {
        self.slots.get(&cell).map(|s| s.state)
    }

    pub fn is_resident(&self, cell: CellKey) -> bool {
        matches!(self.state_of(cell), Some(SlotState::Ready))
    }

    /// Die aktuelle Nachfrage (Clipmap-Zellen mit Prioritäten) setzen.
    /// Neue Zellen werden eingereiht, bekannte behalten ihren
    /// Fortschritt; nachgefragte Residente erneuern ihre LRU-Spur.
    /// Eingereihte/ladende Zellen, die niemand mehr will, fallen weg.
    pub fn sync_demand(&mut self, cells: impl IntoIterator<Item = (CellKey, Priority)>) {
        self.demand.clear();
        for (k, p) in cells {
            self.demand.insert(k, p);
            match self.slots.get_mut(&k) {
                Some(s) if s.state == SlotState::Ready => s.last_use = self.now,
                None => {
                    self.slots.insert(
                        k,
                        Slot {
                            state: SlotState::Queued,
                            bytes: 0,
                            last_use: self.now,
                        },
                    );
                }
                _ => {}
            }
        }
        // Niemand will sie mehr und sie sind nicht Resident → fort werfen.
        let stale: Vec<CellKey> = self
            .slots
            .iter()
            .filter(|(k, s)| s.state != SlotState::Ready && !self.demand.contains_key(k))
            .map(|(k, _)| *k)
            .collect();
        for k in stale {
            self.slots.remove(&k);
            self.source.cancel(k);
        }
    }

    /// Ein Rahmen: laufende Loads pollen, freie Plätze vergeben, Budget
    /// einhalten.
    pub fn drive(&mut self) -> StreamDelta<S::Payload> {
        let mut delta = StreamDelta::default();

        // 1) Laufende pollen — deterministisch sortiert (Nachfrage-
        //    Priorität, dann Schlüssel), nicht in HashMap-Reihenfolge.
        let mut loading: Vec<CellKey> = self
            .slots
            .iter()
            .filter(|(_, s)| s.state == SlotState::Loading)
            .map(|(k, _)| *k)
            .collect();
        loading.sort_by_key(|k| {
            let prio = self
                .demand
                .get(k)
                .copied()
                .unwrap_or(Priority(u8::MAX, u32::MAX));
            (prio, *k)
        });
        for k in loading {
            match self.source.poll(k, self.now) {
                PollResult::Pending => {}
                PollResult::Failed => {
                    self.slots.remove(&k);
                    delta.failed.push(k);
                }
                PollResult::Ready { payload, bytes } => {
                    self.slots.remove(&k);
                    if bytes > self.cfg.budget_bytes {
                        // Passt niemals ins Budget — ehrlich als Fehler.
                        delta.failed.push(k);
                        continue;
                    }
                    self.evict_until_fits(bytes, &mut delta.evicted);
                    self.slots.insert(
                        k,
                        Slot {
                            state: SlotState::Ready,
                            bytes,
                            last_use: self.now,
                        },
                    );
                    self.resident_bytes += bytes;
                    delta.loaded.push((k, payload));
                }
            }
        }

        // 2) Inflight auffüllen — beste Priorität zuerst.
        let inflight = self
            .slots
            .values()
            .filter(|s| s.state == SlotState::Loading)
            .count();
        let mut free = self.cfg.max_inflight.saturating_sub(inflight);
        while free > 0 {
            let next = self
                .slots
                .iter()
                .filter(|(_, s)| s.state == SlotState::Queued)
                .map(|(k, _)| *k)
                .min_by_key(|k| (self.demand[k], *k));
            let Some(k) = next else { break };
            match self.source.start(k, self.now) {
                Ok(()) => {
                    // Nur wenn die Zelle noch existiert (nicht eben erst
                    // aus der Nachfrage fiel), auf Loading stellen.
                    if let Some(s) = self.slots.get_mut(&k) {
                        s.state = SlotState::Loading;
                    }
                }
                Err(LoadError::Unsupported) => {
                    self.slots.remove(&k);
                    delta.failed.push(k);
                }
            }
            free -= 1;
        }

        delta
    }

    /// Verdrängt Residentes, bis `extra` Bytes ins Budget passen.
    /// Reihenfolge: ungeliebte vor geliebten, dann grobe Ebenen vor
    /// feinen, dann LRU (älteste zuerst), dann Schlüssel.
    fn evict_until_fits(&mut self, extra: u64, evicted: &mut Vec<CellKey>) {
        while self.resident_bytes + extra > self.cfg.budget_bytes {
            // (ist geliebt, Ebene absteigend→grob zuerst, alt zuerst, Schlüssel)
            let mut ready: Vec<(u8, std::cmp::Reverse<u8>, i64, CellKey)> = self
                .slots
                .iter()
                .filter(|(_, s)| s.state == SlotState::Ready)
                .map(|(k, s)| {
                    let prio = self.demand.get(k).map(|p| p.0).unwrap_or(u8::MAX);
                    let liked = self.demand.contains_key(k) as u8;
                    (liked, std::cmp::Reverse(prio), s.last_use as i64, *k)
                })
                .collect();
            ready.sort_unstable();
            let Some(&(_, _, _, victim)) = ready.first() else {
                break; // nichts mehr zu verdrängen
            };
            if let Some(slot) = self.slots.remove(&victim) {
                self.resident_bytes -= slot.bytes;
                evicted.push(victim);
            }
        }
    }
}

/// Einfache Platzhalter-Quelle: lädt nach `latency` Sekunden fertig,
/// Payload ist der Zellschlüssel selbst, Größe fest. Für Tests und
/// Offline-Demos — M3c ersetzt sie durch den IO-Thread-Pool.
#[derive(Debug, Clone)]
pub struct MockSource {
    /// Sekunden von `start` bis `Ready`.
    pub latency: f64,
    /// Vorgeschriebene Größe jeder Payload.
    pub bytes: u64,
    /// Zellen, die diese Quelle ablehnt (Fehlerpfad testen).
    pub reject: Vec<CellKey>,
    started: HashMap<CellKey, f64>,
}

impl MockSource {
    pub fn new(latency: f64, bytes: u64) -> Self {
        Self {
            latency,
            bytes,
            reject: Vec::new(),
            started: HashMap::new(),
        }
    }
}

impl CellSource for MockSource {
    type Payload = CellKey;

    fn start(&mut self, cell: CellKey, now: f64) -> Result<(), LoadError> {
        if self.reject.contains(&cell) {
            return Err(LoadError::Unsupported);
        }
        self.started.insert(cell, now);
        Ok(())
    }

    fn poll(&mut self, cell: CellKey, now: f64) -> PollResult<CellKey> {
        match self.started.get(&cell).copied() {
            Some(t0) if now - t0 >= self.latency => {
                self.started.remove(&cell);
                PollResult::Ready {
                    payload: cell,
                    bytes: self.bytes,
                }
            }
            Some(_) => PollResult::Pending,
            None => PollResult::Failed,
        }
    }

    fn cancel(&mut self, cell: CellKey) {
        self.started.remove(&cell);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn streamer(latency: f64, cell_bytes: u64, budget: u64, inflight: usize) -> Streamer<MockSource> {
        Streamer::new(
            MockSource::new(latency, cell_bytes),
            StreamConfig {
                budget_bytes: budget,
                max_inflight: inflight,
            },
        )
    }

    fn key(level: u8, x: i64, y: i64) -> CellKey {
        CellKey::new(level, x, y)
    }

    #[test]
    fn null_latency_laedt_sofort_und_resident() {
        let mut s = streamer(0.0, 100, 1000, 4);
        s.sync_demand(vec![(key(0, 0, 0), Priority(0, 0))]);
        let d = s.drive(); // startet den Load
        assert!(d.loaded.is_empty());
        assert_eq!(s.state_of(key(0, 0, 0)), Some(SlotState::Loading));
        let d = s.drive(); // pollt fertig
        assert_eq!(d.loaded.len(), 1);
        assert_eq!(d.loaded[0].0, key(0, 0, 0));
        assert_eq!(s.resident_bytes(), 100);
        assert!(s.is_resident(key(0, 0, 0)));
    }

    #[test]
    fn latency_entscheidet_wann_fertig() {
        let mut s = streamer(1.0, 10, 1000, 4);
        s.set_now(0.0);
        s.sync_demand(vec![(key(0, 0, 0), Priority(0, 0))]);
        s.drive();
        assert!(s.drive().loaded.is_empty(), "vor latency nicht fertig");
        s.set_now(1.5);
        assert_eq!(s.drive().loaded.len(), 1);
    }

    #[test]
    fn doppelte_anfrage_startt_nur_einmal() {
        let mut s = streamer(0.0, 10, 1000, 4);
        s.sync_demand(vec![(key(0, 0, 0), Priority(0, 0))]);
        s.drive();
        s.sync_demand(vec![(key(0, 0, 0), Priority(0, 1))]);
        // Läuft bereits — genau eine Payload, danach Ruhe.
        assert_eq!(s.drive().loaded.len(), 1);
        assert!(s.drive().loaded.is_empty());
    }

    #[test]
    fn budget_verdraengt_grobe_zuerst() {
        let mut s = streamer(0.0, 100, 250, 8);
        // 2 × 100 = 200 passt, 3. nicht mehr.
        s.sync_demand(vec![
            (key(0, 0, 0), Priority(0, 0)),
            (key(2, 0, 0), Priority(2, 4)),
        ]);
        s.drive();
        s.drive();
        assert_eq!(s.resident().len(), 2);
        // Dritte, feine Zelle kommt dazu — weichen muss die grobe Ebene.
        s.sync_demand(vec![
            (key(0, 0, 0), Priority(0, 0)),
            (key(2, 0, 0), Priority(2, 4)),
            (key(0, 1, 0), Priority(0, 1)),
        ]);
        s.drive();
        let d = s.drive();
        assert_eq!(d.evicted, vec![key(2, 0, 0)], "grobe Ebene muss weichen");
        assert!(s.resident_bytes() <= 250);
    }

    #[test]
    fn verlassene_zelle_bleibt_bis_budgetdruck() {
        let mut s = streamer(0.0, 100, 1000, 4);
        s.sync_demand(vec![(key(0, 0, 0), Priority(0, 0))]);
        s.drive();
        s.drive();
        // Zelle verlässt die Nachfrage — bleibt aber Resident (Pooling).
        s.sync_demand(vec![]);
        assert!(s.is_resident(key(0, 0, 0)), "Pooling statt Entladen");

        // Enges Budget: die neue Nachfrage drängt die ungenutzte raus.
        let mut tight = streamer(0.0, 100, 100, 4);
        tight.sync_demand(vec![(key(0, 0, 0), Priority(0, 0))]);
        tight.drive();
        tight.drive();
        tight.sync_demand(vec![(key(0, 5, 5), Priority(0, 5))]);
        let _ = tight.drive(); // startet die neue, alte bleibt noch Resident
        let d = tight.drive();
        assert_eq!(d.evicted, vec![key(0, 0, 0)], "LRU opfert die ungenutzte Zelle");
    }

    #[test]
    fn abgebrochene_queued_zelle_startet_nie() {
        let mut s = streamer(0.0, 10, 1000, 4);
        s.sync_demand(vec![(key(0, 0, 0), Priority(0, 0))]);
        // Noch kein drive — Zelle ist nur Queued; Nachfrage fällt weg.
        s.sync_demand(vec![]);
        assert_eq!(s.state_of(key(0, 0, 0)), None);
        let d = s.drive();
        assert!(d.loaded.is_empty() && d.failed.is_empty());
    }

    #[test]
    fn abgelehnte_zelle_meldet_sich_als_fehler() {
        let mut src = MockSource::new(0.0, 10);
        let bad = key(0, 7, 7);
        src.reject.push(bad);
        let mut s = Streamer::new(
            src,
            StreamConfig {
                budget_bytes: 100,
                max_inflight: 2,
            },
        );
        s.sync_demand(vec![(bad, Priority(0, 0))]);
        let d = s.drive();
        assert_eq!(d.failed, vec![bad]);
        assert_eq!(s.state_of(bad), None, "defekte Zelle wird fallengelassen");
    }

    #[test]
    fn ueber_grosse_payload_scheitert() {
        let mut s = streamer(0.0, 5000, 1000, 4);
        s.sync_demand(vec![(key(0, 0, 0), Priority(0, 0))]);
        s.drive();
        let d = s.drive();
        assert!(d.loaded.is_empty());
        assert_eq!(d.failed, vec![key(0, 0, 0)], "nie resident, als Fehler");
        assert_eq!(s.resident_bytes(), 0);
    }

    /// Zwei identische Skripte auf frischen Streamern müssen dieselbe
    /// Ereignisfolge liefern — die Basis für Replays.
    #[test]
    fn ereignisfolge_ist_deterministisch() {
        fn script(t0: f64) -> Vec<Vec<String>> {
            let mut s = streamer(0.5, 100, 250, 2);
            let mut log = Vec::new();
            for step in 0..12 {
                s.set_now(t0 + step as f64 * 0.25);
                // Stabile Kerlnachfrage + eine Zelle, die alle 4 Schritte
                // (1 s = 2× Latenz) die Seite wechselt → Pooling, LRU, Reload.
                let sw = if step % 8 < 4 { 5 } else { 7 };
                s.sync_demand(vec![
                    (key(0, 0, 0), Priority(0, 0)),
                    (key(1, 0, 1), Priority(1, 0)),
                    (key(0, sw, 0), Priority(0, sw as u32)),
                ]);
                let d = s.drive();
                let mut frame: Vec<String> = Vec::new();
                for (k, _) in &d.loaded {
                    frame.push(format!("L {k:?}"));
                }
                for k in &d.evicted {
                    frame.push(format!("E {k:?}"));
                }
                for k in &d.failed {
                    frame.push(format!("F {k:?}"));
                }
                log.push(frame);
            }
            log
        }
        let a = script(0.0);
        let b = script(0.0);
        assert_eq!(a, b, "gleiches Skript, gleiche Ereignisse");
        let flat: Vec<&String> = a.iter().flatten().collect();
        assert!(flat.iter().any(|e| e.starts_with('L')), "mindestens ein Load");
        assert!(flat.iter().any(|e| e.starts_with('E')), "mindestens eine Verdrängung");
    }
}
