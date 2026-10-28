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

/// A blob `getBlob` serves from a generator, never held whole in memory: `size` deterministic
/// bytes from `seed` (the M4 acceptance test of a blob at `BLOB_HARD_MAX_BYTES`, `docs/plan.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Generated {
    /// Its length in bytes.
    pub size: u64,
    /// What decides its bytes.
    pub seed: u64,
}

impl Generated {
    /// The bytes of one chunk.
    pub const CHUNK: usize = 64 * 1024;

    /// How many chunks the blob has.
    pub fn chunks(&self) -> u64 {
        self.size.div_ceil(Self::CHUNK as u64)
    }

    /// Chunk `i`: a block of splitmix64 output from the seed, with the chunk's index mixed into
    /// its first 8 bytes so no two chunks are alike; the last one is cut at `size`.
    pub fn chunk(&self, i: u64) -> Vec<u8> {
        let start = i.saturating_mul(Self::CHUNK as u64);
        let len = self.size.saturating_sub(start).min(Self::CHUNK as u64) as usize;
        let mut x = self.seed;
        let mut out = Vec::with_capacity(Self::CHUNK);
        while out.len() < Self::CHUNK {
            x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = x;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            out.extend_from_slice(&(z ^ (z >> 31)).to_le_bytes());
        }
        for (b, m) in out.iter_mut().zip(i.to_le_bytes()) {
            *b ^= m;
        }
        out.truncate(len);
        out
    }

    /// Its CID (raw, sha2-256), hashed a chunk at a time.
    pub fn cid(&self) -> lessdugin_atp::Cid {
        use k256::sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        for i in 0..self.chunks() {
            h.update(self.chunk(i));
        }
        lessdugin_atp::cid::from_sha256_digest(lessdugin_atp::cid::RAW, h.finalize().into())
    }

    /// Every byte at once (the in-memory transport only; HTTP streams the chunks).
    pub fn bytes(&self) -> Vec<u8> {
        (0..self.chunks()).flat_map(|i| self.chunk(i)).collect()
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
    fn generated_blobs_are_deterministic_and_hash_like_their_bytes() {
        let g = Generated {
            size: 3 * Generated::CHUNK as u64 + 5,
            seed: 7,
        };
        assert_eq!(g.chunks(), 4);
        assert_eq!(g.chunk(3).len(), 5);
        assert_ne!(g.chunk(0), g.chunk(1));
        let all = g.bytes();
        assert_eq!(all.len() as u64, g.size);
        assert_eq!(g.cid(), lessdugin_atp::cid::for_raw(&all));
        assert_ne!(g.cid(), Generated { seed: 8, ..g }.cid());
    }

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
