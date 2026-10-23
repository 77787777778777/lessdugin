// SPDX-License-Identifier: AGPL-3.0-or-later
// Strings that exist only in test-only SPA code: the e2e hook and devnet switch, and the mock
// engine with its fixture world and fake sign-in. `build.rs` refuses to embed a build that
// contains any of them into a release binary (`threat-model.md` WEB-5, SUP-3). The lists are
// the ones `web/scripts/check-release-bundle.mjs` checks (`web::assets` tests that they agree).
// Included by `build.rs` and by `src/web/assets.rs`, so it holds items only.

/// Strings found only in src/host/e2e.ts and the e2e branch of src/engine/real.ts.
pub const E2E_MARKERS: &[&str] = &[
    "__lessduginE2E",
    "__LESSDUGIN_E2E__",
    "End-to-end test build",
];

/// Strings found only in the mock engine (packages/engine/src/mock*) and src/engine/mock.ts.
pub const MOCK_MARKERS: &[&str] = &[
    "Demo mode: the in-memory mock engine",
    "Mock sign-in:",
    "resolved from the mock directory",
    "pds.example.test",
];

/// File names the marker scan reads (source maps hold every module's original text, dead
/// branches included, so they are skipped, as the JS check does).
pub fn scanned(name: &str) -> bool {
    [".js", ".mjs", ".html", ".json", ".css", ".wasm", ".txt"]
        .iter()
        .any(|ext| name.ends_with(ext))
}

/// The `mode` values of `build-info.json` that a release binary may embed. A build-info file
/// without `mode` is judged by the markers alone.
pub const RELEASE_MODES: &[&str] = &["release", "production", "pages"];

/// The test-only markers in `body`, if any.
pub fn markers_in(body: &[u8]) -> Vec<&'static str> {
    E2E_MARKERS
        .iter()
        .chain(MOCK_MARKERS)
        .copied()
        .filter(|m| body.windows(m.len()).any(|w| w == m.as_bytes()))
        .collect()
}

/// The `mode` field of a `build-info.json`, if it has one (a minimal reader: the file is
/// `{"sha256": …, "files": {…}, "mode"?: "…"}` as `web/vite.config.ts` writes it).
pub fn build_info_mode(json: &str) -> Option<String> {
    let at = json.find("\"mode\"")?;
    let rest = json[at + 6..].trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}
