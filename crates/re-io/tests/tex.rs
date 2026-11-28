//! Textur-Pfad: natives DDS (unkomprimiert + BC-Blöcke) und die
//! Magie-Erkennung für `image`-Formate.

use re_io::tex;
use std::fs;
use std::path::PathBuf;

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("re_tex_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn put32_at(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

/// 128-Byte-DDS-Kopf: Magie, Maße, pixelFormat bei Versatz 76.
fn dds_header(w: u32, h: u32, pf_flags: u32, fourcc: [u8; 4], bpp: u32, masks: [u32; 4]) -> Vec<u8> {
    let mut b = vec![0u8; 128];
    b[0..4].copy_from_slice(b"DDS ");
    put32_at(&mut b, 4, 124); // Headergröße
    put32_at(&mut b, 8, 0x1 | 0x2 | 0x4 | 0x1000); // Caps|Height|Width|Pitch
    put32_at(&mut b, 12, h);
    put32_at(&mut b, 16, w);
    put32_at(&mut b, 20, w * 4);
    put32_at(&mut b, 76, 32); // pf.dwSize
    put32_at(&mut b, 80, pf_flags);
    b[84..88].copy_from_slice(&fourcc);
    put32_at(&mut b, 88, bpp);
    for (i, m) in masks.iter().enumerate() {
        put32_at(&mut b, 92 + i * 4, *m);
    }
    put32_at(&mut b, 108, 0x1000); // CapsTexture
    b
}

fn put32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_le_bytes());
}

const PF_RGB: u32 = 0x40;
const PF_ALPHA: u32 = 0x1;
const PF_FOURCC: u32 = 0x4;
const PF_LUM: u32 = 0x20_000;
const PF_A: u32 = 0x2;

#[test]
fn dds_rgba8_unkomprimiert() {
    let mut b = dds_header(2, 1, PF_RGB | PF_ALPHA, *b"\0\0\0\0", 32, [0xFF, 0xFF00, 0xFF0000, 0xFF000000]);
    put32(&mut b, 0xFF_00_00_FF); // A=255 B=0 G=0 R=255
    put32(&mut b, 0xFF_00_FF_00); // Grün
    let d = tex::decode_bytes(&b).unwrap();
    assert_eq!((d.width, d.height), (2, 1));
    assert_eq!(&d.rgba[0..4], &[255, 0, 0, 255]);
    assert_eq!(&d.rgba[4..8], &[0, 255, 0, 255]);
}

#[test]
fn dds_bgra_wird_korrekt_geradegriffen() {
    // R-Maske auf Bit 16..23 (BGRA): ein Wort mit Rot oben und Alpha ganz oben.
    let mut b = dds_header(1, 1, PF_RGB | PF_ALPHA, *b"\0\0\0\0", 32, [0xFF0000, 0xFF00, 0xFF, 0xFF000000]);
    put32(&mut b, 0xFFFF_0000); // A=255, R=255 (Bit 16), G/B=0
    let d = tex::decode_bytes(&b).unwrap();
    assert_eq!(&d.rgba[0..4], &[255, 0, 0, 255]);
}

#[test]
fn dds_a8_wird_alpha() {
    let mut b = dds_header(2, 1, PF_A, *b"\0\0\0\0", 8, [0, 0, 0, 0xFF]);
    b.push(0x40);
    b.push(0xC0);
    let d = tex::decode_bytes(&b).unwrap();
    assert_eq!(&d.rgba[0..4], &[0, 0, 0, 0x40]);
    assert_eq!(&d.rgba[4..8], &[0, 0, 0, 0xC0]);
}

#[test]
fn dds_l8_wird_grau() {
    let mut b = dds_header(1, 1, PF_LUM, *b"\0\0\0\0", 8, [0, 0, 0, 0]);
    b.push(0x7F);
    let d = tex::decode_bytes(&b).unwrap();
    assert_eq!(&d.rgba[0..4], &[0x7F, 0x7F, 0x7F, 255]);
}

#[test]
fn dxt1_block_füllt_4x4() {
    // Ein Block: c0 = c1 = Weiß (5-6-5 0xFFFF), alle Selektoren 0.
    let mut b = dds_header(4, 4, PF_FOURCC, *b"DXT1", 0, [0; 4]);
    b.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0]);
    let d = tex::decode_bytes(&b).unwrap();
    assert_eq!((d.width, d.height), (4, 4));
    for px in d.rgba.chunks(4) {
        assert_eq!(px, [255, 255, 255, 255], "ganzes Feld weiß");
    }
}

#[test]
fn dxt1_selektor_eins_ist_zweiter_farbwert() {
    // c0 = Weiß, c1 = Schwarz; alle Selektoren auf 1 → Feld wird schwarz.
    let mut b = dds_header(4, 4, PF_FOURCC, *b"DXT1", 0, [0; 4]);
    b.extend_from_slice(&[0xFF, 0xFF, 0x00, 0x00]);
    b.extend_from_slice(&[0b01_01_01_01; 4]);
    let d = tex::decode_bytes(&b).unwrap();
    assert_eq!(&d.rgba[0..4], &[0, 0, 0, 255], "Selektor 1 = c1");
}

#[test]
fn dxt5_alpha_und_farbe() {
    let mut b = dds_header(4, 4, PF_FOURCC, *b"DXT5", 0, [0; 4]);
    // Alpha: a0=255, a1=0 → Tabelle[0]=255; alle 16 Indizes 0.
    b.extend_from_slice(&[255, 0, 0, 0, 0, 0, 0, 0]);
    // Farbe: c0=c1=Rot (0xF800), Selektoren 0.
    b.extend_from_slice(&[0x00, 0xF8, 0x00, 0xF8, 0, 0, 0, 0]);
    let d = tex::decode_bytes(&b).unwrap();
    assert_eq!(&d.rgba[0..4], &[255, 0, 0, 255]);
    assert_eq!(&d.rgba[15 * 4..16 * 4], &[255, 0, 0, 255], "letztes Pixel");
}

#[test]
fn bc4_rotkanal_ohne_alpha() {
    let mut b = dds_header(4, 4, PF_FOURCC, *b"ATI1", 0, [0; 4]);
    b.extend_from_slice(&[200, 0, 0, 0, 0, 0, 0, 0]);
    let d = tex::decode_bytes(&b).unwrap();
    assert_eq!(&d.rgba[0..4], &[200, 0, 0, 255], "BC4 schreibt Rot, Alpha 255");
}

#[test]
fn dx10_kopf_mit_bc3_format() {
    // pf.FOURCC = "DX10", DXGI 78 (BC3_UNORM) → DXT5-Pfad, Daten ab 148.
    let mut b = dds_header(4, 4, PF_FOURCC, *b"DX10", 0, [0; 4]);
    let mut dx10 = vec![0u8; 20];
    dx10[0..4].copy_from_slice(&78u32.to_le_bytes());
    b.extend_from_slice(&dx10);
    b.extend_from_slice(&[255, 0, 0, 0, 0, 0, 0, 0]); // Alpha
    b.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0]); // Farbe weiß
    let d = tex::decode_bytes(&b).unwrap();
    assert_eq!(&d.rgba[0..4], &[255, 255, 255, 255]);
}

#[test]
fn bc7_legacy_viercc_wird_decodiert() {
    // Modus 6, alle Endpunkte 255 (+P-Bit) → Block gleich weiß.
    let mut b = dds_header(4, 4, PF_FOURCC, *b"BC7\0", 0, [0; 4]);
    b.extend_from_slice(&[
        0xC0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x5B, 0x55, 0x55, 0x55, 0x55, 0x55,
        0x55, 0x55,
    ]);
    let d = tex::decode_bytes(&b).unwrap();
    assert_eq!((d.width, d.height), (4, 4));
    assert!(
        d.rgba.chunks_exact(4).all(|p| p == [255, 255, 255, 255]),
        "{:?}",
        &d.rgba[..32]
    );
}

#[test]
fn bc7_ueber_dx10_kopf() {
    // DXGI 100 = BC7_UNORM; derselbe Block wie im Legacy-Test.
    let mut b = dds_header(4, 4, PF_FOURCC, *b"DX10", 0, [0; 4]);
    let mut dx10 = vec![0u8; 20];
    dx10[0..4].copy_from_slice(&100u32.to_le_bytes());
    b.extend_from_slice(&dx10);
    b.extend_from_slice(&[
        0xC0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x5B, 0x55, 0x55, 0x55, 0x55, 0x55,
        0x55, 0x55,
    ]);
    let d = tex::decode_bytes(&b).unwrap();
    assert!(d.rgba.chunks_exact(4).all(|p| p == [255, 255, 255, 255]));
}

#[test]
fn bc6h_meldet_fehler_statt_müll() {
    let mut b = dds_header(4, 4, PF_FOURCC, *b"DX10", 0, [0; 4]);
    let mut dx10 = vec![0u8; 20];
    dx10[0..4].copy_from_slice(&97u32.to_le_bytes()); // BC6H_UNORM
    b.extend_from_slice(&dx10);
    b.extend_from_slice(&[0u8; 16]);
    let e = tex::decode_bytes(&b).unwrap_err();
    assert!(e.to_string().contains("BC6"), "klare Ansage: {e}");
}

#[test]
fn png_wirdüber_magie_erkannt() {
    let d = tmpdir("png");
    let path = d.join("p.png");
    let rgba = vec![10, 20, 30, 255, 40, 50, 60, 128];
    re_io::write_png_rgba8(&path, 2, 1, &rgba).unwrap();
    let img = tex::decode(&path).unwrap();
    assert_eq!((img.width, img.height), (2, 1));
    assert_eq!(img.rgba, rgba, "RGBA unverändert");
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn fremde_bytes_sind_kein_texturformat() {
    let e = tex::decode_bytes(b"GARBAGE-GARBAGE-GARBAGE").unwrap_err();
    assert!(e.to_string().contains("unbekanntes"), "sauberer Fehler: {e}");
}

// ── KTX 2.0 ──────────────────────────────────────────────────────────

/// Minimaler KTX2-Speicher: 80-Byte-Kopf, ein Level-Index-Eintrag,
/// Daten ab Versatz 104 (dfd/kvd leer).
fn ktx2(vk: u32, w: u32, h: u32, supercomp: u32, data: &[u8]) -> Vec<u8> {
    let mut b = vec![0u8; 80];
    b[0..12].copy_from_slice(&[0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A]);
    b[12..16].copy_from_slice(&vk.to_le_bytes());
    b[16..20].copy_from_slice(&1u32.to_le_bytes());
    b[20..24].copy_from_slice(&w.to_le_bytes());
    b[24..28].copy_from_slice(&h.to_le_bytes());
    b[36..40].copy_from_slice(&1u32.to_le_bytes()); // faceCount
    b[40..44].copy_from_slice(&1u32.to_le_bytes()); // levelCount
    b[44..48].copy_from_slice(&supercomp.to_le_bytes());
    b[48..52].copy_from_slice(&104u32.to_le_bytes()); // dfdByteOffset
    b[56..60].copy_from_slice(&104u32.to_le_bytes()); // kvdByteOffset
    b.extend_from_slice(&104u64.to_le_bytes()); // level 0: offset
    b.extend_from_slice(&(data.len() as u64).to_le_bytes()); // byteLength
    b.extend_from_slice(&(data.len() as u64).to_le_bytes()); // unkomprimiert
    b.extend_from_slice(data);
    b
}

#[test]
fn ktx2_rgba8_unkomprimiert() {
    let data: Vec<u8> = (0..16u8).collect();
    let d = tex::decode_bytes(&ktx2(37, 2, 2, 0, &data)).unwrap();
    assert_eq!((d.width, d.height), (2, 2));
    assert_eq!(d.rgba, data);
}

#[test]
fn ktx2_bgra_wird_umgedreht() {
    let d = tex::decode_bytes(&ktx2(44, 1, 1, 0, &[10, 20, 30, 255])).unwrap();
    assert_eq!(&d.rgba[..], &[30, 20, 10, 255]);
}

#[test]
fn ktx2_bc1_block() {
    // Endepunkte weiß/schwarz, alle Selektoren 0 → 4×4 deckend weiß.
    let d = tex::decode_bytes(&ktx2(133, 4, 4, 0, &[0xFF, 0xFF, 0, 0, 0, 0, 0, 0])).unwrap();
    assert!(d.rgba.chunks_exact(4).all(|p| p == [255, 255, 255, 255]));
}

#[test]
fn ktx2_bc7_srgb_block() {
    // vkFormat 146 = BC7_SRGB — Bytes identisch zu UNORM, derselbe
    // Modus-6-Block wie im DDS-Test.
    let block = [
        0xC0u8, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x5B, 0x55, 0x55, 0x55, 0x55, 0x55,
        0x55, 0x55,
    ];
    let d = tex::decode_bytes(&ktx2(146, 4, 4, 0, &block)).unwrap();
    assert!(d.rgba.chunks_exact(4).all(|p| p == [255, 255, 255, 255]));
}

#[test]
fn ktx2_r8_wird_grau() {
    let d = tex::decode_bytes(&ktx2(9, 2, 1, 0, &[8, 200])).unwrap();
    assert_eq!(d.rgba, vec![8, 8, 8, 255, 200, 200, 200, 255]);
}

#[test]
fn ktx2_etc1s_kündigt_transcoder_an() {
    let e = tex::decode_bytes(&ktx2(0, 4, 4, 1, &[0u8; 8])).unwrap_err();
    let t = e.to_string();
    assert!(t.contains("BasisLZSS") && t.contains("ETC1S"), "klare Ansage: {e}");
}

/// KTX2 mit echtem DFD-Abschnitt und Stufenindex: Index hinter dem
/// Kopf, DFD danach, Bilddaten 4-ausgerichtet hinter dem DFD.
fn ktx2_dfd(vk: u32, w: u32, h: u32, supercomp: u32, dfd: &[u8], levels: &[&[u8]]) -> Vec<u8> {
    assert!(!levels.is_empty());
    let mut b = vec![0u8; 80];
    b[0..12].copy_from_slice(&[0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A]);
    b[12..16].copy_from_slice(&vk.to_le_bytes());
    b[16..20].copy_from_slice(&1u32.to_le_bytes());
    b[20..24].copy_from_slice(&w.to_le_bytes());
    b[24..28].copy_from_slice(&h.to_le_bytes());
    b[36..40].copy_from_slice(&1u32.to_le_bytes()); // faceCount
    b[40..44].copy_from_slice(&(levels.len() as u32).to_le_bytes()); // levelCount
    b[44..48].copy_from_slice(&supercomp.to_le_bytes());
    let dfd_off = 80 + levels.len() * 24;
    b[48..52].copy_from_slice(&(dfd_off as u32).to_le_bytes());
    b[52..56].copy_from_slice(&(dfd.len() as u32).to_le_bytes());
    let data_off = (dfd_off + dfd.len()).next_multiple_of(4);
    b[56..60].copy_from_slice(&(data_off as u32).to_le_bytes()); // kvdByteOffset
    let mut off = data_off;
    for lvl in levels {
        b.extend_from_slice(&(off as u64).to_le_bytes()); // LevelOffset
        b.extend_from_slice(&(lvl.len() as u64).to_le_bytes()); // ByteLength
        b.extend_from_slice(&(lvl.len() as u64).to_le_bytes()); // UncompressedByteLength
        off += lvl.len();
    }
    b.extend_from_slice(dfd);
    b.resize(data_off, 0);
    for lvl in levels {
        b.extend_from_slice(lvl);
    }
    b
}

/// Farbdeskriptor mit Modell und Transferfunktion (linear/sRGB).
fn dfd_farbe(model: u32, transfer: u32) -> Vec<u8> {
    let mut d = vec![0u8; 24];
    d[0..4].copy_from_slice(&24u32.to_le_bytes());
    d[6..8].copy_from_slice(&2u16.to_le_bytes());
    d[12..14].copy_from_slice(&4u16.to_le_bytes());
    d[16..20].copy_from_slice(&0x7000u32.to_le_bytes());
    d[20..24].copy_from_slice(&((model | (transfer << 12)) as u32).to_le_bytes());
    d
}

/// Void-Extent-ASTC-4×4-Block in einer Farbe (UNORM16 je Kanal).
fn uastc_block() -> Vec<u8> {
    let mut block = [0xFFu8; 16];
    block[0] = 0xFC;
    block[1] = 0xFD;
    block[8..].copy_from_slice(&[0x34, 0x12, 0x78, 0x56, 0xBC, 0x9A, 0xF0, 0xDE]);
    block.to_vec()
}

#[test]
fn ktx2_uastc_wird_als_astc_4x4_gelesen() {
    // vkFormat 0 + DFD-Modell 166: die Blöcke laufen durch den ASTC-Pfad.
    let d = tex::decode_bytes(&ktx2_dfd(0, 4, 4, 0, &dfd_farbe(166, 2), &[&uastc_block()]))
        .unwrap();
    assert_eq!((d.width, d.height), (4, 4));
    assert!(d.rgba.chunks_exact(4).all(|p| p == [0x12, 0x56, 0x9A, 0xDE]));
}

#[test]
fn ktx2_uastc_kette_mit_stufen() {
    // Zwei Stufen (4×4 und 2×2): je ein Block pro Stufe, Kette wird
    // wie üblich nach `mips` einsortiert.
    let block = uastc_block();
    let d = tex::decode_bytes(&ktx2_dfd(
        0,
        4,
        4,
        0,
        &dfd_farbe(166, 1),
        &[&block[..], &block[..]],
    ))
    .unwrap();
    assert_eq!(d.mips.len(), 1);
    assert_eq!((d.mips[0].width, d.mips[0].height), (2, 2));
    assert_eq!(d.mips[0].rgba.len(), 2 * 2 * 4);
}

#[test]
fn ktx2_etc1s_im_dfd_meldet_transcoder() {
    // Modell 163 ohne BasisLZ-Vorspann: trotzdem ETC1S — klare Ansage.
    let e = tex::decode_bytes(&ktx2_dfd(0, 4, 4, 0, &dfd_farbe(163, 1), &[&[0u8; 16]])).unwrap_err();
    assert!(e.to_string().contains("ETC1S"), "klare Ansage: {e}");
}

#[test]
fn ktx2_uastc_hdr_meldet_klar() {
    let e = tex::decode_bytes(&ktx2_dfd(0, 4, 4, 0, &dfd_farbe(167, 1), &[&[0u8; 16]])).unwrap_err();
    assert!(e.to_string().contains("HDR"), "klare Ansage: {e}");
}

#[test]
fn ktx2_vk0_ohne_dfd_meldet_klar() {
    let e = tex::decode_bytes(&ktx2(0, 4, 4, 0, &[0u8; 16])).unwrap_err();
    assert!(e.to_string().contains("DFD"), "klare Ansage: {e}");
}

#[test]
fn ktx2_etc2_rgb8_block() {
    // Nullblock: Einzel-Modus (Bit 33 = 0), Farbe 0, Index 00 → +2.
    let d = tex::decode_bytes(&ktx2(147, 4, 4, 0, &[0u8; 8])).unwrap();
    assert!(d.rgba.chunks_exact(4).all(|p| p == [2, 2, 2, 255]));
}

#[test]
fn ktx2_etc2_rgba8_alpha_wort_zuerst() {
    // 16-Byte-Block: Wort 0 = EAC-Alpha (Multiplikator 0 → Basis 77),
    // Wort 1 = Farbwort (Einzel, Farbe 0 → +2).
    let mut b = [0u8; 16];
    b[0] = 77;
    let d = tex::decode_bytes(&ktx2(151, 4, 4, 0, &b)).unwrap();
    assert!(d.rgba.chunks_exact(4).all(|p| p == [2, 2, 2, 77]));
}

#[test]
fn ktx2_astc_void_extent_4x4() {
    // Void-Extent 2D (LDR): bits[8:0] = 0b111111100, Bits 10/11 = 1,
    // Extents „alle Einsen“, Farbe als UNORM16 je Kanal.
    let mut block = [0xFFu8; 16];
    block[0] = 0xFC;
    block[1] = 0xFD;
    block[8..].copy_from_slice(&[0x34, 0x12, 0x78, 0x56, 0xBC, 0x9A, 0xF0, 0xDE]);
    let d = tex::decode_bytes(&ktx2(157, 4, 4, 0, &block)).unwrap(); // 4x4 UNORM
    assert!(d.rgba.chunks_exact(4).all(|p| p == [0x12, 0x56, 0x9A, 0xDE]));
}

#[test]
fn ktx2_astc_illegaler_block_wird_magenta() {
    // bits[0:4] == 0 ist reserviert: Norm-Fehlerfarbe, kein Ladeabbruch.
    let d = tex::decode_bytes(&ktx2(158, 4, 4, 0, &[0u8; 16])).unwrap();
    assert!(d.rgba.chunks_exact(4).all(|p| p == [255, 0, 255, 255]));
}

#[test]
fn ktx2_astc_12x12_footprint() {
    // vkFormat 184 = 12x12 SRGB: 5×5 Bild passt in genau einen Block.
    let mut block = [0xFFu8; 16];
    block[0] = 0xFC;
    block[1] = 0xFD;
    block[8..].copy_from_slice(&[0x00, 0x80, 0x00, 0x40, 0x00, 0x20, 0x00, 0x10]);
    let d = tex::decode_bytes(&ktx2(184, 5, 5, 0, &block)).unwrap();
    // UNORM16 0x8000/0x4000/0x2000/0x1000 → RGBA8 = k >> 8.
    assert_eq!(d.rgba.len(), 25 * 4);
    assert!(d.rgba.chunks_exact(4).all(|p| p == [0x80, 0x40, 0x20, 0x10]));
}

#[test]
fn ktx2_eac_snorm_meldet_klar() {
    let e = tex::decode_bytes(&ktx2(154, 4, 4, 0, &[0u8; 8])).unwrap_err();
    assert!(e.to_string().contains("SNORM"), "klare Ansage: {e}");
}

#[test]
fn ktx2_kopf_muss_volständig_sein() {
    let mut b = ktx2(37, 1, 1, 0, &[0u8; 4]);
    b.truncate(60);
    assert!(tex::decode_bytes(&b).is_err());
}

// ── Mip-Ketten ───────────────────────────────────────────────────────

#[test]
fn dds_dxt1_liefert_mip_kette() {
    // 4×4 plus 2×2: zwei volle DXT1-Blöcke, dwMipMapCount = 2.
    let mut b = dds_header(4, 4, PF_FOURCC, *b"DXT1", 0, [0; 4]);
    b[28..32].copy_from_slice(&2u32.to_le_bytes());
    b.extend_from_slice(&[0xFF, 0xFF, 0, 0, 0, 0, 0, 0]); // 4×4 weiß
    b.extend_from_slice(&[0xFF, 0xFF, 0, 0, 0, 0, 0, 0]); // 2×2 weiß
    let d = tex::decode_bytes(&b).unwrap();
    assert_eq!(d.mips.len(), 1);
    assert_eq!((d.mips[0].width, d.mips[0].height), (2, 2));
    assert!(d.mips[0].rgba.chunks_exact(4).all(|p| p == [255, 255, 255, 255]));
}

#[test]
fn dds_abgeschnittene_kette_behält_stufe_null() {
    let mut b = dds_header(4, 4, PF_FOURCC, *b"DXT1", 0, [0; 4]);
    b[28..32].copy_from_slice(&4u32.to_le_bytes());
    b.extend_from_slice(&[0xFF, 0xFF, 0, 0, 0, 0, 0, 0]); // nur Stufe 0 da
    let d = tex::decode_bytes(&b).unwrap();
    assert_eq!((d.width, d.height), (4, 4));
    assert!(d.mips.is_empty());
    assert_eq!(&d.rgba[0..4], &[255, 255, 255, 255]);
}

#[test]
fn dds_l8_mit_zweiter_stufe() {
    // 2×2 Luminanz, Stufe 1 = 1×1: Durchschnitt der vier Werte.
    let mut b = dds_header(2, 2, PF_LUM, *b"\0\0\0\0", 8, [0; 4]);
    b[28..32].copy_from_slice(&2u32.to_le_bytes());
    b.extend_from_slice(&[10u8, 20, 30, 40, 25]);
    let d = tex::decode_bytes(&b).unwrap();
    assert_eq!(d.mips.len(), 1);
    assert_eq!(&d.mips[0].rgba[..], &[25, 25, 25, 255]);
}

#[test]
fn ktx2_liefert_deklarierte_stufen() {
    // Zwei Rohstufen (4×2 und 2×1) mit echten Indexeinträgen.
    let data: Vec<u8> = (0..40u8).collect(); // 32 Byte Stufe 0 + 8 Byte Stufe 1
    let mut b = vec![0u8; 80];
    b[0..12].copy_from_slice(&[0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A]);
    b[12..16].copy_from_slice(&37u32.to_le_bytes());
    b[16..20].copy_from_slice(&1u32.to_le_bytes());
    b[20..24].copy_from_slice(&4u32.to_le_bytes());
    b[24..28].copy_from_slice(&2u32.to_le_bytes());
    b[36..40].copy_from_slice(&1u32.to_le_bytes());
    b[40..44].copy_from_slice(&2u32.to_le_bytes()); // levelCount
    b.extend_from_slice(&128u64.to_le_bytes()); // Stufe 0 @128
    b.extend_from_slice(&32u64.to_le_bytes());
    b.extend_from_slice(&32u64.to_le_bytes());
    b.extend_from_slice(&160u64.to_le_bytes()); // Stufe 1 @160
    b.extend_from_slice(&8u64.to_le_bytes());
    b.extend_from_slice(&8u64.to_le_bytes());
    b.extend_from_slice(&data);
    let d = tex::decode_bytes(&b).unwrap();
    assert_eq!(d.rgba, data[..32].to_vec());
    assert_eq!(d.mips.len(), 1);
    assert_eq!((d.mips[0].width, d.mips[0].height), (2, 1));
    assert_eq!(d.mips[0].rgba, data[32..40].to_vec());
}

#[test]
fn build_mip_chain_mittelt_2x2() {
    // 4×4: zwei weiße Zeilen über zwei schwarzen → Stufe 1 halbiert
    // zeilenweise, Stufe 2 (1×1) mittelt auf 128.
    let mut rgba = vec![0u8; 4 * 4 * 4];
    for px in rgba.chunks_exact_mut(4) {
        px.copy_from_slice(&[0, 0, 0, 255]);
    }
    // Erste zwei Zeilen auf Weiß (Alpha bleibt überall 255).
    for y in 0..2 {
        for x in 0..4 {
            let o = (y * 4 + x) * 4;
            rgba[o] = 255;
            rgba[o + 1] = 255;
            rgba[o + 2] = 255;
        }
    }
    let chain = tex::build_mip_chain(&rgba, 4, 4);
    assert_eq!(chain.len(), 2);
    assert_eq!((chain[0].width, chain[0].height), (2, 2));
    // Stufe 1: obere Zeile noch weiß, untere schwarz …
    assert_eq!(&chain[0].rgba[0..4], &[255, 255, 255, 255]);
    assert_eq!(&chain[0].rgba[4..8], &[255, 255, 255, 255]);
    assert_eq!(&chain[0].rgba[8..12], &[0, 0, 0, 255]);
    assert_eq!(&chain[0].rgba[12..16], &[0, 0, 0, 255]);
    // … Stufe 2 mittelt zu 128.
    assert_eq!(&chain[1].rgba[..], &[128, 128, 128, 255]);
}

#[test]
fn build_mip_chain_ungerade_kanten() {
    // 3×3 → 1×1 mit Klemmen: alle Zellen 8 → Durchschnitt 8.
    let rgba = vec![8u8; 3 * 3 * 4];
    let chain = tex::build_mip_chain(&rgba, 3, 3);
    assert_eq!(chain.len(), 1);
    assert_eq!(&chain[0].rgba[..], &[8, 8, 8, 8]);
}

// ── OpenEXR ──────────────────────────────────────────────────────────

#[test]
fn exr_klemmt_und_kodiert_srgb() {
    // Linear geschrieben: 0, 0.5 und 4.0 → 0, 186 (0.5^(1/2.2)·255)
    // und 255 (geklemmt). Alpha 1.0 → 255.
    let mut path = std::env::temp_dir();
    path.push(format!("re_io_exr_{}.exr", std::process::id()));
    exr::prelude::write_rgba_file(&path, 2, 2, |x, y| {
        let v: f32 = match (x, y) {
            (0, 0) => 0.0,
            (1, 0) => 0.5,
            _ => 4.0,
        };
        (v, v, v, 1.0)
    })
    .unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    let d = tex::decode_bytes(&bytes).unwrap();
    assert_eq!((d.width, d.height), (2, 2));
    assert_eq!(&d.rgba[0..4], &[0, 0, 0, 255]);
    assert_eq!(&d.rgba[4..8], &[186, 186, 186, 255]);
    assert_eq!(&d.rgba[12..16], &[255, 255, 255, 255]);
}

#[test]
fn exr_kopf_abgeschnitten_ist_fehler() {
    // Magic + Kachel-Flag + Längenfeld, aber kein vollständiger Kopf.
    let mut b = vec![0x76, 0x2f, 0x31, 0x01, 0x02, 0, 0, 0, 0x20, 0, 0, 0];
    b.extend_from_slice(b"restlos");
    assert!(tex::decode_bytes(&b).is_err());
}
