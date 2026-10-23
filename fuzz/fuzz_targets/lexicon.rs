// SPDX-License-Identifier: AGPL-3.0-or-later
//! The lexicon validator (`lessdugin_lexicon::validate`), the record validity rules
//! (`validity::check`, rules G1–G10 and the per-collection rules), the typed records and the
//! atproto data model of a record, on raw JSON input (as a record arrives from a PDS or the
//! firehose).
//!
//! The collection is the record's `$type` when that names a record lexicon (in the build or the
//! final namespace), else one picked from the input; the record key and an extra definition to
//! validate against are picked from the input too.
//!
//! Contract: never panics, on a 1 MiB stack (wasm32's default); the caps of `Options` hold; and a
//! record that converts to IPLD and encodes as DAG-CBOR decodes back to the same value.

#![no_main]

use std::sync::LazyLock;

use lessdugin_atp::datamodel;
use lessdugin_atp::syntax::{AtUri, Did, Nsid, RecordKey};
use lessdugin_atp::{dagcbor, datamodel::Limits};
use lessdugin_fuzz::common::FUZZ_DID;
use lessdugin_fuzz::{fnv, on_small_stack};
use lessdugin_lexicon::schemas::{Def, registry};
use lessdugin_lexicon::validate::{Options, Validator, validate_record};
use lessdugin_lexicon::{Record, ns, validity};
use libfuzzer_sys::fuzz_target;
use serde_json::Value;

/// Every record collection (build NSIDs).
static RECORDS: LazyLock<Vec<String>> = LazyLock::new(|| {
    registry()
        .record_nsids()
        .into_iter()
        .map(str::to_owned)
        .collect()
});

/// Every data definition (`nsid#name`) and record of the registry.
static DEFS: LazyLock<Vec<String>> = LazyLock::new(|| {
    let mut out = Vec::new();
    for doc in registry().docs() {
        for (name, def) in &doc.defs {
            if matches!(def, Def::Type(_) | Def::Record(_)) {
                out.push(format!("{}#{name}", doc.id));
            }
        }
    }
    out
});

/// Record keys of every shape the collections use (literal, TID, derived keys, junk).
const RKEYS: &[&str] = &[
    "self",
    "3k2akbibjhm2a",
    "3jzfcijpj2z2a",
    "a",
    "fk1-2222222222222-abcdefghijklmnop",
    "sk1:abc",
    "online.7778777.lessdugin.temp.post",
    "x~y:z._-",
];

fn collection(value: &Value, h: u64) -> String {
    let named = value.get("$type").and_then(Value::as_str).and_then(|t| {
        if RECORDS.iter().any(|r| r == t) {
            Some(t.to_owned())
        } else {
            ns::to_build_nsid(t).filter(|b| RECORDS.contains(b))
        }
    });
    named.unwrap_or_else(|| RECORDS[(h % RECORDS.len() as u64) as usize].clone())
}

fn check(data: &[u8], value: &Value) {
    let h = fnv(data);
    let nsid = collection(value, h);
    let reg = registry();

    let _ = validate_record(&nsid, value).is_valid();
    let opts = Options {
        allow_legacy_blobs: false,
        max_depth: 8,
        max_steps: 512,
        max_violations: 4,
    };
    let rkey = RKEYS[((h >> 8) % RKEYS.len() as u64) as usize];
    let capped = Validator::new(reg, opts).record(&nsid, Some(rkey), value);
    assert!(capped.violations.len() <= opts.max_violations);
    let def = &DEFS[((h >> 16) % DEFS.len() as u64) as usize];
    let _ = Validator::new(reg, Options::default()).def(def, value);
    let _ = Validator::new(reg, Options::default()).with_defaults(&nsid, value);

    if let (Ok(did), Ok(coll), Ok(rk)) = (
        Did::parse(FUZZ_DID),
        Nsid::parse(&nsid),
        RecordKey::parse(rkey),
    ) && let Ok(uri) = AtUri::new(did, coll, rk)
    {
        let v = validity::check(&nsid, &uri, value);
        let _ = (v.is_effective(), v.first_rule(), v.rule_ids());
    }
    let _ = validity::snapshot_renderable(value);
    if let Some(Ok(rec)) = Record::from_nsid_value(&nsid, value.clone()) {
        let _ = rec.to_value();
        let _ = (rec.collection(), rec.r#type());
    }

    if let Ok(ipld) = datamodel::record_json_to_ipld(value) {
        let _ = datamodel::blob_refs(&ipld);
        if let Ok(cbor) = dagcbor::encode_with(&ipld, &Limits::DEFAULT) {
            assert_eq!(
                dagcbor::decode_with(&cbor, &Limits::DEFAULT).as_ref(),
                Ok(&ipld),
                "an encoded record must decode back"
            );
            let _ = datamodel::record_cbor_to_json(&cbor);
        }
        let _ = datamodel::record_cid(value);
    }
}

fuzz_target!(|data: &[u8]| {
    let _ = datamodel::record_json_bytes_to_ipld(data);
    let Ok(value) = serde_json::from_slice::<Value>(data) else {
        return;
    };
    on_small_stack(|| check(data, &value));
});
