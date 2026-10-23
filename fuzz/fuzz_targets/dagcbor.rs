// SPDX-License-Identifier: AGPL-3.0-or-later
//! Strict DAG-CBOR (`lessdugin_atp::dagcbor`) and the data model on top of it
//! (`lessdugin_atp::datamodel`), plus the raw navigation helpers and the schema decoders that read
//! validated bytes (commits, MST nodes).
//!
//! Contract: never panics, and
//! - `validate_with` reaches exactly the verdict (and error) of `decode_with` under the same limits;
//! - only canonical input decodes, so re-encoding a decoded value gives back the input;
//! - IPLD → atproto JSON → IPLD is the identity whenever the first step succeeds.

#![no_main]

use lessdugin_atp::commit::Commit;
use lessdugin_atp::dagcbor::{
    decode_scalar, decode_with, encode_with, list_items_raw, map_entries_raw, validate_with,
};
use lessdugin_atp::datamodel::{self, Limits};
use lessdugin_atp::mst;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let small = Limits {
        max_depth: 4,
        max_container_len: 8,
        max_string_bytes: 16,
        max_bytes_len: 16,
        max_total_nodes: 32,
    };
    for limits in [Limits::DEFAULT, Limits::STRICT, small] {
        let decoded = decode_with(data, &limits);
        assert_eq!(
            validate_with(data, &limits),
            decoded.as_ref().map(|_| ()).map_err(Clone::clone),
            "validate_with and decode_with disagree"
        );
        let Ok(v) = decoded else { continue };
        assert_eq!(
            encode_with(&v, &limits).as_deref(),
            Ok(data),
            "a decoded value must re-encode to its (canonical) input"
        );
        let _ = datamodel::validate(&v);
        let _ = datamodel::nesting_depth(&v);
        let _ = datamodel::blob_refs(&v);
        if let Ok(json) = datamodel::ipld_to_json(&v) {
            assert_eq!(
                datamodel::json_to_ipld(&json).as_ref(),
                Ok(&v),
                "IPLD -> JSON -> IPLD"
            );
            let _ = datamodel::blob_refs_json(&json);
        }
    }
    let _ = datamodel::record_from_cbor(data);
    let _ = datamodel::record_cbor_to_json(data);

    // Raw navigation never panics, on any bytes (it simply stops early on invalid input).
    let _ = decode_scalar(data);
    if let Some(entries) = map_entries_raw(data) {
        for (_, v) in entries {
            let _ = decode_scalar(v);
            if let Some(items) = list_items_raw(v) {
                items.for_each(|i| drop(decode_scalar(i)));
            }
        }
    }
    if let Some(items) = list_items_raw(data) {
        for i in items {
            let _ = decode_scalar(i);
            if let Some(entries) = map_entries_raw(i) {
                entries.for_each(|(_, v)| drop(decode_scalar(v)));
            }
        }
    }

    // Schema decoders over the same bytes.
    if let Ok(c) = Commit::decode(data) {
        assert_eq!(
            validate_with(c.unsigned_bytes(), &Limits::STRICT),
            Ok(()),
            "the unsigned commit must be strict DAG-CBOR"
        );
    }
    if let Ok(n) = mst::Node::decode(data) {
        let _ = n.keys();
        let _ = n.encode();
    }
});
