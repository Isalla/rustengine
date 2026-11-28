//! BC7 decoder — das Desktop-Qualitätsformat für RGBA-Texturen.
//!
//! Bit-genau nach der Microsoft-Spezifikation („BC7 Format“); die
//! Decode-Logik folgt der MIT-lizenzierten Referenzimplementierung
//! `bcdec` (Copyright (c) 2022 Sergii Kudlai,
//! <https://github.com/iOrange/bcdec>) als reines Rust. Die
//! Partitionstabellen stammen aus ebendieser Referenz, die sie aus der
//! Microsoft-Spezifikation übernimmt (die Khronos-Tische fehlerhaft
//! sind, meldet bcdec ausdrücklich).
//!
//! Kurzüberblick: 8 Blockmodi, 128 Bit je 4×4-Block. Endpunkte werden
//! mit P-Bits auf 5–8 Bit Präzition gebracht und per MSB-Replikation
//! auf 8 Bit normalisiert; Pixelindizes (2–4 Bit) wählen auf einer
//! Geraden zwischen zwei Endpunkten, Fix-up-Indizes je Subset sparen
//! ein Bit. Modi 4/5 können Kanäle rotieren (Skalar statt Alpha).

/// Partitionsmuster für 2 Subsets; Bit 7 markiert den Fix-up-Index.
/// Wert & 3 = Subset-Nummer. Zeilen-major: Index = y * 4 + x.
#[rustfmt::skip]
static PARTITION_2: [[u8; 16]; 64] = [
    [128, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 129],
    [128, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 129],
    [128, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 129],
    [128, 0, 0, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 1, 1, 129],
    [128, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 1, 129],
    [128, 0, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 1, 1, 1, 129],
    [128, 0, 0, 1, 0, 0, 1, 1, 0, 1, 1, 1, 1, 1, 1, 129],
    [128, 0, 0, 0, 0, 0, 0, 1, 0, 0, 1, 1, 0, 1, 1, 129],
    [128, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 1, 129],
    [128, 0, 1, 1, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 129],
    [128, 0, 0, 0, 0, 0, 0, 1, 0, 1, 1, 1, 1, 1, 1, 129],
    [128, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 1, 129],
    [128, 0, 0, 1, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 129],
    [128, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 129],
    [128, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 129],
    [128, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 129],
    [128, 0, 0, 0, 1, 0, 0, 0, 1, 1, 1, 0, 1, 1, 1, 129],
    [128, 1, 129, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0],
    [128, 0, 0, 0, 0, 0, 0, 0, 129, 0, 0, 0, 1, 1, 1, 0],
    [128, 1, 129, 1, 0, 0, 1, 1, 0, 0, 0, 1, 0, 0, 0, 0],
    [128, 0, 129, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0],
    [128, 0, 0, 0, 1, 0, 0, 0, 129, 1, 0, 0, 1, 1, 1, 0],
    [128, 0, 0, 0, 0, 0, 0, 0, 129, 0, 0, 0, 1, 1, 0, 0],
    [128, 1, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 0, 129],
    [128, 0, 129, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0],
    [128, 0, 0, 0, 1, 0, 0, 0, 129, 0, 0, 0, 1, 1, 0, 0],
    [128, 1, 129, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0],
    [128, 0, 129, 1, 0, 1, 1, 0, 0, 1, 1, 0, 1, 1, 0, 0],
    [128, 0, 0, 1, 0, 1, 1, 1, 129, 1, 1, 0, 1, 0, 0, 0],
    [128, 0, 0, 0, 1, 1, 1, 1, 129, 1, 1, 1, 0, 0, 0, 0],
    [128, 1, 129, 1, 0, 0, 0, 1, 1, 0, 0, 0, 1, 1, 1, 0],
    [128, 0, 129, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 1, 0, 0],
    [128, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 129],
    [128, 0, 0, 0, 1, 1, 1, 1, 0, 0, 0, 0, 1, 1, 1, 129],
    [128, 1, 0, 1, 1, 0, 129, 0, 0, 1, 0, 1, 1, 0, 1, 0],
    [128, 0, 1, 1, 0, 0, 1, 1, 129, 1, 0, 0, 1, 1, 0, 0],
    [128, 0, 129, 1, 1, 1, 0, 0, 0, 0, 1, 1, 1, 1, 0, 0],
    [128, 1, 0, 1, 0, 1, 0, 1, 129, 0, 1, 0, 1, 0, 1, 0],
    [128, 1, 1, 0, 1, 0, 0, 1, 0, 1, 1, 0, 1, 0, 0, 129],
    [128, 1, 0, 1, 1, 0, 1, 0, 1, 0, 1, 0, 0, 1, 0, 129],
    [128, 1, 129, 1, 0, 0, 1, 1, 1, 1, 0, 0, 1, 1, 1, 0],
    [128, 0, 0, 1, 0, 0, 1, 1, 129, 1, 0, 0, 1, 0, 0, 0],
    [128, 0, 129, 1, 0, 0, 1, 0, 0, 1, 0, 0, 1, 1, 0, 0],
    [128, 0, 129, 1, 1, 0, 1, 1, 1, 1, 0, 1, 1, 1, 0, 0],
    [128, 1, 129, 0, 1, 0, 0, 1, 1, 0, 0, 1, 0, 1, 1, 0],
    [128, 0, 1, 1, 1, 1, 0, 0, 1, 1, 0, 0, 0, 0, 1, 129],
    [128, 1, 1, 0, 0, 1, 1, 0, 1, 0, 0, 1, 1, 0, 0, 129],
    [128, 0, 0, 0, 0, 1, 129, 0, 0, 1, 1, 0, 0, 0, 0, 0],
    [128, 1, 0, 0, 1, 1, 129, 0, 0, 1, 0, 0, 0, 0, 0, 0],
    [128, 0, 129, 0, 0, 1, 1, 1, 0, 0, 1, 0, 0, 0, 0, 0],
    [128, 0, 0, 0, 0, 0, 129, 0, 0, 1, 1, 1, 0, 0, 1, 0],
    [128, 0, 0, 0, 0, 1, 0, 0, 129, 1, 1, 0, 0, 1, 0, 0],
    [128, 1, 1, 0, 1, 1, 0, 0, 1, 0, 0, 1, 0, 0, 1, 129],
    [128, 0, 1, 1, 0, 1, 1, 0, 1, 1, 0, 0, 1, 0, 0, 129],
    [128, 1, 129, 0, 0, 0, 1, 1, 1, 0, 0, 1, 1, 1, 0, 0],
    [128, 0, 129, 1, 1, 0, 0, 1, 1, 1, 0, 0, 0, 1, 1, 0],
    [128, 1, 1, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 0, 0, 129],
    [128, 1, 1, 0, 0, 0, 1, 1, 0, 0, 1, 1, 1, 0, 0, 129],
    [128, 1, 1, 1, 1, 1, 1, 0, 1, 0, 0, 0, 0, 0, 0, 129],
    [128, 0, 0, 1, 1, 0, 0, 0, 1, 1, 1, 0, 0, 1, 1, 129],
    [128, 0, 0, 0, 1, 1, 1, 1, 0, 0, 1, 1, 0, 0, 1, 129],
    [128, 0, 129, 1, 0, 0, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0],
    [128, 0, 129, 0, 0, 0, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0],
    [128, 1, 0, 0, 0, 1, 0, 0, 0, 1, 1, 1, 0, 1, 1, 129],
];

static PARTITION_3: [[u8; 16]; 64] = [
    [128, 0, 1, 129, 0, 0, 1, 1, 0, 2, 2, 1, 2, 2, 2, 130],
    [128, 0, 0, 129, 0, 0, 1, 1, 130, 2, 1, 1, 2, 2, 2, 1],
    [128, 0, 0, 0, 2, 0, 0, 1, 130, 2, 1, 1, 2, 2, 1, 129],
    [128, 2, 2, 130, 0, 0, 2, 2, 0, 0, 1, 1, 0, 1, 1, 129],
    [128, 0, 0, 0, 0, 0, 0, 0, 129, 1, 2, 2, 1, 1, 2, 130],
    [128, 0, 1, 129, 0, 0, 1, 1, 0, 0, 2, 2, 0, 0, 2, 130],
    [128, 0, 2, 130, 0, 0, 2, 2, 1, 1, 1, 1, 1, 1, 1, 129],
    [128, 0, 1, 1, 0, 0, 1, 1, 130, 2, 1, 1, 2, 2, 1, 129],
    [128, 0, 0, 0, 0, 0, 0, 0, 129, 1, 1, 1, 2, 2, 2, 130],
    [128, 0, 0, 0, 1, 1, 1, 1, 129, 1, 1, 1, 2, 2, 2, 130],
    [128, 0, 0, 0, 1, 1, 129, 1, 2, 2, 2, 2, 2, 2, 2, 130],
    [128, 0, 1, 2, 0, 0, 129, 2, 0, 0, 1, 2, 0, 0, 1, 130],
    [128, 1, 1, 2, 0, 1, 129, 2, 0, 1, 1, 2, 0, 1, 1, 130],
    [128, 1, 2, 2, 0, 129, 2, 2, 0, 1, 2, 2, 0, 1, 2, 130],
    [128, 0, 1, 129, 0, 1, 1, 2, 1, 1, 2, 2, 1, 2, 2, 130],
    [128, 0, 1, 129, 2, 0, 0, 1, 130, 2, 0, 0, 2, 2, 2, 0],
    [128, 0, 0, 129, 0, 0, 1, 1, 0, 1, 1, 2, 1, 1, 2, 130],
    [128, 1, 1, 129, 0, 0, 1, 1, 130, 0, 0, 1, 2, 2, 0, 0],
    [128, 0, 0, 0, 1, 1, 2, 2, 129, 1, 2, 2, 1, 1, 2, 130],
    [128, 0, 2, 130, 0, 0, 2, 2, 0, 0, 2, 2, 1, 1, 1, 129],
    [128, 1, 1, 129, 0, 1, 1, 1, 0, 2, 2, 2, 0, 2, 2, 130],
    [128, 0, 0, 129, 0, 0, 0, 1, 130, 2, 2, 1, 2, 2, 2, 1],
    [128, 0, 0, 0, 0, 0, 129, 1, 0, 1, 2, 2, 0, 1, 2, 130],
    [128, 0, 0, 0, 1, 1, 0, 0, 130, 2, 129, 0, 2, 2, 1, 0],
    [128, 1, 2, 130, 0, 129, 2, 2, 0, 0, 1, 1, 0, 0, 0, 0],
    [128, 0, 1, 2, 0, 0, 1, 2, 129, 1, 2, 2, 2, 2, 2, 130],
    [128, 1, 1, 0, 1, 2, 130, 1, 129, 2, 2, 1, 0, 1, 1, 0],
    [128, 0, 0, 0, 0, 1, 129, 0, 1, 2, 130, 1, 1, 2, 2, 1],
    [128, 0, 2, 2, 1, 1, 0, 2, 129, 1, 0, 2, 0, 0, 2, 130],
    [128, 1, 1, 0, 0, 129, 1, 0, 2, 0, 0, 2, 2, 2, 2, 130],
    [128, 0, 1, 1, 0, 1, 2, 2, 0, 1, 130, 2, 0, 0, 1, 129],
    [128, 0, 0, 0, 2, 0, 0, 0, 130, 2, 1, 1, 2, 2, 2, 129],
    [128, 0, 0, 0, 0, 0, 0, 2, 129, 1, 2, 2, 1, 2, 2, 130],
    [128, 2, 2, 130, 0, 0, 2, 2, 0, 0, 1, 2, 0, 0, 1, 129],
    [128, 0, 1, 129, 0, 0, 1, 2, 0, 0, 2, 2, 0, 2, 2, 130],
    [128, 1, 2, 0, 0, 129, 2, 0, 0, 1, 130, 0, 0, 1, 2, 0],
    [128, 0, 0, 0, 1, 1, 129, 1, 2, 2, 130, 2, 0, 0, 0, 0],
    [128, 1, 2, 0, 1, 2, 0, 1, 130, 0, 129, 2, 0, 1, 2, 0],
    [128, 1, 2, 0, 2, 0, 1, 2, 129, 130, 0, 1, 0, 1, 2, 0],
    [128, 0, 1, 1, 2, 2, 0, 0, 1, 1, 130, 2, 0, 0, 1, 129],
    [128, 0, 1, 1, 1, 1, 130, 2, 2, 2, 0, 0, 0, 0, 1, 129],
    [128, 1, 0, 129, 0, 1, 0, 1, 2, 2, 2, 2, 2, 2, 2, 130],
    [128, 0, 0, 0, 0, 0, 0, 0, 130, 1, 2, 1, 2, 1, 2, 129],
    [128, 0, 2, 2, 1, 129, 2, 2, 0, 0, 2, 2, 1, 1, 2, 130],
    [128, 0, 2, 130, 0, 0, 1, 1, 0, 0, 2, 2, 0, 0, 1, 129],
    [128, 2, 2, 0, 1, 2, 130, 1, 0, 2, 2, 0, 1, 2, 2, 129],
    [128, 1, 0, 1, 2, 2, 130, 2, 2, 2, 2, 2, 0, 1, 0, 129],
    [128, 0, 0, 0, 2, 1, 2, 1, 130, 1, 2, 1, 2, 1, 2, 129],
    [128, 1, 0, 129, 0, 1, 0, 1, 0, 1, 0, 1, 2, 2, 2, 130],
    [128, 2, 2, 130, 0, 1, 1, 1, 0, 2, 2, 2, 0, 1, 1, 129],
    [128, 0, 0, 2, 1, 129, 1, 2, 0, 0, 0, 2, 1, 1, 1, 130],
    [128, 0, 0, 0, 2, 129, 1, 2, 2, 1, 1, 2, 2, 1, 1, 130],
    [128, 2, 2, 2, 0, 129, 1, 1, 0, 1, 1, 1, 0, 2, 2, 130],
    [128, 0, 0, 2, 1, 1, 1, 2, 129, 1, 1, 2, 0, 0, 0, 130],
    [128, 1, 1, 0, 0, 129, 1, 0, 0, 1, 1, 0, 2, 2, 2, 130],
    [128, 0, 0, 0, 0, 0, 0, 0, 2, 1, 129, 2, 2, 1, 1, 130],
    [128, 1, 1, 0, 0, 129, 1, 0, 2, 2, 2, 2, 2, 2, 2, 130],
    [128, 0, 2, 2, 0, 0, 1, 1, 0, 0, 129, 1, 0, 0, 2, 130],
    [128, 0, 2, 2, 1, 1, 2, 2, 129, 1, 2, 2, 0, 0, 2, 130],
    [128, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 129, 1, 130],
    [128, 0, 0, 130, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 129],
    [128, 2, 2, 2, 1, 2, 2, 2, 0, 2, 2, 2, 129, 2, 2, 130],
    [128, 1, 0, 129, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 130],
    [128, 1, 1, 129, 2, 0, 1, 1, 130, 2, 0, 1, 2, 2, 2, 0],
];


const W2: [u32; 4] = [0, 21, 43, 64];
const W3: [u32; 8] = [0, 9, 18, 27, 37, 46, 55, 64];
#[rustfmt::skip]
const W4: [u32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

/// Rohe Endpunkt-Bits je Modus: RGB-Kanäle und Alpha.
const RGBA_BITS: [u32; 8] = [4, 6, 5, 7, 5, 7, 7, 5];
const A_BITS: [u32; 8] = [0, 0, 0, 0, 6, 8, 7, 5];

/// 128-Bit-Block, LSB-zuerst, wie die Spezifikation ihn liest.
struct Bits {
    v: u128,
    pos: u32,
}

impl Bits {
    fn take(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        let x = ((self.v >> self.pos) & ((1u128 << n) - 1)) as u32;
        self.pos += n;
        x
    }
}

/// n-Bit-Wert auf 8 Bit normalisieren: MSB-Replikation nach links.
fn unorm8(v: u32, bits: u32) -> u32 {
    if bits >= 8 {
        return v & 0xFF;
    }
    let s = v << (8 - bits);
    (s | (s >> bits)) & 0xFF
}

fn interp(e0: u32, e1: u32, w: &[u32], idx: u32) -> u8 {
    let t = w[idx as usize];
    (((64 - t) * e0 + t * e1 + 32) >> 6) as u8
}

/// 16 Blockbytes zu 16 RGBA-Pixeln expandieren (zeilenmajor).
pub fn decode_block(b: &[u8], out: &mut [[u8; 4]; 16]) {
    let mut bs = Bits { v: u128::from_le_bytes(b[..16].try_into().unwrap()), pos: 0 };

    // Modus: führende Nullen zählen, dann die Eins. Acht Nullen =
    // reservierter Modus → Block wird transparentes Schwarz.
    let mut mode = 0usize;
    while mode < 8 && bs.take(1) == 0 {
        mode += 1;
    }
    if mode == 8 {
        for p in out.iter_mut() {
            *p = [0; 4];
        }
        return;
    }

    let mut num_subsets = 1usize;
    let mut partition = 0usize;
    let mut rotation = 0u32;
    let mut index_selection = 0u32;
    if matches!(mode, 0 | 1 | 2 | 3 | 7) {
        num_subsets = if mode == 0 || mode == 2 { 3 } else { 2 };
        // Modus 0 wählt aus 16 Mustern (4 Bit), die anderen aus 64.
        partition = bs.take(if mode == 0 { 4 } else { 6 }) as usize;
    }
    let num_endpoints = num_subsets * 2;
    if mode == 4 || mode == 5 {
        rotation = bs.take(2);
        if mode == 4 {
            index_selection = bs.take(1);
        }
    }

    // Endpunkte: erst alle R, dann G, dann B, dann A — je Subset zwei.
    let mut ep = [[0u32; 4]; 8];
    for ch in 0..3 {
        for e in 0..num_endpoints {
            ep[e][ch] = bs.take(RGBA_BITS[mode]);
        }
    }
    if A_BITS[mode] > 0 {
        for e in 0..num_endpoints {
            ep[e][3] = bs.take(A_BITS[mode]);
        }
    }

    // P-Bits: eine zusätzliche LSB für alle Kanäle eines Endpunkts.
    // Modus 1 teilt sie sich pro Subset (zwei Bit), sonst einer je
    // Endpunkt.
    let has_p = matches!(mode, 0 | 1 | 3 | 6 | 7);
    if has_p {
        for e in 0..num_endpoints {
            for c in 0..4 {
                ep[e][c] <<= 1;
            }
        }
        if mode == 1 {
            let p0 = bs.take(1);
            let p1 = bs.take(1);
            for ch in 0..3 {
                ep[0][ch] |= p0;
                ep[1][ch] |= p0;
                ep[2][ch] |= p1;
                ep[3][ch] |= p1;
            }
        } else {
            for e in 0..num_endpoints {
                let p = bs.take(1);
                for c in 0..4 {
                    ep[e][c] |= p;
                }
            }
        }
    }

    // Normalisieren: Präzision inkl. P-Bit, MSB-Replikation; Modi ohne
    // explizites Alpha setzen 255.
    let j_rgb = RGBA_BITS[mode] + has_p as u32;
    let j_a = A_BITS[mode] + has_p as u32;
    for e in 0..num_endpoints {
        for ch in 0..3 {
            ep[e][ch] = unorm8(ep[e][ch], j_rgb);
        }
        ep[e][3] = if A_BITS[mode] == 0 { 255 } else { unorm8(ep[e][3], j_a) };
    }

    // Primäre Indizes, zeilenmajor; der Fix-up-Index je Subset liest
    // ein Bit weniger (seine MSB ist durch die Endpunktordnung 0).
    let ibits = if mode == 0 || mode == 1 {
        3
    } else if mode == 6 {
        4
    } else {
        2
    };
    let ibits2 = if mode == 4 {
        3
    } else if mode == 5 {
        2
    } else {
        0
    };
    let ptable = match num_subsets {
        1 => None,
        2 => Some(&PARTITION_2[partition]),
        _ => Some(&PARTITION_3[partition]),
    };
    let subset_at = |i: usize, j: usize| -> u8 {
        match ptable {
            None => {
                if i | j == 0 {
                    0x80
                } else {
                    0
                }
            }
            Some(t) => t[i * 4 + j],
        }
    };
    let mut idx = [[0u32; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            let ps = subset_at(i, j);
            let n = if ps & 0x80 != 0 { ibits - 1 } else { ibits };
            idx[i][j] = bs.take(n);
        }
    }

    // Einsetzen, sekundäre (Alpha-)Indizes lesen, Kanäle rotieren.
    for i in 0..4 {
        for j in 0..4 {
            let ps = (subset_at(i, j) & 0x03) as usize;
            let index = idx[i][j];
            let (mut r, mut g, mut b, mut a);
            if ibits2 == 0 {
                r = interp(ep[ps * 2][0], ep[ps * 2 + 1][0], weight_table(ibits), index);
                g = interp(ep[ps * 2][1], ep[ps * 2 + 1][1], weight_table(ibits), index);
                b = interp(ep[ps * 2][2], ep[ps * 2 + 1][2], weight_table(ibits), index);
                a = interp(ep[ps * 2][3], ep[ps * 2 + 1][3], weight_table(ibits), index);
            } else {
                // Auch der sekundäre Satz hat seinen Fix-up bei Pixel 0.
                let n2 = if i | j == 0 { ibits2 - 1 } else { ibits2 };
                let index2 = bs.take(n2);
                if index_selection == 0 {
                    r = interp(ep[ps * 2][0], ep[ps * 2 + 1][0], weight_table(ibits), index);
                    g = interp(ep[ps * 2][1], ep[ps * 2 + 1][1], weight_table(ibits), index);
                    b = interp(ep[ps * 2][2], ep[ps * 2 + 1][2], weight_table(ibits), index);
                    a = interp(ep[ps * 2][3], ep[ps * 2 + 1][3], weight_table(ibits2), index2);
                } else {
                    r = interp(ep[ps * 2][0], ep[ps * 2 + 1][0], weight_table(ibits2), index2);
                    g = interp(ep[ps * 2][1], ep[ps * 2 + 1][1], weight_table(ibits2), index2);
                    b = interp(ep[ps * 2][2], ep[ps * 2 + 1][2], weight_table(ibits2), index2);
                    a = interp(ep[ps * 2][3], ep[ps * 2 + 1][3], weight_table(ibits), index);
                }
            }
            match rotation {
                1 => std::mem::swap(&mut a, &mut r),
                2 => std::mem::swap(&mut a, &mut g),
                3 => std::mem::swap(&mut a, &mut b),
                _ => {}
            }
            out[i * 4 + j] = [r, g, b, a];
        }
    }
}

fn weight_table(bits: u32) -> &'static [u32] {
    match bits {
        2 => &W2,
        3 => &W3,
        _ => &W4,
    }
}

#[cfg(test)]
mod tests {
    use super::decode_block;

    /// Writer: schiebt Bits LSB-zuerst in den 128-Bit-Block, exakt so,
    /// wie der Decoder sie liest. `block()` verlangt einen vollen
    /// Block — der Test prüft damit zugleich, dass die Feldfolgen alle
    /// Bits verbrauchen.
    struct Bits {
        v: u128,
        pos: u32,
    }

    impl Bits {
        fn new() -> Self {
            Self { v: 0, pos: 0 }
        }
        fn put(&mut self, n: u32, bits: u32) {
            self.v |= ((bits as u128) & ((1u128 << n) - 1)) << self.pos;
            self.pos += n;
        }
        fn block(&self) -> [u8; 16] {
            assert_eq!(self.pos, 128, "Block muss exakt 128 Bit füllen");
            self.v.to_le_bytes()
        }
    }

    #[test]
    fn modus_6_gleiche_endpunkte() {
        // Modus 6: 7-Bit-Endpunkte + P-Bit, 4-Bit-Indizes. Alle
        // Endpunkte 255 → 4×4 weiß, unabhängig von den Indizes.
        let mut b = Bits::new();
        b.put(7, 64); // 0000001 → Modus 6
        for _ in 0..8 {
            b.put(7, 127); // r0 r1 g0 g1 b0 b1 a0 a1
        }
        b.put(1, 1);
        b.put(1, 1); // P-Bits je Endpunkt
        b.put(3, 5); // Fix-up-Index: ein Bit weniger
        for _ in 1..16 {
            b.put(4, 5);
        }
        let mut out = [[0u8; 4]; 16];
        decode_block(&b.block(), &mut out);
        assert!(out.iter().all(|p| *p == [255, 255, 255, 255]), "{out:?}");
    }

    #[test]
    fn modus_6_gradient_nutzt_gewichtstabelle() {
        // e0 = schwarz (P=0), e1 = weiß; Index 0 → e0, Index 15 → e1.
        let mut b = Bits::new();
        b.put(7, 64);
        for ch in 0..6 {
            b.put(7, if ch % 2 == 0 { 0 } else { 127 }); // r0 r1 g0 g1 b0 b1
        }
        b.put(7, 127);
        b.put(7, 127); // a0 a1
        b.put(1, 0);
        b.put(1, 1); // p0=0 → Alpha von e0 bleibt 254
        b.put(3, 0); // Fix-up-Pixel wählt e0 …
        for _ in 1..16 {
            b.put(4, 15); // … alle anderen e1
        }
        let mut out = [[0u8; 4]; 16];
        decode_block(&b.block(), &mut out);
        assert_eq!(out[0], [0, 0, 0, 254]);
        assert_eq!(out[1], [255, 255, 255, 255]);
        assert_eq!(out[15], [255, 255, 255, 255]);
    }

    #[test]
    fn modus_1_partition_scheidet_endpunkte() {
        // Modus 1: 2 Subsets, Partition 0 trennt links/rechts;
        // Subset 0 rot, Subset 1 grün. P-Bits werden paarweise
        // geteilt, Fix-ups liegen bei Pixel 0 und 15.
        let mut b = Bits::new();
        b.put(2, 2); // 01 → Modus 1
        b.put(6, 0); // Partition
        for ch in 0..3 {
            let (a, c) = match ch {
                0 => (63, 0), // r: Subset 0 stark
                1 => (0, 63), // g: Subset 1 stark
                _ => (0, 0),
            };
            b.put(6, a);
            b.put(6, a);
            b.put(6, c);
            b.put(6, c);
        }
        b.put(1, 1); // p0 für e0/e1
        b.put(1, 0); // p1 für e2/e3
        for i in 0..16 {
            let fix = i == 0 || i == 15;
            b.put(if fix { 2 } else { 3 }, 0); // Gewicht 0 → je e0 des Subsets
        }
        let mut out = [[0u8; 4]; 16];
        decode_block(&b.block(), &mut out);
        assert_eq!(out[0], [255, 2, 2, 255]);
        assert_eq!(out[1], [255, 2, 2, 255]);
        assert_eq!(out[2], [0, 253, 0, 255]);
        assert_eq!(out[5], [255, 2, 2, 255]);
        assert_eq!(out[6], [0, 253, 0, 255]);
        assert_eq!(out[15], [0, 253, 0, 255]);
    }

    #[test]
    fn modus_4_rotiert_alpha_nach_rot() {
        // Modus 4: Rotation 1 vertauscht A und R; Index-Selection 0
        // hält Alpha am sekundären Index (3 Bit, eigenes Fix-up).
        let mut b = Bits::new();
        b.put(5, 16); // 00001 → Modus 4
        b.put(2, 1); // Rotation A ↔ R
        b.put(1, 0); // Index-Selection: Farbe aus primärem Index
        b.put(5, 31);
        b.put(5, 0); // r0 r1
        b.put(5, 15);
        b.put(5, 0); // g0 g1
        b.put(5, 7);
        b.put(5, 0); // b0 b1
        b.put(6, 32);
        b.put(6, 0); // a0 a1
        b.put(1, 0); // primäres Fix-up (2−1 Bit)
        for _ in 1..16 {
            b.put(2, 0);
        }
        b.put(2, 0); // sekundäres Fix-up (3−1 Bit)
        for _ in 1..16 {
            b.put(3, 0);
        }
        let mut out = [[0u8; 4]; 16];
        decode_block(&b.block(), &mut out);
        // unorm8: 31/5→255, 15→123, 7→57, 32/6→130; Rotation tauscht R und A.
        assert!(out.iter().all(|p| *p == [130, 123, 57, 255]), "{out:?}");
    }

    #[test]
    fn modus_0_drei_subsets() {
        // Modus 0: 3 Subsets, 4-Bit-Partition, 6 P-Bits, 3-Bit-Index.
        // Gleiche Endpunkte → einheitlich, aber alle Felder werden
        // gelesen (Partition 0 hat Fix-ups bei Pixel 0, 3 und 15).
        let mut b = Bits::new();
        b.put(1, 1); // 1 → Modus 0
        b.put(4, 0); // Partition
        for _ in 0..18 {
            b.put(4, 15); // r/g/b je 6 Endpunkte
        }
        for _ in 0..6 {
            b.put(1, 1); // P-Bits je Endpunkt
        }
        for i in 0..16 {
            let fix = i == 0 || i == 3 || i == 15;
            b.put(if fix { 2 } else { 3 }, 0);
        }
        let mut out = [[0u8; 4]; 16];
        decode_block(&b.block(), &mut out);
        assert!(out.iter().all(|p| *p == [255, 255, 255, 255]), "{out:?}");
    }

    #[test]
    fn modus_7_alpha_endpunkte() {
        // Modus 7: 2 Subsets, 5-Bit-RGB und 5-Bit-A plus P-Bit.
        let mut b = Bits::new();
        b.put(8, 128); // 00000001 → Modus 7
        b.put(6, 0); // Partition
        for _ in 0..12 {
            b.put(5, 31); // RGB × 4 Endpunkte
        }
        for _ in 0..4 {
            b.put(5, 15); // A × 4 Endpunkte
        }
        for _ in 0..4 {
            b.put(1, 1); // P-Bits je Endpunkt
        }
        for i in 0..16 {
            let fix = i == 0 || i == 15;
            b.put(if fix { 1 } else { 2 }, 0);
        }
        let mut out = [[0u8; 4]; 16];
        decode_block(&b.block(), &mut out);
        // RGB: 31<<1|1 = 63 → MSB-repliziert 255; A: 15<<1|1 = 31 → 125.
        assert!(out.iter().all(|p| *p == [255, 255, 255, 125]), "{out:?}");
    }

    #[test]
    fn modus_2_drei_subsets_zwei_bit() {
        // Modus 2: 3 Subsets, 6-Bit-Partition, 5-Bit-RGB, kein P-Bit,
        // 2-Bit-Index. Gleiche Endpunkte → einheitlich weiß.
        let mut b = Bits::new();
        b.put(3, 4); // 001 → Modus 2
        b.put(6, 0); // Partition
        for _ in 0..18 {
            b.put(5, 31); // r/g/b je 6 Endpunkte
        }
        for i in 0..16 {
            let fix = i == 0 || i == 3 || i == 15;
            b.put(if fix { 1 } else { 2 }, 0);
        }
        let mut out = [[0u8; 4]; 16];
        decode_block(&b.block(), &mut out);
        assert!(out.iter().all(|p| *p == [255, 255, 255, 255]), "{out:?}");
    }

    #[test]
    fn modus_5_sekundärer_index_ohne_selection_bit() {
        // Modus 5: 7-Bit-RGB, 8-Bit-A, kein P-Bit, kein
        // Index-Selection-Bit — Alpha kommt immer aus dem sekundären
        // 2-Bit-Index. Uniform: alle Endpunkte gleich.
        let mut b = Bits::new();
        b.put(6, 32); // 000001 → Modus 5
        b.put(2, 0); // keine Rotation
        for _ in 0..6 {
            b.put(7, 127); // r0 r1 g0 g1 b0 b1
        }
        b.put(8, 200);
        b.put(8, 200); // a0 a1
        for i in 0..16 {
            b.put(if i == 0 { 1 } else { 2 }, 0); // primär, Fix-up bei Pixel 0
        }
        for i in 0..16 {
            b.put(if i == 0 { 1 } else { 2 }, 0); // sekundär, eigenes Fix-up
        }
        let mut out = [[0u8; 4]; 16];
        decode_block(&b.block(), &mut out);
        // RGB: unorm8(127, 7) = 255; Alpha bleibt exakt 200.
        assert!(out.iter().all(|p| *p == [255, 255, 255, 200]), "{out:?}");
    }
}
