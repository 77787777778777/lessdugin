// SPDX-License-Identifier: AGPL-3.0-or-later
//! Order-preserving tuple encoding for store keys (`docs/spec/sync.md`, store schema).
//!
//! A key is a sequence of components. The byte encoding sorts exactly like the tuple:
//! - `Str`/`Bytes`: tag `0x02`, then the bytes with `0x00` escaped as `0x00 0xFF`, then `0x00`.
//! - `Int` (i64): tag `0x15`, then 8 bytes big-endian with the sign bit flipped.
//! - `Max`: tag `0xFF` with no payload — sorts after every other component (for range ends).
//!
//! Because a terminated string component never prefixes a longer one, a prefix of components is
//! a byte prefix of every key that extends it, so prefix scans select exactly those keys.

use thiserror::Error;

/// One tuple component.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Part {
    /// A byte string (strings are encoded as their UTF-8 bytes).
    Bytes(Vec<u8>),
    /// A signed integer.
    Int(i64),
    /// Greater than any other component (only for range ends; never stored).
    Max,
}

impl From<&str> for Part {
    fn from(s: &str) -> Self {
        Part::Bytes(s.as_bytes().to_vec())
    }
}
impl From<String> for Part {
    fn from(s: String) -> Self {
        Part::Bytes(s.into_bytes())
    }
}
impl From<&[u8]> for Part {
    fn from(b: &[u8]) -> Self {
        Part::Bytes(b.to_vec())
    }
}
impl From<i64> for Part {
    fn from(i: i64) -> Self {
        Part::Int(i)
    }
}

const TAG_BYTES: u8 = 0x02;
const TAG_INT: u8 = 0x15;
const TAG_MAX: u8 = 0xFF;

/// Encode a tuple.
pub fn encode(parts: &[Part]) -> Vec<u8> {
    let mut out = Vec::new();
    for p in parts {
        encode_part(&mut out, p);
    }
    out
}

/// Append one component to an encoded key.
pub fn encode_part(out: &mut Vec<u8>, part: &Part) {
    match part {
        Part::Bytes(b) => {
            out.push(TAG_BYTES);
            for &byte in b {
                out.push(byte);
                if byte == 0x00 {
                    out.push(0xFF);
                }
            }
            out.push(0x00);
        }
        Part::Int(i) => {
            out.push(TAG_INT);
            out.extend_from_slice(&((*i as u64) ^ (1u64 << 63)).to_be_bytes());
        }
        Part::Max => out.push(TAG_MAX),
    }
}

/// Decoding errors.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum KeyError {
    /// The key ended in the middle of a component.
    #[error("truncated key")]
    Truncated,
    /// An unknown component tag.
    #[error("unknown tag {0:#04x}")]
    UnknownTag(u8),
}

/// Decode a tuple.
pub fn decode(mut key: &[u8]) -> Result<Vec<Part>, KeyError> {
    let mut parts = Vec::new();
    while let Some((&tag, rest)) = key.split_first() {
        key = rest;
        match tag {
            TAG_BYTES => {
                let mut b = Vec::new();
                loop {
                    let (&byte, rest) = key.split_first().ok_or(KeyError::Truncated)?;
                    key = rest;
                    if byte == 0x00 {
                        if key.first() == Some(&0xFF) {
                            b.push(0x00);
                            key = &key[1..];
                        } else {
                            break;
                        }
                    } else {
                        b.push(byte);
                    }
                }
                parts.push(Part::Bytes(b));
            }
            TAG_INT => {
                if key.len() < 8 {
                    return Err(KeyError::Truncated);
                }
                let mut arr = [0u8; 8];
                arr.copy_from_slice(&key[..8]);
                key = &key[8..];
                parts.push(Part::Int((u64::from_be_bytes(arr) ^ (1u64 << 63)) as i64));
            }
            TAG_MAX => parts.push(Part::Max),
            other => return Err(KeyError::UnknownTag(other)),
        }
    }
    Ok(parts)
}

/// Convenience: build a key from heterogeneous parts.
#[macro_export]
macro_rules! key {
    ($($p:expr),* $(,)?) => {
        $crate::store::key::encode(&[$($crate::store::key::Part::from($p)),*])
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn arb_part() -> impl Strategy<Value = Part> {
        prop_oneof![
            proptest::collection::vec(any::<u8>(), 0..8).prop_map(Part::Bytes),
            any::<i64>().prop_map(Part::Int),
        ]
    }

    proptest! {
        #[test]
        fn roundtrip(parts in proptest::collection::vec(arb_part(), 0..5)) {
            prop_assert_eq!(decode(&encode(&parts)).unwrap(), parts);
        }

        #[test]
        fn order_preserved(a in proptest::collection::vec(arb_part(), 0..4),
                           b in proptest::collection::vec(arb_part(), 0..4)) {
            // Only compare tuples whose components have the same kinds position by position,
            // mirroring how store keys are used (fixed schemas per prefix).
            let same_shape = a.iter().zip(&b).all(|(x, y)| std::mem::discriminant(x) == std::mem::discriminant(y));
            prop_assume!(same_shape);
            prop_assert_eq!(a.cmp(&b), encode(&a).cmp(&encode(&b)));
        }

        #[test]
        fn prefix_is_byte_prefix(a in proptest::collection::vec(arb_part(), 1..4),
                                 rest in proptest::collection::vec(arb_part(), 0..3)) {
            let mut full = a.clone();
            full.extend(rest);
            prop_assert!(encode(&full).starts_with(&encode(&a)));
        }
    }

    #[test]
    fn max_sorts_last() {
        assert!(encode(&["a".into(), Part::Max]) > encode(&["a".into(), "\u{10FFFF}".into()]));
        assert!(encode(&["a".into(), Part::Max]) < encode(&["b".into()]));
    }

    #[test]
    fn macro_builds_keys() {
        let k = crate::key!("rec", "fk123", 5i64);
        assert_eq!(
            decode(&k).unwrap(),
            vec![Part::from("rec"), Part::from("fk123"), Part::Int(5)]
        );
    }
}
