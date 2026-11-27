//! GPU-Handoff: von der fertigen Zell-Payload sicher in den VRAM (M3c).
//!
//! Zwischen [`Streamer`](crate::stream::Streamer) und Render-loop liegt
//! ein Engpass mit zwei falschen Antworten: sofort hochladen (der Update-
//! Thread blockiert im Treiber) oder pro Frame alles auf einmal (ein
//! Ruckler, weil die PCIe-Leitung voll läuft, während gezeichnet wird).
//! Dieses Modul antwortet richtig: eine **Triple-Buffer-Pipeline**, die
//! Uploads in Batches zerlegt, pro Frame nur ein Byte-Kontingent
//! durchlässt und höchstens [`HandoffConfig::slots`] Batches gleichzeitig
//! in der Leitung hält. Ist alles belegt, staut sich die Pipeline —
//! sie blockiert nie.
//!
//! Lebenszyklus einer Payload:
//!
//! ```text
//! enqueue ──► pending ──► filling ──► submitted (in flight) ──► resident
//!                        (Kontingent   (Executor hat sie,        (GPU fertig,
//!                         pro Frame)    noch nicht sicher)        zeichenbar)
//! ```
//!
//! Verdrängung [`evict`](CellHandoff::evict) läuft gespiegelt: Residente
//! Zellen werden erst nach einer Schonfrist freigegeben — sonst
//! zerstört man Geometrie, die ein noch laufender Frame zeichnet.
//! Eine Verdrängung mitten im Flug wird erst wirksam, wenn der Flug
//! endet (Tombstone), nie durch Eingriff in die Leitung.
//!
//! Determinismus: Ein einziger Takt [`tick`](CellHandoff::tick) pro
//! Frame treibt alles; die einzigen Zeitgrößen sind Frame-Zähler. Der
//! eigentliche GPU-Aufruf steckt im [`GpuUpload`]-Trait, damit der
//! Scheduler kopflos und damit auf jedem Rechner testbar bleibt.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::grid::CellKey;

/// Ausführende Seite — typischerweise das Render-Backend.
///
/// [`submit`](GpuUpload::submit) darf die Payload übernehmen und so
/// lange brauchen, wie der echte Upload dauert; der Scheduler ruft es
/// bewusst einmal pro Frame mit einem gedeckelten Batch auf, nie
/// mitten in einem anderen System.
pub trait GpuUpload<P> {
    /// Batch fest übernehmen: GPU-Ressourcen für jede Zelle anlegen.
    fn submit(&mut self, frame: u64, jobs: Vec<(CellKey, P, u64)>);

    /// GPU-Ressourcen einer Zelle endgültig freigeben. Kommt
    /// garantiert erst, nachdem die Zelle ihre Schonfrist hinter sich
    /// hat — nie während ein Frame sie noch gezeichnet haben könnte.
    fn release(&mut self, cell: CellKey);
}

/// Parameter der Pipeline.
#[derive(Clone, Copy, Debug)]
pub struct HandoffConfig {
    /// Puffer in der Leitung (klassisches Triple-Buffering: 3).
    /// Einer füllt sich, einer wird übergeben, einer fliegt.
    pub slots: usize,
    /// Höchste Byte-Menge, die ein Frame die Leitung hinunterdarf.
    /// Deckelt die Upload-Zeit pro Frame und hält die Framerate glatt.
    pub upload_limit_bytes: u64,
    /// Frames, die ein übergebenes Batch in der Leitung gilt, bevor die
    /// Zelle als resident zählt (GPU-Kopie + laufende Draw-Calls).
    pub retire_frames: u64,
}

impl Default for HandoffConfig {
    fn default() -> Self {
        Self { slots: 3, upload_limit_bytes: 8 * 1024 * 1024, retire_frames: 2 }
    }
}

impl HandoffConfig {
    fn checked(&self) {
        assert!(self.slots >= 2, "slots muss >= 2 sein (füllen + fliegen)");
        assert!(self.retire_frames >= 1, "retire_frames muss >= 1 sein");
    }
}

/// Ein übergebenes, noch in der Leitung befindliches Batch.
struct InFlight {
    /// Frame, ab dem die Zellen als resident zählen.
    retire_at: u64,
    /// Zellen mit ihrer je eigenen Größe (Budget bleibt ehrlich).
    cells: Vec<(CellKey, u64)>,
    bytes: u64,
}

/// Summen-Report für Diagnose und VRAM-Budget-Regie.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HandoffStats {
    /// Noch nicht in die Füllung gelangt (Kontingent aufgebraucht).
    pub pending: usize,
    /// Aktuell in der Füllung dieses Frames.
    pub filling: usize,
    /// Batches in der Leitung.
    pub inflight: usize,
    /// Zeichenbereite Zellen.
    pub resident: usize,
    /// Davon zum Fregeben vorgemerkt.
    pub to_release: usize,
    /// VRAM der residenten Zellen.
    pub resident_bytes: u64,
    /// resident + inflight + filling: alles, was VRAM belegt oder
    /// gleich belegen wird.
    pub committed_bytes: u64,
}

/// Scheduler zwischen Streamer-Deltas und GPU (siehe Modul-Doku).
pub struct CellHandoff<P> {
    cfg: HandoffConfig,
    frame: u64,
    /// Warteschlange: Kontingent reicht nicht für alle auf einmal.
    pending: VecDeque<(CellKey, P, u64)>,
    /// Was dieser Frame in die Leitung schickt.
    filling: Vec<(CellKey, P, u64)>,
    filling_bytes: u64,
    /// Batches in der Leitung, aufsteigend nach `retire_at`.
    inflight: VecDeque<InFlight>,
    /// Zeichenbereit: Zelle → Byte.
    resident: HashMap<CellKey, u64>,
    /// Verdrängt, während die Payload noch flog/gefüllt wurde — wird
    /// beim Resident-Werden sofort weitergereicht.
    to_release: HashSet<CellKey>,
    /// Freigabe-TERMINE: (fällig ab Frame, Zelle).
    releases: VecDeque<(u64, CellKey)>,
}

impl<P> CellHandoff<P> {
    pub fn new(cfg: HandoffConfig) -> Self {
        cfg.checked();
        Self {
            cfg,
            frame: 0,
            pending: VecDeque::new(),
            filling: Vec::new(),
            filling_bytes: 0,
            inflight: VecDeque::new(),
            resident: HashMap::new(),
            to_release: HashSet::new(),
            releases: VecDeque::new(),
        }
    }

    pub fn config(&self) -> &HandoffConfig {
        &self.cfg
    }

    /// Aktueller Frame-Zähler (1-basiert nach dem ersten Takt).
    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// Frisch geladene Payload anliefern (aus
    /// [`StreamDelta::loaded`](crate::stream::StreamDelta::loaded)).
    pub fn enqueue(&mut self, cell: CellKey, payload: P, bytes: u64) {
        self.pending.push_back((cell, payload, bytes));
    }

    /// Zelle soll aus dem VRAM (Verdrängung, Tier-Wechsel). Wirkt in
    /// jeder Phase — nur nie durch Eingriff in eine laufende Übergabe.
    pub fn evict(&mut self, cell: CellKey) {
        // Noch nicht losgeschickt: einfach aus der Warteschlange werfen.
        self.pending.retain(|(c, _, _)| *c != cell);
        self.filling.retain(|(c, _, _)| *c != cell);
        self.filling_bytes = self.filling.iter().map(|(_, _, b)| *b).sum();
        if self.resident.remove(&cell).is_some() {
            // Zeichnet vielleicht noch der Frame davor — Schonfrist.
            let due = self.frame + self.cfg.retire_frames;
            self.releases.push_back((due, cell));
        } else if self.inflight.iter().any(|b| b.cells.iter().any(|(c, _)| *c == cell)) {
            // Fliegt gerade: Tombstone, beim Resident-Werden freigeben.
            self.to_release.insert(cell);
        }
        // Wenn nichts davon zutraf, war die Zelle nie in der Pipeline.
    }

    /// Zeichnet die GPU diese Zelle schon?
    pub fn is_resident(&self, cell: CellKey) -> bool {
        self.resident.contains_key(&cell)
    }

    /// Residente Zellen, sortiert — für Render-Setzlisten und Tests.
    pub fn resident_cells(&self) -> Vec<CellKey> {
        let mut v: Vec<_> = self.resident.keys().copied().collect();
        v.sort_unstable();
        v
    }

    pub fn stats(&self) -> HandoffStats {
        let inflight_bytes: u64 = self.inflight.iter().map(|b| b.bytes).sum();
        let resident_bytes = self.resident.values().sum();
        HandoffStats {
            pending: self.pending.len(),
            filling: self.filling.len(),
            inflight: self.inflight.len(),
            resident: self.resident.len(),
            to_release: self.to_release.len(),
            resident_bytes,
            committed_bytes: resident_bytes + inflight_bytes + self.filling_bytes,
        }
    }

    /// Ein Frame: retiren, freigeben, übergeben, neu füllen.
    ///
    /// Aufruf genau einmal pro Frame, bevor die Batches gesammelt
    /// werden — dann dürfen genau die [`is_resident`](Self::is_resident)-
    /// Zellen gezeichnet werden.
    pub fn tick(&mut self, gpu: &mut impl GpuUpload<P>) {
        self.frame += 1;
        let frame = self.frame;

        // 1) Abgelaufene Batches: Zellen zählen als angekommen.
        while self.inflight.front().is_some_and(|b| b.retire_at <= frame) {
            let batch = self.inflight.pop_front().unwrap();
            for (cell, bytes) in batch.cells {
                if self.to_release.remove(&cell) {
                    // Verdrängt, während es flog: nie resident gewesen,
                    // direkt zur Freigabe — aber mit Schonfrist, denn
                    // der Treiber hat die Ressourcen gerade erst angelegt.
                    self.releases.push_back((frame + self.cfg.retire_frames, cell));
                    continue;
                }
                self.resident.insert(cell, bytes);
            }
        }

        // 2) Fällige Freigaben (Schonfrist abgelaufen).
        while self.releases.front().is_some_and(|(due, _)| *due <= frame) {
            let (_, cell) = self.releases.pop_front().unwrap();
            gpu.release(cell);
        }

        // 3) Füllung übergeben, wenn ein Slot frei ist.
        if !self.filling.is_empty() && self.inflight.len() + 1 < self.cfg.slots {
            let jobs = std::mem::take(&mut self.filling);
            self.filling_bytes = 0;
            let bytes = jobs.iter().map(|(_, _, b)| *b).sum();
            let cells = jobs.iter().map(|(c, _, b)| (*c, *b)).collect();
            gpu.submit(frame, jobs);
            self.inflight.push_back(InFlight { retire_at: frame + self.cfg.retire_frames, cells, bytes });
        }

        // 4) Neue Füllung aus der Warteschlange — bis zum Kontingent.
        while let Some((_, _, bytes)) = self.pending.front() {
            let fits = self.filling.is_empty() || self.filling_bytes + *bytes <= self.cfg.upload_limit_bytes;
            if !fits {
                break; // nächster Frame holt ab; nie blockieren
            }
            let (cell, payload, bytes) = self.pending.pop_front().unwrap();
            self.filling_bytes += bytes;
            self.filling.push((cell, payload, bytes));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(x: i64) -> CellKey {
        CellKey::new(0, x, 0)
    }

    /// Aufzeichnender Schein-Executor: merkt Übergaben und Freigaben.
    struct Spy<P> {
        submits: Vec<(u64, Vec<CellKey>)>,
        submit_bytes: Vec<u64>,
        releases: Vec<CellKey>,
        _payload: std::marker::PhantomData<P>,
    }

    impl<P> Spy<P> {
        fn new() -> Self {
            Self {
                submits: Vec::new(),
                submit_bytes: Vec::new(),
                releases: Vec::new(),
                _payload: std::marker::PhantomData,
            }
        }
    }

    impl<P> GpuUpload<P> for Spy<P> {
        fn submit(&mut self, frame: u64, jobs: Vec<(CellKey, P, u64)>) {
            self.submit_bytes.push(jobs.iter().map(|(_, _, b)| *b).sum());
            self.submits.push((frame, jobs.into_iter().map(|(c, _, _)| c).collect()));
        }
        fn release(&mut self, cell: CellKey) {
            self.releases.push(cell);
        }
    }

    fn cfg(slots: usize, limit: u64, retire: u64) -> HandoffConfig {
        HandoffConfig { slots, upload_limit_bytes: limit, retire_frames: retire }
    }

    #[test]
    fn payload_durchlaeuft_die_pipeline_ueber_framen() {
        let mut h = CellHandoff::new(cfg(3, 1000, 2));
        let mut spy = Spy::new();
        h.enqueue(key(1), b"hallo".to_vec(), 5);

        h.tick(&mut spy); // füllt
        assert_eq!(spy.submits.len(), 0, "gefüllt wird vor der ersten Übergabe");
        h.tick(&mut spy); // übergibt an Frame 2
        assert_eq!(spy.submits, vec![(2, vec![key(1)])]);
        assert!(!h.is_resident(key(1)), "in der Leitung ist noch nicht resident");
        // retire_at = 2 + 2 = 4
        h.tick(&mut spy);
        assert!(!h.is_resident(key(1)));
        h.tick(&mut spy);
        assert!(h.is_resident(key(1)), "nach retire_frames ist die Zelle zeichenbar");
        assert_eq!(h.resident_cells(), vec![key(1)]);
        assert_eq!(spy.releases, vec![]);
    }

    #[test]
    fn kontingent_deckelt_den_upload_pro_frame() {
        let mut h = CellHandoff::new(cfg(3, 250, 1));
        let mut spy = Spy::new();
        for i in 0..5 {
            h.enqueue(key(i), vec![0u8; 100], 100);
        }
        h.tick(&mut spy); // füllt 2×100 (dritte würde 300 > 250)
        h.tick(&mut spy); // übergibt 2, füllt nach
        h.tick(&mut spy); // übergibt 2, füllt nach
        h.tick(&mut spy); // übergibt die letzte
        let sizes: Vec<_> = spy.submits.iter().map(|(_, c)| c.len()).collect();
        assert_eq!(sizes, vec![2, 2, 1], "pro Frame nur aufs Kontingent");
        assert!(spy.submit_bytes.iter().all(|b| *b <= 250));
    }

    #[test]
    fn einzelner_grosser_block_kommt_durch_kontingent_hindurch() {
        // Ein Block größer als das Frame-Kontingent darf die Pipeline
        // nicht verstopfen: leere Füllung lässt ihn als Einzelstarter.
        let mut h = CellHandoff::new(cfg(3, 100, 1));
        let mut spy = Spy::new();
        h.enqueue(key(7), vec![0u8; 5000], 5000);
        for _ in 0..4 {
            h.tick(&mut spy);
        }
        assert!(h.is_resident(key(7)));
    }

    #[test]
    fn leitung_hoechstens_slots_minus_eins_in_flight() {
        // slots=2 → immer nur EIN Batch fliegt; Übergaben nur, wenn
        // das vorige retiret ist. retire=3 → Rhythmus alle 3 Frames.
        let mut h = CellHandoff::new(cfg(2, 10, 3));
        let mut spy = Spy::new();
        for i in 0..10 {
            h.enqueue(key(i), vec![0u8; 10], 10);
        }
        let mut inflight_peaks = Vec::new();
        for _ in 0..12 {
            h.tick(&mut spy);
            inflight_peaks.push(h.stats().inflight);
        }
        assert!(inflight_peaks.iter().all(|&n| n <= 1), "nie mehr Slots als erlaubt");
        let frames: Vec<_> = spy.submits.iter().map(|(f, _)| *f).collect();
        assert_eq!(frames, vec![2, 5, 8, 11], "Übergaben im Retire-Rhythmus");
        // Stau löst sich vollständig auf: letzte Übergabe Frame 29.
        for _ in 0..30 {
            h.tick(&mut spy);
        }
        assert_eq!(h.resident_cells().len(), 10, "Stau löst sich vollständig auf");
    }

    #[test]
    fn verdraengung_gibt_gpu_erst_nach_schonfrist_frei() {
        let mut h = CellHandoff::new(cfg(3, 1000, 2));
        let mut spy = Spy::new();
        h.enqueue(key(1), vec![0u8; 10], 10);
        for _ in 0..4 {
            h.tick(&mut spy);
        }
        assert!(h.is_resident(key(1)));

        h.evict(key(1)); // Frame 4 → fällig ab Frame 6
        assert_eq!(h.stats().resident, 0, "heraus aus der Zeichenliste");
        h.tick(&mut spy); // Frame 5: noch Schonfrist
        assert!(spy.releases.is_empty(), "zerstört nicht, was noch zeichnen könnte");
        h.tick(&mut spy); // Frame 6: fällig
        assert_eq!(spy.releases, vec![key(1)]);
    }

    #[test]
    fn evict_mitten_im_flug_hinterlaesst_kein_zombie() {
        let mut h = CellHandoff::new(cfg(3, 1000, 2));
        let mut spy = Spy::new();
        h.enqueue(key(9), vec![0u8; 10], 10);
        h.tick(&mut spy);
        h.tick(&mut spy); // übergeben, fliegt
        assert_eq!(h.stats().inflight, 1);
        h.evict(key(9));

        for _ in 0..2 {
            h.tick(&mut spy); // f3, f4 — beim Retiren wird der Tombstone entfacht
        }
        assert!(!h.is_resident(key(9)), "nie zeichenbar gewesen");
        assert_eq!(h.stats().to_release, 0);
        // Freigabe kommt nach eigener Schonfrist.
        h.tick(&mut spy); // f5 — noch nicht fällig
        assert!(spy.releases.is_empty());
        h.tick(&mut spy); // f6 — fällig
        assert_eq!(spy.releases, vec![key(9)], "GPU-Ressourcen werden trotzdem los");
        assert_eq!(h.stats().committed_bytes, 0, "nichts bleibt gebucht");

        // Dieselbe Zelle darf danach regulär wieder einziehen.
        h.enqueue(key(9), vec![1u8; 10], 10);
        for _ in 0..5 {
            h.tick(&mut spy);
        }
        assert!(h.is_resident(key(9)));
    }

    #[test]
    fn evict_in_der_warteschlange_ohne_gpu_kontakt() {
        let mut h = CellHandoff::new(cfg(3, 100, 1));
        let mut spy = Spy::new();
        for i in 0..5 {
            h.enqueue(key(i), vec![0u8; 50], 50);
        }
        h.evict(key(3)); // noch in pending
        for _ in 0..10 {
            h.tick(&mut spy);
        }
        let alle: Vec<_> = spy.submits.iter().flat_map(|(_, c)| c.iter().copied()).collect();
        assert!(!alle.contains(&key(3)), "nie übergeben, nie freigegeben");
        assert!(spy.releases.is_empty());
    }

    #[test]
    fn budget_buchfuehrung_stimmt_ueber_alle_phasen() {
        let mut h = CellHandoff::new(cfg(3, 100, 2));
        let mut spy = Spy::new();
        for i in 0..4 {
            h.enqueue(key(i), vec![0u8; 80], 80);
        }
        h.tick(&mut spy);
        assert_eq!(h.stats().committed_bytes, 80, "eine Füllung, Kontingent bremst");
        h.tick(&mut spy);
        assert_eq!(h.stats().committed_bytes, 160, "in flight + neue Füllung");
        for _ in 0..10 {
            h.tick(&mut spy);
        }
        assert_eq!(h.stats().resident_bytes, 320);
        assert_eq!(h.stats().committed_bytes, 320, "Pipeline leer, nur VRAM zählt");
    }

    #[test]
    fn streamer_und_handoff_ketten_deterministisch() {
        use crate::stream::{MockSource, Priority, StreamConfig, Streamer};

        // MockSource lädt verzögerungsfrei (latency 0) — jede getriebene
        // Zelle wird sofort resident und wandert in den Handoff.
        let cells = [key(0), key(1), key(2)];
        let mut streamer = Streamer::new(
            MockSource::new(0.0, 40),
            StreamConfig { budget_bytes: 1000, max_inflight: 8 },
        );
        streamer.sync_demand(cells.iter().map(|k| (*k, Priority(0, 0))).collect::<Vec<_>>());

        let mut h = CellHandoff::new(cfg(3, 1000, 2));
        let mut spy = Spy::new();
        // Zwei Takte: Streamer füttert, Handoff pipelt.
        for _ in 0..8 {
            let delta = streamer.drive();
            for (cell, payload) in delta.loaded {
                h.enqueue(cell, payload, 40);
            }
            h.tick(&mut spy);
        }
        // Alle drei Streamer-Zellen sind nun GPU-resident.
        assert_eq!(h.resident_cells(), cells.to_vec(), "sortierte Übergabe");
        assert_eq!(spy.submits.iter().map(|(_, c)| c.len()).sum::<usize>(), 3);
        assert!(spy.releases.is_empty(), "nichts verdrängt, nichts freigegeben");
    }
}
