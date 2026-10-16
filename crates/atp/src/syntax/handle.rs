// SPDX-License-Identifier: MIT OR Apache-2.0
//! Handles (`/specs/handle`).

use super::{SyntaxError, string_type_impls, valid_label};

/// Maximum handle length.
pub const MAX_HANDLE_LEN: usize = 253;

/// TLDs that are syntactically valid but must fail registration and resolution
/// (`/specs/handle`, "Additional Non-Syntax Restrictions").
pub const DISALLOWED_TLDS: &[&str] = &[
    "alt",
    "arpa",
    "example",
    "internal",
    "invalid",
    "local",
    "localhost",
    "onion",
];

/// The special placeholder handle used when a DID has no valid handle.
pub const INVALID_HANDLE: &str = "handle.invalid";

const KIND: &str = "handle";

/// A syntactically valid handle, normalized to ASCII lowercase.
///
/// [`Handle::parse`] accepts any ASCII case and lowercases; `as_str()` is always the normalized
/// form. Syntax validity does not imply resolvability: see [`Handle::has_disallowed_tld`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Handle(String);

impl Handle {
    /// Parses and lowercases a handle.
    pub fn parse(s: &str) -> Result<Self, SyntaxError> {
        validate_hostname(s)?;
        Ok(Self(s.to_ascii_lowercase()))
    }

    /// The normalized (lowercase) handle.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The last label (top-level domain).
    pub fn tld(&self) -> &str {
        self.0.rsplit('.').next().unwrap_or("")
    }

    /// `true` if the TLD is in [`DISALLOWED_TLDS`] (such handles must never resolve).
    pub fn has_disallowed_tld(&self) -> bool {
        DISALLOWED_TLDS.contains(&self.tld())
    }

    /// `true` for the `.test` TLD (development only; should fail in real-world environments).
    pub fn is_test_tld(&self) -> bool {
        self.tld() == "test"
    }

    /// `true` for the special [`INVALID_HANDLE`] placeholder.
    pub fn is_invalid_placeholder(&self) -> bool {
        self.0 == INVALID_HANDLE
    }
}

/// Validates handle syntax (a hostname with at least two labels whose TLD does not start with a
/// digit). Case-insensitive.
pub(crate) fn validate_hostname(s: &str) -> Result<(), SyntaxError> {
    let b = s.as_bytes();
    if b.is_empty() {
        return Err(SyntaxError::new(KIND, "empty"));
    }
    if b.len() > MAX_HANDLE_LEN {
        return Err(SyntaxError::new(KIND, "too long"));
    }
    let mut labels = 0usize;
    let mut last: &[u8] = &[];
    for label in b.split(|&c| c == b'.') {
        if !valid_label(label) {
            return Err(SyntaxError::new(KIND, "invalid label"));
        }
        labels += 1;
        last = label;
    }
    if labels < 2 {
        return Err(SyntaxError::new(KIND, "needs at least two labels"));
    }
    if !last[0].is_ascii_alphabetic() {
        return Err(SyntaxError::new(KIND, "TLD must start with a letter"));
    }
    Ok(())
}

string_type_impls!(Handle);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes() {
        let h = Handle::parse("XX.LCS.MIT.EDU").unwrap();
        assert_eq!(h.as_str(), "xx.lcs.mit.edu");
        assert_eq!(h.tld(), "edu");
        assert!(!h.has_disallowed_tld());
    }

    #[test]
    fn tlds() {
        assert!(Handle::parse("laptop.local").unwrap().has_disallowed_tld());
        assert!(Handle::parse("blah.arpa").unwrap().has_disallowed_tld());
        assert!(
            Handle::parse("handle.invalid")
                .unwrap()
                .is_invalid_placeholder()
        );
        assert!(Handle::parse("alice.test").unwrap().is_test_tld());
        assert!(Handle::parse("john.0").is_err());
        assert!(Handle::parse("org").is_err());
        assert!(Handle::parse("name.org.").is_err());
    }
}
