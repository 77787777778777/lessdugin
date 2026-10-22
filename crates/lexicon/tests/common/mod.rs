// SPDX-License-Identifier: MIT OR Apache-2.0
//! Shared helpers for the integration tests: fixture paths and a schema-driven generator of
//! maximal valid sample values (every property present).
//!
//! Most of these tests also run on wasm32 under node (`cargo xtask test-wasm`), where there is no
//! filesystem: there the fixtures are embedded at compile time (`EMBEDDED`), natively they are
//! read from the repository.
#![allow(dead_code)]

#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

use lessdugin_lexicon::ns::NS;
use lessdugin_lexicon::schemas::{Def, Registry, StringFormat, TypeDef, registry};
use serde_json::{Map, Value, json};

/// A blessed raw-codec CID (blob refs).
pub const RAW_CID: &str = "bafkreiccldh766hwcnuxnf2wh6jgzepf2nlu2lvcllt63eww5p6chi4ity";
/// A blessed dag-cbor CID (record references).
pub const DAG_CID: &str = "bafyreie5737gdxlw5i64vzichcalba3z2v5n6icifvx5xytvske7mr3hpm";

/// `<repo>/<rel>`.
#[cfg(not(target_arch = "wasm32"))]
pub fn repo_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The text of the repository file `<repo>/<rel>`.
#[cfg(not(target_arch = "wasm32"))]
pub fn text(rel: &str) -> String {
    let p = repo_path(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The text of the repository file `<repo>/<rel>` (the copy embedded at compile time).
#[cfg(target_arch = "wasm32")]
pub fn text(rel: &str) -> String {
    EMBEDDED
        .iter()
        .find(|(p, _)| *p == rel)
        .unwrap_or_else(|| panic!("{rel} is not embedded (crates/lexicon/tests/common/mod.rs)"))
        .1
        .to_owned()
}

/// Every fixture the tests read, embedded for wasm32 (a missing entry fails with a clear panic).
#[cfg(target_arch = "wasm32")]
macro_rules! embed {
    ($($rel:literal),* $(,)?) => {
        &[$(($rel, include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../", $rel)))),*]
    };
}

#[cfg(target_arch = "wasm32")]
static EMBEDDED: &[(&str, &str)] = embed![
    "testdata/interop/data-model/data-model-invalid.json",
    "testdata/interop/data-model/data-model-valid.json",
    "testdata/interop/syntax/atidentifier_syntax_invalid.txt",
    "testdata/interop/syntax/atidentifier_syntax_valid.txt",
    "testdata/interop/syntax/aturi_syntax_invalid.txt",
    "testdata/interop/syntax/aturi_syntax_valid.txt",
    "testdata/interop/syntax/cid_syntax_invalid.txt",
    "testdata/interop/syntax/cid_syntax_valid.txt",
    "testdata/interop/syntax/datetime_parse_invalid.txt",
    "testdata/interop/syntax/datetime_syntax_invalid.txt",
    "testdata/interop/syntax/datetime_syntax_valid.txt",
    "testdata/interop/syntax/did_syntax_invalid.txt",
    "testdata/interop/syntax/did_syntax_valid.txt",
    "testdata/interop/syntax/handle_syntax_invalid.txt",
    "testdata/interop/syntax/handle_syntax_valid.txt",
    "testdata/interop/syntax/language_parse_invalid.txt",
    "testdata/interop/syntax/language_syntax_invalid.txt",
    "testdata/interop/syntax/language_syntax_valid.txt",
    "testdata/interop/syntax/nsid_syntax_invalid.txt",
    "testdata/interop/syntax/nsid_syntax_valid.txt",
    "testdata/interop/syntax/recordkey_syntax_invalid.txt",
    "testdata/interop/syntax/recordkey_syntax_valid.txt",
    "testdata/interop/syntax/tid_syntax_invalid.txt",
    "testdata/interop/syntax/tid_syntax_valid.txt",
    "testdata/interop/syntax/uri_syntax_invalid.txt",
    "testdata/interop/syntax/uri_syntax_valid.txt",
    "testdata/vectors/keys/fk.json",
    "testdata/vectors/keys/rkeys.json",
    "testdata/vectors/keys/sk.json",
    "testdata/vectors/lfm1/quote.json",
    "testdata/vectors/limits/graphemes.json",
];

/// Test lines of an interop `.txt` list: skips blank lines and `#` comments, keeps whitespace.
pub fn interop_lines(rel: &str) -> Vec<String> {
    let text = text(&format!("testdata/interop/{rel}"));
    text.split('\n')
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

/// A build-namespace NSID: `nsid("post")` = `NS.post`.
pub fn nsid(suffix: &str) -> String {
    format!("{NS}.{suffix}")
}

fn string_sample(f: Option<StringFormat>) -> String {
    match f {
        None => "x".into(),
        Some(StringFormat::AtIdentifier) => "did:plc:ewvi7nxzyoun6zhxrhs64oiz".into(),
        Some(StringFormat::AtUri) => {
            format!("at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/{NS}.forum/rationality")
        }
        Some(StringFormat::Cid) => DAG_CID.into(),
        Some(StringFormat::Datetime) => "2024-01-02T03:04:05.678Z".into(),
        Some(StringFormat::Did) => "did:plc:abcdefghijklmnopqrstuvwx".into(),
        Some(StringFormat::Handle) => "alice.example.com".into(),
        Some(StringFormat::Nsid) => "com.example.thing".into(),
        Some(StringFormat::Tid) => "3l2s5xxv2ze2c".into(),
        Some(StringFormat::RecordKey) => "self".into(),
        Some(StringFormat::Uri) => "https://example.com/x".into(),
        Some(StringFormat::Language) => "en".into(),
    }
}

fn mime_for(accept: Option<&Vec<String>>) -> String {
    match accept.and_then(|a| a.first()).map(String::as_str) {
        None | Some("*/*") => "application/octet-stream".into(),
        Some(p) => match p.strip_suffix("/*") {
            Some("image") => "image/png".into(),
            Some("text") => "text/markdown".into(),
            Some(t) => format!("{t}/x-sample"),
            None => p.into(),
        },
    }
}

/// A maximal valid sample of a type: every object property present, one array item, the first
/// union member (with `$type`).
pub fn sample(reg: &Registry, t: &TypeDef, depth: usize) -> Value {
    assert!(depth < 24, "schema too deep for samples");
    match t {
        TypeDef::Null => Value::Null,
        TypeDef::Boolean(b) => json!(b.const_value.or(b.default).unwrap_or(true)),
        TypeDef::Integer(i) => {
            let v = i
                .const_value
                .or_else(|| i.enum_values.as_ref().and_then(|e| e.first().copied()))
                .or(i.default)
                .or(i.minimum)
                .or(i.maximum)
                .unwrap_or(1);
            json!(v)
        }
        TypeDef::String(s) => {
            let v = s
                .const_value
                .clone()
                .or_else(|| s.enum_values.as_ref().and_then(|e| e.first().cloned()))
                .or_else(|| s.default.clone())
                .or_else(|| s.known_values.first().cloned())
                .unwrap_or_else(|| string_sample(s.format));
            json!(v)
        }
        TypeDef::Bytes(_) => json!({"$bytes": "AAE"}),
        TypeDef::CidLink => json!({"$link": DAG_CID}),
        TypeDef::Blob(b) => {
            let size = b.max_size.map_or(1000, |m| m.min(1000));
            json!({"$type": "blob", "ref": {"$link": RAW_CID}, "mimeType": mime_for(b.accept.as_ref()), "size": size})
        }
        TypeDef::Array(a) => {
            let n = a.min_length.unwrap_or(1).max(1) as usize;
            Value::Array((0..n).map(|_| sample(reg, &a.items, depth + 1)).collect())
        }
        TypeDef::Object(o) => {
            let mut m = Map::new();
            for (name, pt) in &o.properties {
                m.insert(name.clone(), sample(reg, pt, depth + 1));
            }
            Value::Object(m)
        }
        TypeDef::Ref(r) => sample_def(reg, r, depth + 1),
        TypeDef::Union(u) => {
            let r = &u.refs[0];
            let mut v = sample_def(reg, r, depth + 1);
            let t = r.strip_suffix("#main").unwrap_or(r);
            v.as_object_mut().unwrap().insert("$type".into(), json!(t));
            v
        }
        TypeDef::Unknown => json!({"$type": "com.example.unknown", "a": 1}),
    }
}

/// A sample of a definition reference (`nsid#name`); records get their `$type`.
pub fn sample_def(reg: &Registry, r: &str, depth: usize) -> Value {
    match reg.def(r) {
        Some(Def::Type(t)) => sample(reg, t, depth),
        Some(Def::Record(rd)) => {
            let mut v = sample(reg, &TypeDef::Object(rd.record.clone()), depth);
            let id = r.strip_suffix("#main").unwrap_or(r);
            v.as_object_mut().unwrap().insert("$type".into(), json!(id));
            v
        }
        other => panic!("no sample for {r}: {other:?}"),
    }
}

/// A maximal sample record of the build-namespace collection `nsid`.
pub fn sample_record(nsid: &str) -> Value {
    sample_def(registry(), &format!("{nsid}#main"), 0)
}

/// Adds `"zzUnknown": {"nested": [1, "x"]}` to every object of `v`, except `$link`/`$bytes`
/// objects and blob objects' `ref`.
pub fn add_unknown_fields(v: &mut Value) {
    match v {
        Value::Array(a) => a.iter_mut().for_each(add_unknown_fields),
        Value::Object(m) => {
            if m.contains_key("$link") || m.contains_key("$bytes") {
                return;
            }
            for (_, x) in m.iter_mut() {
                add_unknown_fields(x);
            }
            m.insert(
                "zzUnknown".into(),
                json!({"nested": [1, "x", {"deep": true}]}),
            );
        }
        _ => {}
    }
}
