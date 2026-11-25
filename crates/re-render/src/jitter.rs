//! Subpixel-Jitter für temporale Verfahren (M5, Box 3).
//!
//! TAA und alle temporalen Upscaler (FSR 2/3, XeSS, DLSS) verlangen
//! denselben Dienst: Die Projektion bekommt pro Frame einen anderen,
//! gleichmäßig verteilten Subpixel-Versatz, damit mehrere Frames zusammen
//! die volle (oder höhere) Abtastabdeckung ergeben. Der Jitter ist die
//! einzige Eingabe, die das Rendering für diese Adapter liefern muss —
//! die Filterung selbst passiert im jeweiligen Backend/SDK.
//!
//! Muster-Lieferant ist die Halton-Folge (Basen 2 und 3): niedrig
//! discrepant, zustandslos und damit deterministisch — Frame `n` liefert
//! immer denselben Versatz, unabhängig davon, wie oft schon gerendert
//! wurde. Genau das brauchen Replay und headless Tests.

/// `i`-tes Element (i ≥ 1) der Halton-Folge zur Basis `base`, Wert in
/// `0..1`. `index = 0` liefert `0.0`.
pub fn halton(index: u32, base: u32) -> f32 {
    let mut i = index;
    let mut f = 1.0 / base as f32;
    let mut r = 0.0;
    while i > 0 {
        r += f * (i % base) as f32;
        i /= base;
        f /= base as f32;
    }
    r
}

/// Zyklisches Subpixel-Jitter-Muster in Pixel-Einheiten.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JitterPattern {
    /// Frames pro Zyklus (typisch 8..16); Frame-Zähler laufen darauf.
    pub frames: u32,
}

impl Default for JitterPattern {
    fn default() -> Self {
        Self { frames: 16 }
    }
}

impl JitterPattern {
    /// Muster mit `frames` Zyklenlänge (mindestens 1).
    pub fn new(frames: u32) -> Self {
        Self { frames: frames.max(1) }
    }

    /// Versatz in Pixeln, zentriert um 0 (jeder Kanal in `-0.5 .. 0.5`),
    /// für Frame `frame`. Der Frame-Zähler läuft zyklisch über `frames`.
    pub fn offset(&self, frame: u32) -> [f32; 2] {
        let i = frame % self.frames + 1;
        [halton(i, 2) - 0.5, halton(i, 3) - 0.5]
    }

    /// Versatz in NDC-Einheiten (-1..1 Bildschirm) für eine Rendergröße
    /// in Pixeln — direkt in die Projektionsmatrix einrechenbar.
    pub fn offset_ndc(&self, frame: u32, render_size: [u32; 2]) -> [f32; 2] {
        let [jx, jy] = self.offset(frame);
        let w = render_size[0].max(1) as f32;
        let h = render_size[1].max(1) as f32;
        [2.0 * jx / w, 2.0 * jy / h]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn halton_bekannte_werte() {
        // Basis 2: 1/2, 1/4, 3/4, 1/8, ...
        assert!(approx(halton(1, 2), 0.5, 1e-6));
        assert!(approx(halton(2, 2), 0.25, 1e-6));
        assert!(approx(halton(3, 2), 0.75, 1e-6));
        assert!(approx(halton(4, 2), 0.125, 1e-6));
        // Basis 3: 1/3, 2/3, 1/9, ...
        assert!(approx(halton(1, 3), 1.0 / 3.0, 1e-6));
        assert!(approx(halton(2, 3), 2.0 / 3.0, 1e-6));
        assert!(approx(halton(3, 3), 1.0 / 9.0, 1e-6));
        assert_eq!(halton(0, 2), 0.0);
    }

    #[test]
    fn halton_bleibt_in_null_eins() {
        for base in [2, 3, 5, 7] {
            for i in 1..=256 {
                let v = halton(i, base);
                assert!((0.0..1.0).contains(&v), "halton({i},{base}) = {v}");
            }
        }
    }

    #[test]
    fn offset_zentriert_und_begrenzt() {
        let p = JitterPattern::new(16);
        for f in 0..64 {
            let [x, y] = p.offset(f);
            assert!(x.abs() <= 0.5 && y.abs() <= 0.5, "Frame {f}: [{x},{y}]");
        }
    }

    #[test]
    fn offset_zyklisch_und_deterministisch() {
        let p = JitterPattern::new(8);
        for f in 0..8 {
            assert_eq!(p.offset(f), p.offset(f + 8), "Zyklus bei {f}");
            assert_eq!(p.offset(f), p.offset(f), "Determinismus bei {f}");
        }
    }

    #[test]
    fn muster_gleichverteilt_ohne_dubletten() {
        let p = JitterPattern::new(16);
        let mut seen = std::collections::HashSet::new();
        let (mut sx, mut sy) = (0.0f32, 0.0f32);
        for f in 0..16 {
            let [x, y] = p.offset(f);
            // Exakte Dubletten wären ein kaputtes Muster.
            assert!(seen.insert((x.to_bits(), y.to_bits())), "Dublette bei Frame {f}");
            sx += x;
            sy += y;
        }
        // Im Mittel zentriert (Halton ist über eine Periode fast ausgewogen).
        assert!((sx / 16.0).abs() < 0.1, "x-Schwerpunkt {}", sx / 16.0);
        assert!((sy / 16.0).abs() < 0.1, "y-Schwerpunkt {}", sy / 16.0);
    }

    #[test]
    fn ndc_umrechnung() {
        let p = JitterPattern::new(16);
        let [jx, jy] = p.offset(0);
        let [nx, ny] = p.offset_ndc(0, [1920, 1080]);
        assert!(approx(nx, 2.0 * jx / 1920.0, 1e-7));
        assert!(approx(ny, 2.0 * jy / 1080.0, 1e-7));
        // NDC-Versatz bleibt weit innerhalb -1..1 (Subpixel!).
        for f in 0..16 {
            let [nx, ny] = p.offset_ndc(f, [800, 600]);
            assert!(nx.abs() < 0.01 && ny.abs() < 0.01, "NDC {nx},{ny}");
        }
    }

    #[test]
    fn klemmt_sinnlos() {
        assert_eq!(JitterPattern::new(0).frames, 1);
        // Ein Frame Zyklus: jeder Frame liefert denselben (ersten) Versatz.
        let p = JitterPattern::new(1);
        assert_eq!(p.offset(0), p.offset(123));
    }
}
