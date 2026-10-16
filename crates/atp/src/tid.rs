// SPDX-License-Identifier: MIT OR Apache-2.0
//! Timestamp identifiers (TIDs, `/specs/tid`): parse, format, and generate from an injected clock.
//!
//! A TID is a 64-bit integer (top bit 0; 53 bits of microseconds since the Unix epoch; 10 bits of
//! clock id) written as 13 characters of base32-sortable (`234567abcdefghijklmnopqrstuvwxyz`).
//! String order equals integer order.

use crate::syntax::{SyntaxError, string_type_impls};

const ALPHABET: &[u8; 32] = b"234567abcdefghijklmnopqrstuvwxyz";
const KIND: &str = "tid";
/// Length of a TID string.
pub const TID_LEN: usize = 13;
/// Largest timestamp (microseconds) a TID can hold: 2^53 − 1.
pub const MAX_TIMESTAMP_MICROS: u64 = (1 << 53) - 1;
/// Largest clock id: 2^10 − 1.
pub const MAX_CLOCK_ID: u16 = 1023;

/// A syntactically valid TID.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tid([u8; TID_LEN]);

impl Tid {
    /// Parses a TID: 13 base32-sortable characters, the first in `[2-7a-j]` (so the value fits in
    /// 64 bits).
    pub fn parse(s: &str) -> Result<Self, SyntaxError> {
        let b = s.as_bytes();
        if b.len() != TID_LEN {
            return Err(SyntaxError::new(KIND, "must be 13 characters"));
        }
        if !b.iter().all(|c| ALPHABET.contains(c)) {
            return Err(SyntaxError::new(KIND, "invalid base32-sortable character"));
        }
        if !matches!(b[0], b'2'..=b'7' | b'a'..=b'j') {
            return Err(SyntaxError::new(KIND, "value does not fit in 64 bits"));
        }
        let mut out = [0u8; TID_LEN];
        out.copy_from_slice(b);
        Ok(Self(out))
    }

    /// Builds a TID from its 64-bit integer value. Every `u64` has a syntactically valid TID
    /// (the first character carries the top 4 bits, hence `[2-7a-j]`); well-formed TIDs keep the
    /// top bit 0, which [`Tid::from_parts`] guarantees.
    pub fn from_u64(v: u64) -> Self {
        let mut out = [0u8; TID_LEN];
        for (i, slot) in out.iter_mut().enumerate() {
            let shift = 5 * (TID_LEN - 1 - i);
            *slot = ALPHABET[((v >> shift) & 31) as usize];
        }
        Self(out)
    }

    /// Builds a TID from a timestamp (µs since the Unix epoch, < 2^53) and a clock id (< 1024).
    pub fn from_parts(timestamp_micros: u64, clock_id: u16) -> Result<Self, SyntaxError> {
        if timestamp_micros > MAX_TIMESTAMP_MICROS {
            return Err(SyntaxError::new(KIND, "timestamp out of range"));
        }
        if clock_id > MAX_CLOCK_ID {
            return Err(SyntaxError::new(KIND, "clock id out of range"));
        }
        Ok(Self::from_u64(
            (timestamp_micros << 10) | u64::from(clock_id),
        ))
    }

    /// The TID string.
    pub fn as_str(&self) -> &str {
        // Always ASCII from ALPHABET.
        core::str::from_utf8(&self.0).unwrap_or_default()
    }

    /// The 64-bit integer value.
    pub fn to_u64(&self) -> u64 {
        self.0.iter().fold(0u64, |acc, c| {
            let d = ALPHABET.iter().position(|a| a == c).unwrap_or(0) as u64;
            (acc << 5) | d
        })
    }

    /// Microseconds since the Unix epoch (`value >> 10`).
    pub fn timestamp_micros(&self) -> u64 {
        self.to_u64() >> 10
    }

    /// The clock id (low 10 bits).
    pub fn clock_id(&self) -> u16 {
        (self.to_u64() & 0x3ff) as u16
    }
}

impl core::fmt::Debug for Tid {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Tid({})", self.as_str())
    }
}

string_type_impls!(Tid);

/// A strictly monotonic TID generator driven by a caller-supplied clock.
///
/// Each [`TidGenerator::next`] returns a TID greater than every TID previously returned or
/// [`observed`](TidGenerator::observe), even if the clock stalls or goes backwards: in that case
/// the timestamp advances by one microsecond past the last value (keeping the clock id).
#[derive(Debug, Clone)]
pub struct TidGenerator {
    clock_id: u16,
    last: u64,
}

impl TidGenerator {
    /// Creates a generator with a clock id (only the low 10 bits are used; callers pick it
    /// randomly once per process/device).
    pub fn new(clock_id: u16) -> Self {
        Self {
            clock_id: clock_id & MAX_CLOCK_ID,
            last: 0,
        }
    }

    /// The clock id in use.
    pub fn clock_id(&self) -> u16 {
        self.clock_id
    }

    /// Ensures future TIDs sort after `tid` (for example the repo's latest `rev`).
    pub fn observe(&mut self, tid: &Tid) {
        self.last = self.last.max(tid.to_u64());
    }

    /// Returns the next TID for the given wall-clock time (µs since the Unix epoch; negative
    /// values count as 0). Fails only when the timestamp space (2^53 µs, year 2255) is exhausted.
    pub fn next(&mut self, now_micros: i64) -> Result<Tid, SyntaxError> {
        let now = u64::try_from(now_micros)
            .unwrap_or(0)
            .min(MAX_TIMESTAMP_MICROS);
        let mut v = (now << 10) | u64::from(self.clock_id);
        if v <= self.last {
            let ts = (self.last >> 10)
                .checked_add(1)
                .filter(|t| *t <= MAX_TIMESTAMP_MICROS)
                .ok_or(SyntaxError::new(KIND, "timestamp space exhausted"))?;
            v = (ts << 10) | u64::from(self.clock_id);
        }
        self.last = v;
        Ok(Tid::from_u64(v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        assert_eq!(Tid::from_u64(0).as_str(), "2222222222222");
        let t = Tid::parse("3jzfcijpj2z2a").unwrap();
        assert_eq!(Tid::from_u64(t.to_u64()), t);
        assert_eq!(
            Tid::from_parts(t.timestamp_micros(), t.clock_id()).unwrap(),
            t
        );
        assert_eq!(Tid::from_u64(i64::MAX as u64).as_str(), "bzzzzzzzzzzzz");
        let max = Tid::from_u64(u64::MAX);
        assert_eq!(max.as_str(), "jzzzzzzzzzzzz");
        assert_eq!(Tid::parse("jzzzzzzzzzzzz").unwrap().to_u64(), u64::MAX);
        assert!(Tid::parse("kzzzzzzzzzzzz").is_err());
        assert!(Tid::from_parts(MAX_TIMESTAMP_MICROS + 1, 0).is_err());
        assert!(Tid::from_parts(0, 1024).is_err());
        assert_eq!(format!("{t:?}"), "Tid(3jzfcijpj2z2a)");
    }

    #[test]
    fn order_matches_value() {
        let a = Tid::from_parts(1_700_000_000_000_000, 5).unwrap();
        let b = Tid::from_parts(1_700_000_000_000_001, 0).unwrap();
        assert!(a < b);
        assert!(a.as_str() < b.as_str());
        assert!(a.to_u64() < b.to_u64());
    }

    #[test]
    fn generator_monotonic() {
        let mut g = TidGenerator::new(7);
        let t1 = g.next(1_700_000_000_000_000).unwrap();
        let t2 = g.next(1_700_000_000_000_000).unwrap();
        let t3 = g.next(1_600_000_000_000_000).unwrap(); // clock went backwards
        let t4 = g.next(1_800_000_000_000_000).unwrap();
        assert!(t1 < t2 && t2 < t3 && t3 < t4);
        assert_eq!(t1.clock_id(), 7);
        assert_eq!(t3.clock_id(), 7);
        assert_eq!(t4.timestamp_micros(), 1_800_000_000_000_000);
        let future = Tid::from_parts(1_900_000_000_000_000, 1).unwrap();
        g.observe(&future);
        assert!(g.next(1_800_000_000_000_001).unwrap() > future);
        assert_eq!(
            g.next(-5).unwrap().timestamp_micros(),
            1_900_000_000_000_002
        );
        let mut end = TidGenerator::new(0);
        end.observe(&Tid::from_parts(MAX_TIMESTAMP_MICROS, 0).unwrap());
        assert!(end.next(0).is_err());
    }
}
