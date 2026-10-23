// SPDX-License-Identifier: AGPL-3.0-or-later
//! Response headers and limits of `lessdugin web` (`threat-model.md` §6, §7; `limits.md` §18).
//!
//! Every response — static file, bridge answer, error, the OAuth callback — carries the §6.1
//! Content Security Policy (§6.2 under `--dev`) and the §6.3 headers ([`apply`]), and none
//! carries an `Access-Control-*` header (WEB-4).

use axum::http::{HeaderMap, HeaderName, HeaderValue, header};

/// `WEB_PORT_DEFAULT`: the default port on `127.0.0.1` (WEB-2).
pub const WEB_PORT_DEFAULT: u16 = 47778;
/// `BRIDGE_CAPABILITY_BYTES`: random bytes per capability (§7.2).
pub const BRIDGE_CAPABILITY_BYTES: usize = 32;
/// `BRIDGE_TOKEN_BYTES`: random bytes per session token (§7.2).
pub const BRIDGE_TOKEN_BYTES: usize = 32;
/// `BRIDGE_JSON_MAX_BODY_BYTES`: JSON bodies of `/bridge/session`, `/bridge/login` and
/// `/bridge/xrpc`.
pub const BRIDGE_JSON_MAX_BODY_BYTES: usize = 1_048_576;
/// `BRIDGE_KV_MAX_BODY_BYTES`: one `/bridge/kv` request.
pub const BRIDGE_KV_MAX_BODY_BYTES: usize = 67_108_864;
/// `uploadBlob` bodies (`BLOB_HARD_MAX_BYTES`).
pub const BRIDGE_BLOB_MAX_BODY_BYTES: usize = lessdugin_core::limits::BLOB_HARD_MAX_BYTES as usize;
/// `OAUTH_LOGIN_TTL_MS`: a pending bridge login expires after this (`limits.md` §16).
pub const OAUTH_LOGIN_TTL_MS: u64 = 600_000;
/// Unused capabilities kept at most (older ones are dropped when more links are minted).
pub const MAX_UNUSED_CAPABILITIES: usize = 64;
/// `BRIDGE_CAPABILITY_TTL_MS`: an unused capability (link) expires this long after it was
/// minted (§7.2).
pub const BRIDGE_CAPABILITY_TTL_MS: u64 = 300_000;
/// `BRIDGE_LOGIN_CONFIRM_MS`: how long a browser-started sign-in waits for the terminal's
/// confirmation before it is refused (§7.4).
pub const BRIDGE_LOGIN_CONFIRM_MS: u64 = 300_000;
/// `BRIDGE_LOCK_WAIT_MS`: the longest one `/bridge/lock` request waits for a busy lock before
/// answering `{"held": false}` (the tab asks again).
pub const BRIDGE_LOCK_WAIT_MS: u64 = 20_000;
/// `BRIDGE_LOCK_HEARTBEAT_MS`: a held `/bridge/lock` writes a byte this often, so a closed
/// connection releases the lock within about this time (§7.4).
pub const BRIDGE_LOCK_HEARTBEAT_MS: u64 = 1_000;
/// `WEB_SHUTDOWN_GRACE_MS`: after Ctrl-C, open connections get this long to finish before
/// the server stops anyway (a second Ctrl-C stops it at once).
pub const WEB_SHUTDOWN_GRACE_MS: u64 = 3_000;

/// The §6.1 production policy, one line.
pub const PRODUCTION_CSP: &str = "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; \
    worker-src 'self'; style-src 'self'; font-src 'self'; img-src 'self' blob:; media-src blob:; \
    connect-src 'self' https: wss:; object-src 'none'; base-uri 'none'; form-action 'none'; \
    frame-ancestors 'none'; require-trusted-types-for 'script'; \
    trusted-types lessdugin-md lessdugin-worker dompurify";

/// The §6.2 development policy (`--dev` only): the production policy with
/// `connect-src 'self' https: wss: http: ws:`.
pub const DEV_CSP: &str = "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; \
    worker-src 'self'; style-src 'self'; font-src 'self'; img-src 'self' blob:; media-src blob:; \
    connect-src 'self' https: wss: http: ws:; object-src 'none'; base-uri 'none'; \
    form-action 'none'; frame-ancestors 'none'; require-trusted-types-for 'script'; \
    trusted-types lessdugin-md lessdugin-worker dompurify";

/// The §6.3 headers sent on every response.
pub const COMMON_HEADERS: [(&str, &str); 4] = [
    ("referrer-policy", "no-referrer"),
    ("x-content-type-options", "nosniff"),
    ("cross-origin-opener-policy", "same-origin"),
    (
        "permissions-policy",
        "camera=(), microphone=(), geolocation=(), payment=()",
    ),
];

/// Adds the CSP and the §6.3 headers, and removes any `Access-Control-*` header. A
/// `Cache-Control` already set by the handler is kept; otherwise `no-store` is set.
pub fn apply(headers: &mut HeaderMap, dev: bool) {
    let csp = if dev { DEV_CSP } else { PRODUCTION_CSP };
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(csp),
    );
    for (name, value) in COMMON_HEADERS {
        headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    if !headers.contains_key(header::CACHE_CONTROL) {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    let cors: Vec<HeaderName> = headers
        .keys()
        .filter(|k| k.as_str().starts_with("access-control-"))
        .cloned()
        .collect();
    for k in cors {
        headers.remove(k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The policy text of `threat-model.md` §6.1, joined as the spec says.
    #[test]
    fn csp_is_the_spec_text() {
        let spec = include_str!("../../../../docs/spec/threat-model.md");
        let start = spec.find("### 6.1 Production policy").unwrap();
        let block = &spec[start..];
        let open = block.find("```\n").unwrap() + 4;
        let close = open + block[open..].find("```").unwrap();
        let text = block[open..close].trim_end().replace('\n', " ");
        assert_eq!(PRODUCTION_CSP, text);
        assert_eq!(
            DEV_CSP,
            PRODUCTION_CSP.replace(
                "connect-src 'self' https: wss:;",
                "connect-src 'self' https: wss: http: ws:;"
            )
        );
        // The same text as the hosted site's `_headers`.
        let headers = include_str!("../../../../deploy/site/_headers");
        assert!(
            headers.contains(&format!("Content-Security-Policy: {PRODUCTION_CSP}\n")),
            "deploy/site/_headers"
        );
        for (name, value) in COMMON_HEADERS {
            let line = headers
                .lines()
                .find(|l| {
                    l.trim()
                        .to_ascii_lowercase()
                        .starts_with(&format!("{name}:"))
                })
                .unwrap_or_else(|| panic!("{name} in _headers"));
            assert_eq!(line.split_once(':').unwrap().1.trim(), value);
        }
    }

    #[test]
    fn apply_sets_headers_and_strips_cors() {
        let mut h = HeaderMap::new();
        h.insert("access-control-allow-origin", HeaderValue::from_static("*"));
        apply(&mut h, false);
        assert_eq!(h[header::CONTENT_SECURITY_POLICY], PRODUCTION_CSP);
        assert_eq!(h[header::CACHE_CONTROL], "no-store");
        assert!(!h.contains_key("access-control-allow-origin"));
        let mut h = HeaderMap::new();
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        apply(&mut h, true);
        assert_eq!(h[header::CONTENT_SECURITY_POLICY], DEV_CSP);
        assert_eq!(h[header::CACHE_CONTROL], "no-cache");
    }
}
