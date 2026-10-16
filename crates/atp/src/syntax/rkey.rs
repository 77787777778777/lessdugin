// SPDX-License-Identifier: MIT OR Apache-2.0
//! Record keys (`/specs/record-key`).

use super::{Nsid, SyntaxError, Tid, string_type_impls};

/// Maximum record key length.
pub const MAX_RECORD_KEY_LEN: usize = 512;
const KIND: &str = "record key";

/// A syntactically valid record key: 1..=512 chars of `[A-Za-z0-9._:~-]`, not `.` or `..`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RecordKey(String);

impl RecordKey {
    /// Parses a record key.
    pub fn parse(s: &str) -> Result<Self, SyntaxError> {
        validate(s)?;
        Ok(Self(s.to_owned()))
    }

    /// The record key string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The key as a TID, if it has TID syntax.
    pub fn as_tid(&self) -> Option<Tid> {
        Tid::parse(&self.0).ok()
    }

    /// Checks this key against a lexicon record-key type.
    pub fn check_type(&self, ty: &RecordKeyType) -> Result<(), SyntaxError> {
        ty.check(self)
    }
}

impl From<Tid> for RecordKey {
    fn from(t: Tid) -> Self {
        Self(t.as_str().to_owned())
    }
}

impl From<Nsid> for RecordKey {
    fn from(n: Nsid) -> Self {
        Self(n.as_str().to_owned())
    }
}

/// Lexicon record-key type (`"key"` of a record definition).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RecordKeyType {
    /// `tid`: the key must be a TID.
    Tid,
    /// `nsid`: the key must be an NSID.
    Nsid,
    /// `literal:<value>`: the key must equal the value.
    Literal(String),
    /// `any`: any valid record key.
    Any,
}

impl RecordKeyType {
    /// Parses a lexicon key type string: `tid`, `nsid`, `any` or `literal:<value>`.
    pub fn parse(s: &str) -> Result<Self, SyntaxError> {
        match s {
            "tid" => Ok(Self::Tid),
            "nsid" => Ok(Self::Nsid),
            "any" => Ok(Self::Any),
            _ => match s.strip_prefix("literal:") {
                Some(v) => {
                    validate(v)?;
                    Ok(Self::Literal(v.to_owned()))
                }
                None => Err(SyntaxError::new("record key type", "unknown key type")),
            },
        }
    }

    /// Checks a record key against this type.
    pub fn check(&self, key: &RecordKey) -> Result<(), SyntaxError> {
        let ok = match self {
            Self::Tid => Tid::parse(key.as_str()).is_ok(),
            Self::Nsid => Nsid::parse(key.as_str()).is_ok(),
            Self::Literal(v) => key.as_str() == v,
            Self::Any => true,
        };
        if ok {
            Ok(())
        } else {
            Err(SyntaxError::new(KIND, "does not match the record key type"))
        }
    }
}

impl core::fmt::Display for RecordKeyType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Tid => f.write_str("tid"),
            Self::Nsid => f.write_str("nsid"),
            Self::Literal(v) => write!(f, "literal:{v}"),
            Self::Any => f.write_str("any"),
        }
    }
}

/// `true` for a byte allowed in record keys (and repo path segments).
pub(crate) fn is_rkey_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b':' | b'~')
}

fn validate(s: &str) -> Result<(), SyntaxError> {
    if s.is_empty() || s.len() > MAX_RECORD_KEY_LEN {
        return Err(SyntaxError::new(KIND, "must be 1-512 characters"));
    }
    if s == "." || s == ".." {
        return Err(SyntaxError::new(KIND, "'.' and '..' are not allowed"));
    }
    if !s.bytes().all(is_rkey_byte) {
        return Err(SyntaxError::new(KIND, "invalid character"));
    }
    Ok(())
}

string_type_impls!(RecordKey);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types() {
        let tid = RecordKey::parse("3l2s5xxv2ze2c").unwrap();
        let slf = RecordKey::parse("self").unwrap();
        let nsid = RecordKey::parse("com.example.thing").unwrap();
        assert!(tid.as_tid().is_some());
        assert!(slf.as_tid().is_none());
        let t = RecordKeyType::parse("tid").unwrap();
        assert!(t.check(&tid).is_ok());
        assert!(t.check(&slf).is_err());
        let l = RecordKeyType::parse("literal:self").unwrap();
        assert!(l.check(&slf).is_ok());
        assert!(l.check(&tid).is_err());
        assert_eq!(l.to_string(), "literal:self");
        let n = RecordKeyType::parse("nsid").unwrap();
        assert!(n.check(&nsid).is_ok());
        assert!(n.check(&slf).is_err());
        assert!(RecordKeyType::parse("any").unwrap().check(&slf).is_ok());
        assert!(RecordKeyType::parse("literal:a/b").is_err());
        assert!(RecordKeyType::parse("uuid").is_err());
        assert!(slf.check_type(&RecordKeyType::Any).is_ok());
    }
}
