// SPDX-License-Identifier: AGPL-3.0-or-later
//! The shared record-store scenario (`sync.md` §10, §12.4): every [`KvStore`] implementation
//! (MemKv, SqliteKv, IndexedDB) runs [`record_store_conformance`] on a fresh, empty store, so the
//! record store's prefix scans, key order, atomic batches and large rows are exercised on the real
//! backend. It panics with a description of the first violation.

use std::collections::BTreeSet;

use lessdugin_atp::datamodel;
use lessdugin_atp::syntax::AtUri;
use lessdugin_core::host::{Dir, KvStore};
use lessdugin_core::store::admit::{AdmittedRecord, NoValidator, Observation, admit_own_write};
use lessdugin_core::store::records::{
    Admission, Delete, DeleteEvidence, OrderKey, RecordStore, TombOutcome,
};
use lessdugin_core::store::repo::{HostRow, RepoRow, RepoStatus};
use lessdugin_lexicon::ns::NS;
use serde_json::{Value, json};

const DID: &str = "did:plc:kvconformancekvconformaa";
const FK: &str = "kvconfkvconf";

fn uri(coll: &str, rkey: &str) -> AtUri {
    AtUri::parse(&format!("at://{DID}/{NS}.{coll}/{rkey}")).expect("test uri")
}

fn record(u: &AtUri, extra: Value) -> AdmittedRecord {
    let mut v = json!({"$type": u.collection().as_str(), "createdAt": "2025-01-01T00:00:00.000Z"});
    if let (Value::Object(m), Value::Object(e)) = (&mut v, extra) {
        m.extend(e);
    }
    let cid = datamodel::record_cid(&v).expect("record cid");
    admit_own_write(u, &v, &cid.to_string(), &NoValidator).expect("admission")
}

/// Runs the scenario on `kv` (which should start empty). Panics on the first violation.
pub async fn record_store_conformance<K: KvStore>(kv: &K) {
    let store = RecordStore::new(kv);
    // rkeys whose bytes exercise the key encoding: '-' < '.' < digits < ':' < letters < '~'
    let keys = [
        format!("{FK}-2a"),
        format!("{FK}-2a-x"),
        format!("{FK}-2a.x"),
        format!("{FK}-2a:x"),
        format!("{FK}-2b"),
        format!("{FK}-~-zzz"),
    ];
    let mut b = store.batch();
    for (i, k) in keys.iter().enumerate() {
        let u = uri("post", k);
        let r = b
            .put_version(
                record(&u, json!({"i": i}))
                    .into_version(Observation::listing("3l2a", 10, true, 10)),
            )
            .await
            .expect("put");
        assert_eq!(r.admission, Admission::Inserted, "{k}");
    }
    // a large row (about 1 MB of DAG-CBOR) must round-trip
    let big = uri("post", &format!("{FK}-3big"));
    let big_rec = record(&big, json!({"s": "x".repeat(999_000)}));
    let big_cid = big_rec.cid;
    b.put_version(big_rec.into_version(Observation::own_write("3l2b", 11, 11)))
        .await
        .expect("put big");
    // a global row and the membership
    let decl = uri("forum", "conformance");
    b.put_version(record(&decl, json!({})).into_version(Observation::point_fetch("", 5, 5)))
        .await
        .expect("put decl");
    let member = uri("membership", FK);
    b.put_version(record(&member, json!({})).into_version(Observation::point_fetch("3l2a", 5, 5)))
        .await
        .expect("put member");
    let mut repo = RepoRow::new("https://pds.example");
    repo.colls = vec![format!("{NS}.post")];
    repo.observe_head("3l2b", big_cid, 12);
    b.put_repo(DID, &repo).expect("repo row");
    b.put_host_caps("https://pds.example", &HostRow::default())
        .expect("host row");
    // nothing is visible before the commit
    assert!(
        store.get(&decl).await.expect("get").is_none(),
        "batch leaked before commit"
    );
    let c = b.commit().await.expect("commit");
    assert_eq!(c.changed.len(), keys.len() + 3);

    // read back
    let row = store.get(&big).await.expect("get big").expect("big row");
    assert_eq!(row.cid, big_cid);
    assert_eq!(lessdugin_atp::cid::for_dag_cbor(&row.cbor), big_cid, "I2");
    assert!(store.get(&decl).await.expect("get decl").is_some());

    // key order within the range = rkey byte order, both directions
    let fwd: Vec<String> = store
        .scan_repo_range(DID, &format!("{NS}.post"), FK, Dir::Forward)
        .with_page_size(2)
        .collect()
        .await
        .expect("scan")
        .into_iter()
        .map(|r| r.key.rkey)
        .collect();
    let mut want: Vec<String> = keys.to_vec();
    want.push(format!("{FK}-3big"));
    want.sort();
    assert_eq!(fwd, want, "forward range order");
    let rev: Vec<String> = store
        .scan_repo_range(DID, &format!("{NS}.post"), FK, Dir::Reverse)
        .with_page_size(3)
        .collect()
        .await
        .expect("scan")
        .into_iter()
        .map(|r| r.key.rkey)
        .collect();
    want.reverse();
    assert_eq!(rev, want, "reverse range order");
    // the forum prefix holds the posts and the membership, not the declaration
    let forum = store
        .scan_forum(FK, None)
        .collect()
        .await
        .expect("scan forum");
    assert_eq!(forum.len(), keys.len() + 2);
    assert!(forum.iter().all(|r| r.key.scope == FK));

    // a complete, stable listing without two keys: one tombstoned, one beyond the cap dropped
    let mut b = store.batch();
    let mut listed: BTreeSet<String> = keys[1..5].iter().cloned().collect();
    listed.insert(format!("{FK}-3big"));
    let diff = b
        .reconcile_range(
            DID,
            &format!("{NS}.post"),
            FK,
            &listed,
            &OrderKey::new("3l2c", 20),
            Some(&keys[1]),
            20,
        )
        .await
        .expect("reconcile");
    assert_eq!(diff.dropped, vec![keys[0].clone()]);
    assert_eq!(diff.tombstoned.len(), 1);
    assert!(matches!(
        diff.tombstoned[0].1,
        TombOutcome::Created { removed: Some(_) }
    ));
    let mut r = store.repo(DID).await.expect("repo").expect("repo row");
    r.apply_stable_pass("3l2c", big_cid, [(FK, std::collections::BTreeMap::new())]);
    b.put_repo(DID, &r).expect("repo row");
    b.commit().await.expect("commit");
    // listed at 3l2c ≥ the head seen (3l2b): Fresh
    assert_eq!(
        store
            .repo(DID)
            .await
            .expect("repo")
            .expect("repo row")
            .repo_status(FK),
        RepoStatus::Fresh
    );
    let tomb = uri("post", &keys[5]);
    assert!(store.get(&tomb).await.expect("get").is_none());
    let t = store
        .tombstone_of(&tomb)
        .await
        .expect("tomb")
        .expect("tomb row");
    assert_eq!(t.key(), OrderKey::new("3l2c", 20));
    // the tombstone suppresses an older observation and is absent for the fold
    let mut b = store.batch();
    let old = b
        .put_version(
            record(&tomb, json!({"i": 5})).into_version(Observation::listing("3l2a", 30, true, 30)),
        )
        .await
        .expect("put old");
    assert_eq!(old.admission, Admission::Suppressed);
    let del = b
        .tombstone(
            &uri("post", &keys[1]),
            Delete {
                key: OrderKey::new("3l2d", 40),
                evidence: DeleteEvidence::RecordNotFound,
                at_ms: 40,
            },
        )
        .await
        .expect("tombstone");
    assert!(matches!(del.outcome, TombOutcome::Created { .. }));
    b.commit().await.expect("commit");
    let absent = store
        .absent(
            FK,
            [
                tomb.as_str(),
                uri("post", &keys[1]).as_str(),
                uri("post", &keys[2]).as_str(),
                uri("post", &format!("{FK}-never")).as_str(),
            ],
        )
        .await
        .expect("absent");
    let want: BTreeSet<String> = [
        tomb.to_string(),
        uri("post", &keys[1]).to_string(),
        uri("post", &format!("{FK}-never")).to_string(),
    ]
    .into();
    assert_eq!(absent, want);
}
