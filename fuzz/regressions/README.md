<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->

# Kept crash reproducers

Minimised inputs of crashes the fuzz targets found, one directory per target. Seeding
(`cargo xtask fuzz`) copies them into `corpus/<target>/`, so every run replays them first. Each
one also has a regression test in the crate that was fixed. The test holds the input inline, so
the crates' own test runs (native and wasm32) never read this directory.

| Target | File | Bug | Fix and regression test |
|---|---|---|---|
| `render` | `invalid-punycode-host` | `safe_href` linked hosts that the WHATWG URL parser rejects or rewrites (malformed Punycode, out-of-range or short IPv4, invalid IPv6), so ammonia stripped the `href` | `crates/markdown/src/url.rs` `whatwg_host_ok` and `punycode_ok` (`host_ok` of `lfm-1.md` §5.1, NOTES bug 22); `url::tests::hosts_the_url_parser_accepts`, `tests/props.rs` `linked_hosts_survive_ammonia`, `links.json#punycode-invalid-dropped` and siblings |
