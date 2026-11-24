//! # re-world — World Partition ohne Ladebildschirm
//!
//! Umsetzung von `docs/architecture.md` Abschnitt 5 (Welt-Streaming):
//! Die Welt liegt in gleich großen [`grid`]‑Zellen; um die Kamera liegen
//! mehrere [`clipmap`]‑Ebenen mit wachsender Zellgröße, und [`stream`]
//! hält die Zellen innerhalb eines Ressourcen-Budgets lebendig.
//! [`tier`] vergibt die Qualitätsstufen `T0..T4` pro Instanz — der
//! Kameraflug ist damit ein reiner Stufen-Übergang.
//!
//! Bewusste Entscheidungen (M3b):
//!
//! * **Kopflos und deterministisch:** kein Fenster-, Thread- oder
//!   Render-Bezug. [`WorldPartition::update`] bekommt Kamera und `dt`
//!   von außen; gleiche Eingaben liefern immer dieselbe Ereignisfolge.
//!   Das macht das Streaming auf dem Pi testbar, bevor GPU-Ressourcen
//!   überhaupt existieren.
//! * **Zellquelle als Trait:** [`stream::CellSource`] ist die Naht-
//!   stelle. M3b liefert einen synchronen Mock; der echte IO-Thread-
//!   Pool (Async-Load-Pipeline) hängt sich später als Implementierung
//!   an, ohne dass Clipmap, Budget oder Events ihre Form ändern.
//! * **Ebene-zu-Ebene überlappungsarm:** jede Zelle gehört zur feinsten
//!   Ebene, die sie abdeckt (Ringbox-Regel, siehe [`clipmap`]). Doppelte
//!   Abdeckung entsteht nur an den Bandgrenzen, maximal eine Zellbreite
//!   breit — das Budget bleibt dadurch ehrlich.
//! * **Pooling statt Entladen:** verlassene Zellen bleiben als
//!   Resident-LRU liegen, bis das Budget sie verdrängt; Sichtwechsel
//!   ändern nur Prioritäten, nie die Szene.

pub mod clipmap;
pub mod grid;
pub mod partition;
pub mod stream;
pub mod tier;

pub use clipmap::{ClipDiff, Clipmap, ClipmapConfig};
pub use grid::CellKey;
pub use partition::{Instance, PartitionConfig, PartitionDelta, TierChange, WorldPartition};
pub use stream::{
    CellSource, LoadError, MockSource, PollResult, Priority, SlotState, StreamConfig, StreamDelta,
    Streamer,
};
pub use tier::{Tier, TierPolicy, TierState};
