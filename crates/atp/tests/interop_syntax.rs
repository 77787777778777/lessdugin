// SPDX-License-Identifier: MIT OR Apache-2.0
//! atproto interop syntax fixtures (`testdata/interop/syntax`).

mod common;

use lessdugin_atp::cid::is_valid_cid_syntax;
use lessdugin_atp::syntax::{
    AtIdentifier, AtUri, Datetime, Did, Handle, LooseAtUri, Nsid, RecordKey, Tid,
};

fn check<T, E>(name: &str, parse: impl Fn(&str) -> Result<T, E>) -> usize {
    let valid = common::lines(&format!("syntax/{name}_syntax_valid.txt"));
    let invalid = common::lines(&format!("syntax/{name}_syntax_invalid.txt"));
    assert!(
        !valid.is_empty() && !invalid.is_empty(),
        "{name}: empty fixture lists"
    );
    for v in &valid {
        assert!(parse(v).is_ok(), "{name}: should be valid: {v:?}");
    }
    for v in &invalid {
        assert!(parse(v).is_err(), "{name}: should be invalid: {v:?}");
    }
    valid.len() + invalid.len()
}

#[test]
fn handle() {
    check("handle", Handle::parse);
}

#[test]
fn handle_normalizes_lowercase() {
    for v in common::lines("syntax/handle_syntax_valid.txt") {
        let h = Handle::parse(&v).unwrap();
        assert_eq!(h.as_str(), v.to_ascii_lowercase());
    }
}

#[test]
fn did() {
    check("did", Did::parse);
}

#[test]
fn nsid() {
    check("nsid", Nsid::parse);
}

#[test]
fn aturi_lexicon_syntax() {
    check("aturi", LooseAtUri::parse);
}

#[test]
fn aturi_strict_subset() {
    // The strict form accepts exactly the valid URIs with a DID authority and a full record path
    // (and a lowercase collection authority).
    let mut strict = 0;
    for v in common::lines("syntax/aturi_syntax_valid.txt") {
        let loose = LooseAtUri::parse(&v).unwrap();
        let full = matches!(loose.authority(), AtIdentifier::Did(_)) && loose.rkey().is_some();
        assert_eq!(AtUri::parse(&v).is_ok(), full, "{v}");
        if full {
            assert_eq!(loose.to_strict().unwrap().as_str(), v);
            strict += 1;
        }
    }
    assert!(strict > 5);
    for v in common::lines("syntax/aturi_syntax_invalid.txt") {
        assert!(AtUri::parse(&v).is_err(), "{v:?}");
    }
}

#[test]
fn recordkey() {
    check("recordkey", RecordKey::parse);
}

#[test]
fn tid() {
    check("tid", Tid::parse);
    for v in common::lines("syntax/tid_syntax_valid.txt") {
        let t = Tid::parse(&v).unwrap();
        assert_eq!(Tid::from_u64(t.to_u64()).as_str(), v);
    }
}

#[test]
fn datetime() {
    check("datetime", Datetime::parse);
    for v in common::lines("syntax/datetime_parse_invalid.txt") {
        assert!(Datetime::parse(&v).is_err(), "semantically invalid: {v:?}");
    }
    // the canonical writer round-trips every valid instant at millisecond precision
    for v in common::lines("syntax/datetime_syntax_valid.txt") {
        let d = Datetime::parse(&v).unwrap();
        let c = d.to_canonical();
        assert!(c.is_canonical());
        assert_eq!(Datetime::parse(c.as_str()).unwrap(), c);
        assert_eq!(
            c.timestamp_micros(),
            d.timestamp_micros().div_euclid(1000) * 1000
        );
    }
}

#[test]
fn atidentifier() {
    check("atidentifier", AtIdentifier::parse);
}

#[test]
fn cid_syntax() {
    check("cid", |s| {
        if is_valid_cid_syntax(s) {
            Ok(())
        } else {
            Err(())
        }
    });
}
