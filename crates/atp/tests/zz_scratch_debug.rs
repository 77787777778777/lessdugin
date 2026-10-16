use std::collections::BTreeMap;
use lessdugin_atp::car::{CarLimits, read_car, write_car};
use lessdugin_atp::proof::{ProofCaps, verify_record_proof};
use lessdugin_atp::syntax::{Did, Nsid, RecordKey, Tid};
use lessdugin_atp::{Cid, cid, datamodel, mst};
const NOW: i64 = 1_760_000_000_000_000;
#[test]
fn find() {
    let (car, did, coll, rkey, key) = sample_proof();
    for i in 0..car.len() {
        for val in [0u8, 0xff, 1] {
            let mut t = car.clone();
            if t[i] == val { continue; }
            t[i] = val;
            if let Ok(o) = verify_record_proof(&t, &did, &coll, &rkey, &key, NOW, &ProofCaps::default()) {
                let c = read_car(&t, &CarLimits::PROOF).unwrap();
                let c0 = read_car(&car, &CarLimits::PROOF).unwrap();
                println!("byte {i} (orig {:#x}) -> {val:#x} still verifies; blocks {} vs {}, present {}", car[i], c.blocks.len(), c0.blocks.len(), o.is_present());
                println!("len {}", car.len());
            }
        }
    }
}
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
    let mut blocks: Vec<(Cid, Vec<u8>)> = vec![(ccid, cbytes), (rcid, rec)];
    blocks.extend(nodes);
    let car = write_car(&[ccid], blocks.iter().map(|(c, b)| (c, b.as_slice())));
    (
        car,
        did,
        coll,
        rkey,
        lessdugin_atp::crypto::PublicKey::Secp256k1(*sk.verifying_key()),
    )
}
