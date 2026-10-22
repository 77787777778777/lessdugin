// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests of admission (`sync.md` §5.5).

use lessdugin_atp::datamodel;
use lessdugin_atp::syntax::{AtUri, Did};
use lessdugin_lexicon::ns::NS;
use serde_json::{Value, json};

use super::*;
use crate::store::records::{Level, OrderKey, Source};

const DID: &str = "did:plc:ewvi7nxzyoun6zhxrhs64oiz";
const FK: &str = "uetjixxr3a33";

fn post() -> String {
    format!("{NS}.post")
}

fn rkey() -> String {
    format!("{FK}-3l2s5xxv2ze2c")
}

fn uri() -> String {
    format!("at://{DID}/{}/{}", post(), rkey())
}

fn did() -> Did {
    Did::parse(DID).unwrap()
}

fn cid_of(v: &Value) -> String {
    datamodel::record_cid(v).unwrap().to_string()
}

fn value() -> Value {
    json!({"$type": post(), "title": "hello", "n": 3, "l": [{"$link": "bafyreidfayvfuwqa7qlnopdjiqrxzs6blmoeu4rujcjtnci5beludirz2a"}]})
}

fn get_body(uri: &str, cid: &str, value_text: &str) -> Vec<u8> {
    format!(r#"{{"uri":"{uri}","cid":"{cid}","value":{value_text}}}"#).into_bytes()
}

fn get(body: &[u8]) -> Result<AdmittedRecord, AdmitError> {
    let did = did();
    let p = post();
    let k = rkey();
    let expect = Expect {
        did: &did,
        collection: &p,
        rkey: Some(&k),
    };
    admit_get_record(body, &expect, &NoValidator)
}

#[test]
fn get_record_happy_path() {
    let v = value();
    let text = serde_json::to_string(&v).unwrap();
    let r = get(&get_body(&uri(), &cid_of(&v), &text)).unwrap();
    assert_eq!(r.uri.as_str(), uri());
    assert_eq!(r.cid.to_string(), cid_of(&v));
    assert_eq!(lessdugin_atp::cid::for_dag_cbor(&r.cbor), r.cid);
    assert_eq!(r.value, v);
    assert!(r.validity.is_valid());
    // the same CID in another multibase (base32 upper) compares equal in binary
    let upper = format!("B{}", cid_of(&v)[1..].to_uppercase());
    assert!(get(&get_body(&uri(), &upper, &text)).is_ok());
}

#[test]
fn cid_checks() {
    let v = value();
    let text = serde_json::to_string(&v).unwrap();
    let other = cid_of(&json!({"$type": post(), "n": 4}));
    assert!(matches!(
        get(&get_body(&uri(), &other, &text)),
        Err(AdmitError::CidMismatch { .. })
    ));
    // a raw-codec CID, a CIDv0 and garbage are not record CIDs
    let raw = lessdugin_atp::cid::for_raw(b"x").to_string();
    for bad in [
        raw.as_str(),
        "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG",
        "not-a-cid",
    ] {
        assert!(
            matches!(get(&get_body(&uri(), bad, &text)), Err(AdmitError::Cid(_))),
            "{bad}"
        );
    }
    // a missing cid
    let body = format!(r#"{{"uri":"{}","value":{text}}}"#, uri());
    assert!(matches!(get(body.as_bytes()), Err(AdmitError::Cid(_))));
}

#[test]
fn shape_checks() {
    let v = value();
    let text = serde_json::to_string(&v).unwrap();
    let cid = cid_of(&v);
    let bad_uris = [
        // another repo
        format!(
            "at://did:plc:abcdefghijklmnopqrstuvwx/{}/{}",
            post(),
            rkey()
        ),
        // another collection
        format!("at://{DID}/{NS}.comment/{}", rkey()),
        // another rkey
        format!("at://{DID}/{}/{FK}-3l2s5xxv2ze3c", post()),
        // not canonical
        format!("at://alice.example/{}/{}", post(), rkey()),
        format!("at://{DID}/{}/{}/", post(), rkey()),
    ];
    for u in bad_uris {
        assert!(
            matches!(get(&get_body(&u, &cid, &text)), Err(AdmitError::Shape(_))),
            "{u}"
        );
    }
    // not an object
    assert!(matches!(
        get(&get_body(&uri(), &cid, "[1]")),
        Err(AdmitError::Shape(_))
    ));
    // too large in its response encoding (whitespace inside the value keeps the CID; whitespace
    // around it is not part of the value)
    let padded = format!("{{{}{}", " ".repeat(1_000_000), &text[1..]);
    assert!(matches!(
        get(&get_body(&uri(), &cid, &padded)),
        Err(AdmitError::Shape(_))
    ));
    let around = format!("{}{}", " ".repeat(1_000_000), text);
    assert!(get(&get_body(&uri(), &cid, &around)).is_ok());
    let just = format!("{{{}{}", " ".repeat(1_000_000 - text.len()), &text[1..]);
    assert_eq!(just.len(), 1_000_000);
    assert!(get(&get_body(&uri(), &cid, &just)).is_ok());
}

#[test]
fn strict_json_drops_the_record() {
    let v = json!({"$type": post(), "n": 1});
    let cid = cid_of(&v);
    for text in [
        format!(r#"{{"$type":"{}","n":1.0}}"#, post()),
        format!(r#"{{"$type":"{}","n":1e0}}"#, post()),
        format!(r#"{{"$type":"{}","n":9007199254740992}}"#, post()),
        format!(r#"{{"$type":"{}","n":0,"n":1}}"#, post()),
    ] {
        assert!(
            matches!(
                get(&get_body(&uri(), &cid, &text)),
                Err(AdmitError::Json(_))
            ),
            "{text}"
        );
    }
    // a duplicate in the envelope too
    let body = format!(
        r#"{{"uri":"{}","cid":"{cid}","cid":"{cid}","value":{{"$type":"{}","n":1}}}}"#,
        uri(),
        post()
    );
    assert!(matches!(get(body.as_bytes()), Err(AdmitError::Json(_))));
}

#[test]
fn malformed_or_too_deep_responses_are_rejected_whole() {
    let e = get(b"{\"uri\":").unwrap_err();
    assert!(e.rejects_response());
    // the value at depth 36 makes the document 37 deep
    let deep = format!("{}{}", "[".repeat(35), "]".repeat(35));
    let text = format!(r#"{{"$type":"{}","d":{deep}}}"#, post());
    let e = get(&get_body(&uri(), "bafy", &text)).unwrap_err();
    assert!(matches!(e, AdmitError::Malformed(_)), "{e}");
}

#[test]
fn deep_values_are_admitted_invalid_g10() {
    // a value of depth 34 (1 object + 33 lists): converted, CID-checked, invalid by G10
    let mut d = json!(1);
    for _ in 0..33 {
        d = json!([d]);
    }
    let v = json!({"$type": post(), "d": d});
    let r = get(&get_body(
        &uri(),
        &cid_of_with_depth(&v),
        &serde_json::to_string(&v).unwrap(),
    ))
    .unwrap();
    assert_eq!(r.validity.rule.as_deref(), Some("G10"));
    assert!(!r.validity.is_valid());
    assert_eq!(
        datamodel::nesting_depth(&datamodel::json_to_ipld_with(&v, &RECORD_LIMITS).unwrap()),
        34
    );
}

fn cid_of_with_depth(v: &Value) -> String {
    datamodel::record_cid_with(v, &RECORD_LIMITS)
        .unwrap()
        .to_string()
}

#[test]
fn oversized_cbor_is_invalid_g6() {
    let v = json!({"$type": post(), "s": "x".repeat(1_000_100)});
    let cbor = datamodel::record_to_cbor_with(&v, &RECORD_LIMITS).unwrap();
    let cid = lessdugin_atp::cid::for_dag_cbor(&cbor);
    let u = AtUri::parse(&uri()).unwrap();
    let r = admit_block(&u, &cid, &cbor, &NoValidator).unwrap();
    assert_eq!(r.validity.rule.as_deref(), Some("G6"));
    assert_eq!(r.value, v);
    // a block that does not hash to the CID is refused
    let other = lessdugin_atp::cid::for_dag_cbor(b"\xa0");
    assert!(matches!(
        admit_block(&u, &other, &cbor, &NoValidator),
        Err(AdmitError::CidMismatch { .. })
    ));
    // so is non-canonical CBOR
    let noncanon = [0xb9, 0x00, 0x00]; // empty map, 2-byte length
    let c = lessdugin_atp::cid::for_dag_cbor(&noncanon);
    assert!(matches!(
        admit_block(&u, &c, &noncanon, &NoValidator),
        Err(AdmitError::DataModel(_))
    ));
}

#[test]
fn lexicon_validity_is_recorded_not_dropped() {
    let did = did();
    let p = post();
    let k = rkey();
    let expect = Expect {
        did: &did,
        collection: &p,
        rkey: Some(&k),
    };
    // $type of another collection: G1
    let v = json!({"$type": format!("{NS}.comment"), "n": 1});
    let body = get_body(&uri(), &cid_of(&v), &serde_json::to_string(&v).unwrap());
    let r = admit_get_record(&body, &expect, &LexiconValidator).unwrap();
    assert_eq!(r.validity.rule.as_deref(), Some("G1"));
    // a post without its required fields: G2
    let v = json!({"$type": post()});
    let body = get_body(&uri(), &cid_of(&v), &serde_json::to_string(&v).unwrap());
    let r = admit_get_record(&body, &expect, &LexiconValidator).unwrap();
    assert_eq!(r.validity.rule.as_deref(), Some("G2"), "{:?}", r.validity);
    assert!(!r.validity.violations.is_empty());
    // a well-formed post is valid under every rule
    let forum = format!("at://{DID}/{NS}.forum/rationality");
    let fk = lessdugin_lexicon::keys::fk(&forum).unwrap();
    let good_uri = format!("at://{DID}/{}/{fk}-3l2s5xxv2ze2c", post());
    let good = json!({"$type": post(), "forum": forum, "title": "Hello",
                      "createdAt": "2024-01-01T00:00:00.000Z"});
    let k2 = format!("{fk}-3l2s5xxv2ze2c");
    let expect2 = Expect {
        did: &did,
        collection: &p,
        rkey: Some(&k2),
    };
    let body = get_body(
        &good_uri,
        &cid_of(&good),
        &serde_json::to_string(&good).unwrap(),
    );
    let r = admit_get_record(&body, &expect2, &LexiconValidator).unwrap();
    assert!(r.validity.is_valid(), "{:?}", r.validity);
    // the same post under a key of another forum: stored, invalid by its key rules
    let body = get_body(
        &uri(),
        &cid_of(&good),
        &serde_json::to_string(&good).unwrap(),
    );
    let r = admit_get_record(&body, &expect, &LexiconValidator).unwrap();
    assert!(!r.validity.is_valid());
    assert!(
        r.validity.violations.iter().any(|v| v.starts_with("PO1")),
        "{:?}",
        r.validity
    );
    // a collection outside the namespace is only checked for $type
    let bsky = AtUri::parse(&format!("at://{DID}/app.bsky.actor.profile/self")).unwrap();
    assert!(
        LexiconValidator
            .violations(
                &bsky,
                &json!({"$type": "app.bsky.actor.profile", "anything": 1})
            )
            .is_empty()
    );
    assert_eq!(
        LexiconValidator.violations(&bsky, &json!({"$type": "x"}))[0].rule,
        "G1"
    );
}

#[test]
fn validity_orders_rules() {
    let v = Validity::from_violations(vec![
        RuleViolation::new("PO3", "no title"),
        RuleViolation::new("G10", "deep"),
        RuleViolation::new("G2", "x".repeat(1_000)),
        RuleViolation::new("CO1", "key"),
        RuleViolation::new("X9", "an unknown rule sorts last"),
    ]);
    assert_eq!(v.rule.as_deref(), Some("G2"));
    // spec order: G1…G10, then the collections' rules in table order (post before comment)
    assert_eq!(v.violations[1], "G10: deep");
    assert_eq!(v.violations[2], "PO3: no title");
    assert_eq!(v.violations[3], "CO1: key");
    assert!(v.violations[4].starts_with("X9"));
    assert!(v.violations[0].len() <= MAX_VIOLATION_BYTES);
    let many = Validity::from_violations(
        (0..40)
            .map(|i| RuleViolation::new("G2", format!("{i}")))
            .collect(),
    );
    assert_eq!(many.violations.len(), MAX_STORED_VIOLATIONS);
    assert!(Validity::from_violations(Vec::new()).is_valid());
}

#[test]
fn list_page_entries_are_admitted_one_by_one() {
    let did = did();
    let p = post();
    let expect = Expect {
        did: &did,
        collection: &p,
        rkey: None,
    };
    let u = |k: &str| format!("at://{DID}/{}/{FK}-{k}", post());
    let good = json!({"$type": post(), "n": 1});
    let good_text = serde_json::to_string(&good).unwrap();
    let cursor = format!("{FK}-a");
    let body = format!(
        r#"{{"cursor":"{c}","records":[
            {{"uri":"{u0}","cid":"{c0}","value":{good_text}}},
            {{"uri":"{u1}","cid":"{c0}","value":{{"$type":"{p}","n":1.5}}}},
            {{"uri":"{u2}","cid":"{c0}","value":{good_text}}},
            {{"uri":"at://{DID}/{NS}.comment/{FK}-x","cid":"{c0}","value":{good_text}}},
            {{"uri":"{u4}","cid":"{c0}","value":{good_text},"value":{good_text}}},
            {{"uri":"at://alice.example/{p}/x","cid":"{c0}","value":{good_text}}},
            7
        ]}}"#,
        c = cursor,
        u0 = u("e"),
        u1 = u("d"),
        u2 = u("c"),
        u4 = u("a"),
        c0 = cid_of(&good),
        p = post(),
    );
    let page = admit_list_page(body.as_bytes(), &expect, &NoValidator).unwrap();
    assert_eq!(page.cursor.as_deref(), Some(format!("{FK}-a").as_str()));
    assert_eq!(page.entries.len(), 7);
    let e = &page.entries;
    assert!(e[0].result.is_ok());
    assert_eq!(e[0].rkey(), Some(format!("{FK}-e").as_str()));
    assert!(matches!(e[1].result, Err(AdmitError::Json(_))));
    assert_eq!(
        e[1].rkey(),
        Some(format!("{FK}-d").as_str()),
        "a dropped entry still has its key"
    );
    // same value, other uri: the CID does not depend on the uri
    assert!(e[2].result.is_ok());
    assert!(matches!(e[3].result, Err(AdmitError::Shape(_))));
    assert!(matches!(e[4].result, Err(AdmitError::Json(_))));
    assert!(e[5].uri.is_none());
    assert!(e[5].raw_uri.is_some());
    assert!(matches!(e[6].result, Err(AdmitError::Shape(_))));
    assert!(e[6].raw_uri.is_none());
}

#[test]
fn list_page_envelope_errors_reject_the_page() {
    let did = did();
    let p = post();
    let expect = Expect {
        did: &did,
        collection: &p,
        rkey: None,
    };
    for body in [
        r#"{"records":[],"records":[]}"#,
        r#"{"records":{}}"#,
        r#"{"cursor":5,"records":[]}"#,
        r#"{"cursor":"x"}"#,
        r#"[]"#,
        r#"{"records":[}"#,
    ] {
        assert!(
            admit_list_page(body.as_bytes(), &expect, &NoValidator)
                .unwrap_err()
                .rejects_response(),
            "{body}"
        );
    }
    let empty = admit_list_page(br#"{"records":[],"cursor":null}"#, &expect, &NoValidator).unwrap();
    assert!(empty.entries.is_empty() && empty.cursor.is_none());
    // unknown envelope fields are ignored, even odd ones
    assert!(admit_list_page(br#"{"records":[],"x":1.5}"#, &expect, &NoValidator).is_ok());
}

/// A page with more records than its request's `limit` is rejected whole before any record is
/// converted; within the limit, issues and value sizes are attributed per record (in one pass
/// over the scan, so a page the size of the 4 MiB cap costs linear time).
#[test]
fn list_pages_over_the_limit_are_rejected_and_attribution_is_per_record() {
    let did = did();
    let p = post();
    let expect = Expect {
        did: &did,
        collection: &p,
        rkey: None,
    };
    let good = json!({"$type": post(), "n": 1});
    let good_text = serde_json::to_string(&good).unwrap();
    let c0 = cid_of(&good);
    let n = 3_000usize;
    let mut recs = Vec::with_capacity(n);
    for i in (0..n).rev() {
        let value = if i % 1_000 == 7 {
            format!(r#"{{"$type":"{p}","n":1.5}}"#)
        } else {
            good_text.clone()
        };
        recs.push(format!(
            r#"{{"uri":"at://{DID}/{p}/{FK}-k{i:06}","cid":"{c0}","value":{value}}}"#
        ));
    }
    let body = format!(r#"{{"records":[{}]}}"#, recs.join(","));
    let err = admit_list_page_capped(body.as_bytes(), &expect, &NoValidator, 100).unwrap_err();
    assert!(err.rejects_response(), "{err}");
    let page = admit_list_page_capped(body.as_bytes(), &expect, &NoValidator, n).unwrap();
    assert_eq!(page.entries.len(), n);
    let bad: Vec<usize> = page
        .entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.result.is_err())
        .map(|(i, _)| n - 1 - i)
        .collect();
    assert_eq!(bad, [2_007, 1_007, 7]);
    assert!(
        page.entries
            .iter()
            .filter_map(|e| e.result.as_ref().ok())
            .all(|r| r.cid.to_string() == c0)
    );
}

#[test]
fn own_writes_and_observations() {
    let u = AtUri::parse(&uri()).unwrap();
    let v = value();
    let r = admit_own_write(&u, &v, &cid_of(&v), &NoValidator).unwrap();
    assert_eq!(r.cid.to_string(), cid_of(&v));
    let other = cid_of(&json!({"$type": post()}));
    assert!(matches!(
        admit_own_write(&u, &v, &other, &NoValidator),
        Err(AdmitError::CidMismatch { .. })
    ));
    let ver = r
        .clone()
        .into_version(Observation::own_write("3l2a", 10, 12).with_first_seen(5));
    assert_eq!(ver.key, OrderKey::new("3l2a", 10));
    assert!(ver.exact);
    assert_eq!(ver.src, Source::Own);
    assert_eq!((ver.first_seen_us, ver.fetched_at_ms), (5, 12));
    let p = Observation::proof("3l2b", 3, Source::Indexer(DID.into()), 4);
    assert_eq!(
        (p.level, p.proven_rev.as_deref(), p.exact),
        (Level::Proven, Some("3l2b"), true)
    );
    let g = Observation::point_fetch("3l2a", 3, 4).with_id_hint(true);
    assert!(!g.exact && g.id_hint && g.src == Source::Get);
    assert_eq!(g.first_seen_us, 4_000);
    let l = Observation::listing("3l2a", 3, false, 4);
    assert!(!l.exact && l.src == Source::List && l.level == Level::Pds);
}
