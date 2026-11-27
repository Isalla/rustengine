//! IO-Telemetrie: gemessene Load-Qualität treibt die Stufen-Regel (M3).
//!
//! Der Streamer weiß als Einziger, wie sich die echte Platte verhält:
//! wie lange ein Block von `start` bis `Ready` brauchte und wie viele
//! Bytes dabei flossen. Diese Datei verdichtet diese Messungen zu einer
//! **Gesundheit** `0..=1` (1 = schnell und zuverlässig) und übersetzt
//! sie in [`crate::tier::TierPolicy`]-Werte: langsame Platte → breitere
//! Wechselzonen, längere Verweilzeiten und kleinere Stufenradien
//! („früher abwerten“), damit das Streaming der Nachschublast hinterher-
//! läuft, statt ihr hinterherzuhecheln.
//!
//! Determinismus: es fließen nur die hereingereichten Messwerte ein —
//! keine Systemzeit, kein Zufall. Dieselbe Messfolge ⇒ derselbe Zustand.

/// Config für [`IoTelemetry`] — die Referenzwerte hängen an der Hardware:
/// was hier „gut“ heißt, muss auf der Zielpalette auch gut sein.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TelemetryConfig {
    /// EWMA-Gewicht jedes neuen Messwerts (`0..1`, klassisch 0.05…0.2).
    pub alpha: f64,
    /// Latenz, ab der es volle Punktzahl gibt (Sekunden). Darunter bleibt
    /// die Latenz-Note auch bei besseren Werten bei 1 — eine schnelle
    /// Platte soll nicht „zu gut“ für ihre eigenen Stufen sein.
    pub ref_latency: f64,
    /// Durchsatz-Referenz in Bytes/Sekunde für die volle Note.
    pub ref_throughput: f64,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            alpha: 0.1,
            ref_latency: 0.25,
            ref_throughput: 262_144.0,
        }
    }
}

/// Gleitende Mittel über Lade-Latenz, Durchsatz und Fehlerquote.
#[derive(Clone, Debug)]
pub struct IoTelemetry {
    cfg: TelemetryConfig,
    samples: u64,
    latency_ewma: f64,
    throughput_ewma: f64,
    /// EWMA über Fehler-Ereignisse (1 = Fehler, 0 = Erfolg).
    fail_ewma: f64,
    failures: u64,
}

impl IoTelemetry {
    pub fn new(cfg: TelemetryConfig) -> Self {
        Self {
            cfg,
            samples: 0,
            latency_ewma: 0.0,
            throughput_ewma: 0.0,
            fail_ewma: 0.0,
            failures: 0,
        }
    }

    /// Erfolgsmessung: ein Block von `bytes` brauchte `latency_secs`.
    /// Latenz ≤ 0 (Sofort-Quelle, Test-Mock) zählt nicht als Durchsatz —
    /// sonst würde eine Scheibe mit Latenz 0 als unendlich schnell gebucht.
    pub fn record_load(&mut self, latency_secs: f64, bytes: u64) {
        let a = self.cfg.alpha;
        if self.samples == 0 {
            self.latency_ewma = latency_secs.max(0.0);
        } else {
            self.latency_ewma += a * (latency_secs.max(0.0) - self.latency_ewma);
        }
        self.samples += 1;
        if latency_secs > 0.0 && bytes > 0 {
            let thr = bytes as f64 / latency_secs;
            self.throughput_ewma = if self.throughput_ewma == 0.0 {
                thr
            } else {
                self.throughput_ewma + a * (thr - self.throughput_ewma)
            };
        }
        // Erfolg zieht die Fehlernote sanft wieder herunter.
        self.fail_ewma += a * (0.0 - self.fail_ewma);
    }

    /// Fehlschlag einer Ladung (Quelle kennt die Zelle nicht, IO-KI).
    pub fn record_failure(&mut self) {
        let a = self.cfg.alpha;
        self.failures += 1;
        self.fail_ewma = if self.samples + self.failures == 1 {
            1.0
        } else {
            self.fail_ewma + a * (1.0 - self.fail_ewma)
        };
    }

    pub fn config(&self) -> &TelemetryConfig {
        &self.cfg
    }

    /// Anzahl erfolgreicher Lasten (die eigentlichen Messwerte).
    pub fn samples(&self) -> u64 {
        self.samples
    }

    pub fn failures(&self) -> u64 {
        self.failures
    }

    /// Mittlere Load-Latenz in Sekunden (0 bis die erste Ladung fertig ist).
    pub fn latency_ewma(&self) -> f64 {
        self.latency_ewma
    }

    /// Gemittelter Durchsatz in Bytes/Sekunde (0 ohne gültige Stichprobe).
    pub fn throughput_ewma(&self) -> f64 {
        self.throughput_ewma
    }

    /// Gesundheit `0..=1`: gewichtetes Mittel aus Latenz- und
    /// Durchsatznote, mal der Zuverlässigkeitsnote. Ohne Messungen gilt
    /// die Anlage als gesund — Anpassung ist dann die Identität.
    pub fn health(&self) -> f32 {
        if self.samples == 0 {
            return 1.0;
        }
        let lat = (self.cfg.ref_latency / self.latency_ewma.max(1e-9)).min(1.0).max(0.0);
        let thr = if self.throughput_ewma > 0.0 {
            (self.throughput_ewma / self.cfg.ref_throughput).min(1.0).max(0.0)
        } else {
            1.0
        };
        let rel = 1.0 - self.fail_ewma.clamp(0.0, 1.0);
        // Latenz und Durchsatz dominieren, Fehlerquote bricht zusätzlich.
        let h = 0.45 * lat + 0.45 * thr + 0.10 * rel;
        (h * rel).clamp(0.0, 1.0) as f32
    }

    /// Stufenregel passend zur aktuellen IO-Lage. Bei `health = 1` exakt
    /// die Basis-Regel; bei schlechter werdender Platte:
    ///
    /// * Radien schrumpfen um bis zu 25 % → Instanzen **früher** abwerten,
    /// * Hysterese wächst um bis zu +0.35 → weniger Grenzpendeln,
    /// * Verweilzeit wächst um bis zu +2 s → Wechsel kosten ja Nachschub.
    pub fn adapt(&self, base: &crate::tier::TierPolicy) -> crate::tier::TierPolicy {
        let slow = 1.0 - self.health();
        crate::tier::TierPolicy {
            radii: base.radii.map(|r| r * (1.0 - 0.25 * slow)),
            hysteresis: (base.hysteresis + 0.35 * slow).min(0.45),
            min_dwell: base.min_dwell + 2.0 * slow,
        }
    }
}

impl Default for IoTelemetry {
    fn default() -> Self {
        Self::new(TelemetryConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tier::{Tier, TierPolicy};

    #[test]
    fn ohne_messungen_ist_alles_gut() {
        let t = IoTelemetry::default();
        assert_eq!(t.health(), 1.0);
        let base = TierPolicy::default();
        let adapted = t.adapt(&base);
        assert_eq!(adapted.radii, base.radii);
        assert_eq!(adapted.hysteresis, base.hysteresis);
        assert_eq!(adapted.min_dwell, base.min_dwell);
    }

    #[test]
    fn schnelle_platte_bleibt_bei_eins() {
        let mut t = IoTelemetry::default();
        for _ in 0..50 {
            t.record_load(0.02, 512 * 1024); // 20 ms für halbes MiB
        }
        assert!(t.health() > 0.99, "health = {}", t.health());
    }

    #[test]
    fn langsame_platte_drueckt_die_gesundheit() {
        let mut t = IoTelemetry::default();
        for _ in 0..50 {
            t.record_load(2.0, 8 * 1024); // 2 s für 8 KiB → ~4 KiB/s
        }
        assert!(t.health() < 0.2, "health = {}", t.health());
        assert!(t.latency_ewma() > 1.9);
        assert!(t.throughput_ewma() > 0.0);
    }

    #[test]
    fn fehler_ziehen_zusaetzlich() {
        let mut sauber = IoTelemetry::default();
        let mut kaputt = IoTelemetry::default();
        for _ in 0..30 {
            sauber.record_load(0.1, 64 * 1024);
            kaputt.record_load(0.1, 64 * 1024);
        }
        for _ in 0..5 {
            kaputt.record_failure();
        }
        assert!(kaputt.health() < sauber.health());
        assert_eq!(kaputt.failures(), 5);
    }

    #[test]
    fn latenz_null_schwaecht_den_durchsatz_nicht() {
        // Sofort-Quelle (Mock): ohne gültige Durchsatz-Stichprobe zählt
        // nur die Latenznote — und die ist bei 0 besser als Referenz.
        let mut t = IoTelemetry::default();
        for _ in 0..10 {
            t.record_load(0.0, 100);
        }
        assert_eq!(t.throughput_ewma(), 0.0);
        assert_eq!(t.health(), 1.0);
    }

    /// Der Kernsatz des Meilensteins: langsame Platte wertet früher ab
    /// und wechselt träger — bei gleicher Distanz.
    #[test]
    fn langsame_platte_wertet_frueher_ab() {
        let base = TierPolicy {
            radii: [25.0, 100.0, 300.0, 800.0],
            hysteresis: 0.1,
            min_dwell: 0.5,
        };
        let mut t = IoTelemetry::default();
        for _ in 0..80 {
            t.record_load(1.5, 4 * 1024);
        }
        let slow = t.adapt(&base);
        assert!(slow.radii[0] < base.radii[0]);
        assert!(slow.hysteresis > base.hysteresis);
        assert!(slow.min_dwell > base.min_dwell + 1.0);
        // Bei 90 m gab es bisher T1, mit langsamer Platte nur noch T2.
        assert_eq!(base.desired(90.0), Tier::T1);
        assert_eq!(slow.desired(90.0), Tier::T2);
    }

    /// Gleiche Messfolge, zweimal gefahren → auf den Bit identisch.
    #[test]
    fn telemetrie_ist_deterministisch() {
        fn run() -> (f64, f64, f32) {
            let mut t = IoTelemetry::new(TelemetryConfig {
                alpha: 0.15,
                ref_latency: 0.2,
                ref_throughput: 100_000.0,
            });
            for i in 0..40 {
                t.record_load(0.05 + (i as f64 * 0.37).sin() * 0.05, 900 + i * 13);
                if i % 7 == 0 {
                    t.record_failure();
                }
            }
            (t.latency_ewma(), t.throughput_ewma(), t.health())
        }
        assert_eq!(run(), run());
    }
}
