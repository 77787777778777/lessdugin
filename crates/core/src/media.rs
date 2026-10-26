// SPDX-License-Identifier: AGPL-3.0-or-later
//! Media pipeline: sniffing, metadata stripping, orientation and downscaling.
//!
//! `threat-model.md` PRIV-4 and §5.9.1: images and MP4 carry metadata the user did not intend to
//! publish — EXIF with GPS, IPTC captions, XMP, MPF second images, PNG text chunks, WebP EXIF,
//! GIF comments, MP4 `udta` boxes. The pipeline **rebuilds** each container by allowlist, in the
//! original order, dropping everything the table does not name (including any data after the end
//! marker), so nothing can ride along in a segment the allowlist forgot.
//!
//! **What is stripped.** Images and MP4 only (`threat-model.md` PRIV-7: other files upload as
//! they are, and the UI warns). The per-format tables are §5.9.1:
//!
//! | Format | Kept | Dropped |
//! |---|---|---|
//! | JPEG | SOI; SOF\*; DHT, DAC, DQT, DRI; SOS with its entropy-coded data and RST\* ; APP2 with the `ICC_PROFILE\0` identifier; APP14 `Adobe`; EOI | APP0, APP1 (EXIF, XMP), APP13 (IPTC), every other APP\*, COM, MPF APP2, anything after EOI |
//! | PNG | IHDR, PLTE, tRNS, cHRM, gAMA, iCCP, sBIT, sRGB, cICP, mDCV, cLLI, bKGD, pHYs, acTL, fcTL, IDAT, fdAT, IEND | tEXt, iTXt, zTXt, eXIf, tIME, every other chunk, anything after IEND |
//! | WebP | RIFF header with the size recomputed; VP8, VP8L, ALPH, ANIM, ANMF, ICCP; VP8X with its EXIF and XMP flag bits cleared | EXIF, XMP, every other chunk |
//! | GIF | the image and extension blocks, except comments | comment extensions |
//! | MP4 / MOV | **every byte** — offsets must never shift | every `udta` and `meta` box at any depth, overwritten with a `free` box whose payload is zeroed |
//!
//! **Orientation.** An EXIF orientation other than 1 is applied physically (the pixels are
//! rotated and re-encoded) before the EXIF is dropped, so a rotated photo keeps looking right
//! (`sync.md`, PRIV-4).
//!
//! **Bounded walks.** Every walk is iterative and capped (`limits.md` §6), so a truncated or
//! cyclic container cannot hang or overflow the stack: a walk that runs off the end stops and
//! treats the file as unstrippable rather than looping.

use crate::engine::EngineError;

/// The largest container the pipeline will walk (`limits.md` §6).
pub const MAX_SCAN_BYTES: usize = 300 * 1024 * 1024;

/// What a sniff found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// JPEG (JFIF/EXIF stream).
    Jpeg,
    /// PNG.
    Png,
    /// GIF87a or GIF89a.
    Gif,
    /// WebP.
    Webp,
    /// ISO-BMFF: MP4, MOV, M4A, M4B.
    Iso,
}

/// Sniffs the container format of `b` (the *format*, not the MIME type: `data-model.md` §8.8's
/// table is [`crate::outbox::blob::normalise_mime`], which also decides audio vs video for ISO).
pub fn sniff(b: &[u8]) -> Option<Format> {
    if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(Format::Jpeg);
    }
    if b.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some(Format::Png);
    }
    if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        return Some(Format::Gif);
    }
    if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        return Some(Format::Webp);
    }
    if b.len() >= 12 && &b[4..8] == b"ftyp" {
        return Some(Format::Iso);
    }
    None
}

/// The result of the metadata pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stripped {
    /// The rebuilt bytes.
    pub bytes: Vec<u8>,
    /// True when something was actually dropped.
    pub changed: bool,
    /// The EXIF orientation that was found, if any (`1`..=`8`); the caller applies it.
    pub orientation: Option<u16>,
}

/// Strips metadata from `b` by allowlist rebuild (§5.9.1). A format outside the table, or one the
/// walk cannot parse, is returned unchanged with `changed: false`: the pipeline never destroys a
/// file it does not understand, and PRIV-7 says the others upload as they are.
pub fn strip_metadata(b: &[u8]) -> Stripped {
    if b.len() > MAX_SCAN_BYTES {
        return Stripped {
            bytes: b.to_vec(),
            changed: false,
            orientation: None,
        };
    }
    match sniff(b) {
        Some(Format::Jpeg) => strip_jpeg(b),
        Some(Format::Png) => strip_png(b),
        Some(Format::Webp) => strip_webp(b),
        Some(Format::Gif) => strip_gif(b),
        Some(Format::Iso) => strip_iso(b),
        None => Stripped {
            bytes: b.to_vec(),
            changed: false,
            orientation: None,
        },
    }
}

fn unchanged(b: &[u8]) -> Stripped {
    Stripped {
        bytes: b.to_vec(),
        changed: false,
        orientation: None,
    }
}

// --- JPEG -----------------------------------------------------------------------------------

/// JPEG markers that carry image data and must be kept as they are.
fn jpeg_kept(marker: u8) -> bool {
    // SOF0..SOF15 (minus DHT/DAC/DQT/DRI, which are separate markers below), plus the
    // arithmetic/huffman tables and the restart interval.
    matches!(
        marker,
        // SOF0..SOF15, which already covers DHT (0xC4) and DAC (0xCC)
        0xC0..=0xCF
        | 0xDB        // DQT
        | 0xDD
    ) // DRI
}

/// Strips a JPEG by walking its segments (`threat-model.md` §5.9.1).
///
/// Kept: SOI, the frame/table segments, APP2 with the ICC_PROFILE identifier, APP14 Adobe, and the
/// scan (SOS) together with its entropy-coded data and RST markers, up to EOI. Dropped: APP0
/// (JFIF, which may hold a thumbnail), APP1 (EXIF/XMP), APP13 (IPTC), every other APP*, COM, the
/// MPF APP2, and anything after EOI.
fn strip_jpeg(b: &[u8]) -> Stripped {
    if b.len() < 4 {
        return unchanged(b);
    }
    let mut out = Vec::with_capacity(b.len());
    out.extend_from_slice(&b[0..2]); // SOI
    let mut i = 2;
    let mut changed = false;
    let mut orientation = None;
    // Bound the walk by the byte length; a truncated segment ends the loop and we return the
    // input unchanged rather than emitting a half-stripped file.
    while i < b.len() {
        if b[i] != 0xFF {
            return unchanged(b);
        }
        // Skip fill bytes (a run of 0xFF before a marker is legal padding). Collapsing them
        // is a change to the file, so it must be reported as one: `changed == false` promises
        // the caller byte-identical output.
        let fill_at = i;
        while i < b.len() && b[i] == 0xFF {
            i += 1;
        }
        if i > fill_at + 1 {
            changed = true;
        }
        let Some(&marker) = b.get(i) else {
            return unchanged(b);
        };
        i += 1;
        match marker {
            0xD9 => {
                // EOI: nothing after it survives
                out.extend_from_slice(&[0xFF, 0xD9]);
                return Stripped {
                    bytes: out,
                    changed: changed || i != b.len(),
                    orientation,
                };
            }
            0xD8 => {
                out.extend_from_slice(&[0xFF, 0xD8]);
                continue;
            }
            0xDA => {
                // SOS: its header segment, then the entropy-coded data up to the next marker.
                let Some(len_at) = b.get(i..i + 2) else {
                    return unchanged(b);
                };
                let len = u16::from_be_bytes([len_at[0], len_at[1]]) as usize;
                if len < 2 || i + len > b.len() {
                    return unchanged(b);
                }
                out.extend_from_slice(&[0xFF, 0xDA]);
                out.extend_from_slice(&b[i..i + len]);
                i += len;
                // entropy-coded data: copy until a marker that is not a stuffed 0x00 and not RSTn
                loop {
                    let Some(&c) = b.get(i) else {
                        return unchanged(b);
                    };
                    if c != 0xFF {
                        out.push(c);
                        i += 1;
                        continue;
                    }
                    let Some(&n) = b.get(i + 1) else {
                        return unchanged(b);
                    };
                    if n == 0x00 {
                        out.extend_from_slice(&[0xFF, 0x00]);
                        i += 2;
                        continue;
                    }
                    if (0xD0..=0xD7).contains(&n) {
                        out.extend_from_slice(&[0xFF, n]);
                        i += 2;
                        continue;
                    }
                    // a real marker: leave it for the outer loop
                    break;
                }
                continue;
            }
            _ => {}
        }
        let Some(len_at) = b.get(i..i + 2) else {
            return unchanged(b);
        };
        let len = u16::from_be_bytes([len_at[0], len_at[1]]) as usize;
        if len < 2 || i + len > b.len() {
            return unchanged(b);
        }
        let payload = &b[i + 2..i + len];
        let keep = if jpeg_kept(marker) {
            true
        } else if marker == 0xE2 {
            // APP2 keeps only the ICC profile
            payload.starts_with(b"ICC_PROFILE\0")
        } else if marker == 0xEE {
            // APP14 Adobe changes colour decoding
            payload.starts_with(b"Adobe")
        } else {
            // APP0, APP1 (EXIF/XMP), APP13 (IPTC), other APP*, COM, MPF
            false
        };
        if marker == 0xE1 && orientation.is_none() {
            orientation = exif_orientation(payload);
        }
        if keep {
            out.extend_from_slice(&[0xFF, marker]);
            out.extend_from_slice(&b[i..i + len]);
        } else {
            changed = true;
        }
        i += len;
    }
    // no EOI: a truncated stream
    unchanged(b)
}

/// The EXIF orientation tag (0x0112) from an APP1 payload, if present and sane.
fn exif_orientation(app1: &[u8]) -> Option<u16> {
    // "Exif\0\0" then a TIFF header ("II"/"MM", 42, offset to IFD0)
    let tiff = app1.strip_prefix(b"Exif\0\0")?;
    let little = match tiff.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |i: usize| -> Option<u16> {
        let a = tiff.get(i..i + 2)?;
        Some(if little {
            u16::from_le_bytes([a[0], a[1]])
        } else {
            u16::from_be_bytes([a[0], a[1]])
        })
    };
    let u32_at = |i: usize| -> Option<u32> {
        let a = tiff.get(i..i + 4)?;
        Some(if little {
            u32::from_le_bytes([a[0], a[1], a[2], a[3]])
        } else {
            u32::from_be_bytes([a[0], a[1], a[2], a[3]])
        })
    };
    if u16_at(2)? != 42 {
        return None;
    }
    let ifd = u32_at(4)? as usize;
    let count = u16_at(ifd)? as usize;
    // bound the entry walk (limits.md §6)
    for e in 0..count.min(4096) {
        let at = ifd + 2 + e * 12;
        let tag = u16_at(at)?;
        if tag == 0x0112 {
            let value = u16_at(at + 8)?;
            return (1..=8).contains(&value).then_some(value);
        }
    }
    None
}

// --- PNG ------------------------------------------------------------------------------------

/// Strips a PNG by chunk allowlist (`threat-model.md` §5.9.1).
///
/// Kept: IHDR, PLTE, tRNS, cHRM, gAMA, iCCP, sBIT, sRGB, cICP, mDCV, cLLI, bKGD, pHYs, acTL,
/// fcTL, IDAT, fdAT, IEND. Dropped: tEXt, iTXt, zTXt, eXIf, tIME, every other chunk, and anything
/// after IEND.
fn strip_png(b: &[u8]) -> Stripped {
    const KEEP: [&[u8; 4]; 17] = [
        b"IHDR", b"PLTE", b"tRNS", b"cHRM", b"gAMA", b"iCCP", b"sBIT", b"sRGB", b"cICP", b"mDCV",
        b"cLLI", b"bKGD", b"pHYs", b"acTL", b"fcTL", b"IDAT", b"fdAT",
    ];

    // signature is 8 bytes
    if b.len() < 8 {
        return unchanged(b);
    }
    let mut out = Vec::with_capacity(b.len());
    out.extend_from_slice(&b[0..8]);
    let mut i = 8;
    let mut changed = false;
    while i + 8 <= b.len() {
        let len = u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
        let kind = &b[i + 4..i + 8];
        // len + type + data + CRC, with a bound so a corrupt length cannot overflow
        let Some(end) = i.checked_add(12).and_then(|v| v.checked_add(len)) else {
            return unchanged(b);
        };
        if end > b.len() {
            return unchanged(b);
        }
        match kind {
            b"IEND" => {
                out.extend_from_slice(&b[i..end]);
                return Stripped {
                    bytes: out,
                    changed: changed || end != b.len(),
                    orientation: None,
                };
            }
            k if KEEP.iter().any(|x| **x == *k) => out.extend_from_slice(&b[i..end]),
            _ => changed = true,
        }
        i = end;
    }
    unchanged(b)
}

// --- WebP -----------------------------------------------------------------------------------

/// RIFF chunk ids WebP keeps (`threat-model.md` §5.9.1).
fn webp_kept(id: &[u8]) -> bool {
    matches!(
        id,
        b"VP8 " | b"VP8L" | b"ALPH" | b"ANIM" | b"ANMF" | b"ICCP" | b"VP8X"
    )
}

/// Strips a WebP by RIFF chunk allowlist, recomputing the RIFF size and clearing VP8X's EXIF and
/// XMP flag bits (`threat-model.md` §5.9.1).
fn strip_webp(b: &[u8]) -> Stripped {
    if b.len() < 12 {
        return unchanged(b);
    }
    let mut out = Vec::with_capacity(b.len());
    out.extend_from_slice(&b[0..12]);
    let mut i = 12;
    let mut changed = false;
    while i + 8 <= b.len() {
        let id = &b[i..i + 4];
        let len = u32::from_le_bytes([b[i + 4], b[i + 5], b[i + 6], b[i + 7]]) as usize;
        // chunks are padded to an even size
        let padded = len + (len & 1);
        let Some(end) = i.checked_add(8).and_then(|v| v.checked_add(padded)) else {
            return unchanged(b);
        };
        if end > b.len() {
            return unchanged(b);
        }
        if webp_kept(id) {
            if id == b"VP8X" && len >= 1 {
                // byte 0 holds the flag bits: bit 3 = EXIF, bit 4 = XMP (RFC 9649)
                let mut chunk = b[i..end].to_vec();
                let mut flags = chunk[8];
                let before = flags;
                flags &= !(1 << 3); // EXIF
                flags &= !(1 << 4); // XMP
                if flags != before {
                    chunk[8] = flags;
                    changed = true;
                }
                out.extend_from_slice(&chunk);
            } else {
                out.extend_from_slice(&b[i..end]);
            }
        } else {
            // EXIF, XMP and every other chunk
            changed = true;
        }
        i = end;
    }
    if i != b.len() {
        // trailing bytes after the last chunk
        changed = true;
    }
    // recompute the RIFF size: everything after the first 8 bytes
    if out.len() < 8 {
        return unchanged(b);
    }
    let size = (out.len() - 8) as u32;
    out[4..8].copy_from_slice(&size.to_le_bytes());
    Stripped {
        bytes: out,
        changed,
        orientation: None,
    }
}

// --- GIF ------------------------------------------------------------------------------------

/// Strips GIF comment extensions (`threat-model.md` §5.9.1 keeps the image and other extension
/// blocks; the synthesis table adds "except NETSCAPE loop" for application extensions).
fn strip_gif(b: &[u8]) -> Stripped {
    if b.len() < 13 {
        return unchanged(b);
    }
    let mut out = Vec::with_capacity(b.len());
    // header (6) + logical screen descriptor (7)
    out.extend_from_slice(&b[0..13]);
    let mut i: usize = 13;
    let mut changed = false;
    // global colour table, if the flag is set
    let packed = b[10];
    if packed & 0x80 != 0 {
        let entries = 1usize << ((packed & 0x07) + 1);
        let Some(end) = i.checked_add(entries * 3) else {
            return unchanged(b);
        };
        if end > b.len() {
            return unchanged(b);
        }
        out.extend_from_slice(&b[i..end]);
        i = end;
    }
    // walk blocks
    let mut guard = 0usize;
    while i < b.len() {
        guard += 1;
        if guard > 1_000_000 {
            return unchanged(b);
        }
        match b[i] {
            0x21 => {
                // extension: label, then sub-blocks
                let Some(&label) = b.get(i + 1) else {
                    return unchanged(b);
                };
                let Some(start) = i.checked_add(2) else {
                    return unchanged(b);
                };
                // walk the sub-block chain to find the end
                let mut j = start;
                let mut blocks = Vec::new();
                loop {
                    let Some(&sz) = b.get(j) else {
                        return unchanged(b);
                    };
                    let sz = sz as usize;
                    if sz == 0 {
                        blocks.push(0u8);
                        j += 1;
                        break;
                    }
                    let Some(end) = j.checked_add(1).and_then(|v| v.checked_add(sz)) else {
                        return unchanged(b);
                    };
                    if end > b.len() {
                        return unchanged(b);
                    }
                    blocks.extend_from_slice(&b[j..end]);
                    j = end;
                }
                // 0xF9 = graphic control, 0x01 = plain text, 0xFF = application,
                // 0xFE = comment (dropped)
                if label == 0xFE {
                    changed = true;
                } else {
                    out.push(0x21);
                    out.push(label);
                    out.extend_from_slice(&blocks);
                }
                i = j;
            }
            0x2C => {
                // image descriptor (9 bytes) + optional local colour table + LZW sub-blocks
                let Some(start) = i.checked_add(10) else {
                    return unchanged(b);
                };
                if start > b.len() {
                    return unchanged(b);
                }
                let mut j = start;
                let lp = b[i + 9];
                if lp & 0x80 != 0 {
                    let entries = 1usize << ((lp & 0x07) + 1);
                    let Some(e) = j.checked_add(entries * 3) else {
                        return unchanged(b);
                    };
                    if e > b.len() {
                        return unchanged(b);
                    }
                    j = e;
                }
                // LZW minimum code size, then sub-blocks
                let Some(_mcs) = b.get(j) else {
                    return unchanged(b);
                };
                j += 1;
                loop {
                    let Some(&sz) = b.get(j) else {
                        return unchanged(b);
                    };
                    let sz = sz as usize;
                    if sz == 0 {
                        j += 1;
                        break;
                    }
                    let Some(e) = j.checked_add(1).and_then(|v| v.checked_add(sz)) else {
                        return unchanged(b);
                    };
                    if e > b.len() {
                        return unchanged(b);
                    }
                    j = e;
                }
                out.extend_from_slice(&b[i..j]);
                i = j;
            }
            0x3B => {
                // trailer
                out.push(0x3B);
                return Stripped {
                    bytes: out,
                    changed: changed || i + 1 != b.len(),
                    orientation: None,
                };
            }
            _ => return unchanged(b),
        }
    }
    unchanged(b)
}

// --- MP4 / ISO-BMFF -------------------------------------------------------------------------

/// The four-byte box types whose payload is dropped and whose type becomes `free`.
fn iso_dropped(kind: &[u8]) -> bool {
    kind == b"udta" || kind == b"meta"
}

/// Strips an MP4 by overwriting every `udta` and `meta` box **in place** with a `free` box whose
/// payload is zeroed (`threat-model.md` §5.9.1). No byte offset shifts, which is why this format is
/// rewritten rather than rebuilt.
fn strip_iso(b: &[u8]) -> Stripped {
    let mut out = b.to_vec();
    let mut changed = false;
    // (start, end) of the region to walk; iterative, so nesting cannot overflow the stack
    let mut stack: Vec<(usize, usize)> = vec![(0, out.len())];
    let mut guard = 0usize;
    while let Some((start, end)) = stack.pop() {
        let mut i = start;
        while i + 8 <= end {
            guard += 1;
            if guard > 1_000_000 {
                return unchanged(b);
            }
            let size32 = u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as u64;
            let kind = &b[i + 4..i + 8];
            let (size, header) = match size32 {
                0 => ((end - i) as u64, 8usize), // to end of the enclosing box
                1 => {
                    // 64-bit size follows the type
                    let Some(l) = b.get(i + 8..i + 16) else {
                        break;
                    };
                    (u64::from_be_bytes(l.try_into().unwrap()), 16usize)
                }
                n => (n, 8usize),
            };
            if size < header as u64 {
                break;
            }
            let box_end = match (i as u64).checked_add(size) {
                Some(v) if v <= end as u64 => v as usize,
                // a size that runs past its container: stop this level
                _ => end,
            };
            if iso_dropped(kind) {
                // overwrite the type with `free` and zero the payload
                out[i + 4..i + 8].copy_from_slice(b"free");
                for byte in out[i + header..box_end].iter_mut() {
                    *byte = 0;
                }
                changed = true;
            } else if matches!(
                kind,
                b"moov"
                    | b"trak"
                    | b"mdia"
                    | b"minf"
                    | b"stbl"
                    | b"edts"
                    | b"dinf"
                    | b"mvex"
                    | b"moof"
                    | b"traf"
                    | b"mfra"
                    | b"udta"
            ) {
                // container boxes: descend
                stack.push((i + header, box_end));
            }
            i = box_end;
        }
    }
    if changed {
        Stripped {
            bytes: out,
            changed,
            orientation: None,
        }
    } else {
        unchanged(b)
    }
}

/// Applies EXIF orientation physically, by re-encoding the pixels (so no EXIF survives). Returns
/// the input unchanged when the orientation is 1/absent or the image cannot be decoded.
///
/// Only the formats the `image` crate can decode in this build are handled (PNG, JPEG, GIF, WebP);
/// anything else keeps its bytes, and the caller still strips its metadata.
#[cfg(feature = "orientation")]
pub fn apply_orientation(bytes: &[u8], orientation: u16) -> Vec<u8> {
    use image::{DynamicImage, ImageFormat};
    if !(2..=8).contains(&orientation) {
        return bytes.to_vec();
    }
    let Ok(img) = image::load_from_memory(bytes) else {
        return bytes.to_vec();
    };
    // Orientations 5-8 swap the axes, so they are 90/270 rotations plus flips.
    let rotated: DynamicImage = match orientation {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.fliph().rotate90(),
        6 => img.rotate90(),
        7 => img.fliph().rotate270(),
        8 => img.rotate270(),
        _ => return bytes.to_vec(),
    };
    let mut out = Vec::new();
    let fmt = match sniff(bytes) {
        Some(Format::Png) => ImageFormat::Png,
        Some(Format::Gif) => ImageFormat::Gif,
        Some(Format::Webp) => ImageFormat::WebP,
        _ => ImageFormat::Jpeg,
    };
    if rotated
        .write_to(&mut std::io::Cursor::new(&mut out), fmt)
        .is_err()
    {
        return bytes.to_vec();
    }
    out
}

/// The pipeline: strip metadata, then apply any EXIF orientation physically.
///
/// Returns the bytes to upload. A refused type is the caller's concern (`BlobData::media`).
pub fn prepare(bytes: Vec<u8>) -> Result<Vec<u8>, EngineError> {
    let stripped = strip_metadata(&bytes);
    #[cfg(feature = "orientation")]
    if let Some(o) = stripped.orientation.filter(|o| *o != 1) {
        return Ok(apply_orientation(&stripped.bytes, o));
    }
    Ok(stripped.bytes)
}

// --- the engine entry point -----------------------------------------------------------------

use crate::dto::{MediaInput, MediaRef};

/// `prepare_media` (`engine-api.md` §3.6, M4): sniff, strip, orient, compute the CID. The upload
/// happens at publish time (`data-model.md` §8.8), so this returns a reference, not a blob.
///
/// The declared type is checked against the sniffed one through §8.8's table, which is also what
/// decides whether the type is acceptable at all; a type outside the table is refused rather than
/// uploaded as something the reader will not accept. A file whose metadata cannot be parsed is
/// still returned (the pipeline never destroys a file it does not understand, PRIV-7), but it is
/// returned with the bytes it came in with.
impl<H: crate::host::Host> crate::engine::Engine<H> {
    /// See [`prepare_media`](Self::prepare_media).
    pub fn prepare_media(
        &self,
        bytes: Vec<u8>,
        input: &MediaInput,
    ) -> Result<MediaRef, crate::engine::EngineError> {
        let sniffed = crate::outbox::blob::normalise_mime(&bytes).ok_or_else(|| {
            crate::engine::EngineError::invalid_input("unsupported media type (data-model.md §8.8)")
                .with_detail("kind", "mediaRejected")
        })?;
        // §8.8: the declared type is normalised from the sniffed one, so a mismatch is not an
        // error in the file - it is a caller that guessed. The sniffed type wins.
        if let Some(declared) = input.declared_mime.as_deref().filter(|d| *d != sniffed) {
            tracing::debug!(
                declared = %declared,
                sniffed = %sniffed,
                "prepare_media: the sniffed type wins (data-model.md §8.8)"
            );
        }
        let prepared = prepare(bytes)?;
        let mime = crate::outbox::blob::normalise_mime(&prepared).unwrap_or(sniffed);
        let mime = if mime == sniffed {
            sniffed
        } else {
            // stripping changed the container enough to change its type: trust the result
            mime
        };
        let cid = lessdugin_atp::cid::for_raw(&prepared).to_string();
        Ok(MediaRef {
            id: cid.clone(),
            cid,
            mime: mime.to_owned(),
            size_bytes: prepared.len() as u64,
            alt: input.alt.clone(),
            aspect_ratio: input.aspect_ratio.clone(),
            duration_ms: None,
            name: input.name.clone(),
        })
    }
}

/// The upload size the pipeline aims for before trying (§8.8, "downscale (try 5 MiB first, fit on
/// 413)").
pub const UPLOAD_TARGET_BYTES: usize = 5 * 1024 * 1024;

/// Re-encodes an image smaller so it fits `max_bytes`, halving the longest side until it does.
///
/// Returns `None` when the bytes are not a decodable image or cannot be made to fit - the caller
/// then refuses the upload rather than sending something the PDS will reject again (§8.8: "On 413,
/// the client downscales (images) or refuses"). Audio, video, PDF and text never downscale.
#[cfg(feature = "orientation")]
pub fn downscale_to_fit(bytes: &[u8], max_bytes: usize) -> Option<Vec<u8>> {
    use image::ImageFormat;
    let format = sniff(bytes)?;
    let image_format = match format {
        Format::Png => ImageFormat::Png,
        Format::Gif => return None, // an animation is not resized frame by frame here
        Format::Webp => ImageFormat::WebP,
        Format::Jpeg => ImageFormat::Jpeg,
        Format::Iso => return None,
    };
    let mut img = image::load_from_memory(bytes).ok()?;
    // bounded: at most 20 halvings (a 2^20-fold reduction), so this cannot spin
    for _ in 0..20 {
        let mut out = Vec::new();
        if img
            .write_to(&mut std::io::Cursor::new(&mut out), image_format)
            .is_err()
        {
            return None;
        }
        if out.len() <= max_bytes {
            return Some(out);
        }
        let (w, h) = (img.width(), img.height());
        if w <= 1 && h <= 1 {
            return None;
        }
        img = img.resize(
            (w / 2).max(1),
            (h / 2).max(1),
            image::imageops::FilterType::Lanczos3,
        );
    }
    None
}

/// §8.8's upload policy: try the prepared bytes, and on a size refusal downscale an image or give
/// up. `max_bytes` is the PDS's limit from the 413 (or [`UPLOAD_TARGET_BYTES`] beforehand).
///
/// The returned bytes are what should be uploaded, and the caller MUST recompute the CID from
/// them - the record's `ref.$link` has to be the CID of the bytes the PDS actually stored.
pub fn fit_for_upload(prepared: Vec<u8>, max_bytes: usize) -> Result<Vec<u8>, EngineError> {
    if prepared.len() <= max_bytes {
        return Ok(prepared);
    }
    #[cfg(feature = "orientation")]
    if let Some(smaller) = downscale_to_fit(&prepared, max_bytes) {
        return Ok(smaller);
    }
    Err(EngineError::new(
        crate::engine::ErrorCode::TooLarge,
        "the file is too large to upload and cannot be downscaled (data-model.md §8.8)",
    )
    .with_detail("kind", "mediaRejected")
    .with_detail("bytes", prepared.len() as u64)
    .with_detail("limit", max_bytes as u64))
}
