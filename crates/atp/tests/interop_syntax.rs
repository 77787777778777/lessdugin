// SPDX-License-Identifier: MIT OR Apache-2.0
//! atproto interop syntax fixtures (`testdata/interop/syntax`).

mod common;

// Natively these are libtest tests; on wasm32 (`cargo xtask test-wasm`) the same bodies run
// under wasm-bindgen-test in node.
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

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
    // The canonical form accepts exactly the valid URIs with a DID authority that passes the
    // lessdugin DID policy (did:plc / host-only did:web) and a full record path (and a lowercase
    // collection authority). The fixtures' DIDs are generic (`did:plc:asdf123`, `did:abc:123`),
    // so each is also checked with its DID replaced by a canonical one.
    const CANONICAL: &str = "did:plc:ewvi7nxzyoun6zhxrhs64oiz";
    let mut strict = 0;
    for v in common::lines("syntax/aturi_syntax_valid.txt") {
        let loose = LooseAtUri::parse(&v).unwrap();
        let did = match loose.authority() {
            AtIdentifier::Did(d) => Some(d.clone()),
            AtIdentifier::Handle(_) => None,
        };
        let full = did.is_some() && loose.rkey().is_some();
        let policy = did
            .as_ref()
            .is_some_and(|d| d.check_lessdugin_policy().is_ok());
        assert_eq!(AtUri::parse(&v).is_ok(), full && policy, "{v}");
        if let Some(d) = did {
            let swapped = v.replacen(d.as_str(), CANONICAL, 1);
            assert_eq!(AtUri::parse(&swapped).is_ok(), full, "{swapped}");
            if full {
                let l = LooseAtUri::parse(&swapped).unwrap();
                assert_eq!(l.to_strict().unwrap().as_str(), swapped);
                assert!(loose.to_strict().is_none() || policy);
                strict += 1;
            }
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
