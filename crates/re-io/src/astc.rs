//! ASTC-Dekodierer für 2D-Texturen (LDR- und HDR-Profil).
//!
//! Der Decoder folgt dem Anhang „Adaptive Scalable Texture Compression“ der
//! Khronos Data Format Specification. Die Ausgabe ist blockweise bitgenau
//! gegen zwei unabhängige, gegen ARM astcenc fuzzgetestete Referenzdekoder
//! validiert (Details in `THIRD_PARTY_NOTICES.md`).
//!
//! Verhalten am Blockrand:
//! * Illegal kodierte Blöcke liefern die Norm-Fehlerfarbe (deckendes Magenta:
//!   `255,0,255,255` bzw. FP16 `1.0,0.0,1.0,1.0`).
//! * Void-Extent-Blöcke werden unterstützt; die Extent-Koordinaten werden nur
//!   auf Plausibilität geprüft (`min < max`, außer „alle Bits 1“), nie als
//!   Bildinformation benutzt.
//! * HDR-Void-Extent-Farben mit NaN/Unendlich gelten als Fehlerblock (die
//!   Spezifikation nennt das Verhalten „undefined“; beide Referenzen
//!   verwerfen diese Blöcke standardmäßig, also tun wir das auch).
//! * `*_UNORM`-Formate nutzen den linear-LDR-Pfad (Bit-Replikation bei der
//!   Endpunkt-Interpolation), `*_SRGB`-Formate den sRGB-Pfad (`| 0x80` gemäß
//!   aktueller Spek-Revision, inklusive Alphakanal).
//! * Texel mit HDR-Endpunkten in LDR-Dekodierung ergeben die Fehlerfarbe;
//!   die LDR-Partitionen desselben Blocks dekodieren normal.
//! * 3D-Blockmodi (aus KTX2-3D-Texturen) werden als Fehlerblock behandelt —
//!   der Decoder kennt nur 2D-Blöcke.

use anyhow::{Result, bail};

/// Bitmuster eines 128-Bit-ASTC-Blocks.
#[derive(Clone, Copy)]
struct Block([u64; 2]);

impl Block {
    fn read(data: &[u8]) -> Self {
        let mut w = [0u64; 2];
        for (i, word) in w.iter_mut().enumerate() {
            let mut b = [0u8; 8];
            b.copy_from_slice(&data[i * 8..i * 8 + 8]);
            *word = u64::from_le_bytes(b);
        }
        Block(w)
    }

    /// Bits `[lo, lo+n)` als Little-Endian-Wert (entspricht `getBits` der
    /// Referenzen).
    #[inline]
    fn bits(&self, lo: u32, n: u32) -> u32 {
        debug_assert!(n > 0 && n <= 32 && lo + n <= 128);
        let wi = (lo >> 6) as usize;
        let sh = lo & 63;
        let raw = if sh + n <= 64 {
            self.0[wi] >> sh
        } else {
            (self.0[wi] >> sh) | (self.0[wi + 1] << (64 - sh))
        };
        (raw & (u64::MAX >> (64 - n))) as u32
    }

    /// Liest `n` Bits ab `pos` und schiebt `pos` weiter (`next_bits`).
    #[inline]
    fn take(&self, pos: &mut u32, n: u32) -> u32 {
        let v = self.bits(*pos, n);
        *pos += n;
        v
    }

    /// Liest `n` Bits des Gewichtsstroms: die Referenzen dekodieren Gewichte
    /// aus den komplett bitumgekehrten 128 Bit von Bit 0 (aufwärts) ab. Das
    /// entspricht physikalischen Bits `[128-pos-n, 128-pos)` mit umgekehrter
    /// Bitreihenfolge.
    #[inline]
    fn take_top(&self, pos: &mut u32, n: u32) -> u32 {
        debug_assert!(n > 0);
        let p = 128 - *pos - n;
        *pos += n;
        let v = self.bits(p, n);
        v.reverse_bits() >> (32 - n)
    }
}

/// ISE-Bereichstabelle: `[Bits, Trits, Quints]` je Bereichsindex 0..=20.
const ISE_RANGES: [[u32; 3]; 21] = [
    [1, 0, 0], // 0..1
    [0, 1, 0], // 0..2
    [2, 0, 0], // 0..3
    [0, 0, 1], // 0..4
    [1, 1, 0], // 0..5   kleinster Endpunkt-Bereich
    [3, 0, 0], // 0..7
    [1, 0, 1], // 0..9
    [2, 1, 0], // 0..11
    [4, 0, 0], // 0..15
    [2, 0, 1], // 0..19
    [3, 1, 0], // 0..23
    [5, 0, 0], // 0..31   größter Gewichts-Bereich
    [3, 0, 1], // 0..39
    [4, 1, 0], // 0..47
    [6, 0, 0], // 0..63
    [4, 0, 1], // 0..79
    [5, 1, 0], // 0..95
    [7, 0, 0], // 0..127
    [5, 0, 1], // 0..159
    [6, 1, 0], // 0..191
    [8, 0, 0], // 0..255
];

/// Anzahl BISE-Stufen je Bereichsindex (`{2,3,4,5} * 2^bits`).
#[inline]
const fn ise_levels(range: usize) -> usize {
    let t = ISE_RANGES[range];
    ((1 + 2 * t[1] + 4 * t[2]) << t[0]) as usize
}

/// Von `count` Werten des Bereichs `range` belegte Bits (Spezifikation
/// „Data Size Determination“).
#[inline]
const fn ise_sequence_bits(count: usize, range: usize) -> usize {
    let t = ISE_RANGES[range];
    (t[0] as usize) * count
        + (8 * t[1] as usize * count + 4) / 5
        + (7 * t[2] as usize * count + 2) / 3
}

const FIRST_ENDPOINT_RANGE: usize = 4; // 0..5 — kleiner ist für Endpunkte illegal
const LAST_ENDPOINT_RANGE: usize = 20; // 0..255
const LAST_WEIGHT_RANGE: usize = 11; // 0..31
const MAX_ENDPOINT_VALUES: usize = 18;
const MAX_GRID_WEIGHTS: usize = 64;

/// Trit-Dekodierung 8 Bit → fünf Tritwerte (Spezifikations-Algorithmus,
/// tabelliert; identisch zu beiden Validierungsreferenzen).
const TRIT_DECODE: [[u8; 5]; 256] = [
    [0, 0, 0, 0, 0],
    [1, 0, 0, 0, 0],
    [2, 0, 0, 0, 0],
    [0, 0, 2, 0, 0],
    [0, 1, 0, 0, 0],
    [1, 1, 0, 0, 0],
    [2, 1, 0, 0, 0],
    [1, 0, 2, 0, 0],
    [0, 2, 0, 0, 0],
    [1, 2, 0, 0, 0],
    [2, 2, 0, 0, 0],
    [2, 0, 2, 0, 0],
    [0, 2, 2, 0, 0],
    [1, 2, 2, 0, 0],
    [2, 2, 2, 0, 0],
    [2, 0, 2, 0, 0],
    [0, 0, 1, 0, 0],
    [1, 0, 1, 0, 0],
    [2, 0, 1, 0, 0],
    [0, 1, 2, 0, 0],
    [0, 1, 1, 0, 0],
    [1, 1, 1, 0, 0],
    [2, 1, 1, 0, 0],
    [1, 1, 2, 0, 0],
    [0, 2, 1, 0, 0],
    [1, 2, 1, 0, 0],
    [2, 2, 1, 0, 0],
    [2, 1, 2, 0, 0],
    [0, 0, 0, 2, 2],
    [1, 0, 0, 2, 2],
    [2, 0, 0, 2, 2],
    [0, 0, 2, 2, 2],
    [0, 0, 0, 1, 0],
    [1, 0, 0, 1, 0],
    [2, 0, 0, 1, 0],
    [0, 0, 2, 1, 0],
    [0, 1, 0, 1, 0],
    [1, 1, 0, 1, 0],
    [2, 1, 0, 1, 0],
    [1, 0, 2, 1, 0],
    [0, 2, 0, 1, 0],
    [1, 2, 0, 1, 0],
    [2, 2, 0, 1, 0],
    [2, 0, 2, 1, 0],
    [0, 2, 2, 1, 0],
    [1, 2, 2, 1, 0],
    [2, 2, 2, 1, 0],
    [2, 0, 2, 1, 0],
    [0, 0, 1, 1, 0],
    [1, 0, 1, 1, 0],
    [2, 0, 1, 1, 0],
    [0, 1, 2, 1, 0],
    [0, 1, 1, 1, 0],
    [1, 1, 1, 1, 0],
    [2, 1, 1, 1, 0],
    [1, 1, 2, 1, 0],
    [0, 2, 1, 1, 0],
    [1, 2, 1, 1, 0],
    [2, 2, 1, 1, 0],
    [2, 1, 2, 1, 0],
    [0, 1, 0, 2, 2],
    [1, 1, 0, 2, 2],
    [2, 1, 0, 2, 2],
    [1, 0, 2, 2, 2],
    [0, 0, 0, 2, 0],
    [1, 0, 0, 2, 0],
    [2, 0, 0, 2, 0],
    [0, 0, 2, 2, 0],
    [0, 1, 0, 2, 0],
    [1, 1, 0, 2, 0],
    [2, 1, 0, 2, 0],
    [1, 0, 2, 2, 0],
    [0, 2, 0, 2, 0],
    [1, 2, 0, 2, 0],
    [2, 2, 0, 2, 0],
    [2, 0, 2, 2, 0],
    [0, 2, 2, 2, 0],
    [1, 2, 2, 2, 0],
    [2, 2, 2, 2, 0],
    [2, 0, 2, 2, 0],
    [0, 0, 1, 2, 0],
    [1, 0, 1, 2, 0],
    [2, 0, 1, 2, 0],
    [0, 1, 2, 2, 0],
    [0, 1, 1, 2, 0],
    [1, 1, 1, 2, 0],
    [2, 1, 1, 2, 0],
    [1, 1, 2, 2, 0],
    [0, 2, 1, 2, 0],
    [1, 2, 1, 2, 0],
    [2, 2, 1, 2, 0],
    [2, 1, 2, 2, 0],
    [0, 2, 0, 2, 2],
    [1, 2, 0, 2, 2],
    [2, 2, 0, 2, 2],
    [2, 0, 2, 2, 2],
    [0, 0, 0, 0, 2],
    [1, 0, 0, 0, 2],
    [2, 0, 0, 0, 2],
    [0, 0, 2, 0, 2],
    [0, 1, 0, 0, 2],
    [1, 1, 0, 0, 2],
    [2, 1, 0, 0, 2],
    [1, 0, 2, 0, 2],
    [0, 2, 0, 0, 2],
    [1, 2, 0, 0, 2],
    [2, 2, 0, 0, 2],
    [2, 0, 2, 0, 2],
    [0, 2, 2, 0, 2],
    [1, 2, 2, 0, 2],
    [2, 2, 2, 0, 2],
    [2, 0, 2, 0, 2],
    [0, 0, 1, 0, 2],
    [1, 0, 1, 0, 2],
    [2, 0, 1, 0, 2],
    [0, 1, 2, 0, 2],
    [0, 1, 1, 0, 2],
    [1, 1, 1, 0, 2],
    [2, 1, 1, 0, 2],
    [1, 1, 2, 0, 2],
    [0, 2, 1, 0, 2],
    [1, 2, 1, 0, 2],
    [2, 2, 1, 0, 2],
    [2, 1, 2, 0, 2],
    [0, 2, 2, 2, 2],
    [1, 2, 2, 2, 2],
    [2, 2, 2, 2, 2],
    [2, 0, 2, 2, 2],
    [0, 0, 0, 0, 1],
    [1, 0, 0, 0, 1],
    [2, 0, 0, 0, 1],
    [0, 0, 2, 0, 1],
    [0, 1, 0, 0, 1],
    [1, 1, 0, 0, 1],
    [2, 1, 0, 0, 1],
    [1, 0, 2, 0, 1],
    [0, 2, 0, 0, 1],
    [1, 2, 0, 0, 1],
    [2, 2, 0, 0, 1],
    [2, 0, 2, 0, 1],
    [0, 2, 2, 0, 1],
    [1, 2, 2, 0, 1],
    [2, 2, 2, 0, 1],
    [2, 0, 2, 0, 1],
    [0, 0, 1, 0, 1],
    [1, 0, 1, 0, 1],
    [2, 0, 1, 0, 1],
    [0, 1, 2, 0, 1],
    [0, 1, 1, 0, 1],
    [1, 1, 1, 0, 1],
    [2, 1, 1, 0, 1],
    [1, 1, 2, 0, 1],
    [0, 2, 1, 0, 1],
    [1, 2, 1, 0, 1],
    [2, 2, 1, 0, 1],
    [2, 1, 2, 0, 1],
    [0, 0, 1, 2, 2],
    [1, 0, 1, 2, 2],
    [2, 0, 1, 2, 2],
    [0, 1, 2, 2, 2],
    [0, 0, 0, 1, 1],
    [1, 0, 0, 1, 1],
    [2, 0, 0, 1, 1],
    [0, 0, 2, 1, 1],
    [0, 1, 0, 1, 1],
    [1, 1, 0, 1, 1],
    [2, 1, 0, 1, 1],
    [1, 0, 2, 1, 1],
    [0, 2, 0, 1, 1],
    [1, 2, 0, 1, 1],
    [2, 2, 0, 1, 1],
    [2, 0, 2, 1, 1],
    [0, 2, 2, 1, 1],
    [1, 2, 2, 1, 1],
    [2, 2, 2, 1, 1],
    [2, 0, 2, 1, 1],
    [0, 0, 1, 1, 1],
    [1, 0, 1, 1, 1],
    [2, 0, 1, 1, 1],
    [0, 1, 2, 1, 1],
    [0, 1, 1, 1, 1],
    [1, 1, 1, 1, 1],
    [2, 1, 1, 1, 1],
    [1, 1, 2, 1, 1],
    [0, 2, 1, 1, 1],
    [1, 2, 1, 1, 1],
    [2, 2, 1, 1, 1],
    [2, 1, 2, 1, 1],
    [0, 1, 1, 2, 2],
    [1, 1, 1, 2, 2],
    [2, 1, 1, 2, 2],
    [1, 1, 2, 2, 2],
    [0, 0, 0, 2, 1],
    [1, 0, 0, 2, 1],
    [2, 0, 0, 2, 1],
    [0, 0, 2, 2, 1],
    [0, 1, 0, 2, 1],
    [1, 1, 0, 2, 1],
    [2, 1, 0, 2, 1],
    [1, 0, 2, 2, 1],
    [0, 2, 0, 2, 1],
    [1, 2, 0, 2, 1],
    [2, 2, 0, 2, 1],
    [2, 0, 2, 2, 1],
    [0, 2, 2, 2, 1],
    [1, 2, 2, 2, 1],
    [2, 2, 2, 2, 1],
    [2, 0, 2, 2, 1],
    [0, 0, 1, 2, 1],
    [1, 0, 1, 2, 1],
    [2, 0, 1, 2, 1],
    [0, 1, 2, 2, 1],
    [0, 1, 1, 2, 1],
    [1, 1, 1, 2, 1],
    [2, 1, 1, 2, 1],
    [1, 1, 2, 2, 1],
    [0, 2, 1, 2, 1],
    [1, 2, 1, 2, 1],
    [2, 2, 1, 2, 1],
    [2, 1, 2, 2, 1],
    [0, 2, 1, 2, 2],
    [1, 2, 1, 2, 2],
    [2, 2, 1, 2, 2],
    [2, 1, 2, 2, 2],
    [0, 0, 0, 1, 2],
    [1, 0, 0, 1, 2],
    [2, 0, 0, 1, 2],
    [0, 0, 2, 1, 2],
    [0, 1, 0, 1, 2],
    [1, 1, 0, 1, 2],
    [2, 1, 0, 1, 2],
    [1, 0, 2, 1, 2],
    [0, 2, 0, 1, 2],
    [1, 2, 0, 1, 2],
    [2, 2, 0, 1, 2],
    [2, 0, 2, 1, 2],
    [0, 2, 2, 1, 2],
    [1, 2, 2, 1, 2],
    [2, 2, 2, 1, 2],
    [2, 0, 2, 1, 2],
    [0, 0, 1, 1, 2],
    [1, 0, 1, 1, 2],
    [2, 0, 1, 1, 2],
    [0, 1, 2, 1, 2],
    [0, 1, 1, 1, 2],
    [1, 1, 1, 1, 2],
    [2, 1, 1, 1, 2],
    [1, 1, 2, 1, 2],
    [0, 2, 1, 1, 2],
    [1, 2, 1, 1, 2],
    [2, 2, 1, 1, 2],
    [2, 1, 2, 1, 2],
    [0, 2, 2, 2, 2],
    [1, 2, 2, 2, 2],
    [2, 2, 2, 2, 2],
    [2, 1, 2, 2, 2],
];

/// Quint-Dekodierung 7 Bit → drei Quintwerte.
const QUINT_DECODE: [[u8; 3]; 128] = [
    [0, 0, 0],
    [1, 0, 0],
    [2, 0, 0],
    [3, 0, 0],
    [4, 0, 0],
    [0, 4, 0],
    [4, 4, 0],
    [4, 4, 4],
    [0, 1, 0],
    [1, 1, 0],
    [2, 1, 0],
    [3, 1, 0],
    [4, 1, 0],
    [1, 4, 0],
    [4, 4, 1],
    [4, 4, 4],
    [0, 2, 0],
    [1, 2, 0],
    [2, 2, 0],
    [3, 2, 0],
    [4, 2, 0],
    [2, 4, 0],
    [4, 4, 2],
    [4, 4, 4],
    [0, 3, 0],
    [1, 3, 0],
    [2, 3, 0],
    [3, 3, 0],
    [4, 3, 0],
    [3, 4, 0],
    [4, 4, 3],
    [4, 4, 4],
    [0, 0, 1],
    [1, 0, 1],
    [2, 0, 1],
    [3, 0, 1],
    [4, 0, 1],
    [0, 4, 1],
    [4, 0, 4],
    [0, 4, 4],
    [0, 1, 1],
    [1, 1, 1],
    [2, 1, 1],
    [3, 1, 1],
    [4, 1, 1],
    [1, 4, 1],
    [4, 1, 4],
    [1, 4, 4],
    [0, 2, 1],
    [1, 2, 1],
    [2, 2, 1],
    [3, 2, 1],
    [4, 2, 1],
    [2, 4, 1],
    [4, 2, 4],
    [2, 4, 4],
    [0, 3, 1],
    [1, 3, 1],
    [2, 3, 1],
    [3, 3, 1],
    [4, 3, 1],
    [3, 4, 1],
    [4, 3, 4],
    [3, 4, 4],
    [0, 0, 2],
    [1, 0, 2],
    [2, 0, 2],
    [3, 0, 2],
    [4, 0, 2],
    [0, 4, 2],
    [2, 0, 4],
    [3, 0, 4],
    [0, 1, 2],
    [1, 1, 2],
    [2, 1, 2],
    [3, 1, 2],
    [4, 1, 2],
    [1, 4, 2],
    [2, 1, 4],
    [3, 1, 4],
    [0, 2, 2],
    [1, 2, 2],
    [2, 2, 2],
    [3, 2, 2],
    [4, 2, 2],
    [2, 4, 2],
    [2, 2, 4],
    [3, 2, 4],
    [0, 3, 2],
    [1, 3, 2],
    [2, 3, 2],
    [3, 3, 2],
    [4, 3, 2],
    [3, 4, 2],
    [2, 3, 4],
    [3, 3, 4],
    [0, 0, 3],
    [1, 0, 3],
    [2, 0, 3],
    [3, 0, 3],
    [4, 0, 3],
    [0, 4, 3],
    [0, 0, 4],
    [1, 0, 4],
    [0, 1, 3],
    [1, 1, 3],
    [2, 1, 3],
    [3, 1, 3],
    [4, 1, 3],
    [1, 4, 3],
    [0, 1, 4],
    [1, 1, 4],
    [0, 2, 3],
    [1, 2, 3],
    [2, 2, 3],
    [3, 2, 3],
    [4, 2, 3],
    [2, 4, 3],
    [0, 2, 4],
    [1, 2, 4],
    [0, 3, 3],
    [1, 3, 3],
    [2, 3, 3],
    [3, 3, 3],
    [4, 3, 3],
    [3, 4, 3],
    [0, 3, 4],
    [1, 3, 4],
];

/// Integer-Sequence-Decodierung. `top` = `false`: Strom aufwärts ab `pos`
/// (Endpunkte); `top` = `true`: Gewichtsstrom von Bit 127 abwärts.
fn decode_bise(block: &Block, range: usize, vals: &mut [u32], pos_start: u32, top: bool) {
    let t = ISE_RANGES[range];
    let bpv = t[0];
    let mut pos = pos_start;
    let read = |pos: &mut u32, n: u32| -> u32 {
        if n == 0 {
            return 0;
        }
        if top {
            block.take_top(pos, n)
        } else {
            block.take(pos, n)
        }
    };
    if t[1] == 1 {
        // Trits: fünf Werte pro Trit-Block, Trit-Felder 2,2,1,2,1 Bit
        const TBITS: [u32; 5] = [2, 2, 1, 2, 1];
        let mut i = 0;
        while i < vals.len() {
            let m = (vals.len() - i).min(5);
            let mut trit = 0u32;
            let mut tofs = 0u32;
            let mut ms = [0u32; 5];
            for c in 0..m {
                if bpv > 0 {
                    ms[c] = read(&mut pos, bpv);
                }
                trit |= read(&mut pos, TBITS[c]) << tofs;
                tofs += TBITS[c];
            }
            let tab = TRIT_DECODE[trit as usize];
            for c in 0..m {
                vals[i + c] = ((tab[c] as u32) << bpv) | ms[c];
            }
            i += m;
        }
    } else if t[2] == 1 {
        // Quints: drei Werte pro Quint-Block, Quint-Felder 3,2,2 Bit
        const QBITS: [u32; 3] = [3, 2, 2];
        let mut i = 0;
        while i < vals.len() {
            let m = (vals.len() - i).min(3);
            let mut quint = 0u32;
            let mut qofs = 0u32;
            let mut ms = [0u32; 3];
            for c in 0..m {
                if bpv > 0 {
                    ms[c] = read(&mut pos, bpv);
                }
                quint |= read(&mut pos, QBITS[c]) << qofs;
                qofs += QBITS[c];
            }
            let tab = QUINT_DECODE[quint as usize];
            for c in 0..m {
                vals[i + c] = ((tab[c] as u32) << bpv) | ms[c];
            }
            i += m;
        }
    } else {
        for slot in vals.iter_mut() {
            *slot = read(&mut pos, bpv);
        }
    }
}

/// Bitreplikation von `from` auf `to` Bits.
const fn bit_replication(val: u32, from: u32, to: u32) -> u32 {
    if from >= to {
        return val & ((1 << to) - 1);
    }
    let mut v = val;
    let mut bits = from;
    while bits < to {
        let take = if to - bits < from { to - bits } else { from };
        v = (v << take) | (val >> (from - take));
        bits += take;
    }
    v & ((1 << to) - 1)
}

/// LDR-Endpunkt-Dequantisierung auf 0..255 (Spezifikationsparameter; gegen
/// beide Referenzen bitgenau verifiziert).
fn dequant_endpoint(val: u32, range: usize) -> u32 {
    let t = ISE_RANGES[range];
    let (num_bits, trit, quint) = (t[0], t[1], t[2]);
    if trit == 0 && quint == 0 {
        return bit_replication(val, num_bits, 8);
    }
    // Zeilenindex der Dequantisierungs-Parameter (Tabelle 103)
    let idx = (num_bits * 2 + quint - 2) as usize;
    let bits = val & ((1 << num_bits) - 1);
    let d = val >> num_bits;
    let a = bits & 1;
    let b = (bits >> 1) & 1;
    let c = (bits >> 2) & 1;
    let dd = (bits >> 3) & 1;
    let e = (bits >> 4) & 1;
    let f = (bits >> 5) & 1;
    let aa = if a != 0 { 0x1FFu32 } else { 0 };
    let bb: u32 = match idx {
        0 | 1 => 0,
        2 => (b << 1) | (b << 2) | (b << 4) | (b << 8),
        3 => (b << 2) | (b << 3) | (b << 8),
        4 => b | (c << 1) | (b << 2) | (c << 3) | (b << 7) | (c << 8),
        5 => c | (b << 1) | (c << 2) | (b << 7) | (c << 8),
        6 => b | (c << 1) | (dd << 2) | (b << 6) | (c << 7) | (dd << 8),
        7 => c | (dd << 1) | (b << 6) | (c << 7) | (dd << 8),
        8 => dd | (e << 1) | (b << 5) | (c << 6) | (dd << 7) | (e << 8),
        9 => e | (b << 5) | (c << 6) | (dd << 7) | (e << 8),
        _ => f | (b << 4) | (c << 5) | (dd << 6) | (e << 7) | (f << 8),
    };
    const CC: [u32; 11] = [204, 113, 93, 54, 44, 26, 22, 13, 11, 6, 5];
    let mut u = d * CC[idx] + bb;
    u ^= aa;
    (aa & 0x80) | (u >> 2)
}

/// Gewichts-Dequantisierung auf 0..64 (Interpolationsskala der Referenzen;
/// die +1-Korrektur über 32 entspricht der aktuellen Spek-Revision).
fn dequant_weight(val: u32, range: usize) -> u32 {
    let t = ISE_RANGES[range];
    let (num_bits, quint) = (t[0], t[2]);
    let mut u: u32 = match range {
        0 => {
            if val != 0 {
                63
            } else {
                0
            }
        }
        1 => [0, 32, 63][val as usize],
        2 => bit_replication(val, 2, 6),
        3 => [0, 16, 32, 47, 63][val as usize],
        5 => bit_replication(val, 3, 6),
        8 => bit_replication(val, 4, 6),
        11 => bit_replication(val, 5, 6),
        4 | 6 | 7 | 9 | 10 => {
            let idx = (num_bits * 2 + quint) as usize;
            let bits = val & ((1 << num_bits) - 1);
            let d = val >> num_bits;
            let a = bits & 1;
            let b = (bits >> 1) & 1;
            let c = (bits >> 2) & 1;
            let aa = if a != 0 { 0x7Fu32 } else { 0 };
            let bb: u32 = match idx {
                4 => (b << 6) | (b << 2) | b,
                5 => (b << 6) | (b << 1),
                _ => (c << 6) | (b << 5) | (c << 1) | b,
            };
            const CT: [u32; 5] = [50, 28, 23, 13, 11];
            let mut u = d * CT[idx - 2] + bb;
            u ^= aa;
            (aa & 0x20) | (u >> 2)
        }
        _ => {
            unreachable!("Gewichtsbereich {range} wurde in unpack_block verworfen")
        }
    };
    if u > 32 {
        u += 1;
    }
    u
}

#[inline]
fn hash52(mut p: u32) -> u32 {
    p ^= p >> 15;
    p = p.wrapping_sub(p << 17);
    p = p.wrapping_add(p << 7);
    p = p.wrapping_add(p << 4);
    p ^= p >> 5;
    p = p.wrapping_add(p << 16);
    p ^= p >> 7;
    p ^= p >> 3;
    p ^= p << 6;
    p ^= p >> 17;
    p
}

/// Partitionsindex eines Texels — `select_partition` der Spezifikation mit
/// `z = 0`; `seed` ist das 10-Bit-Seed-Feld `[22:13]`.
fn select_partition(seed_in: u32, x: u32, y: u32, partitions: usize, small_block: bool) -> usize {
    let (mut x, mut y) = (x, y);
    if small_block {
        x <<= 1;
        y <<= 1;
    }
    let seed = seed_in.wrapping_add(1024 * (partitions as u32 - 1));
    let rnum = hash52(seed);
    let mut s = [0u8; 12];
    let shifts = [0u32, 4, 8, 12, 16, 20, 24, 28, 18, 22, 26, 30];
    for (i, sh) in shifts.iter().enumerate() {
        s[i] = ((rnum >> *sh) & 0xF) as u8;
    }
    s[11] = (((rnum >> 30) | (rnum << 2)) & 0xF) as u8;
    for v in s.iter_mut() {
        *v = v.wrapping_mul(*v);
    }
    let (sh1, sh2);
    if seed & 1 != 0 {
        sh1 = if seed & 2 != 0 { 4 } else { 5 };
        sh2 = if partitions == 3 { 6 } else { 5 };
    } else {
        sh1 = if partitions == 3 { 6 } else { 5 };
        sh2 = if seed & 2 != 0 { 4 } else { 5 };
    }
    let sh3 = if seed & 0x10 != 0 { sh1 } else { sh2 };
    let _ = sh3; // wird nur für die (hier entfallenden) z-Terme benötigt
    let g = |i: usize, sh: u32| (s[i] >> sh) as u32;
    // z = 0, daher entfallen die seed9..seed12-Multiplikationsterme.
    let a = (g(0, sh1) * x + g(1, sh2) * y + (rnum >> 14)) & 0x3F;
    let b = (g(2, sh1) * x + g(3, sh2) * y + (rnum >> 10)) & 0x3F;
    let mut c = (g(4, sh1) * x + g(5, sh2) * y + (rnum >> 6)) & 0x3F;
    let mut d = (g(6, sh1) * x + g(7, sh2) * y + (rnum >> 2)) & 0x3F;
    if partitions < 4 {
        d = 0;
    }
    if partitions < 3 {
        c = 0;
    }
    if a >= b && a >= c && a >= d {
        0
    } else if b >= c && b >= d {
        1
    } else if c >= d {
        2
    } else {
        3
    }
}

/// Zehn 2D-Blockmodus-Zeilen der Spezifikation (Tabelle „2D block modes“):
/// (DualPlane-Bitposition, P-Bitposition, W-Position, W-Bits, H-Position,
/// H-Bits, W-Bias, H-Bias, rho-Bitpositionen p0, p1, p2). `-1` = Feld fehlt.
const DEC_ROWS: [(i32, i32, i32, u32, i32, u32, u32, u32, i32, i32, i32); 10] = [
    (10, 9, 7, 2, 5, 2, 4, 2, 4, 0, 1), // 4 2
    (10, 9, 7, 2, 5, 2, 8, 2, 4, 0, 1), // 8 2
    (10, 9, 5, 2, 7, 2, 2, 8, 4, 0, 1), // 2 8
    (10, 9, 5, 2, 7, 1, 2, 6, 4, 0, 1), // 2 6
    (10, 9, 7, 1, 5, 2, 2, 2, 4, 0, 1), // 2 2
    (10, 9, 0, 0, 5, 2, 12, 2, 4, 2, 3), // 12 2
    (10, 9, 5, 2, 0, 0, 2, 12, 4, 2, 3), // 2 12
    (10, 9, 0, 0, 0, 0, 6, 10, 4, 2, 3), // 6 10
    (10, 9, 0, 0, 0, 0, 10, 6, 4, 2, 3), // 10 6
    (-1, -1, 5, 2, 9, 2, 6, 6, 4, 2, 3), // 6 6
];

#[inline]
const fn is_hdr_cem(cem: u32) -> bool {
    matches!(cem, 2 | 3 | 7 | 11 | 14 | 15)
}

/// Anzahl der ISE-kodierten Endpunkt-Werte pro CEM.
#[inline]
const fn cem_values(cem: u32) -> usize {
    2 + 2 * (cem >> 2) as usize
}

#[derive(Clone, Copy, Debug, Default)]
struct Endpoints {
    e0: [i32; 4],
    e1: [i32; 4],
}

fn blue_contract(r: i32, g: i32, b: i32, a: i32) -> [i32; 4] {
    [(r + b) >> 1, (g + b) >> 1, b, a]
}

/// Dekodiert das Basis-Delta in den Basiswert zurück (Referenz-
/// `bit_transfer_signed` als Rechenwertversion).
#[inline]
fn bts(a: i32, b: i32) -> (i32, i32) {
    let b = (b >> 1) | (a & 0x80);
    let mut a = (a >> 1) & 0x3F;
    if a & 0x20 != 0 {
        a -= 0x40;
    }
    (a, b)
}

/// Vorzeichenerweiterung eines `bits`-stelligen Zweierkomplementwerts.
#[inline]
fn sign_extend(v: i32, bits: i32) -> i32 {
    if v & (1 << (bits - 1)) != 0 {
        v | !((1 << bits) - 1)
    } else {
        v & ((1 << bits) - 1)
    }
}

#[inline]
fn clamp_unorm8_pair(ep: &mut Endpoints) {
    for v in ep.e0.iter_mut().chain(ep.e1.iter_mut()) {
        *v = (*v).clamp(0, 0xFF);
    }
}

/// Dekodiert die beiden Endpunkte einer Partition aus den dequantisierten
/// Werten (0..255). HDR-Moden liefern 12-Bit-Werte (0..0xFFF), bei Modus 14
/// ist Alpha 8-Bit-unorm, bei Modus 15 HDR-Alpha.
fn decode_cem(cem: u32, v: &[u32]) -> Endpoints {
    let vv = |i: usize| v[i] as i32;
    match cem {
        0 => Endpoints { e0: [vv(0), vv(0), vv(0), 0xFF], e1: [vv(1), vv(1), vv(1), 0xFF] },
        1 => {
            let l0 = (vv(0) >> 2) | (vv(1) & 0xC0);
            let mut l1 = l0 + (vv(1) & 0x3F);
            if l1 > 0xFF {
                l1 = 0xFF;
            }
            Endpoints { e0: [l0, l0, l0, 0xFF], e1: [l1, l1, l1, 0xFF] }
        }
        4 => Endpoints { e0: [vv(0), vv(0), vv(0), vv(2)], e1: [vv(1), vv(1), vv(1), vv(3)] },
        5 => {
            let (a, c) = bts(vv(1), vv(0));
            let (b, d) = bts(vv(3), vv(2));
            let mut ep = Endpoints { e0: [c, c, c, d], e1: [c + a, c + a, c + a, d + b] };
            clamp_unorm8_pair(&mut ep);
            ep
        }
        6 => Endpoints {
            e0: [(vv(0) * vv(3)) >> 8, (vv(1) * vv(3)) >> 8, (vv(2) * vv(3)) >> 8, 0xFF],
            e1: [vv(0), vv(1), vv(2), 0xFF],
        },
        8 => {
            let s0 = vv(0) + vv(2) + vv(4);
            let s1 = vv(1) + vv(3) + vv(5);
            if s1 >= s0 {
                Endpoints {
                    e0: [vv(0), vv(2), vv(4), 0xFF],
                    e1: [vv(1), vv(3), vv(5), 0xFF],
                }
            } else {
                Endpoints {
                    e0: blue_contract(vv(1), vv(3), vv(5), 0xFF),
                    e1: blue_contract(vv(0), vv(2), vv(4), 0xFF),
                }
            }
        }
        9 => {
            let (x1, x0) = bts(vv(1), vv(0));
            let (x3, x2) = bts(vv(3), vv(2));
            let (x5, x4) = bts(vv(5), vv(4));
            let mut ep = if x1 + x3 + x5 >= 0 {
                Endpoints {
                    e0: [x0, x2, x4, 0xFF],
                    e1: [x0 + x1, x2 + x3, x4 + x5, 0xFF],
                }
            } else {
                Endpoints {
                    e0: blue_contract(x0 + x1, x2 + x3, x4 + x5, 0xFF),
                    e1: blue_contract(x0, x2, x4, 0xFF),
                }
            };
            clamp_unorm8_pair(&mut ep);
            ep
        }
        10 => Endpoints {
            e0: [(vv(0) * vv(3)) >> 8, (vv(1) * vv(3)) >> 8, (vv(2) * vv(3)) >> 8, vv(4)],
            e1: [vv(0), vv(1), vv(2), vv(5)],
        },
        12 => {
            let s0 = vv(0) + vv(2) + vv(4);
            let s1 = vv(1) + vv(3) + vv(5);
            if s1 >= s0 {
                Endpoints {
                    e0: [vv(0), vv(2), vv(4), vv(6)],
                    e1: [vv(1), vv(3), vv(5), vv(7)],
                }
            } else {
                Endpoints {
                    e0: blue_contract(vv(1), vv(3), vv(5), vv(7)),
                    e1: blue_contract(vv(0), vv(2), vv(4), vv(6)),
                }
            }
        }
        13 => {
            let (x1, x0) = bts(vv(1), vv(0));
            let (x3, x2) = bts(vv(3), vv(2));
            let (x5, x4) = bts(vv(5), vv(4));
            let (x7, x6) = bts(vv(7), vv(6));
            let mut ep = if x1 + x3 + x5 >= 0 {
                Endpoints {
                    e0: [x0, x2, x4, x6],
                    e1: [x0 + x1, x2 + x3, x4 + x5, x6 + x7],
                }
            } else {
                Endpoints {
                    e0: blue_contract(x0 + x1, x2 + x3, x4 + x5, x6 + x7),
                    e1: blue_contract(x0, x2, x4, x6),
                }
            };
            clamp_unorm8_pair(&mut ep);
            ep
        }
        2 => {
            let (y0, y1);
            if vv(1) >= vv(0) {
                y0 = vv(0) << 4;
                y1 = vv(1) << 4;
            } else {
                y0 = (vv(1) << 4) + 8;
                y1 = (vv(0) << 4) - 8;
            }
            Endpoints { e0: [y0, y0, y0, 0x780], e1: [y1, y1, y1, 0x780] }
        }
        3 => {
            let (y0, d);
            if v[0] & 0x80 != 0 {
                y0 = ((v[1] & 0xE0) << 4) | ((v[0] & 0x7F) << 2);
                d = (v[1] & 0x1F) << 2;
            } else {
                y0 = ((v[1] & 0xF0) << 4) | ((v[0] & 0x7F) << 1);
                d = (v[1] & 0x0F) << 1;
            }
            let y0 = y0 as i32;
            let mut y1 = y0 + d as i32;
            if y1 > 0xFFF {
                y1 = 0xFFF;
            }
            Endpoints { e0: [y0, y0, y0, 0x780], e1: [y1, y1, y1, 0x780] }
        }
        7 => decode_hdr_mode7(v),
        11 => decode_hdr_mode11(v),
        14 => {
            // RGB wie Modus 11, Alpha als 8-Bit-UNORM-Werte
            let mut ep = decode_hdr_mode11(v);
            ep.e0[3] = vv(6);
            ep.e1[3] = vv(7);
            ep
        }
        15 => {
            // RGB wie Modus 11, Alpha HDR mit eigenem 2-Bit-Modusfeld
            let mut ep = decode_hdr_mode11(v);
            let mode = ((v[6] >> 7) & 1) | ((v[7] >> 6) & 2);
            let mut v6 = (v[6] & 0x7F) as i32;
            let mut v7 = (v[7] & 0x7F) as i32;
            if mode == 3 {
                ep.e0[3] = v6 << 5;
                ep.e1[3] = v7 << 5;
            } else {
                v6 |= (v7 << (mode + 1)) & 0x780;
                v7 &= (0x3F >> mode) as i32;
                v7 ^= (0x20 >> mode) as i32;
                v7 -= (0x20 >> mode) as i32;
                v6 = ((v6 as u32) << (4 - mode)) as i32;
                v7 = ((v7 as u32) << (4 - mode)) as i32;
                v7 += v6;
                ep.e0[3] = v6;
                ep.e1[3] = v7.clamp(0, 0xFFF);
            }
            ep
        }
        _ => Endpoints::default(),
    }
}

/// HDR-Endpunkt-Modus 7: Base+Scale mit verteilten Extra-Bits.
fn decode_hdr_mode7(v: &[u32]) -> Endpoints {
    let modeval = ((v[0] & 0xC0) >> 6) | ((v[1] & 0x80) >> 5) | ((v[2] & 0x80) >> 4);
    let (majcomp, mode);
    if modeval & 0xC != 0xC {
        majcomp = (modeval >> 2) as usize;
        mode = (modeval & 3) as usize;
    } else if modeval != 0xF {
        majcomp = (modeval & 3) as usize;
        mode = 4;
    } else {
        majcomp = 0;
        mode = 5;
    }
    let mut red = (v[0] & 0x3F) as i32;
    let mut green = (v[1] & 0x1F) as i32;
    let mut blue = (v[2] & 0x1F) as i32;
    let mut scale = (v[3] & 0x1F) as i32;
    let x0 = ((v[1] >> 6) & 1) as i32;
    let x1 = ((v[1] >> 5) & 1) as i32;
    let x2 = ((v[2] >> 6) & 1) as i32;
    let x3 = ((v[2] >> 5) & 1) as i32;
    let x4 = ((v[3] >> 7) & 1) as i32;
    let x5 = ((v[3] >> 6) & 1) as i32;
    let x6 = ((v[3] >> 5) & 1) as i32;
    let ohm = 1i32 << mode;
    if ohm & 0x30 != 0 {
        green |= x0 << 6;
    }
    if ohm & 0x3A != 0 {
        green |= x1 << 5;
    }
    if ohm & 0x30 != 0 {
        blue |= x2 << 6;
    }
    if ohm & 0x3A != 0 {
        blue |= x3 << 5;
    }
    if ohm & 0x3D != 0 {
        scale |= x6 << 5;
    }
    if ohm & 0x2D != 0 {
        scale |= x5 << 6;
    }
    if ohm & 0x04 != 0 {
        scale |= x4 << 7;
    }
    if ohm & 0x3B != 0 {
        red |= x4 << 6;
    }
    if ohm & 0x04 != 0 {
        red |= x3 << 6;
    }
    if ohm & 0x10 != 0 {
        red |= x5 << 7;
    }
    if ohm & 0x0F != 0 {
        red |= x2 << 7;
    }
    if ohm & 0x05 != 0 {
        red |= x1 << 8;
    }
    if ohm & 0x0A != 0 {
        red |= x0 << 8;
    }
    if ohm & 0x05 != 0 {
        red |= x0 << 9;
    }
    if ohm & 0x02 != 0 {
        red |= x6 << 9;
    }
    if ohm & 0x01 != 0 {
        red |= x3 << 10;
    }
    if ohm & 0x02 != 0 {
        red |= x5 << 10;
    }
    const SHAMTS: [i32; 6] = [1, 1, 2, 3, 4, 5];
    let sh = SHAMTS[mode];
    red <<= sh;
    green <<= sh;
    blue <<= sh;
    scale <<= sh;
    if mode != 5 {
        green = red - green;
        blue = red - blue;
    }
    if majcomp == 1 {
        std::mem::swap(&mut red, &mut green);
    }
    if majcomp == 2 {
        std::mem::swap(&mut red, &mut blue);
    }
    Endpoints {
        e1: [red, green, blue, 0x780].map(|x| x.clamp(0, 0xFFF)),
        e0: [red - scale, green - scale, blue - scale, 0x780].map(|x| x.clamp(0, 0xFFF)),
    }
}

/// HDR-Endpunkt-Modus 11 und die RGB-Anteile von 14/15 (Alpha dort später
/// überschrieben bzw. auf den Neutralwert 0x780 vorbelegt).
fn decode_hdr_mode11(v: &[u32]) -> Endpoints {
    let majcomp = ((v[4] & 0x80) >> 7) | ((v[5] & 0x80) >> 6);
    if majcomp == 3 {
        return Endpoints {
            e0: [(v[0] << 4) as i32, (v[2] << 4) as i32, ((v[4] & 0x7F) << 5) as i32, 0x780],
            e1: [(v[1] << 4) as i32, (v[3] << 4) as i32, ((v[5] & 0x7F) << 5) as i32, 0x780],
        };
    }
    let mode = ((v[1] & 0x80) >> 7) | ((v[2] & 0x80) >> 6) | ((v[3] & 0x80) >> 5);
    let mut va = (v[0] | ((v[1] & 0x40) << 2)) as i32;
    let mut vb0 = (v[2] & 0x3F) as i32;
    let mut vb1 = (v[3] & 0x3F) as i32;
    let mut vc = (v[1] & 0x3F) as i32;
    const DBITS: [i32; 8] = [7, 6, 7, 6, 5, 6, 5, 6];
    let mut vd0 = sign_extend((v[4] & 0x7F) as i32, DBITS[mode as usize]);
    let mut vd1 = sign_extend((v[5] & 0x7F) as i32, DBITS[mode as usize]);
    let x0 = ((v[2] >> 6) & 1) as i32;
    let x1 = ((v[3] >> 6) & 1) as i32;
    let x2 = ((v[4] >> 6) & 1) as i32;
    let x3 = ((v[5] >> 6) & 1) as i32;
    let x4 = ((v[4] >> 5) & 1) as i32;
    let x5 = ((v[5] >> 5) & 1) as i32;
    let ohm = 1i32 << mode;
    if ohm & 0xA4 != 0 {
        va |= x0 << 9;
    }
    if ohm & 0x08 != 0 {
        va |= x2 << 9;
    }
    if ohm & 0x50 != 0 {
        va |= x4 << 9;
    }
    if ohm & 0x50 != 0 {
        va |= x5 << 10;
    }
    if ohm & 0xA0 != 0 {
        va |= x1 << 10;
    }
    if ohm & 0xC0 != 0 {
        va |= x2 << 11;
    }
    if ohm & 0x04 != 0 {
        vc |= x1 << 6;
    }
    if ohm & 0xE8 != 0 {
        vc |= x3 << 6;
    }
    if ohm & 0x20 != 0 {
        vc |= x2 << 7;
    }
    if ohm & 0x5B != 0 {
        vb0 |= x0 << 6;
    }
    if ohm & 0x5B != 0 {
        vb1 |= x1 << 6;
    }
    if ohm & 0x12 != 0 {
        vb0 |= x2 << 7;
    }
    if ohm & 0x12 != 0 {
        vb1 |= x3 << 7;
    }
    let shamt = ((mode >> 1) ^ 3) as i32;
    va = ((va as u32) << shamt) as i32;
    vb0 = ((vb0 as u32) << shamt) as i32;
    vb1 = ((vb1 as u32) << shamt) as i32;
    vc = ((vc as u32) << shamt) as i32;
    vd0 = ((vd0 as u32) << shamt) as i32;
    vd1 = ((vd1 as u32) << shamt) as i32;
    let mut ep = Endpoints {
        e1: [va, va - vb0, va - vb1, 0x780].map(|x| x.clamp(0, 0xFFF)),
        e0: [va - vc, va - vb0 - vc - vd0, va - vb1 - vc - vd1, 0x780].map(|x| x.clamp(0, 0xFFF)),
    };
    if majcomp == 1 {
        ep.e0.swap(0, 1);
        ep.e1.swap(0, 1);
    } else if majcomp == 2 {
        ep.e0.swap(0, 2);
        ep.e1.swap(0, 2);
    }
    ep
}

/// FP16-Konvertierung mit Rundung „Richtung Null“, exakt die Konvertierung
/// der Referenzen (entspricht Intel-F32TO16 mit RTZ) für Werte < 1.0.
fn f16_toward_zero(v: f32) -> u16 {
    let bits = v.to_bits() as i32;
    let flt_m = bits & 0x7F_FF_FFFF & 0x7F_FFFF;
    let flt_e = ((bits >> 23) & 0xFF) as i32;
    let (mut e, mut m) = (0i32, 0i32);
    if flt_e == 0xFF {
        e = 31;
        if flt_m != 0 {
            m = 1;
        }
    } else if flt_e != 0 {
        let new_exp = flt_e - 127;
        if new_exp > 15 {
            e = 31;
        } else if new_exp < -14 {
            m = (v.abs() * 16_777_216.0f32) as i32;
        } else {
            e = new_exp + 15;
            m = (flt_m as f32 * (1.0f32 / 8192.0f32)) as i32;
        }
    }
    if m == 1024 {
        e += 1;
        m = 0;
    }
    ((e << 10) | m) as u16
}

#[inline]
fn f16_from_ldr_unorm16(k: u32) -> u16 {
    if k == 0xFFFF {
        0x3C00 // 1.0
    } else {
        f16_toward_zero(k as f32 * (1.0f32 / 65536.0f32))
    }
}

/// Pseudo-Log-Interpolationswert (16 Bit) → FP16 gemäß Spezifikation.
#[inline]
fn qlog16_to_half(k: i32) -> u16 {
    let e = (k & 0xF800) >> 11;
    let m = k & 0x7FF;
    let mt = if m < 512 {
        3 * m
    } else if m >= 1536 {
        5 * m - 2048
    } else {
        4 * m - 512
    };
    ((e << 10) + (mt >> 3)) as u16
}

#[inline]
const fn is_f16_inf_or_nan(h: u16) -> bool {
    (h & 0x7C00) == 0x7C00
}

/// Metadaten eines ASTC-Vulkanformats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AstcFormat {
    /// Blockbreite in Texel.
    pub block_w: usize,
    /// Blockhöhe in Texel.
    pub block_h: usize,
    /// `true` für HDR-Profile (FP16-Ausgabe).
    pub hdr: bool,
    /// `true` für SRGB-Varianten (Interpolationspfad mit `| 0x80`).
    pub srgb: bool,
}

/// Kanonische ASTC-2D-Blockgrößen in Vulkan-Zählreihenfolge.
const DIMS: [(usize, usize); 14] = [
    (4, 4),
    (5, 4),
    (5, 5),
    (6, 5),
    (6, 6),
    (8, 5),
    (8, 6),
    (8, 8),
    (10, 5),
    (10, 6),
    (10, 8),
    (10, 10),
    (12, 10),
    (12, 12),
];

/// Ordnet eine Vulkanformatnummer (`VK_FORMAT_ASTC_*` = 157–184) einem
/// ASTC-Format zu. Vulkan zählt je Blockgröße UNORM, dann SRGB; HDR-ASTC
/// (FP16-Ausgabe) hat keine Vulkannummer und bleibt interne Sache
/// (z. B. Validierungs-Helfer).
pub fn astc_format(vk: u32) -> Option<AstcFormat> {
    let off = vk.checked_sub(157)? as usize;
    if off >= DIMS.len() * 2 {
        return None;
    }
    let (block_w, block_h) = DIMS[off / 2];
    Some(AstcFormat { block_w, block_h, hdr: false, srgb: off % 2 == 1 })
}

/// Block FOOTPRINT (Texel je Block) einer ASTC-Vulkanformatnummer — für
/// Byteschnittberechnungen im Container.
pub fn block_dims(vk: u32) -> Option<(usize, usize)> {
    astc_format(vk).map(|f| (f.block_w, f.block_h))
}

/// Bytes pro Texel im Ausgabeformat (RGBA8 bzw. RGBA-FP16).
#[inline]
pub fn bytes_per_texel(fmt: AstcFormat) -> usize {
    if fmt.hdr {
        8
    } else {
        4
    }
}

/// Blockgröße in Bytes (immer 128 Bit).
pub const BLOCK_BYTES: usize = 16;

/// Aus einem physischen Block gewonnene logische Werte.
struct LogicalBlock {
    block_w: usize,
    block_h: usize,
    grid_w: usize,
    grid_h: usize,
    weight_range: usize,
    endpoint_range: usize,
    dual_plane: bool,
    ccs: usize,
    partitions: usize,
    seed: u32,
    cems: [u32; 4],
    /// ISE-Symbole der Endpunkte (nach Level-Prüfung dequantisiert)
    endpoints: [u32; MAX_ENDPOINT_VALUES],
    total_cem_values: usize,
    /// ISE-Symbole der Gewichte; bei Dual-Plane sind die Ebenen je
    /// Rasterposition verschränkt
    weights: [u32; MAX_GRID_WEIGHTS * 2],
    total_weights: usize,
}

enum BlockContent {
    /// Illegal kodierter Block: Norm-Fehlerfarbe.
    Error,
    /// Void-Extent: Direktfarbe je Kanal (UNORM16 bzw. FP16-Bits).
    Void { solid: [u16; 4], hdr: bool },
    Normal(Box<LogicalBlock>),
}

fn decode_config(block: &Block, bw: usize, bh: usize) -> BlockContent {
    // Reservierte und 3D-Blockmodi ablehnen (identisch zur Referenz).
    if block.bits(0, 4) == 0 {
        return BlockContent::Error;
    }
    if block.bits(0, 2) == 0 && block.bits(6, 3) == 0b111 && block.bits(2, 4) != 0b1111 {
        return BlockContent::Error;
    }
    // Void-Extent 2D: Blockmodus 0b111111100 in Bits [8:0]
    if block.bits(0, 9) == 0b111111100 {
        // Bits 10 und 11 sind reserviert und müssen 1 sein.
        if block.bits(10, 2) != 0b11 {
            return BlockContent::Error;
        }
        let mut pos = 12u32;
        let min_s = block.take(&mut pos, 13);
        let max_s = block.take(&mut pos, 13);
        let min_t = block.take(&mut pos, 13);
        let max_t = block.take(&mut pos, 13);
        let all_ones = min_s == 0x1FFF && max_s == 0x1FFF && min_t == 0x1FFF && max_t == 0x1FFF;
        if !all_ones && (min_s >= max_s || min_t >= max_t) {
            return BlockContent::Error;
        }
        let hdr = block.bits(9, 1) != 0;
        let mut solid = [0u16; 4];
        for c in 0..4 {
            solid[c] = block.bits(64 + 16 * c as u32, 16) as u16;
        }
        if hdr && solid.iter().any(|h| is_f16_inf_or_nan(*h)) {
            return BlockContent::Error;
        }
        return BlockContent::Void { solid, hdr };
    }
    // 2D-Blockmodus-Zeile bestimmen
    let x0_2 = block.bits(0, 2);
    let x2_2 = block.bits(2, 2);
    let x5_4 = block.bits(5, 4);
    let x8_1 = block.bits(8, 1);
    let x7_2 = block.bits(7, 2);
    let row_index: usize = if x0_2 == 0 {
        if x7_2 == 0b00 {
            5
        } else if x7_2 == 0b01 {
            6
        } else if x5_4 == 0b1100 {
            7
        } else if x5_4 == 0b1101 {
            8
        } else if x7_2 == 0b10 {
            9
        } else {
            return BlockContent::Error;
        }
    } else if x2_2 == 0b00 {
        0
    } else if x2_2 == 0b01 {
        1
    } else if x2_2 == 0b10 {
        2
    } else if x8_1 == 0 {
        3
    } else {
        4
    };
    let r = &DEC_ROWS[row_index];
    let p = r.1 >= 0 && block.bits(r.1 as u32, 1) != 0;
    let dp = r.0 >= 0 && block.bits(r.0 as u32, 1) != 0;
    let mut w = r.6;
    let mut h = r.7;
    if r.3 > 0 {
        w += block.bits(r.2 as u32, r.3);
    }
    if r.5 > 0 {
        h += block.bits(r.4 as u32, r.5);
    }
    let pval = block.bits(r.8 as u32, 1)
        | (block.bits(r.9 as u32, 1) << 1)
        | (block.bits(r.10 as u32, 1) << 2);
    // rho 0..1 sind nicht definiert
    if pval < 2 {
        return BlockContent::Error;
    }
    let weight_range = ((pval - 2) + if p { 6 } else { 0 }) as usize;
    if weight_range > LAST_WEIGHT_RANGE {
        return BlockContent::Error;
    }
    BlockContent::Normal(Box::new(LogicalBlock {
        block_w: bw,
        block_h: bh,
        grid_w: w as usize,
        grid_h: h as usize,
        weight_range,
        endpoint_range: FIRST_ENDPOINT_RANGE,
        dual_plane: dp,
        ccs: 0,
        partitions: 1,
        seed: 0,
        cems: [0; 4],
        endpoints: [0; MAX_ENDPOINT_VALUES],
        total_cem_values: 0,
        weights: [0; MAX_GRID_WEIGHTS * 2],
        total_weights: 0,
    }))
}

/// Entpackt einen physischen in einen logischen Block. `BlockContent::Error`
/// bedeutet „ illegal according to the specification or decoding“ bzw. eine
/// Verletzung der verbotenen Kodierungen.
fn unpack_block(block: &Block, bw: usize, bh: usize) -> BlockContent {
    let content = decode_config(block, bw, bh);
    let mut content = match content {
        BlockContent::Normal(_) => content,
        other => return other,
    };
    let bd = match &mut content {
        BlockContent::Normal(bd) => bd.as_mut(),
        _ => unreachable!(),
    };
    let total_weights = bd.grid_w * bd.grid_h * if bd.dual_plane { 2 } else { 1 };
    // Gewichtsgrid darf den Block nicht überragen (Fehlerfarbe, wie die
    // Referenz in decode_block).
    if bd.grid_w > bw || bd.grid_h > bh {
        return BlockContent::Error;
    }
    let total_weight_bits = ise_sequence_bits(total_weights, bd.weight_range);
    // Verbotene Kodierungen (Spezifikation „Illegal Encodings“)
    if total_weights == 0
        || total_weights > MAX_GRID_WEIGHTS
        || total_weight_bits < 24
        || total_weight_bits > 96
    {
        return BlockContent::Error;
    }
    let end_of_weight_bits = 128 - total_weight_bits;

    // CEM-Feld und Partitionen
    let n = (block.bits(11, 2) + 1) as usize;
    bd.partitions = n;
    let mut extra_cem_bits = 0usize;
    if n == 1 {
        bd.cems[0] = block.bits(13, 4);
    } else {
        if bd.dual_plane && n == 4 {
            return BlockContent::Error;
        }
        bd.seed = block.bits(13, 10);
        let mut cem_bits = block.bits(23, 6);
        if cem_bits & 3 == 0 {
            // Alle CEMs gleich (niederwertigster Teil des Bitsatzes)
            let shared = cem_bits >> 2;
            for i in 0..n {
                bd.cems[i] = shared;
            }
        } else {
            // CEMs verschieden, aber in maximal zwei benachbarten Klassen
            let first_class = ((cem_bits & 3) - 1) * 4;
            extra_cem_bits = 3 * n - 4;
            if total_weight_bits + extra_cem_bits > 128 {
                return BlockContent::Error;
            }
            let mut cpos = (end_of_weight_bits - extra_cem_bits) as u32;
            cem_bits >>= 2;
            let mut c = [0u32; 4];
            for i in 0..n {
                c[i] = cem_bits & 1;
                cem_bits >>= 1;
            }
            let mut m = [0u32; 4];
            match n {
                2 => {
                    m[0] = cem_bits & 3;
                    m[1] = block.take(&mut cpos, 2);
                }
                3 => {
                    m[0] = (cem_bits & 1) | (block.take(&mut cpos, 1) << 1);
                    m[1] = block.take(&mut cpos, 2);
                    m[2] = block.take(&mut cpos, 2);
                }
                _ => {
                    for i in 0..4 {
                        m[i] = block.take(&mut cpos, 2);
                    }
                }
            }
            debug_assert_eq!(cpos as usize, end_of_weight_bits);
            for i in 0..n {
                bd.cems[i] = first_class + c[i] * 4 + m[i];
            }
        }
    }
    if n > 1 && bd.seed >= 1024 {
        return BlockContent::Error;
    }
    for i in 0..n {
        if bd.cems[i] > 15 {
            return BlockContent::Error;
        }
    }
    // CCS direkt unterhalb etwaiger Extra-CEM-Bits
    let mut total_extra = extra_cem_bits;
    if bd.dual_plane {
        total_extra += 2;
        if total_extra > end_of_weight_bits {
            return BlockContent::Error;
        }
        let ccs_pos = (end_of_weight_bits - total_extra) as u32;
        bd.ccs = block.bits(ccs_pos, 2) as usize;
    }
    // Datenstromgrenzen
    let mut total_cem_values = 0usize;
    for i in 0..n {
        total_cem_values += cem_values(bd.cems[i]);
    }
    if total_cem_values > MAX_ENDPOINT_VALUES {
        return BlockContent::Error;
    }
    let config_bits = 13 + if n == 1 { 4 } else { 10 + 6 };
    let Some(remaining) = 128usize
        .checked_sub(config_bits + total_extra)
        .and_then(|r| r.checked_sub(total_weight_bits))
    else {
        return BlockContent::Error;
    };
    // Größter Endpunkt-ISE-Bereich, der in die Restbits passt
    let mut endpoint_range = None;
    for k in (FIRST_ENDPOINT_RANGE..=LAST_ENDPOINT_RANGE).rev() {
        if ise_sequence_bits(total_cem_values, k) <= remaining {
            endpoint_range = Some(k);
            break;
        }
    }
    let Some(endpoint_range) = endpoint_range else {
        return BlockContent::Error; // [0..5] ist Mindestkodierung für Endpunkte
    };
    bd.endpoint_range = endpoint_range;
    bd.total_cem_values = total_cem_values;
    bd.total_weights = total_weights;
    // Endpunkte vorwärts, Gewichte rückwärts dekodieren
    decode_bise(
        block,
        endpoint_range,
        &mut bd.endpoints[..total_cem_values],
        config_bits as u32,
        false,
    );
    decode_bise(block, bd.weight_range, &mut bd.weights[..total_weights], 0, true);
    // ISE-Symbole können durch Trit-/Quint-Überlappung über die Stufenzahl
    // hinauslaufen — solche Blöcke sind illegal.
    let ep_levels = ise_levels(endpoint_range);
    if bd.endpoints[..total_cem_values]
        .iter()
        .any(|v| *v as usize >= ep_levels)
    {
        return BlockContent::Error;
    }
    let w_levels = ise_levels(bd.weight_range);
    if bd.weights[..total_weights].iter().any(|v| *v as usize >= w_levels) {
        return BlockContent::Error;
    }
    content
}

/// Bilinear-Taps je Texelposition berechnen (Infill-Tabelle der
/// Spezifikation). Für Grid == Blockauflösung wird kopiert (Span = 0).
struct Tap {
    base: usize,
    col_span: usize,
    w: [u32; 4], // w00, w01, w10, w11
}

fn make_taps(grid_w: usize, grid_h: usize, bw: usize, bh: usize) -> Vec<Tap> {
    let mut taps = Vec::with_capacity(bw * bh);
    if grid_w == bw && grid_h == bh {
        for y in 0..bh {
            for x in 0..bw {
                taps.push(Tap { base: x + y * grid_w, col_span: 0, w: [64, 0, 0, 0] });
            }
        }
        return taps;
    }
    let ds_x = (1024 + bw / 2) / (bw - 1);
    let ds_y = (1024 + bh / 2) / (bh - 1);
    for y in 0..bh {
        for x in 0..bw {
            let gx = ((ds_x * x * (grid_w - 1) + 32) >> 6) as u32;
            let gy = ((ds_y * y * (grid_h - 1) + 32) >> 6) as u32;
            let (jx, fx) = ((gx >> 4) as usize, gx & 0xF);
            let (jy, fy) = ((gy >> 4) as usize, gy & 0xF);
            let w11 = (fx * fy + 8) >> 4;
            let w10 = fy - w11;
            let w01 = fx - w11;
            let w00 = 16 + w11 - fx - fy; // Umgestellt: Zwischenwerte bleiben ≥ 0
            taps.push(Tap { base: jx + jy * grid_w, col_span: grid_w, w: [w00, w01, w10, w11] });
        }
    }
    taps
}

/// Gewicht (0..64) eines Texels aus dem Gitter interpolieren. Indizes
/// laufen wie in den Referenzen linear über das Gitter (Zeilen-Wrapper).
#[inline]
fn sample_weight(tap: &Tap, plane: &[u32]) -> u32 {
    if tap.col_span == 0 {
        return plane[tap.base];
    }
    let [w00, w01, w10, w11] = tap.w;
    let mut total = 8u32;
    if w00 != 0 {
        total += plane[tap.base] * w00;
    }
    if w01 != 0 {
        total += plane[tap.base + 1] * w01;
    }
    if w10 != 0 {
        total += plane[tap.base + tap.col_span] * w10;
    }
    if w11 != 0 {
        total += plane[tap.base + tap.col_span + 1] * w11;
    }
    total >> 4
}

#[inline]
fn weight_interpolate(l: i32, h: i32, w: u32) -> i32 {
    (l * (64 - w as i32) + h * w as i32 + 32) >> 6
}

/// Fehlerfarbe für den gesamten Block.
fn write_error(out: &mut [u8], hdr: bool) {
    if hdr {
        for px in out.chunks_exact_mut(8) {
            px.copy_from_slice(&[0x00, 0x3C, 0x00, 0x00, 0x00, 0x3C, 0x00, 0x3C]);
        }
    } else {
        for px in out.chunks_exact_mut(4) {
            px.copy_from_slice(&[255, 0, 255, 255]);
        }
    }
}

/// Dekodiert einen logischen Block in den Texelblock-Puffer `out`
/// (`block_w * block_h * bytes_per_texel`). Gibt `false` zurück, wenn
/// wenigstens ein Texel die Fehlerfarbe erhalten hat.
fn decode_block(content: &BlockContent, fmt: AstcFormat, out: &mut [u8]) -> bool {
    let hdr = fmt.hdr;
    match content {
        BlockContent::Error => {
            write_error(out, hdr);
            return false;
        }
        BlockContent::Void { solid, hdr: void_hdr } => {
            if *void_hdr == hdr {
                // HDR-Block in HDR-Pfad: Rohwerte, LDR-Block in LDR-Pfad:
                // oberes Byte.
                for px in out.chunks_exact_mut(if hdr { 8 } else { 4 }) {
                    for c in 0..4 {
                        if hdr {
                            px[c * 2..c * 2 + 2].copy_from_slice(&solid[c].to_le_bytes());
                        } else {
                            px[c] = (solid[c] >> 8) as u8;
                        }
                    }
                }
            } else if hdr {
                // LDR-Farbe im HDR-Pfad: UNORM16 → FP16 (RTZ)
                let h = solid.map(|k| f16_from_ldr_unorm16(k as u32));
                for px in out.chunks_exact_mut(8) {
                    for c in 0..4 {
                        px[c * 2..c * 2 + 2].copy_from_slice(&h[c].to_le_bytes());
                    }
                }
            } else {
                // HDR-Farbe im LDR-Pfad: kompletter Block Fehlerfarbe
                write_error(out, false);
                return false;
            }
            return true;
        }
        BlockContent::Normal(bd) => {
            let (bw, bh) = (bd.block_w, bd.block_h);            // Endpunkte dequantisieren
            let mut ep = [0u32; MAX_ENDPOINT_VALUES];
            for (i, slot) in ep.iter_mut().enumerate().take(bd.total_cem_values) {
                *slot = dequant_endpoint(bd.endpoints[i], bd.endpoint_range);
            }
            // Gewichte dequantisieren, Ebenen separieren (bei Dual-Plane
            // liegen sie je Rasterposition verschränkt im Strom)
            let n_weights = bd.total_weights;
            let plane_len = bd.grid_w * bd.grid_h;
            let mut plane = vec![0u32; 2 * plane_len];
            for i in 0..n_weights {
                let wq = dequant_weight(bd.weights[i], bd.weight_range);
                if bd.dual_plane {
                    plane[(i & 1) * plane_len + (i >> 1)] = wq;
                } else {
                    plane[i] = wq;
                }
            }
            // Gewichtsgrid auf Blockauflösung hochrechnen
            let taps = make_taps(bd.grid_w, bd.grid_h, bw, bh);
            let mut up = vec![[0u32; 2]; bw * bh];
            for (t, tap) in taps.iter().enumerate() {
                up[t][0] = sample_weight(tap, &plane[..plane_len]);
                if bd.dual_plane {
                    up[t][1] = sample_weight(tap, &plane[plane_len..]);
                }
            }
            // Endpunkte je Partition
            let mut eps = [Endpoints::default(); 4];
            let mut off = 0usize;
            for i in 0..bd.partitions {
                eps[i] = decode_cem(bd.cems[i], &ep[off..]);
                off += cem_values(bd.cems[i]);
            }
            let small_block = bw * bh < 31;
            let ccs = if bd.dual_plane { bd.ccs } else { usize::MAX };
            let mut ok = true;
            for t in 0..bw * bh {
                let (x, y) = ((t % bw) as u32, (t / bw) as u32);
                let subset = if bd.partitions > 1 {
                    select_partition(bd.seed, x, y, bd.partitions, small_block)
                } else {
                    0
                };
                let cem = bd.cems[subset];
                let hdr_cem = is_hdr_cem(cem);
                let e = &eps[subset];
                let (w0, w1) = (up[t][0], up[t][1]);
                if hdr {
                    let dst = &mut out[t * 8..t * 8 + 8];
                    for c in 0..4 {
                        let w = if c == ccs { w1 } else { w0 };
                        let o = if !hdr_cem || (cem == 14 && c == 3) {
                            // LDR-Endpunkte im HDR-Pfad: lineare
                            // Bit-Replikation, dann RTZ → FP16
                            let (l, h) = (e.e0[c], e.e1[c]);
                            let k = weight_interpolate((l << 8) | l, (h << 8) | h, w);
                            if k == 0xFFFF {
                                0x3C00
                            } else {
                                f16_toward_zero(k as f32 * (1.0f32 / 65536.0f32))
                            }
                        } else {
                            let k = weight_interpolate(e.e0[c] << 4, e.e1[c] << 4, w);
                            let h = qlog16_to_half(k);
                            if is_f16_inf_or_nan(h) {
                                0x7BFF
                            } else {
                                h
                            }
                        };
                        dst[c * 2..c * 2 + 2].copy_from_slice(&o.to_le_bytes());
                    }
                } else {
                    if hdr_cem {
                        // HDR-Endpunkte in LDR-Dekodierung: dieses Texel
                        // Fehlerfarbe, der Rest des Blocks dekodiert normal.
                        out[t * 4..t * 4 + 4].copy_from_slice(&[255, 0, 255, 255]);
                        ok = false;
                        continue;
                    }
                    let dst = &mut out[t * 4..t * 4 + 4];
                    for c in 0..4 {
                        let w = if c == ccs { w1 } else { w0 };
                        let (l, h) = (e.e0[c], e.e1[c]);
                        let (l, h) = if fmt.srgb {
                            ((l << 8) | 0x80, (h << 8) | 0x80)
                        } else {
                            ((l << 8) | l, (h << 8) | h)
                        };
                        dst[c] = (weight_interpolate(l, h, w) >> 8) as u8;
                    }
                }
            }
            ok
        }
    }
}

/// Dekodiert alle Blöcke einer KTX2-Stufe. `fmt` kommt aus
/// [`astc_format`]; `rgba` fasst `w * h * bytes_per_texel(fmt)` Bytes —
/// RGBA8 bei UNORM/SRGB, FP16-RGBA bei HDR. Illegal kodierte Blöcke
/// erscheinen als Norm-Fehlerfarbe, Randblöcke werden abgeschnitten.
/// (Für den Validierungs-Fuzz-Helper auch `pub`; regulärer Aufrufer ist
/// `tex::ktx2`.)
pub fn decode_blocks(
    fmt: AstcFormat,
    b: &[u8],
    off: usize,
    w: u32,
    h: u32,
    rgba: &mut [u8],
) -> Result<()> {
    let (w, h) = (w as usize, h as usize);
    let bpp = bytes_per_texel(fmt);
    let (bx, by) = (w.div_ceil(fmt.block_w), h.div_ceil(fmt.block_h));
    if b.len() < off + bx * by * BLOCK_BYTES {
        bail!("astc: Stufendaten abgeschnitten");
    }
    if rgba.len() < w * h * bpp {
        bail!("astc: Zielpuffer zu klein");
    }
    let mut block_out = vec![0u8; fmt.block_w * fmt.block_h * bpp];
    for y in 0..by {
        for x in 0..bx {
            let block = Block::read(&b[off + (y * bx + x) * BLOCK_BYTES..]);
            let content = unpack_block(&block, fmt.block_w, fmt.block_h);
            decode_block(&content, fmt, &mut block_out);
            for ty in 0..fmt.block_h {
                let dy = y * fmt.block_h + ty;
                if dy >= h {
                    break;
                }
                let srow = ty * fmt.block_w * bpp;
                let drow = dy * w * bpp;
                let cols = fmt.block_w.min(w - x * fmt.block_w) * bpp;
                let dst = drow + x * fmt.block_w * bpp;
                rgba[dst..dst + cols].copy_from_slice(&block_out[srow..srow + cols]);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vk_format_abdeckung() {
        assert!(astc_format(156).is_none());
        assert!(astc_format(185).is_none());
        let f = astc_format(157).unwrap();
        assert_eq!((f.block_w, f.block_h, f.srgb, f.hdr), (4, 4, false, false));
        let f = astc_format(158).unwrap();
        assert_eq!((f.block_w, f.block_h, f.srgb), (4, 4, true));
        let f = astc_format(184).unwrap();
        assert_eq!((f.block_w, f.block_h, f.srgb), (12, 12, true));
        assert_eq!(block_dims(163), Some((6, 5)));
        assert_eq!(bytes_per_texel(astc_format(171).unwrap()), 4);
    }

    #[test]
    fn ise_stufen_planen() {
        // Stufenzahl-Formel der Spezifikation über alle 21 Bereiche.
        let expected = [
            2u32, 3, 4, 5, 6, 8, 10, 12, 16, 20, 24, 32, 40, 48, 64, 80, 96, 128, 160, 192, 256,
        ];
        for (r, e) in expected.iter().enumerate() {
            assert_eq!(ise_levels(r) as u32, *e, "Bereich {r}");
        }
    }
}
