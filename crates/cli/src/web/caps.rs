// SPDX-License-Identifier: AGPL-3.0-or-later
//! Capabilities and sessions of the bridge (`threat-model.md` §7.2).
//!
//! ```text
//! mint:      cap = base64url_nopad(32 random bytes); unused += (cap, now)
//! exchange:  c ∈ unused, minted ≥ now − BRIDGE_CAPABILITY_TTL_MS
//!                       → unused −= c; used[c] = token = base64url_nopad(32 random bytes);
//!                         sessions += token; Token(token)
//!            c ∈ used   → sessions −= used[c]; Reused        (the link was used twice: revoke)
//!            otherwise  → Refused                             (unknown, or unused and expired)
//! ```
//!
//! An unused capability expires [`BRIDGE_CAPABILITY_TTL_MS`] after it was minted, so a link
//! left in terminal scrollback, a tmux log or a CI log cannot be redeemed later.
//!
//! Everything lives in this process's memory only. Every lookup compares the presented value
//! with every stored one in constant time (no early exit, no hashing of secrets), and values
//! are overwritten when dropped (best effort). Nothing here is ever logged (AUTH-10).

use std::sync::Mutex;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use super::policy::{BRIDGE_CAPABILITY_BYTES, BRIDGE_CAPABILITY_TTL_MS, MAX_UNUSED_CAPABILITIES};

/// Length of a capability or token: base64url without padding of 32 bytes.
pub const SECRET_LEN: usize = 43;

const _: () = assert!(BRIDGE_CAPABILITY_BYTES == 32 && super::policy::BRIDGE_TOKEN_BYTES == 32);

/// A capability or token (43 ASCII characters).
struct Key([u8; SECRET_LEN]);

impl Key {
    fn from_random(bytes: &[u8; 32]) -> Self {
        let mut out = [0u8; SECRET_LEN];
        let n = URL_SAFE_NO_PAD
            .encode_slice(bytes, &mut out)
            .expect("32 bytes encode to 43 characters");
        debug_assert_eq!(n, SECRET_LEN);
        Self(out)
    }

    /// Parses a presented value; only its length is checked here (not secret).
    fn parse(s: &str) -> Option<Self> {
        <[u8; SECRET_LEN]>::try_from(s.as_bytes()).ok().map(Self)
    }

    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).expect("base64url is ASCII")
    }

    /// Constant-time equality.
    fn ct_eq(&self, other: &Self) -> bool {
        let mut diff = 0u8;
        for (a, b) in self.0.iter().zip(other.0.iter()) {
            diff |= a ^ b;
        }
        std::hint::black_box(diff) == 0
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        self.0.fill(0);
        std::hint::black_box(&self.0);
    }
}

/// The index of `k` in `list`, comparing with every element (constant time per element).
fn find(list: &[Key], k: &Key) -> Option<usize> {
    let mut found = None;
    for (i, item) in list.iter().enumerate() {
        if item.ct_eq(k) {
            found = Some(i);
        }
    }
    found
}

#[derive(Default)]
struct Inner {
    /// Unused capabilities and when they were minted (ms).
    unused: Vec<(Key, i64)>,
    /// `(cap, token)` for every exchanged capability.
    used: Vec<(Key, Key)>,
    sessions: Vec<Key>,
}

/// The outcome of `POST /bridge/session`.
#[derive(Debug, PartialEq, Eq)]
pub enum Exchange {
    /// A fresh session token.
    Token(String),
    /// The capability was exchanged before: its session is revoked now.
    Reused,
    /// Unknown capability.
    Refused,
}

/// The capability and session table.
#[derive(Default)]
pub struct Caps {
    inner: Mutex<Inner>,
}

impl Caps {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Mints a capability from 32 random bytes (from the OS CSPRNG) at `now_ms` and returns
    /// it.
    pub fn mint(&self, random: &[u8; 32], now_ms: i64) -> String {
        let key = Key::from_random(random);
        let cap = key.as_str().to_owned();
        let mut inner = self.lock();
        inner.unused.push((key, now_ms));
        if inner.unused.len() > MAX_UNUSED_CAPABILITIES {
            inner.unused.remove(0);
        }
        cap
    }

    /// Exchanges a capability for a session token made from `random` (32 bytes from the OS
    /// CSPRNG), at `now_ms`.
    pub fn exchange(&self, cap: &str, random: &[u8; 32], now_ms: i64) -> Exchange {
        let Some(c) = Key::parse(cap) else {
            return Exchange::Refused;
        };
        let mut inner = self.lock();
        // Expired unused capabilities are forgotten (constant time over the list).
        let ttl = i64::try_from(BRIDGE_CAPABILITY_TTL_MS).unwrap_or(i64::MAX);
        inner
            .unused
            .retain(|(_, minted)| now_ms.saturating_sub(*minted) <= ttl);
        let unused: Vec<Key> = inner.unused.iter().map(|(k, _)| Key(k.0)).collect();
        if let Some(i) = find(&unused, &c) {
            let (cap, _) = inner.unused.remove(i);
            let token = Key::from_random(random);
            let out = token.as_str().to_owned();
            inner.sessions.push(Key(token.0));
            inner.used.push((cap, token));
            return Exchange::Token(out);
        }
        let used: Vec<Key> = inner.used.iter().map(|(c, _)| Key(c.0)).collect();
        if let Some(i) = find(&used, &c) {
            let token = Key(inner.used[i].1.0);
            if let Some(j) = find(&inner.sessions, &token) {
                inner.sessions.remove(j);
            }
            return Exchange::Reused;
        }
        Exchange::Refused
    }

    /// True if `token` names a live session.
    pub fn is_session(&self, token: &str) -> bool {
        let Some(t) = Key::parse(token) else {
            return false;
        };
        find(&self.lock().sessions, &t).is_some()
    }

    /// Live sessions.
    #[cfg(test)]
    pub fn session_count(&self) -> usize {
        self.lock().sessions.len()
    }
}

/// True if `s` has the shape of a capability or token (43 base64url characters).
#[cfg(test)]
pub fn is_secret_shape(s: &str) -> bool {
    s.len() == SECRET_LEN
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rnd(n: u8) -> [u8; 32] {
        [n; 32]
    }

    const T0: i64 = 1_800_000_000_000;

    #[test]
    fn single_use_and_revoke_on_reuse() {
        let caps = Caps::default();
        let cap = caps.mint(&rnd(1), T0);
        assert!(is_secret_shape(&cap));
        let Exchange::Token(token) = caps.exchange(&cap, &rnd(2), T0) else {
            panic!("first exchange")
        };
        assert!(is_secret_shape(&token));
        assert_ne!(token, cap);
        assert!(caps.is_session(&token));
        assert_eq!(caps.session_count(), 1);
        // The second use revokes the first session.
        assert_eq!(caps.exchange(&cap, &rnd(3), T0), Exchange::Reused);
        assert!(!caps.is_session(&token));
        assert_eq!(caps.session_count(), 0);
        // And stays refused.
        assert_eq!(caps.exchange(&cap, &rnd(4), T0), Exchange::Reused);
        assert_eq!(caps.session_count(), 0);
    }

    #[test]
    fn unknown_values_are_refused() {
        let caps = Caps::default();
        let cap = caps.mint(&rnd(1), T0);
        assert_eq!(caps.exchange("", &rnd(2), T0), Exchange::Refused);
        assert_eq!(caps.exchange(&cap[..42], &rnd(2), T0), Exchange::Refused);
        let mut other = cap.clone().into_bytes();
        other[42] = if other[42] == b'A' { b'B' } else { b'A' };
        assert_eq!(
            caps.exchange(std::str::from_utf8(&other).unwrap(), &rnd(2), T0),
            Exchange::Refused
        );
        assert!(!caps.is_session(&cap), "a capability is not a token");
        // Still usable once.
        assert!(matches!(
            caps.exchange(&cap, &rnd(2), T0),
            Exchange::Token(_)
        ));
    }

    #[test]
    fn several_links_are_independent() {
        let caps = Caps::default();
        let a = caps.mint(&rnd(1), T0);
        let b = caps.mint(&rnd(2), T0);
        let Exchange::Token(ta) = caps.exchange(&a, &rnd(3), T0) else {
            panic!()
        };
        let Exchange::Token(tb) = caps.exchange(&b, &rnd(4), T0) else {
            panic!()
        };
        assert_eq!(caps.exchange(&a, &rnd(5), T0), Exchange::Reused);
        assert!(!caps.is_session(&ta));
        assert!(caps.is_session(&tb));
    }

    #[test]
    fn old_unused_capabilities_are_dropped() {
        let caps = Caps::default();
        let first = caps.mint(&rnd(0), T0);
        for i in 0..MAX_UNUSED_CAPABILITIES {
            let mut r = [0u8; 32];
            r[0] = 1;
            r[1] = i as u8;
            caps.mint(&r, T0);
        }
        assert_eq!(caps.exchange(&first, &rnd(9), T0), Exchange::Refused);
    }

    #[test]
    fn unused_capabilities_expire() {
        let ttl = i64::try_from(BRIDGE_CAPABILITY_TTL_MS).unwrap();
        let caps = Caps::default();
        let old = caps.mint(&rnd(1), T0);
        let fresh = caps.mint(&rnd(2), T0 + ttl);
        // Just inside the lifetime.
        let Exchange::Token(_) = caps.exchange(&fresh, &rnd(3), T0 + 2 * ttl) else {
            panic!("a capability is valid for its whole lifetime")
        };
        // Past it: refused, and forgotten.
        assert_eq!(
            caps.exchange(&old, &rnd(4), T0 + ttl + 1),
            Exchange::Refused
        );
        assert_eq!(caps.exchange(&old, &rnd(5), T0), Exchange::Refused);
        // An exchanged capability's reuse is still detected after the lifetime.
        assert_eq!(
            caps.exchange(&fresh, &rnd(6), T0 + 10 * ttl),
            Exchange::Reused
        );
    }
}
