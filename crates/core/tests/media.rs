// SPDX-License-Identifier: AGPL-3.0-or-later
//! The media pipeline (`threat-model.md` PRIV-4, §5.9.1): metadata is gone, benign content
//! survives, offsets never shift, and a truncated or cyclic container cannot hang.
//!
//! The fixtures are built here, byte by byte, so the privacy properties are asserted against
//! containers whose metadata this test controls exactly (`threat-model.md`'s fixture row).

use lessdugin_core::media::{Format, apply_orientation, prepare, sniff, strip_metadata};

// --- builders -------------------------------------------------------------------------------

/// A JPEG segment: `FF marker`, big-endian length (including itself), payload.
fn seg(marker: u8, payload: &[u8]) -> Vec<u8> {
    let mut v = vec![0xFF, marker];
    let len = (payload.len() + 2) as u16;
    v.extend_from_slice(&len.to_be_bytes());
    v.extend_from_slice(payload);
    v
}

/// A minimal but structurally valid JPEG: SOI, APP0, APP1(EXIF), APP2(ICC), COM, DQT, SOF0, DHT,
/// SOS + entropy data, EOI.
fn jpeg_with_metadata(orientation: u16, gps: bool) -> Vec<u8> {
    let mut v = vec![0xFF, 0xD8];
    // APP0 JFIF (dropped: may hold a thumbnail)
    v.extend(seg(0xE0, b"JFIF\0\x01\x02\0\0\x01\0\x01\0\0"));
    // APP1 EXIF with an orientation tag and (optionally) a GPS IFD pointer
    let mut tiff = Vec::new();
    tiff.extend_from_slice(b"II\x2a\x00"); // little endian, magic
    tiff.extend_from_slice(&8u32.to_le_bytes()); // IFD0 at offset 8
    // IFD0: two entries (orientation, maybe GPS pointer), then next-IFD 0
    let entries: u16 = if gps { 2 } else { 1 };
    tiff.extend_from_slice(&entries.to_le_bytes());
    // 0x0112 Orientation, SHORT, count 1
    tiff.extend_from_slice(&0x0112u16.to_le_bytes());
    tiff.extend_from_slice(&3u16.to_le_bytes());
    tiff.extend_from_slice(&1u32.to_le_bytes());
    tiff.extend_from_slice(&orientation.to_le_bytes());
    tiff.extend_from_slice(&[0, 0]);
    if gps {
        // 0x8825 GPSInfo pointer
        tiff.extend_from_slice(&0x8825u16.to_le_bytes());
        tiff.extend_from_slice(&4u16.to_le_bytes());
        tiff.extend_from_slice(&1u32.to_le_bytes());
        tiff.extend_from_slice(&40u32.to_le_bytes());
    }
    tiff.extend_from_slice(&0u32.to_le_bytes());
    // pad to the GPS offset and put a marker payload there
    while tiff.len() < 40 {
        tiff.push(0);
    }
    tiff.extend_from_slice(b"GPS-GPS-GPS");
    let mut app1 = b"Exif\0\0".to_vec();
    app1.extend_from_slice(&tiff);
    v.extend(seg(0xE1, &app1));
    // APP2 ICC profile (kept)
    let mut icc = b"ICC_PROFILE\0\x01\x01".to_vec();
    icc.extend_from_slice(b"ICC-BYTES");
    v.extend(seg(0xE2, &icc));
    // APP2 MPF (dropped)
    v.extend(seg(0xE2, b"MPF\0second-image"));
    // APP13 IPTC (dropped)
    v.extend(seg(0xED, b"Photoshop 3.0\0IPTC"));
    // COM (dropped)
    v.extend(seg(0xFE, b"a secret comment"));
    // DQT
    v.extend(seg(0xDB, &[0, 1, 2, 3, 4, 5, 6, 7]));
    // SOF0
    v.extend(seg(0xC0, &[8, 0, 8, 0, 8, 1, 1, 0x11, 0]));
    // DHT
    v.extend(seg(0xC4, &[0, 0, 1, 0, 1, 0, 0, 0]));
    // SOS, then entropy-coded data with a stuffed FF00 and an RST0
    v.extend(seg(0xDA, &[1, 1, 0, 0, 63, 0]));
    v.extend_from_slice(&[0x12, 0x34, 0xFF, 0x00, 0x56, 0xFF, 0xD0, 0x78]);
    v.extend_from_slice(&[0xFF, 0xD9]);
    v
}

/// A PNG chunk: length, type, data, CRC (left zero: the stripper does not verify it).
fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut v = (data.len() as u32).to_be_bytes().to_vec();
    v.extend_from_slice(kind);
    v.extend_from_slice(data);
    v.extend_from_slice(&[0, 0, 0, 0]);
    v
}

fn png_with_metadata() -> Vec<u8> {
    let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    v.extend(png_chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]));
    v.extend(png_chunk(b"tEXt", b"Author\0GPS: 51.5,-0.12"));
    v.extend(png_chunk(b"zTXt", b"Comment\0\0compressed"));
    v.extend(png_chunk(b"iTXt", b"XML\0\0\0\0\0<x/>"));
    v.extend(png_chunk(b"eXIf", b"II\x2a\x00gps"));
    v.extend(png_chunk(b"tIME", &[7, 0xE8, 1, 1, 0, 0, 0]));
    v.extend(png_chunk(b"iCCP", b"profile\0\0icc"));
    v.extend(png_chunk(b"IDAT", b"pixels"));
    v.extend(png_chunk(b"IEND", b""));
    v.extend_from_slice(b"AFTER-IEND-TRAILER");
    v
}

/// A WebP: RIFF/WEBP, VP8X with EXIF+XMP flags set, an ICCP chunk, an EXIF chunk, an XMP chunk and
/// a VP8 lossy chunk.
fn webp_with_metadata() -> Vec<u8> {
    let chunk = |id: &[u8; 4], data: &[u8]| {
        let mut v = id.to_vec();
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(data);
        if data.len() % 2 == 1 {
            v.push(0); // pad
        }
        v
    };
    let mut body = Vec::new();
    // VP8X: 1 byte flags, 3 bytes reserved, 3 bytes width-1, 3 bytes height-1
    body.extend(chunk(b"VP8X", &[0b0001_1000, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
    body.extend(chunk(b"ICCP", b"icc-profile"));
    body.extend(chunk(b"EXIF", b"Exif\0\0GPS"));
    body.extend(chunk(b"XMP ", b"<xmp>secret</xmp>"));
    body.extend(chunk(b"VP8 ", b"\x00\x01\x02\x03"));
    let mut v = b"RIFF".to_vec();
    v.extend_from_slice(&((body.len() + 4) as u32).to_le_bytes());
    v.extend_from_slice(b"WEBP");
    v.extend_from_slice(&body);
    v
}

/// A GIF89a with a comment extension and a NETSCAPE application extension.
fn gif_with_comment() -> Vec<u8> {
    let mut v = b"GIF89a".to_vec();
    // logical screen descriptor: 1x1, no global colour table
    v.extend_from_slice(&[1, 0, 1, 0, 0x00, 0, 0]);
    // NETSCAPE loop application extension (kept)
    v.extend_from_slice(&[0x21, 0xFF, 0x0B]);
    v.extend_from_slice(b"NETSCAPE2.0");
    v.extend_from_slice(&[0x03, 0x01, 0x00, 0x00, 0x00]);
    // comment extension (dropped)
    v.extend_from_slice(&[0x21, 0xFE, 0x09]);
    v.extend_from_slice(b"a secret!");
    v.push(0x00);
    // graphic control extension (kept)
    v.extend_from_slice(&[0x21, 0xF9, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00]);
    // image descriptor 1x1 at 0,0, no local colour table
    v.extend_from_slice(&[0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0x00]);
    // minimal LZW data
    v.extend_from_slice(&[0x02, 0x02, 0x44, 0x01, 0x00]);
    v.push(0x3B);
    v
}

/// An MP4: `ftyp`, then a `moov` holding `mvhd` and a nested `udta` (with GPS), then `mdat`.
fn mp4_with_udta() -> Vec<u8> {
    let boxed = |kind: &[u8; 4], payload: &[u8]| {
        let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(payload);
        v
    };
    let udta = boxed(b"udta", b"\xa9xyzGPS:51.5,-0.12-heres-my-location");
    let mvhd = boxed(b"mvhd", &[0u8; 20]);
    let mut moov_payload = mvhd.clone();
    moov_payload.extend_from_slice(&udta);
    let moov = boxed(b"moov", &moov_payload);
    let ftyp = boxed(b"ftyp", b"isom\0\0\x02\0isomiso2");
    let mdat = boxed(b"mdat", b"video-bytes-here");
    let mut v = ftyp;
    v.extend_from_slice(&moov);
    v.extend_from_slice(&mdat);
    v
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

// --- JPEG -----------------------------------------------------------------------------------

#[test]
fn jpeg_drops_every_personal_segment_and_keeps_the_image() {
    let input = jpeg_with_metadata(1, true);
    let out = strip_metadata(&input);
    assert!(out.changed);
    // nothing personal survives
    assert!(!contains(&out.bytes, b"GPS-GPS-GPS"), "GPS IFD survived");
    assert!(!contains(&out.bytes, b"secret comment"), "COM survived");
    assert!(!contains(&out.bytes, b"Photoshop"), "APP13/IPTC survived");
    assert!(!contains(&out.bytes, b"second-image"), "MPF APP2 survived");
    assert!(!contains(&out.bytes, b"JFIF"), "APP0 survived");
    assert!(!contains(&out.bytes, b"Exif"), "APP1/EXIF survived");
    // the image itself survives: ICC profile, tables, and the entropy-coded scan
    assert!(
        contains(&out.bytes, b"ICC_PROFILE"),
        "ICC profile was dropped"
    );
    assert!(contains(&out.bytes, b"ICC-BYTES"));
    assert!(out.bytes.starts_with(&[0xFF, 0xD8]), "no SOI");
    assert!(out.bytes.ends_with(&[0xFF, 0xD9]), "no EOI");
    // the entropy-coded scan survives, including the stuffed FF00 and the RST0 marker
    assert!(contains(
        &out.bytes,
        &[0x12, 0x34, 0xFF, 0x00, 0x56, 0xFF, 0xD0, 0x78]
    ));
}

#[test]
fn jpeg_orientation_is_reported_then_removed() {
    let input = jpeg_with_metadata(6, false);
    let out = strip_metadata(&input);
    assert_eq!(
        out.orientation,
        Some(6),
        "the EXIF orientation must be read"
    );
    assert!(
        !contains(&out.bytes, b"Exif"),
        "EXIF must be gone after the pass"
    );
    // orientation 1 is reported as 1 (the caller skips the rotation)
    let plain = strip_metadata(&jpeg_with_metadata(1, false));
    assert_eq!(plain.orientation, Some(1));
}

#[test]
fn a_fixed_point_is_reached() {
    // stripping an already-stripped file changes nothing (it is idempotent)
    let once = strip_metadata(&jpeg_with_metadata(1, true)).bytes;
    let twice = strip_metadata(&once);
    assert!(!twice.changed, "a second pass must find nothing to drop");
    assert_eq!(twice.bytes, once);
}

#[test]
fn truncated_and_hostile_jpegs_do_not_panic() {
    let full = jpeg_with_metadata(6, true);
    for cut in [2usize, 4, 20, 60, 120, full.len() - 1] {
        let part = &full[..cut.min(full.len())];
        let out = strip_metadata(part);
        // an unparseable stream comes back untouched rather than half-stripped
        assert_eq!(out.bytes.len(), part.len());
    }
    // a length that runs past the end
    let mut evil = vec![0xFF, 0xD8];
    evil.extend_from_slice(&[0xFF, 0xE1, 0xFF, 0xFF]);
    evil.extend_from_slice(b"truncated");
    let out = strip_metadata(&evil);
    assert_eq!(out.bytes, evil);
    // a marker loop: fill bytes with no terminator
    let filler = vec![0xFFu8; 64];
    let _ = strip_metadata(&filler);
}

// --- PNG ------------------------------------------------------------------------------------

#[test]
fn png_drops_text_chunks_exif_and_the_trailer() {
    let input = png_with_metadata();
    let out = strip_metadata(&input);
    assert!(out.changed);
    for needle in [
        b"GPS: 51.5".as_slice(),
        b"Author",
        b"compressed",
        b"<x/>",
        b"eXIf",
        b"tIME",
        b"AFTER-IEND-TRAILER",
    ] {
        assert!(!contains(&out.bytes, needle), "{needle:?} survived");
    }
    // IHDR, iCCP and IDAT survive; IEND ends the file
    assert!(contains(&out.bytes, b"IHDR"));
    assert!(contains(&out.bytes, b"iCCP"));
    assert!(contains(&out.bytes, b"IDAT"));
    // the file ends with the IEND chunk (type + its 4-byte CRC); the trailer is gone
    assert!(
        out.bytes
            .windows(4)
            .last()
            .is_some_and(|w| w == [0, 0, 0, 0]),
        "IEND must be last"
    );
    assert!(contains(&out.bytes, b"IEND"));
    assert!(out.bytes.starts_with(&[0x89, b'P', b'N', b'G']));
}

#[test]
fn truncated_pngs_do_not_panic() {
    let full = png_with_metadata();
    for cut in [8usize, 12, 33, 60] {
        let part = &full[..cut.min(full.len())];
        let out = strip_metadata(part);
        assert_eq!(out.bytes.len(), part.len());
    }
    // a chunk length of u32::MAX must not overflow into a panic
    let mut evil = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    evil.extend_from_slice(&u32::MAX.to_be_bytes());
    evil.extend_from_slice(b"tEXt");
    let out = strip_metadata(&evil);
    assert_eq!(out.bytes, evil);
}

// --- WebP -----------------------------------------------------------------------------------

#[test]
fn webp_drops_exif_and_xmp_and_clears_the_flag_bits() {
    let input = webp_with_metadata();
    let out = strip_metadata(&input);
    assert!(out.changed);
    assert!(!contains(&out.bytes, b"Exif"), "EXIF chunk survived");
    assert!(
        !contains(&out.bytes, b"<xmp>secret</xmp>"),
        "XMP chunk survived"
    );
    // ICCP and the image chunk survive
    assert!(contains(&out.bytes, b"ICCP"));
    assert!(contains(&out.bytes, b"VP8 "));
    // the RIFF size is recomputed and matches the body
    let size = u32::from_le_bytes([out.bytes[4], out.bytes[5], out.bytes[6], out.bytes[7]]);
    assert_eq!(
        size as usize,
        out.bytes.len() - 8,
        "RIFF size not recomputed"
    );
    // VP8X flag bits 3 (EXIF) and 4 (XMP) are cleared
    let vp8x = out
        .bytes
        .windows(4)
        .position(|w| w == b"VP8X")
        .expect("VP8X kept");
    let flags = out.bytes[vp8x + 8];
    assert_eq!(flags & (1 << 3), 0, "EXIF flag still set");
    assert_eq!(flags & (1 << 4), 0, "XMP flag still set");
}

#[test]
fn truncated_webp_does_not_panic() {
    let full = webp_with_metadata();
    for cut in [12usize, 20, 40] {
        let part = &full[..cut.min(full.len())];
        let out = strip_metadata(part);
        assert_eq!(out.bytes.len(), part.len());
    }
    let mut evil = b"RIFF".to_vec();
    evil.extend_from_slice(&u32::MAX.to_le_bytes());
    evil.extend_from_slice(b"WEBPVP8X");
    let _ = strip_metadata(&evil);
}

// --- GIF ------------------------------------------------------------------------------------

#[test]
fn gif_drops_comments_and_keeps_the_loop_and_frames() {
    let input = gif_with_comment();
    let out = strip_metadata(&input);
    assert!(out.changed);
    assert!(!contains(&out.bytes, b"a secret!"), "the comment survived");
    assert!(
        contains(&out.bytes, b"NETSCAPE2.0"),
        "the loop extension was dropped"
    );
    assert!(out.bytes.starts_with(b"GIF89a"));
    assert!(out.bytes.ends_with(&[0x3B]), "no trailer");
    // the image block is intact
    assert!(contains(&out.bytes, &[0x2C, 0, 0, 0, 0, 1, 0, 1, 0]));
}

#[test]
fn a_cyclic_or_truncated_gif_does_not_hang() {
    let full = gif_with_comment();
    for cut in [13usize, 20, 30] {
        let part = &full[..cut.min(full.len())];
        let _ = strip_metadata(part);
    }
    // a block chain that never reaches a zero-size terminator
    let mut evil = b"GIF89a".to_vec();
    evil.extend_from_slice(&[1, 0, 1, 0, 0x00, 0, 0]);
    evil.extend_from_slice(&[0x21, 0xFE, 0x05, 1, 2, 3]);
    let out = strip_metadata(&evil);
    assert_eq!(out.bytes.len(), evil.len());
}

// --- MP4 ------------------------------------------------------------------------------------

#[test]
fn mp4_zeroes_udta_without_moving_a_single_offset() {
    let input = mp4_with_udta();
    let out = strip_metadata(&input);
    assert!(out.changed);
    assert!(
        !contains(&out.bytes, b"GPS:51.5"),
        "the udta payload survived"
    );
    // the length is identical: MP4 is overwritten in place, never rebuilt
    assert_eq!(out.bytes.len(), input.len(), "offsets must not shift");
    // `mdat` still starts at the same offset
    let at = |hay: &[u8], n: &[u8]| hay.windows(n.len()).position(|w| w == n);
    assert_eq!(
        at(&out.bytes, b"mdat"),
        at(&input, b"mdat"),
        "the media data moved"
    );
    // the udta box became `free`, and the mdat payload is untouched
    assert!(
        contains(&out.bytes, b"free"),
        "udta was not overwritten as free"
    );
    assert!(
        contains(&out.bytes, b"video-bytes-here"),
        "mdat was damaged"
    );
    assert!(contains(&out.bytes, b"mvhd"), "mvhd was damaged");
}

#[test]
fn nested_meta_boxes_are_dropped_too() {
    let boxed = |kind: &[u8; 4], payload: &[u8]| {
        let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(payload);
        v
    };
    // moov > trak > meta > (secret)
    let meta = boxed(b"meta", b"\0\0\0\0hidden-metadata");
    let trak = boxed(b"trak", &meta);
    let moov = boxed(b"moov", &trak);
    let ftyp = boxed(b"ftyp", b"isom\0\0\x02\0isomiso2");
    let mut input = ftyp;
    input.extend_from_slice(&moov);
    let out = strip_metadata(&input);
    assert!(
        !contains(&out.bytes, b"hidden-metadata"),
        "nested meta survived"
    );
    assert_eq!(out.bytes.len(), input.len());
}

#[test]
fn a_truncated_or_cyclic_mp4_does_not_hang() {
    let full = mp4_with_udta();
    for cut in [8usize, 16, 40, full.len() - 4] {
        let part = &full[..cut.min(full.len())];
        let out = strip_metadata(part);
        assert_eq!(out.bytes.len(), part.len());
    }
    // a box whose size is 1 (64-bit) but with no room for the 64-bit length
    let mut evil = b"\0\0\0\x01udta".to_vec();
    evil.extend_from_slice(b"short");
    let _ = strip_metadata(&evil);
    // a zero size means "to the end": reachable and bounded
    let mut zero = b"\0\0\0\x00udta".to_vec();
    zero.extend_from_slice(b"payload");
    let _ = strip_metadata(&zero);
}

// --- dispatch and the pipeline --------------------------------------------------------------

#[test]
fn sniff_and_the_unhandled_types() {
    assert_eq!(sniff(&jpeg_with_metadata(1, false)), Some(Format::Jpeg));
    assert_eq!(sniff(&png_with_metadata()), Some(Format::Png));
    assert_eq!(sniff(&webp_with_metadata()), Some(Format::Webp));
    assert_eq!(sniff(&gif_with_comment()), Some(Format::Gif));
    assert_eq!(sniff(&mp4_with_udta()), Some(Format::Iso));
    // PRIV-7: a type not in the image/MP4 tables is uploaded as it is
    for other in [b"%PDF-1.7 secret".as_slice(), b"OggS\0meta", b"<svg/>", b""] {
        assert_eq!(sniff(other), None);
        let out = strip_metadata(other);
        assert!(!out.changed);
        assert_eq!(out.bytes, other);
    }
}

#[test]
fn prepare_strips_then_applies_orientation() {
    // orientation 1: strip only, and the EXIF is gone
    let plain = prepare(jpeg_with_metadata(1, true)).unwrap();
    assert!(!contains(&plain, b"Exif"));
    // orientation 6 on a stream this fixture cannot decode: the bytes still come back stripped
    let rotated = prepare(jpeg_with_metadata(6, true)).unwrap();
    assert!(!contains(&rotated, b"Exif"));
    assert!(!contains(&rotated, b"GPS-GPS-GPS"));
}

#[test]
fn apply_orientation_rotates_a_real_image() {
    // a real 2x1 PNG, encoded by the image crate so the decoder accepts it
    let mut buf = image::RgbaImage::new(2, 1);
    buf.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
    buf.put_pixel(1, 0, image::Rgba([0, 255, 0, 255]));
    let mut px = Vec::new();
    image::DynamicImage::ImageRgba8(buf)
        .write_to(&mut std::io::Cursor::new(&mut px), image::ImageFormat::Png)
        .unwrap();
    let rotated = apply_orientation(&px, 6);
    assert_ne!(rotated, px, "orientation 6 must re-encode");
    // the result decodes, and its dimensions are swapped
    let img = image::load_from_memory(&rotated).expect("re-encoded PNG decodes");
    assert_eq!(
        (img.width(), img.height()),
        (1, 2),
        "90 degrees on 2x1 is 1x2"
    );
    // orientation 1 or an undecodable stream returns the input unchanged
    assert_eq!(apply_orientation(&px, 1), px);
    assert_eq!(apply_orientation(b"not an image", 6), b"not an image");
}
