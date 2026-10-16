// SPDX-License-Identifier: MIT OR Apache-2.0
//! Namespaced identifiers (`/specs/nsid`).

use super::{SyntaxError, string_type_impls, valid_label};

/// Maximum NSID length.
pub const MAX_NSID_LEN: usize = 317;
const KIND: &str = "nsid";

/// A syntactically valid NSID, such as `online.7778777.lessdugin.post`.
///
/// The domain authority (all but the last segment, in reversed-domain order) follows hostname
/// rules: segments may start with a digit, except the first (the TLD). The name is
/// `[A-Za-z][A-Za-z0-9]{0,62}`. NSIDs are stored as written (never force-lowercased).
///
/// The overall length is capped at 317. The spec's separate 253-character cap on the authority is
/// *not* enforced, matching the reference implementations and the interop fixtures (which list a
/// 283-character authority as valid).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Nsid(String);

impl Nsid {
    /// Parses an NSID.
    pub fn parse(s: &str) -> Result<Self, SyntaxError> {
        validate(s)?;
        Ok(Self(s.to_owned()))
    }

    /// The full NSID.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The authority segments in NSID order, e.g. `online.7778777.lessdugin`.
    pub fn authority(&self) -> &str {
        match self.0.rfind('.') {
            Some(i) => &self.0[..i],
            None => "",
        }
    }

    /// The authority as a hostname (segments reversed), e.g. `lessdugin.7778777.online`.
    pub fn domain_authority(&self) -> String {
        let mut segs: Vec<&str> = self.authority().split('.').collect();
        segs.reverse();
        segs.join(".")
    }

    /// The final (name) segment, e.g. `post`.
    pub fn name(&self) -> &str {
        match self.0.rfind('.') {
            Some(i) => &self.0[i + 1..],
            None => &self.0,
        }
    }

    /// `true` if the authority part is lowercase (the normalized form for references).
    pub fn is_authority_normalized(&self) -> bool {
        !self.authority().bytes().any(|b| b.is_ascii_uppercase())
    }
}

fn validate(s: &str) -> Result<(), SyntaxError> {
    let b = s.as_bytes();
    if b.len() > MAX_NSID_LEN {
        return Err(SyntaxError::new(KIND, "too long"));
    }
    let Some(dot) = b.iter().rposition(|&c| c == b'.') else {
        return Err(SyntaxError::new(KIND, "needs at least three segments"));
    };
    let (authority, name) = (&b[..dot], &b[dot + 1..]);
    let mut segments = 0usize;
    for (i, seg) in authority.split(|&c| c == b'.').enumerate() {
        if !valid_label(seg) {
            return Err(SyntaxError::new(KIND, "invalid authority segment"));
        }
        if i == 0 && !seg[0].is_ascii_alphabetic() {
            return Err(SyntaxError::new(
                KIND,
                "first segment must start with a letter",
            ));
        }
        segments += 1;
    }
    if segments < 2 {
        return Err(SyntaxError::new(KIND, "needs at least three segments"));
    }
    if name.is_empty() || name.len() > 63 {
        return Err(SyntaxError::new(KIND, "name must be 1-63 characters"));
    }
    if !name[0].is_ascii_alphabetic() || !name.iter().all(u8::is_ascii_alphanumeric) {
        return Err(SyntaxError::new(KIND, "name must be [A-Za-z][A-Za-z0-9]*"));
    }
    Ok(())
}

string_type_impls!(Nsid);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digit_leading_segments() {
        let n = Nsid::parse("online.7778777.lessdugin.post").unwrap();
        assert_eq!(n.authority(), "online.7778777.lessdugin");
        assert_eq!(n.domain_authority(), "lessdugin.7778777.online");
        assert_eq!(n.name(), "post");
        assert!(n.is_authority_normalized());
        assert!(Nsid::parse("7778777.online.post").is_err());
        assert!(Nsid::parse("online.7778777.2post").is_err());
        assert!(
            !Nsid::parse("Com.Example.foo")
                .unwrap()
                .is_authority_normalized()
        );
    }
}
