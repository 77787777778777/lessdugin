// SPDX-License-Identifier: AGPL-3.0-or-later
//! `uploadBlob` MIME handling: the PDS sniffs PNG, JPEG, GIF and WebP and stores the sniffed
//! type whatever was declared; anything else keeps the declared type without parameters
//! (`text/plain; charset=utf-8` → `text/plain`), or `application/octet-stream` (ADR 0005).

/// The MIME type a reference-like PDS stores for `bytes` declared as `declared`.
pub fn sniff_mime(bytes: &[u8], declared: &str) -> String {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return "image/png".to_owned();
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return "image/jpeg".to_owned();
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return "image/gif".to_owned();
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return "image/webp".to_owned();
    }
    let base = declared
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if base.is_empty() || !base.contains('/') {
        "application/octet-stream".to_owned()
    } else {
        base
    }
}

/// A tiny valid PNG (1×1, RGBA) whose last pixel byte is `seed`, for upload tests.
pub fn tiny_png(seed: u8) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    fn adler32(data: &[u8]) -> u32 {
        let (mut a, mut b) = (1u32, 0u32);
        for &d in data {
            a = (a + u32::from(d)) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut c = kind.to_vec();
        c.extend_from_slice(data);
        out.extend_from_slice(&c);
        out.extend_from_slice(&crc32(&c).to_be_bytes());
    }
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&1u32.to_be_bytes());
    ihdr.extend_from_slice(&1u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &ihdr);
    // One scanline: filter 0 + RGBA, stored in an uncompressed deflate block.
    let raw = [0u8, 0xFF, 0x00, 0x00, seed];
    let mut z = vec![0x78, 0x01, 0x01];
    z.extend_from_slice(&(raw.len() as u16).to_le_bytes());
    z.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
    z.extend_from_slice(&raw);
    z.extend_from_slice(&adler32(&raw).to_be_bytes());
    chunk(&mut png, b"IDAT", &z);
    chunk(&mut png, b"IEND", &[]);
    png
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffing() {
        assert_eq!(sniff_mime(&tiny_png(1), "image/jpeg"), "image/png");
        assert_eq!(sniff_mime(&tiny_png(1), ""), "image/png");
        assert_eq!(sniff_mime(b"\xFF\xD8\xFF\xE0xx", "x/y"), "image/jpeg");
        assert_eq!(sniff_mime(b"GIF89a....", ""), "image/gif");
        assert_eq!(sniff_mime(b"RIFF\0\0\0\0WEBPVP8 ", ""), "image/webp");
        assert_eq!(
            sniff_mime(b"hello", "Text/Plain; charset=utf-8"),
            "text/plain"
        );
        assert_eq!(sniff_mime(b"# x", "text/markdown"), "text/markdown");
        assert_eq!(sniff_mime(b"x", ""), "application/octet-stream");
        assert_ne!(tiny_png(1), tiny_png(2));
    }
}
