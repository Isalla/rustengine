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
    /// AgX (Blender/Filament-Port). Neutraler, mit sanfterem Highlight-
    /// Roll-off und besserer Farb-Erhaltung in hellen Bereichen.
    /// Arbeitet auf ganzen RGB-Tripeln — via [`ToneCurve::map_rgb`].
    Agx,
}

impl Default for ToneCurve {
    fn default() -> Self {
        ToneCurve::Aces
    }
}

impl ToneCurve {
    /// Ein Kanal: HDR-linear → Display-linear (0..=1). Für kurven, die
    /// kanalunabhängig sind, ist das exakt `map_rgb` auf der Graiachse.
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
            // AgX ist nicht kanalunabhängig — auf der Graiachse ausgewertet.
            ToneCurve::Agx => agx([x, x, x])[0],
        };
        y.clamp(0.0, 1.0)
    }

    /// Ein RGB-Tripel: HDR-linear → Display-linear (0..=1). Kanalu-\
    /// unabhängige Kurven wenden [`Self::map`] pro Kanal an; AgX läuft
    /// durch seine volle Matrix-Kette.
    pub fn map_rgb(&self, x: [f32; 3]) -> [f32; 3] {
        match self {
            ToneCurve::Agx => agx(x),
            _ => x.map(|c| self.map(c)),
        }
    }
}

// ── AgX ────────────────────────────────────────────────────────────────
//
// Port der Referenz-Implementierung aus three.js/Filament (ihrerseits
// Blender-Port mit Rec.2020-Primaries); Konstanten von
// https://iolite-engine.com/blog_posts/minimal_agx_implementation .
// Die Matrizen sind als Spalten notiert (GLSL-Konvention), angewandt als
// M·v = c0·x + c1·y + c2·z.

/// 3×3-Matrix aus drei Spalten-Vektoren auf einen Vektor.
fn m3(c0: [f32; 3], c1: [f32; 3], c2: [f32; 3], v: [f32; 3]) -> [f32; 3] {
    [
        c0[0] * v[0] + c1[0] * v[1] + c2[0] * v[2],
        c0[1] * v[0] + c1[1] * v[1] + c2[1] * v[2],
        c0[2] * v[0] + c1[2] * v[1] + c2[2] * v[2],
    ]
}

/// Die AgX-Sigmoid-Kontrastkurve als Polynom-Fit (Mittelfehler² ≈ 3.7e-6).
fn agx_sigmoid(x: f32) -> f32 {
    let x2 = x * x;
    let x4 = x2 * x2;
    15.5 * x4 * x2
        - 40.14 * x4 * x
        + 31.96 * x4
        - 6.868 * x2 * x
        + 0.4298 * x2
        + 0.1191 * x
        - 0.00232
}

/// AgX-Tonemapping: HDR-linear (sRGB-Primaries) → Display-linear.
///
/// Kette: sRGB→Rec.2020 → AgX-Inbettung → log2-Zustand (Normalisierung
/// auf den Belichtungsbereich der Kurve) → Sigmoid-Kontrast →
/// AgX-Ausbettung → Linearisierung → Rec.2020→sRGB → Klemmung.
fn agx(color: [f32; 3]) -> [f32; 3] {
    // sRGB <-> Rec.2020 (ITU-R BT.2407), Spalten wie im Referenz-Shader.
    const SRGB_TO_2020: [[f32; 3]; 3] = [
        [0.6274, 0.0691, 0.0164],
        [0.3293, 0.9195, 0.0880],
        [0.0433, 0.0113, 0.8956],
    ];
    const SRGB_FROM_2020: [[f32; 3]; 3] = [
        [1.6605, -0.1246, -0.0182],
        [-0.5876, 1.1329, -0.1006],
        [-0.0728, -0.0083, 1.1187],
    ];
    const INSET: [[f32; 3]; 3] = [
        [0.856627153315983, 0.137318972929847, 0.11189821299995],
        [0.0951212405381588, 0.761241990602591, 0.0767994186031903],
        [0.0482516061458583, 0.101439036467562, 0.811302368396859],
    ];
    const OUTSET: [[f32; 3]; 3] = [
        [1.1271005818144368, -0.1413297634984383, -0.14132976349843826],
        [-0.11060664309660323, 1.157823702216272, -0.11060664309660294],
        [-0.016493938717834573, -0.016493938717834257, 1.2519364065950405],
    ];
    // log2(Pow(2, ±)) · MIDDLE_GRAY — der Belichtungsbereich der Kurve.
    const MIN_EV: f32 = -12.47393;
    const MAX_EV: f32 = 4.026069;

    let mut c = m3(SRGB_TO_2020[0], SRGB_TO_2020[1], SRGB_TO_2020[2], color);
    c = m3(INSET[0], INSET[1], INSET[2], c);
    // Log2-Encoding des Zustands, normalisiert auf 0..=1.
    c = c.map(|v| v.max(1e-10).log2());
    c = c.map(|v| ((v - MIN_EV) / (MAX_EV - MIN_EV)).clamp(0.0, 1.0));
    c = c.map(agx_sigmoid);
    c = m3(OUTSET[0], OUTSET[1], OUTSET[2], c);
    // Linearisierung (Sigmoid-Raum → linear), dann Primaries zurück.
    c = c.map(|v| v.max(0.0).powf(2.2));
    c = m3(SRGB_FROM_2020[0], SRGB_FROM_2020[1], SRGB_FROM_2020[2], c);
    c.map(|v| v.clamp(0.0, 1.0))
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
        let exposed = linear.map(|c| c * self.exposure);
        self.curve.map_rgb(exposed)
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

// ── Luminanz und Messung ──────────────────────────────────────────────

/// Rec.709-Leuchtdichte eines linearen RGB-Werts (Gewichtssumme = 1).
pub fn luminance(rgb: [f32; 3]) -> f32 {
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

/// Mittlere Leuchtdichte einer Menge linearer Pixel (arithmetisch).
/// Leere Eingabe liefert 0.
pub fn mean_luminance(pixels: &[[f32; 3]]) -> f32 {
    if pixels.is_empty() {
        return 0.0;
    }
    pixels.iter().map(|p| luminance(*p)).sum::<f32>() / pixels.len() as f32
}

/// Standard-Key der Belichtungsmessung: mittleres Grau.
pub const MIDDLE_GRAY: f32 = 0.18;

/// EV100 aus gemittelter Szenen-Leuchtdichte (linear) — Formel der
/// einfallenden Messung: `ev100 = log2(L · 100 / 12.19)`.
/// Eine Verdopplung von L erhöht EV100 um genau 1 Blende.
pub fn ev100_from_luminance(l: f32) -> f32 {
    (l * 100.0 / 12.19).log2()
}

// ── Zeitliche Belichtungs-Anpassung ───────────────────────────────────

/// Augen-Adaption: die Belichtung wandert mit begrenzter Geschwindigkeit
/// (in Blenden/Sekunde) zum Zielwert, im log2-Raum — wie die Pupille.
///
/// Richtungsgesondert: steigt die Belichtung (Szene wird dunkler),
/// adaptiert das Auge schnell; sinkt sie (Szene wird heller, Blendung),
/// langsam. Beide Tempi sind konfigurierbar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdaptiveExposure {
    /// Aktueller Belichtungs-Multiplikator.
    pub exposure: f32,
    /// Erlaubter Bereich des Multiplikators.
    pub min: f32,
    pub max: f32,
    /// Blenden/Sekunde, wenn die Belichtung steigt (dunkle Szene).
    pub speed_up: f32,
    /// Blenden/Sekunde, wenn die Belichtung sinkt (helle Szene).
    pub speed_down: f32,
}

impl Default for AdaptiveExposure {
    fn default() -> Self {
        Self { exposure: 1.0, min: 1e-4, max: 1e4, speed_up: 3.0, speed_down: 1.0 }
    }
}

impl AdaptiveExposure {
    /// Ziel-Belichtung für eine gemessene Szenen-Leuchtdichte:
    /// `key / L`, in `[min, max]` geklemmt. Schwarze Szenen (L ≤ 0)
    /// ziehen auf `max` (stärkstes Aufhellen).
    pub fn target_for(&self, mean_lum: f32, key: f32) -> f32 {
        if mean_lum <= 0.0 {
            return self.max;
        }
        (key / mean_lum).clamp(self.min, self.max)
    }

    /// Ein Zeitschritt: bewegt `exposure` um höchstens `speed·dt` Blenden
    /// Richtung Ziel (überschießt nie) und gibt den neuen Wert zurück.
    pub fn tick(&mut self, mean_lum: f32, key: f32, dt: f32) -> f32 {
        let target = self.target_for(mean_lum, key);
        let cur = self.exposure.max(1e-9).log2();
        let tgt = target.max(1e-9).log2();
        let delta = self.speed_for(tgt > cur).max(0.0) * dt.max(0.0);
        let new_log = if tgt > cur {
            (cur + delta).min(tgt)
        } else {
            (cur - delta).max(tgt)
        };
        self.exposure = new_log.exp2();
        self.exposure
    }

    /// Tempo je Richtung: steigen ist schnell, sinken ist langsam.
    fn speed_for(&self, rising: bool) -> f32 {
        if rising {
            self.speed_up
        } else {
            self.speed_down
        }
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
            ToneCurve::Agx,
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

    #[test]
    fn luminanz_anker_und_gewichte() {
        assert!(approx(luminance([0.0, 0.0, 0.0]), 0.0, 1e-6));
        assert!(approx(luminance([1.0, 1.0, 1.0]), 1.0, 1e-6));
        assert!(approx(luminance([0.18, 0.18, 0.18]), 0.18, 1e-6));
        // Gewichtssumme = 1 (Reziprok-Test über Primärvalenzen).
        let sum = luminance([1.0, 0.0, 0.0]) + luminance([0.0, 1.0, 0.0]) + luminance([0.0, 0.0, 1.0]);
        assert!(approx(sum, 1.0, 1e-6));
        // Grün dominiert die Luminanz.
        assert!(luminance([0.0, 1.0, 0.0]) > luminance([1.0, 0.0, 0.0]));
    }

    #[test]
    fn mittlere_luminanz() {
        assert_eq!(mean_luminance(&[]), 0.0);
        let m = mean_luminance(&[[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]]);
        assert!(approx(m, 0.5, 1e-6));
    }

    #[test]
    fn ev100_anker_und_blendenregel() {
        // L = 0.1219 ist per Definition EV100 = 0.
        assert!(approx(ev100_from_luminance(0.1219), 0.0, 1e-4));
        // Verdopplung der Leuchtdichte = +1 Blende.
        assert!(approx(ev100_from_luminance(0.2438) - ev100_from_luminance(0.1219), 1.0, 1e-4));
        // Konsistenz mit der Belichtung: 2^-ev100 == 0.1219 / L.
        let l = 0.7;
        assert!(approx(exposure_from_ev100(ev100_from_luminance(l)), 0.1219 / l, 1e-4));
    }

    #[test]
    fn ziel_belichtung_aus_messung() {
        let a = AdaptiveExposure::default();
        // Szene ist genau mittleres Grau -> Belichtung 1,0.
        assert!(approx(a.target_for(MIDDLE_GRAY, MIDDLE_GRAY), 1.0, 1e-6));
        // Halb so hell -> doppelte Belichtung.
        assert!(approx(a.target_for(MIDDLE_GRAY * 0.5, MIDDLE_GRAY), 2.0, 1e-5));
        // Schwarz zieht aufs Maximum, extrem hell unters Minimum.
        assert_eq!(a.target_for(0.0, MIDDLE_GRAY), a.max);
        let eng = AdaptiveExposure { min: 0.5, max: 2.0, ..Default::default() };
        assert_eq!(eng.target_for(1000.0, MIDDLE_GRAY), 0.5);
    }

    #[test]
    fn adaption_naehert_schrittweise_ohne_ueberschiessen() {
        // Dunkle Szene: Ziel ist 32 ( = +5 Blenden), speed_up 3 Blenden/s.
        let mut a = AdaptiveExposure::default();
        let dunkel = MIDDLE_GRAY / 32.0;
        assert!(approx(a.target_for(dunkel, MIDDLE_GRAY), 32.0, 1e-4));
        // dt = 0 ändert nichts.
        assert!(approx(a.tick(dunkel, MIDDLE_GRAY, 0.0), 1.0, 1e-6));
        // Nach 1 s: 3 Blenden hoch -> 8.
        assert!(approx(a.tick(dunkel, MIDDLE_GRAY, 1.0), 8.0, 1e-4));
        // Nach 2 s: Ziel erreicht (nur noch 2 Blenden) -> 32, und es bleibt dort.
        assert!(approx(a.tick(dunkel, MIDDLE_GRAY, 1.0), 32.0, 1e-3));
        assert!(approx(a.tick(dunkel, MIDDLE_GRAY, 5.0), 32.0, 1e-3), "überschießt nicht");
    }

    #[test]
    fn adaption_richungsunterschied() {
        // Heller Szenenwechsel (Belichtung muss sinken) ist langsam
        // (speed_down = 1), der dunkle Wechsel schnell (speed_up = 3).
        let mut langsam = AdaptiveExposure { exposure: 32.0, ..Default::default() };
        let hellig = langsam.tick(MIDDLE_GRAY, MIDDLE_GRAY, 1.0); // Ziel 1
        assert!(approx(hellig, 16.0, 1e-3), "sinkt 1 Blende/s: {hellig}");

        let mut schnell = AdaptiveExposure { exposure: 1.0 / 32.0, ..Default::default() };
        let dunkel = schnell.tick(MIDDLE_GRAY / 32.0, MIDDLE_GRAY, 1.0); // Ziel 1
        assert!(approx(dunkel, 8.0 / 32.0, 1e-3), "steigt 3 Blenden/s: {dunkel}");
        // In Blenden gemessen: steigen bewegt 3x mehr als sinken.
        let steigt = (schnell.exposure * 32.0).log2();
        let sinkt = (32.0 / hellig).log2();
        assert!(approx(sinkt, 1.0, 1e-3) && approx(steigt, 3.0, 1e-3), "3 Blenden vs 1 Blende");
    }

    #[test]
    fn adaption_deterministisch_und_begrenzt() {
        let folge = |seed: f32| {
            let mut a = AdaptiveExposure { exposure: seed, ..Default::default() };
            let mut out = Vec::new();
            for i in 0..20 {
                let l = MIDDLE_GRAY * (1.0 + (i as f32 * 0.7).sin());
                out.push(a.tick(l, MIDDLE_GRAY, 1.0 / 60.0));
            }
            out
        };
        let a = folge(1.0);
        let b = folge(1.0);
        assert_eq!(a, b, "gleiche Eingabe -> gleiche Folge");
        let eng = AdaptiveExposure { min: 0.25, max: 4.0, ..Default::default() };
        let mut g = eng;
        for _ in 0..200 {
            g.tick(1e-6, MIDDLE_GRAY, 1.0);
            assert!(g.exposure <= 4.0 + 1e-5 && g.exposure >= 0.25 - 1e-5, "Ausreißer {}", g.exposure);
            g.tick(1e6, MIDDLE_GRAY, 1.0);
            assert!(g.exposure <= 4.0 + 1e-5 && g.exposure >= 0.25 - 1e-5, "Ausreißer {}", g.exposure);
        }
    }

    #[test]
    fn agx_erhaelt_grauachse() {
        // AgX-Matrizen haben Zeilensumme ~1: Neutralgrau bleibt neutral.
        // Toleranz 5e-3, weil die Referenz-Matrizen (BT.2407) nur auf
        // 4 Nachkommastellen gerundet sind.
        for &g in &[0.01f32, 0.18, 0.5, 2.0, 40.0] {
            let out = ToneCurve::Agx.map_rgb([g, g, g]);
            assert!(
                (out[0] - out[1]).abs() < 5e-3 && (out[1] - out[2]).abs() < 5e-3,
                "grau {g} -> {out:?}"
            );
        }
    }

    #[test]
    fn agx_anker_und_monotonie() {
        assert_eq!(ToneCurve::Agx.map_rgb([0.0, 0.0, 0.0]), [0.0, 0.0, 0.0]);
        let hot = ToneCurve::Agx.map_rgb([100.0, 100.0, 100.0]);
        assert!(hot.iter().all(|&c| c > 0.95), "weiß -> {hot:?}");
        let mut last = -1.0;
        let mut x = 0.0f32;
        while x <= 200.0 {
            let y = ToneCurve::Agx.map(x);
            assert!((0.0..=1.0).contains(&y) && y >= last, "Agx nicht monoton bei {x}");
            last = y;
            x += 0.1;
        }
    }

    #[test]
    fn agx_erhaelt_farbreihenfolge() {
        // Warme HDR-Lichter bleiben warm (R > G > B) — AgX desättiert
        // sanft in Highlights, kippt aber die Farbordnung nicht.
        let out = ToneCurve::Agx.map_rgb([3.0, 1.2, 0.3]);
        assert!(out[0] > out[1] && out[1] > out[2], "Reihenfolge {out:?}");
        // Und AgX verhält sich anders als ACES (eigene Kurve, kein Alias).
        let aces = ToneCurve::Aces.map_rgb([3.0, 1.2, 0.3]);
        let delta = (out[0] - aces[0]).abs() + (out[1] - aces[1]).abs() + (out[2] - aces[2]).abs();
        assert!(delta > 0.01, "AgX == ACES? {out:?} vs {aces:?}");
    }

    #[test]
    fn pipeline_mit_agx() {
        let p = HdrPipeline { exposure: 1.3, curve: ToneCurve::Agx };
        let out = p.process([0.5, 0.35, 0.2]);
        assert!(out.iter().all(|&c| (0.0..=1.0).contains(&c)), "{out:?}");
        // Dunkleres Bild bei höherem EV100 gilt für AgX ebenso.
        let dunkel = HdrPipeline { curve: ToneCurve::Agx, ..HdrPipeline::with_ev100(3.0) }.process([0.5, 0.35, 0.2]);
        let hell = HdrPipeline { curve: ToneCurve::Agx, ..HdrPipeline::with_ev100(-1.0) }.process([0.5, 0.35, 0.2]);
        assert!(dunkel[0] < hell[0]);
    }
}
