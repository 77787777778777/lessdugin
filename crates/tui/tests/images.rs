// SPDX-License-Identifier: AGPL-3.0-or-later
//! The terminal's image caps (synthesis-v1.md F35/F36): a decompression bomb is refused from its
//! HEADER, before any pixels are allocated, and a disabled client decodes nothing.

use lessdugin_tui::image::{Images, Placeholder, limits};

/// A PNG whose IHDR declares `w` x `h` and whose body is a single IDAT chunk. The pixels are not
/// valid LZW, which is the point: the caps must bite before any decoder reads them.
fn png_declaring(w: u32, h: u32) -> Vec<u8> {
    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut v = (data.len() as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(data);
        v.extend_from_slice(&[0, 0, 0, 0]);
        v
    }
    let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = w.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    v.extend(chunk(b"IHDR", &ihdr));
    v.extend(chunk(b"IDAT", b"not really pixels"));
    v.extend(chunk(b"IEND", b""));
    v
}

#[test]
fn a_huge_declared_frame_is_refused_from_the_header() {
    let mut img = Images::disabled();
    // 100000 x 100000 declarations: the file itself is tiny, the allocation would be 40 GB
    let bomb = png_declaring(100_000, 100_000);
    assert!(bomb.len() < 1000, "the fixture must be tiny");
    let why = img.load(bomb);
    assert_eq!(
        why,
        Some(Placeholder::TooLarge),
        "the header cap must refuse it"
    );
    assert!(!img.has_image());
    assert_eq!(img.placeholder(), Some(Placeholder::TooLarge));
    assert_eq!(img.placeholder().unwrap().text(), "[image too large]");
}

#[test]
fn a_disabled_client_decodes_nothing() {
    let mut img = Images::disabled();
    assert!(!img.has_image());
    assert_eq!(img.placeholder(), Some(Placeholder::Unsupported));
    // even a perfectly good image stays unsupported: a pipe or a test backend shows no pixels
    let real = png_declaring(2, 2);
    assert!(!img.has_image(), "{real:?} must not be decoded");
    assert!(!img.poll(), "nothing to apply");
}

#[test]
fn the_limits_are_the_ones_the_spec_names() {
    // synthesis-v1.md F35/F36: 64 MB, 8192^2 pixels, 200 GIF frames.
    assert_eq!(limits::MAX_INPUT_BYTES, 64 * 1024 * 1024);
    assert_eq!(limits::MAX_SIDE, 8_192);
    assert_eq!(limits::MAX_GIF_FRAMES, 200);
    assert_eq!(limits::DECODE_TIMEOUT.as_secs(), 10);
}

#[test]
fn a_file_over_the_input_cap_is_refused_before_parsing() {
    let mut img = Images::disabled();
    // one byte over the cap, with no valid header at all: the size check must come first
    let big = vec![0u8; limits::MAX_INPUT_BYTES + 1];
    assert_eq!(img.load(big), Some(Placeholder::TooLarge));
}

#[test]
fn garbage_is_unsupported_not_a_panic() {
    let mut img = Images::disabled();
    assert_eq!(img.load(vec![0xFF; 64]), Some(Placeholder::Unsupported));
    assert_eq!(img.load(Vec::new()), Some(Placeholder::Unsupported));
    // every placeholder has text to show
    for p in [
        Placeholder::TooLarge,
        Placeholder::Unsupported,
        Placeholder::Loading,
        Placeholder::Failed,
    ] {
        assert!(p.text().starts_with('['), "{p:?}");
        assert!(!p.text().is_empty());
    }
}
