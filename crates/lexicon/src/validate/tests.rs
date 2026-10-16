// SPDX-License-Identifier: MIT OR Apache-2.0
//! Validator unit tests: one valid and one invalid case (at least) per type, constraint and
//! format, over a purpose-built test lexicon.

use serde_json::{Value, json};

use super::*;
use crate::schemas::Registry;

const CID: &str = "bafkreiccldh766hwcnuxnf2wh6jgzepf2nlu2lvcllt63eww5p6chi4ity";
const DAG_CID: &str = "bafyreie5737gdxlw5i64vzichcalba3z2v5n6icifvx5xytvske7mr3hpm";

const TEST_LEXICON: &str = r##"{
    "lexicon": 1,
    "id": "com.example.test",
    "defs": {
        "main": {
            "type": "record",
            "key": "tid",
            "record": {
                "type": "object",
                "required": ["s"],
                "nullable": ["n", "rn"],
                "properties": {
                    "s": {"type": "string", "minLength": 1, "maxLength": 5},
                    "g": {"type": "string", "minGraphemes": 1, "maxGraphemes": 2},
                    "kv": {"type": "string", "knownValues": ["a", "b"], "maxLength": 64},
                    "en": {"type": "string", "enum": ["x", "y"]},
                    "c": {"type": "string", "const": "k"},
                    "d": {"type": "string", "default": "zz", "maxLength": 2},
                    "i": {"type": "integer", "minimum": 0, "maximum": 10, "default": 3},
                    "ie": {"type": "integer", "enum": [1, 2]},
                    "ic": {"type": "integer", "const": 3},
                    "b": {"type": "boolean", "default": true},
                    "bc": {"type": "boolean", "const": true},
                    "by": {"type": "bytes", "minLength": 1, "maxLength": 3},
                    "l": {"type": "cid-link"},
                    "blob": {"type": "blob", "accept": ["image/*", "text/plain"], "maxSize": 100},
                    "anyblob": {"type": "blob"},
                    "arr": {"type": "array", "items": {"type": "integer"}, "minLength": 1, "maxLength": 2},
                    "obj": {"type": "ref", "ref": "#obj"},
                    "u": {"type": "union", "refs": ["#obj", "com.example.other"]},
                    "uc": {"type": "union", "refs": ["#obj"], "closed": true},
                    "unk": {"type": "unknown"},
                    "n": {"type": "string"},
                    "rn": {"type": "integer"},
                    "x": {"type": "ref", "ref": "com.example.other#thing"},
                    "nul": {"type": "null"},
                    "node": {"type": "ref", "ref": "#node"},
                    "loop": {"type": "ref", "ref": "#loop1"},
                    "tok": {"type": "ref", "ref": "#tok"},
                    "f_atid": {"type": "string", "format": "at-identifier"},
                    "f_aturi": {"type": "string", "format": "at-uri"},
                    "f_cid": {"type": "string", "format": "cid"},
                    "f_dt": {"type": "string", "format": "datetime"},
                    "f_did": {"type": "string", "format": "did"},
                    "f_handle": {"type": "string", "format": "handle"},
                    "f_nsid": {"type": "string", "format": "nsid"},
                    "f_tid": {"type": "string", "format": "tid"},
                    "f_rkey": {"type": "string", "format": "record-key"},
                    "f_uri": {"type": "string", "format": "uri"},
                    "f_lang": {"type": "string", "format": "language"}
                }
            }
        },
        "obj": {
            "type": "object",
            "required": ["a"],
            "properties": {"a": {"type": "integer", "maximum": 5}}
        },
        "node": {
            "type": "object",
            "properties": {"next": {"type": "ref", "ref": "#node"}}
        },
        "loop1": {"type": "ref", "ref": "#loop2"},
        "loop2": {"type": "ref", "ref": "#loop1"},
        "tok": {"type": "token"}
    }
}"##;

fn test_registry() -> Registry {
    let mut r = Registry::new();
    r.add_json(TEST_LEXICON).unwrap();
    r.add_json(
        &json!({
            "lexicon": 1,
            "id": "com.example.other",
            "defs": {
                "main": {"type": "object", "properties": {"z": {"type": "string", "maxLength": 1}}},
                "thing": {"type": "object", "required": ["t"], "properties": {"t": {"type": "boolean"}}}
            }
        })
        .to_string(),
    )
    .unwrap();
    assert!(r.unresolved_refs().is_empty(), "{:?}", r.unresolved_refs());
    r
}

/// Validates `{"$type": "com.example.test", "s": "ok", <field>: v}` and returns the report.
fn field(name: &str, v: Value) -> Report {
    let reg = test_registry();
    let mut rec = json!({"$type": "com.example.test", "s": "ok"});
    rec[name] = v;
    Validator::new(&reg, Options::default()).record("com.example.test", None, &rec)
}

#[track_caller]
fn ok(name: &str, v: Value) {
    let r = field(name, v.clone());
    assert!(r.is_valid(), "{name} = {v}: {:?}", r.violations);
}

#[track_caller]
fn bad(name: &str, v: Value, code: Code) {
    let r = field(name, v.clone());
    assert!(
        r.has(code),
        "{name} = {v}: expected {code}, got {:?}",
        r.violations
    );
}

#[test]
fn record_type_and_key() {
    let reg = test_registry();
    let val = Validator::new(&reg, Options::default());
    let good = json!({"$type": "com.example.test", "s": "ok"});
    assert!(val.record("com.example.test", None, &good).is_valid());
    assert!(
        val.record("com.example.test", Some("3l2s5xxv2ze2c"), &good)
            .is_valid()
    );
    assert!(
        val.record("com.example.test", Some("self"), &good)
            .has(Code::RecordKey)
    );
    assert!(
        val.record("com.example.test", Some("a/b"), &good)
            .has(Code::RecordKey)
    );
    for (v, code) in [
        (json!({"s": "ok"}), Code::RecordType),
        (
            json!({"$type": "com.example.other", "s": "ok"}),
            Code::RecordType,
        ),
        (json!({"$type": 1, "s": "ok"}), Code::RecordType),
        (json!({"$type": "com.example.test"}), Code::Required),
        (json!([1]), Code::Type),
        (json!("x"), Code::Type),
    ] {
        assert!(val.record("com.example.test", None, &v).has(code), "{v}");
    }
    assert!(
        val.record("com.example.nope", None, &good)
            .has(Code::Schema)
    );
    // unknown fields are allowed and not inspected
    let extra = json!({"$type": "com.example.test", "s": "ok", "zz": {"deep": [1.5, null]}});
    assert!(val.record("com.example.test", None, &extra).is_valid());
}

#[test]
fn required_and_nullable() {
    ok("n", Value::Null);
    ok("rn", Value::Null);
    bad("s", Value::Null, Code::Type);
    bad("kv", Value::Null, Code::Type);
    let r = field("s", json!("ok"));
    assert!(r.is_valid());
    let reg = test_registry();
    let r = Validator::new(&reg, Options::default()).record(
        "com.example.test",
        None,
        &json!({"$type": "com.example.test"}),
    );
    assert_eq!(r.violations.len(), 1);
    assert_eq!(r.violations[0].path, "$.s");
    assert_eq!(r.violations[0].code, Code::Required);
}

#[test]
fn strings() {
    ok("s", json!("abcde"));
    bad("s", json!("abcdef"), Code::Length);
    bad("s", json!(""), Code::Length);
    // maxLength counts UTF-8 bytes: "é" is 2 bytes, so "ééé" is 6
    bad("s", json!("ééé"), Code::Length);
    ok("s", json!("éé"));
    bad("s", json!(5), Code::Type);
    // graphemes: an emoji ZWJ sequence and a combining sequence are one grapheme each
    ok("g", json!("👨‍👩‍👧e\u{301}"));
    bad("g", json!("abc"), Code::Graphemes);
    bad("g", json!(""), Code::Graphemes);
    // knownValues are open
    ok("kv", json!("a"));
    ok("kv", json!("zzz"));
    // enum is closed
    ok("en", json!("x"));
    bad("en", json!("z"), Code::Enum);
    // const
    ok("c", json!("k"));
    bad("c", json!("j"), Code::Const);
    // default is informational; the value still has to satisfy the type
    ok("d", json!("ab"));
    bad("d", json!("abc"), Code::Length);
}

#[test]
fn integers() {
    ok("i", json!(0));
    ok("i", json!(10));
    // sync.md §5.5: any number with a fraction or exponent is a float, even an integral one
    bad("i", json!(7.0), Code::Type);
    let parsed = |s: &str| serde_json::from_str::<Value>(s).unwrap();
    bad("i", parsed("1e0"), Code::Type);
    bad("i", parsed("3.0"), Code::Type);
    ok("i", parsed("3"));
    // and integers stay within ±(2^53 − 1)
    ok("rn", json!(9_007_199_254_740_991_i64));
    ok("rn", json!(-9_007_199_254_740_991_i64));
    bad("rn", json!(9_007_199_254_740_992_i64), Code::Type);
    bad("rn", json!(-9_007_199_254_740_992_i64), Code::Type);
    bad("rn", json!(i64::MIN), Code::Type);
    bad("i", json!(11), Code::Range);
    bad("i", json!(-1), Code::Range);
    bad("i", json!(1.5), Code::Type);
    bad("i", json!("1"), Code::Type);
    bad("i", json!(u64::MAX), Code::Type);
    ok("ie", json!(2));
    bad("ie", json!(3), Code::Enum);
    ok("ic", json!(3));
    bad("ic", json!(4), Code::Const);
}

#[test]
fn booleans_and_null() {
    ok("b", json!(false));
    bad("b", json!(0), Code::Type);
    bad("b", json!("true"), Code::Type);
    ok("bc", json!(true));
    bad("bc", json!(false), Code::Const);
    ok("nul", Value::Null);
    bad("nul", json!(0), Code::Type);
}

#[test]
fn bytes() {
    ok("by", json!({"$bytes": "AAE"}));
    ok("by", json!({"$bytes": "AAEC"}));
    bad("by", json!({"$bytes": ""}), Code::Length);
    bad("by", json!({"$bytes": "AAECAw"}), Code::Length);
    bad("by", json!({"$bytes": "!!"}), Code::Bytes);
    bad("by", json!({"$bytes": 1}), Code::Bytes);
    bad("by", json!({"$bytes": "AAE", "x": 1}), Code::Bytes);
    bad("by", json!("AAE"), Code::Bytes);
}

#[test]
fn cid_links() {
    ok("l", json!({"$link": DAG_CID}));
    ok("l", json!({"$link": CID}));
    bad("l", json!({"$link": "bafy"}), Code::Link);
    bad("l", json!({"$link": DAG_CID, "x": 1}), Code::Link);
    bad("l", json!({"$link": 1}), Code::Link);
    bad("l", json!(DAG_CID), Code::Link);
    // only blessed CIDs (v1, sha2-256, dag-cbor or raw): here a dag-pb CID
    let dag_pb = lessdugin_atp::cid::from_sha256_digest(0x70, [7; 32]).to_string();
    bad("l", json!({ "$link": dag_pb }), Code::Link);
}

#[test]
fn blobs() {
    let blob = |mime: &str, size: i64| json!({"$type": "blob", "ref": {"$link": CID}, "mimeType": mime, "size": size});
    ok("blob", blob("image/png", 100));
    ok("blob", blob("IMAGE/JPEG", 1));
    ok("blob", blob("text/plain; charset=utf-8", 1));
    bad("blob", blob("text/markdown", 1), Code::BlobAccept);
    bad("blob", blob("video/mp4", 1), Code::BlobAccept);
    bad("blob", blob("image/png", 101), Code::BlobSize);
    bad("blob", blob("", 1), Code::Blob);
    bad("blob", blob("image/png", -1), Code::Blob);
    // the declared mimeType is a type/subtype MIME type (RFC 6838 names, no wildcard)
    ok("blob", blob("text/plain ; charset=utf-8", 1));
    ok("blob", blob("image/svg+xml", 1));
    for not_mime in [
        " image/png",
        "image/png ",
        "image/png/x",
        "image/*",
        "*/*",
        "image",
        "image/",
        "/png",
        "image/p ng",
        "imagé/png",
    ] {
        bad("blob", blob(not_mime, 1), Code::Blob);
    }
    // the ref is a raw-codec CID (data model: "CID reference to blob, with codec type raw")
    bad(
        "blob",
        json!({"$type": "blob", "ref": {"$link": DAG_CID}, "mimeType": "image/png", "size": 1}),
        Code::Blob,
    );
    bad(
        "blob",
        json!({"$type": "blob", "ref": {"$link": CID}, "mimeType": "image/png", "size": 9_007_199_254_740_992_u64}),
        Code::Blob,
    );
    bad(
        "blob",
        json!({"$type": "blob", "ref": {"$link": "x"}, "mimeType": "image/png", "size": 1}),
        Code::Blob,
    );
    bad(
        "blob",
        json!({"$type": "blob", "mimeType": "image/png", "size": 1}),
        Code::Blob,
    );
    bad(
        "blob",
        json!({"$type": "blob", "ref": {"$link": CID}, "mimeType": "image/png", "size": "1"}),
        Code::Blob,
    );
    bad(
        "blob",
        json!({"$type": "notblob", "ref": {"$link": CID}, "mimeType": "image/png", "size": 1}),
        Code::Blob,
    );
    bad("blob", json!({"mimeType": "image/png"}), Code::Blob);
    bad("blob", json!("blob"), Code::Type);
    ok("anyblob", blob("application/x-anything", 1 << 40));
    // legacy form: accepted by default (Lexicon reading rules), rejected when disallowed
    let legacy = json!({"cid": CID, "mimeType": "image/png"});
    ok("blob", legacy.clone());
    bad(
        "blob",
        json!({"cid": CID, "mimeType": "video/mp4"}),
        Code::BlobAccept,
    );
    let reg = test_registry();
    let opts = Options {
        allow_legacy_blobs: false,
        ..Options::default()
    };
    let rec = json!({"$type": "com.example.test", "s": "ok", "blob": legacy});
    let r = Validator::new(&reg, opts).record("com.example.test", None, &rec);
    assert!(r.has(Code::LegacyBlob), "{:?}", r.violations);
}

#[test]
fn accept_globs() {
    assert!(accept_matches("*/*", "anything/at-all"));
    assert!(accept_matches("image/*", "image/webp"));
    assert!(accept_matches("image/*", "Image/WebP; q=1"));
    assert!(!accept_matches("image/*", "imagex/png"));
    assert!(!accept_matches("image/*", "image/"));
    assert!(!accept_matches("*/*", "nonsense"));
    assert!(accept_matches(
        "text/markdown",
        "text/markdown;charset=utf-8"
    ));
    assert!(!accept_matches("text/markdown", "text/plain"));
    assert_eq!(mime_essence(" Text/Plain ; charset=x"), "text/plain");
}

#[test]
fn arrays() {
    ok("arr", json!([1]));
    ok("arr", json!([1, 2]));
    bad("arr", json!([]), Code::Length);
    bad("arr", json!([1, 2, 3]), Code::Length);
    bad("arr", json!(["a"]), Code::Type);
    bad("arr", json!({"0": 1}), Code::Type);
    let r = field("arr", json!([1, "x"]));
    assert_eq!(r.violations[0].path, "$.arr[1]");
}

#[test]
fn objects_and_refs() {
    ok("obj", json!({"a": 5}));
    ok("obj", json!({"a": 5, "$type": "whatever", "extra": true}));
    bad("obj", json!({"a": 6}), Code::Range);
    bad("obj", json!({}), Code::Required);
    bad("obj", json!([1]), Code::Type);
    // cross-document reference
    ok("x", json!({"t": true}));
    bad("x", json!({"t": 1}), Code::Type);
    bad("x", json!({}), Code::Required);
    let r = field("x", json!({"t": "no"}));
    assert_eq!(r.violations[0].path, "$.x.t");
    // tokens
    ok("tok", json!("com.example.test#tok"));
    bad("tok", json!("other"), Code::Type);
}

#[test]
fn unions() {
    ok("u", json!({"$type": "com.example.test#obj", "a": 1}));
    ok("u", json!({"$type": "com.example.other", "z": "a"}));
    ok("u", json!({"$type": "com.example.other#main", "z": "a"}));
    bad(
        "u",
        json!({"$type": "com.example.test#obj", "a": 9}),
        Code::Range,
    );
    bad(
        "u",
        json!({"$type": "com.example.other", "z": "ab"}),
        Code::Length,
    );
    // open: an unknown member is left alone
    ok(
        "u",
        json!({"$type": "com.example.unknown", "a": "anything"}),
    );
    // but it must be an object with a string $type
    bad("u", json!({"a": 1}), Code::Union);
    bad("u", json!({"$type": 3}), Code::Union);
    bad("u", json!({"$type": ""}), Code::Union);
    bad("u", json!("com.example.other"), Code::Type);
    // closed unions reject unknown members
    ok("uc", json!({"$type": "com.example.test#obj", "a": 1}));
    bad("uc", json!({"$type": "com.example.unknown"}), Code::Union);
}

#[test]
fn unknown_type() {
    ok("unk", json!({}));
    ok(
        "unk",
        json!({"$type": "x.y.z", "anything": [1, {"deep": true}]}),
    );
    bad("unk", json!([]), Code::Type);
    bad("unk", json!("s"), Code::Type);
    bad("unk", json!(1), Code::Type);
    // `/specs/lexicon`: not the structure of a compound data type ($link, $bytes, blob)
    bad("unk", json!({ "$link": DAG_CID }), Code::Type);
    bad("unk", json!({"$bytes": "AAE"}), Code::Type);
    bad("unk", json!({"$link": DAG_CID, "x": 1}), Code::Type);
    bad(
        "unk",
        json!({"$type": "blob", "ref": {"$link": CID}, "mimeType": "image/png", "size": 1}),
        Code::Type,
    );
    // nested ones are fine: only the top-level data must be an object
    ok(
        "unk",
        json!({"$type": "x.y.z", "l": {"$link": DAG_CID}, "b": {"$bytes": "AAE"}}),
    );
}

#[test]
fn union_members_are_objects_or_records() {
    // a member in another document that is not an object or record is a schema error, so a
    // union step always consumes a value level (no loop through refs)
    let mut reg = Registry::new();
    reg.add_json(
        &json!({"lexicon": 1, "id": "com.example.u", "defs": {"main": {"type": "object",
            "properties": {"u": {"type": "union", "refs": ["com.example.v#alias", "com.example.v#s", "com.example.v#o", "com.example.v#gone"]}}}}})
        .to_string(),
    )
    .unwrap();
    reg.add_json(
        &json!({"lexicon": 1, "id": "com.example.v", "defs": {
            "alias": {"type": "ref", "ref": "com.example.u"},
            "s": {"type": "string"},
            "o": {"type": "object", "properties": {"u": {"type": "ref", "ref": "com.example.u"}}}}})
        .to_string(),
    )
    .unwrap();
    let v = Validator::new(&reg, Options::default());
    for member in ["alias", "s", "gone"] {
        let val = json!({"u": {"$type": format!("com.example.v#{member}")}});
        let r = v.def("com.example.u", &val);
        assert!(r.has(Code::Schema), "{member}: {:?}", r.violations);
        assert_eq!(v.with_defaults("com.example.u", &val), val);
    }
    // an object member is validated, and recursion through it is bounded by the depth
    let mut deep = json!({});
    for _ in 0..100 {
        deep = json!({"u": {"$type": "com.example.v#o", "u": deep}});
    }
    let r = v.def("com.example.u", &deep);
    assert!(r.has(Code::Depth), "{:?}", r.violations);
    let _ = v.with_defaults("com.example.u", &deep);
}

#[test]
fn formats() {
    let cases: &[(&str, &str, &str)] = &[
        ("f_atid", "did:plc:ewvi7nxzyoun6zhxrhs64oiz", "@alice"),
        ("f_atid", "alice.example.com", "did:x"),
        (
            "f_aturi",
            "at://alice.example.com/app.bsky.feed.post/3l2s5xxv2ze2c",
            "https://x.com",
        ),
        ("f_cid", CID, "Qmabcdefghijk"),
        ("f_dt", "2024-01-02T03:04:05.678Z", "2024-01-02 03:04:05Z"),
        ("f_did", "did:web:example.com", "did:Web:x"),
        ("f_handle", "alice.example.com", "alice"),
        (
            "f_nsid",
            "online.7778777.lessdugin.post",
            "7778777.online.post",
        ),
        ("f_tid", "3l2s5xxv2ze2c", "3l2s5xxv2ze2"),
        ("f_rkey", "uetjixxr3a33-~-x", ".."),
        ("f_uri", "https://example.com/a?b#c", "example.com"),
        ("f_lang", "pt-BR", "JA"),
    ];
    for (f, good, badv) in cases {
        ok(f, json!(good));
        bad(f, json!(badv), Code::Format);
    }
}

#[test]
fn paths_are_json_paths() {
    let reg = test_registry();
    let rec = json!({
        "$type": "com.example.test", "s": "ok",
        "u": {"$type": "com.example.test#obj", "a": "x"},
    });
    let r = Validator::new(&reg, Options::default()).record("com.example.test", None, &rec);
    assert_eq!(r.violations[0].path, "$.u.a");
    assert_eq!(
        render_path(&[
            Seg::Key("a b"),
            Seg::Index(2),
            Seg::Key("$type"),
            Seg::Key("9x")
        ]),
        "$[\"a b\"][2].$type[\"9x\"]"
    );
}

#[test]
fn depth_is_bounded() {
    let reg = test_registry();
    let val = Validator::new(&reg, Options::default());
    // a chain of `node` objects: the record is depth 1, each node one more
    let chain = |n: usize| {
        let mut v = json!({});
        for _ in 1..n {
            v = json!({"next": v});
        }
        json!({"$type": "com.example.test", "s": "ok", "node": v})
    };
    assert!(val.record("com.example.test", None, &chain(31)).is_valid());
    let r = val.record("com.example.test", None, &chain(32));
    assert!(r.has(Code::Depth), "{:?}", r.violations);
    assert_eq!(r.violations.len(), 1);
    // unknown fields count too
    let mut deep = json!(1);
    for _ in 0..40 {
        deep = json!([deep]);
    }
    let rec = json!({"$type": "com.example.test", "s": "ok", "zz": deep});
    assert!(val.record("com.example.test", None, &rec).has(Code::Depth));
    // $link and $bytes objects are leaves
    let mut v = json!({"$link": DAG_CID});
    for _ in 0..31 {
        v = json!([v]);
    }
    assert_eq!(nesting_depth(&v, 64).0, 31);
    assert_eq!(nesting_depth(&json!({}), 64).0, 1);
    assert_eq!(nesting_depth(&json!(1), 64).0, 0);
    let (d, at) = nesting_depth(&json!({"a": [1, {"b": [[]]}]}), 3);
    assert_eq!(d, 4);
    assert_eq!(at.as_deref(), Some("$.a[1].b"));
}

#[test]
fn ref_cycles_are_cut() {
    let reg = test_registry();
    let r = Validator::new(&reg, Options::default()).def("com.example.test#loop1", &json!({}));
    assert!(r.has(Code::Schema), "{:?}", r.violations);
    assert!(field("loop", json!({})).has(Code::Schema));
}

#[test]
fn budgets() {
    let reg = test_registry();
    let opts = Options {
        max_steps: 3,
        ..Options::default()
    };
    let rec = json!({"$type": "com.example.test", "s": "ok", "arr": [1, 2], "i": 1, "b": true});
    let r = Validator::new(&reg, opts).record("com.example.test", None, &rec);
    assert!(r.truncated);
    assert!(r.has(Code::Budget));
    let opts = Options {
        max_violations: 2,
        ..Options::default()
    };
    let rec = json!({"$type": "com.example.test", "s": 1, "g": 1, "kv": 1, "en": 1});
    let r = Validator::new(&reg, opts).record("com.example.test", None, &rec);
    assert_eq!(r.violations.len(), 2);
    assert!(r.truncated);
    assert!(!r.is_valid());
}

#[test]
fn defaults_are_filled_for_reading() {
    let reg = test_registry();
    let v = Validator::new(&reg, Options::default());
    let rec = json!({"$type": "com.example.test", "s": "ok", "i": 7});
    let filled = v.with_defaults("com.example.test", &rec);
    assert_eq!(filled["d"], "zz");
    assert_eq!(filled["b"], true);
    assert_eq!(filled["i"], 7, "present values are kept");
    assert!(filled.get("kv").is_none(), "no default, nothing filled");
    assert!(v.record("com.example.test", None, &filled).is_valid());
    // through the real lexicons: forum.membership and nested policy defaults, union members
    let ns = crate::ns::NS;
    let forum = json!({
        "$type": format!("{ns}.forum"), "name": "x", "createdAt": "2024-01-01T00:00:00.000Z",
        "policy": {"karma": {}}, "about": {"$type": format!("{ns}.markdown"), "text": "hi"}
    });
    let f = Validator::new(crate::schemas::registry(), Options::default())
        .with_defaults(&format!("{ns}.forum"), &forum);
    assert_eq!(f["membership"], "approval");
    assert_eq!(f["defaultTier"], "personal");
    assert_eq!(f["policy"]["voting"], "reacts");
    assert_eq!(f["policy"]["authorModeration"], true);
    assert_eq!(f["policy"]["karma"]["pairCap"], 25);
    assert!(
        f["policy"].get("ranking").is_none(),
        "absent objects are not created"
    );
    assert_eq!(f["about"]["profile"], "lfm/1");
    // a non-object value is returned unchanged
    assert_eq!(
        Validator::new(&reg, Options::default()).with_defaults("com.example.test", &json!(3)),
        json!(3)
    );
}

#[test]
fn output_schemas() {
    let reg = crate::schemas::registry();
    let ns = crate::ns::NS;
    let v = Validator::new(reg, Options::default());
    let out = json!({
        "heads": [],
        "asOf": {"seq": 1, "timeUs": 2, "lagMs": 3},
        "indexer": {"did": "did:web:index.example.com", "coreVersion": "0.1.0"}
    });
    let r = v.output(&format!("{ns}.index.getRepoHeads"), &out);
    assert!(r.is_valid(), "{:?}", r.violations);
    let r = v.output(&format!("{ns}.index.getRepoHeads"), &json!({"heads": []}));
    assert!(r.has(Code::Required));
    assert!(
        v.output(&format!("{ns}.index.getRecordProofs"), &out)
            .has(Code::Schema)
    );
    assert!(v.output(&format!("{ns}.post"), &out).has(Code::Schema));
}

#[test]
fn scalar_problems_for_defaults() {
    let t = TypeDef::Integer(IntegerDef {
        minimum: Some(1),
        ..IntegerDef::default()
    });
    assert!(scalar_problem(&t, &json!(1)).is_none());
    assert!(scalar_problem(&t, &json!(0)).is_some());
    assert_eq!(as_integer(&json!(-3)), Ok(-3));
    assert!(as_integer(&json!(9_007_199_254_740_992.0_f64)).is_err());
    assert!(as_integer(&json!(3.0)).is_err());
    assert!(as_integer(&json!(9_007_199_254_740_992_u64)).is_err());
    assert_eq!(
        as_integer(&json!(9_007_199_254_740_991_u64)),
        Ok(9_007_199_254_740_991)
    );
    // the whole-value scan used by validity rule G2
    assert_eq!(first_non_integer(&json!({"a": [1, {"b": 2}]})), None);
    let (path, _) = first_non_integer(&json!({"a": [1, {"b": 2.0}], "c": 1.5})).unwrap();
    assert_eq!(path, "$.a[1].b");
    let (path, _) = first_non_integer(&json!([{"x": 9_007_199_254_740_992_u64}])).unwrap();
    assert_eq!(path, "$[0].x");
}
