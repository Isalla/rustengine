//! FSR1-Mathematik-Kern (M5, Box 3): EASU + RCAS als deterministische
//! CPU-Referenz.
//!
//! Port von AMD FidelityFX Super Resolution 1.0 — `ffx_fsr1.h`
//! (v1.20210629, MIT-Lizenz), Variante *Non-Packed 32-Bit* (die `F`-Formen).
//! Die WGSL-Seite von `re-wgpu` spiegelt diese Funktionen zug-fuer-zug;
//! der Pixel-Diff-Test beweist die Gleichheit. Der Pi-Profil-Pfad (M13:
//! interne Render-Skala + Upscaler) nutzt genau diese Kette.
//!
//! Abweichungen vom Original — beide Seiten zeigen sie identisch, damit
//! die Gleichheit erhalten bleibt:
//!
//! - AMDs Naherungs-Instruktionen (`rcp`, `rsqrt`, `APrx*`) sind durch
//!   exakte IEEE-Division/`sqrt` ersetzt. Ergebnis: deterministisch und
//!   auf CPU und GPU bitweise dieselbe Rechnung.
//! - Wo die Naeherung in flachen Gebieten `1/0 → inf → 0*inf → NaN`
//!   erzeugt (und die Hardware das per `saturate` auf 0 fallen laesst),
//!   steht hier eine explizite Epsilon-Klemme (1e-30). Gleicher
//!   Grenzwert, kein NaN. Bei 8-Bit-Eingabe (Luma-Stufen ≥ 1/255)
//!   trifft die Klemme ausschließlich den exakten Nullfall.
//!
//! Eingabe-Vertrag (wie bei AMD): Farbe als RGB in 0..1, bereits
//! display-kodiert (Tonemap + sRGB-Kurve) — genau die Bytes, die der
//! Tone-Mapping-Pass im Final-Target hinterlaesst.

/// Luma mal 2 — AMDs billigste Mehrkanal-Naeherung: `B*0.5 + (R*0.5 + G)`.
/// Die Klammerung ist Teil der Spiegelung (beide Seiten dieselbe Ordnung).
#[inline]
fn luma2(c: &[f32; 3]) -> f32 {
    c[2] * 0.5 + (c[0] * 0.5 + c[1])
}

/// Richtung/Laenge eines Bilinear-Quadranten aufsammeln — gespiegeltes
/// `FsrEasuSetF`. `w` ist der Bilinear-Gewicht des Quadranten, die fünf
/// Luma-Werte liegen wie im Original:
/// ```text
///    a
///  b c d     c = Zentrum des Quadranten
///    e
/// ```
// Die AMD-Originalnamen (lA..lE, lenX, dirX) sind Teil der Spiegelung —
// sie machen den Zug-für-Zug-Vergleich mit ffx_fsr1.h moeglich.
#[allow(non_snake_case, clippy::too_many_arguments)]
fn easu_set(dir: &mut [f32; 2], len: &mut f32, w: f32, lA: f32, lB: f32, lC: f32, lD: f32, lE: f32) {
    // Richtung ist die '+'-Differenz; die Laenge holt ihre Groesse aus dem
    // Absolut-Mittel beider Seiten von 'c' (Gradientenumkehr -> 0,
    // glatte Kante -> 1, quadratisch geformt).
    let dc = lD - lC;
    let cb = lC - lB;
    let mut lenX = dc.abs().max(cb.abs());
    lenX = 1.0 / lenX.max(1e-30);
    let dirX = lD - lB;
    dir[0] += dirX * w;
    lenX = (dirX.abs() * lenX).clamp(0.0, 1.0);
    lenX *= lenX;
    *len += lenX * w;
    // Gleiches Spiel vertikal.
    let ec = lE - lC;
    let ca = lC - lA;
    let mut lenY = ec.abs().max(ca.abs());
    lenY = 1.0 / lenY.max(1e-30);
    let dirY = lE - lA;
    dir[1] += dirY * w;
    lenY = (dirY.abs() * lenY).clamp(0.0, 1.0);
    lenY *= lenY;
    *len += lenY * w;
}

/// Ein Tap gegen den Akkumulator — gespiegeltes `FsrEasuTapF`. Der
/// Offset wird in die Kantennormierte gedreht, anisotrop gedehnt und
/// mit der modifizierten Lanczos-2-Naeherung gewichtet (ohne sin/rcp/
/// sqrt — reine Polynome in d²).
#[allow(clippy::too_many_arguments, non_snake_case)]
fn easu_tap(
    aC: &mut [f32; 3],
    aW: &mut f32,
    off: [f32; 2],
    pp: [f32; 2],
    dir: [f32; 2],
    len2: [f32; 2],
    lob: f32,
    clp: f32,
    c: &[f32; 3],
) {
    let ox = off[0] - pp[0];
    let oy = off[1] - pp[1];
    let vx = (ox * dir[0] + oy * dir[1]) * len2[0];
    let vy = (ox * (-dir[1]) + oy * dir[0]) * len2[1];
    let d2 = (vx * vx + vy * vy).min(clp);
    // Basis (a·(b·x²−1)² − (a−1)) mit a=25/16, b=2/5 mal Fenster (¼x²−1)².
    let mut wB = (2.0 / 5.0) * d2 + (-1.0);
    let mut wA = lob * d2 + (-1.0);
    wB *= wB;
    wA *= wA;
    wB = (25.0 / 16.0) * wB + (-(25.0 / 16.0 - 1.0));
    let w = wB * wA;
    aC[0] += c[0] * w;
    aC[1] += c[1] * w;
    aC[2] += c[2] * w;
    *aW += w;
}

/// EASU — Edge Adaptive Spatial Upsampling. Skaliert `src` (sw×sh) nach
/// (dw×dh) mit dem 12-Tap-Kantennormierten Lanczos-Fenster; der Ausgabewert
/// wird an das Minimum/Maximum der naechsten 2×2-Texel geklemmt (entfernt
/// das negative Lobus-Ringing). Rand: Klemmen wie ein Clamp-Sampler.
// Tap- und Akkumulator-Namen spiegeln ffx_fsr1.h (bL..oL, aC, aW, dirR).
#[allow(non_snake_case)]
pub fn easu(src: &[[f32; 3]], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<[f32; 3]> {
    assert!(sw > 0 && sh > 0 && dw > 0 && dh > 0, "FSR1: leerer Bildrand");
    assert_eq!(src.len(), sw * sh, "FSR1: Quelldaten passen nicht zur Groesse");
    // Konstanten — gespiegeltes `FsrEasuCon` (Viewport = ganze Quelle).
    // Dieselben Werte gehen als Uniform auf die GPU, damit beide Seiten
    // bitidentisch rechnen.
    let scale = [sw as f32 / dw as f32, sh as f32 / dh as f32];
    let ofs = [0.5 * scale[0] - 0.5, 0.5 * scale[1] - 0.5];
    let load = |x: i64, y: i64| -> [f32; 3] {
        let x = x.clamp(0, sw as i64 - 1) as usize;
        let y = y.clamp(0, sh as i64 - 1) as usize;
        src[y * sw + x]
    };
    let mut out = vec![[0.0f32; 3]; dw * dh];
    for dy in 0..dh {
        for dx in 0..dw {
            // Position von 'f'; pp ist der gebrochene Rest in der f-Zelle.
            let mut pp = [dx as f32 * scale[0] + ofs[0], dy as f32 * scale[1] + ofs[1]];
            let fp = [pp[0].floor(), pp[1].floor()];
            pp = [pp[0] - fp[0], pp[1] - fp[1]];
            let f = [fp[0] as i64, fp[1] as i64];
            // 12-Tap-Kernel relativ zu f:
            //    b c
            //  e f g h
            //  i j k l
            //    n o
            let col = |ox: i64, oy: i64| load(f[0] + ox, f[1] + oy);
            let b = col(0, -1);
            let c = col(1, -1);
            let e = col(-1, 0);
            let fc = col(0, 0);
            let g = col(1, 0);
            let h = col(2, 0);
            let i = col(-1, 1);
            let j = col(0, 1);
            let k = col(1, 1);
            let l = col(2, 1);
            let n = col(0, 2);
            let o = col(1, 2);
            let (bL, cL) = (luma2(&b), luma2(&c));
            let (eL, fL, gL, hL) = (luma2(&e), luma2(&fc), luma2(&g), luma2(&h));
            let (iL, jL, kL, lL) = (luma2(&i), luma2(&j), luma2(&k), luma2(&l));
            let (nL, oL) = (luma2(&n), luma2(&o));

            // Richtung und Laenge ueber die vier Bilinear-Quadranten.
            let mut dir = [0.0f32; 2];
            let mut len = 0.0f32;
            let (wx, wy) = (pp[0], pp[1]);
            easu_set(&mut dir, &mut len, (1.0 - wx) * (1.0 - wy), bL, eL, fL, gL, jL);
            easu_set(&mut dir, &mut len, wx * (1.0 - wy), cL, fL, gL, hL, kL);
            easu_set(&mut dir, &mut len, (1.0 - wx) * wy, fL, iL, jL, kL, nL);
            easu_set(&mut dir, &mut len, wx * wy, gL, jL, kL, lL, oL);

            // Richtung normalisieren (reziproker sqrt, Nullfall -> horizontal),
            // Laenge in den Bereich 0..1 bringen und quadrieren.
            let dirR2 = dir[0] * dir[0] + dir[1] * dir[1];
            let zro = dirR2 < 1.0 / 32768.0;
            let mut dirR = 1.0 / dirR2.sqrt();
            if zro {
                dirR = 1.0;
                dir[0] = 1.0;
            }
            dir[0] *= dirR;
            dir[1] *= dirR;
            len *= 0.5;
            len *= len;
            // Fenster strecken (1.0 achsparallel bis sqrt(2) diagonal) und
            // die Fensterkante je nach Kantennaehe verschieben.
            let stretch = (dir[0] * dir[0] + dir[1] * dir[1]) * (1.0 / dir[0].abs().max(dir[1].abs()));
            let len2 = [1.0 + (stretch - 1.0) * len, 1.0 + (-0.5) * len];
            let lob = 0.5 + ((1.0 / 4.0 - 0.04) - 0.5) * len;
            let clp = 1.0 / lob;

            // Akkumulation in der Tap-Reihenfolge des Originals.
            let mut aC = [0.0f32; 3];
            let mut aW = 0.0f32;
            easu_tap(&mut aC, &mut aW, [0.0, -1.0], pp, dir, len2, lob, clp, &b);
            easu_tap(&mut aC, &mut aW, [1.0, -1.0], pp, dir, len2, lob, clp, &c);
            easu_tap(&mut aC, &mut aW, [-1.0, 1.0], pp, dir, len2, lob, clp, &i);
            easu_tap(&mut aC, &mut aW, [0.0, 1.0], pp, dir, len2, lob, clp, &j);
            easu_tap(&mut aC, &mut aW, [0.0, 0.0], pp, dir, len2, lob, clp, &fc);
            easu_tap(&mut aC, &mut aW, [-1.0, 0.0], pp, dir, len2, lob, clp, &e);
            easu_tap(&mut aC, &mut aW, [1.0, 1.0], pp, dir, len2, lob, clp, &k);
            easu_tap(&mut aC, &mut aW, [2.0, 1.0], pp, dir, len2, lob, clp, &l);
            easu_tap(&mut aC, &mut aW, [2.0, 0.0], pp, dir, len2, lob, clp, &h);
            easu_tap(&mut aC, &mut aW, [1.0, 0.0], pp, dir, len2, lob, clp, &g);
            easu_tap(&mut aC, &mut aW, [1.0, 2.0], pp, dir, len2, lob, clp, &o);
            easu_tap(&mut aC, &mut aW, [0.0, 2.0], pp, dir, len2, lob, clp, &n);

            // Normieren und an die naechsten 2×2 (f, g, j, k) klemmen —
            // das ist der Dering-Schutz gegen die negativen Loben.
            let min4 = [0, 1, 2].map(|ch| fc[ch].min(g[ch]).min(j[ch]).min(k[ch]));
            let max4 = [0, 1, 2].map(|ch| fc[ch].max(g[ch]).max(j[ch]).max(k[ch]));
            out[dy * dw + dx] =
                [0, 1, 2].map(|ch| max4[ch].min(min4[ch].max(aC[ch] / aW)));
        }
    }
    out
}

/// RCAS — Robust Contrast Adaptive Sharpening. Nicht-skaliertes
/// Nachschaerfen nach EASU: der Lobe wird je Pixel so weit geoeffnet,
/// wie das Ergebnis ohne Clipping aushaelt (`FsrRcasF`).
///
/// `sharpness_stops` ist AMDs Blendentreppe: 0 = schaerfste Einstellung,
/// jede weitere Blende halbiert (`linear = 2^-stops`). Die Rausch-
/// Unterdrueckung (`FSR_RCAS_DENOISE`) ist wie in den meisten
/// Integrationen deaktiviert — Filmkorn gehört hinter das Schärfen.
// lobeM/lobeF/rcpL spiegeln die Original-Namen aus FsrRcasF.
#[allow(non_snake_case)]
pub fn rcas(src: &[[f32; 3]], w: usize, h: usize, sharpness_stops: f32) -> Vec<[f32; 3]> {
    assert!(w > 0 && h > 0, "RCAS: leeres Bild");
    assert_eq!(src.len(), w * h, "RCAS: Quelldaten passen nicht zur Groesse");
    // FsrRcasCon: Blenden -> linear. Dieser Wert geht auch als Uniform
    // auf die GPU, damit beide Seiten denselben Konstanten nutzen.
    let sharp = (-sharpness_stops).exp2();
    let load = |x: i64, y: i64| -> [f32; 3] {
        let x = x.clamp(0, w as i64 - 1) as usize;
        let y = y.clamp(0, h as i64 - 1) as usize;
        src[y * w + x]
    };
    let mut out = vec![[0.0f32; 3]; w * h];
    for y in 0..h {
        for x in 0..w {
            let (xi, yi) = (x as i64, y as i64);
            // 5-Punkt-Kreuz:    b
            //                 d  e  f
            //                    h
            let b = load(xi, yi - 1);
            let d = load(xi - 1, yi);
            let e = load(xi, yi);
            let f = load(xi + 1, yi);
            let hh = load(xi, yi + 1);
            // Ring-Min/Max (Reihenfolge wie im Original: min(min(min(b,d),f),h)).
            let mn4 = [0, 1, 2].map(|ch| b[ch].min(d[ch]).min(f[ch]).min(hh[ch]));
            let mx4 = [0, 1, 2].map(|ch| b[ch].max(d[ch]).max(f[ch]).max(hh[ch]));
            // Begrenzer: wie weit darf der Lobe gehen, ohne dass das
            // Ergebnis 0..1 verlaesst? (Die Nenner sind nur bei exakt
            // schwarz/weiß kritisch — Epsilon-Klemme statt inf/NaN.)
            let mut lobe = [0.0f32; 3];
            for ch in 0..3 {
                let hit_min = mn4[ch].min(e[ch]) * (1.0 / (4.0 * mx4[ch]).max(1e-30));
                let hit_max = (1.0 - mx4[ch].max(e[ch])) * (1.0 / (4.0 * mn4[ch] + (-4.0)).min(-1e-30));
                lobe[ch] = (-hit_min).max(hit_max);
            }
            let lobeM = lobe[0].max(lobe[1]).max(lobe[2]);
            // FSR_RCAS_LIMIT = 0.25 - 1/16 = 0.1875 — die harte Obergrenze
            // des Schaerfens (Mehr waere unnatuerlich).
            let lobeF = (-0.1875f32).max(lobeM.min(0.0)) * sharp;
            let rcpL = 1.0 / (4.0 * lobeF + 1.0);
            // Aufloesung: (lobe·Ring + Mitte)/(4·lobe+1) — Energieerhaltend.
            out[y * w + x] = [0, 1, 2].map(|ch| {
                (lobeF * b[ch] + lobeF * d[ch] + lobeF * hh[ch] + lobeF * f[ch] + e[ch]) * rcpL
            });
        }
    }
    out
}

/// Die ganze FSR1-Kette: EASU (skaliert) dann RCAS (schaerft) — der
/// Upscale-Pfad des Pi-Profils. Farbeingabe display-kodiert in 0..1.
/// Zwischen beiden liegt dieselbe 8-Bit-Quantisierung wie im
/// GPU-Zwischenziel (EASU schreibt Rgba8Unorm, RCAS liest es zurueck) —
/// ohne sie vergliche man zwei verschiedene Bilder.
pub fn fsr1(
    src: &[[f32; 3]],
    sw: usize,
    sh: usize,
    dw: usize,
    dh: usize,
    sharpness_stops: f32,
) -> Vec<[f32; 3]> {
    let up = easu(src, sw, sh, dw, dh);
    let q: Vec<[f32; 3]> = up
        .iter()
        .map(|p| [0, 1, 2].map(|k| (p[k] * 255.0).round() / 255.0))
        .collect();
    rcas(&q, dw, dh, sharpness_stops)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn konst(w: usize, h: usize, c: [f32; 3]) -> Vec<[f32; 3]> {
        vec![c; w * h]
    }

    /// Vertikale Stufenkante bei x = w/2, Werte a|b.
    fn stufe(w: usize, h: usize, a: f32, b: f32) -> Vec<[f32; 3]> {
        (0..h)
            .flat_map(|_| {
                (0..w).map(move |x| {
                    let v = if x < w / 2 { a } else { b };
                    [v, v, v]
                })
            })
            .collect()
    }

    #[test]
    fn easu_haelt_konstantes_bild_stabil() {
        let src = konst(8, 8, [0.4, 0.5, 0.6]);
        let out = easu(&src, 8, 8, 16, 16);
        assert_eq!(out.len(), 16 * 16);
        for (i, p) in out.iter().enumerate() {
            for ch in 0..3 {
                assert!(
                    (p[ch] - src[i % 64][ch]).abs() < 1e-5,
                    "Pixel {i} Kanal {ch}: {p:?} driftet vom konstanten Input ab"
                );
            }
        }
    }

    #[test]
    fn easu_klemmt_gegen_ringing_innen_min_max() {
        // Treppenszene mit hartem Kontrast: Lanczos hat negative Loben —
        // der Dering-Schutz muss jeden Ausgabewert innerhalb des globalen
        // Wertebereichs der Quelle halten.
        let src = stufe(16, 16, 0.0, 1.0);
        let out = easu(&src, 16, 16, 32, 32);
        for (i, p) in out.iter().enumerate() {
            for ch in 0..3 {
                assert!(
                    (0.0..=1.0).contains(&p[ch]),
                    "Pixel {i} Kanal {ch}: {p:?} ueberschreitet den Quellbereich (Ringing)"
                );
            }
        }
    }

    #[test]
    fn easu_kantenuergang_ist_hoechstens_bilinear_breit() {
        // Kante bei x=8 (16->32, 2x): Bilinear übergeht 2 Display-Pixel
        // je Seite; EASU darf nicht breiter uebergehen (sonst waere es
        // nur Brei) und muss monoton bleiben.
        let src = stufe(16, 16, 0.0, 1.0);
        let out = easu(&src, 16, 16, 32, 32);
        let zeile = 17; // y=17 -> pp.y = 17*0.5-0.5 = 8.0 -> mitten in einer Quellzeile
        let mut letzter = -1.0;
        let mut rampen = 0;
        for x in 0..32 {
            let v = out[zeile * 32 + x][0];
            assert!(v >= letzter - 1e-6, "nicht monoton an der Kante bei x={x}");
            letzter = v;
            if v > 0.01 && v < 0.99 {
                rampen += 1;
            }
        }
        assert!(rampen <= 4, "Kantenuebergang {rampen} Pixel breit — breiter als Bilinear?");
    }

    #[test]
    fn rcas_schaerft_kante_ohne_clipping() {
        // 0.25/0.75-Stufe: RCAS darf die Flanken nicht antasten, muss die
        // Kante aber anheben/absenken und bleibt dabei in 0..1.
        let src = stufe(16, 16, 0.25, 0.75);
        let out = rcas(&src, 16, 16, 0.0);
        // Innenbereich unveraendert (Ring einheitlich -> lobe 0).
        assert!((out[8 * 16 + 2][0] - 0.25).abs() < 1e-5, "flach links");
        assert!((out[8 * 16 + 13][0] - 0.75).abs() < 1e-5, "flach rechts");
        // Direkt an der Kante: dunkle Seite dunkler, helle Seite heller.
        assert!(out[8 * 16 + 7][0] < 0.25, "dunkle Flanke muss absinken: {}", out[8 * 16 + 7][0]);
        assert!(out[8 * 16 + 8][0] > 0.75, "helle Flanke muss steigen: {}", out[8 * 16 + 8][0]);
        for p in &out {
            for ch in 0..3 {
                assert!((-1e-6..=1.0 + 1e-6).contains(&p[ch]), "Clipping bei {p:?}");
            }
        }
    }

    #[test]
    fn rcas_laesst_konstantes_bild_in_ruhe() {
        let src = konst(8, 8, [0.3, 0.5, 0.9]);
        let out = rcas(&src, 8, 8, 0.0);
        for (i, p) in out.iter().enumerate() {
            for ch in 0..3 {
                assert!((p[ch] - src[i][ch]).abs() < 1e-5, "Pixel {i}: {p:?}");
            }
        }
    }

    #[test]
    fn blenden_treppe_halbiert_pro_stufe() {
        // linear = 2^-stops: 0 -> 1, 1 -> 0.5, 2 -> 0.25.
        assert!(((-0.0f32).exp2() - 1.0).abs() < 1e-7);
        assert!(((-1.0f32).exp2() - 0.5).abs() < 1e-7);
        assert!(((-2.0f32).exp2() - 0.25).abs() < 1e-7);
        // Und die Wirkung: hoehere Blende schaerft weniger als niedrige.
        let src = stufe(16, 16, 0.25, 0.75);
        let scharf = rcas(&src, 16, 16, 0.0);
        let milde = rcas(&src, 16, 16, 2.0);
        assert!(
            (scharf[8 * 16 + 7][0] - 0.25).abs() > (milde[8 * 16 + 7][0] - 0.25).abs(),
            "Blende 0 muss staerker schaerfen als Blende 2"
        );
    }

    #[test]
    fn kette_ist_deterministisch() {
        // Pseudo-zufaellige Szene durch die ganze Kette — zweimal identisch.
        let src: Vec<[f32; 3]> = (0..12 * 12)
            .map(|i| {
                let v = |k: usize| (((i * 37 + k * 91) % 255) as f32) / 255.0;
                [v(0), v(1), v(2)]
            })
            .collect();
        let a = fsr1(&src, 12, 12, 24, 24, 2.0);
        let b = fsr1(&src, 12, 12, 24, 24, 2.0);
        assert_eq!(a, b, "gleiche Eingabe -> gleiche Ausgabe");
        assert_eq!(a.len(), 24 * 24);
    }
}
