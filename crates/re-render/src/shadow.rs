//! Cascaded Shadow Maps — Kaskaden-Aufteilung (M5, Shadow 2.0).
//!
//! Nahe Schatten brauchen hohe Auflösung, ferne geringe. CSM schneidet die
//! Sichttiefe in mehrere Kaskaden und legt pro Kaskade eine eigene
//! Shadow-Map. Dieser Kern berechnet **nur die Tiefen-Aufteilung** — reine
//! 1D-Mathematik, backend-neutral und headless testbar. Die 3D-Geometrie
//! (Licht-Raum-Fit pro Kaskade, Texel-Snapping) folgt mit dem GPU-Pass,
//! sobald das Ergebnis visuell prüfbar ist.
//!
//! Aufteilung nach dem verbreiteten „practical split"-Schema: eine
//! Mischung aus logarithmischer und gleichmäßiger Verteilung.
//!
//! ```text
//! split(i) = lerp( log(i), uniform(i), lambda )
//!   lambda = 0  -> rein logarithmisch (packt Kaskaden nah an die Kamera)
//!   lambda = 1  -> gleichmäßig
//!   typisch     -> 0.5 .. 0.8
//! ```

/// Kaskaden-Konfiguration für die Tiefen-Aufteilung.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CascadeSetup {
    /// Anzahl Kaskaden (1..=4).
    pub count: usize,
    /// Kamera-Nahgrenze (> 0).
    pub near: f32,
    /// Reichweite der Schatten (= Kamera-Ferngrenze für den Schattenwurf).
    pub far: f32,
    /// Mischung Log↔Uniform, 0..=1.
    pub lambda: f32,
}

impl CascadeSetup {
    /// Konfiguration mit sinnvollen Klemmen: `count` auf 1..=4,
    /// `lambda` auf 0..=1, `far` strikt über `near`.
    pub fn new(count: usize, near: f32, far: f32, lambda: f32) -> Self {
        let near = near.max(1e-4);
        let far = far.max(near * 1.001);
        Self {
            count: count.clamp(1, 4),
            near,
            far,
            lambda: lambda.clamp(0.0, 1.0),
        }
    }

    /// Die `count + 1` Grenzen, monoton steigend, von `near` bis `far`.
    ///
    /// `boundaries()[0] == near`, `boundaries().last() == far`.
    pub fn boundaries(&self) -> Vec<f32> {
        let n = self.count;
        let mut out = Vec::with_capacity(n + 1);
        let ratio = self.far / self.near;
        for i in 0..=n {
            let f = i as f32 / n as f32;
            let uniform = self.near + (self.far - self.near) * f;
            let log = self.near * ratio.powf(f);
            // lerp(log, uniform, lambda): lambda=0 -> log, lambda=1 -> uniform.
            let split = log + (uniform - log) * self.lambda;
            out.push(if i == n { self.far } else { split });
        }
        out
    }

    /// Die `[near, far]`-Tiefenbereiche je Kaskade, aneinandergereiht.
    /// Länge == `count`; lückenlos und überlappungsfrei.
    pub fn cascade_ranges(&self) -> Vec<(f32, f32)> {
        let b = self.boundaries();
        b.windows(2).map(|w| (w[0], w[1])).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn grenzen_verankern_bei_near_und_far() {
        let s = CascadeSetup::new(4, 0.5, 200.0, 0.6);
        let b = s.boundaries();
        assert_eq!(b.len(), 5);
        assert!(approx(b[0], 0.5, 1e-5));
        assert!(approx(*b.last().unwrap(), 200.0, 1e-5));
    }

    #[test]
    fn grenzen_streng_monoton() {
        for lambda in [0.0, 0.3, 0.6, 1.0] {
            let s = CascadeSetup::new(4, 1.0, 500.0, lambda);
            let b = s.boundaries();
            for w in b.windows(2) {
                assert!(w[1] > w[0], "lambda={lambda}: {w:?} nicht steigend");
            }
        }
    }

    #[test]
    fn uniform_bei_lambda_eins() {
        // lambda = 1 -> gleichmäßige Abstände.
        let s = CascadeSetup::new(4, 0.0, 100.0, 1.0);
        let b = s.boundaries();
        let step = 25.0;
        for (i, &v) in b.iter().enumerate() {
            assert!(approx(v, step * i as f32, 1e-3), "uniform {i}: {v}");
        }
    }

    #[test]
    fn log_packt_nahe_bei_lambda_null() {
        // lambda = 0 -> logarithmisch: der erste Abstand ist der kleinste,
        // der letzte der größte (Kaskaden drängen sich nah an der Kamera).
        let s = CascadeSetup::new(4, 1.0, 256.0, 0.0);
        let b = s.boundaries();
        let gaps: Vec<f32> = b.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(gaps[0] < gaps[1] && gaps[1] < gaps[2] && gaps[2] < gaps[3], "log-Gaps {gaps:?}");
    }

    #[test]
    fn kaskaden_bereiche_lueckenlos() {
        let s = CascadeSetup::new(3, 2.0, 120.0, 0.7);
        let r = s.cascade_ranges();
        assert_eq!(r.len(), 3);
        assert!(approx(r[0].0, 2.0, 1e-5));
        assert!(approx(r[2].1, 120.0, 1e-5));
        for w in r.windows(2) {
            assert!(approx(w[0].1, w[1].0, 1e-5), "Lücke/Überlappung {w:?}");
        }
    }

    #[test]
    fn eine_kaskade_ist_ganzer_bereich() {
        let s = CascadeSetup::new(1, 0.5, 80.0, 0.5);
        let r = s.cascade_ranges();
        assert_eq!(r.len(), 1);
        assert!(approx(r[0].0, 0.5, 1e-5) && approx(r[0].1, 80.0, 1e-5));
    }

    #[test]
    fn klemmt_unsinnige_eingaben() {
        // count > 4 -> 4, count 0 -> 1.
        assert_eq!(CascadeSetup::new(9, 1.0, 100.0, 0.5).count, 4);
        assert_eq!(CascadeSetup::new(0, 1.0, 100.0, 0.5).count, 1);
        // far <= near wird auf near * 1.001 gehoben -> bleibt monoton.
        let s = CascadeSetup::new(2, 10.0, 5.0, 0.5);
        let b = s.boundaries();
        assert!(b.windows(2).all(|w| w[1] > w[0]), "far<=near: {b:?}");
        // lambda außerhalb 0..=1 wird geklemmt.
        assert_eq!(CascadeSetup::new(2, 1.0, 10.0, 3.0).lambda, 1.0);
        assert_eq!(CascadeSetup::new(2, 1.0, 10.0, -2.0).lambda, 0.0);
    }
}
