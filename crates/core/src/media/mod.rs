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
//! | GIF | the header, logical screen descriptor, global colour table, graphic control extensions, image descriptors with local colour tables and image data, the `NETSCAPE2.0` application extension, trailer | comment and plain-text extensions, other application extensions (XMP), anything after the trailer |
//! | MP4 / MOV | **every byte** — offsets must never shift | every `udta` and `meta` box at any depth, overwritten with a `free` box whose payload is zeroed |
//!
//! **Orientation.** An EXIF orientation other than 1 (from a JPEG's APP1, a PNG's `eXIf` or a
//! WebP's `EXIF` chunk) is applied physically (the pixels are rotated and re-encoded) before the
//! EXIF is dropped, so a rotated photo keeps looking right (`sync.md`, PRIV-4).
//!
//! **Fail closed.** A container of the table that the walk cannot parse (a truncated stream, a
//! length past the end, a byte that is no marker, a missing end marker) is reported `malformed`
//! and refused for upload by [`prepare`]: nothing proves that what the table drops is gone, and
//! browsers display such files anyway, so uploading them as they are would publish their EXIF.
//!
//! **Bounded walks.** Every walk is iterative and capped (`limits.md` §6), so a truncated or
//! cyclic container cannot hang or overflow the stack: a walk that runs off the end stops and
//! treats the file as malformed rather than looping.

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
    /// The container nests deeper than [`MEDIA_CONTAINER_MAX_DEPTH`], so §6 refuses it for
    /// upload. The bytes are the input's, unmodified; this is not a "nothing to drop" answer.
    pub too_deep: bool,
    /// The container is one of the table's but the walk could not parse it, so nothing proves
    /// that its metadata is gone: [`prepare`] refuses it for upload (fail closed). The bytes are
    /// the input's, unmodified; this is not a "nothing to drop" answer either.
    pub malformed: bool,
}

/// Nesting of MP4 boxes (and RIFF chunks) walked by the sniffer and the strippers
/// (`limits.md` §6, `MEDIA_CONTAINER_MAX_DEPTH`). Deeper: the file is refused for upload.
pub const MEDIA_CONTAINER_MAX_DEPTH: u32 = 32;

/// Strips metadata from `b` by allowlist rebuild (§5.9.1). A format outside the table is returned
/// unchanged with `changed: false` (PRIV-7: the others upload as they are). A container of the
/// table that the walk cannot parse comes back unchanged too, but `malformed` (the pipeline never
/// destroys a file it does not understand, and never uploads it either); one nested past
/// [`MEDIA_CONTAINER_MAX_DEPTH`] sets `too_deep` instead (§6: refused for upload).
pub fn strip_metadata(b: &[u8]) -> Stripped {
    let format = sniff(b);
    if b.len() > MAX_SCAN_BYTES {
        // never walked, so never vouched for
        return Stripped {
            malformed: format.is_some(),
            ..unchanged(b)
        };
    }
    match format {
        Some(Format::Jpeg) => strip_jpeg(b),
        Some(Format::Png) => strip_png(b),
        Some(Format::Webp) => strip_webp(b),
        Some(Format::Gif) => strip_gif(b),
        Some(Format::Iso) => strip_iso(b),
        None => unchanged(b),
    }
}

fn unchanged(b: &[u8]) -> Stripped {
    Stripped {
        bytes: b.to_vec(),
        changed: false,
        orientation: None,
        too_deep: false,
        malformed: false,
    }
}

/// The result of a rebuild that dropped something (or not) and parsed to the end.
fn rebuilt(bytes: Vec<u8>, changed: bool, orientation: Option<u16>) -> Stripped {
    Stripped {
        bytes,
        changed,
        orientation,
        too_deep: false,
        malformed: false,
    }
}

/// §6's refusal: the container nests past [`MEDIA_CONTAINER_MAX_DEPTH`], so it is refused for
/// upload with its bytes unmodified rather than half-processed.
fn refused_too_deep(b: &[u8]) -> Stripped {
    Stripped {
        too_deep: true,
        ..unchanged(b)
    }
}

/// The walk could not parse `b`: its bytes unmodified, refused for upload by [`prepare`].
fn malformed(b: &[u8]) -> Stripped {
    Stripped {
        malformed: true,
        ..unchanged(b)
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
        return malformed(b);
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
            return malformed(b);
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
            return malformed(b);
        };
        i += 1;
        match marker {
            0xD9 => {
                // EOI: nothing after it survives
                out.extend_from_slice(&[0xFF, 0xD9]);
                return rebuilt(out, changed || i != b.len(), orientation);
            }
            0xD8 => {
                out.extend_from_slice(&[0xFF, 0xD8]);
                continue;
            }
            0xDA => {
                // SOS: its header segment, then the entropy-coded data up to the next marker.
                let Some(len_at) = b.get(i..i + 2) else {
                    return malformed(b);
                };
                let len = u16::from_be_bytes([len_at[0], len_at[1]]) as usize;
                if len < 2 || i + len > b.len() {
                    return malformed(b);
                }
                out.extend_from_slice(&[0xFF, 0xDA]);
                out.extend_from_slice(&b[i..i + len]);
                i += len;
                // entropy-coded data: copy until a marker that is not a stuffed 0x00 and not RSTn
                loop {
                    let Some(&c) = b.get(i) else {
                        return malformed(b);
                    };
                    if c != 0xFF {
                        out.push(c);
                        i += 1;
                        continue;
                    }
                    let Some(&n) = b.get(i + 1) else {
                        return malformed(b);
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
            return malformed(b);
        };
        let len = u16::from_be_bytes([len_at[0], len_at[1]]) as usize;
        if len < 2 || i + len > b.len() {
            return malformed(b);
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
    malformed(b)
}

/// The EXIF orientation tag (0x0112) from an APP1 payload, if present and sane.
fn exif_orientation(app1: &[u8]) -> Option<u16> {
    // "Exif\0\0" then a TIFF header ("II"/"MM", 42, offset to IFD0)
    tiff_orientation(app1.strip_prefix(b"Exif\0\0")?)
}

/// The orientation tag of a PNG `eXIf` or WebP `EXIF` chunk: the TIFF structure itself, which
/// some writers still prefix with JPEG's `Exif\0\0`.
fn chunk_orientation(payload: &[u8]) -> Option<u16> {
    tiff_orientation(payload.strip_prefix(b"Exif\0\0").unwrap_or(payload))
}

/// The orientation tag (0x0112) of IFD0 of a TIFF structure, if present and sane.
fn tiff_orientation(tiff: &[u8]) -> Option<u16> {
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
        return malformed(b);
    }
    let mut out = Vec::with_capacity(b.len());
    out.extend_from_slice(&b[0..8]);
    let mut i = 8;
    let mut changed = false;
    let mut orientation = None;
    while i + 8 <= b.len() {
        let len = u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
        let kind = &b[i + 4..i + 8];
        // len + type + data + CRC, with a bound so a corrupt length cannot overflow
        let Some(end) = i.checked_add(12).and_then(|v| v.checked_add(len)) else {
            return malformed(b);
        };
        if end > b.len() {
            return malformed(b);
        }
        match kind {
            b"IEND" => {
                out.extend_from_slice(&b[i..end]);
                return rebuilt(out, changed || end != b.len(), orientation);
            }
            k if KEEP.iter().any(|x| **x == *k) => out.extend_from_slice(&b[i..end]),
            k => {
                // eXIf is dropped like the rest, but its orientation is applied first
                if k == b"eXIf" && orientation.is_none() {
                    orientation = chunk_orientation(&b[i + 8..i + 8 + len]);
                }
                changed = true;
            }
        }
        i = end;
    }
    // no IEND: a truncated stream
    malformed(b)
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
        return malformed(b);
    }
    let mut out = Vec::with_capacity(b.len());
    out.extend_from_slice(&b[0..12]);
    let mut i = 12;
    let mut changed = false;
    let mut orientation = None;
    while i + 8 <= b.len() {
        let id = &b[i..i + 4];
        let len = u32::from_le_bytes([b[i + 4], b[i + 5], b[i + 6], b[i + 7]]) as usize;
        // chunks are padded to an even size
        let padded = len + (len & 1);
        let Some(end) = i.checked_add(8).and_then(|v| v.checked_add(padded)) else {
            return malformed(b);
        };
        if end > b.len() {
            return malformed(b);
        }
        if webp_kept(id) {
            if id == b"VP8X" && len >= 1 {
                // byte 0 holds the flag bits, MSB first `Rsv Rsv I L E X A R` (RFC 9649):
                // ICC 0x20, alpha 0x10, EXIF 0x08, XMP 0x04, animation 0x02
                let mut chunk = b[i..end].to_vec();
                let mut flags = chunk[8];
                let before = flags;
                flags &= !0x08; // EXIF
                flags &= !0x04; // XMP
                if flags != before {
                    chunk[8] = flags;
                    changed = true;
                }
                out.extend_from_slice(&chunk);
            } else {
                out.extend_from_slice(&b[i..end]);
            }
        } else {
            // EXIF, XMP and every other chunk; EXIF's orientation is applied first
            if id == b"EXIF" && orientation.is_none() {
                orientation = chunk_orientation(&b[i + 8..i + 8 + len]);
            }
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
        return malformed(b);
    }
    let size = ((out.len() - 8) as u32).to_le_bytes();
    // a header that misstated the size is a change too (`changed: false` promises the input)
    changed |= out[4..8] != size;
    out[4..8].copy_from_slice(&size);
    rebuilt(out, changed, orientation)
}

// --- GIF ------------------------------------------------------------------------------------

/// True when an application extension's (`0xFF`) data sub-blocks begin with the `NETSCAPE2.0`
/// identifier, which makes it the loop-count extension §5.9.1 keeps. `blocks` is the collected
/// sub-block chain: a size byte, that many bytes, …, terminated by a zero size byte.
fn is_netscape_loop(blocks: &[u8]) -> bool {
    blocks.first() == Some(&0x0B) && blocks.get(1..12) == Some(b"NETSCAPE2.0")
}

/// Strips a GIF by extension allowlist (`threat-model.md` §5.9.1): the graphic control and the
/// `NETSCAPE2.0` loop extension are kept, and comment, plain-text and every other application
/// extension (XMP) are dropped, as is anything after the trailer.
fn strip_gif(b: &[u8]) -> Stripped {
    if b.len() < 13 {
        return malformed(b);
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
            return malformed(b);
        };
        if end > b.len() {
            return malformed(b);
        }
        out.extend_from_slice(&b[i..end]);
        i = end;
    }
    // walk blocks
    let mut guard = 0usize;
    while i < b.len() {
        guard += 1;
        if guard > 1_000_000 {
            return malformed(b);
        }
        match b[i] {
            0x21 => {
                // extension: label, then sub-blocks
                let Some(&label) = b.get(i + 1) else {
                    return malformed(b);
                };
                let Some(start) = i.checked_add(2) else {
                    return malformed(b);
                };
                // walk the sub-block chain to find the end
                let mut j = start;
                let mut blocks = Vec::new();
                loop {
                    let Some(&sz) = b.get(j) else {
                        return malformed(b);
                    };
                    let sz = sz as usize;
                    if sz == 0 {
                        blocks.push(0u8);
                        j += 1;
                        break;
                    }
                    let Some(end) = j.checked_add(1).and_then(|v| v.checked_add(sz)) else {
                        return malformed(b);
                    };
                    if end > b.len() {
                        return malformed(b);
                    }
                    blocks.extend_from_slice(&b[j..end]);
                    j = end;
                }
                // Allowlist rebuild (`threat-model.md` §5.9.1's GIF row): the kept extensions are
                // the graphic control and the `NETSCAPE2.0` application extension (loop count).
                // Dropped: comment (0xFE), plain text (0x01) and every other application
                // extension - which is where XMP lives - plus any label the row does not name,
                // so nothing can ride along in one the allowlist forgot.
                let keep = match label {
                    0xF9 => true,
                    0xFF => is_netscape_loop(&blocks),
                    _ => false,
                };
                if keep {
                    out.push(0x21);
                    out.push(label);
                    out.extend_from_slice(&blocks);
                } else {
                    changed = true;
                }
                i = j;
            }
            0x2C => {
                // image descriptor (9 bytes) + optional local colour table + LZW sub-blocks
                let Some(start) = i.checked_add(10) else {
                    return malformed(b);
                };
                if start > b.len() {
                    return malformed(b);
                }
                let mut j = start;
                let lp = b[i + 9];
                if lp & 0x80 != 0 {
                    let entries = 1usize << ((lp & 0x07) + 1);
                    let Some(e) = j.checked_add(entries * 3) else {
                        return malformed(b);
                    };
                    if e > b.len() {
                        return malformed(b);
                    }
                    j = e;
                }
                // LZW minimum code size, then sub-blocks
                let Some(_mcs) = b.get(j) else {
                    return malformed(b);
                };
                j += 1;
                loop {
                    let Some(&sz) = b.get(j) else {
                        return malformed(b);
                    };
                    let sz = sz as usize;
                    if sz == 0 {
                        j += 1;
                        break;
                    }
                    let Some(e) = j.checked_add(1).and_then(|v| v.checked_add(sz)) else {
                        return malformed(b);
                    };
                    if e > b.len() {
                        return malformed(b);
                    }
                    j = e;
                }
                out.extend_from_slice(&b[i..j]);
                i = j;
            }
            0x3B => {
                // trailer
                out.push(0x3B);
                return rebuilt(out, changed || i + 1 != b.len(), None);
            }
            _ => return malformed(b),
        }
    }
    // no trailer: a truncated stream
    malformed(b)
}

// --- MP4 / ISO-BMFF -------------------------------------------------------------------------

/// The four-byte box types whose payload is dropped and whose type becomes `free`.
fn iso_dropped(kind: &[u8]) -> bool {
    kind == b"udta" || kind == b"meta"
}

/// Strips an MP4 by overwriting every `udta` and `meta` box **in place** with a `free` box whose
/// payload is zeroed (`threat-model.md` §5.9.1). No byte offset shifts, which is why this format is
/// rewritten rather than rebuilt.
///
/// Nesting is capped at [`MEDIA_CONTAINER_MAX_DEPTH`] (`limits.md` §6: "Deeper: the file is
/// refused for upload"), which is what makes the walk bounded rather than merely iterative.
fn strip_iso(b: &[u8]) -> Stripped {
    let mut out = b.to_vec();
    let mut changed = false;
    // (start, end, depth) of the region to walk; iterative, so nesting cannot overflow the stack
    let mut stack: Vec<(usize, usize, u32)> = vec![(0, out.len(), 1)];
    let mut guard = 0usize;
    while let Some((start, end, depth)) = stack.pop() {
        let mut i = start;
        while i + 8 <= end {
            guard += 1;
            if guard > 1_000_000 {
                return malformed(b);
            }
            let size32 = u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as u64;
            let kind = &b[i + 4..i + 8];
            let (size, header) = match size32 {
                0 => ((end - i) as u64, 8usize), // to end of the enclosing box
                1 => {
                    // 64-bit size follows the type
                    let Some(l) = b.get(i + 8..i + 16) else {
                        return malformed(b);
                    };
                    (u64::from_be_bytes(l.try_into().unwrap()), 16usize)
                }
                n => (n, 8usize),
            };
            // A size smaller than its own header leaves the rest of this level unframed: a
            // `udta` or `meta` in it could not be found, so the walk cannot vouch for the file.
            if size < header as u64 {
                return malformed(b);
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
                // container boxes: descend, but never past §6's depth cap. `out` may already hold
                // neutralised boxes from this walk, so a refusal returns the ORIGINAL bytes rather
                // than a half-processed file.
                if depth >= MEDIA_CONTAINER_MAX_DEPTH {
                    return refused_too_deep(b);
                }
                stack.push((i + header, box_end, depth + 1));
            }
            i = box_end;
        }
    }
    if changed {
        rebuilt(out, true, None)
    } else {
        unchanged(b)
    }
}

/// SUP-9's decode caps (`threat-model.md` SUP-9, `limits.md` §6). These belong in
/// [`crate::limits`] once it mirrors §6's media rows (limits.md §21 notes that module mirrors only
/// part of the table today); the values equal the rows exactly.
#[cfg(feature = "orientation")]
const DECODE_MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;
/// The longest side, in pixels, a decoder is given (SUP-9).
#[cfg(feature = "orientation")]
const DECODE_MAX_SIDE: usize = 8_192;
/// A decoder's allocation cap: one 8,192² RGBA frame (SUP-9).
#[cfg(feature = "orientation")]
const DECODE_MAX_ALLOC_BYTES: usize = 256 * 1024 * 1024;

/// True when SUP-9 lets us decode `bytes` at all. A hostile header can declare an enormous frame,
/// and a decoder would allocate it, so the caps are checked BEFORE any pixels are read: the frame
/// size comes from `imagesize` (the container header only), and when it cannot parse the header,
/// from the decoder's own header parse ([`image::ImageReader::into_dimensions`], no pixels).
/// Their walkers differ (a JPEG fill byte before a marker throws `imagesize` off, while the
/// decoder accepts it), so a header neither can measure is refused: fail closed (F35/F36).
///
/// The three caps are 64 MiB of input, 8192 pixels per side, and a 256 MiB allocation at 4 bytes
/// a pixel. Public so every surface that decodes media - the pipeline here, and the TUI's image
/// widget - uses ONE policy; [`decode_image`] applies the same caps inside the decoder too.
#[cfg(feature = "orientation")]
pub fn decoding_allowed(bytes: &[u8]) -> bool {
    if bytes.len() > DECODE_MAX_INPUT_BYTES {
        return false;
    }
    let (width, height) = match imagesize::blob_size(bytes) {
        Ok(size) => (size.width, size.height),
        Err(_) => match reader(bytes).and_then(|r| r.into_dimensions().ok()) {
            Some((w, h)) => (w as usize, h as usize),
            None => return false,
        },
    };
    if width.max(height) > DECODE_MAX_SIDE {
        return false;
    }
    // RGBA is the largest allocation the decoders here make per pixel.
    width.saturating_mul(height).saturating_mul(4) <= DECODE_MAX_ALLOC_BYTES
}

/// An image reader over `bytes` with SUP-9's caps as the decoder's own limits, so a frame past
/// them fails inside the decoder even if a header check was fooled (the `image` crate's default
/// is 512 MiB of allocation and no side cap).
#[cfg(feature = "orientation")]
fn reader(bytes: &[u8]) -> Option<image::ImageReader<std::io::Cursor<&[u8]>>> {
    let mut r = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(DECODE_MAX_SIDE as u32);
    limits.max_image_height = Some(DECODE_MAX_SIDE as u32);
    limits.max_alloc = Some(DECODE_MAX_ALLOC_BYTES as u64);
    r.limits(limits);
    Some(r)
}

/// Decodes `bytes` under SUP-9: `None` unless [`decoding_allowed`] holds and the decoder, run
/// with the same caps as its limits, succeeds. Every decode of the pipeline goes through here.
#[cfg(feature = "orientation")]
pub fn decode_image(bytes: &[u8]) -> Option<image::DynamicImage> {
    if !decoding_allowed(bytes) {
        return None;
    }
    reader(bytes)?.decode().ok()
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
    let Some(img) = decode_image(bytes) else {
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
    // §6: a container nested past MEDIA_CONTAINER_MAX_DEPTH is refused for upload, not stripped.
    if stripped.too_deep {
        return Err(EngineError::invalid_input(format!(
            "the container nests deeper than {MEDIA_CONTAINER_MAX_DEPTH} levels (limits.md §6)",
        ))
        .with_detail("kind", "mediaRejected"));
    }
    // Fail closed (PRIV-4, §5.9.1 "the output keeps only what the table allows"): a damaged
    // image or MP4 is not uploaded with its metadata, it is refused.
    if stripped.malformed {
        return Err(EngineError::invalid_input(
            "the file is damaged (truncated or not well formed), so its metadata cannot be \
             removed safely: re-save it with an image or video editor and attach it again",
        )
        .with_detail("kind", "mediaRejected")
        .with_detail("reason", "malformed"));
    }
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
/// uploaded as something the reader will not accept. An image or MP4 whose container cannot be
/// parsed is refused too (fail closed, [`prepare`]): its metadata could not be removed.
impl<H: crate::host::Host> crate::engine::Engine<H> {
    /// See [`prepare_media`](Self::prepare_media).
    pub async fn prepare_media(
        &self,
        bytes: Vec<u8>,
        input: &MediaInput,
    ) -> Result<MediaRef, crate::engine::EngineError> {
        // limits.md §6: the SPA's client refuses a larger file before it crosses into wasm; this
        // is the same line for a caller that sent it anyway (natively there is no window)
        if cfg!(target_arch = "wasm32")
            && bytes.len() as u64 > crate::limits::MEDIA_WASM_WINDOW_BYTES
        {
            return Err(too_large_for_the_browser(bytes.len() as u64));
        }
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
        let prepared = fit_to_target(prepare(bytes)?);
        let mime = crate::outbox::blob::normalise_mime(&prepared).unwrap_or(sniffed);
        let mime = if mime == sniffed {
            sniffed
        } else {
            // stripping changed the container enough to change its type: trust the result
            mime
        };
        let cid = lessdugin_atp::cid::for_raw(&prepared).to_string();
        // Keep the PREPARED bytes under their CID: the reference in the draft is only that,
        // a reference, and the outbox needs the bytes to upload at publish time
        // (data-model.md §8.8). They are stored, not held in memory, so a composer that is
        // closed and reopened still publishes the file it prepared.
        self.store_prepared_media(&cid, &prepared, mime).await?;
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
/// 413)"): `limits.md` §12 `IMAGE_UPLOAD_TARGET_BYTES`.
pub const UPLOAD_TARGET_BYTES: usize = crate::limits::IMAGE_UPLOAD_TARGET_BYTES as usize;

/// §8.8's first step: an image larger than [`UPLOAD_TARGET_BYTES`] is downscaled to it while it is
/// prepared, so the CID the draft's `cid:` URLs name is the CID that is uploaded. One that cannot
/// be downscaled (an animation, an image the decoders here do not take) keeps its bytes: the
/// PDS's answer then decides (a 413 is refused, [`fit_for_upload`]).
fn fit_to_target(prepared: Vec<u8>) -> Vec<u8> {
    if prepared.len() <= UPLOAD_TARGET_BYTES {
        return prepared;
    }
    #[cfg(feature = "orientation")]
    if let Some(smaller) = downscale_to_fit(&prepared, UPLOAD_TARGET_BYTES) {
        return smaller;
    }
    prepared
}

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
    let mut img = decode_image(bytes)?;
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
/// up. `max_bytes` is the target after a 413: half the size of the last encoding (`limits.md`
/// §12), so a retry never sends the same body again.
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

#[cfg(all(test, feature = "orientation"))]
mod tests {
    use super::*;

    /// A PNG whose IHDR declares `w`×`h` and nothing else: enough for `imagesize` to read the
    /// declared frame, which is all the guard consults.
    fn png_claiming(w: u32, h: u32) -> Vec<u8> {
        let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let mut ihdr = w.to_be_bytes().to_vec();
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        v.extend_from_slice(&(ihdr.len() as u32).to_be_bytes());
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&ihdr);
        v.extend_from_slice(&[0; 4]);
        v
    }

    #[test]
    fn the_sup9_caps_are_checked_before_any_decode() {
        // exactly at the caps: one 8192² RGBA frame = 256 MiB (limits.md §6)
        assert!(decoding_allowed(&png_claiming(8_192, 8_192)));
        assert!(decoding_allowed(&png_claiming(8_192, 1)));
        // one pixel past the side cap
        assert!(!decoding_allowed(&png_claiming(8_193, 1)));
        assert!(!decoding_allowed(&png_claiming(1, 8_193)));
        // within the side cap but past the allocation cap
        assert!(!decoding_allowed(&png_claiming(8_192, 8_193)));
        // a header nothing can measure is refused (fail closed, F3)
        assert!(!decoding_allowed(b"not an image at all"));
        assert!(!decoding_allowed(&[0xFF, 0xD8, 0xFF, 0xFF, 0xFF]));
    }

    /// F3: a real JPEG whose SOF0 declares 12000×12000 behind one fill byte (legal, T.81 B.1.1.2).
    /// `imagesize` loses its way at the fill byte, the decoder does not: without a fallback the
    /// guard said "allowed" and the decoder allocated the frame.
    #[test]
    fn a_fill_byte_cannot_smuggle_a_frame_past_the_side_cap() {
        let mut jpeg = Vec::new();
        image::DynamicImage::new_rgb8(8, 8)
            .write_to(
                &mut std::io::Cursor::new(&mut jpeg),
                image::ImageFormat::Jpeg,
            )
            .unwrap();
        let sof = jpeg.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        // SOF0: length (2), precision (1), height (2), width (2)
        jpeg[sof + 5..sof + 9].copy_from_slice(&[0x2E, 0xE0, 0x2E, 0xE0]);
        jpeg.insert(sof, 0xFF);
        assert!(
            imagesize::blob_size(&jpeg).is_err(),
            "the fallback is untested"
        );
        assert!(!decoding_allowed(&jpeg));
        assert!(decode_image(&jpeg).is_none());
        assert_eq!(apply_orientation(&jpeg, 6), jpeg);
        assert_eq!(downscale_to_fit(&jpeg, 1), None);
        // the same fill byte in front of a frame within the caps decodes as before
        let mut small = Vec::new();
        image::DynamicImage::new_rgb8(8, 4)
            .write_to(
                &mut std::io::Cursor::new(&mut small),
                image::ImageFormat::Jpeg,
            )
            .unwrap();
        let sof = small.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        small.insert(sof, 0xFF);
        assert!(decoding_allowed(&small));
        assert_eq!(
            decode_image(&small).map(|i| (i.width(), i.height())),
            Some((8, 4))
        );
    }
}

pub mod policy;
pub use policy::{Autoload, LoadReason, MediaCandidate, autoload, host_of, is_bluesky_host};

/// The refusal of a file larger than `MEDIA_WASM_WINDOW_BYTES` in the browser (`limits.md` §6).
pub fn too_large_for_the_browser(size: u64) -> EngineError {
    EngineError::new(
        crate::engine::ErrorCode::TooLarge,
        "the file is larger than 64 MiB, which the browser does not prepare for upload \
         (limits.md §6): attach a smaller file or use the CLI",
    )
    .with_detail("kind", "mediaRejected")
    .with_detail("bytes", size)
    .with_detail("limit", crate::limits::MEDIA_WASM_WINDOW_BYTES)
}

/// `("prep", cid)`: what was prepared under `cid`.
const PREP: &str = "prep";
/// `("prep.blob", cid, n)`: its bytes, in chunks of at most 1 MiB (as the outbox keeps them).
const PREP_BLOB: &str = "prep.blob";
/// The family the first M4 builds kept prepared bytes in, as one JSON number array per file.
const LEGACY_MEDIA: (&str, &str) = ("loc", "media");

/// The row of a prepared file (`sync.md` §10.4).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct PreparedRow {
    v: u32,
    mime: String,
    size: u64,
    chunks: u32,
    /// When it was prepared (ms), for [`crate::limits::PREPARED_MEDIA_TTL_MS`].
    at: i64,
}

/// The prepared bytes of a media reference, kept until the write that uses them is enqueued.
///
/// A `MediaRef` is only a reference: `prepare_media` computes the CID and returns it, but the
/// outbox has to upload the bytes when the record write goes out (`data-model.md` §8.8), so they
/// are stored here under their CID: raw, in chunks of at most 1 MiB, never as JSON. `publish`,
/// `comment` and `edit` read them back with [`Engine::prepared_media`] and hand them to the
/// outbox, which keeps its own copy; then [`Engine::forget_prepared_media`] removes them. Files
/// prepared for a draft that is never sent are removed after
/// [`crate::limits::PREPARED_MEDIA_TTL_MS`].
impl<H: crate::host::Host> crate::engine::Engine<H> {
    /// Stores the prepared bytes under their CID, and removes what expired.
    async fn store_prepared_media(
        &self,
        cid: &str,
        bytes: &[u8],
        mime: &str,
    ) -> Result<(), crate::engine::EngineError> {
        use crate::host::{KvOp, KvStore};
        let now = self.host().now_ms();
        let mut ops = self.expired_prepared_media(now).await?;
        let chunks: Vec<&[u8]> = bytes
            .chunks(crate::outbox::model::BLOB_CHUNK_BYTES)
            .collect();
        let row = PreparedRow {
            v: 1,
            mime: mime.to_owned(),
            size: bytes.len() as u64,
            chunks: u32::try_from(chunks.len()).unwrap_or(u32::MAX),
            at: now,
        };
        let encode = |e: crate::store::row::RowError| {
            crate::engine::EngineError::internal(format!("a prepared file: {e}"))
        };
        ops.push(KvOp::DeleteRange(crate::host::KeyRange::prefix(
            &crate::key!(PREP_BLOB, cid),
        )));
        for (n, c) in chunks.into_iter().enumerate() {
            let chunk = crate::outbox::model::ChunkRow {
                v: 1,
                b: c.to_vec(),
            };
            ops.push(KvOp::Put(
                crate::key!(PREP_BLOB, cid, n as i64),
                crate::store::row::encode(&chunk).map_err(encode)?,
            ));
        }
        ops.push(KvOp::Put(
            crate::key!(PREP, cid),
            crate::store::row::encode(&row).map_err(encode)?,
        ));
        KvStore::write(self.host().state(), ops).await?;
        Ok(())
    }

    /// The deletions of every prepared file older than [`crate::limits::PREPARED_MEDIA_TTL_MS`],
    /// and of the legacy rows.
    async fn expired_prepared_media(
        &self,
        now: i64,
    ) -> Result<Vec<crate::host::KvOp>, crate::engine::EngineError> {
        use crate::host::{Dir, KeyRange, KvOp, KvStore};
        let mut ops = vec![KvOp::DeleteRange(KeyRange::prefix(&crate::key!(
            LEGACY_MEDIA.0,
            LEGACY_MEDIA.1
        )))];
        let mut range = KeyRange::prefix(&crate::key!(PREP));
        loop {
            let page = KvStore::scan(self.host().state(), range.clone(), Dir::Forward, 256).await?;
            let full = page.len() >= 256;
            let last = page.last().map(|(k, _)| k.clone());
            for (k, v) in page {
                let stale = crate::store::row::decode::<PreparedRow>(&v).map_or(true, |r| {
                    now.saturating_sub(r.at) > crate::limits::PREPARED_MEDIA_TTL_MS
                });
                let cid = match crate::store::key::decode(&k).ok().as_deref() {
                    Some([_, crate::store::key::Part::Bytes(c)]) => {
                        String::from_utf8(c.clone()).ok()
                    }
                    _ => None,
                };
                if let (true, Some(cid)) = (stale, cid) {
                    ops.push(KvOp::Delete(k));
                    ops.push(KvOp::DeleteRange(KeyRange::prefix(&crate::key!(
                        PREP_BLOB,
                        cid.as_str()
                    ))));
                }
            }
            match last {
                Some(k) if full => range.start = std::ops::Bound::Excluded(k),
                _ => break,
            }
        }
        Ok(ops)
    }

    /// The prepared bytes of `cid`, as [`crate::outbox::BlobData`] hands them to the outbox.
    ///
    /// `None` when the bytes are gone (a different device, a cleared store, expired, already
    /// sent): the caller then refuses the publish rather than sending a reference the PDS cannot
    /// resolve, which is the failure the devnet surfaced as `BlobNotFound (HTTP 400)`.
    pub async fn prepared_media(
        &self,
        cid: &str,
    ) -> Result<Option<crate::outbox::BlobData>, crate::engine::EngineError> {
        use crate::host::KvStore;
        let rows = KvStore::get_many(self.host().state(), vec![crate::key!(PREP, cid)]).await?;
        let Some(Some(raw)) = rows.into_iter().next() else {
            return Ok(None);
        };
        let Ok(row) = crate::store::row::decode::<PreparedRow>(&raw) else {
            return Ok(None);
        };
        let keys = (0..row.chunks)
            .map(|n| crate::key!(PREP_BLOB, cid, i64::from(n)))
            .collect();
        let mut bytes = Vec::with_capacity(usize::try_from(row.size).unwrap_or(0));
        for chunk in KvStore::get_many(self.host().state(), keys).await? {
            let Some(c) = chunk
                .and_then(|c| crate::store::row::decode::<crate::outbox::model::ChunkRow>(&c).ok())
            else {
                return Ok(None);
            };
            bytes.extend_from_slice(&c.b);
        }
        // a torn or tampered row is not uploaded under a CID it does not have
        if bytes.len() as u64 != row.size || lessdugin_atp::cid::for_raw(&bytes).to_string() != cid
        {
            return Ok(None);
        }
        Ok(Some(crate::outbox::BlobData {
            cid: cid.to_owned(),
            mime: row.mime,
            bytes,
        }))
    }

    /// [`Engine::forget_prepared_media`] after a write is enqueued: the write stands either way,
    /// so a failure only leaves the rows for the expiry sweep.
    pub(crate) async fn sent_prepared_media(&self, media: &[crate::dto::MediaRef]) {
        if let Err(e) = self.forget_prepared_media(media).await {
            tracing::warn!(error = %e, "prepared media are left for the expiry sweep");
        }
    }

    /// Removes the prepared bytes of `media` once a write that uploads them is enqueued: the
    /// outbox keeps its own copy from then on (`("out.blob", …)`, `sync.md` §10.4).
    pub(crate) async fn forget_prepared_media(
        &self,
        media: &[crate::dto::MediaRef],
    ) -> Result<(), crate::engine::EngineError> {
        use crate::host::{KeyRange, KvOp, KvStore};
        if media.is_empty() {
            return Ok(());
        }
        let ops = media
            .iter()
            .flat_map(|m| {
                [
                    KvOp::Delete(crate::key!(PREP, m.cid.as_str())),
                    KvOp::DeleteRange(KeyRange::prefix(&crate::key!(PREP_BLOB, m.cid.as_str()))),
                ]
            })
            .collect();
        KvStore::write(self.host().state(), ops).await?;
        Ok(())
    }
}

impl<H: crate::host::Host> crate::engine::Engine<H> {
    /// The blobs an edit uploads (`data-model.md` §8.8): the media references of `media` that the
    /// cached version `cached` does not already hold. Those it holds were uploaded with that
    /// version and are referenced by it, so the PDS keeps them; their blob objects in `new` are
    /// replaced by the cached ones, verbatim, rather than re-synthesised from the reference.
    pub(crate) async fn edit_media_blobs(
        &self,
        cached: &serde_json::Value,
        new: &mut serde_json::Value,
        media: &[crate::dto::MediaRef],
    ) -> Result<Vec<crate::outbox::BlobData>, crate::engine::EngineError> {
        use crate::outbox::blob;
        fn objects(v: &serde_json::Value, out: &mut Vec<(String, serde_json::Value)>) {
            if let Some(l) = blob::blob_link(v) {
                out.push((l.to_owned(), v.clone()));
                return;
            }
            match v {
                serde_json::Value::Object(m) => m.values().for_each(|x| objects(x, out)),
                serde_json::Value::Array(a) => a.iter().for_each(|x| objects(x, out)),
                _ => {}
            }
        }
        let mut held = Vec::new();
        objects(cached, &mut held);
        for (link, object) in &held {
            blob::substitute_blob(new, link, object);
        }
        let fresh: Vec<crate::dto::MediaRef> = media
            .iter()
            .filter(|m| !held.iter().any(|(l, _)| *l == m.cid))
            .cloned()
            .collect();
        self.media_blobs(&fresh).await
    }

    /// The blob data for a body's media references, ready for `ContentOpts.blobs`.
    ///
    /// Fails when a reference has no prepared bytes on this device: publishing a reference whose
    /// bytes were never uploaded is what the PDS answers with `BlobNotFound (HTTP 400)`, and a
    /// clear refusal here is better than a failed write the user cannot act on.
    pub(crate) async fn media_blobs(
        &self,
        media: &[crate::dto::MediaRef],
    ) -> Result<Vec<crate::outbox::BlobData>, crate::engine::EngineError> {
        let mut out = Vec::with_capacity(media.len());
        for m in media {
            match self.prepared_media(&m.cid).await? {
                Some(blob) => out.push(blob),
                None => {
                    return Err(crate::engine::EngineError::invalid_input(format!(
                        "the bytes for {} are not on this device: prepare the file again",
                        m.name.as_deref().unwrap_or(&m.cid)
                    ))
                    .with_detail("kind", "mediaBytesMissing"));
                }
            }
        }
        Ok(out)
    }
}

/// The type a fetched blob is displayed as (`sync.md` §11.2): the §8.8 table of the sniffed
/// prefix, plus AVIF (inline in the SPA only, never uploaded), or `None` (a download card).
pub fn display_type(prefix: &[u8]) -> Option<&'static str> {
    if let Some(t) = crate::outbox::blob::normalise_mime(prefix) {
        return matches!(
            t,
            "image/png"
                | "image/jpeg"
                | "image/gif"
                | "image/webp"
                | "video/mp4"
                | "video/webm"
                | "audio/mp4"
                | "audio/mpeg"
                | "audio/ogg"
        )
        .then_some(t);
    }
    (prefix.len() >= 12 && &prefix[4..8] == b"ftyp" && matches!(&prefix[8..12], b"avif" | b"avis"))
        .then_some("image/avif")
}

/// A declared `mimeType` through §8.8's names: parameters and case dropped, and the aliases the
/// table names (`audio/x-m4a`, `application/ogg`) mapped to their type.
fn normalise_declared(declared: &str) -> String {
    let base = declared
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match base.as_str() {
        "audio/x-m4a" | "audio/m4a" => "audio/mp4".to_owned(),
        "application/ogg" => "audio/ogg".to_owned(),
        _ => base,
    }
}

/// Whose blob [`load_media`](crate::engine::Engine::load_media) fetches, and what the record
/// declares of it: the item's [`crate::dto::MediaView`], or only the author's DID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaOwner<'a> {
    /// The author's DID: their PDS has the blob.
    pub author: &'a str,
    /// The view's CID, which must be the one asked for.
    pub cid: Option<&'a str>,
    /// The record's `mimeType` (the sniffed type must agree with it to display inline).
    pub mime: Option<&'a str>,
    /// The record's `size` (0 or absent: not stated; the hard cap applies).
    pub size_bytes: u64,
}

impl<'a> MediaOwner<'a> {
    /// The owner and declaration of a media item, from its view.
    pub fn of_view(m: &'a crate::dto::MediaView) -> Self {
        Self {
            author: &m.author,
            cid: Some(&m.cid),
            mime: Some(&m.mime),
            size_bytes: m.size_bytes,
        }
    }

    /// Only the author (the sniffed type decides the display alone).
    pub fn author(did: &'a str) -> Self {
        Self {
            author: did,
            cid: None,
            mime: None,
            size_bytes: 0,
        }
    }
}

impl<H: crate::host::Host> crate::engine::Engine<H> {
    /// The PDS of `did` from its DID document (`sync.md` §11.1 "require identity(did) is doc"),
    /// never from a view or a record, which anyone can write.
    async fn author_pds(
        &self,
        did: &lessdugin_atp::syntax::Did,
    ) -> Result<String, crate::engine::EngineError> {
        let ident = self
            .resolver()
            .resolve_did_doc(did, crate::identity::DID_DOC_TTL_MS)
            .await?;
        ident.pds.ok_or_else(|| {
            crate::engine::EngineError::not_found(format!(
                "{did} has no usable PDS: {}",
                ident.pds_problem.as_deref().unwrap_or("unknown")
            ))
        })
    }

    /// `loadMedia(cid, media?)`: fetches a media blob for display (`sync.md` §11.1).
    ///
    /// With the item's owner (its [`crate::dto::MediaView`], as a rendered body lists it, or only
    /// its author's DID), the blob comes from the AUTHOR's PDS, resolved from their DID document
    /// here (the view's `pds` is not trusted for this), as the author's DID, for any reader,
    /// signed in or not; the cap is the declared `size`, so a longer body is aborted, within
    /// `BLOB_HARD_MAX_BYTES`. Without it the active account's own repo is asked, as before the
    /// owner was passed (a client of the first M4 builds).
    ///
    /// The host streams and CID-checks the bytes, so they never pass through wasm memory
    /// (INFRA-6); the core reads only the first `BLOB_SNIFF_PREFIX_BYTES` to sniff the type.
    /// `mime` is set only when the sniffed type is on the display allowlist and agrees with the
    /// declared one (when the owner states one), and it is the allowlist's constant string;
    /// otherwise the client shows a download card. The caller is the one that decided to load -
    /// [`super::policy`] says whether it may do that without asking.
    pub async fn load_media(
        &self,
        cid: &str,
        owner: Option<MediaOwner<'_>>,
    ) -> Result<crate::dto::LoadedMedia, crate::engine::EngineError> {
        use crate::engine::{EngineError, ErrorCode};
        use crate::host::HostError;
        let hard = crate::limits::BLOB_HARD_MAX_BYTES;
        let (pds, did, declared, max) = match owner {
            Some(m) => {
                if let Some(other) = m.cid.filter(|c| *c != cid) {
                    return Err(EngineError::invalid_input(format!(
                        "loadMedia: the media view is of {other}, not {cid}"
                    )));
                }
                let did = lessdugin_atp::syntax::Did::parse(m.author).map_err(|e| {
                    EngineError::invalid_input(format!("loadMedia: author {:?}: {e}", m.author))
                })?;
                if m.size_bytes > hard {
                    return Err(EngineError::new(
                        ErrorCode::TooLarge,
                        "the file is larger than 300 MiB, which is never loaded (limits.md §5)",
                    )
                    .with_detail("bytes", m.size_bytes));
                }
                let pds = self.author_pds(&did).await?;
                // a declared size of 0 is no size: the hard cap applies
                let max = if m.size_bytes == 0 {
                    hard
                } else {
                    m.size_bytes
                };
                (pds, did, m.mime, max)
            }
            None => {
                let session = self.active_session().await?;
                (session.pds.to_string(), session.did.clone(), None, hard)
            }
        };
        let pds = url::Url::parse(&pds)
            .map_err(|e| EngineError::internal(format!("the PDS URL {pds:?}: {e}")))?;
        let handle = match self.host().fetch_blob(pds, did, cid.to_owned(), max).await {
            Ok(h) => h,
            Err(HostError::HttpStatus(400, _)) => {
                // "Blob not found": not retried, and no copy is kept (sync.md §11.4)
                let _ = self.host().blob_delete(cid).await;
                return Err(EngineError::not_found(
                    "the author's PDS does not have this file (any more)",
                )
                .with_detail("kind", "mediaMissing"));
            }
            Err(HostError::CidMismatch) => {
                return Err(EngineError::new(
                    ErrorCode::Network,
                    "the file failed verification: its bytes do not match its CID",
                )
                .with_detail("kind", "mediaFailedVerification"));
            }
            Err(HostError::TooLarge) => {
                return Err(EngineError::new(
                    ErrorCode::TooLarge,
                    "the PDS sent more bytes than the file declares",
                )
                .with_detail("limit", max));
            }
            Err(e) => return Err(EngineError::from(e)),
        };
        let prefix = self
            .host()
            .blob_read(&handle.cid, 0, crate::limits::BLOB_SNIFF_PREFIX_BYTES)
            .await?;
        let sniffed = display_type(&prefix);
        let mime = match (sniffed, declared) {
            (Some(t), Some(d)) => (normalise_declared(d) == t).then_some(t),
            (t, None) => t,
            (None, Some(_)) => None,
        };
        Ok(crate::dto::LoadedMedia {
            cid: handle.cid,
            size: handle.size,
            url: handle.locator,
            mime: mime.map(str::to_owned),
        })
    }
}

// --- spilled bodies and the autoload inputs of views (`sync.md` §11, `lfm-1.md` §1.3) ---------

impl<H: crate::host::Host> crate::engine::Engine<H> {
    /// Fetches the spilled bodies (`textBlob`) of the items `uris` of `model` that `renders`
    /// does not hold yet, so the views render them (`lfm-1.md` §1.3): from the AUTHOR's PDS,
    /// through the streaming, CID-checked `Host::fetch_blob` with `BLOB_TEXT_MAX_BYTES`, then
    /// read back through `blob_read` and decoded (UTF-8, one leading U+FEFF stripped). A body
    /// that cannot be fetched now (the network, a missing blob) stays pending and is asked for
    /// again after `BODY_BLOB_RETRY_MS`; one that does not match its CID, is not UTF-8 or is too
    /// large renders as unreadable.
    pub(crate) async fn load_spilled_bodies(
        &self,
        model: &crate::model::ForumModel,
        uris: &[String],
        renders: &crate::model::RenderCache,
    ) {
        use crate::model::render::text_blob_cid;
        let Ok(values) = self.values(model, uris).await else {
            return;
        };
        let now = self.host().now_ms();
        for (uri, v) in &values {
            let Some(md) = v.get("body").filter(|b| b.get("text").is_none()) else {
                continue;
            };
            let (Some(blob), Some(x)) = (text_blob_cid(md), model.skel(uri)) else {
                continue;
            };
            if renders.contains(&x.cid)
                || renders.spilled(blob).is_some()
                || renders.spilled_retry_at(blob).is_some_and(|t| now < t)
            {
                continue;
            }
            match self.fetch_text_blob(&x.did, blob).await {
                Ok(s) => renders.put_spilled(blob, s),
                Err(e) => {
                    tracing::debug!(uri = %uri, error = %e, "a spilled body stays pending");
                    renders.spilled_unavailable(
                        blob,
                        now.saturating_add(crate::limits::BODY_BLOB_RETRY_MS),
                    );
                }
            }
        }
    }

    /// One `textBlob` of `did`, fetched and decoded (`data-model.md` §5.2 rule 3).
    async fn fetch_text_blob(
        &self,
        did: &str,
        cid: &str,
    ) -> Result<crate::model::render::Spilled, crate::engine::EngineError> {
        use crate::host::HostError;
        use crate::model::render::Spilled;
        use lessdugin_markdown::BodyError;
        let did = lessdugin_atp::syntax::Did::parse(did)
            .map_err(|e| crate::engine::EngineError::invalid_input(format!("{did:?}: {e}")))?;
        let pds = self.author_pds(&did).await?;
        let pds = url::Url::parse(&pds).map_err(|e| {
            crate::engine::EngineError::internal(format!("the PDS URL {pds:?}: {e}"))
        })?;
        let max = crate::limits::BLOB_TEXT_MAX_BYTES;
        let h = match self.host().fetch_blob(pds, did, cid.to_owned(), max).await {
            Ok(h) => h,
            Err(HostError::CidMismatch) => return Ok(Spilled::Unverified),
            Err(HostError::TooLarge) => return Ok(Spilled::Error(BodyError::TooLarge)),
            Err(e @ HostError::HttpStatus(400, _)) => {
                // "Blob not found": no copy is kept (sync.md §11.4); it stays pending
                let _ = self.host().blob_delete(cid).await;
                return Err(e.into());
            }
            Err(e) => return Err(e.into()),
        };
        // a copy stored by another fetch is held to the same cap
        if h.size > max {
            return Ok(Spilled::Error(BodyError::TooLarge));
        }
        let mut bytes = Vec::with_capacity(h.size as usize);
        while (bytes.len() as u64) < h.size {
            let w = self
                .host()
                .blob_read(&h.cid, bytes.len() as u64, crate::host::BLOB_READ_MAX_BYTES)
                .await?;
            if w.is_empty() {
                break;
            }
            bytes.extend_from_slice(&w);
        }
        Ok(match lessdugin_markdown::decode_text_blob(&bytes) {
            Ok(text) => Spilled::Text(text.into()),
            Err(e) => Spilled::Error(e),
        })
    }

    /// The device's media autoload inputs for views over `model` whose bodies are `values`
    /// (`sync.md` §11.3): the `mediaAutoload` switch (on unless `off`), the `mediaHosts`
    /// allowlist and, when some body has media, how many participants each PDS host serves by
    /// their cached identity rows, and which of them are at level `doc` (nothing is resolved
    /// here).
    pub(crate) async fn media_prefs(
        &self,
        model: &crate::model::ForumModel,
        values: &std::collections::BTreeMap<String, serde_json::Value>,
    ) -> crate::model::render::MediaPrefs {
        use crate::host::KvStore;
        let mut prefs = crate::model::render::MediaPrefs::default();
        let has_media = values.values().any(|v| {
            v.get("body")
                .and_then(|b| b.get("media"))
                .and_then(serde_json::Value::as_array)
                .is_some_and(|m| !m.is_empty())
        });
        if !has_media {
            return prefs;
        }
        let switch = self
            .host()
            .state()
            .get_many(vec![crate::key!("loc", "setting", "mediaAutoload")])
            .await
            .ok()
            .and_then(|mut v| v.pop().flatten());
        prefs.enabled = switch.as_deref() != Some(b"off".as_slice());
        if !prefs.enabled {
            return prefs;
        }
        prefs.allowlist = self.media_hosts().await.unwrap_or_default();
        let dids: Vec<lessdugin_atp::syntax::Did> = model
            .participants()
            .iter()
            .filter_map(|d| lessdugin_atp::syntax::Did::parse(d).ok())
            .collect();
        let found = futures::future::join_all(dids.iter().map(|d| self.resolver().cached(d))).await;
        for id in found.into_iter().flatten() {
            // the forum's sync talks to every participant's PDS, whatever level its row has; an
            // author's own media autoload only at level doc (§11.1)
            if id.level == crate::identity::IdentityLevel::Doc {
                prefs.doc.insert(id.did.to_string());
            }
            if let Some(host) = id
                .pds
                .as_deref()
                .map(policy::host_of)
                .filter(|h| !h.is_empty())
            {
                *prefs.hosts.entry(host).or_default() += 1;
            }
        }
        prefs
    }
}
