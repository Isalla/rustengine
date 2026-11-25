//! # re-world — World Partition ohne Ladebildschirm
//!
//! Umsetzung von `docs/architecture.md` Abschnitt 5 (Welt-Streaming):
//! Die Welt liegt in gleich großen [`grid`]‑Zellen; um die Kamera liegen
//! mehrere [`clipmap`]‑Ebenen mit wachsender Zellgröße, und [`stream`]
//! hält die Zellen innerhalb eines Ressourcen-Budgets lebendig.
//! [`tier`] vergibt die Qualitätsstufen `T0..T4` pro Instanz — der
//! Kameraflug ist damit ein reiner Stufen-Übergang.
//!
//! Bewusste Entscheidungen:
//!
//! * **Kopfloser Kern:** Clipmap, Tiers und Streamer kennen kein
//!   Fenster, keinen Thread und keine Render-Referenz.
//!   [`WorldPartition::update`] bekommt Kamera und `dt` von außen;
//!   gleiche Eingaben liefern immer dieselbe Ereignisfolge. Das macht
//!   das Streaming auf dem Pi testbar, bevor GPU-Ressourcen überhaupt
//!   existieren. Der einzige Thread-Code lebt in [`io`] — als
//!   austauschbare Quelle, nicht als Voraussetzung.
//! * **Zellquelle als Trait:** [`stream::CellSource`] ist die Naht-
//!   stelle. [`stream::MockSource`] hält den Kern deterministisch,
//!   [`io::AsyncSource`] liefert denselben Vertrag mit echtem
//!   IO-Thread-Pool — Clipmap, Budget und Events bleiben unverändert.
//! * **Ebene-zu-Ebene überlappungsarm:** jede Zelle gehört zur feinsten
//!   Ebene, die sie abdeckt (Ringbox-Regel, siehe [`clipmap`]). Doppelte
//!   Abdeckung entsteht nur an den Bandgrenzen, maximal eine Zellbreite
//!   breit — das Budget bleibt dadurch ehrlich.
//! * **Pooling statt Entladen:** verlassene Zellen bleiben als
//!   Resident-LRU liegen, bis das Budget sie verdrängt; Sichtwechsel
//!   ändern nur Prioritäten, nie die Szene.
//! * **GPU-Handoff als eigene Naht:** [`handoff::CellHandoff`] puffert
//!   frische Payloads in einer Triple-Buffer-Pipeline und übergibt sie
//!   über das Trait [`handoff::GpuUpload`] der GPU — pro Frame nur ein
//!   Byte-Kontingent, nie blockierend. Der Kern bleibt kopflos: Der
//!   Scheduler zählt nur Frames, der echte Upload steckt im Backend.

pub mod clipmap;
pub mod grid;
pub mod handoff;
pub mod hlod;
pub mod io;
pub mod partition;
pub mod stream;
pub mod telemetry;
pub mod tier;

pub use clipmap::{ClipDiff, Clipmap, ClipmapConfig};
pub use grid::CellKey;
pub use hlod::{HlodCell, HlodConfig, HlodDelta, HlodSet};
pub use handoff::{CellHandoff, GpuUpload, HandoffConfig, HandoffStats};
pub use io::{AsyncSource, BlockStore, CellCodec, FileStore};
pub use partition::{Instance, PartitionConfig, PartitionDelta, TierChange, WorldPartition};
pub use stream::{
    CellSource, LoadError, MockSource, PollResult, Priority, SlotState, StreamConfig, StreamDelta,
    Streamer,
};
pub use telemetry::{IoTelemetry, TelemetryConfig};
pub use tier::{Tier, TierPolicy, TierState};
