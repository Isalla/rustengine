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
fn bc7_meldet_fehler_statt_müll() {
    let mut b = dds_header(4, 4, PF_FOURCC, *b"BC7\0", 0, [0; 4]);
    b.extend_from_slice(&[0u8; 16]);
    let e = tex::decode_bytes(&b).unwrap_err();
    assert!(e.to_string().contains("BC7"), "klare Ansage: {e}");
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
