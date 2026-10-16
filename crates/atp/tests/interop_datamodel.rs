// SPDX-License-Identifier: MIT OR Apache-2.0
//! atproto interop data-model fixtures (`testdata/interop/data-model`), cross-checked against
//! `serde_ipld_dagcbor`.

mod common;

use lessdugin_atp::{Ipld, dagcbor, datamodel};

#[test]
fn fixtures_json_cbor_cid() {
    let fixtures = common::json("data-model/data-model-fixtures.json");
    let fixtures = fixtures.as_array().unwrap();
    assert!(fixtures.len() >= 3);
    for f in fixtures {
        let json = &f["json"];
        let expected_cbor = common::b64(f["cbor_base64"].as_str().unwrap());
        let expected_cid = f["cid"].as_str().unwrap();

        let ipld = datamodel::record_json_to_ipld(json).unwrap();
        let cbor = dagcbor::encode(&ipld).unwrap();
        assert_eq!(cbor, expected_cbor, "cbor for {json}");
        assert_eq!(datamodel::record_to_cbor(json).unwrap(), expected_cbor);
        assert_eq!(
            datamodel::record_cid(json).unwrap().to_string(),
            expected_cid
        );
        assert_eq!(
            lessdugin_atp::cid::for_dag_cbor(&cbor).to_string(),
            expected_cid
        );

        // CBOR -> JSON round trip
        let decoded = dagcbor::decode(&expected_cbor).unwrap();
        assert_eq!(decoded, ipld);
        assert_eq!(&datamodel::ipld_to_json(&decoded).unwrap(), json);
        assert_eq!(
            &datamodel::record_cbor_to_json(&expected_cbor).unwrap(),
            json
        );

        // reference encoder agrees byte-for-byte, reference decoder agrees on the value
        assert_eq!(serde_ipld_dagcbor::to_vec(&ipld).unwrap(), expected_cbor);
        let reference: Ipld = serde_ipld_dagcbor::from_slice(&expected_cbor).unwrap();
        assert_eq!(reference, decoded);
    }
}

#[test]
fn valid_values() {
    let fixtures = common::json("data-model/data-model-valid.json");
    for f in fixtures.as_array().unwrap() {
        let ipld = datamodel::record_json_to_ipld(&f["json"])
            .unwrap_or_else(|e| panic!("{}: {e}", f["note"]));
        let cbor = dagcbor::encode(&ipld).unwrap();
        assert_eq!(datamodel::record_from_cbor(&cbor).unwrap(), ipld);
    }
}

#[test]
fn invalid_values() {
    let fixtures = common::json("data-model/data-model-invalid.json");
    for f in fixtures.as_array().unwrap() {
        assert!(
            datamodel::record_json_to_ipld(&f["json"]).is_err(),
            "should be invalid: {}",
            f["note"]
        );
    }
}

#[test]
fn live_record_cid() {
    let rec = common::json("live/bsky.app-profile-self.record.json");
    let cid = datamodel::record_cid(&rec["value"]).unwrap();
    assert_eq!(cid.to_string(), rec["cid"].as_str().unwrap());
    let blobs = datamodel::blob_refs_json(&rec["value"]);
    assert_eq!(blobs.len(), 2);
    assert!(
        blobs
            .iter()
            .all(|b| b.mime_type == "image/jpeg" && b.cid.codec() == lessdugin_atp::cid::RAW)
    );
}
