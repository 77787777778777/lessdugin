// SPDX-License-Identifier: MIT OR Apache-2.0
//! atproto interop MST fixtures (`testdata/interop/mst`, `testdata/interop/firehose`).

mod common;

// Natively these are libtest tests; on wasm32 (`cargo xtask test-wasm`) the same bodies run
// under wasm-bindgen-test in node.
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

use std::collections::BTreeMap;

use lessdugin_atp::Cid;
use lessdugin_atp::mst::{self, Proof};

#[test]
fn key_heights() {
    let fixtures = common::json("mst/key_heights.json");
    let fixtures = fixtures.as_array().unwrap();
    assert!(fixtures.len() >= 9);
    for f in fixtures {
        let key = f["key"].as_str().unwrap();
        assert_eq!(
            u64::from(mst::key_height(key.as_bytes())),
            f["height"].as_u64().unwrap(),
            "{key:?}"
        );
    }
}

#[test]
fn common_prefix() {
    for f in common::json("mst/common_prefix.json").as_array().unwrap() {
        let (l, r) = (f["left"].as_str().unwrap(), f["right"].as_str().unwrap());
        assert_eq!(
            mst::common_prefix_len(l.as_bytes(), r.as_bytes()) as u64,
            f["len"].as_u64().unwrap()
        );
    }
}

#[test]
fn example_keys_have_their_heights() {
    // gen_keys.py names keys "<letter><height>/<n>"
    for k in common::lines("mst/example_keys.txt") {
        let expected: u32 = k[1..2].parse().unwrap();
        assert_eq!(mst::key_height(k.as_bytes()), expected, "{k}");
    }
}

fn cid(s: &serde_json::Value) -> Cid {
    lessdugin_atp::cid::parse(s.as_str().unwrap()).unwrap()
}

fn strs(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn commit_proof_fixtures() {
    let fixtures = common::json("firehose/commit-proof-fixtures.json");
    let fixtures = fixtures.as_array().unwrap();
    assert_eq!(fixtures.len(), 6);
    for f in fixtures {
        let name = f["comment"].as_str().unwrap();
        let leaf = cid(&f["leafValue"]);
        let keys = strs(&f["keys"]);
        let adds = strs(&f["adds"]);
        let dels = strs(&f["dels"]);

        let (before, _) = mst::build(keys.iter().map(|k| (k.as_bytes(), leaf))).unwrap();
        assert_eq!(before, cid(&f["rootBeforeCommit"]), "{name}: root before");

        let mut after_keys: Vec<&String> = keys.iter().chain(&adds).collect();
        after_keys.retain(|k| !dels.contains(k));
        let (after, blocks) = mst::build(after_keys.iter().map(|k| (k.as_bytes(), leaf))).unwrap();
        assert_eq!(after, cid(&f["rootAfterCommit"]), "{name}: root after");

        // full validation of the built tree
        let listed = mst::list_entries(&after, &blocks, 1000).unwrap();
        assert_eq!(listed.len(), after_keys.len());

        // the proof blocks alone prove every operation
        let proof_cids: Vec<Cid> = f["blocksInProof"]
            .as_array()
            .unwrap()
            .iter()
            .map(cid)
            .collect();
        let proof_blocks: BTreeMap<Cid, Vec<u8>> = proof_cids
            .iter()
            .map(|c| {
                (
                    *c,
                    blocks
                        .get(c)
                        .unwrap_or_else(|| panic!("{name}: {c} not in tree"))
                        .clone(),
                )
            })
            .collect();
        for k in &adds {
            assert_eq!(
                mst::prove(&after, k.as_bytes(), &proof_blocks).unwrap(),
                Proof::Present(leaf),
                "{name}: {k}"
            );
        }
        for k in &dels {
            assert_eq!(
                mst::prove(&after, k.as_bytes(), &proof_blocks).unwrap(),
                Proof::Absent,
                "{name}: {k}"
            );
        }
        // and every key is provable against the full tree
        for k in &after_keys {
            assert_eq!(
                mst::prove(&after, k.as_bytes(), &blocks).unwrap(),
                Proof::Present(leaf)
            );
        }
    }
}
