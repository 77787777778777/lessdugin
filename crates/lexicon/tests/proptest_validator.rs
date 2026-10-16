// SPDX-License-Identifier: MIT OR Apache-2.0
//! Property tests: the validator, validity, record parsing and key derivation never panic on
//! arbitrary JSON or strings, and validation is deterministic.

mod common;

use common::{nsid, sample_record};
use lessdugin_atp::syntax::AtUri;
use lessdugin_lexicon::Collection;
use lessdugin_lexicon::keys;
use lessdugin_lexicon::ns::NS;
use lessdugin_lexicon::records::Record;
use lessdugin_lexicon::schemas::StringFormat;
use lessdugin_lexicon::validate::formats::{check_format, is_valid_language};
use lessdugin_lexicon::validate::{Options, Validator, validate_record};
use lessdugin_lexicon::validity::check;
use proptest::prelude::*;
use serde_json::{Map, Value, json};

/// Keys that hit schema properties and data-model special keys often.
fn key() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => prop::sample::select(vec![
            "$type", "$link", "$bytes", "text", "textBlob", "uri", "cid", "forum", "subject",
            "root", "parent", "body", "action", "scope", "title", "kind", "tags", "reacts",
            "react", "record", "op", "links", "media", "blob", "ref", "mimeType", "size",
            "chapters", "posts", "parents", "slug", "createdAt", "name", "policy", "karma",
            "head", "allowed", "level", "tier", "pinned", "hidden", "subjectCid", "adopts",
        ])
        .prop_map(str::to_owned),
        1 => "[a-zA-Z$]{1,6}",
    ]
}

/// Strings that are often meaningful: our NSIDs, at-uris, CIDs, dates, slugs.
fn text() -> impl Strategy<Value = String> {
    let post =
        format!("at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/{NS}.post/uetjixxr3a33-3l2s5xxv2ze2c");
    let forum = format!("at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/{NS}.forum/rationality");
    prop_oneof![
        3 => prop::sample::select(vec![
            nsid("markdown"), nsid("post"), nsid("forum"), nsid("modAction#hide"),
            nsid("modAction#pin"), nsid("modAction#tier"), "blob".into(), post, forum,
            "did:plc:abcdefghijklmnopqrstuvwx".into(), "tag:ai".into(),
            common::RAW_CID.into(), common::DAG_CID.into(), "2024-01-02T03:04:05.678Z".into(),
            "image/png".into(), "text/markdown".into(), "quickTake".into(), "link".into(),
            "author".into(), "forum".into(), "thread".into(), "delete".into(), "curated".into(),
        ]),
        2 => any::<String>(),
        1 => "[a-z0-9-]{0,12}",
    ]
}

fn leaf() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        (-1000i64..1000).prop_map(Value::from),
        any::<u64>().prop_map(Value::from),
        any::<f64>()
            .prop_filter("finite", |f| f.is_finite())
            .prop_map(|f| json!(f)),
        text().prop_map(Value::from),
    ]
}

fn arb_json() -> impl Strategy<Value = Value> {
    leaf().prop_recursive(8, 96, 8, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            prop::collection::vec((key(), inner), 0..8)
                .prop_map(|kv| Value::Object(kv.into_iter().collect::<Map<_, _>>())),
        ]
    })
}

fn collection() -> impl Strategy<Value = Collection> {
    prop::sample::select(Collection::ALL.to_vec())
}

fn rkey() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("self".to_owned()),
        Just("rationality".to_owned()),
        "[2-7a-z]{12}(-[2-7a-z]{13}){0,2}",
        "[2-7a-z]{12}-[2-7a-z]{16}",
        "[a-zA-Z0-9._:~-]{1,40}",
    ]
}

/// Replaces the value at a pseudo-random path of `v` by `x`.
fn mutate(v: &mut Value, mut choice: usize, x: Value) {
    let mut cur = v;
    loop {
        let len = match cur {
            Value::Object(m) => m.len(),
            Value::Array(a) => a.len(),
            _ => 0,
        };
        if len == 0 || choice.is_multiple_of(4) {
            *cur = x;
            return;
        }
        let i = (choice / 4) % len;
        choice /= 7;
        cur = match cur {
            Value::Object(m) => m.values_mut().nth(i).unwrap(),
            Value::Array(a) => &mut a[i],
            _ => unreachable!(),
        };
        if choice == 0 {
            *cur = x;
            return;
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    #[test]
    fn validator_and_validity_never_panic_on_arbitrary_json(
        c in collection(),
        v in arb_json(),
        k in rkey(),
    ) {
        let r = validate_record(c.nsid(), &v);
        prop_assert_eq!(&r, &validate_record(c.nsid(), &v));
        if let Ok(uri) = AtUri::parse(&format!("at://did:plc:abcdefghijklmnopqrstuvwx/{}/{k}", c.nsid())) {
            let a = check(c.nsid(), &uri, &v);
            prop_assert_eq!(&a, &check(c.nsid(), &uri, &v));
            prop_assert_eq!(a.valid, a.violations.is_empty());
        }
        let _ = Record::from_value(c, v);
    }

    #[test]
    fn mutated_samples_never_panic(
        c in collection(),
        choice in any::<usize>(),
        x in arb_json(),
        k in rkey(),
    ) {
        let mut v = sample_record(c.nsid());
        mutate(&mut v, choice, x);
        let opts = Options { allow_legacy_blobs: false, ..Options::default() };
        let _ = Validator::new(lessdugin_lexicon::schemas::registry(), opts).record(c.nsid(), Some(&k), &v);
        if let Ok(uri) = AtUri::parse(&format!("at://did:plc:abcdefghijklmnopqrstuvwx/{}/{k}", c.nsid())) {
            let a = check(c.nsid(), &uri, &v);
            prop_assert_eq!(a.valid, a.violations.is_empty());
        }
        if let Ok(rec) = Record::from_value(c, v.clone()) {
            // whatever parses serialises back
            prop_assert!(rec.to_value().is_ok());
        }
    }

    #[test]
    fn deep_values_never_panic(depth in 0usize..200, c in collection()) {
        let mut v = json!({"$type": c.nsid()});
        let mut inner = json!(1);
        for i in 0..depth {
            inner = if i.is_multiple_of(2) { json!([inner]) } else { json!({"body": inner}) };
        }
        v["body"] = inner;
        let uri = AtUri::parse(&format!("at://did:plc:abcdefghijklmnopqrstuvwx/{}/self", c.nsid())).unwrap();
        let a = check(c.nsid(), &uri, &v);
        prop_assert_eq!(a.fails(lessdugin_lexicon::RuleId::G10), depth + 1 > 32);
    }

    #[test]
    fn key_functions_never_panic(a in text(), b in text(), n in 0usize..64, bytes in prop::collection::vec(any::<u8>(), 0..40)) {
        let _ = keys::b32s(&bytes, n);
        let _ = keys::fk(&a);
        let _ = keys::h16(&a, &[&b]);
        let _ = keys::thread_tid(NS, &a);
        let _ = keys::stid(NS, &a);
        let _ = keys::revision_prefix(NS, &a);
        let _ = keys::post_rkey(&a, &b);
        let _ = keys::comment_rkey(NS, &a, &b, &a);
        let _ = keys::split_revision_key(&a);
        let _ = keys::fk_of(&a);
        for c in Collection::ALL {
            let _ = keys::matches_grammar(c, &a);
        }
        let action = json!({"$type": a, "scope": b});
        let _ = keys::action_key(NS, action.as_object().unwrap());
    }

    #[test]
    fn formats_never_panic(s in any::<String>(), t in "[a-zA-Z0-9-]{0,30}") {
        for f in [
            StringFormat::AtIdentifier, StringFormat::AtUri, StringFormat::Cid,
            StringFormat::Datetime, StringFormat::Did, StringFormat::Handle, StringFormat::Nsid,
            StringFormat::Tid, StringFormat::RecordKey, StringFormat::Uri, StringFormat::Language,
        ] {
            let _ = check_format(f, &s);
            let _ = check_format(f, &t);
        }
        let _ = is_valid_language(&t);
    }

    #[test]
    fn b32s_matches_a_reference(bytes in prop::collection::vec(any::<u8>(), 0..33)) {
        // reference: big-integer style extraction, one bit at a time
        let n = bytes.len() * 8 / 5;
        let mut bits = Vec::new();
        for b in &bytes {
            for i in (0..8).rev() {
                bits.push((b >> i) & 1);
            }
        }
        let want: String = bits
            .chunks(5)
            .take(n)
            .map(|c| keys::ALPHABET[c.iter().fold(0usize, |a, b| a * 2 + *b as usize)] as char)
            .collect();
        prop_assert_eq!(keys::b32s(&bytes, n).unwrap(), want);
        prop_assert!(keys::b32s(&bytes, n + 1).is_none() || (n + 1) * 5 <= bytes.len() * 8);
    }
}
