// SPDX-License-Identifier: MIT OR Apache-2.0
//! atproto datetimes (`/specs/lexicon`, `datetime` format).

use core::cmp::Ordering;

use super::{SyntaxError, string_type_impls};

const KIND: &str = "datetime";
const MAX_LEN: usize = 64;
const MAX_FRACTION_DIGITS: usize = 20;
const MICROS_PER_SEC: i64 = 1_000_000;
const MICROS_PER_DAY: i64 = 86_400 * MICROS_PER_SEC;

/// A valid atproto datetime, kept exactly as written (no normalization).
///
/// Syntax is the RFC 3339 / ISO 8601 / WHATWG intersection: `YYYY-MM-DDTHH:MM:SS[.f{1,20}]`
/// followed by `Z` or `±HH:MM` (but not `-00:00`), at most 64 characters. Semantics are checked
/// (real calendar dates, `HH ≤ 23`, `MM ≤ 59`, `SS ≤ 59`) and the instant must not be before
/// `0000-01-01T00:00:00Z`.
///
/// Ordering is by instant ([`Datetime::timestamp_micros`]), then by the string.
/// [`Datetime::from_micros`] is the canonical writer (`YYYY-MM-DDTHH:MM:SS.sssZ`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Datetime {
    s: String,
    micros: i64,
}

impl Datetime {
    /// Parses and validates a datetime.
    pub fn parse(s: &str) -> Result<Self, SyntaxError> {
        let micros = parse_micros(s)?;
        Ok(Self {
            s: s.to_owned(),
            micros,
        })
    }

    /// The datetime string as written.
    pub fn as_str(&self) -> &str {
        &self.s
    }

    /// Microseconds since the Unix epoch (UTC), truncating any sub-microsecond digits.
    /// Negative for instants before 1970.
    pub fn timestamp_micros(&self) -> i64 {
        self.micros
    }

    /// Canonical writer: formats `micros` (since the Unix epoch) as RFC 3339 UTC with millisecond
    /// precision and a `Z` suffix, flooring to the millisecond. Fails outside years 0000..=9999.
    pub fn from_micros(micros: i64) -> Result<Self, SyntaxError> {
        let ms = micros.div_euclid(1000);
        let days = ms.div_euclid(86_400_000);
        let rem = ms.rem_euclid(86_400_000);
        let (y, m, d) = civil_from_days(days);
        if !(0..=9999).contains(&y) {
            return Err(SyntaxError::new(KIND, "year out of range 0000-9999"));
        }
        let (h, rem) = (rem / 3_600_000, rem % 3_600_000);
        let (mi, rem) = (rem / 60_000, rem % 60_000);
        let (sec, milli) = (rem / 1000, rem % 1000);
        let s = format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{sec:02}.{milli:03}Z");
        Ok(Self {
            s,
            micros: ms * 1000,
        })
    }

    /// Milliseconds since the Unix epoch, a convenience for [`Datetime::from_micros`].
    pub fn from_millis(ms: i64) -> Result<Self, SyntaxError> {
        Self::from_micros(ms.saturating_mul(1000))
    }

    /// This instant re-written in canonical form (UTC, millisecond precision, `Z`).
    ///
    /// The one instant range the writer cannot express is after `9999-12-31T23:59:59.999Z`
    /// (a year-9999 local time with a negative offset); such values are returned unchanged.
    pub fn to_canonical(&self) -> Self {
        Self::from_micros(self.micros).unwrap_or_else(|_| self.clone())
    }

    /// `true` if the string is already in canonical form.
    pub fn is_canonical(&self) -> bool {
        self.to_canonical().s == self.s
    }
}

impl PartialOrd for Datetime {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Datetime {
    fn cmp(&self, other: &Self) -> Ordering {
        self.micros
            .cmp(&other.micros)
            .then_with(|| self.s.cmp(&other.s))
    }
}

fn digits(b: &[u8], start: usize, len: usize) -> Option<i64> {
    let slice = b.get(start..start + len)?;
    let mut v = 0i64;
    for &c in slice {
        if !c.is_ascii_digit() {
            return None;
        }
        v = v * 10 + i64::from(c - b'0');
    }
    Some(v)
}

fn parse_micros(s: &str) -> Result<i64, SyntaxError> {
    let b = s.as_bytes();
    let bad = |reason| SyntaxError::new(KIND, reason);
    if b.len() > MAX_LEN {
        return Err(bad("too long"));
    }
    if b.len() < 20 {
        return Err(bad("too short"));
    }
    let syntax = || bad("expected YYYY-MM-DDTHH:MM:SS");
    let year = digits(b, 0, 4).ok_or_else(syntax)?;
    let month = digits(b, 5, 2).ok_or_else(syntax)?;
    let day = digits(b, 8, 2).ok_or_else(syntax)?;
    let hour = digits(b, 11, 2).ok_or_else(syntax)?;
    let minute = digits(b, 14, 2).ok_or_else(syntax)?;
    let second = digits(b, 17, 2).ok_or_else(syntax)?;
    if b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return Err(syntax());
    }
    let mut pos = 19;
    let mut frac_micros = 0i64;
    if b[pos] == b'.' {
        pos += 1;
        let start = pos;
        while pos < b.len() && b[pos].is_ascii_digit() {
            pos += 1;
        }
        let n = pos - start;
        if n == 0 || n > MAX_FRACTION_DIGITS {
            return Err(bad("fraction must have 1-20 digits"));
        }
        for i in 0..6 {
            let d = if i < n {
                i64::from(b[start + i] - b'0')
            } else {
                0
            };
            frac_micros = frac_micros * 10 + d;
        }
    }
    let tz = &b[pos..];
    let offset_secs = match tz {
        b"Z" => 0,
        [sign @ (b'+' | b'-'), _, _, b':', _, _] => {
            if tz == b"-00:00" {
                return Err(bad("-00:00 is not allowed"));
            }
            let oh = digits(tz, 1, 2).ok_or_else(|| bad("invalid timezone offset"))?;
            let om = digits(tz, 4, 2).ok_or_else(|| bad("invalid timezone offset"))?;
            if oh > 23 || om > 59 {
                return Err(bad("invalid timezone offset"));
            }
            let v = oh * 3600 + om * 60;
            if *sign == b'-' { -v } else { v }
        }
        _ => return Err(bad("timezone must be 'Z' or ±HH:MM")),
    };
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return Err(bad("invalid date"));
    }
    if hour > 23 || minute > 59 || second > 59 {
        return Err(bad("invalid time"));
    }
    let days = days_from_civil(year, month, day);
    let secs = hour * 3600 + minute * 60 + second - offset_secs;
    let micros = days * MICROS_PER_DAY + secs * MICROS_PER_SEC + frac_micros;
    if micros < days_from_civil(0, 1, 1) * MICROS_PER_DAY {
        return Err(bad("before year 0"));
    }
    Ok(micros)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(y) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date (H. Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`].
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

string_type_impls!(Datetime);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_and_offsets() {
        assert_eq!(
            Datetime::parse("1970-01-01T00:00:00Z")
                .unwrap()
                .timestamp_micros(),
            0
        );
        let a = Datetime::parse("1985-04-12T23:20:50.123Z").unwrap();
        assert_eq!(a.timestamp_micros(), 482_196_050_123_000);
        let b = Datetime::parse("1985-04-13T00:20:50.123+01:00").unwrap();
        assert_eq!(a.timestamp_micros(), b.timestamp_micros());
        assert!(a < b); // same instant, string tie-break
        let c = Datetime::parse("1985-04-12T23:20:50.1234567Z").unwrap();
        assert_eq!(c.timestamp_micros(), 482_196_050_123_456);
        assert_eq!(c.to_canonical().as_str(), "1985-04-12T23:20:50.123Z");
        assert!(a.is_canonical());
        assert!(!b.is_canonical());
    }

    #[test]
    fn writer() {
        assert_eq!(
            Datetime::from_micros(0).unwrap().as_str(),
            "1970-01-01T00:00:00.000Z"
        );
        assert_eq!(
            Datetime::from_micros(-1).unwrap().as_str(),
            "1969-12-31T23:59:59.999Z"
        );
        assert_eq!(
            Datetime::from_millis(951_782_400_000).unwrap().as_str(),
            "2000-02-29T00:00:00.000Z"
        );
        assert!(Datetime::from_micros(i64::MAX).is_err());
        assert!(Datetime::from_micros(i64::MIN).is_err());
        let min = Datetime::parse("0000-01-01T00:00:00.000Z").unwrap();
        assert_eq!(Datetime::from_micros(min.timestamp_micros()).unwrap(), min);
        let max = Datetime::parse("9999-12-31T23:59:59.999Z").unwrap();
        assert_eq!(Datetime::from_micros(max.timestamp_micros()).unwrap(), max);
    }

    #[test]
    fn calendar() {
        assert!(Datetime::parse("2000-02-29T00:00:00Z").is_ok());
        assert!(Datetime::parse("1900-02-29T00:00:00Z").is_err());
        assert!(Datetime::parse("2023-04-31T00:00:00Z").is_err());
        assert!(Datetime::parse("2023-04-30T24:00:00Z").is_err());
        assert!(Datetime::parse("2023-04-30T23:59:60Z").is_err());
        assert!(Datetime::parse("2023-04-30T23:59:59+24:00").is_err());
        for days in [-800_000i64, -1, 0, 1, 59, 60, 10_957, 2_932_896] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
    }
}
