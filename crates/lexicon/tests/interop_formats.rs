// SPDX-License-Identifier: MIT OR Apache-2.0
//! The atproto interop syntax fixtures (`testdata/interop/syntax`) through the validator's
//! string formats, both directly and through a full validation of a string field.

mod common;

// Natively these are libtest tests; on wasm32 (`cargo xtask test-wasm`) the same bodies run
// under wasm-bindgen-test in node.
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

use common::interop_lines;
use lessdugin_lexicon::schemas::{Registry, StringFormat};
use lessdugin_lexicon::validate::formats::{check_format, is_valid_language};
use lessdugin_lexicon::validate::{Code, Options, Validator};
use serde_json::{Value, json};

const CASES: &[(&str, StringFormat)] = &[
    ("atidentifier", StringFormat::AtIdentifier),
    ("aturi", StringFormat::AtUri),
    ("cid", StringFormat::Cid),
    ("datetime", StringFormat::Datetime),
    ("did", StringFormat::Did),
    ("handle", StringFormat::Handle),
    ("nsid", StringFormat::Nsid),
    ("recordkey", StringFormat::RecordKey),
    ("tid", StringFormat::Tid),
    ("uri", StringFormat::Uri),
    ("language", StringFormat::Language),
];

fn registry() -> Registry {
    let mut props = serde_json::Map::new();
    for (_, f) in CASES {
        props.insert(
            f.as_str().into(),
            json!({"type": "string", "format": f.as_str()}),
        );
    }
    let doc = json!({
        "lexicon": 1,
        "id": "com.example.formats",
        "defs": {"main": {"type": "object", "properties": Value::Object(props)}}
    });
    let mut r = Registry::new();
    r.add_json(&doc.to_string()).unwrap();
    r
}

/// The JSON path of a top-level key: `$.key`, or `$["key"]` when not identifier-like.
fn key_path(k: &str) -> String {
    if k.bytes().all(|b| b.is_ascii_alphanumeric()) {
        format!("$.{k}")
    } else {
        format!("$[\"{k}\"]")
    }
}

#[test]
fn syntax_fixtures() {
    let reg = registry();
    let v = Validator::new(&reg, Options::default());
    let mut total = 0;
    for (name, f) in CASES {
        let valid = interop_lines(&format!("syntax/{name}_syntax_valid.txt"));
        let invalid = interop_lines(&format!("syntax/{name}_syntax_invalid.txt"));
        assert!(!valid.is_empty() && !invalid.is_empty(), "{name}");
        for s in &valid {
            assert!(check_format(*f, s).is_ok(), "{name} should be valid: {s:?}");
            let r = v.def("com.example.formats", &json!({ f.as_str(): s }));
            assert!(r.is_valid(), "{name}: {s:?}: {:?}", r.violations);
        }
        for s in &invalid {
            assert!(
                check_format(*f, s).is_err(),
                "{name} should be invalid: {s:?}"
            );
            let r = v.def("com.example.formats", &json!({ f.as_str(): s }));
            assert!(r.has(Code::Format), "{name}: {s:?}");
            assert_eq!(r.violations[0].path, key_path(f.as_str()));
        }
        total += valid.len() + invalid.len();
    }
    assert!(total > 300, "{total}");
}

#[test]
fn datetime_semantics() {
    for s in interop_lines("syntax/datetime_parse_invalid.txt") {
        assert!(check_format(StringFormat::Datetime, &s).is_err(), "{s:?}");
    }
}

#[test]
fn language_validity() {
    // well-formed but not valid (repeated variant or singleton): the format accepts them (it is a
    // syntax check), the stricter validity check rejects them
    for s in interop_lines("syntax/language_parse_invalid.txt") {
        assert!(check_format(StringFormat::Language, &s).is_ok(), "{s:?}");
        assert!(!is_valid_language(&s), "{s:?}");
    }
    for s in interop_lines("syntax/language_syntax_valid.txt") {
        assert!(is_valid_language(&s), "{s:?}");
    }
}

#[test]
fn lessdugin_nsids_and_keys() {
    // ADR 0003: the digit NSID, `.temp` and index ids, and our rkey and cursor shapes
    for n in [
        "online.7778777.lessdugin.post",
        "online.7778777.lessdugin.temp.post",
        "online.7778777.lessdugin.index.listPosts",
    ] {
        assert!(check_format(StringFormat::Nsid, n).is_ok(), "{n}");
    }
    assert!(check_format(StringFormat::Nsid, "7778777.online.post").is_err());
    for k in [
        "aaaaaaaaaaaa-3lzzzzzzzzz2a",
        "aaaaaaaaaaaa.",
        "aaaaaaaaaaaa-~-x",
        "~",
        "3l.",
    ] {
        assert!(check_format(StringFormat::RecordKey, k).is_ok(), "{k}");
    }
}
