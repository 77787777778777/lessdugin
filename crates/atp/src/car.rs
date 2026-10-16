// SPDX-License-Identifier: MIT OR Apache-2.0
//! CARv1 reader with hard caps (no preallocation from declared lengths), and a writer.
//!
//! Format: `varint(len) ‖ header` then sections `varint(len) ‖ cid ‖ block`, where the header is
//! DAG-CBOR `{version: 1, roots: [cid…]}`. Every block is verified against its CID (sha2-256;
//! other hashes and CIDv0 are rejected). Duplicate blocks are tolerated when identical. Varints are
//! unsigned LEB128, minimal, at most 9 bytes (63 bits).

use std::collections::BTreeMap;

use ::cid::Cid;

use crate::Ipld;
use crate::cid::CidError;
use crate::datamodel::Limits;

/// Caps for [`read_car`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CarLimits {
    /// Maximum size of the whole CAR.
    pub max_total_bytes: usize,
    /// Maximum number of block sections (duplicates count).
    pub max_blocks: usize,
    /// Maximum size of one block (excluding its CID).
    pub max_block_bytes: usize,
    /// Maximum size of the header.
    pub max_header_bytes: usize,
    /// Maximum number of roots.
    pub max_roots: usize,
}

impl CarLimits {
    /// Caps for `proofs/1` CARs (`docs/design/synthesis-v1.md` §3.6): 1 MB total, ≤ 2,000
    /// blocks.
    pub const PROOF: Self = Self {
        max_total_bytes: 1_000_000,
        max_blocks: 2_000,
        max_block_bytes: 1_000_000,
        max_header_bytes: 4096,
        max_roots: 16,
    };
}

impl Default for CarLimits {
    fn default() -> Self {
        Self::PROOF
    }
}

/// CAR errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CarError {
    /// The CAR exceeds `max_total_bytes`.
    #[error("CAR too large ({len} bytes, max {max})")]
    TooLarge {
        /// Input size.
        len: usize,
        /// The cap.
        max: usize,
    },
    /// More sections than `max_blocks`.
    #[error("too many blocks (max {0})")]
    TooManyBlocks(usize),
    /// A block exceeds `max_block_bytes`.
    #[error("block too large at byte {offset}")]
    BlockTooLarge {
        /// Offset of the section.
        offset: usize,
    },
    /// Malformed varint.
    #[error("invalid varint at byte {0}")]
    Varint(usize),
    /// The input ends inside a section or the header.
    #[error("truncated CAR at byte {0}")]
    Truncated(usize),
    /// Malformed header.
    #[error("invalid CAR header: {0}")]
    Header(&'static str),
    /// A section's CID is malformed or unsupported.
    #[error("invalid block CID at byte {offset}: {err}")]
    Cid {
        /// Offset of the section.
        offset: usize,
        /// The CID problem.
        err: CidError,
    },
    /// A block does not hash to its CID.
    #[error("block does not match its CID {0}")]
    HashMismatch(Cid),
    /// The same CID appears twice with different bytes (only possible for non-blessed codecs
    /// sharing a digest, kept for completeness).
    #[error("conflicting duplicate block {0}")]
    ConflictingDuplicate(Cid),
}

/// A parsed, fully verified CAR.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Car {
    /// Header roots, in order.
    pub roots: Vec<Cid>,
    /// Blocks by CID (each verified against its CID).
    pub blocks: BTreeMap<Cid, Vec<u8>>,
}

impl Car {
    /// The block for `cid`, if present.
    pub fn get(&self, cid: &Cid) -> Option<&[u8]> {
        self.blocks.get(cid).map(Vec::as_slice)
    }
}

/// Reads an unsigned LEB128 varint at `pos` (minimal, ≤ 9 bytes / 63 bits). Returns the value and
/// the number of bytes used.
pub fn read_uvarint(buf: &[u8], pos: usize) -> Result<(u64, usize), CarError> {
    let mut v: u64 = 0;
    for i in 0..9 {
        let Some(&b) = buf.get(pos + i) else {
            return Err(CarError::Truncated(pos + i));
        };
        v |= u64::from(b & 0x7f) << (7 * i);
        if b & 0x80 == 0 {
            if b == 0 && i > 0 {
                return Err(CarError::Varint(pos)); // non-minimal
            }
            return Ok((v, i + 1));
        }
    }
    Err(CarError::Varint(pos))
}

/// Appends an unsigned LEB128 varint.
pub fn write_uvarint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn header_limits() -> Limits {
    Limits {
        max_depth: 4,
        max_container_len: 256,
        max_string_bytes: 64,
        max_bytes_len: 128,
        max_total_nodes: 1024,
    }
}

/// Parses and verifies a CARv1 under `limits`.
pub fn read_car(buf: &[u8], limits: &CarLimits) -> Result<Car, CarError> {
    if buf.len() > limits.max_total_bytes {
        return Err(CarError::TooLarge {
            len: buf.len(),
            max: limits.max_total_bytes,
        });
    }
    let (hlen, n) = read_uvarint(buf, 0)?;
    let hlen = usize::try_from(hlen).map_err(|_| CarError::Header("length"))?;
    if hlen == 0 || hlen > limits.max_header_bytes {
        return Err(CarError::Header("bad header length"));
    }
    let hend = n
        .checked_add(hlen)
        .filter(|e| *e <= buf.len())
        .ok_or(CarError::Truncated(n))?;
    let roots = parse_header(&buf[n..hend], limits)?;
    let mut car = Car {
        roots,
        blocks: BTreeMap::new(),
    };
    let mut pos = hend;
    let mut count = 0usize;
    while pos < buf.len() {
        let section = pos;
        count += 1;
        if count > limits.max_blocks {
            return Err(CarError::TooManyBlocks(limits.max_blocks));
        }
        let (len, n) = read_uvarint(buf, pos)?;
        pos += n;
        let len = usize::try_from(len).map_err(|_| CarError::Truncated(section))?;
        if len == 0 {
            return Err(CarError::Truncated(section));
        }
        // CID (≤ ~100 bytes) + block.
        if len > limits.max_block_bytes.saturating_add(128) {
            return Err(CarError::BlockTooLarge { offset: section });
        }
        let end = pos
            .checked_add(len)
            .filter(|e| *e <= buf.len())
            .ok_or(CarError::Truncated(section))?;
        let body = &buf[pos..end];
        let (cid, used) = crate::cid::read_prefix(body).map_err(|err| CarError::Cid {
            offset: section,
            err,
        })?;
        let data = &body[used..];
        if data.len() > limits.max_block_bytes {
            return Err(CarError::BlockTooLarge { offset: section });
        }
        crate::cid::verify(&cid, data).map_err(|err| match err {
            CidError::Mismatch(c) => CarError::HashMismatch(c),
            err => CarError::Cid {
                offset: section,
                err,
            },
        })?;
        match car.blocks.get(&cid) {
            Some(existing) if existing.as_slice() != data => {
                return Err(CarError::ConflictingDuplicate(cid));
            }
            Some(_) => {}
            None => {
                car.blocks.insert(cid, data.to_vec());
            }
        }
        pos = end;
    }
    Ok(car)
}

fn parse_header(bytes: &[u8], limits: &CarLimits) -> Result<Vec<Cid>, CarError> {
    let v = crate::dagcbor::decode_with(bytes, &header_limits())
        .map_err(|_| CarError::Header("not DAG-CBOR"))?;
    let Ipld::Map(map) = v else {
        return Err(CarError::Header("not a map"));
    };
    if map.get("version") != Some(&Ipld::Integer(1)) {
        return Err(CarError::Header("version must be 1"));
    }
    let Some(Ipld::List(roots)) = map.get("roots") else {
        return Err(CarError::Header("missing roots"));
    };
    if roots.len() > limits.max_roots {
        return Err(CarError::Header("too many roots"));
    }
    roots
        .iter()
        .map(|r| match r {
            Ipld::Link(c) => Ok(*c),
            _ => Err(CarError::Header("root is not a link")),
        })
        .collect()
}

/// Writes a CARv1 with the given roots and blocks (in the given order). Intended for tests,
/// fixtures and fake servers; blocks are written as given (not re-verified).
pub fn write_car<'a>(
    roots: &[Cid],
    blocks: impl IntoIterator<Item = (&'a Cid, &'a [u8])>,
) -> Vec<u8> {
    let mut header = std::collections::BTreeMap::new();
    header.insert(
        "roots".to_owned(),
        Ipld::List(roots.iter().map(|c| Ipld::Link(*c)).collect()),
    );
    header.insert("version".to_owned(), Ipld::Integer(1));
    // A two-key map of links never fails to encode.
    let h = crate::dagcbor::encode(&Ipld::Map(header)).unwrap_or_default();
    let mut out = Vec::new();
    write_uvarint(&mut out, h.len() as u64);
    out.extend_from_slice(&h);
    for (cid, data) in blocks {
        let c = cid.to_bytes();
        write_uvarint(&mut out, (c.len() + data.len()) as u64);
        out.extend_from_slice(&c);
        out.extend_from_slice(data);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> (Cid, Cid, Vec<u8>) {
        let a = crate::dagcbor::encode(&Ipld::String("a".into())).unwrap();
        let b = b"raw bytes".to_vec();
        let ca = crate::cid::for_dag_cbor(&a);
        let cb = crate::cid::for_raw(&b);
        let car = write_car(&[ca], [(&ca, a.as_slice()), (&cb, b.as_slice())]);
        (ca, cb, car)
    }

    #[test]
    fn roundtrip() {
        let (ca, cb, car) = sample();
        let c = read_car(&car, &CarLimits::PROOF).unwrap();
        assert_eq!(c.roots, vec![ca]);
        assert_eq!(c.blocks.len(), 2);
        assert_eq!(c.get(&cb), Some(&b"raw bytes"[..]));
    }

    #[test]
    fn varints() {
        let mut v = Vec::new();
        for n in [0u64, 1, 127, 128, 300, 16_384, (1 << 63) - 1] {
            v.clear();
            write_uvarint(&mut v, n);
            assert_eq!(read_uvarint(&v, 0).unwrap(), (n, v.len()));
        }
        assert!(read_uvarint(&[0x80, 0x00], 0).is_err()); // non-minimal
        assert!(read_uvarint(&[0x80], 0).is_err()); // truncated
        assert!(read_uvarint(&[0xff; 10], 0).is_err()); // too long / overflow
        assert!(read_uvarint(&[], 0).is_err());
    }

    #[test]
    fn caps() {
        let (_, _, car) = sample();
        let tiny = CarLimits {
            max_total_bytes: car.len() - 1,
            ..CarLimits::PROOF
        };
        assert!(matches!(
            read_car(&car, &tiny),
            Err(CarError::TooLarge { .. })
        ));
        let one = CarLimits {
            max_blocks: 1,
            ..CarLimits::PROOF
        };
        assert!(matches!(
            read_car(&car, &one),
            Err(CarError::TooManyBlocks(1))
        ));
        let small = CarLimits {
            max_block_bytes: 4,
            ..CarLimits::PROOF
        };
        assert!(matches!(
            read_car(&car, &small),
            Err(CarError::BlockTooLarge { .. })
        ));
    }

    #[test]
    fn malformed() {
        let (ca, _, car) = sample();
        // truncated anywhere: never panics; inside the header always fails
        for cut in 0..car.len() {
            let r = read_car(&car[..cut], &CarLimits::PROOF);
            if cut <= usize::from(car[0]) {
                assert!(r.is_err());
            }
        }
        // truncation inside a section always fails
        assert!(read_car(&car[..car.len() - 1], &CarLimits::PROOF).is_err());
        // corrupted block byte
        let mut bad = car.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert!(matches!(
            read_car(&bad, &CarLimits::PROOF),
            Err(CarError::HashMismatch(_))
        ));
        // huge declared section length
        let mut huge = car[..car.len()].to_vec();
        huge.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x0f]);
        assert!(read_car(&huge, &CarLimits::PROOF).is_err());
        // zero-length section
        let mut zero = car.clone();
        zero.push(0);
        assert!(read_car(&zero, &CarLimits::PROOF).is_err());
        // header: wrong version
        let mut h = BTreeMap::new();
        h.insert("roots".to_owned(), Ipld::List(vec![Ipld::Link(ca)]));
        h.insert("version".to_owned(), Ipld::Integer(2));
        let hb = crate::dagcbor::encode(&Ipld::Map(h)).unwrap();
        let mut v2 = Vec::new();
        write_uvarint(&mut v2, hb.len() as u64);
        v2.extend_from_slice(&hb);
        assert!(matches!(
            read_car(&v2, &CarLimits::PROOF),
            Err(CarError::Header(_))
        ));
        assert!(read_car(&[0x00], &CarLimits::PROOF).is_err());
        assert!(read_car(&[], &CarLimits::PROOF).is_err());
    }

    #[test]
    fn duplicates_and_unsupported() {
        let a = crate::dagcbor::encode(&Ipld::Bool(true)).unwrap();
        let ca = crate::cid::for_dag_cbor(&a);
        let car = write_car(&[ca], [(&ca, a.as_slice()), (&ca, a.as_slice())]);
        assert_eq!(read_car(&car, &CarLimits::PROOF).unwrap().blocks.len(), 1);
        // identity-hash CID cannot be verified
        let mh = multihash::Multihash::<64>::wrap(0x00, b"x").unwrap();
        let id = Cid::new_v1(0x55, mh);
        let car = write_car(&[ca], [(&id, &b"x"[..])]);
        assert!(matches!(
            read_car(&car, &CarLimits::PROOF),
            Err(CarError::Cid { .. })
        ));
    }
}
