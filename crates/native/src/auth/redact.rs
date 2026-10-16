// SPDX-License-Identifier: AGPL-3.0-or-later
//! Secret hygiene (`threat-model.md` AUTH-10, TERM-6).
//!
//! - [`Secret`] holds a token, password or code. Its `Debug` prints `<redacted>`, it has no
//!   `Display`, and its buffer is overwritten when dropped (best effort).
//! - [`SecretJson`] does the same for structured secrets (a private DPoP JWK).
//! - [`redact_secrets`] removes secrets from free text: JWTs, app-password-shaped strings, and
//!   the values of sensitive `key=value` / `"key": "value"` / `Header: value` pairs. The log
//!   writer of the CLI and every error message that may quote a request pass through it.

use serde::{Deserialize, Serialize};

/// The replacement text for a removed secret.
pub const REDACTED: &str = "<redacted>";

/// A secret string. `Debug` never shows it; use [`Secret::expose`] at the single place where
/// the value is put on the wire.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    /// Wraps a secret.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The secret value.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// True if the secret is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<String> for Secret {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(REDACTED)
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

/// Overwrites a string's bytes with NULs in place (best effort: copies made earlier by
/// reallocation or by libraries are out of reach).
pub fn wipe(s: &mut String) {
    let len = s.len();
    if len > 0 {
        s.replace_range(..len, &"\0".repeat(len));
        std::hint::black_box(&s);
    }
}

/// A secret JSON value (a private JWK). `Debug` never shows it.
#[derive(Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretJson(pub serde_json::Value);

impl std::fmt::Debug for SecretJson {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(REDACTED)
    }
}

/// Key names whose values are secrets (compared case-insensitively, with `-` and `_`
/// ignored), from AUTH-10: `authorization`, `dpop`, `cookie`, `refresh*`, `password`, `code`,
/// `code_verifier`, plus the token field names of atproto and OAuth.
fn is_sensitive_key(key: &str) -> bool {
    let k: String = key
        .chars()
        .filter(|c| *c != '-' && *c != '_')
        .map(|c| c.to_ascii_lowercase())
        .collect();
    k.starts_with("refresh")
        || matches!(
            k.as_str(),
            "authorization"
                | "proxyauthorization"
                | "dpop"
                | "cookie"
                | "setcookie"
                | "password"
                | "passwd"
                | "apppassword"
                | "code"
                | "codeverifier"
                | "accesstoken"
                | "accessjwt"
                | "idtoken"
                | "token"
                | "clientassertion"
                | "secret"
                | "authfactortoken"
                | "capability"
                | "bridgetoken"
        )
}

fn is_b64url(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Length of a JWT-like token (`eyJ…​.…​.…`) starting at `i`, if there is one.
fn jwt_len(s: &[u8], i: usize) -> Option<usize> {
    if !s[i..].starts_with(b"eyJ") || (i > 0 && is_b64url(s[i - 1])) {
        return None;
    }
    let mut j = i;
    let mut dots = 0;
    while j < s.len() && (is_b64url(s[j]) || (s[j] == b'.' && dots < 2)) {
        if s[j] == b'.' {
            dots += 1;
        }
        j += 1;
    }
    (dots == 2 && j - i > 10).then_some(j - i)
}

/// Length of an app-password-shaped word (`xxxx-xxxx-xxxx-xxxx`) at `i`, if there is one.
fn app_password_len(s: &[u8], i: usize) -> Option<usize> {
    const LEN: usize = 19;
    if i + LEN > s.len() || (i > 0 && (is_word(s[i - 1]) || s[i - 1] == b'-')) {
        return None;
    }
    let w = &s[i..i + LEN];
    let shape = w.iter().enumerate().all(|(k, b)| {
        if k % 5 == 4 {
            *b == b'-'
        } else {
            b.is_ascii_alphanumeric()
        }
    });
    let end_ok = i + LEN == s.len() || !(is_word(s[i + LEN]) || s[i + LEN] == b'-');
    (shape && end_ok).then_some(LEN)
}

/// If a sensitive key ends at `key_end` (exclusive) and is followed by a separator, returns
/// the range of its value.
fn sensitive_value(s: &[u8], key_start: usize, key_end: usize) -> Option<(usize, usize)> {
    let key = std::str::from_utf8(&s[key_start..key_end]).ok()?;
    if !is_sensitive_key(key) {
        return None;
    }
    let mut j = key_end;
    if j < s.len() && (s[j] == b'"' || s[j] == b'\'') {
        j += 1;
    }
    while j < s.len() && s[j] == b' ' {
        j += 1;
    }
    if j >= s.len() || !(s[j] == b'=' || s[j] == b':') {
        return None;
    }
    j += 1;
    while j < s.len() && s[j] == b' ' {
        j += 1;
    }
    let quoted = j < s.len() && (s[j] == b'"' || s[j] == b'\'');
    if quoted {
        j += 1;
    }
    // `Authorization: DPoP <token>`: keep the scheme, redact the credential.
    for scheme in [&b"Bearer "[..], b"DPoP ", b"Basic ", b"bearer ", b"dpop "] {
        if s[j..].starts_with(scheme) {
            j += scheme.len();
            break;
        }
    }
    let start = j;
    while j < s.len() {
        let b = s[j];
        let stop = if quoted {
            b == b'"' || b == b'\''
        } else {
            b.is_ascii_whitespace() || matches!(b, b'&' | b',' | b';' | b'"' | b'\'' | b'}' | b')')
        };
        if stop {
            break;
        }
        j += 1;
    }
    (j > start).then_some((start, j))
}

/// Removes secrets from free text (see the module docs). Non-secret text is unchanged.
pub fn redact_secrets(text: &str) -> String {
    let s = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut copied = 0;
    while i < s.len() {
        if let Some(n) = jwt_len(s, i).or_else(|| app_password_len(s, i)) {
            out.push_str(&text[copied..i]);
            out.push_str(REDACTED);
            i += n;
            copied = i;
            continue;
        }
        if is_word(s[i]) && (i == 0 || !(is_word(s[i - 1]) || s[i - 1] == b'-')) {
            let mut k = i;
            while k < s.len() && (is_word(s[k]) || s[k] == b'-') {
                k += 1;
            }
            if let Some((vs, ve)) = sensitive_value(s, i, k) {
                out.push_str(&text[copied..vs]);
                out.push_str(REDACTED);
                i = ve;
                copied = ve;
                continue;
            }
            i = k;
            continue;
        }
        i += 1;
    }
    out.push_str(&text[copied..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_debug_and_serde() {
        let s = Secret::new("hunter2-token");
        assert_eq!(format!("{s:?}"), "<redacted>");
        assert_eq!(format!("{:?}", Some(&s)), "Some(<redacted>)");
        assert_eq!(serde_json::to_string(&s).unwrap(), "\"hunter2-token\"");
        let back: Secret = serde_json::from_str("\"x\"").unwrap();
        assert_eq!(back.expose(), "x");
        let j = SecretJson(serde_json::json!({"d": "private"}));
        assert!(!format!("{j:?}").contains("private"));
        let mut w = String::from("abc");
        wipe(&mut w);
        assert_eq!(w, "\0\0\0");
    }

    #[test]
    fn redacts_jwts_and_app_passwords() {
        let jwt = "eyJhbGciOiJFUzI1NiJ9.eyJzdWIiOiJkaWQ6cGxjOngifQ.c2lnbmF0dXJl";
        let r = redact_secrets(&format!("token {jwt} end"));
        assert_eq!(r, "token <redacted> end");
        assert_eq!(
            redact_secrets("pw abcd-efgh-ijkl-mnop."),
            "pw <redacted>."
        );
        // Longer dash-separated words are not app passwords.
        assert_eq!(
            redact_secrets("id abcd-efgh-ijkl-mnop-qrst"),
            "id abcd-efgh-ijkl-mnop-qrst"
        );
        assert_eq!(redact_secrets("eyJ alone"), "eyJ alone");
    }

    #[test]
    fn redacts_sensitive_pairs() {
        assert_eq!(
            redact_secrets("GET /cb?code=abc123&state=xyz"),
            "GET /cb?code=<redacted>&state=xyz"
        );
        assert_eq!(
            redact_secrets(r#"{"accessJwt": "aaa", "handle": "alice.test", "refreshJwt":"bbb"}"#),
            r#"{"accessJwt": "<redacted>", "handle": "alice.test", "refreshJwt":"<redacted>"}"#
        );
        assert_eq!(
            redact_secrets("authorization: DPoP tok.en x"),
            "authorization: DPoP <redacted> x"
        );
        assert_eq!(
            redact_secrets("Authorization=Bearer abc"),
            "Authorization=Bearer <redacted>"
        );
        assert_eq!(
            redact_secrets("refresh_token=r1 code_verifier=v1 password: p1"),
            "refresh_token=<redacted> code_verifier=<redacted> password: <redacted>"
        );
        // Ordinary text keeps its words.
        assert_eq!(
            redact_secrets("error code 4: the code is wrong"),
            "error code 4: the code is wrong"
        );
        assert_eq!(redact_secrets("barcode=1 xcode: 2"), "barcode=1 xcode: 2");
        assert_eq!(redact_secrets("ünïcode ok"), "ünïcode ok");
    }
}
