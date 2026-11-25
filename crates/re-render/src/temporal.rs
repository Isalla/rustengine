//! Temporal-Filter-Referenz (M5, Box 3): TAA und Upscaler-Grundlage.
//!
//! Zeitliche Filter (TAA, TAA-getriebene Upscaler wie FSR 2/3, DLSS)
//! brauchen drei Schritte, die hier als CPU-Referenz leben —
//! backend-neutral, deterministisch, headless testbar:
//!
//! 1. **Reprojektion**: Das History-Pixel liegt bei `pixel − velocity`
//!    (Motion-Vektoren aus dem G-Buffer, [`Surface::velocity`]).
//! 2. **Box-Clamp**: Die reprojizierte History-Farbe wird in die
//!    Farb-Box der aktuellen 3×3-Nachbarschaft geklemmt — das ist der
//!    Ghosting-Schutz an verdeckten Kanten.
//! 3. **Akkumulation**: exponentielle Mischung aus (geclimpter) History
//!    und aktuellem Farbwert.
//!
//! Der GPU-Pass spiegelt später exakt diese Schritte; der Jitter kommt
//! aus [`jitter`](crate::jitter), die Velocity aus dem G-Buffer.
//!
//! [`Surface::velocity`]: crate::gbuffer::Surface

/// Achsenparallele Box im RGB-Raum (Nachbarschafts-Grenzen eines Pixels).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RgbBox {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl RgbBox {
    /// Engste Box um die gegebenen Samples (Min/Max je Kanal).
    /// Leere Sample-Liste ergibt die Null-Box.
    pub fn from_samples(samples: &[[f32; 3]]) -> Self {
        let mut b = Self { min: [0.0; 3], max: [0.0; 3] };
        if samples.is_empty() {
            return b;
        }
        b.min = [f32::MAX; 3];
        b.max = [f32::MIN; 3];
        for s in samples {
            for k in 0..3 {
                b.min[k] = b.min[k].min(s[k]);
                b.max[k] = b.max[k].max(s[k]);
            }
        }
        b
    }

    /// Mittelwert ± k·Standardabweichung je Kanal (variance box) —
    /// robuster gegen einzelne Ausreißer als die reine Min/Max-Box.
    pub fn from_variance(samples: &[[f32; 3]], k: f32) -> Self {
        if samples.is_empty() {
            return Self::default_box();
        }
        let n = samples.len() as f32;
        let mut mean = [0.0f32; 3];
        for s in samples {
            for i in 0..3 {
                mean[i] += s[i] / n;
            }
        }
        let mut var = [0.0f32; 3];
        for s in samples {
            for i in 0..3 {
                let d = s[i] - mean[i];
                var[i] += d * d / n;
            }
        }
        let std = var.map(|v| v.sqrt());
        Self {
            min: [
                mean[0] - k * std[0],
                mean[1] - k * std[1],
                mean[2] - k * std[2],
            ],
            max: [
                mean[0] + k * std[0],
                mean[1] + k * std[1],
                mean[2] + k * std[2],
            ],
        }
    }

    fn default_box() -> Self {
        Self { min: [0.0; 3], max: [0.0; 3] }
    }

    /// Klemmt eine Farbe in die Box.
    pub fn clamp(&self, c: [f32; 3]) -> [f32; 3] {
        [
            c[0].clamp(self.min[0], self.max[0]),
            c[1].clamp(self.min[1], self.max[1]),
            c[2].clamp(self.min[2], self.max[2]),
        ]
    }

    /// Box-Diagonale zur History-Farbe hin ausdehnen (Jimenez-2014-Trick),
    /// bevor geklemmt wird: sonst schneidet der Clamp legitime
    /// Kontrastkanten ab und das Bild shimmert. `amount` = 0 deaktiviert.
    pub fn extended_toward(&self, c: [f32; 3], amount: f32) -> Self {
        if amount <= 0.0 {
            return *self;
        }
        let mut out = *self;
        for k in 0..3 {
            let center = (self.min[k] + self.max[k]) * 0.5;
            let p = center + (c[k] - center) * amount;
            out.min[k] = out.min[k].min(p);
            out.max[k] = out.max[k].max(p);
        }
        out
    }
}

/// History-Puffer: RGB (linear) + Akkumulations-Gewicht pro Pixel.
///
/// Gewicht 0 = „kein gültiger Vorgänger" (Frame 1, nach Object ID changes
/// oder Resizes). Koordinaten werden auf den Rand geklemmt — Auslesen
/// außerhalb ist sicher.
#[derive(Clone, Debug, PartialEq)]
pub struct History {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[f32; 4]>,
}

impl History {
    pub fn new(width: u32, height: u32) -> Self {
        let (w, h) = (width.max(1), height.max(1));
        Self { width: w, height: h, pixels: vec![[0.0; 4]; (w * h) as usize] }
    }

    pub fn get(&self, x: u32, y: u32) -> [f32; 4] {
        let x = x.min(self.width - 1) as usize;
        let y = y.min(self.height - 1) as usize;
        self.pixels[y * self.width as usize + x]
    }

    pub fn set(&mut self, x: u32, y: u32, v: [f32; 4]) {
        let x = x.min(self.width - 1) as usize;
        let y = y.min(self.height - 1) as usize;
        self.pixels[y * self.width as usize + x] = v;
    }

    /// Bilineares RGB-Sample in Float-Pixelkoordinaten (Rand wird
    /// geklemmt — kein Kraterrand bei Reprojektion über die Grenze).
    pub fn sample_bilinear(&self, fx: f32, fy: f32) -> [f32; 3] {
        let (w, h) = (self.width as f32 - 1.0, self.height as f32 - 1.0);
        let fx = fx.clamp(0.0, w);
        let fy = fy.clamp(0.0, h);
        let x0 = fx.floor() as u32;
        let y0 = fy.floor() as u32;
        let (x1, y1) = (x0 + 1 < self.width, y0 + 1 < self.height);
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let a = self.get(x0, y0);
        let b = self.get(if x1 { x0 + 1 } else { x0 }, y0);
        let c = self.get(x0, if y1 { y0 + 1 } else { y0 });
        let d = self.get(if x1 { x0 + 1 } else { x0 }, if y1 { y0 + 1 } else { y0 });
        let mix = |i: usize| {
            let ab = a[i] + (b[i] - a[i]) * tx;
            let cd = c[i] + (d[i] - c[i]) * tx;
            ab + (cd - ab) * ty
        };
        [mix(0), mix(1), mix(2)]
    }

    /// Bilinear interpoliertes Gewicht (Gültigkeit) an Float-Koordinaten.
    pub fn weight_bilinear(&self, fx: f32, fy: f32) -> f32 {
        let (w, h) = (self.width as f32 - 1.0, self.height as f32 - 1.0);
        let fx = fx.clamp(0.0, w);
        let fy = fy.clamp(0.0, h);
        let x0 = fx.floor() as u32;
        let y0 = fy.floor() as u32;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let a = self.get(x0, y0)[3];
        let b = self.get(x1, y0)[3];
        let c = self.get(x0, y1)[3];
        let d = self.get(x1, y1)[3];
        let ab = a + (b - a) * tx;
        let cd = c + (d - c) * tx;
        ab + (cd - ab) * ty
    }
}

/// TAA-Zustand: double-buffered History + ein Filterschritt pro Pixel.
#[derive(Clone, Debug)]
pub struct Taa {
    history: History,
    next: History,
    /// Dehnung der Clamp-Box zur History hin (0 = hart geklemmt).
    pub box_extend: f32,
}

impl Taa {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            history: History::new(width, height),
            next: History::new(width, height),
            box_extend: 0.5,
        }
    }

    /// Ein Frame-Schritt für Pixel `(x, y)`.
    ///
    /// * `current` — aktueller linearer RGB-Wert (nach Jitter-Rekonstruktion)
    /// * `neighborhood` — die 3×3-Nachbarschaft von `current` (Box-Quelle)
    /// * `velocity` — Bewegung in Bildschirm-Pixeln seit letztem Frame
    /// * `blend` — Anteil der aktuellen Farbe (0..=1, typ. 0.1 bei 16 Samples)
    ///
    /// Schreibt in den Next-Buffer; erst [`Self::commit`] macht ihn zur
    /// History. Gibt das gemischte Pixel zurück.
    pub fn step(
        &mut self,
        x: u32,
        y: u32,
        current: [f32; 3],
        neighborhood: &[[f32; 3]],
        velocity: [f32; 2],
        blend: f32,
    ) -> [f32; 3] {
        let hx = x as f32 - velocity[0];
        let hy = y as f32 - velocity[1];
        let h_rgb = self.history.sample_bilinear(hx, hy);
        let valid = self.history.weight_bilinear(hx, hy) > 1e-3;
        let clipped = RgbBox::from_samples(neighborhood)
            .extended_toward(h_rgb, self.box_extend)
            .clamp(h_rgb);
        let t = if valid { blend.clamp(0.0, 1.0) } else { 1.0 };
        let out = [
            clipped[0] + (current[0] - clipped[0]) * t,
            clipped[1] + (current[1] - clipped[1]) * t,
            clipped[2] + (current[2] - clipped[2]) * t,
        ];
        self.next.set(x, y, [out[0], out[1], out[2], 1.0]);
        out
    }

    /// Frame beenden: Next wird History, der alte Puffer wird geleert.
    pub fn commit(&mut self) {
        std::mem::swap(&mut self.history, &mut self.next);
        for p in self.next.pixels.iter_mut() {
            *p = [0.0; 4];
        }
    }

    pub fn history(&self) -> &History {
        &self.history
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gray(v: f32) -> [f32; 3] {
        [v, v, v]
    }

    #[test]
    fn erster_frame_ist_durchgereicht() {
        let mut t = Taa::new(2, 2);
        let out = t.step(0, 0, gray(0.7), &[gray(0.7)], [0.0, 0.0], 0.1);
        assert_eq!(out, gray(0.7), "ohne History muss current 1:1 durchgehen");
    }

    #[test]
    fn akkumulation_konvergiert_exponentiell() {
        let mut t = Taa::new(1, 1);
        let wide = [gray(0.0), gray(1.0)]; // Box deckt alles ab
        t.step(0, 0, gray(0.0), &wide, [0.0, 0.0], 0.5);
        t.commit();
        // Ziel wechselt auf 1.0 — Mischung bei blend 0.5 halbiert pro Frame.
        let mut last = 0.0;
        for expected in [0.5, 0.75, 0.875] {
            let out = t.step(0, 0, gray(1.0), &wide, [0.0, 0.0], 0.5);
            assert!((out[0] - expected).abs() < 1e-5, "Frame-Konvergenz {out:?}");
            assert!(out[0] > last, "monoton steigend");
            last = out[0];
            t.commit();
        }
        let mut out = gray(0.875);
        for _ in 0..8 {
            out = t.step(0, 0, gray(1.0), &wide, [0.0, 0.0], 0.5);
            t.commit();
        }
        assert!((out[0] - 1.0).abs() < 0.01, "Zielwert erreicht: {out:?}");
    }

    #[test]
    fn ghosting_wird_in_die_nachbarschaft_geclippt() {
        let mut t = Taa::new(1, 1);
        t.box_extend = 0.0; // harte Klemme für den Test
        let red = [1.0, 0.0, 0.0];
        let green = [0.0, 1.0, 0.0];
        t.step(0, 0, red, &[red], [0.0, 0.0], 0.5);
        t.commit();
        // Szene wechselt zu Grün, Geometrie bewegt sich nicht: die alte
        // rote History liegt außerhalb der grünen Box und muss weg.
        let out = t.step(0, 0, green, &[green], [0.0, 0.0], 0.5);
        assert_eq!(out, green, "kein Rot-Anteil erlaubt: {out:?}");
    }

    #[test]
    fn reprojektion_folgt_der_velocity() {
        let mut t = Taa::new(4, 1);
        let wide = [gray(0.0), gray(1.0)];
        for x in 0..4 {
            let c = if x == 2 { gray(1.0) } else { gray(0.2) };
            t.step(x, 0, c, &wide, [0.0, 0.0], 0.5);
        }
        t.commit();
        // Frame 2: alles dunkel, aber die Szene hat sich um 1 px nach
        // rechts bewegt (velocity +1) -> der helle Streif liegt in der
        // History bei x-1. Bei x=3 muss er auftauchen, bei x=0 nicht.
        let out3 = t.step(3, 0, gray(0.2), &wide, [1.0, 0.0], 0.5);
        let out0 = t.step(0, 0, gray(0.2), &wide, [1.0, 0.0], 0.5);
        assert!(out3[0] > out0[0] + 0.2, "History folgte nicht: {out3:?} vs {out0:?}");
    }

    #[test]
    fn box_dehnung_verhindert_ueberklemmen() {
        let b = RgbBox { min: [0.4; 3], max: [0.6; 3] };
        let c = [0.8; 3];
        assert_eq!(b.clamp(c), [0.6; 3], "ohne Dehnung: hart an der Kante");
        let e = b.extended_toward(c, 0.5);
        let v = e.clamp(c);
        assert!(v[0] > 0.6 && v[0] <= 0.8, "Dehnung lässt Luft: {v:?}");
        // amount 0 ist die Identität.
        assert_eq!(b.extended_toward(c, 0.0), b);
    }

    #[test]
    fn variance_box_und_leere_faelle() {
        let b = RgbBox::from_variance(&[gray(0.0), gray(2.0)], 1.0);
        assert!((b.min[0] - 0.0).abs() < 1e-5 && (b.max[0] - 2.0).abs() < 1e-5);
        let p = RgbBox::from_variance(&[gray(1.0), gray(1.0)], 3.0);
        assert_eq!((p.min, p.max), ([1.0; 3], [1.0; 3]), "k*0-Streuung -> Punkt");
        assert_eq!(RgbBox::from_samples(&[]), RgbBox::default_box());
    }

    #[test]
    fn rand_und_entartete_velocity_nan_frei() {
        let mut t = Taa::new(2, 2);
        t.step(0, 0, gray(0.5), &[gray(0.5)], [0.0, 0.0], 0.5);
        t.commit();
        // Velocity weit über den Rand hinaus: geklemmt, nicht NaN.
        for v in [[100.0, -100.0], [-100.0, 0.0], [0.5, 0.5]] {
            let out = t.step(0, 0, gray(0.5), &[gray(0.5)], v, 0.5);
            assert!(out.iter().all(|c| c.is_finite()), "{v:?} -> {out:?}");
        }
    }
}
