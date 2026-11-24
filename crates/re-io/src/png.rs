//! Dependency-free PNG writer for baked sprite sheets.
//!
//! Writes 8-bit RGBA PNGs with *stored* (uncompressed) DEFLATE blocks — valid
//! PNG that every importer (game engines, image viewers, ffmpeg, PIL) reads,
//! and no `image`/`png` crate is needed for the cross-compiled binaries.

use std::path::Path;

use anyhow::{bail, Result};

const CRC_POLY: u32 = 0xEDB8_8320;

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ CRC_POLY
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    let mut crc_input = Vec::with_capacity(4 + body.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(body);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// zlib stream made of stored DEFLATE blocks (max 64 KiB each).
fn zlib_stored(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() + 64);
    out.push(0x78); // CMF: deflate, 32 KiB window
    out.push(0x01); // FLG: check bits for CMF 0x78
    if raw.is_empty() {
        out.push(0x01); // BFINAL=1, BTYPE=00 (stored)
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0xFFFFu16.to_le_bytes());
    }
    let mut pos = 0;
    while pos < raw.len() {
        let n = (raw.len() - pos).min(65535);
        let last = pos + n == raw.len();
        out.push(if last { 1 } else { 0 });
        out.extend_from_slice(&(n as u16).to_le_bytes());
        out.extend_from_slice(&(!(n as u16)).to_le_bytes());
        out.extend_from_slice(&raw[pos..pos + n]);
        pos += n;
    }
    out.extend_from_slice(&adler32(raw).to_be_bytes());
    out
}

/// Encode `rgba` (width*height*4 bytes) as an RGBA8 PNG at `path`.
pub fn write_png_rgba8(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    if width == 0 || height == 0 {
        bail!("PNG needs a positive size");
    }
    let expect = (width as usize) * (height as usize) * 4;
    if rgba.len() != expect {
        bail!("PNG pixel buffer too small: {} bytes, expected {expect}", rgba.len());
    }
    let mut raw = Vec::with_capacity(expect + height as usize);
    let w = width as usize;
    for y in 0..height as usize {
        raw.push(0u8); // filter: None
        raw.extend_from_slice(&rgba[y * w * 4..(y + 1) * w * 4]);
    }

    let mut out: Vec<u8> = Vec::with_capacity(raw.len() + 128);
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // bit depth 8, color type RGBA
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &zlib_stored(&raw));
    chunk(&mut out, b"IEND", b"");

    std::fs::write(path, out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test scratch inside the workspace (system temp dirs are off-limits).
    fn scratch_dir() -> std::path::PathBuf {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp-tests");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Re-parse a PNG written by us: chunks, CRCs and stored-block pixels.
    fn decode(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
        assert_eq!(&bytes[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        let mut pos = 8;
        let mut idat = Vec::new();
        let mut ihdr: Option<(u32, u32)> = None;
        while pos < bytes.len() {
            let len = u32::from_be_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
            let kind = &bytes[pos + 4..pos + 8];
            let body = &bytes[pos + 8..pos + 8 + len];
            let want = u32::from_be_bytes(bytes[pos + 8 + len..pos + 12 + len].try_into().unwrap());
            let mut ci = Vec::new();
            ci.extend_from_slice(kind);
            ci.extend_from_slice(body);
            assert_eq!(crc32(&ci), want, "CRC of chunk {:?}", std::str::from_utf8(kind));
            match kind {
                b"IHDR" => {
                    ihdr = Some((
                        u32::from_be_bytes(body[0..4].try_into().unwrap()),
                        u32::from_be_bytes(body[4..8].try_into().unwrap()),
                    ));
                    assert_eq!(body[8], 8, "bit depth");
                    assert_eq!(body[9], 6, "color type RGBA");
                }
                b"IDAT" => idat.extend_from_slice(body),
                b"IEND" => break,
                _ => {}
            }
            pos += 12 + len;
        }
        // zlib header
        assert_eq!(&idat[..2], &[0x78, 0x01]);
        // stored blocks -> raw scanlines
        let mut raw = Vec::new();
        let mut p = 2;
        loop {
            let head = idat[p];
            let n = u16::from_le_bytes(idat[p + 1..p + 3].try_into().unwrap()) as usize;
            let nlen = u16::from_le_bytes(idat[p + 3..p + 5].try_into().unwrap());
            assert_eq!(n as u16, !nlen, "stored block complement");
            raw.extend_from_slice(&idat[p + 5..p + 5 + n]);
            p += 5 + n;
            if head & 1 != 0 {
                break;
            }
        }
        let (w, h) = ihdr.unwrap();
        // strip filter bytes
        let stride = (w as usize) * 4;
        let mut pixels = Vec::with_capacity(raw.len() - h as usize);
        for y in 0..h as usize {
            assert_eq!(raw[y * (stride + 1)], 0, "filter byte");
            pixels.extend_from_slice(&raw[y * (stride + 1) + 1..y * (stride + 1) + 1 + stride]);
        }
        // adler footer present
        let adler = u32::from_be_bytes(idat[p..p + 4].try_into().unwrap());
        assert_eq!(adler, adler32(&raw));
        (w, h, pixels)
    }

    #[test]
    fn png_roundtrip_exact() {
        let (w, h) = (5u32, 3u32);
        let img: Vec<u8> = (0..w * h)
            .flat_map(|i| [((i * 40) % 256) as u8, ((i * 7) % 256) as u8, 128, (i % 2) as u8 * 255])
            .collect();
        let dir = scratch_dir();
        let path = dir.join(format!("t_{}.png", std::process::id()));
        write_png_rgba8(&path, w, h, &img).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let (dw, dh, decoded) = decode(&bytes);
        assert_eq!((dw, dh), (w, h));
        assert_eq!(decoded, img, "pixels must survive exactly");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn large_image_splits_into_multiple_stored_blocks() {
        let n = 300u32;
        let img = vec![7u8; (n * n * 4) as usize];
        let dir = scratch_dir();
        let path = dir.join(format!("big_{}.png", std::process::id()));
        write_png_rgba8(&path, n, n, &img).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let (w, h, decoded) = decode(&bytes);
        assert_eq!((w, h), (n, n));
        assert_eq!(decoded, img);
        std::fs::remove_file(&path).ok();
    }
}
