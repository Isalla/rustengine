//! ETC2/EAC-Decoder — das Pi-Zielformat, nativ in KTX2.
//!
//! Umsetzung nach der veröffentlichten ETC2/EAC-Spezifikation
//! (OpenGL ES 3.x, Anhang C: Blocklayouts, Modi, Modifikatoren).
//! Bitpositionen (gepackte Anordnung inkl. der Redundanz-Kopien der
//! T-/H-/Planar-Modi), Modi-Diskriminierer (Overflow-Test über R, G, B)
//! und Formeln wurden gegen drei unabhängige Open-Source-Dekoder
//! abgeglichen: ANGLE (`loadimage_etc.cpp`, BSD-3), SwiftShader
//! (`etcdec.cxx`, Apache-2.0) und AssetStudio (`etc.cpp`, MIT) —
//! Attribution siehe `THIRD_PARTY_NOTICES.md`.
//!
//! Kurzüberblick: 4×4-Blöcke, 64 Bit für RGB8/RGB8A1/EAC_R11, 128 Bit
//! für RGBA8 (Alpha-Wort zuerst) und EAC_RG11 (Rot-Wort zuerst).
//! Farbmodi: Einzel (zwei Subset-Farben 4-4-4, nur RGB8, Bit 33 = 0),
//! Differenzial (5-5-5 mit Delta), T- und H-Modus (zwei 4-4-4-Farben
//! mit Distanz-Palette), Planar (Gradient aus drei Farben O/H/V). Der
//! Discriminator prüft bei Diff-Bit (Bit 33 = 1, bei RGB8A1 immer), ob
//! Basis+Delta aus [0,31] läuft: R→T, G→H, B→Planar, sonst
//! Differenzial. EAC: Codewort + Modifier-Tabelle; RGBA8-Alpha rechnet
//! `Basis + Mod × Multiplikator` direkt (Multiplikator 0 → Basis),
//! R11/RG11 über den 11-Bit-Umweg (×8, Rundung +4, Multiplikator 0
//! zählt als 1) und schiebt auf 8 Bit.

use anyhow::{Result, bail};

/// Distanztabelle der T-/H-Modi (Spezifikation, Tabelle C.8).
const DIST: [i32; 8] = [3, 6, 11, 16, 23, 32, 41, 64];

/// Intensitäts-Modifier für Einzel/Differenzial (Tabelle C.6/C.7);
/// Index = Tabellenzeile (3 Bit), dann Pixelindex msb:lsb =
/// [+klein, +groß, −klein, −groß].
#[rustfmt::skip]
const MOD: [[i32; 4]; 8] = [
    [2, 8, -2, -8],
    [5, 17, -5, -17],
    [9, 29, -9, -29],
    [13, 42, -13, -42],
    [18, 60, -18, -60],
    [24, 80, -24, -80],
    [33, 106, -33, -106],
    [47, 183, -47, -183],
];

/// Modifier im nicht-opaken RGB8A1-Pfad (C.1.9): Index 0 = unverändert,
/// 1 = +groß, 2 = stanzen, 3 = −groß.
#[rustfmt::skip]
const MOD_NO: [[i32; 4]; 8] = [
    [0, 8, 0, -8],
    [0, 17, 0, -17],
    [0, 29, 0, -29],
    [0, 42, 0, -42],
    [0, 60, 0, -60],
    [0, 80, 0, -80],
    [0, 106, 0, -106],
    [0, 183, 0, -183],
];

/// Modifier der EAC-Tabellen (Tabelle C.10); Zeile = 4-Bit-Tabellen-
/// index, Spalte = 3-Bit-Pixelindex.
#[rustfmt::skip]
const AMOD: [[i32; 8]; 16] = [
    [-3, -6,  -9, -15, 2, 5, 8, 14],
    [-3, -7, -10, -13, 2, 6, 9, 12],
    [-2, -5,  -8, -13, 1, 4, 7, 12],
    [-2, -4,  -6, -13, 1, 3, 5, 12],
    [-3, -6,  -8, -12, 2, 5, 7, 11],
    [-3, -7,  -9, -11, 2, 6, 8, 10],
    [-4, -7,  -8, -11, 3, 6, 7, 10],
    [-3, -5,  -8, -11, 2, 4, 7, 10],
    [-2, -6,  -8, -10, 1, 5, 7,  9],
    [-2, -5,  -8, -10, 1, 4, 7,  9],
    [-2, -4,  -8, -10, 1, 3, 7,  9],
    [-2, -5,  -7, -10, 1, 4, 6,  9],
    [-3, -4,  -7, -10, 2, 3, 6,  9],
    [-1, -2,  -3, -10, 0, 1, 2,  9],
    [-4, -6,  -8,  -9, 3, 5, 7,  8],
    [-3, -5,  -7,  -9, 2, 4, 6,  8],
];

/// Bitreplikation n→8 (C.2.4/C.2.8).
fn e4(v: i32) -> i32 {
    (v << 4) | v
}
fn e5(v: i32) -> i32 {
    (v << 3) | (v >> 2)
}
fn e6(v: i32) -> i32 {
    (v << 2) | (v >> 4)
}
fn e7(v: i32) -> i32 {
    (v << 1) | (v >> 6)
}
fn c8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}
fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes(b[..4].try_into().unwrap())
}
fn be64(b: &[u8]) -> u64 {
    ((be32(b) as u64) << 32) | be32(&b[4..]) as u64
}

/// Feld ab Bitposition `hi` (MSB) mit `n` Bits, Zählung von oben
/// (Bit 63 = MSB des Big-Endian-Blockworts).
fn b(v: u64, hi: u32, n: u32) -> i32 {
    ((v >> (hi + 1 - n)) & ((1u64 << n) - 1)) as i32
}
/// Drei-Bit-Delta vorzeichenbehaftet.
fn s3(v: i32) -> i32 {
    (v << 29) >> 29
}

/// 2-Bit-Pixelindex (msb:lsb) für Pixel n = Spalte*4 + Zeile: MSB an
/// Bit 16+n, LSB an Bit n des Blockworts (Tabelle C.2/C.3f; das
/// Spezifikationsbeispiel „Pixel d → Bits 19 und 3“ bestätigt die
/// spaltenweise Nummerierung).
fn index2(v: u64, col: usize, row: usize) -> usize {
    let n = (col * 4 + row) as u32;
    ((((v >> (16 + n)) & 1) << 1) | ((v >> n) & 1)) as usize
}

/// Einzel-/Differenzial-Farbbelegung: Flip teilt in zwei Subsets (Flip
/// 0 = links/rechts, Flip 1 = oben/unten), der Pixelindex wählt den
/// Modifier; bei nicht-opaken A1-Blöcken stanzt Index 2.
fn subset_paint(
    v: u64,
    cs: [[i32; 3]; 2],
    out: &mut [[u8; 3]; 16],
    hole: &mut [bool; 16],
    no: bool,
) {
    let t = [b(v, 39, 3) as usize, b(v, 36, 3) as usize];
    let flip = b(v, 32, 1) == 1;
    let tables = if no { &MOD_NO } else { &MOD };
    for (n, px) in out.iter_mut().enumerate() {
        let (col, row) = (n / 4, n % 4);
        let sb = if flip { (row >= 2) as usize } else { (col >= 2) as usize };
        let sel = index2(v, col, row);
        if no && sel == 2 {
            hole[n] = true;
            *px = [0, 0, 0];
            continue;
        }
        let m = tables[t[sb]][sel];
        *px = [c8(cs[sb][0] + m), c8(cs[sb][1] + m), c8(cs[sb][2] + m)];
    }
}

/// Palette der Modi T und H; Index 2 stanzt bei nicht-opaken Blöcken
/// (in T = Farbe c1, in H = c1+d — dieselbe Stelle im Farbsatz).
fn palette_paint(
    v: u64,
    pal: [[i32; 3]; 4],
    out: &mut [[u8; 3]; 16],
    hole: &mut [bool; 16],
    no: bool,
) {
    for (n, px) in out.iter_mut().enumerate() {
        let (col, row) = (n / 4, n % 4);
        let sel = index2(v, col, row);
        if no && sel == 2 {
            hole[n] = true;
            *px = [0, 0, 0];
            continue;
        }
        *px = [c8(pal[sel][0]), c8(pal[sel][1]), c8(pal[sel][2])];
    }
}

/// T-Modus (C.1.10): Palette {c0, c1+d, c1, c1−d}. Die Felder sitzen
/// gepackt mit Redundanzkopien: R0 = Bits 60,59,57,56; G0 55..52;
/// B0 51..48; R1 47..44; G1 43..40; B1 39..36; da = 35..34, db = 32
/// (Bit 33 ist bei A1 das Opaken-Bit und wird übersprungen).
fn t_mode(v: u64, out: &mut [[u8; 3]; 16], hole: &mut [bool; 16], no: bool) {
    let c0 = [e4(b(v, 60, 2) << 2 | b(v, 57, 2)), e4(b(v, 55, 4)), e4(b(v, 51, 4))];
    let c1 = [e4(b(v, 47, 4)), e4(b(v, 43, 4)), e4(b(v, 39, 4))];
    let d = DIST[((b(v, 35, 2) << 1) | b(v, 32, 1)) as usize];
    let pal = [c0, c1.map(|x| x + d), c1, c1.map(|x| x - d)];
    palette_paint(v, pal, out, hole, no);
}

/// H-Modus (C.1.11): Palette {c0+d, c0−d, c1+d, c1−d}. Gepackte
/// Felder: R0 = 62..59; G0 = 58,57,56,52; B0 = 51,50,49,47;
/// R1 = 46..43; G1 = 42,41,40,39; B1 = 38..35; Distanzindex =
/// (Bit 34 << 2) | (Bit 32 << 1) | Trickbit, Trickbit = lexikaler
/// Farbvergleich c0 ≥ c1.
fn h_mode(v: u64, out: &mut [[u8; 3]; 16], hole: &mut [bool; 16], no: bool) {
    let c0 = [
        e4(b(v, 62, 4)),
        e4(b(v, 58, 3) << 1 | b(v, 52, 1)),
        e4(b(v, 51, 3) << 1 | b(v, 47, 1)),
    ];
    let c1 = [
        e4(b(v, 46, 4)),
        e4(b(v, 42, 3) << 1 | b(v, 39, 1)),
        e4(b(v, 38, 4)),
    ];
    let trick = (c0[0] > c1[0]
        || (c0[0] == c1[0] && (c0[1] > c1[1] || (c0[1] == c1[1] && c0[2] >= c1[2]))))
        as i32;
    let d = DIST[((b(v, 34, 1) << 2) | (b(v, 32, 1) << 1) | trick) as usize];
    let pal = [
        c0.map(|x| x + d),
        c0.map(|x| x - d),
        c1.map(|x| x + d),
        c1.map(|x| x - d),
    ];
    palette_paint(v, pal, out, hole, no);
}

/// Planar-Modus (C.1.12): drei Farben O/H/V in 6-7-6, Gradientformel
/// C.2.12; immer opak. Gepackte Felder mit Kopien: RO 62..57;
/// GO 56|54..49; BO 48|44..43|41..39; RH 38..34|32; GH 31..25;
/// BH 24..19; RV 18..13; GV 12..6; BV 5..0. (Ein echter Planar-Block
/// hat in den Kopie-Bits 47..45 Einsen, die den B-Überlauf auslösen —
/// sonst wäre es kein Planar-Block.)
fn p_mode(v: u64, out: &mut [[u8; 3]; 16]) {
    let ro = e6(b(v, 62, 6));
    let go = e7(b(v, 56, 1) << 6 | b(v, 54, 6));
    let bo = e6(b(v, 48, 1) << 5 | b(v, 44, 2) << 3 | b(v, 41, 3));
    let rh = e6(b(v, 38, 5) << 1 | b(v, 32, 1));
    let gh = e7(b(v, 31, 7));
    let bh = e6(b(v, 24, 6));
    let rv = e6(b(v, 18, 6));
    let gv = e7(b(v, 12, 7));
    let bv = e6(b(v, 5, 6));
    for (n, px) in out.iter_mut().enumerate() {
        let (x, y) = (n / 4, n % 4);
        px[0] = c8((x as i32 * (rh - ro) + y as i32 * (rv - ro) + 4 * ro + 2) >> 2);
        px[1] = c8((x as i32 * (gh - go) + y as i32 * (gv - go) + 4 * go + 2) >> 2);
        px[2] = c8((x as i32 * (bh - bo) + y as i32 * (bv - bo) + 4 * bo + 2) >> 2);
    }
}

/// Ein ETC2-Farbblock (RGB8/RGB8A1/Farbwort von RGBA8) aus 64 Bit.
/// `punch = Some(op)` für RGB8A1: Bit 33 ist dort das Opaken-Bit, die
/// Modi-Wahl erfolgt immer per Overflow-Test (C.1.9). `None` für
/// RGB-Farbblöcke: Bit 33 = Differenzial-Bit, 0 → Einzel-Modus.
fn color_block(v: u64, punch: Option<bool>, out: &mut [[u8; 3]; 16], hole: &mut [bool; 16]) {
    hole.fill(false);
    let no = punch == Some(false);
    let r5 = b(v, 63, 5);
    let g5 = b(v, 55, 5);
    let b5 = b(v, 47, 5);
    let dr = s3(b(v, 58, 3));
    let dg = s3(b(v, 50, 3));
    let db = s3(b(v, 42, 3));
    if punch.is_none() && b(v, 33, 1) == 0 {
        // Einzel-Modus: Subblock-Farben je 4 Bit (Tabelle C.3c).
        let cs = [
            [e4(b(v, 63, 4)), e4(b(v, 55, 4)), e4(b(v, 47, 4))],
            [e4(b(v, 59, 4)), e4(b(v, 51, 4)), e4(b(v, 43, 4))],
        ];
        subset_paint(v, cs, out, hole, no);
        return;
    }
    if r5 + dr < 0 || r5 + dr > 31 {
        t_mode(v, out, hole, no);
    } else if g5 + dg < 0 || g5 + dg > 31 {
        h_mode(v, out, hole, no);
    } else if b5 + db < 0 || b5 + db > 31 {
        p_mode(v, out);
    } else {
        let cs = [
            [e5(r5), e5(g5), e5(b5)],
            [e5(r5 + dr), e5(g5 + dg), e5(b5 + db)],
        ];
        subset_paint(v, cs, out, hole, no);
    }
}

/// Ein EAC-Wort (8 Bytes): Codewort (Bits 63..56), Multiplikator
/// (55..52), Tabellenindex (51..48), 16 Dreier-Pixelindizes ab Bit 47,
/// Pixel n an Bits 47−3n..45−3n. `alpha8` = RGBA8-Alpha: direkt
/// `Basis + Mod × Multiplikator` (Multiplikator 0 → Basis). Sonst
/// 11-Bit-Umweg `Basis×8 + Mod×(×8) + 4` (bzw. +1023 bei `signed`,
/// Basis dann vorzeichenbehaftet), clamp [0,2047], rechts 3.
fn eac_word(d: &[u8], alpha8: bool, signed: bool) -> [u8; 16] {
    let cw = if signed { (d[0] as i8) as i32 } else { d[0] as i32 };
    let mult = (d[1] >> 4) as i32;
    let tab = (d[1] & 0xF) as usize;
    let v = be64(d);
    let mut o = [0u8; 16];
    if alpha8 && mult == 0 {
        o.fill(cw as u8);
        return o;
    }
    // Multiplikator 0 zählt im 11-Bit-Pfad als 1 (ohne ×8).
    let m8 = if mult == 0 { 1 } else { mult * 8 };
    let off = if signed { 1023 } else { 4 };
    for (n, byte) in o.iter_mut().enumerate() {
        let m = AMOD[tab][((v >> (45 - 3 * n as u32)) & 7) as usize];
        *byte = if alpha8 {
            c8(cw + m * mult)
        } else {
            ((cw * 8 + m * m8 + off).clamp(0, 2047) >> 3) as u8
        };
    }
    o
}

/// Dekodiere alle Blöcke einer KTX2-Stufe nach RGBA8. `vk` ist die
/// VkFormat-Nummer; nur die unären ETC2/EAC-Formate (147–153, 155) —
/// SNORM-Varianten und ASTC lehnt der Aufrufer (`tex::ktx2`) vorher ab.
pub(super) fn decode_blocks(
    vk: u32,
    b: &[u8],
    off: usize,
    w: u32,
    h: u32,
    rgba: &mut [u8],
) -> Result<()> {
    let (bs, kind) = match vk {
        147 | 148 => (8, 0u8), // RGB8
        149 | 150 => (8, 1),   // RGB8A1 (Stanzen)
        151 | 152 => (16, 2),  // RGBA8 (EAC-Alpha-Wort + Farbwort)
        153 => (8, 3),         // EAC_R11 → Graustufen
        155 => (16, 4),        // EAC_RG11 → Rot/Grün
        other => bail!("etc2: VkFormat {other} hat keinen ETC2/EAC-Pfad"),
    };
    let (bx, by) = ((w as usize).div_ceil(4), (h as usize).div_ceil(4));
    if b.len() < off + bx * by * bs || rgba.len() < (w as usize) * (h as usize) * 4 {
        bail!("etc2: Stufendaten abgeschnitten");
    }
    let mut px = [[0u8; 3]; 16];
    let mut hole = [false; 16];
    let mut al = [255u8; 16];
    for y in 0..by {
        for x in 0..bx {
            let d = &b[off + (y * bx + x) * bs..];
            px.iter_mut().for_each(|p| *p = [0, 0, 0]);
            hole.fill(false);
            al.fill(255);
            match kind {
                0 => color_block(be64(d), None, &mut px, &mut hole),
                1 => {
                    // Farbwort mit Opaken-Bit an Stelle des Diff-Bits.
                    let op = (d[3] >> 1) & 1 == 1;
                    color_block(be64(d), Some(op), &mut px, &mut hole);
                }
                2 => {
                    color_block(be64(&d[8..]), None, &mut px, &mut hole);
                    al = eac_word(d, true, false);
                }
                3 => {
                    let r = eac_word(d, false, false);
                    for (n, p) in px.iter_mut().enumerate() {
                        *p = [r[n], r[n], r[n]];
                    }
                }
                _ => {
                    // EAC_RG11: erstes Wort = Rot, zweites = Grün (C.1.5).
                    let r = eac_word(d, false, false);
                    let g = eac_word(&d[8..], false, false);
                    for (n, p) in px.iter_mut().enumerate() {
                        *p = [r[n], g[n], 0];
                    }
                }
            }
            for row in 0..4 {
                for col in 0..4 {
                    let (gx, gy) = (x * 4 + col, y * 4 + row);
                    if gx >= w as usize || gy >= h as usize {
                        continue;
                    }
                    // Pixelnummerierung des Blocklayouts: n = col*4 + row.
                    let n = col * 4 + row;
                    let o = (gy * w as usize + gx) * 4;
                    let a = if hole[n] { 0 } else { al[n] };
                    rgba[o..o + 4].copy_from_slice(&[px[n][0], px[n][1], px[n][2], a]);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blk(w1: u32, w2: u32) -> [u8; 8] {
        let mut o = [0u8; 8];
        o[..4].copy_from_slice(&w1.to_be_bytes());
        o[4..].copy_from_slice(&w2.to_be_bytes());
        o
    }

    /// Differenzial-Grundblock: R=21 G=11 B=9, dR=+2 dG=−1 dB=−3,
    /// Tabellen 4/5, Flip 0, Diff-Bit (33) gesetzt.
    fn diff_w1() -> u32 {
        (21 << 27)
            | (2 << 24)
            | (11 << 19)
            | (7 << 16)
            | (9 << 11)
            | (5 << 8)
            | (4 << 5)
            | (5 << 2)
            | (1 << 1)
    }

    #[test]
    fn differential_basis_und_delta() {
        // Alle Pixelindex-Paare 00 (= +klein, Tab 4 → +18 / Tab 5 → +24).
        let mut px = [[0u8; 3]; 16];
        let mut hole = [false; 16];
        color_block((diff_w1() as u64) << 32, None, &mut px, &mut hole);
        let c0 = [e5(21), e5(11), e5(9)];
        let c1 = [e5(23), e5(10), e5(6)];
        for n in 0..16 {
            let col = n / 4;
            let (base, t) = if col < 2 { (c0, MOD[4][0]) } else { (c1, MOD[5][0]) };
            assert_eq!(px[n], [c8(base[0] + t), c8(base[1] + t), c8(base[2] + t)], "px {n}");
        }
        assert!(!hole.iter().any(|h| *h));
    }

    #[test]
    fn einzel_modus_und_flip() {
        // Bit 33 = 0 → Einzel. Subblock-Farben: R1=63..60, R2=59..56,
        // G1=55..52, G2=51..48, B1=47..44, B2=43..40. Flip 1 → oben/unten.
        // Tab1 (39..37) = 0, Tab2 (36..34) = 7; alle Indizes 01 (= +groß).
        let v: u64 = (0xFu64 << 60)
            | (0x1 << 56)
            | (0x7 << 52)
            | (0x5 << 48)
            | (0xA << 44)
            | (0x4 << 40)
            | (0 << 37)
            | (7u64 << 34)
            | (1u64 << 32)
            | 0x0000_FFFF; // LSB-Hälfte 1, MSB-Hälfte 0 → Index 1
        let mut px = [[0u8; 3]; 16];
        let mut hole = [false; 16];
        color_block(v, None, &mut px, &mut hole);
        for n in 0..16 {
            let row = n % 4;
            // Flip 1: rows 0–1 = Subblock 1 (Tab 0), rows 2–3 = Subblock 2.
            let (base, t) = if row < 2 {
                ([0xFF, 0x77, 0xAA], MOD[0][1])
            } else {
                ([0x11, 0x55, 0x44], MOD[7][1])
            };
            assert_eq!(px[n], [c8(base[0] + t), c8(base[1] + t), c8(base[2] + t)], "px {n}");
        }
    }

    #[test]
    fn t_modus_palette() {
        // Overflow erzwingen: R5 = 31 (Bits 63..59), dR = +1 (58..56);
        // Diff-Bit 33 = 1 (sonst Einzel-Modus). R0 = [60,59,57,56] = 13.
        let v: u64 = (31u64 << 59)
            | (1u64 << 56)
            | (1u64 << 52)
            | (7u64 << 48)
            | (5u64 << 44)
            | (0xAu64 << 40)
            | (4u64 << 36)
            | (3u64 << 34)
            | (1u64 << 33)
            | (1u64 << 32);
        let mut px = [[0u8; 3]; 16];
        let mut hole = [false; 16];
        color_block(v, None, &mut px, &mut hole);
        let d = DIST[7]; // (3 << 1) | 1 = 7 → 64
        assert_eq!(d, 64);
        let c0 = [e4(13), e4(1), e4(7)];
        let c1 = [e4(5), e4(0xA), e4(4)];
        // Alle Indizes 00 → Palette 0.
        for p in &px {
            assert_eq!(*p, [c8(c0[0]), c8(c0[1]), c8(c0[2])]);
        }
        // Palette 1 (Index 01): LSB-Hälfte setzen.
        let mut px1 = [[0u8; 3]; 16];
        color_block(v | 0xFFFF, None, &mut px1, &mut hole);
        for p in &px1 {
            assert_eq!(*p, [c8(c1[0] + d), c8(c1[1] + d), c8(c1[2] + d)]);
        }
        // Palette 2 (Index 10) = c1.
        let mut px2 = [[0u8; 3]; 16];
        color_block(v | 0xFFFF_0000, None, &mut px2, &mut hole);
        for p in &px2 {
            assert_eq!(*p, [c8(c1[0]), c8(c1[1]), c8(c1[2])]);
        }
        // Palette 3 (Index 11).
        let mut px3 = [[0u8; 3]; 16];
        color_block(v | 0xFFFF_FFFF, None, &mut px3, &mut hole);
        for p in &px3 {
            assert_eq!(*p, [c8(c1[0] - d), c8(c1[1] - d), c8(c1[2] - d)]);
        }
    }

    #[test]
    fn h_modus_und_trickbit() {
        // R-Test muss durchgehen, G-Test überlaufen:
        // R5 (63..59) = 0b01010 → Bit 63 = 0, R0 = Bits 62..59 = 10.
        // dR (58..56) = 0b011 → +3 → 13 ok; zugleich G0-Bits 58,57,56.
        // G5 (55..51) = 11111 → Bit 52 (G0-Vierener) = 1; dG (50..48) = 001.
        let v: u64 = (0b01010u64 << 59)
            | (0b011u64 << 56)
            | (0b11111u64 << 51)
            | (0b001u64 << 48)
            | (0b1 << 47) // B0-Bit 47; B5-Bits 47..43 Rest 0
            | (1u64 << 39) // G1-Vierentes Bit 39
            | (0b1100u64 << 35) // B1 = Bits 38..35
            | (1u64 << 34) // da = 1
            | (1u64 << 33) // Diff-Bit (RGB8: T/H/P nur bei Diff = 1)
            | (0u64 << 32); // db = 0
        // Rekonstruierte Felder laut H-Layout:
        // R0 = [62..59] = 10; G0 = [58,57,56,52] = 0,1,1,1 = 7;
        // B0 = [51,50,49,47] = 1,0,0,1 = 9; R1 = [46..43] = 0;
        // G1 = [42,41,40,39] = 0,0,0,1 = 1 (dB-Reste ungenutzt);
        // B1 = [38..35] = 0b1100 = 12.
        let c0 = [e4(0b1010), e4(0b0111), e4(0b1001)];
        let c1 = [e4(0b0000), e4(0b0001), e4(0b1100)];
        assert!(c0[0] > c1[0]); // Trickbit = 1
        let d = DIST[(1 << 2) | (0 << 1) | 1]; // → Index 5 → 32
        assert_eq!(d, 32);
        let mut px = [[0u8; 3]; 16];
        let mut hole = [false; 16];
        color_block(v, None, &mut px, &mut hole);
        // Index 00 → c0+d.
        for p in &px {
            assert_eq!(*p, [c8(c0[0] + d), c8(c0[1] + d), c8(c0[2] + d)]);
        }
        // Index 01 → c0−d (LSB-Hälfte setzen).
        let mut px1 = [[0u8; 3]; 16];
        color_block(v | 0xFFFF, None, &mut px1, &mut hole);
        for p in &px1 {
            assert_eq!(*p, [c8(c0[0] - d), c8(c0[1] - d), c8(c0[2] - d)]);
        }
        // Index 10 → c1+d.
        let mut px2 = [[0u8; 3]; 16];
        color_block(v | 0xFFFF_0000, None, &mut px2, &mut hole);
        for p in &px2 {
            assert_eq!(*p, [c8(c1[0] + d), c8(c1[1] + d), c8(c1[2] + d)]);
        }
    }

    #[test]
    fn planar_gleichfarben() {
        // O = H = V → flacher Block; Felder mit Lücken (GO, BO) prüfen.
        // RO/RH/RV = 20, GO/GH/GV = 30, BO/BH/BV = 46. Die Kopie-Bits
        // 47..45 = 111 und dB-Bit 42 = 0 erzwingen den B-Überlauf
        // (B5 = 29, dB = +3 → 32), der den Planar-Modus ausgewählt hat.
        let set = |v: u64, hi: u32, n: u32, val: u64| v | (val << (hi + 1 - n));
        let mut v = 1u64 << 33; // Diff-Bit
        v = set(v, 62, 6, 20); // RO
        v = set(v, 54, 6, 30 & 0b11_1111); // GO untere 6 (MSB an 56 = 0)
        v = set(v, 48, 1, 46 >> 5); // BO1
        v |= 0b111u64 << 45; // Kopie-Bits (B5 = 29)
        v = set(v, 44, 2, (46 >> 3) & 3); // BO2
        v = set(v, 41, 3, 46 & 7); // BO3 (Bits 41..39)
        v = set(v, 38, 5, 20 >> 1); // RH Bits 38..34
        v = set(v, 32, 1, 20 & 1); // RH LSB an 32
        v = set(v, 31, 7, 30); // GH
        v = set(v, 24, 6, 46); // BH 24..19
        v = set(v, 18, 6, 20); // RV
        v = set(v, 12, 7, 30); // GV
        v = set(v, 5, 6, 46); // BV
        let mut px = [[0u8; 3]; 16];
        let mut hole = [false; 16];
        color_block(v, None, &mut px, &mut hole);
        let want = [c8(e6(20)), c8(e7(30)), c8(e6(46))];
        for p in &px {
            assert_eq!(*p, want);
        }
        assert!(!hole.iter().any(|h| *h));
    }

    #[test]
    fn a1_stanzt_nur_nicht_opak() {
        // Differenzial-Block mit Pixel n = 3 (col 0, row 3) auf Index 10
        // (= Palette/Modifier 2 → Stanzen wenn nicht opak).
        let w2 = 1u32 << 19; // msb Bit 19 für n = 3
        // Opaken-Bit = Bit 33 = data[3] Bit 1.
        let d_opak = blk(diff_w1() | 2, w2);
        assert_eq!((d_opak[3] >> 1) & 1, 1);
        let mut px = [[0u8; 3]; 16];
        let mut hole = [false; 16];
        color_block(be64(&d_opak), Some(true), &mut px, &mut hole);
        assert!(!hole.iter().any(|h| *h));
        // opak: Index 2 = −klein wie gehabt.
        let col = [e5(21), e5(11), e5(9)];
        assert_eq!(
            px[3],
            [c8(col[0] + MOD[4][2]), c8(col[1] + MOD[4][2]), c8(col[2] + MOD[4][2])]
        );
        // nicht opak (Bit 33 = 0): Pixel 3 stantzt.
        let d_no = blk(diff_w1() & !2, w2);
        color_block(be64(&d_no), Some(false), &mut px, &mut hole);
        assert!(hole[3]);
        assert_eq!(px[3], [0, 0, 0]);
        // Die Modifier der Nicht-opak-Tabelle (Zeile 4): Index 0 = +0,
        // 1 = +groß (60), 3 = −groß (−60).
        let mut px1 = [[0u8; 3]; 16];
        color_block(be64(&blk(diff_w1() & !2, 0xFFFF)), Some(false), &mut px1, &mut hole);
        assert_eq!(px1[0], [c8(col[0] + 60), c8(col[1] + 60), c8(col[2] + 60)]);
        let mut px3 = [[0u8; 3]; 16];
        color_block(be64(&blk(diff_w1() & !2, 0xFFFF_FFFF)), Some(false), &mut px3, &mut hole);
        assert_eq!(px3[0], [c8(col[0] - 60), c8(col[1] - 60), c8(col[2] - 60)]);
        let mut px0 = [[0u8; 3]; 16];
        color_block(be64(&blk(diff_w1() & !2, 0)), Some(false), &mut px0, &mut hole);
        assert_eq!(px0[0], col.map(|x| c8(x)));
    }

    #[test]
    fn eac_alpha_formel() {
        // Spec-Beispiel: Basis 60, Tabelle 13, Pixelindex 3 (Modifier
        // −10), Multiplikator 2 → 60 − 20 = 40 (8-Bit-Pfad ohne ×8).
        let mut d = [0u8; 8];
        d[0] = 60;
        d[1] = (2 << 4) | 13;
        d[7] = 3; // Pixel n = 15: Bits 2..0
        let v = eac_word(&d, true, false);
        assert_eq!(v[15], 40);
        // Index 7 (+9): 60 + 18 = 78.
        d[7] = 7;
        let v = eac_word(&d, true, false);
        assert_eq!(v[15], 78);
        // Multiplikator 0: alle Pixel = Basis.
        d[1] = 13;
        let v = eac_word(&d, true, false);
        assert!(v.iter().all(|a| *a == 60));
    }

    #[test]
    fn eac_r11_elfbit_umweg() {
        // Spec-Beispiel: Basis 60, Tabelle 13, Index 3, Multiplikator 2 →
        // 480 − 160 = 320; +4-Rundung: (320+4)>>3 = 40.
        let mut d = [0u8; 8];
        d[0] = 60;
        d[1] = (2 << 4) | 13;
        d[2] = 3 << 5; // Pixel n = 0: Bits 47..45 (Byte 2 = Bits 47..40)
        let v = eac_word(&d, false, false);
        assert_eq!(v[0], 40);
        // Multiplikator 0: Basis×8 + Modifier (ohne ×8): 480 − 10 + 4 = 474.
        d[1] = 13;
        let v = eac_word(&d, false, false);
        assert_eq!(v[0], (474 >> 3) as u8);
        // Überlauf: Basis 255, Index 7 (+9, Tabelle 13), Multiplikator 15
        // → clamp [0,2047] → 255.
        d[0] = 255;
        d[1] = (15 << 4) | 13;
        d[2] = 7 << 5;
        let v = eac_word(&d, false, false);
        assert_eq!(v[0], 255);
    }

    #[test]
    fn decode_blocks_rgb8a1_stanzt_in_kante() {
        // 4×4-Bild, ein Block; Pixel n = 3 = global (0,3) stantzt.
        let block = blk(diff_w1() & !2, 1 << 19);
        let mut rgba = vec![0u8; 64];
        decode_blocks(149, &block, 0, 4, 4, &mut rgba).unwrap();
        // Nicht-opaker Pfad: Index 00 = Modifier +0 → Basisfarbe.
        let col = [e5(21), e5(11), e5(9)].map(c8);
        assert_eq!(&rgba[0..4], &[col[0], col[1], col[2], 255]); // (0,0)
        assert_eq!(&rgba[48..52], &[0, 0, 0, 0]); // (0,3) gestanzt
        // Opak: kein Alpha-Loch, Index 2 = −klein (Tabelle 4 → −18).
        let block = blk(diff_w1() | 2, 1 << 19);
        decode_blocks(149, &block, 0, 4, 4, &mut rgba).unwrap();
        let m = MOD[4][2];
        let want = [e5(21) + m, e5(11) + m, e5(9) + m].map(c8);
        assert_eq!(&rgba[48..52], &[want[0], want[1], want[2], 255]);
    }

    #[test]
    fn decode_blocks_rg11_rot_zuerst() {
        // Erstes Wort = Rot, zweites = Grün (Spec C.1.5/C.1.8).
        let mut d = [0u8; 16];
        d[0] = 100;
        d[1] = 1 << 4; // Multiplikator 1, Tabelle 0 → Index 0 = −3
        d[8] = 20;
        d[9] = 1 << 4;
        let mut rgba = vec![0u8; 4 * 16];
        decode_blocks(155, &d, 0, 4, 4, &mut rgba).unwrap();
        // R: (100*8 − 3*8 + 4)>>3 = 780>>3 = 97; G: (20*8 − 24 + 4)>>3 = 17.
        assert_eq!(&rgba[0..4], &[97, 17, 0, 255]);
        assert_eq!(&rgba[60..64], &[97, 17, 0, 255]);
    }

    #[test]
    fn decode_blocks_r11_graustufen() {
        let mut d = [0u8; 8];
        d[0] = 128;
        d[1] = 2 << 4; // Multiplikator 2, Tabelle 0 → Index 0 = −3
        let mut rgba = vec![0u8; 4 * 16];
        decode_blocks(153, &d, 0, 4, 4, &mut rgba).unwrap();
        // (128*8 − 3*16 + 4) >> 3 = 980 >> 3 = 122.
        assert_eq!(&rgba[0..4], &[122, 122, 122, 255]);
    }

    #[test]
    fn decode_blocks_rgba8_alpha_wort_zuerst() {
        // 16-Byte-Block: Wort 0 = EAC-Alpha (Multiplikator 0 → 77),
        // Wort 1 = Farbwort: Bit 33 = 0 → Einzel mit Farbe 0, Index 00
        // → Modifier +2.
        let mut d = [0u8; 16];
        d[0] = 77;
        let mut rgba = vec![0u8; 4 * 16];
        decode_blocks(151, &d, 0, 4, 4, &mut rgba).unwrap();
        for px in rgba.chunks_exact(4) {
            assert_eq!(px, &[2, 2, 2, 77]);
        }
    }

    #[test]
    fn decode_blocks_kantenbeschneidung() {
        // 2×2-Bild aus einem 8-Byte-Block (RGB8): nur 4 Pixel beschrieben.
        let block = blk(diff_w1(), 0);
        let mut rgba = vec![7u8; 16];
        decode_blocks(147, &block, 0, 2, 2, &mut rgba).unwrap();
        assert!(rgba[..16].iter().any(|v| *v != 7));
        assert_eq!(rgba.len(), 16);
    }

    #[test]
    fn abgeschnittene_daten_melden() {
        let d = [0u8; 12];
        let mut rgba = vec![0u8; 64];
        // 8×8 → vier Blöcke à 8 Byte = 32 nötig.
        assert!(decode_blocks(147, &d, 0, 8, 8, &mut rgba).is_err());
    }
}
