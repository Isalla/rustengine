//! Fuzz-Helfer (nicht Teil der stabilen API): dekodiert 16-Byte-Blöcke von
//! der Standardeingabe und schreibt je Texel eine Dezimalzeile —
//! Ausgabeformat identisch zu `harness_bin` (Bodenwahrheit-Vergleich).
//!
//! Aufruf: `cargo run -p re-io --example astc_fuzz -- W H mode`
//! mit `mode`: 0 = sRGB8, 1 = UNORM8 (linear), 2 = HDR16.

use std::io::{Read, Write};

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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let w: usize = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(4);
    let h: usize = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(4);
    let mode: u32 = args.get(3).and_then(|a| a.parse().ok()).unwrap_or(0);
    DIMS.iter()
        .position(|d| *d == (w, h))
        .expect("Blockgröße nicht unter den 14 ASTC-2D-Formaten");
    let fmt = re_io::astc::AstcFormat {
        block_w: w,
        block_h: h,
        hdr: mode == 2,
        srgb: mode == 0,
    };
    let bpp = if mode == 2 { 8 } else { 4 };

    let mut buf = [0u8; 16];
    let mut out = vec![0u8; w * h * bpp];
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    while let Ok(16) = stdin.read(&mut buf) {
        // Ein Block je Aufruf: Bildgröße == Blockgröße, Quelle == 16 Byte.
        re_io::astc::decode_blocks(fmt, &buf, 0, w as u32, h as u32, &mut out)
            .expect("Dekodierung fehlgeschlagen");
        for i in 0..w * h {
            if mode == 2 {
                let mut c = [0u16; 4];
                for (k, v) in c.iter_mut().enumerate() {
                    *v = u16::from_le_bytes([out[i * 8 + k * 2], out[i * 8 + k * 2 + 1]]);
                }
                writeln!(stdout, "{} {} {} {}", c[0], c[1], c[2], c[3]).unwrap();
            } else {
                writeln!(
                    stdout,
                    "{} {} {} {}",
                    out[i * 4],
                    out[i * 4 + 1],
                    out[i * 4 + 2],
                    out[i * 4 + 3]
                )
                .unwrap();
            }
        }
    }
}
