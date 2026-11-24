//! Qualitätsstufen `T0..T4` pro Instanz (docs/architecture.md, Abschnitt 5).
//!
//! * `T0` — Cinematic-/Nahdetall-Mesh,
//! * `T1` — Spiel-Mesh mit Animation,
//! * `T2` — Visual-Hull / vereinfachtes Mesh,
//! * `T3` — Billboard-Batch,
//! * `T4` — Icon/Marker auf der Minimap.
//!
//! Der Kameraflug ist damit reine Stufenfolge: beim Hineinfliegen
//! `T3 → T2 → T1 → T0`. Diese Datei entscheidet **nur** über die Stufen
//! — welches Mesh dann gezeichnet wird, entscheidet der Renderer anhand
//! des Stufen-Indexes.
//!
//! Zwei Dinge verhindern Flackern an Stufengrenzen:
//!
//! * **Hysterese** — eine Stufe wird erst nach `radius · (1 ± h)`
//!   gewechselt, nicht schon am Nennradius;
//! * **Verweilzeit** — nach einem Wechsel bleibt die Stufe
//!   `min_dwell` Sekunden stehen (0 schaltet sie ab).

use std::fmt;

/// Stufe; `T0` ist die beste. `Ord` folgt der Reihenfolge T0 < … < T4,
/// damit „besser werden“ einem kleineren Wert entspricht.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    T0,
    T1,
    T2,
    T3,
    T4,
}

impl Tier {
    pub const ALL: [Tier; 5] = [Tier::T0, Tier::T1, Tier::T2, Tier::T3, Tier::T4];

    /// Index 0..=4 — direkt als LOD-Schlüssel verwendbar.
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(i: usize) -> Option<Tier> {
        Tier::ALL.get(i).copied()
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "T{}", self.index())
    }
}

/// Wechselregel inkl. Gedenksekunden.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TierPolicy {
    /// Maximaldistanz für T0..T3; darüber ist T4. Muss streng monoton
    /// steigend sein.
    pub radii: [f32; 4],
    /// Hysterese-Anteil, z. B. 0.1 = 10 % breitere Wechselzone.
    pub hysteresis: f32,
    /// Sekunden, die eine Stufe mindestens hält, bevor sie wieder darf.
    pub min_dwell: f32,
}

impl Default for TierPolicy {
    fn default() -> Self {
        Self {
            radii: [25.0, 100.0, 300.0, 800.0],
            hysteresis: 0.1,
            min_dwell: 0.5,
        }
    }
}

impl TierPolicy {
    /// Zielstufe für eine Distanz ohne Hysterese.
    pub fn desired(&self, dist: f32) -> Tier {
        for (i, r) in self.radii.iter().enumerate() {
            if dist <= *r {
                return Tier::from_index(i).unwrap();
            }
        }
        Tier::T4
    }

    /// Ein Update-Schritt: liefert die neue Stufe, falls [`TierState`]
    /// sich ändert, sonst `None`. `now` ist Weltzeit in Sekunden.
    ///
    /// Regeln: Besser werden will `dist < r_fein · (1−h)`, schlechter
    /// werden `dist > r_grob · (1+h)`; beides nur nach Ablauf der
    /// Verweilzeit. Sprünge über mehrere Stufen sind erlaubt (Teleport).
    pub fn update(&self, state: &mut TierState, dist: f32, now: f32) -> Option<Tier> {
        debug_assert!(self.is_valid(), "TierPolicy ist inkonsistent");
        let target = self.desired(dist);
        if target == state.tier {
            return None;
        }
        if now - state.since < self.min_dwell {
            return None;
        }
        // Grenzwert zwischen der feineren der beiden beteiligten Stufen.
        let fine = target.min(state.tier);
        let r = self.radii[fine.index()];
        let ok = if target < state.tier {
            dist <= r * (1.0 - self.hysteresis)
        } else {
            dist >= r * (1.0 + self.hysteresis)
        };
        if !ok {
            return None;
        }
        state.tier = target;
        state.since = now;
        Some(target)
    }

    /// Konsistenzprüfung (Aufruf nur in Debug-Pfaden und beim Aufbau).
    pub fn is_valid(&self) -> bool {
        self.radii.windows(2).all(|w| w[0] < w[1])
            && (0.0..0.5).contains(&self.hysteresis)
            && self.min_dwell >= 0.0
    }
}

/// Der zu betreuende Zustand einer Instanz (lebt z. B. als Komponente).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TierState {
    pub tier: Tier,
    /// Zeitpunkt des letzten Wechsels; `-inf`-Start erlaubt sofortigen.
    pub since: f32,
}

impl TierState {
    /// Frische Instanz: Startstufe aus Distanz, Verweilzeit läuft ab `t0`.
    pub fn new(policy: &TierPolicy, dist: f32, t0: f32) -> Self {
        Self {
            tier: policy.desired(dist),
            since: t0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> TierPolicy {
        TierPolicy::default()
    }

    #[test]
    fn stufen_und_anzeige() {
        assert_eq!(Tier::T3.to_string(), "T3");
        assert!(Tier::T1 < Tier::T4);
        assert_eq!(Tier::from_index(4), Some(Tier::T4));
        assert_eq!(Tier::from_index(5), None);
    }

    #[test]
    fn desired_an_den_nennradien() {
        let pol = p();
        assert_eq!(pol.desired(10.0), Tier::T0);
        assert_eq!(pol.desired(25.0), Tier::T0);
        assert_eq!(pol.desired(60.0), Tier::T1);
        assert_eq!(pol.desired(500.0), Tier::T3);
        assert_eq!(pol.desired(5000.0), Tier::T4);
    }

    /// Kernfall Kameraflug: beim gleichmäßigen Hineinfliegen muss die
    /// Stufenfolge T4 → T3 → T2 → T1 → T0 ohne Auslassung oder Rücksprung
    /// durchlaufen werden.
    #[test]
    fn kameraflug_hinein_ist_monoton() {
        let pol = p();
        let mut st = TierState::new(&pol, 2000.0, 0.0);
        assert_eq!(st.tier, Tier::T4);
        let mut saw = vec![st.tier];
        let mut t = 0.0;
        for d in (0..=2000).rev().step_by(1) {
            t += 1.0 / 60.0;
            if let Some(n) = pol.update(&mut st, d as f32, t) {
                assert!(n < *saw.last().unwrap(), "Rücksprung bei {d} m");
                saw.push(n);
            }
        }
        assert_eq!(saw, vec![Tier::T4, Tier::T3, Tier::T2, Tier::T1, Tier::T0]);
    }

    /// Wackeln um einen Nennradius herum darf kein einziges Mal
    /// durch die Hysteresezone wechseln.
    #[test]
    fn hysterese_verhindert_flackern() {
        let pol = p();
        let mut st = TierState::new(&pol, 20.0, 0.0); // T0
        let mut t = 0.0;
        let mut changes = 0;
        for i in 0..600 {
            t += 1.0 / 60.0;
            // zwischen 22 und 27 m pendeln: beide Werte liegen in der
            // Wechselzone (25 · 0.9 = 22.5 … 25 · 1.1 = 27.5).
            let d = if i % 2 == 0 { 22.2 } else { 27.2 };
            if pol.update(&mut st, d, t).is_some() {
                changes += 1;
            }
        }
        assert_eq!(changes, 0, "kein Wechsel innerhalb der Hysteresezone");
    }

    #[test]
    fn klare_ueberschreitung_wechselt() {
        let pol = p();
        let mut st = TierState::new(&pol, 10.0, 0.0);
        assert_eq!(pol.update(&mut st, 40.0, 1.0), Some(Tier::T1));
        assert_eq!(st.tier, Tier::T1);
    }

    #[test]
    fn verweilzeit_bremst_erst_nach_dwell() {
        let pol = TierPolicy {
            min_dwell: 0.5,
            ..Default::default()
        };
        let mut st = TierState::new(&pol, 10.0, 0.0);
        assert_eq!(pol.update(&mut st, 200.0, 0.3), None, "zu früh");
        assert_eq!(pol.update(&mut st, 200.0, 0.6), Some(Tier::T2));
    }

    #[test]
    fn teleport_darf_mehrere_stufen_springen() {
        let pol = TierPolicy {
            min_dwell: 0.0,
            ..Default::default()
        };
        let mut st = TierState::new(&pol, 10.0, 0.0);
        assert_eq!(pol.update(&mut st, 5000.0, 1.0), Some(Tier::T4));
    }
}
