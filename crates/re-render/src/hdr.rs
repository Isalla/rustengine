//! HDR-Farb-Pipeline (M5a).
//!
//! Der Kern einer „HDR-Pipeline" ist die Farbverarbeitung: Die Szene
//! rendert linear mit Werten über 1,0 (High Dynamic Range), und am Ende
//! muss daraus ein Display-Bild in 0..1 werden. Diese Umrechnung ist
//! reine Mathematik — backend-neutral, deterministisch und headless
//! testbar. Sowohl der wgpu-Post-Pass (WGSL) als auch jedes andere
//! Backend nutzen exakt dieselben Kurven, damit das Bild über Backends
//! hinweg identisch aussieht.
//!
//! Pipeline-Reihenfolge pro Pixel:
//! 1. **Belichtung** — Szenen-Luminanz (EV100) auf Display-Weiß normieren.
//! 2. **Tone-Mapping** — HDR-linear (0..∞) auf Display-linear (0..1) stauchen.
//! 3. **sRGB-Encoding** — Display-linear → nichtlineare Display-Werte.
//!
//! Alle Funktionen arbeiten auf einzelnen Kanälen (`f32`) oder
//! RGB-Tripeln `[f32; 3]`; die WGSL-Seite spiegelt sie Kanal-für-Kanal.

/// `1 / 2.4` — der sRGB-Exponent für die Rückrichtung.
const INV_GAMMA: f32 = 1.0 / 2.4;

// ── sRGB-Transfer ─────────────────────────────────────────────────────

/// sRGB (0..1, nichtlinear) → linear (0..1).
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// linear (0..1) → sRGB (0..1, nichtlinear).
pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(INV_GAMMA) - 0.055
    }
}

// ── Belichtung ────────────────────────────────────────────────────────

/// Belichtungs-Multiplikator aus EV100. Höheres EV100 = hellere Szene =
/// kleinerer Multiplikator. `ev100 = 0` liefert 1,0 (unverändert).
pub fn exposure_from_ev100(ev100: f32) -> f32 {
    (-ev100).exp2()
}

// ── Tone-Mapping-Kurven ───────────────────────────────────────────────

/// Staucht HDR-lineare Werte (0..∞) auf Display-linear (0..1).
///
/// Alle Kurven sind monoton steigend, fixieren `0 → 0` und bleiben für
/// positiven Input in `0..=1`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ToneCurve {
    /// Reinhard: `x / (1 + x)`. Einfach, weiche Sättigung.
    Reinhard,
    /// Extended Reinhard mit Weißpunkt: `x·(1 + x/white²) / (1 + x)`.
    /// Werte ab `white` und darüber werden auf 1,0 gekämmt.
    ReinhardExtended { white: f32 },
    /// ACES-Filmic (Narkowicz-Approximation). Der Default moderner
    /// Engines — kräftige, filmische Sättigung.
    Aces,
}

impl Default for ToneCurve {
    fn default() -> Self {
        ToneCurve::Aces
    }
}

impl ToneCurve {
    /// Ein Kanal: HDR-linear → Display-linear (0..=1).
    pub fn map(&self, x: f32) -> f32 {
        if x <= 0.0 {
            return 0.0;
        }
        let y = match self {
            ToneCurve::Reinhard => x / (1.0 + x),
            ToneCurve::ReinhardExtended { white } => {
                let w = white.max(1e-3);
                (x * (1.0 + x / (w * w))) / (1.0 + x)
            }
            ToneCurve::Aces => {
                // (x·(2.51x + 0.03)) / (x·(2.43x + 0.59) + 0.14)
                const A: f32 = 2.51;
                const B: f32 = 0.03;
                const C: f32 = 2.43;
                const D: f32 = 0.59;
                const E: f32 = 0.14;
                (x * (A * x + B)) / (x * (C * x + D) + E)
            }
        };
        y.clamp(0.0, 1.0)
    }
}

// ── Die Pipeline ──────────────────────────────────────────────────────

/// Konfiguration der Farb-Pipeline: Belichtung + Tone-Kurve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HdrPipeline {
    /// Belichtungs-Multiplikator (aus [`exposure_from_ev100`] oder frei).
    pub exposure: f32,
    /// Tone-Mapping-Kurve.
    pub curve: ToneCurve,
}

impl Default for HdrPipeline {
    fn default() -> Self {
        Self { exposure: 1.0, curve: ToneCurve::default() }
    }
}

impl HdrPipeline {
    /// Pipeline mit Belichtung aus EV100 aufsetzen.
    pub fn with_ev100(ev100: f32) -> Self {
        Self { exposure: exposure_from_ev100(ev100), curve: ToneCurve::default() }
    }

    /// Belichten + tonemappen, Ergebnis in Display-linear (0..=1).
    pub fn tonemap(&self, linear: [f32; 3]) -> [f32; 3] {
        linear.map(|c| self.curve.map(c * self.exposure))
    }

    /// Belichten + tonemappen + sRGB-Encoding. Ergebnis in Display-sRGB
    /// (0..=1) — das, was ins Framebuffer/Swapchain gehört.
    pub fn process(&self, linear: [f32; 3]) -> [f32; 3] {
        self.tonemap(linear).map(linear_to_srgb)
    }

    /// Wie [`Self::process`], aber als 8-Bit-RGB für PNG/Readback.
    pub fn process_u8(&self, linear: [f32; 3]) -> [u8; 3] {
        self.process(linear)
            .map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn srgb_und_linear_sind_invers() {
        for &c in &[0.0, 0.01, 0.18, 0.5, 0.75, 1.0] {
            assert!(approx(srgb_to_linear(linear_to_srgb(c)), c, 1e-4), "linear→srgb→linear {c}");
            assert!(approx(linear_to_srgb(srgb_to_linear(c)), c, 1e-4), "srgb→linear→srgb {c}");
        }
    }

    #[test]
    fn srgb_ankerpunkte() {
        // Bekannte Werte des sRGB-Standards.
        assert!(approx(srgb_to_linear(0.5), 0.21404, 1e-4));
        assert!(approx(linear_to_srgb(0.21404), 0.5, 1e-3));
        assert_eq!(srgb_to_linear(0.0), 0.0);
        assert!(approx(srgb_to_linear(1.0), 1.0, 1e-6));
        assert_eq!(linear_to_srgb(0.0), 0.0);
        assert!(approx(linear_to_srgb(1.0), 1.0, 1e-6));
    }

    #[test]
    fn belichtung_exponentiell() {
        assert!(approx(exposure_from_ev100(0.0), 1.0, 1e-6));
        assert!(approx(exposure_from_ev100(1.0), 0.5, 1e-6));
        assert!(approx(exposure_from_ev100(-1.0), 2.0, 1e-6));
        assert!(approx(exposure_from_ev100(2.0), 0.25, 1e-6));
    }

    #[test]
    fn kurven_monoton_und_begrenzt() {
        let kurven = [
            ToneCurve::Reinhard,
            ToneCurve::ReinhardExtended { white: 4.0 },
            ToneCurve::Aces,
        ];
        for curve in kurven {
            assert_eq!(curve.map(0.0), 0.0, "{curve:?}: Schwarz bleibt Schwarz");
            let mut last = -1.0;
            let mut x = 0.0;
            while x <= 1000.0 {
                let y = curve.map(x);
                assert!((0.0..=1.0).contains(&y), "{curve:?}: {x} -> {y} außerhalb 0..1");
                assert!(y >= last, "{curve:?}: nicht monoton bei {x}");
                last = y;
                x += 0.25;
            }
            // Sehr hell -> nahe Weiß.
            assert!(curve.map(1000.0) > 0.99, "{curve:?}: sättigt nicht auf Weiß");
        }
    }

    #[test]
    fn aces_bekannte_referenz() {
        // Narkowicz-ACES bei x=1: (2.51+0.03)/(2.43+0.59+0.14) = 2.54/3.16.
        assert!(approx(ToneCurve::Aces.map(1.0), 2.54 / 3.16, 1e-5));
    }

    #[test]
    fn reinhard_und_extended_anker() {
        assert!(approx(ToneCurve::Reinhard.map(1.0), 0.5, 1e-6));
        // Extended kämmt den Weißpunkt auf genau 1,0.
        assert!(approx(ToneCurve::ReinhardExtended { white: 4.0 }.map(4.0), 1.0, 1e-5));
        assert!(ToneCurve::ReinhardExtended { white: 4.0 }.map(8.0) <= 1.0);
    }

    #[test]
    fn pipeline_erhaelt_grauachse_und_klemmt() {
        let p = HdrPipeline::default();
        // Neutralgrau bleibt neutral (R=G=B).
        let out = p.process([0.18, 0.18, 0.18]);
        assert!(approx(out[0], out[1], 1e-6) && approx(out[1], out[2], 1e-6));
        // Sehr heller Input -> alle Kanäle nahe 1.
        let hot = p.process([50.0, 50.0, 50.0]);
        assert!(hot.iter().all(|&c| c > 0.95));
        // Schwarz bleibt Schwarz.
        assert_eq!(p.process([0.0, 0.0, 0.0]), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn pipeline_dunkelt_hoheres_ev_ab() {
        let linear = [0.5, 0.3, 0.2];
        let dunkel = HdrPipeline::with_ev100(2.0).process(linear);
        let hell = HdrPipeline::with_ev100(-1.0).process(linear);
        assert!(
            dunkel[0] < hell[0],
            "höheres EV100 muss dunkler werden: {:?} vs {:?}",
            dunkel,
            hell
        );
    }

    #[test]
    fn process_u8_im_berreich() {
        let p = HdrPipeline::default();
        let black = p.process_u8([0.0, 0.0, 0.0]);
        assert_eq!(black, [0, 0, 0]);
        let white = p.process_u8([100.0, 100.0, 100.0]);
        assert!(white.iter().all(|&v| v >= 250), "weiß -> {white:?}");
        let mid = p.process_u8([0.18, 0.18, 0.18]);
        assert!(mid.iter().all(|&v| (1..=254).contains(&v)), "grau -> {mid:?}");
    }
}
