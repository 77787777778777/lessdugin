// SPDX-License-Identifier: MIT OR Apache-2.0
//! Property tests: JSON ⇄ IPLD ⇄ DAG-CBOR round trips (cross-checked against
//! `serde_ipld_dagcbor`), and decoders/parsers never panic on arbitrary input.

use std::collections::BTreeMap;

use lessdugin_atp::car::{CarLimits, read_car, write_car};
use lessdugin_atp::commit::Commit;
use lessdugin_atp::datamodel::Limits;
use lessdugin_atp::proof::{BundleCaps, ProofBundle, ProofCaps, verify_record_proof};
use lessdugin_atp::syntax::{
    AtIdentifier, AtUri, Datetime, Did, Handle, LooseAtUri, Nsid, RecordKey, Tid,
};
use lessdugin_atp::{Cid, Ipld, cid, dagcbor, datamodel, mst};
use proptest::prelude::*;

fn arb_cid() -> impl Strategy<Value = Cid> {
    (
        any::<[u8; 32]>(),
        prop_oneof![Just(cid::DAG_CBOR), Just(cid::RAW)],
    )
        .prop_map(|(d, codec)| cid::from_sha256_digest(codec, d))
}

fn arb_leaf() -> impl Strategy<Value = Ipld> {
    prop_oneof![
        Just(Ipld::Null),
        any::<bool>().prop_map(Ipld::Bool),
        any::<i64>().prop_map(|i| Ipld::Integer(i128::from(i))),
        ".{0,24}".prop_map(Ipld::String),
        proptest::collection::vec(any::<u8>(), 0..40).prop_map(Ipld::Bytes),
        arb_cid().prop_map(Ipld::Link),
    ]
}

/// Data-model-valid IPLD: map keys avoid the reserved `$link` / `$bytes` / `$type`.
fn arb_ipld() -> impl Strategy<Value = Ipld> {
    arb_leaf().prop_recursive(6, 64, 8, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..8).prop_map(Ipld::List),
            proptest::collection::btree_map("[a-zA-Z0-9_.é]{0,10}", inner, 0..8)
                .prop_map(Ipld::Map),
        ]
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn cbor_roundtrip_and_reference_agreement(v in arb_ipld()) {
        let bytes = dagcbor::encode(&v).unwrap();
        prop_assert_eq!(&dagcbor::decode(&bytes).unwrap(), &v);
        prop_assert_eq!(&serde_ipld_dagcbor::to_vec(&v).unwrap(), &bytes);
        let reference: Ipld = serde_ipld_dagcbor::from_slice(&bytes).unwrap();
        prop_assert_eq!(&reference, &v);
    }

    #[test]
    fn json_roundtrip(v in arb_ipld()) {
        let json = datamodel::ipld_to_json(&v).unwrap();
        prop_assert_eq!(&datamodel::json_to_ipld(&json).unwrap(), &v);
        // through JSON text as well
        let text = serde_json::to_vec(&json).unwrap();
        let reparsed: serde_json::Value = serde_json::from_slice(&text).unwrap();
        prop_assert_eq!(&datamodel::json_to_ipld(&reparsed).unwrap(), &v);
        if matches!(v, Ipld::Map(_)) {
            let cid1 = datamodel::record_cid(&json).unwrap();
            prop_assert_eq!(cid1, cid::for_dag_cbor(&dagcbor::encode(&v).unwrap()));
        }
    }

    #[test]
    fn decoders_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = dagcbor::decode(&bytes);
        let _ = datamodel::record_from_cbor(&bytes);
        let _ = datamodel::record_json_bytes_to_ipld(&bytes);
        let _ = read_car(&bytes, &CarLimits::PROOF);
        let _ = mst::Node::decode(&bytes);
        let _ = Commit::decode(&bytes);
        let _ = lessdugin_atp::did_doc::DidDocument::parse(&bytes);
        let _ = cid::from_bytes(&bytes);
        let _ = ProofBundle::parse(&bytes, &BundleCaps::default());
        // the raw navigators never panic, even on input that was not validated
        if let Some(it) = dagcbor::map_entries_raw(&bytes) {
            for (_, v) in it {
                let _ = dagcbor::decode_scalar(v);
            }
        }
        if let Some(it) = dagcbor::list_items_raw(&bytes) {
            for v in it {
                let _ = dagcbor::decode_scalar(v);
            }
        }
    }

    #[test]
    fn validate_agrees_with_decode(head in 0x80u8..0xe0, bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
        let mut b = vec![head];
        b.extend(bytes);
        prop_assert_eq!(
            dagcbor::validate_with(&b, &Limits::default()),
            dagcbor::decode(&b).map(|_| ())
        );
        if dagcbor::decode(&b).is_ok() {
            prop_assert!(dagcbor::validate_with(&b, &Limits::STRICT).is_ok());
        }
    }

    #[test]
    fn raw_navigation_matches_decode(v in arb_ipld()) {
        let bytes = dagcbor::encode(&v).unwrap();
        match &v {
            Ipld::Map(m) => {
                let entries: Vec<_> = dagcbor::map_entries_raw(&bytes).unwrap().collect();
                prop_assert_eq!(entries.len(), m.len());
                for (k, raw) in entries {
                    prop_assert_eq!(&dagcbor::decode(raw).unwrap(), &m[k]);
                }
            }
            Ipld::List(l) => {
                let items: Vec<_> = dagcbor::list_items_raw(&bytes).unwrap().collect();
                prop_assert_eq!(items.len(), l.len());
                for (raw, item) in items.into_iter().zip(l) {
                    prop_assert_eq!(&dagcbor::decode(raw).unwrap(), item);
                }
            }
            scalar => prop_assert_eq!(dagcbor::decode_scalar(&bytes), Some(scalar.clone())),
        }
    }

    #[test]
    fn cbor_prefixed_garbage_never_panics(head in 0x80u8..0xe0, bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
        // start with a container/tag head so the decoder goes deeper than a random first byte
        let mut b = vec![head];
        b.extend(bytes);
        let _ = dagcbor::decode(&b);
        let _ = mst::Node::decode(&b);
    }

    #[test]
    fn mutated_car_never_panics(
        idx in any::<prop::sample::Index>(),
        val in any::<u8>(),
        cut in any::<prop::sample::Index>(),
    ) {
        let (car, did, coll, rkey, key) = sample_proof();
        let mut t = car.clone();
        let i = idx.index(t.len());
        t[i] = val;
        let _ = read_car(&t, &CarLimits::PROOF);
        let r = verify_record_proof(&t, &did, &coll, &rkey, &key, NOW, &ProofCaps::default());
        // every block is needed, so any change must break the proof
        if t != car {
            prop_assert!(r.is_err());
        }
        let c = cut.index(car.len());
        let _ = verify_record_proof(&car[..c], &did, &coll, &rkey, &key, NOW, &ProofCaps::default());
    }

    #[test]
    fn syntax_parsers_never_panic(s in "\\PC{0,80}") {
        let _ = Did::parse(&s);
        let _ = Handle::parse(&s);
        let _ = Nsid::parse(&s);
        let _ = RecordKey::parse(&s);
        let _ = AtUri::parse(&s);
        let _ = LooseAtUri::parse(&s);
        let _ = LooseAtUri::parse_user_input(&s);
        let _ = AtIdentifier::parse(&s);
        let _ = Datetime::parse(&s);
        let _ = Tid::parse(&s);
        let _ = cid::parse(&s);
        let _ = lessdugin_atp::crypto::PublicKey::from_did_key(&s);
        let _ = lessdugin_atp::did_doc::normalize_service_endpoint(&s);
    }

    #[test]
    fn datetime_like_strings_never_panic(s in "[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\\.[0-9]{1,25})?(Z|[+-][0-9]{2}:[0-9]{2}|.{0,3})") {
        if let Ok(d) = Datetime::parse(&s) {
            let c = d.to_canonical();
            prop_assert_eq!(Datetime::parse(c.as_str()).unwrap().timestamp_micros(), c.timestamp_micros());
        }
    }

    #[test]
    fn tid_roundtrip(v in any::<u64>()) {
        let t = Tid::from_u64(v);
        prop_assert_eq!(Tid::parse(t.as_str()).unwrap().to_u64(), v);
    }

    #[test]
    fn tid_order_matches_integer_order(a in any::<u64>(), b in any::<u64>()) {
        prop_assert_eq!(Tid::from_u64(a).cmp(&Tid::from_u64(b)), a.cmp(&b));
    }

    #[test]
    fn datetime_writer_roundtrip(us in -62_167_219_200_000_000i64..253_402_300_800_000_000i64) {
        let d = Datetime::from_micros(us).unwrap();
        prop_assert!(d.is_canonical());
        let p = Datetime::parse(d.as_str()).unwrap();
        prop_assert_eq!(p.timestamp_micros(), us.div_euclid(1000) * 1000);
    }

    #[test]
    fn mst_build_prove_list(keys in proptest::collection::btree_set("[a-c]{1,3}\\.[a-c]{1,3}\\.[a-c]{1,3}/[a-z0-9]{1,6}", 0..60)) {
        let leaf = cid::for_dag_cbor(b"\xa0");
        let (root, blocks) = mst::build(keys.iter().map(|k| (k.as_bytes(), leaf))).unwrap();
        let listed = mst::list_entries(&root, &blocks, 10_000).unwrap();
        prop_assert_eq!(listed.len(), keys.len());
        for ((k, _), e) in listed.iter().zip(&keys) {
            prop_assert_eq!(k.as_slice(), e.as_bytes());
        }
        for k in &keys {
            prop_assert_eq!(mst::prove(&root, k.as_bytes(), &blocks).unwrap(), mst::Proof::Present(leaf));
            let absent = format!("{k}~");
            prop_assert_eq!(mst::prove(&root, absent.as_bytes(), &blocks).unwrap(), mst::Proof::Absent);
        }
    }
}

const NOW: i64 = 1_760_000_000_000_000;

#[allow(clippy::type_complexity)]
fn sample_proof() -> (
    Vec<u8>,
    Did,
    Nsid,
    RecordKey,
    lessdugin_atp::crypto::PublicKey,
) {
    use k256::ecdsa::signature::Signer as _;
    let did = Did::parse("did:plc:ewvi7nxzyoun6zhxrhs64oiz").unwrap();
    let coll = Nsid::parse("online.7778777.lessdugin.post").unwrap();
    let rkey = RecordKey::parse("uetjixxr3a33-3l2s5xxv2ze2c").unwrap();
    let rec = datamodel::record_to_cbor(&serde_json::json!({"$type": coll.as_str(), "text": "hi"}))
        .unwrap();
    let rcid = cid::for_dag_cbor(&rec);
    let mut entries = BTreeMap::new();
    entries.insert(mst::repo_path(&coll, &rkey), rcid);
    for i in 0..20 {
        entries.insert(format!("{}/k{i}", coll.as_str()), rcid);
    }
    let (root, nodes) = mst::build(entries.iter().map(|(k, v)| (k.as_bytes(), *v))).unwrap();
    let sk = k256::ecdsa::SigningKey::from_slice(&[5u8; 32]).unwrap();
    let u = lessdugin_atp::commit::UnsignedCommit {
        did: did.clone(),
        data: root,
        rev: Tid::from_parts(NOW as u64, 0).unwrap(),
        prev: None,
    };
    let sig: k256::ecdsa::Signature = sk.sign(&u.signing_bytes());
    let (ccid, cbytes) = u.signed(&sig.to_bytes());
    // only the blocks the proof needs: commit, the nodes on the path, the record
    let path = mst::repo_path(&coll, &rkey);
    let mut blocks: Vec<(Cid, Vec<u8>)> = vec![(ccid, cbytes), (rcid, rec)];
    let mut node_cid = root;
    loop {
        let bytes = nodes[&node_cid].clone();
        let n = mst::Node::decode(&bytes).unwrap();
        blocks.push((node_cid, bytes));
        let keys = n.keys().unwrap();
        let idx = keys.partition_point(|k| k.as_slice() < path.as_bytes());
        if keys
            .get(idx)
            .is_some_and(|k| k.as_slice() == path.as_bytes())
        {
            break;
        }
        node_cid = if idx == 0 {
            n.left
        } else {
            n.entries[idx - 1].tree
        }
        .unwrap();
    }
    let car = write_car(&[ccid], blocks.iter().map(|(c, b)| (c, b.as_slice())));
    (
        car,
        did,
        coll,
        rkey,
        lessdugin_atp::crypto::PublicKey::Secp256k1(*sk.verifying_key()),
    )
}
