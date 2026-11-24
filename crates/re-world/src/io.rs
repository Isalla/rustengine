//! Asynchroner Zell-Nachschub: IO-Thread-Pool hinter [`CellSource`] (M3c).
//!
//! Der [`Streamer`](crate::stream::Streamer) stellt die Fragen — dieses
//! Modul liefert die Antworten, **ohne den aufrufenden Thread jemals zu
//! blockieren**: [`AsyncSource::start`] wirft einen Ladeauftrag nur in
//! eine Warteschlange, [`AsyncSource::poll`] sammelt fertig Gestelltes
//! ein. Lesen und Dekodieren geschehen auf Worker-Threads; die Payload
//! wandert über ein Kanal-Ergebnis zurück auf den Haupt-Thread.
//!
//! Zwei austauschbare Seiten:
//!
//! * [`BlockStore`] — woher die rohen Block-Bytes kommen (M3c:
//!   [`FileStore`] vom Datenträger; später Archiv oder HTTP).
//! * [`CellCodec`] — wie aus Bytes eine Payload wird (z. B. Binär →
//!   Szenen-Chunks oder Upload-Befehle für die GPU).
//!
//! Die Budget-Buchführung rechnet mit der **RoheBlocgröße** (Länge der
//! Store-Bytes) — der Codec ändert daran nichts, sonst wäre das Budget
//! nicht mehr ehrlich.
//!
//! Abbruch ist höflich, nicht hart: [`cancel`](CellSource::cancel)
//! verwirft einen Auftrag, falls die Worker ihn noch nicht begonnen
//! haben, und verwirft sein Ergebnis auch nachträglich — eine einmal
//! abgebrochene Zelle liefert niemals `Ready`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::grid::CellKey;
use crate::stream::{CellSource, LoadError, PollResult};

/// Woher die rohen Block-Bytes einer Zelle kommen.
///
/// Der Aufruf geschieht auf einem Worker-Thread und darf dort so lange
/// brauchen, wie er will — der Haupt-Thread wartet nie auf ihn.
pub trait BlockStore: Send + Sync + 'static {
    /// Block-Inhalt lesen; `Ok(None)` heißt: diesen Block gibt es nicht
    /// (die Anforderung schlägt fehl, die Quelle bleibt aber nutzbar).
    fn read(&self, cell: CellKey) -> io::Result<Option<Vec<u8>>>;
}

/// Dateisystem-Store: ein Block pro Datei unter einem Wurzelverzeichnis,
/// Name `l{ebene}_x{x}_y{y}.bin` — flach, sortierbar, ohne Ratespiele.
#[derive(Debug, Clone)]
pub struct FileStore {
    root: PathBuf,
}

impl FileStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Dateiname eines Blocks relativ zur Wurzel.
    pub fn block_file_name(cell: CellKey) -> String {
        format!("l{}_x{}_y{}.bin", cell.level, cell.x, cell.y)
    }

    pub fn path_for(&self, cell: CellKey) -> PathBuf {
        self.root.join(Self::block_file_name(cell))
    }

    /// Block schreiben (inkl. Verzeichnisse) — das natürliche Gegenstück
    /// zu [`read`](BlockStore::read) für Editoren, Pack-Werkzeuge und
    /// Tests.
    pub fn write_block(&self, cell: CellKey, bytes: &[u8]) -> io::Result<()> {
        let path = self.path_for(cell);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, bytes)
    }
}

impl BlockStore for FileStore {
    fn read(&self, cell: CellKey) -> io::Result<Option<Vec<u8>>> {
        match fs::read(self.path_for(cell)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// Bytes → Payload. Läuft auf dem Worker; ein `Err` lässt die Zelle als
/// Fehler scheitern, ohne die Quelle zu beschädigen.
pub type CellCodec<P> = Arc<dyn Fn(CellKey, &[u8]) -> Result<P, String> + Send + Sync + 'static>;

/// Ein Ladeauftrag an die Worker.
#[derive(Clone, Copy)]
struct Job {
    cell: CellKey,
    /// Auftrags-Generation: schützt vor Ergebnissen einer längst
    /// abgebrochenen Ladung derselben Zelle.
    seq: u64,
}

/// Ergebnis eines Workers (an den Haupt-Thread zurück).
struct Done<P> {
    cell: CellKey,
    seq: u64,
    outcome: Outcome<P>,
}

enum Outcome<P> {
    Ok { payload: P, bytes: u64 },
    /// Block nicht vorhanden oder Codec-Fehler — beides: Zelle fällt.
    Failed(String),
}

/// Zellquelle mit Worker-Pool (siehe Modul-Doku).
///
/// Determinismus-Hinweis: Die *Reihenfolge*, in der Zellen fertig
/// werden, bestimmt die Laufzeit — der [`Streamer`](crate::stream::Streamer)
/// pollt sie trotzdem in seiner festen Prioritätsordnung, und alle
/// Budget- und Verdrängungs-Entscheidungen bleiben reproduzierbar.
pub struct AsyncSource<P: Send + 'static> {
    jobs: Sender<Job>,
    results: Receiver<Done<P>>,
    /// Begonnene, noch nicht abgeholte Aufträge: Zelle → Seq.
    live: HashMap<CellKey, u64>,
    /// Stornierte Seqn (mit den Workern geteilt); Einträge, an denen ein
    /// Worker vorbeigekommen ist, entfernt der Abholer.
    cancelled: Arc<Mutex<HashSet<u64>>>,
    /// Fertige, noch nicht abgeholte Ergebnisse.
    ready: HashMap<CellKey, (P, u64)>,
    failed: HashMap<CellKey, String>,
    next_seq: u64,
}

impl<P: Send + 'static> AsyncSource<P> {
    /// `workers` hintere Store-Threads; 0 wird zu 1. Der Pool lebt, bis
    /// die Quelle fällt — dann schließen sich die Kanäle und die Worker
    /// beenden sich von selbst.
    pub fn new<S: BlockStore>(store: S, codec: CellCodec<P>, workers: usize) -> Self {
        let store = Arc::new(store);
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let (done_tx, done_rx) = mpsc::channel::<Done<P>>();
        let job_rx = Arc::new(Mutex::new(job_rx));
        let cancelled = Arc::new(Mutex::new(HashSet::new()));

        for _ in 0..workers.max(1) {
            let store = Arc::clone(&store);
            let codec = Arc::clone(&codec);
            let job_rx = Arc::clone(&job_rx);
            let done_tx = done_tx.clone();
            let cancelled_shared = Arc::clone(&cancelled);
            thread::spawn(move || loop {
                let job = {
                    let Ok(guard) = job_rx.lock() else { return };
                    match guard.recv() {
                        Ok(job) => job,
                        // Quelle fallengelassen — Pool beendet sich.
                        Err(_) => return,
                    }
                };
                // Höflichkeits-Abbruch: vielleicht wird der Block gar
                // nicht mehr gebraucht. (Wettlauf ist erlaubt — der
                // Abholer verwirft Ergebnisse toter Seqn ohnehin.)
                if cancelled_shared.lock().unwrap().contains(&job.seq) {
                    continue;
                }
                let outcome = match store.read(job.cell) {
                    Ok(Some(bytes)) => match codec(job.cell, &bytes) {
                        Ok(payload) => Outcome::Ok {
                            payload,
                            bytes: bytes.len() as u64,
                        },
                        Err(msg) => Outcome::Failed(msg),
                    },
                    Ok(None) => Outcome::Failed("block not found".into()),
                    Err(e) => Outcome::Failed(e.to_string()),
                };
                if done_tx
                    .send(Done {
                        cell: job.cell,
                        seq: job.seq,
                        outcome,
                    })
                    .is_err()
                {
                    return;
                }
            });
        }
        // Das done_tx-Original verwerfen: wenn die Quelle fällt, findet
        // jeder Worker beim nächsten Ergebnis keinen Empfänger mehr und
        // verabschiedet sich zuverlässig.
        drop(done_tx);

        Self {
            jobs: job_tx,
            results: done_rx,
            live: HashMap::new(),
            cancelled,
            ready: HashMap::new(),
            failed: HashMap::new(),
            next_seq: 0,
        }
    }

    /// Anzahl noch nicht abgeholter fertiger Zellen (Diagnose/Tests).
    pub fn pending_results(&self) -> usize {
        self.ready.len() + self.failed.len()
    }

    /// Kanal-Ergebnisse abholen und in die Bereit-Tabellen einbuchen.
    fn harvest(&mut self) {
        while let Ok(done) = self.results.try_recv() {
            let is_current = self.live.get(&done.cell) == Some(&done.seq);
            if !is_current {
                // Auftrag war storniert oder nie (mehr) lebendig.
                self.cancelled.lock().unwrap().remove(&done.seq);
                continue;
            }
            self.live.remove(&done.cell);
            match done.outcome {
                Outcome::Ok { payload, bytes } => {
                    self.ready.insert(done.cell, (payload, bytes));
                }
                Outcome::Failed(msg) => {
                    self.failed.insert(done.cell, msg);
                }
            }
        }
    }
}

impl<P: Send + 'static> CellSource for AsyncSource<P> {
    type Payload = P;

    fn start(&mut self, cell: CellKey, _now: f64) -> Result<(), LoadError> {
        if self.live.contains_key(&cell) || self.ready.contains_key(&cell) {
            return Ok(()); // läuft schon / steht bereit
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        self.live.insert(cell, seq);
        self.jobs
            .send(Job { cell, seq })
            .map_err(|_| LoadError::Unsupported)?;
        Ok(())
    }

    fn poll(&mut self, cell: CellKey, _now: f64) -> PollResult<P> {
        self.harvest();
        if let Some((payload, bytes)) = self.ready.remove(&cell) {
            return PollResult::Ready { payload, bytes };
        }
        if self.failed.remove(&cell).is_some() {
            return PollResult::Failed;
        }
        if self.live.contains_key(&cell) {
            return PollResult::Pending;
        }
        // Nie begonnen oder bereits abgeholt — der Streamer hat für
        // diesen Fall eine Lücke in seiner Tabelle; als Fehler melden.
        PollResult::Failed
    }

    fn cancel(&mut self, cell: CellKey) {
        if let Some(seq) = self.live.remove(&cell) {
            self.cancelled.lock().unwrap().insert(seq);
        }
        // Vielleicht war das Ergebnis schon da — sicherheitshalber weg.
        self.ready.remove(&cell);
        self.failed.remove(&cell);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::{Priority, StreamConfig, Streamer};

    fn temp_store(name: &str) -> FileStore {
        let dir = std::env::temp_dir().join("re-world-tests").join(name);
        let _ = fs::remove_dir_all(&dir);
        FileStore::new(dir)
    }

    /// Payload ist der Block-Inhalt selbst.
    fn bytes_codec() -> CellCodec<Vec<u8>> {
        Arc::new(|_, b| Ok(b.to_vec()))
    }

    fn key(level: u8, x: i64, y: i64) -> CellKey {
        CellKey::new(level, x, y)
    }

    /// Store, dessen `read` auf ein Signal wartet — simuliert träge IO,
    /// ohne echte Zeitabhängigkeit.
    #[derive(Clone)]
    struct GateStore {
        gate: Arc<std::sync::atomic::AtomicBool>,
        data: Arc<HashMap<CellKey, Vec<u8>>>,
    }

    impl GateStore {
        fn new(cells: Vec<(CellKey, Vec<u8>)>) -> Self {
            Self {
                gate: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                data: Arc::new(cells.into_iter().collect()),
            }
        }
        fn open(&self) {
            self.gate
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    impl BlockStore for GateStore {
        fn read(&self, cell: CellKey) -> io::Result<Option<Vec<u8>>> {
            while !self.gate.load(std::sync::atomic::Ordering::Relaxed) {
                thread::sleep(std::time::Duration::from_millis(1));
            }
            Ok(self.data.get(&cell).cloned())
        }
    }

    #[test]
    fn rundlauf_laedt_dateien_ohne_hauptthread_block() {
        let store = temp_store("rundlauf");
        let a = key(0, 0, 0);
        let b = key(0, 1, 2);
        let c = key(2, -3, 4);
        store.write_block(a, b"alpha").unwrap();
        store.write_block(b, b"beta!!").unwrap();
        store.write_block(c, b"c").unwrap();

        let mut src = AsyncSource::new(store, bytes_codec(), 2);
        src.start(a, 0.0).unwrap();
        src.start(b, 0.0).unwrap();
        src.start(c, 0.0).unwrap();

        // Abholen, bis alle drei fertig sind (Zeitfenster großzügig).
        let mut got = HashMap::new();
        for _ in 0..2000 {
            for k in [a, b, c] {
                if got.contains_key(&k) {
                    continue;
                }
                if let PollResult::Ready { payload, bytes } = src.poll(k, 0.0) {
                    assert_eq!(bytes, payload.len() as u64);
                    got.insert(k, payload);
                }
            }
            if got.len() == 3 {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(got.get(&a).map(Vec::as_slice), Some(&b"alpha"[..]));
        assert_eq!(got.get(&b).map(Vec::as_slice), Some(&b"beta!!"[..]));
        assert_eq!(got.get(&c).map(Vec::as_slice), Some(&b"c"[..]));
    }

    #[test]
    fn poll_blockiert_nie_und_liefert_spaeter() {
        let gate = GateStore::new(vec![(key(0, 9, 9), b"spaet".to_vec())]);
        let mut src = AsyncSource::new(gate.clone(), bytes_codec(), 1);
        let k = key(0, 9, 9);
        src.start(k, 0.0).unwrap();
        // Tor zu: poll muss sofort Pending liefern (würde der Haupt-
        // Thread selbst lesen, hinge er hier fest).
        for _ in 0..20 {
            assert!(matches!(src.poll(k, 0.0), PollResult::Pending));
        }
        gate.open();
        for _ in 0..2000 {
            if let PollResult::Ready { payload, .. } = src.poll(k, 0.0) {
                assert_eq!(payload, b"spaet");
                return;
            }
            thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("Ladung wurde nie fertig");
    }

    #[test]
    fn fehlender_block_scheitert_und_quelle_bleibt_nutzbar() {
        let store = temp_store("fehlt");
        store.write_block(key(0, 0, 0), b"x").unwrap();
        let mut src = AsyncSource::new(store, bytes_codec(), 1);

        let good = key(0, 0, 0);
        let missing = key(0, 7, 7);
        src.start(good, 0.0).unwrap();
        src.start(missing, 0.0).unwrap();

        let mut good_done = false;
        let mut missing_failed = false;
        for _ in 0..2000 {
            if !good_done && matches!(src.poll(good, 0.0), PollResult::Ready { .. }) {
                good_done = true;
            }
            if !missing_failed && matches!(src.poll(missing, 0.0), PollResult::Failed) {
                missing_failed = true;
            }
            if good_done && missing_failed {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(good_done, "vorhandener Block lädt");
        assert!(missing_failed, "fehlender Block meldet Fehler");
        // Quelle bleibt nutzbar: neue Anfrage für die gute Zelle läuft.
        src.start(good, 0.0).unwrap();
        assert!(!matches!(src.poll(good, 0.0), PollResult::Failed));
    }

    #[test]
    fn codec_fehler_zaehlt_als_zellfehler() {
        let store = temp_store("codecfehler");
        store.write_block(key(0, 1, 1), b"kaputt").unwrap();
        let boom: CellCodec<Vec<u8>> = Arc::new(|_, _| Err("Codec sagt nein".into()));
        let mut src = AsyncSource::new(store, boom, 1);
        let k = key(0, 1, 1);
        src.start(k, 0.0).unwrap();
        for _ in 0..2000 {
            match src.poll(k, 0.0) {
                PollResult::Failed => return,
                PollResult::Pending => thread::sleep(std::time::Duration::from_millis(1)),
                PollResult::Ready { .. } => panic!("defekter Codec darf nicht Ready liefern"),
            }
        }
        panic!("Fehlermeldung kam nie an");
    }

    #[test]
    fn abbruch_verwirft_spaetere_ergebnisse() {
        let gate = GateStore::new(vec![(key(0, 2, 2), b"egal".to_vec())]);
        let mut src = AsyncSource::new(gate.clone(), bytes_codec(), 1);
        let k = key(0, 2, 2);
        src.start(k, 0.0).unwrap();
        src.cancel(k);
        gate.open();
        // Viel Zeit fürs (nun sinnlose) Ergebnis — es darf nie Ready sein.
        for _ in 0..50 {
            assert!(
                matches!(src.poll(k, 0.0), PollResult::Failed),
                "abgebrochene Ladung liefert nie Ready"
            );
            thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(src.pending_results(), 0, "abgebrochene Ergebnisse sammeln sich nicht");
    }

    #[test]
    fn streamer_betreibt_die_async_quelle() {
        let store = temp_store("streamer_async");
        let cells = [key(0, 0, 0), key(0, 1, 0), key(0, 2, 0), key(0, 3, 0)];
        for (i, k) in cells.iter().enumerate() {
            store.write_block(*k, &[b'a' + i as u8; 100]).unwrap();
        }
        let store_later = store.clone();
        let src = AsyncSource::new(store, bytes_codec(), 3);
        let mut s = Streamer::new(
            src,
            StreamConfig {
                budget_bytes: 350,
                max_inflight: 8,
            },
        );

        // 1) Alle vier anfragen — bei 350 Budget für je 100 bleibt nur
        //    eine dreifache Menge übrig; eine muss weichen.
        s.sync_demand(cells.iter().map(|k| (*k, Priority(0, 0))).collect::<Vec<_>>());
        let mut loaded = 0;
        let mut evicted = 0;
        for step in 0..4000 {
            s.set_now(step as f64 * 0.001);
            let d = s.drive();
            loaded += d.loaded.len();
            evicted += d.evicted.len();
            assert!(d.failed.is_empty(), "keine Fehler bei sauberen Blöcken");
            if loaded >= 4 && evicted == 1 {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(loaded >= 4, "alle Blöcke kommen durch den Pool an");
        assert_eq!(evicted, 1, "Budget drängt genau eine raus");
        assert!(s.resident_bytes() <= 350);
        assert_eq!(s.resident().len(), 3);

        // 2) Eine Zelle fällt aus der Nachfrage, eine neue (noch nie
        //    geladene) kommt: die ungeliebte muss zuerst weichen.
        let neu = key(0, 9, 9);
        store_later.write_block(neu, &vec![b'x'; 100]).unwrap();
        s.sync_demand(
            cells[1..]
                .iter()
                .map(|k| (*k, Priority(0, 0)))
                .chain([(neu, Priority(0, 1))])
                .collect::<Vec<_>>(),
        );
        for step in 4000..8000 {
            s.set_now(step as f64 * 0.001);
            s.drive();
            if s.is_resident(neu) && !s.is_resident(cells[0]) {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(s.is_resident(neu), "neue Zelle lädt nach");
        assert!(s.resident_bytes() <= 350);
    }
}
