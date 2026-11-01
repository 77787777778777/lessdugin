<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->

# Kept crash reproducers

Minimised inputs of crashes the fuzz targets found, one directory per target. Seeding
(`cargo xtask fuzz`) copies them into `corpus/<target>/`, so every run replays them first. Each
one also has a regression test in the crate that was fixed. The test holds the input inline, so
the crates' own test runs (native and wasm32) never read this directory.

| Target | File | Bug | Fix and regression test |
|---|---|---|---|
| `render` | `invalid-punycode-host` | `safe_href` linked hosts that the WHATWG URL parser rejects or rewrites (malformed Punycode, out-of-range or short IPv4, invalid IPv6), so ammonia stripped the `href` | `crates/markdown/src/url.rs` `whatwg_host_ok` and `punycode_ok` (`host_ok` of `lfm-1.md` §5.1, NOTES bug 22); `url::tests::hosts_the_url_parser_accepts`, `tests/props.rs` `linked_hosts_survive_ammonia`, `links.json#punycode-invalid-dropped` and siblings |
| `media` | `redundant-jpeg-fill-bytes` | a JPEG with a run of `0xFF` fill bytes had them collapsed but was reported `changed: false`, which promises byte-identical output | `crates/core/src/media.rs` (collapsing padding is a reported change); `crates/core/tests/media.rs` `redundant_jpeg_fill_bytes_are_a_reported_change` |
| `media` | `oversized-extended-box-header` | an oversized extended-size MP4 box was clamped to its parent's end, leaving its 16-byte header outside the box and causing an out-of-bounds payload slice | `crates/core/src/media/mod.rs` (check payload bounds before slicing); `crates/core/tests/media.rs` `an_oversized_extended_mp4_box_is_rejected_before_its_header_is_sliced` |
