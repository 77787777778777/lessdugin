// SPDX-License-Identifier: MIT OR Apache-2.0
//! Syntax types: [`Did`], [`Handle`], [`Nsid`], [`RecordKey`], [`AtUri`], [`LooseAtUri`],
//! [`AtIdentifier`], [`Datetime`] (and [`Tid`], re-exported from [`crate::tid`]).
//!
//! Every type validates on construction, so holding a value proves its syntax. All types
//! implement `FromStr`, `Display`, `AsRef<str>`, `as_str()`, `Ord` and serde as plain strings
//! (deserialization validates). Parsing never panics and never allocates more than the input.
//!
//! Rules follow the atproto specs (`/specs/did`, `/specs/handle`, `/specs/nsid`,
//! `/specs/record-key`, `/specs/at-uri-scheme`, `/specs/lexicon` datetime); lessdugin-specific
//! policy (only `did:plc` and host-only `did:web`) is exposed separately via
//! [`Did::check_lessdugin_policy`] and [`AtUri::check_lessdugin_canonical`].

mod aturi;
mod datetime;
mod did;
mod handle;
mod nsid;
mod rkey;

pub use aturi::{AtIdentifier, AtUri, LooseAtUri, MAX_AT_URI_LEN};
pub use datetime::Datetime;
pub use did::{Did, DidMethod, MAX_DID_LEN};
pub use handle::{DISALLOWED_TLDS, Handle, INVALID_HANDLE, MAX_HANDLE_LEN};
pub use nsid::{MAX_NSID_LEN, Nsid};
pub use rkey::{MAX_RECORD_KEY_LEN, RecordKey, RecordKeyType};

pub use crate::tid::Tid;
pub(crate) use rkey::is_rkey_byte;

/// A syntax validation failure: which kind of identifier, and why.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid {kind}: {reason}")]
pub struct SyntaxError {
    /// The identifier kind (`"did"`, `"handle"`, `"nsid"`, …).
    pub kind: &'static str,
    /// A short, static explanation.
    pub reason: &'static str,
}

impl SyntaxError {
    pub(crate) const fn new(kind: &'static str, reason: &'static str) -> Self {
        Self { kind, reason }
    }
}

/// Implements the common string-newtype traits for a type with `parse(&str)` and `as_str()`.
macro_rules! string_type_impls {
    ($ty:ident) => {
        impl ::core::fmt::Display for $ty {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl ::core::str::FromStr for $ty {
            type Err = $crate::syntax::SyntaxError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::parse(s)
            }
        }

        impl ::core::convert::TryFrom<&str> for $ty {
            type Error = $crate::syntax::SyntaxError;
            fn try_from(s: &str) -> Result<Self, Self::Error> {
                Self::parse(s)
            }
        }

        impl ::core::convert::TryFrom<String> for $ty {
            type Error = $crate::syntax::SyntaxError;
            fn try_from(s: String) -> Result<Self, Self::Error> {
                Self::parse(&s)
            }
        }

        impl ::core::convert::AsRef<str> for $ty {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl From<$ty> for String {
            fn from(v: $ty) -> String {
                v.as_str().to_owned()
            }
        }

        impl PartialEq<str> for $ty {
            fn eq(&self, other: &str) -> bool {
                self.as_str() == other
            }
        }

        impl PartialEq<&str> for $ty {
            fn eq(&self, other: &&str) -> bool {
                self.as_str() == *other
            }
        }

        impl ::serde::Serialize for $ty {
            fn serialize<S: ::serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $ty {
            fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                struct V;
                impl ::serde::de::Visitor<'_> for V {
                    type Value = $ty;
                    fn expecting(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                        f.write_str(concat!("a string holding a valid ", stringify!($ty)))
                    }
                    fn visit_str<E: ::serde::de::Error>(self, v: &str) -> Result<$ty, E> {
                        $ty::parse(v).map_err(E::custom)
                    }
                }
                d.deserialize_str(V)
            }
        }
    };
}
pub(crate) use string_type_impls;

/// `true` for an ASCII letter or digit.
#[inline]
pub(crate) fn is_alnum(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

/// Validates one hostname label (handle / NSID authority segment): 1..=63 chars of
/// `[A-Za-z0-9-]`, not starting or ending with `-`.
pub(crate) fn valid_label(label: &[u8]) -> bool {
    !label.is_empty()
        && label.len() <= 63
        && label.iter().all(|&b| is_alnum(b) || b == b'-')
        && label[0] != b'-'
        && label[label.len() - 1] != b'-'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert!(valid_label(b"a"));
        assert!(valid_label(b"a-b"));
        assert!(valid_label(b"7778777"));
        assert!(!valid_label(b""));
        assert!(!valid_label(b"-a"));
        assert!(!valid_label(b"a-"));
        assert!(!valid_label(b"a_b"));
        assert!(!valid_label(&[b'a'; 64]));
    }

    #[test]
    fn serde_roundtrip_all_types() {
        let did: Did = serde_json::from_str("\"did:plc:ewvi7nxzyoun6zhxrhs64oiz\"").unwrap();
        assert_eq!(
            serde_json::to_string(&did).unwrap(),
            "\"did:plc:ewvi7nxzyoun6zhxrhs64oiz\""
        );
        assert!(serde_json::from_str::<Did>("\"nope\"").is_err());
        let h: Handle = serde_json::from_str("\"Alice.Example.COM\"").unwrap();
        assert_eq!(h.as_str(), "alice.example.com");
        let n: Nsid = serde_json::from_str("\"online.7778777.lessdugin.post\"").unwrap();
        assert_eq!(n.name(), "post");
        let k: RecordKey = serde_json::from_str("\"self\"").unwrap();
        assert_eq!(k, "self");
        let u: AtUri = serde_json::from_str(
            "\"at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/app.bsky.feed.post/3l2s5xxv2ze2c\"",
        )
        .unwrap();
        assert_eq!(u.rkey().as_str(), "3l2s5xxv2ze2c");
        let d: Datetime = serde_json::from_str("\"2024-01-02T03:04:05.678Z\"").unwrap();
        assert_eq!(d.to_string(), "2024-01-02T03:04:05.678Z");
        let t: Tid = serde_json::from_str("\"3l2s5xxv2ze2c\"").unwrap();
        assert_eq!(serde_json::to_string(&t).unwrap(), "\"3l2s5xxv2ze2c\"");
        assert!(serde_json::from_str::<Tid>("12").is_err());
    }
}
