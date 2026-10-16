// SPDX-License-Identifier: MIT OR Apache-2.0
//! Strict canonical DAG-CBOR (DRISL) codec over [`Ipld`], restricted to the atproto data model.
//!
//! **Encoding** is canonical: shortest-form heads, map keys sorted length-first then bytewise (the
//! RFC 7049 canonical order used by DAG-CBOR; `serde_ipld_dagcbor` 0.7 produces the same order by
//! sorting encoded entries, which the tests cross-check), CIDs as tag 42 over `0x00 ‖ cid-bytes`.
//! Floats, integers outside `i64` and CIDv0 links are rejected.
//!
//! **Decoding** accepts only canonical input and the atproto subset: no floats, no indefinite
//! lengths, no tags other than 42 (in its 2-byte form), no `undefined`/simple values, minimal-length
//! heads, string map keys in strictly increasing canonical order (so duplicates are rejected),
//! valid UTF-8, integers within `i64`, CIDv1 links only, no trailing bytes. Caps from [`Limits`]
//! (depth, container length, string/byte length, total node count) are checked *before* recursing
//! or allocating; nothing is preallocated from a declared length.

use std::collections::BTreeMap;

use ::cid::{Cid, Version};

use crate::Ipld;
use crate::datamodel::{DataModelError, Limits};

const TAG_CID: u64 = 42;

/// Encodes a value as canonical DAG-CBOR, using [`Limits::default`] for the depth guard.
pub fn encode(v: &Ipld) -> Result<Vec<u8>, DataModelError> {
    encode_with(v, &Limits::default())
}

/// Encodes a value as canonical DAG-CBOR with an explicit depth guard (`limits.max_depth`).
pub fn encode_with(v: &Ipld, limits: &Limits) -> Result<Vec<u8>, DataModelError> {
    let mut out = Vec::new();
    enc(&mut out, v, 0, limits.max_depth)?;
    Ok(out)
}

fn write_head(out: &mut Vec<u8>, major: u8, arg: u64) {
    let m = major << 5;
    if arg < 24 {
        out.push(m | arg as u8);
    } else if arg <= u64::from(u8::MAX) {
        out.push(m | 24);
        out.push(arg as u8);
    } else if arg <= u64::from(u16::MAX) {
        out.push(m | 25);
        out.extend_from_slice(&(arg as u16).to_be_bytes());
    } else if arg <= u64::from(u32::MAX) {
        out.push(m | 26);
        out.extend_from_slice(&(arg as u32).to_be_bytes());
    } else {
        out.push(m | 27);
        out.extend_from_slice(&arg.to_be_bytes());
    }
}

/// Canonical DAG-CBOR map key order: shorter keys first, then bytewise.
pub fn canonical_key_cmp(a: &str, b: &str) -> core::cmp::Ordering {
    a.len()
        .cmp(&b.len())
        .then_with(|| a.as_bytes().cmp(b.as_bytes()))
}

fn enc(out: &mut Vec<u8>, v: &Ipld, depth: usize, max_depth: usize) -> Result<(), DataModelError> {
    match v {
        Ipld::Null => out.push(0xf6),
        Ipld::Bool(false) => out.push(0xf4),
        Ipld::Bool(true) => out.push(0xf5),
        Ipld::Integer(i) => {
            let i = i64::try_from(*i).map_err(|_| DataModelError::IntegerOutOfRange)?;
            if i >= 0 {
                write_head(out, 0, i as u64);
            } else {
                // -1 - i is in 0..=i64::MAX for negative i.
                write_head(out, 1, (-1 - i) as u64);
            }
        }
        Ipld::Float(_) => return Err(DataModelError::Float),
        Ipld::String(s) => {
            write_head(out, 3, s.len() as u64);
            out.extend_from_slice(s.as_bytes());
        }
        Ipld::Bytes(b) => {
            write_head(out, 2, b.len() as u64);
            out.extend_from_slice(b);
        }
        Ipld::List(items) => {
            let d = enter(depth, max_depth)?;
            write_head(out, 4, items.len() as u64);
            for item in items {
                enc(out, item, d, max_depth)?;
            }
        }
        Ipld::Map(map) => {
            let d = enter(depth, max_depth)?;
            write_head(out, 5, map.len() as u64);
            let mut entries: Vec<(&String, &Ipld)> = map.iter().collect();
            entries.sort_by(|a, b| canonical_key_cmp(a.0, b.0));
            for (k, val) in entries {
                write_head(out, 3, k.len() as u64);
                out.extend_from_slice(k.as_bytes());
                enc(out, val, d, max_depth)?;
            }
        }
        Ipld::Link(cid) => {
            if cid.version() != Version::V1 {
                return Err(DataModelError::InvalidLink("CIDv0 is not allowed".into()));
            }
            let bytes = cid.to_bytes();
            write_head(out, 6, TAG_CID);
            write_head(out, 2, bytes.len() as u64 + 1);
            out.push(0x00);
            out.extend_from_slice(&bytes);
        }
    }
    Ok(())
}

fn enter(depth: usize, max_depth: usize) -> Result<usize, DataModelError> {
    let d = depth + 1;
    if d > max_depth {
        Err(DataModelError::DepthExceeded { max: max_depth })
    } else {
        Ok(d)
    }
}

/// Decodes strict canonical DAG-CBOR with [`Limits::default`].
pub fn decode(bytes: &[u8]) -> Result<Ipld, DataModelError> {
    decode_with(bytes, &Limits::default())
}

/// Decodes strict canonical DAG-CBOR with explicit limits. The whole slice must be one value.
pub fn decode_with(bytes: &[u8], limits: &Limits) -> Result<Ipld, DataModelError> {
    let mut d = Decoder {
        buf: bytes,
        pos: 0,
        limits,
        nodes: 0,
    };
    let v = d.value(0)?;
    if d.pos != bytes.len() {
        return Err(d.err("trailing bytes after value"));
    }
    Ok(v)
}

fn string_too_long(len: usize, max: usize) -> DataModelError {
    DataModelError::StringTooLong { len, max }
}

fn bytes_too_long(len: usize, max: usize) -> DataModelError {
    DataModelError::BytesTooLong { len, max }
}

fn container_too_large(len: usize, max: usize) -> DataModelError {
    DataModelError::ContainerTooLarge { len, max }
}

struct Decoder<'a> {
    buf: &'a [u8],
    pos: usize,
    limits: &'a Limits,
    nodes: usize,
}

enum Head {
    Major(u8, u64),
    Simple(u8),
}

impl<'a> Decoder<'a> {
    fn err(&self, reason: &'static str) -> DataModelError {
        DataModelError::Cbor {
            offset: self.pos,
            reason,
        }
    }

    fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], DataModelError> {
        if n > self.remaining() {
            return Err(self.err("unexpected end of input"));
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn head(&mut self) -> Result<Head, DataModelError> {
        let b = self.take(1)?[0];
        let (major, info) = (b >> 5, b & 0x1f);
        if major == 7 {
            return Ok(Head::Simple(b));
        }
        let arg = match info {
            0..=23 => u64::from(info),
            24 => {
                let v = u64::from(self.take(1)?[0]);
                if v < 24 {
                    return Err(self.err("non-minimal integer encoding"));
                }
                v
            }
            25 => {
                let s = self.take(2)?;
                let v = u64::from(u16::from_be_bytes([s[0], s[1]]));
                if v <= u64::from(u8::MAX) {
                    return Err(self.err("non-minimal integer encoding"));
                }
                v
            }
            26 => {
                let s = self.take(4)?;
                let v = u64::from(u32::from_be_bytes([s[0], s[1], s[2], s[3]]));
                if v <= u64::from(u16::MAX) {
                    return Err(self.err("non-minimal integer encoding"));
                }
                v
            }
            27 => {
                let s = self.take(8)?;
                let mut a = [0u8; 8];
                a.copy_from_slice(s);
                let v = u64::from_be_bytes(a);
                if v <= u64::from(u32::MAX) {
                    return Err(self.err("non-minimal integer encoding"));
                }
                v
            }
            31 => return Err(self.err("indefinite-length items are not allowed")),
            _ => return Err(self.err("reserved additional information")),
        };
        Ok(Head::Major(major, arg))
    }

    /// Checks a declared length against a cap before anything is read or allocated.
    fn len_arg(
        arg: u64,
        max: usize,
        too_big: fn(usize, usize) -> DataModelError,
    ) -> Result<usize, DataModelError> {
        let len = usize::try_from(arg).unwrap_or(usize::MAX);
        if len > max {
            Err(too_big(len, max))
        } else {
            Ok(len)
        }
    }

    fn string(&mut self, arg: u64) -> Result<String, DataModelError> {
        let len = Self::len_arg(arg, self.limits.max_string_bytes, string_too_long)?;
        let start = self.pos;
        let raw = self.take(len)?;
        match core::str::from_utf8(raw) {
            Ok(s) => Ok(s.to_owned()),
            Err(_) => Err(DataModelError::Cbor {
                offset: start,
                reason: "invalid UTF-8 in string",
            }),
        }
    }

    fn value(&mut self, depth: usize) -> Result<Ipld, DataModelError> {
        self.nodes += 1;
        if self.nodes > self.limits.max_total_nodes {
            return Err(DataModelError::TooManyNodes {
                max: self.limits.max_total_nodes,
            });
        }
        match self.head()? {
            Head::Simple(b) => match b {
                0xf4 => Ok(Ipld::Bool(false)),
                0xf5 => Ok(Ipld::Bool(true)),
                0xf6 => Ok(Ipld::Null),
                0xf9..=0xfb => Err(DataModelError::Float),
                _ => Err(self.err("unsupported simple value")),
            },
            Head::Major(0, arg) => {
                let v = i64::try_from(arg).map_err(|_| DataModelError::IntegerOutOfRange)?;
                Ok(Ipld::Integer(i128::from(v)))
            }
            Head::Major(1, arg) => {
                let n = i64::try_from(arg).map_err(|_| DataModelError::IntegerOutOfRange)?;
                Ok(Ipld::Integer(-1 - i128::from(n)))
            }
            Head::Major(2, arg) => {
                let len = Self::len_arg(arg, self.limits.max_bytes_len, bytes_too_long)?;
                Ok(Ipld::Bytes(self.take(len)?.to_vec()))
            }
            Head::Major(3, arg) => Ok(Ipld::String(self.string(arg)?)),
            Head::Major(4, arg) => {
                let len = Self::len_arg(arg, self.limits.max_container_len, container_too_large)?;
                let d = enter(depth, self.limits.max_depth)?;
                if len > self.remaining() {
                    return Err(self.err("array longer than remaining input"));
                }
                let mut items = Vec::new();
                for _ in 0..len {
                    items.push(self.value(d)?);
                }
                Ok(Ipld::List(items))
            }
            Head::Major(5, arg) => {
                let len = Self::len_arg(arg, self.limits.max_container_len, container_too_large)?;
                let d = enter(depth, self.limits.max_depth)?;
                if len > self.remaining() / 2 {
                    return Err(self.err("map longer than remaining input"));
                }
                let mut map = BTreeMap::new();
                let mut prev: Option<String> = None;
                for _ in 0..len {
                    let key_at = self.pos;
                    let key = match self.head()? {
                        Head::Major(3, arg) => self.string(arg)?,
                        _ => {
                            return Err(DataModelError::Cbor {
                                offset: key_at,
                                reason: "map key must be a string",
                            });
                        }
                    };
                    if let Some(p) = &prev
                        && canonical_key_cmp(p, &key) != core::cmp::Ordering::Less
                    {
                        return Err(DataModelError::Cbor {
                            offset: key_at,
                            reason: "map keys must be unique and in canonical order",
                        });
                    }
                    let val = self.value(d)?;
                    prev = Some(key.clone());
                    map.insert(key, val);
                }
                Ok(Ipld::Map(map))
            }
            Head::Major(6, tag) => {
                if tag != TAG_CID {
                    return Err(self.err("only tag 42 (CID) is allowed"));
                }
                let at = self.pos;
                let Head::Major(2, arg) = self.head()? else {
                    return Err(DataModelError::Cbor {
                        offset: at,
                        reason: "tag 42 must wrap a byte string",
                    });
                };
                let len = Self::len_arg(arg, self.limits.max_bytes_len, bytes_too_long)?;
                let raw = self.take(len)?;
                let Some((&0x00, cid_bytes)) = raw.split_first() else {
                    return Err(DataModelError::Cbor {
                        offset: at,
                        reason: "CID must start with the 0x00 multibase prefix",
                    });
                };
                let cid: Cid = crate::cid::from_bytes(cid_bytes)
                    .map_err(|e| DataModelError::InvalidLink(e.to_string()))?;
                Ok(Ipld::Link(cid))
            }
            Head::Major(..) => Err(self.err("invalid major type")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        data_encoding::HEXLOWER.decode(s.as_bytes()).unwrap()
    }

    #[test]
    fn integers_minimal() {
        for (v, h) in [
            (0i128, "00"),
            (23, "17"),
            (24, "1818"),
            (255, "18ff"),
            (256, "190100"),
            (65_536, "1a00010000"),
            (4_294_967_296, "1b0000000100000000"),
            (-1, "20"),
            (-25, "3818"),
            (i128::from(i64::MAX), "1b7fffffffffffffff"),
            (i128::from(i64::MIN), "3b7fffffffffffffff"),
        ] {
            let b = encode(&Ipld::Integer(v)).unwrap();
            assert_eq!(b, hex(h), "{v}");
            assert_eq!(decode(&b).unwrap(), Ipld::Integer(v));
        }
        assert!(encode(&Ipld::Integer(i128::from(i64::MAX) + 1)).is_err());
        assert!(decode(&hex("1b8000000000000000")).is_err());
        assert!(decode(&hex("3b8000000000000000")).is_err());
        for non_minimal in ["1817", "190017", "1a0000ffff", "1b00000000ffffffff", "3817"] {
            assert!(decode(&hex(non_minimal)).is_err(), "{non_minimal}");
        }
    }

    #[test]
    fn map_order() {
        let mut m = BTreeMap::new();
        for k in ["bb", "a", "c", "aaa", "ab"] {
            m.insert(k.to_owned(), Ipld::Null);
        }
        let b = encode(&Ipld::Map(m.clone())).unwrap();
        // a, c, ab, bb, aaa
        assert_eq!(b, hex("a56161f66163f6626162f6626262f663616161f6"));
        assert_eq!(decode(&b).unwrap(), Ipld::Map(m));
        // bytewise order (aaa before c) is not canonical
        assert!(decode(&hex("a263616161f66163f6")).is_err());
        // duplicate keys
        assert!(decode(&hex("a26161f66161f6")).is_err());
        // non-string key
        assert!(decode(&hex("a101f6")).is_err());
    }

    #[test]
    fn rejects_non_atproto() {
        for bad in [
            "f93c00",                                                                           // f16
            "fa3f800000",         // f32
            "fb3ff0000000000000", // f64
            "f7",                 // undefined
            "f0",                 // simple
            "5f4100ff",           // indefinite bytes
            "9fff",               // indefinite array
            "bfff",               // indefinite map
            "c11a514b67b0",       // tag 1
            "d9002a4100",         // tag 42, non-minimal
            "d82a4101",           // CID without 0x00 prefix
            "d82a6100",           // CID as text
            "d82a5823001220000000000000000000000000000000000000000000000000000000000000000000", // CIDv0
            "1c",     // reserved
            "6161ff", // trailing
            "62c328", // invalid UTF-8
            "",
            "8201",
        ] {
            assert!(decode(&hex(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn caps() {
        let deep = |n: usize| {
            let mut v = Ipld::List(vec![]);
            for _ in 1..n {
                v = Ipld::List(vec![v]);
            }
            v
        };
        assert!(encode(&deep(32)).is_ok());
        assert!(matches!(
            encode(&deep(33)),
            Err(DataModelError::DepthExceeded { .. })
        ));
        let mut b = vec![0x81u8; 32];
        b.push(0x80);
        assert!(matches!(
            decode(&b),
            Err(DataModelError::DepthExceeded { .. })
        ));
        b.drain(..1);
        assert!(decode(&b).is_ok());
        // declared huge lengths fail fast without allocation
        assert!(decode(&hex("9b00000000ffffffff")).is_err());
        assert!(decode(&hex("5b00000000ffffffff")).is_err());
        assert!(decode(&hex("7a7fffffff")).is_err());
        assert!(decode(&hex("9a00010000")).is_err());
        let lim = Limits {
            max_container_len: 2,
            max_string_bytes: 3,
            max_bytes_len: 1,
            max_total_nodes: 5,
            ..Limits::default()
        };
        assert!(decode_with(&hex("83010203"), &lim).is_err());
        assert!(decode_with(&hex("8201820203"), &lim).is_ok());
        assert!(decode_with(&hex("820182028103"), &lim).is_err());
        assert!(decode_with(&hex("6461626364"), &lim).is_err());
        assert!(decode_with(&hex("420102"), &lim).is_err());
    }

    #[test]
    fn link_roundtrip() {
        let cid = crate::cid::for_dag_cbor(b"x");
        let b = encode(&Ipld::Link(cid)).unwrap();
        assert_eq!(&b[..4], &hex("d82a5825")[..]);
        assert_eq!(decode(&b).unwrap(), Ipld::Link(cid));
    }
}
