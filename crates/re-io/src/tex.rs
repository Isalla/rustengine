//! Textur-Laden über alle unterstützten Dateitypen hinweg.
//!
//! Die Engine liest Texturen immer als RGBA8. PNG/JPEG/TGA/BMP/QOI
//! übernimmt das `image`-Crate (nur Feature-Schalter, keine neuen
//! Pakete); **DDS** decodiert `re-io` selbst, denn das `image`-Crate
//! kennt das Format nicht und DDS ist der Hauslieferant für
//! GPU-Texturpakete: unkomprimiert (32-Bit nach Bitmaske — RGBA/BGRA/
//! A2B10G10R10, 16-Bit 5-6-5 und 1-5-5-5, 8-Bit Luminanz/Alpha) sowie
//! die blockkomprimierten BC1–BC5 (DXT1/3/4/5) sowie BC7 (`bc7`-Modul,
//! das Desktop-Format für RGBA-Qualität), inklusive DX10-Header.
//! BC6H meldet einen klaren Fehler, statt stillschweigend Müll zu
//! liefern — offen, bis es Float-Texturen gibt.
//!
//! **KTX2** liest `re-io` ebenfalls selbst: unkomprimierte Payloads
//! (R8/RGBA/BGRA), die BC-Formate über denselben Block-Decoder wie DDS
//! (Payloads sind bitgleich) sowie ETC2/EAC und ASTC (alle 14
//! 2D-Blockgrößen, UNORM und SRGB; eigene Decoder-Module mit
//! gegen zwei Referenzen fuzzvalidierter Bitgenauigkeit).
//! Basis-Superkompression (ETC1S/UASTC) meldet einen klaren Fehler —
//! sie steht mit im Textur-Nachzug der Roadmap.
//!
//! **OpenEXR** dekodiert das `exr`-Crate (reines Rust). EXR ist
//! linearer Fließkomma; für den RGBA8-Pfad wird klemmt und
//! sRGB-kodiert, damit Albedo aussieht wie eine PNG-Textur. Echte
//! HDR-Pipeline (Float-Texturen, Tonemapping im Shader) folgt später;
//! eigene Mip-Stufen aus der Datei ebenfalls.
//!
//! Mip-Ketten: DDS und KTX2 liefern eigene Stufen — alle
//! deklarierten und vollständigen wandern in [`Decoded::mips`];
//! abgeschnittene Ketten reduzieren sich auf die brauchbaren Stufen,
//! statt zu scheitern. Für Formate ohne Mips rechnet
//! [`build_mip_chain`] die Kette.

use anyhow::Result;
use std::fs;
use std::path::Path;

/// Dekodierte Textur: Breite, Höhe, RGBA-Bytes (Zeile für Zeile von
/// oben links).
#[derive(Debug)]
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Weitere Stufen, wie die Datei sie liefert (DDS, KTX2); Formate
    /// ohne Mips lassen das Feld leer — aufrufende Seiten können die
    /// Kette mit [`build_mip_chain`] ergänzen.
    pub mips: Vec<Mip>,
}

/// Eine Mip-Stufe (Maße in Texel, RGBA wie bei [`Decoded`]).
#[derive(Debug)]
pub struct Mip {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Box-Filter-Kette für Formate ohne Mips im Kopf: jede Stufe
/// halbiert (abgerundet, mindestens 1) und mittelt 2×2 mit
/// Klemmen an ungeraden Kanten — billig, ohne Abhängigkeit, und
/// für Streaming näherungsweise GPU-trilinear.
pub fn build_mip_chain(rgba: &[u8], width: u32, height: u32) -> Vec<Mip> {
    let mut chain = Vec::new();
    let (mut w, mut h) = (width.max(1), height.max(1));
    let mut cur: Vec<u8> = rgba.to_vec();
    while w > 1 || h > 1 {
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let mut next = vec![0u8; (nw * nh * 4) as usize];
        for y in 0..nh {
            let sy = (2 * y).min(h - 1);
            let sy1 = (2 * y + 1).min(h - 1);
            for x in 0..nw {
                let sx = (2 * x).min(w - 1);
                let sx1 = (2 * x + 1).min(w - 1);
                for c in 0..4 {
                    let a = cur[(sy * w + sx) as usize * 4 + c] as u32;
                    let b = cur[(sy * w + sx1) as usize * 4 + c] as u32;
                    let d = cur[(sy1 * w + sx) as usize * 4 + c] as u32;
                    let e = cur[(sy1 * w + sx1) as usize * 4 + c] as u32;
                    next[(y * nw + x) as usize * 4 + c] = ((a + b + d + e + 2) / 4) as u8;
                }
            }
        }
        chain.push(Mip { width: nw, height: nh, rgba: next });
        cur = chain.last().unwrap().rgba.clone();
        w = nw;
        h = nh;
    }
    chain
}

/// Textur anhand des Dateiinhalts decodieren (Magie schlägt Endung).
pub fn decode(path: &Path) -> Result<Decoded> {
    let bytes = fs::read(path)?;
    let d = decode_bytes(&bytes)
        .map_err(|e| e.context(format!("textur {}", path.display())))?;
    Ok(d)
}

/// wie [`decode`], aber mit fertigen Bytes (z. B. aus dem Paket-Puffer).
pub fn decode_bytes(bytes: &[u8]) -> anyhow::Result<Decoded> {
    if bytes.starts_with(b"DDS ") {
        return dds::decode(bytes);
    }
    if bytes.starts_with(&ktx2::MAGIC) {
        return ktx2::decode(bytes);
    }
    if bytes.starts_with(&EXR_MAGIC) {
        return decode_exr(bytes);
    }
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()?;
    if reader.format().is_none() {
        anyhow::bail!("unbekanntes Texturformat");
    }
    let img = reader.decode()?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    Ok(Decoded { width: w, height: h, rgba: rgba.into_raw(), mips: Vec::new() })
}

/// Endungen, die [`decode`] versteht (für Auswahl-Dialoge/Doku).
pub const TEXTURE_EXTENSIONS: &[&str] =
    &["png", "jpg", "jpeg", "tga", "bmp", "qoi", "dds", "ktx2", "exr"];

// ── DDS ──────────────────────────────────────────────────────────────

mod dds {
    use super::{Decoded, Mip};
    use anyhow::{Result, bail};

    const DDPF_ALPHAPIXELS: u32 = 0x1;
    const DDPF_FOURCC: u32 = 0x4;
    const DDPF_RGB: u32 = 0x40;
    const DDPF_LUMINANCE: u32 = 0x20_000;

    fn u32le(b: &[u8], off: usize) -> Result<u32> {
        let s = b.get(off..off + 4).ok_or_else(|| anyhow::anyhow!("dds: Datei zu kurz"))?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    fn fourcc(b: &[u8], off: usize) -> [u8; 4] {
        [b[off], b[off + 1], b[off + 2], b[off + 3]]
    }

    /// 5-6-5 → RGB8 (Alpha 255).
    fn unpack565(c: u16) -> [u8; 3] {
        let r = ((c >> 11) & 0x1F) as u32;
        let g = ((c >> 5) & 0x3F) as u32;
        let b = (c & 0x1F) as u32;
        [
            ((r << 3) | (r >> 2)) as u8,
            ((g << 2) | (g >> 4)) as u8,
            ((b << 3) | (b >> 2)) as u8,
        ]
    }

    /// n-Bit-Feld normalisiert auf 8 Bit.
    fn widen(raw: u32, bits: u32) -> u8 {
        if bits == 0 {
            return 0;
        }
        if bits >= 8 {
            (raw >> (bits - 8)) as u8
        } else {
            ((raw * 255) / ((1u32 << bits) - 1)) as u8
        }
    }

    /// BC1-Farbpalette aus zwei 5-6-5-Endpunkten.
    fn bc1_palette(c0: u16, c1: u16) -> [[u8; 3]; 4] {
        let a = unpack565(c0);
        let b = unpack565(c1);
        let mut out = [[a[0], a[1], a[2]], [b[0], b[1], b[2]], [0u8; 3], [0u8; 3]];
        if c0 > c1 {
            out[2] = [
                ((2 * a[0] as u32 + b[0] as u32) / 3) as u8,
                ((2 * a[1] as u32 + b[1] as u32) / 3) as u8,
                ((2 * a[2] as u32 + b[2] as u32) / 3) as u8,
            ];
            out[3] = [
                ((a[0] as u32 + 2 * b[0] as u32) / 3) as u8,
                ((a[1] as u32 + 2 * b[1] as u32) / 3) as u8,
                ((a[2] as u32 + 2 * b[2] as u32) / 3) as u8,
            ];
        } else {
            out[2] = [
                ((a[0] as u32 + b[0] as u32) / 2) as u8,
                ((a[1] as u32 + b[1] as u32) / 2) as u8,
                ((a[2] as u32 + b[2] as u32) / 2) as u8,
            ];
            out[3] = [0, 0, 0]; // vierter Farbton = transparent (1-Bit-Alpha)
        }
        out
    }

    /// Ein 4×4-BC1-Farbblock expandieren (8 Byte Blockdaten).
    fn decode_bc1_block(blk: &[u8], out: &mut [[u8; 3]; 16]) {
        let c0 = u16::from_le_bytes([blk[0], blk[1]]);
        let c1 = u16::from_le_bytes([blk[2], blk[3]]);
        let pal = bc1_palette(c0, c1);
        for row in 0..4 {
            let bits = blk[4 + row];
            for col in 0..4 {
                let sel = ((bits >> (col * 2)) & 3) as usize;
                out[row * 4 + col] = pal[sel];
            }
        }
    }

    /// BC3/BC4-Alpha-Palette: zwei 8-Bit-Endpunkte, 6/7/8 Stufen.
    fn bc_alpha_table(a0: u8, a1: u8) -> [u8; 8] {
        let mut t = [0u8; 8];
        if a0 > a1 {
            for i in 0..8 {
                t[i] = (((7 - i) as u32 * a0 as u32 + i as u32 * a1 as u32) / 7) as u8;
            }
        } else {
            for i in 0..6 {
                t[i] = (((5 - i) as u32 * a0 as u32 + i as u32 * a1 as u32) / 5) as u8;
            }
            t[6] = 0;
            t[7] = 255;
        }
        t
    }

    /// 48 Bits als u64 (little-endian, 6 Byte).
    fn bits48(s: &[u8]) -> u64 {
        let mut v = 0u64;
        for (i, &byte) in s.iter().enumerate().take(6) {
            v |= (byte as u64) << (i * 8);
        }
        v
    }

    /// BC1/BC2/BC3/BC4/BC5/BC7 blockweise nach RGBA8 expandieren —
    /// shared von DDS und KTX2 (beide Payloads sind bitgleich).
    pub(super) fn decode_blocks(
        b: &[u8],
        off: usize,
        width: u32,
        height: u32,
        four: &[u8; 4],
        rgba: &mut [u8],
    ) -> Result<()> {
        let kind = match four {
            b"DXT1" | b"BC1 " => 1,
            b"DXT2" | b"DXT3" | b"BC2 " => 2,
            b"DXT4" | b"DXT5" | b"BC3 " => 3,
            b"ATI1" | b"BC4 " => 4,
            b"ATI2" | b"BC5 " => 5,
            b"BC7\0" => 7,
            other => bail!(
                "dds: VierCC '{}' wird nicht decodiert (BC6H gehört in die Offline-Pipeline)",
                String::from_utf8_lossy(other)
            ),
        };
        let block_bytes = if kind == 1 || kind == 4 { 8 } else { 16 };
        let bw = (width as usize).div_ceil(4);
        let bh = (height as usize).div_ceil(4);
        let need = off + bw * bh * block_bytes;
        if b.len() < need {
            bail!("dds: zu wenig Blockdaten ({} < {need})", b.len());
        }
        for by in 0..bh {
            for bx in 0..bw {
                let base = off + (by * bw + bx) * block_bytes;
                if kind == 7 {
                    // BC7 liefert RGBA in einem Durchgang (bc7-Modul).
                    let mut px = [[0u8; 4]; 16];
                    crate::bc7::decode_block(&b[base..base + 16], &mut px);
                    for (i, p) in px.iter().enumerate() {
                        let gx = bx * 4 + i % 4;
                        let gy = by * 4 + i / 4;
                        if gx >= width as usize || gy >= height as usize {
                            continue;
                        }
                        rgba[(gy * width as usize + gx) * 4..][..4].copy_from_slice(p);
                    }
                    continue;
                }
                let mut color = [[0u8; 3]; 16];
                let mut alpha = [255u8; 16];
                match kind {
                    1 | 2 | 3 => {
                        let cblk = if kind == 1 { base } else { base + 8 };
                        decode_bc1_block(&b[cblk..cblk + 8], &mut color);
                        if kind == 2 {
                            // DXT3: 16 explizite 4-Bit-Alphas (8 Byte).
                            for i in 0..8 {
                                let byte = b[base + i];
                                alpha[i * 2] = (byte & 0xF) * 17;
                                alpha[i * 2 + 1] = (byte >> 4) * 17;
                            }
                        } else if kind == 3 {
                            // DXT5: interpolierte Alpha-Tabelle (8 Byte).
                            let t = bc_alpha_table(b[base], b[base + 1]);
                            let bits = bits48(&b[base + 2..base + 8]);
                            for i in 0..16 {
                                alpha[i] = t[((bits >> (i * 3)) & 7) as usize];
                            }
                        }
                    }
                    4 | 5 => {
                        // BC4: Rot aus der Alpha-Interpolation, BC5: + Grün.
                        let tr = bc_alpha_table(b[base], b[base + 1]);
                        let rbits = bits48(&b[base + 2..base + 8]);
                        let tg = if kind == 5 {
                            bc_alpha_table(b[base + 8], b[base + 9])
                        } else {
                            [0u8; 8]
                        };
                        let gbits = if kind == 5 { bits48(&b[base + 10..base + 16]) } else { 0 };
                        for i in 0..16 {
                            color[i][0] = tr[((rbits >> (i * 3)) & 7) as usize];
                            color[i][1] = if kind == 5 { tg[((gbits >> (i * 3)) & 7) as usize] } else { 0 };
                        }
                    }
                    _ => unreachable!(),
                }
                for py in 0..4 {
                    for px in 0..4 {
                        let gx = bx * 4 + px;
                        let gy = by * 4 + py;
                        if gx >= width as usize || gy >= height as usize {
                            continue;
                        }
                        let o = (gy * width as usize + gx) * 4;
                        let i = py * 4 + px;
                        rgba[o] = color[i][0];
                        rgba[o + 1] = color[i][1];
                        rgba[o + 2] = color[i][2];
                        rgba[o + 3] = if kind == 4 { 255 } else { alpha[i] };
                    }
                }
            }
        }
        Ok(())
    }

    /// Nach Level entscheidbare Pixelformat-Auswahl.
    enum Sel {
        Blocks([u8; 4]),
        Rgba { bpp: u32, masks: [u32; 4], alpha: bool },
        Luma { bpp: u32, alpha_only: bool, la8: bool },
    }

    fn block_bytes(four: &[u8; 4]) -> usize {
        match four {
            b"DXT1" | b"BC1 " | b"ATI1" | b"BC4 " => 8,
            _ => 16,
        }
    }

    /// Bytes einer Mip-Stufe bei gegebenen Maßen.
    fn level_bytes(sel: &Sel, w: u32, h: u32) -> usize {
        match sel {
            Sel::Blocks(four) => {
                (w as usize).div_ceil(4) * (h as usize).div_ceil(4) * block_bytes(four)
            }
            Sel::Rgba { bpp, .. } | Sel::Luma { bpp, .. } => {
                (w as usize) * (h as usize) * (*bpp as usize / 8)
            }
        }
    }

    /// Eine Stufe nach RGBA decodieren; Datengröße muss die
    /// aufrufende Seite über [`level_bytes`] sichergestellt haben.
    fn decode_level(b: &[u8], off: usize, width: u32, height: u32, sel: &Sel) -> Result<Vec<u8>> {
        let pixels = (width as usize) * (height as usize);
        let mut rgba = vec![0u8; pixels * 4];
        match sel {
            Sel::Blocks(four) => decode_blocks(b, off, width, height, four, &mut rgba)?,
            Sel::Rgba { bpp, masks, alpha } => {
                // Unkomprimiert: nach Bitmasken entpacken.
                let take = |c: u64, m: u32| -> u8 {
                    if m == 0 {
                        return 0;
                    }
                    let lo = m.trailing_zeros();
                    let n = m.count_ones();
                    let raw = ((c >> lo) & (m >> lo) as u64) as u32;
                    widen(raw, n)
                };
                let [mr, mg, mb, ma] = *masks;
                for i in 0..pixels {
                    let step = (*bpp / 8) as usize;
                    let o = off + i * step;
                    if o + step > b.len() {
                        bail!("dds: Datendatei zu kurz");
                    }
                    let c = if *bpp == 32 {
                        u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as u64
                    } else {
                        u16::from_le_bytes([b[o], b[o + 1]]) as u64
                    };
                    rgba[i * 4] = take(c, mr);
                    rgba[i * 4 + 1] = take(c, mg);
                    rgba[i * 4 + 2] = take(c, mb);
                    rgba[i * 4 + 3] = if *alpha { take(c, ma) } else { 255 };
                }
            }
            Sel::Luma { bpp, alpha_only, la8 } => {
                // L8 / A8 / LA8 / L16 — Graustufen und reine Alpha-Kanäle.
                for i in 0..pixels {
                    if *bpp == 8 {
                        let o = off + i;
                        if o >= b.len() {
                            bail!("dds: Datendatei zu kurz");
                        }
                        let v = b[o];
                        let px = if *alpha_only { [0, 0, 0, v] } else { [v, v, v, 255] };
                        rgba[i * 4..i * 4 + 4].copy_from_slice(&px);
                    } else {
                        let o = off + i * 2;
                        if o + 2 > b.len() {
                            bail!("dds: Datendatei zu kurz");
                        }
                        let px = if *la8 {
                            [b[o], b[o], b[o], b[o + 1]]
                        } else {
                            let c = u16::from_le_bytes([b[o], b[o + 1]]);
                            let l = ((c as u32 * 255) / 65535) as u8;
                            [l, l, l, 255]
                        };
                        rgba[i * 4..i * 4 + 4].copy_from_slice(&px);
                    }
                }
            }
        }
        Ok(rgba)
    }

    pub fn decode(b: &[u8]) -> Result<Decoded> {
        if b.len() < 128 || !b.starts_with(b"DDS ") {
            bail!("dds: Kopf fehlt");
        }
        let height = u32le(b, 12)?;
        let width = u32le(b, 16)?;
        if width == 0 || height == 0 || width > 16384 || height > 16384 {
            bail!("dds: Maße {width}x{height} unplausibel");
        }
        // DDS_HEADER: pixelFormat sitzt bei Dateiversatz 76 — dwFlags
        // also bei 80, dwFourCC bei 84, BitCount bei 88, Masken ab 92.
        let pf_flags = u32le(b, 80)?;
        let four = fourcc(b, 84);
        let bpp = u32le(b, 88)?;
        let masks = [u32le(b, 92)?, u32le(b, 96)?, u32le(b, 100)?, u32le(b, 104)?];
        let mut data_off = 128usize;

        // Formatwahl; DX10-Kopf trägt das Format im DXGI-Feld statt im VierCC.
        let sel = if four == *b"DX10" {
            if b.len() < 148 {
                bail!("dds: DX10-Kopf abgeschnitten");
            }
            let dxgi = u32le(b, 128)?;
            // DXGI_FORMAT-Werte: 71–73 BC1, 74–76 BC2, 77–79 BC3,
            // 80–82 BC4, 83–85 BC5, 98–100 BC7 (jeweils
            // TYPELESS/UNORM[SRGB]-Dreiergruppen).
            let four_like = match dxgi {
                71..=73 => *b"DXT1",
                74..=76 => *b"DXT3",
                77..=79 => *b"DXT5",
                80..=82 => *b"ATI1",
                83..=85 => *b"ATI2",
                98..=100 => *b"BC7\0",
                other => bail!(
                    "dds: DX10-Format {other} wird nicht decodiert (BC6H gehört in die Offline-Pipeline)"
                ),
            };
            data_off = 148;
            Sel::Blocks(four_like)
        } else if pf_flags & DDPF_FOURCC != 0 {
            Sel::Blocks(four)
        } else if pf_flags & DDPF_RGB != 0 {
            match bpp {
                32 | 16 => {
                    // 16-Bit ohne Masken: stillschweigend 5-6-5 annehmen.
                    let mut masks = masks;
                    if bpp == 16 && masks == [0, 0, 0, 0] {
                        masks = [0x7C00, 0x03E0, 0x001F, 0];
                    }
                    Sel::Rgba { bpp, masks, alpha: pf_flags & DDPF_ALPHAPIXELS != 0 }
                }
                other => bail!("dds: RGB mit {other} bit/pixel nicht unterstützt"),
            }
        } else if pf_flags & DDPF_LUMINANCE != 0 || pf_flags & 0x2 != 0 {
            match bpp {
                8 | 16 => Sel::Luma {
                    bpp,
                    alpha_only: pf_flags & DDPF_LUMINANCE == 0,
                    la8: bpp == 16 && pf_flags & DDPF_ALPHAPIXELS != 0,
                },
                other => bail!("dds: Luminanz/Alpha mit {other} bit/pixel nicht unterstützt"),
            }
        } else {
            bail!("dds: unbekanntes pixelFormat (flags {pf_flags:#x})");
        };

        // Mip-Stufen: dwMipMapCount bei 28 (0 gezählt wie 1).
        // Würfeldateien (dwCaps2 0x200/0x400) über springen die Stufen
        // über alle faces — für sie bleibt vorerst Stufe 0 von face 0.
        let want = u32le(b, 28)?.max(1);
        let cube = u32le(b, 112)? & 0x600 != 0;
        let mip_count = if cube { 1 } else { want };

        let mut base: Option<Vec<u8>> = None;
        let mut mips = Vec::new();
        let mut off = data_off;
        let (mut lw, mut lh) = (width, height);
        for i in 0..mip_count {
            let need = level_bytes(&sel, lw, lh);
            if i > 0 && off + need > b.len() {
                // Abgeschnittene Kette: was vollständig da ist, gilt —
                // Stufe 0 bleibt trotzdem lesbar.
                break;
            }
            let px = decode_level(b, off, lw, lh, &sel)?;
            match &base {
                None => base = Some(px),
                Some(_) => mips.push(Mip { width: lw, height: lh, rgba: px }),
            }
            off += need;
            lw = (lw / 2).max(1);
            lh = (lh / 2).max(1);
        }
        let rgba = base.ok_or_else(|| anyhow::anyhow!("dds: Datendatei zu kurz"))?;
        Ok(Decoded { width, height, rgba, mips })
    }
}

// ── KTX 2.0 ──────────────────────────────────────────────────────────

mod ktx2 {
    //! KTX2-Container: deklarierte Stufen wandern nach RGBA8 (wie bei
    //! DDS); abgeschnittene Ketten reduzieren sich, statt zu scheitern.
    //!
    //! Verstanden werden unkomprimizierte Payloads (R8, RGBA, BGRA —
    //! zeilenmajor von oben, wie DDS), die blockkomprimierten
    //! BC1–BC5/BC7-Daten über denselben Block-Decoder wie bei DDS
    //! (KTX2-Payloads sind bitidentisch zu DDS) sowie ETC2/EAC und
    //! ASTC in allen 14 2D-Blockgrößen (UNORM/SRGB). Superkompression
    //! (BasisLZSS/ETC1S, Zstd/UASTC) meldet klaren Fehler, bis der
    //! Transcoder einzieht. Cubemap, 3D und Arrays folgen später.

    use super::{Decoded, Mip};
    use anyhow::{Result, bail};

    pub const MAGIC: [u8; 12] =
        [0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A];

    fn u32le(b: &[u8], off: usize) -> u32 {
        u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
    }

    fn u64le(b: &[u8], off: usize) -> u64 {
        u64::from_le_bytes(b[off..off + 8].try_into().unwrap())
    }

    /// Datenversatz der Grundstufe: aus dem Level-Index (eintrag 0
    /// sitzt direkt hinter dem 80-Byte-Kopf) oder — bei levelCount 0,
    /// wo kein Index geschrieben wird — hinter den Metadaten-Abschnitten.
    fn level0_offset(b: &[u8], levels: u32) -> Result<usize> {
        if levels >= 1 {
            if b.len() < 104 {
                bail!("ktx2: Level-Index abgeschnitten");
            }
            return Ok(u64le(b, 80) as usize);
        }
        let dfd_end = u32le(b, 48) as usize + u32le(b, 52) as usize;
        let kvd_end = u32le(b, 56) as usize + u32le(b, 60) as usize;
        let sgd_end = u64le(b, 64) as usize + u64le(b, 72) as usize;
        Ok(dfd_end.max(kvd_end).max(sgd_end).next_multiple_of(4))
    }

    /// Bytes einer Stufe bei gegebener vkFormat-Nummer.
    fn vk_bytes(vk: u32, w: u32, h: u32) -> usize {
        match vk {
            9 => (w as usize) * (h as usize),
            37 | 43 | 44 | 50 => (w as usize) * (h as usize) * 4,
            // BC4/ATI1 und ETC2 RGB8/RGB8A1/EAC R11: 8 Byte je Block.
            131..=134 | 139 | 147..=150 | 153 | 154 => {
                (w as usize).div_ceil(4) * (h as usize).div_ceil(4) * 8
            }
            // ASTC 2D (VK 157–184): 16 Byte je Block im echten Footprint.
            157..=184 => {
                let (bx, by) = crate::astc::block_dims(vk).unwrap_or((4, 4));
                (w as usize).div_ceil(bx) * (h as usize).div_ceil(by) * 16
            }
            _ => (w as usize).div_ceil(4) * (h as usize).div_ceil(4) * 16,
        }
    }

    /// Eine Stufe nach RGBA; Dekoder teilen sich den Block-Pfad mit DDS.
    fn decode_level(vk: u32, b: &[u8], off: usize, width: u32, height: u32) -> Result<Vec<u8>> {
        let pixels = (width as usize) * (height as usize);
        let mut rgba = vec![0u8; pixels * 4];
        let short = |what: &str| anyhow::anyhow!("ktx2: {what}");
        match vk {
            37 | 43 => {
                let src = b.get(off..off + pixels * 4).ok_or_else(|| short("RGBA-Daten fehlen"))?;
                rgba.copy_from_slice(src);
            }
            44 | 50 => {
                let src = b.get(off..off + pixels * 4).ok_or_else(|| short("BGRA-Daten fehlen"))?;
                for (d, s) in rgba.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                    d.copy_from_slice(&[s[2], s[1], s[0], s[3]]);
                }
            }
            9 => {
                let src = b.get(off..off + pixels).ok_or_else(|| short("R8-Daten fehlen"))?;
                for (d, s) in rgba.chunks_exact_mut(4).zip(src.iter()) {
                    d.copy_from_slice(&[*s, *s, *s, 255]);
                }
            }
            131..=139 | 141 | 145 | 146 => {
                let four = match vk {
                    131..=134 => *b"DXT1",
                    135 | 136 => *b"DXT3",
                    137 | 138 => *b"DXT5",
                    139 => *b"ATI1",
                    141 => *b"ATI2",
                    _ => *b"BC7\0",
                };
                super::dds::decode_blocks(b, off, width, height, &four, &mut rgba)
                    .map_err(|e| e.context("ktx2"))?;
            }
            140 | 142 => bail!("ktx2: signierte Blockformate (BC4/BC5 SNORM) passen nicht in RGBA8"),
            143 | 144 => bail!("ktx2: BC6H gehört in die Offline-Pipeline (HDR-Folge)"),
            147..=153 | 155 => {
                crate::etc2::decode_blocks(vk, b, off, width, height, &mut rgba)
                    .map_err(|e| e.context("ktx2"))?;
            }
            154 | 156 => bail!("ktx2: signierte EAC-Formate (SNORM) passen nicht in RGBA8"),
            157..=184 => {
                let fmt = crate::astc::astc_format(vk).ok_or_else(|| short("ASTC-vkFormat unplausibel"))?;
                crate::astc::decode_blocks(fmt, b, off, width, height, &mut rgba)
                    .map_err(|e| e.context("ktx2"))?;
            }
            0 => bail!("ktx2: vkFormat undefined ohne Basis-Superkompression"),
            other => bail!("ktx2: vkFormat {other} wird nicht decodiert"),
        }
        Ok(rgba)
    }

    pub fn decode(b: &[u8]) -> Result<Decoded> {
        if b.len() < 80 || !b.starts_with(&MAGIC) {
            bail!("ktx2: Kopf fehlt");
        }
        let vk = u32le(b, 12);
        let width = u32le(b, 20);
        let height = u32le(b, 24);
        let depth = u32le(b, 28);
        let layers = u32le(b, 32);
        let faces = u32le(b, 36);
        let levels = u32le(b, 40);
        let supercomp = u32le(b, 44);
        if width == 0 || height == 0 || width > 16384 || height > 16384 {
            bail!("ktx2: Maße {width}x{height} unplausibel");
        }
        if depth != 0 || faces != 1 || layers > 1 {
            bail!("ktx2: nur 2D-Einzeltexturen (3D/Cube/Array kommen später)");
        }
        match supercomp {
            0 => {}
            1 => bail!("ktx2: BasisLZSS (ETC1S) — Transcoder steht noch aus"),
            2 => bail!("ktx2: Zstd-Superkompression — Entpacker steht noch aus"),
            other => bail!("ktx2: Superkompression {other} unbekannt"),
        }

        // Stufen: mit levelCount >= 1 steht der Index (24 Byte je
        // Eintrag) direkt hinter dem Kopf; bei levelCount 0 fehlt er —
        // dann gibt es genau die eine berechnete Grundstufe.
        let count = if levels >= 1 { levels } else { 1 };
        let mut base: Option<Vec<u8>> = None;
        let mut mips: Vec<Mip> = Vec::new();
        let (mut lw, mut lh) = (width, height);
        for i in 0..count {
            let off = if levels >= 1 {
                if b.len() < 80 + (i as usize + 1) * 24 {
                    break; // Index abgeschnitten
                }
                u64le(b, 80 + i as usize * 24) as usize
            } else {
                if i > 0 {
                    break;
                }
                level0_offset(b, 0)?
            };
            if i > 0 && off + vk_bytes(vk, lw, lh) > b.len() {
                break; // abgeschnittene Kette: Rest weglassen, Stufe 0 gilt
            }
            let px = decode_level(vk, b, off, lw, lh)?;
            match &base {
                None => base = Some(px),
                Some(_) => mips.push(Mip { width: lw, height: lh, rgba: px }),
            }
            lw = (lw / 2).max(1);
            lh = (lh / 2).max(1);
        }
        let rgba = base.ok_or_else(|| anyhow::anyhow!("ktx2: Datendatei zu kurz"))?;
        Ok(Decoded { width, height, rgba, mips })
    }
}

// ── OpenEXR ──────────────────────────────────────────────────────────

/// EXR-Magic: 0x01312F76 little-endian.
const EXR_MAGIC: [u8; 4] = [0x76, 0x2f, 0x31, 0x01];

/// RGBA8-Speicher für den EXR-Durchstich; der Wächter sitzt im
/// Konstruktor der Deklaration (kein Panic bei Riesenköpfen).
struct ExrRgba {
    w: usize,
    h: usize,
    px: Vec<u8>,
}

/// Lineares Fließkomma nach RGBA8: klemmen, dann sRGB-kodieren —
/// damit ein EXR-Albedo dieselbe Bytesprache spricht wie PNG. Wer
/// echtes HDR will, wartet auf Float-Texturen (BC6H-Zweig).
fn exr_ldr(v: f32) -> u8 {
    (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0 + 0.5) as u8
}

fn decode_exr(b: &[u8]) -> Result<Decoded> {
    use exr::prelude::*;
    let img = read()
        .no_deep_data()
        .largest_resolution_level()
        .rgba_channels(
            |resolution: Vec2<usize>, _| {
                let (w, h) = (resolution.x(), resolution.y());
                let n = match w.checked_mul(h) {
                    Some(n) if w <= 16384 && h <= 16384 && n > 0 => n,
                    _ => 0,
                };
                ExrRgba { w, h, px: vec![0u8; n.saturating_mul(4)] }
            },
            |store: &mut ExrRgba, Vec2(x, y), (r, g, b_, a): (f32, f32, f32, f32)| {
                if x >= store.w || y >= store.h {
                    return; // Wächter hat abgelehnt — Rest ignoriert
                }
                let o = (y * store.w + x) * 4;
                store.px[o] = exr_ldr(r);
                store.px[o + 1] = exr_ldr(g);
                store.px[o + 2] = exr_ldr(b_);
                store.px[o + 3] = (a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            },
        )
        .first_valid_layer()
        .all_attributes()
        .from_buffered(std::io::Cursor::new(b))?;
    let store = img.layer_data.channel_data.pixels;
    if store.px.is_empty() {
        anyhow::bail!("exr: Maße {}x{} unplausibel", store.w, store.h);
    }
    Ok(Decoded {
        width: store.w as u32,
        height: store.h as u32,
        rgba: store.px,
        mips: Vec::new(),
    })
}
