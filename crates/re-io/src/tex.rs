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
//! (R8/RGBA/BGRA) und die BC-Formate über denselben Block-Decoder wie
//! DDS (Payloads sind bitgleich). Basis-Superkompression (ETC1S/UASTC)
//! und ETC2 melden klare Fehler — zusammen mit EXR und Mip-Ketten
//! stehen sie im Textur-Nachzug der Roadmap.
//!
//! Mip-Ketten: gelesen wird Stufe 0 — der Streaming-Pfad baut seine
//! eigenen Stufen; DDS-Dateien mit Mips dürfen nicht scheitern.

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
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()?;
    if reader.format().is_none() {
        anyhow::bail!("unbekanntes Texturformat");
    }
    let img = reader.decode()?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    Ok(Decoded { width: w, height: h, rgba: rgba.into_raw() })
}

/// Endungen, die [`decode`] versteht (für Auswahl-Dialoge/Doku).
pub const TEXTURE_EXTENSIONS: &[&str] =
    &["png", "jpg", "jpeg", "tga", "bmp", "qoi", "dds", "ktx2"];

// ── DDS ──────────────────────────────────────────────────────────────

mod dds {
    use super::Decoded;
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

        // DX10-Erweiterungskopf: Format steht im DXGI-Feld, nicht im VierCC.
        if four == *b"DX10" {
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
            let pixels = (width as usize) * (height as usize);
            let mut rgba = vec![0u8; pixels * 4];
            decode_blocks(b, data_off, width, height, &four_like, &mut rgba)?;
            return Ok(Decoded { width, height, rgba });
        }

        let pixels = (width as usize) * (height as usize);
        let mut rgba = vec![0u8; pixels * 4];

        if pf_flags & DDPF_FOURCC != 0 {
            decode_blocks(b, data_off, width, height, &four, &mut rgba)?;
            return Ok(Decoded { width, height, rgba });
        }

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
        if pf_flags & DDPF_RGB != 0 {
            match bpp {
                32 | 16 => {
                    // 16-Bit ohne Masken: stillschweigend 5-6-5 annehmen.
                    let mut masks = masks;
                    if bpp == 16 && masks == [0, 0, 0, 0] {
                        masks = [0x7C00, 0x03E0, 0x001F, 0];
                    }
                    let [mr, mg, mb, ma] = masks;
                    for i in 0..pixels {
                        let step = (bpp / 8) as usize;
                        let o = data_off + i * step;
                        if o + step > b.len() {
                            bail!("dds: Datendatei zu kurz");
                        }
                        let c = if bpp == 32 {
                            u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as u64
                        } else {
                            u16::from_le_bytes([b[o], b[o + 1]]) as u64
                        };
                        let alpha = if pf_flags & DDPF_ALPHAPIXELS != 0 { take(c, ma) } else { 255 };
                        rgba[i * 4] = take(c, mr);
                        rgba[i * 4 + 1] = take(c, mg);
                        rgba[i * 4 + 2] = take(c, mb);
                        rgba[i * 4 + 3] = alpha;
                    }
                }
                other => bail!("dds: RGB mit {other} bit/pixel nicht unterstützt"),
            }
        } else if pf_flags & DDPF_LUMINANCE != 0 || pf_flags & 0x2 != 0 {
            // L8 / A8 / LA8 / L16 — Graustufen und reine Alpha-Kanäle.
            match bpp {
                8 => {
                    let alpha_only = pf_flags & DDPF_LUMINANCE == 0;
                    for i in 0..pixels {
                        let o = data_off + i;
                        if o >= b.len() {
                            bail!("dds: Datendatei zu kurz");
                        }
                        let v = b[o];
                        let px = if alpha_only { [0, 0, 0, v] } else { [v, v, v, 255] };
                        rgba[i * 4..i * 4 + 4].copy_from_slice(&px);
                    }
                }
                16 => {
                    // LA8 (Luminanz+Alpha je 8 Bit) von L16 unterscheiden.
                    let la8 = pf_flags & DDPF_ALPHAPIXELS != 0;
                    for i in 0..pixels {
                        let o = data_off + i * 2;
                        if o + 2 > b.len() {
                            bail!("dds: Datendatei zu kurz");
                        }
                        let px = if la8 {
                            [b[o], b[o], b[o], b[o + 1]]
                        } else {
                            let c = u16::from_le_bytes([b[o], b[o + 1]]);
                            let l = ((c as u32 * 255) / 65535) as u8;
                            [l, l, l, 255]
                        };
                        rgba[i * 4..i * 4 + 4].copy_from_slice(&px);
                    }
                }
                other => bail!("dds: Luminanz/Alpha mit {other} bit/pixel nicht unterstützt"),
            }
        } else {
            bail!("dds: unbekanntes pixelFormat (flags {pf_flags:#x})");
        }
        Ok(Decoded { width, height, rgba })
    }
}

// ── KTX 2.0 ──────────────────────────────────────────────────────────

mod ktx2 {
    //! KTX2-Container: gelesen wird Stufe 0 direkt nach RGBA8.
    //!
    //! Verstanden werden unkomprimizierte Payloads (R8, RGBA, BGRA —
    //! zeilenmajor von oben, wie DDS) und die blockkomprimierten
    //! BC1–BC5/BC7-Daten über denselben Block-Decoder wie bei DDS:
    //! KTX2-Payloads sind bitidentisch zu DDS. Superkompression
    //! (BasisLZSS/ETC1S, Zstd/UASTC) meldet klaren Fehler, bis der
    //! Transcoder einzieht; ETC2/EAC bleibt als Pi-Zielformat
    //! angekündigt. Cubemap, 3D und Arrays folgen später.

    use super::Decoded;
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
            2 => bail!("ktx2: Zstd-Superkompression (UASTC) — Entpacker steht noch aus"),
            other => bail!("ktx2: Superkompression {other} unbekannt"),
        }
        let off = level0_offset(b, levels)?;

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
            147..=158 => {
                bail!("ktx2: ETC2/EAC ist das Pi-Zielformat — CPU-Decoder folgt als Nachzug")
            }
            0 => bail!("ktx2: vkFormat undefined ohne Basis-Superkompression"),
            other => bail!("ktx2: vkFormat {other} wird nicht decodiert"),
        }
        Ok(Decoded { width, height, rgba })
    }
}
