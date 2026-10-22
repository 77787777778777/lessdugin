// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests of the record store: the pure admission/tombstone rules, the `RecordStore`/`Batch` API
//! over `MemKv`, and property tests of invariants I2–I5 and arrival-order independence.

use std::collections::{BTreeMap, BTreeSet};

use futures::executor::block_on;
use lessdugin_atp::syntax::AtUri;
use lessdugin_atp::{Cid, datamodel};
use lessdugin_lexicon::ns::NS;
use proptest::prelude::*;
use serde_json::json;

use super::*;
use crate::host::{Dir, KvOp, KvStore};
use crate::store::admit::Validity;
use crate::store::blob::{BlobRow, CursorRow, ForumRow};
use crate::store::mem::MemKv;
use crate::store::repo::{HostRow, RepoRow};
use crate::store::schema::{self, RecKey, tag};

const DID: &str = "did:plc:ewvi7nxzyoun6zhxrhs64oiz";
const DID2: &str = "did:plc:abcdefghijklmnopqrstuvwx";
const FK: &str = "uetjixxr3a33";
const FK2: &str = "2xbiijsnjtau";

fn coll(short: &str) -> String {
    format!("{NS}.{short}")
}

fn uri_in(did: &str, short: &str, rkey: &str) -> AtUri {
    AtUri::parse(&format!("at://{did}/{}/{rkey}", coll(short))).unwrap()
}

fn post_uri(n: u32) -> AtUri {
    uri_in(DID, "post", &format!("{FK}-3l2s5xxv2{n:04}"))
}

/// The DAG-CBOR bytes and CID of a small record value numbered `n` in collection `c`.
fn content(c: &str, n: u64) -> (Vec<u8>, Cid) {
    let v = json!({"$type": c, "n": n});
    let cbor = datamodel::record_to_cbor(&v).unwrap();
    let cid = lessdugin_atp::cid::for_dag_cbor(&cbor);
    (cbor, cid)
}

#[derive(Clone)]
struct V {
    level: Level,
    exact: bool,
    src: Source,
    id_hint: bool,
    first_seen: i64,
}

impl Default for V {
    fn default() -> Self {
        Self {
            level: Level::Pds,
            exact: true,
            src: Source::List,
            id_hint: false,
            first_seen: 1_000_000,
        }
    }
}

fn version_with(uri: &AtUri, n: u64, key: OrderKey, o: V) -> Version {
    let (cbor, cid) = content(uri.collection().as_str(), n);
    Version {
        uri: uri.clone(),
        cid,
        cbor,
        validity: if n % 7 == 6 {
            Validity::from_violations(vec![crate::store::admit::RuleViolation::new("G2", "x")])
        } else {
            Validity::valid()
        },
        level: o.level,
        proven_rev: (o.level == Level::Proven).then(|| key.rev_lo.clone()),
        id_hint: o.id_hint,
        key,
        exact: o.exact,
        src: o.src,
        first_seen_us: o.first_seen,
        fetched_at_ms: 7,
    }
}

fn version(uri: &AtUri, n: u64, rev: &str, sent: i64) -> Version {
    version_with(uri, n, OrderKey::new(rev, sent), V::default())
}

fn del(rev: &str, sent: i64) -> Delete {
    Delete {
        key: OrderKey::new(rev, sent),
        evidence: DeleteEvidence::CompleteListing,
        at_ms: sent,
    }
}

fn admit(slot: Slot, v: Version) -> (Slot, Admission) {
    let (s, a, _) = admit_version(slot, v);
    (s, a)
}

fn rec(s: &Slot) -> &RecordRow {
    match s {
        Slot::Rec(r) => r,
        other => panic!("expected a row, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------------
// Pure rules

#[test]
fn insert_merge_replace_and_older() {
    let u = post_uri(1);
    let (s, a) = admit(Slot::Empty, version(&u, 1, "3l2a", 10));
    assert_eq!(a, Admission::Inserted);
    assert_eq!(rec(&s).key(), OrderKey::new("3l2a", 10));
    // a newer key replaces
    let (s, a) = admit(s, version(&u, 2, "3l2b", 5));
    assert_eq!(
        a,
        Admission::Replaced {
            previous: content(u.collection().as_str(), 1).1
        }
    );
    // an older key (by rev, whatever the send time) is dropped (I3)
    let (s2, a) = admit(s.clone(), version(&u, 3, "3l2a", 99));
    assert_eq!(a, Admission::Older);
    assert_eq!(rec(&s2).cid, rec(&s).cid);
    // same rev, later send time, inexact: replaces (it saw a head ≥ that rev later)
    let mut v = version(&u, 4, "3l2b", 6);
    v.exact = false;
    let (s3, a) = admit(s2, v);
    assert!(matches!(a, Admission::Replaced { .. }));
    assert_eq!(rec(&s3).cid, content(u.collection().as_str(), 4).1);
    // equal key, different CID, not both exact: the stored one stays
    let mut v = version(&u, 5, "3l2b", 6);
    v.exact = false;
    let (s4, a) = admit(s3.clone(), v);
    assert_eq!(a, Admission::Older);
    assert_eq!(s4, s3);
}

#[test]
fn same_version_merges_monotonically() {
    let u = post_uri(2);
    let pds = version_with(
        &u,
        1,
        OrderKey::new("3l2a", 10),
        V {
            exact: false,
            src: Source::Get,
            id_hint: true,
            first_seen: 500,
            ..V::default()
        },
    );
    let (s, _) = admit(Slot::Empty, pds);
    let proven = version_with(
        &u,
        1,
        OrderKey::new("3l2c", 20),
        V {
            level: Level::Proven,
            src: Source::Proof,
            first_seen: 900,
            ..V::default()
        },
    );
    let (s, a, changed) = admit_version(s, proven);
    assert_eq!(a, Admission::Merged);
    assert!(changed);
    let r = rec(&s);
    assert_eq!(r.level, Level::Proven);
    assert_eq!(r.rev.as_deref(), Some("3l2c"));
    assert_eq!(r.key(), OrderKey::new("3l2c", 20));
    assert!(r.exact);
    assert!(!r.id_hint, "a doc-level observation clears idHint");
    assert_eq!(r.src, Source::Proof);
    assert_eq!(r.first_seen, 500);
    assert_eq!(r.first_seen_uri, 500);
    // a later pds observation of the same CID never lowers the level (I5) nor the key
    let again = version_with(
        &u,
        1,
        OrderKey::new("3l2b", 30),
        V {
            src: Source::Own,
            first_seen: 100,
            ..V::default()
        },
    );
    let (s2, _, changed) = admit_version(s.clone(), again);
    assert!(changed);
    let r2 = rec(&s2);
    assert_eq!(r2.level, Level::Proven);
    assert_eq!(
        r2.src,
        Source::Proof,
        "the source follows the highest level"
    );
    assert_eq!(r2.key(), OrderKey::new("3l2c", 20));
    assert_eq!(r2.first_seen, 100);
    // an identical observation changes nothing
    let (s3, _, changed) = admit_version(
        s2.clone(),
        version_with(&u, 1, OrderKey::new("3l2a", 1), V::default()),
    );
    assert!(!changed);
    assert_eq!(s3, s2);
}

#[test]
fn same_exact_rev_conflict_keeps_the_higher_level() {
    let u = post_uri(3);
    let listed = version(&u, 1, "3l2a", 10);
    let (s, _) = admit(Slot::Empty, listed);
    // an exact proof at the same rev with another CID and a higher level wins (I3)
    let proof = version_with(
        &u,
        2,
        OrderKey::new("3l2a", 5),
        V {
            level: Level::Proven,
            src: Source::Proof,
            ..V::default()
        },
    );
    let (s2, a) = admit(s.clone(), proof);
    let Admission::Conflict(c) = a else {
        panic!("expected a conflict")
    };
    assert!(c.replaced);
    assert_eq!(c.rev, "3l2a");
    assert_eq!(rec(&s2).level, Level::Proven);
    // the key stays monotone even though the proof was sent earlier
    assert_eq!(rec(&s2).key(), OrderKey::new("3l2a", 10));
    // an exact pds observation at that rev, sent later, does not replace the proven version
    let (s3, a) = admit(s2.clone(), version(&u, 3, "3l2a", 50));
    assert!(matches!(
        a,
        Admission::Conflict(Conflict {
            replaced: false,
            ..
        })
    ));
    assert_eq!(rec(&s3).cid, rec(&s2).cid);
    // at equal level the stored version stays
    let (s4, a) = admit(s.clone(), version(&u, 4, "3l2a", 60));
    assert!(matches!(
        a,
        Admission::Conflict(Conflict {
            replaced: false,
            ..
        })
    ));
    assert_eq!(rec(&s4).cid, rec(&s).cid);
    // a doc-level identity outranks an identity hint at the same level
    let mut hinted = version(&u, 5, "3l2b", 1);
    hinted.id_hint = true;
    let (h, _) = admit(Slot::Empty, hinted);
    let (h2, a) = admit(h, version(&u, 6, "3l2b", 2));
    assert!(matches!(
        a,
        Admission::Conflict(Conflict { replaced: true, .. })
    ));
    assert!(!rec(&h2).id_hint);
}

#[test]
fn tombstones_suppress_older_observations() {
    let u = post_uri(4);
    let (s, _) = admit(Slot::Empty, version(&u, 1, "3l2a", 10));
    // an older delete changes nothing
    let (s1, o, changed) = apply_delete(s.clone(), &del("3l29", 99));
    assert_eq!((o, changed), (TombOutcome::Ignored, false));
    assert_eq!(s1, s);
    // a delete with the same key is not newer: the version stays
    let (s1, o, _) = apply_delete(s.clone(), &del("3l2a", 10));
    assert_eq!(o, TombOutcome::Ignored);
    assert_eq!(s1, s);
    // a newer delete removes the version
    let (t, o, _) = apply_delete(s, &del("3l2b", 20));
    assert_eq!(
        o,
        TombOutcome::Created {
            removed: Some(content(u.collection().as_str(), 1).1)
        }
    );
    let Slot::Tomb(tr) = &t else { panic!() };
    assert_eq!(tr.key(), OrderKey::new("3l2b", 20));
    assert_eq!(tr.ev, DeleteEvidence::CompleteListing);
    // observations with a smaller key are suppressed (I4)…
    let (t2, a) = admit(t.clone(), version(&u, 2, "3l2a", 30));
    assert_eq!(a, Admission::Suppressed);
    assert!(matches!(t2, Slot::Tomb(_)));
    // …an equal key wins over the tombstone, and so does a greater one
    let (r, a) = admit(t.clone(), version(&u, 2, "3l2b", 20));
    assert_eq!(a, Admission::Inserted);
    assert!(matches!(r, Slot::Rec(_)));
    // a tombstone keeps its greatest key
    let (t3, o, _) = apply_delete(t.clone(), &del("3l2a", 1));
    assert_eq!(o, TombOutcome::Unchanged);
    assert_eq!(t3, t);
    let mut raise = del("3l2c", 40);
    raise.evidence = DeleteEvidence::RecordNotFound;
    let (t4, o, _) = apply_delete(t, &raise);
    assert_eq!(o, TombOutcome::Raised);
    let Slot::Tomb(tr) = t4 else { panic!() };
    assert_eq!(
        (tr.key(), tr.at, tr.ev),
        (
            OrderKey::new("3l2c", 40),
            40,
            DeleteEvidence::RecordNotFound
        )
    );
    // a delete of an unknown uri is recorded
    let (e, o, _) = apply_delete(Slot::Empty, &del("", 3));
    assert_eq!(o, TombOutcome::Created { removed: None });
    assert!(matches!(e, Slot::Tomb(_)));
}

#[test]
fn first_seen_is_minimal_and_survives_deletes() {
    let u = post_uri(5);
    let o = |n, rev: &str, sent, fs| {
        version_with(
            &u,
            n,
            OrderKey::new(rev, sent),
            V {
                first_seen: fs,
                ..V::default()
            },
        )
    };
    let (s, _) = admit(Slot::Empty, o(2, "3l2b", 20, 2_000));
    // an older version is dropped, but its first-seen time still counts for the uri
    let (s, a, changed) = admit_version(s, o(1, "3l2a", 10, 1_000));
    assert_eq!(a, Admission::Older);
    assert!(changed);
    let r = rec(&s);
    assert_eq!((r.first_seen, r.first_seen_uri), (2_000, 1_000));
    assert_eq!(r.first_cid, content(u.collection().as_str(), 1).1);
    // the tombstone carries it; the re-created version inherits it
    let (t, _, _) = apply_delete(s, &del("3l2c", 30));
    assert_eq!(t.first_seen_uri(), Some(1_000));
    let (t, a, changed) = admit_version(t, o(9, "3l29", 5, 500));
    assert_eq!(a, Admission::Suppressed);
    assert!(changed);
    assert_eq!(t.first_seen_uri(), Some(500));
    let (r, _) = admit(t, o(3, "3l2d", 40, 4_000));
    let r = rec(&r);
    assert_eq!((r.first_seen, r.first_seen_uri), (4_000, 500));
    // a revert to the earliest CID recovers its first-seen time
    let (s, _) = admit(Slot::Empty, o(1, "3l2a", 10, 1_000));
    let (s, _) = admit(s, o(2, "3l2b", 20, 2_000));
    let (s, _) = admit(s, o(1, "3l2c", 30, 3_000));
    assert_eq!(rec(&s).first_seen, 1_000);
}

#[test]
fn sources_roundtrip() {
    for s in [
        Source::Get,
        Source::List,
        Source::Indexer(DID.into()),
        Source::Proof,
        Source::Own,
    ] {
        assert_eq!(Source::parse(&s.as_string()), Some(s.clone()));
        let bytes = row::encode(&s).unwrap();
        assert_eq!(row::decode::<Source>(&bytes).unwrap(), s);
    }
    assert_eq!(Source::parse("hint"), None);
    assert!(Source::Own > Source::Proof && Source::Proof > Source::List);
}

// ---------------------------------------------------------------------------------------------
// Store over MemKv

fn put(kv: &MemKv, versions: Vec<Version>) -> Vec<AdmitResult> {
    block_on(async {
        let store = RecordStore::new(kv);
        let mut b = store.batch();
        let r = b.put_versions(versions).await.unwrap();
        b.commit().await.unwrap();
        r
    })
}

#[test]
fn store_put_get_and_i2() {
    let kv = MemKv::new();
    let u = post_uri(6);
    let r = put(&kv, vec![version(&u, 1, "3l2a", 10)]);
    assert_eq!(r[0].admission, Admission::Inserted);
    block_on(async {
        let store = RecordStore::new(&kv);
        let row = store.get(&u).await.unwrap().unwrap();
        assert_eq!(row.cid, content(u.collection().as_str(), 1).1);
        assert_eq!(row.value_json().unwrap()["n"], 1);
        assert!(store.tombstone_of(&u).await.unwrap().is_none());
        // I2: bytes that do not hash to the CID are refused, nothing is written
        let mut bad = version(&u, 2, "3l2b", 20);
        bad.cbor = content(u.collection().as_str(), 3).0;
        let mut b = store.batch();
        assert!(matches!(
            b.put_version(bad).await,
            Err(StoreError::CidMismatch { .. })
        ));
        // a raw-codec CID is refused too
        let mut raw = version(&u, 2, "3l2b", 20);
        raw.cid = lessdugin_atp::cid::for_raw(&raw.cbor);
        assert!(b.put_version(raw).await.is_err());
        assert!(b.is_empty());
        assert_eq!(b.commit().await.unwrap().ops, 0);
        assert_eq!(store.get(&u).await.unwrap().unwrap().cid, row.cid);
    });
}

#[test]
fn batch_reads_its_writes_and_commits_atomically() {
    let kv = MemKv::new();
    let u = post_uri(7);
    block_on(async {
        let store = RecordStore::new(&kv);
        let mut b = store.batch();
        b.put_version(version(&u, 1, "3l2a", 10)).await.unwrap();
        let r = b.put_version(version(&u, 2, "3l2b", 20)).await.unwrap();
        assert!(matches!(r.admission, Admission::Replaced { .. }));
        let t = b.tombstone(&u, del("3l2c", 30)).await.unwrap();
        assert!(matches!(
            t.outcome,
            TombOutcome::Created { removed: Some(_) }
        ));
        // nothing is visible before the commit
        assert!(kv.is_empty());
        assert!(matches!(
            b.slot(&RecKey::of_uri(&u)).await.unwrap(),
            Slot::Tomb(_)
        ));
        let c = b.commit().await.unwrap();
        assert_eq!(c.changed, BTreeSet::from([u.to_string()]));
        // one tombstone, no row
        assert_eq!(kv.len(), 1);
        assert!(store.get(&u).await.unwrap().is_none());
        assert_eq!(
            store.tombstone_of(&u).await.unwrap().unwrap().key(),
            OrderKey::new("3l2c", 30)
        );
        // a newer version clears the tombstone (no row and tombstone together)
        let mut b = store.batch();
        b.put_version(version(&u, 3, "3l2d", 40)).await.unwrap();
        b.commit().await.unwrap();
        assert_eq!(kv.len(), 1);
        assert!(store.tombstone_of(&u).await.unwrap().is_none());
        // re-admitting the same version writes nothing
        let mut b = store.batch();
        let r = b.put_version(version(&u, 3, "3l2d", 40)).await.unwrap();
        assert!(!r.changed);
        assert_eq!(b.commit().await.unwrap().ops, 0);
    });
}

fn seed_forum(kv: &MemKv) -> Vec<AtUri> {
    let uris = vec![
        uri_in(DID, "post", &format!("{FK}-3l2s5xxv2ze2c")),
        uri_in(DID, "post", &format!("{FK}-3l2s5xxv2ze3c")),
        uri_in(DID2, "post", &format!("{FK}-3l2s5xxv2ze2c")),
        uri_in(DID, "comment", &format!("{FK}-3l2s5xxv2ze2c-3l2s6aaaaaaa2")),
        uri_in(DID, "membership", FK),
        uri_in(DID, "post", &format!("{FK2}-3l2s5xxv2ze2c")),
        uri_in(DID, "forum", "rationality"),
        uri_in(DID, "profile", "self"),
        uri_in(DID, "post", "3l2s5xxv2ze2c"),
    ];
    put(
        kv,
        uris.iter()
            .enumerate()
            .map(|(i, u)| version(u, i as u64, "3l2a", 10))
            .collect(),
    );
    uris
}

#[test]
fn scans_by_forum_collection_and_range() {
    let kv = MemKv::new();
    let uris = seed_forum(&kv);
    block_on(async {
        let store = RecordStore::new(&kv);
        let all = store.scan_forum(FK, None).collect().await.unwrap();
        let got: Vec<String> = all.iter().map(StoredRecord::uri).collect();
        // the five rows of FK, in key order: (coll, did, rkey)
        let mut want: Vec<(String, String, String)> = uris[..5]
            .iter()
            .map(|u| {
                (
                    u.collection().to_string(),
                    u.did().to_string(),
                    u.rkey().to_string(),
                )
            })
            .collect();
        want.sort();
        let want: Vec<String> = want
            .into_iter()
            .map(|(c, d, r)| format!("at://{d}/{c}/{r}"))
            .collect();
        assert_eq!(got, want);
        // one collection
        let posts = store
            .scan_forum(FK, Some(&coll("post")))
            .collect()
            .await
            .unwrap();
        assert_eq!(posts.len(), 3);
        // small pages give the same rows
        let mut scan = store.scan_forum(FK, None).with_page_size(2);
        let mut paged = Vec::new();
        loop {
            let p = scan.next_page().await.unwrap();
            if p.is_empty() {
                break;
            }
            paged.extend(p);
        }
        assert_eq!(paged, all);
        // a repo's range, both directions
        let fwd = store
            .scan_repo_range(DID, &coll("post"), FK, Dir::Forward)
            .collect()
            .await
            .unwrap();
        let rev = store
            .scan_repo_range(DID, &coll("post"), FK, Dir::Reverse)
            .take(1)
            .await
            .unwrap();
        assert_eq!(fwd.len(), 2);
        assert_eq!(rev[0], fwd[1]);
        // the global scope holds the declaration, the profile and the key without an fk
        let g = store
            .scan_global(DID, &coll("post"))
            .collect()
            .await
            .unwrap();
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].key.scope, schema::GLOBAL_SCOPE);
        assert!(store.get(&uris[6]).await.unwrap().is_some());
        // undecodable rows are skipped (treated as absent)
        kv.write(vec![KvOp::Put(
            schema::rec_key(FK, &coll("post"), DID, &format!("{FK}-zzz")),
            b"garbage".to_vec(),
        )])
        .await
        .unwrap();
        assert_eq!(store.scan_forum(FK, None).collect().await.unwrap().len(), 5);
    });
}

#[test]
fn reconcile_tombstones_unlisted_rows_and_drops_beyond_the_cap() {
    let kv = MemKv::new();
    let rk = |s: &str| format!("{FK}-{s}");
    let keys = [
        "3l2s5xxv2za2c",
        "3l2s5xxv2zb2c",
        "3l2s5xxv2zc2c",
        "3l2s5xxv2zd2c",
    ];
    let uris: Vec<AtUri> = keys.iter().map(|k| uri_in(DID, "post", &rk(k))).collect();
    let mut vs: Vec<Version> = uris
        .iter()
        .enumerate()
        .map(|(i, u)| version(u, i as u64, "3l2a", 10))
        .collect();
    // a row written after the listing started (own write at a later rev) must survive
    let newer = uri_in(DID, "post", &rk("3l2s5xxv2ze2c"));
    vs.push(version(&newer, 9, "3l2z", 50));
    // rows outside the range: the membership (`{fk}` itself) and another repo
    let member = uri_in(DID, "membership", FK);
    vs.push(version(&member, 10, "3l2a", 10));
    let other = uri_in(DID2, "post", &rk("3l2s5xxv2za2c"));
    vs.push(version(&other, 11, "3l2a", 10));
    put(&kv, vs);
    block_on(async {
        let store = RecordStore::new(&kv);
        let mut b = store.batch();
        // the listing returned c and d; it was capped at c (the smallest listed key)
        let listed: BTreeSet<String> = [rk(keys[2]), rk(keys[3])].into();
        // a version admitted earlier in the same batch is seen by the reconcile step
        let fresh = uri_in(DID, "post", &rk("3l2s5xxv2zf2c"));
        b.put_version(version(&fresh, 12, "3l2b", 20))
            .await
            .unwrap();
        let diff = b
            .reconcile_range(
                DID,
                &coll("post"),
                FK,
                &listed,
                &OrderKey::new("3l2b", 20),
                Some(&rk(keys[2])),
                99,
            )
            .await
            .unwrap();
        // a and b are below the boundary: removed without a tombstone
        assert_eq!(diff.dropped, vec![rk(keys[0]), rk(keys[1])]);
        // e (newer than the listing) and f (admitted in this batch at the listing key) are not
        // listed but are above the boundary: tombstoning them is ignored / f is equal → kept
        let outcomes: BTreeMap<String, TombOutcome> = diff.tombstoned.into_iter().collect();
        assert_eq!(outcomes[&rk("3l2s5xxv2ze2c")], TombOutcome::Ignored);
        assert_eq!(outcomes[&rk("3l2s5xxv2zf2c")], TombOutcome::Ignored);
        b.commit().await.unwrap();
        assert!(store.get(&uris[0]).await.unwrap().is_none());
        assert!(store.tombstone_of(&uris[0]).await.unwrap().is_none());
        assert!(store.get(&uris[2]).await.unwrap().is_some());
        assert!(store.get(&newer).await.unwrap().is_some());
        assert!(store.get(&member).await.unwrap().is_some());
        assert!(store.get(&other).await.unwrap().is_some());

        // an uncapped complete listing that misses d tombstones it
        let mut b = store.batch();
        let listed: BTreeSet<String> =
            [rk(keys[2]), rk("3l2s5xxv2ze2c"), rk("3l2s5xxv2zf2c")].into();
        let diff = b
            .reconcile_range(
                DID,
                &coll("post"),
                FK,
                &listed,
                &OrderKey::new("3l2z", 60),
                None,
                100,
            )
            .await
            .unwrap();
        assert_eq!(
            diff.tombstoned,
            vec![(
                rk(keys[3]),
                TombOutcome::Created {
                    removed: Some(content(&coll("post"), 3).1)
                }
            )]
        );
        b.commit().await.unwrap();
        let t = store.tombstone_of(&uris[3]).await.unwrap().unwrap();
        assert_eq!((t.key(), t.at), (OrderKey::new("3l2z", 60), 100));
    });
}

#[test]
fn forget_range_discards_rows_and_tombstones_of_one_repo() {
    let kv = MemKv::new();
    let uris = seed_forum(&kv);
    block_on(async {
        let store = RecordStore::new(&kv);
        // a tombstone in the same range
        let mut b = store.batch();
        b.tombstone(&uris[1], del("3l2z", 99)).await.unwrap();
        b.commit().await.unwrap();
        let mut b = store.batch();
        let gone = b.forget_range(DID, &coll("post"), FK).await.unwrap();
        assert_eq!(gone, vec![uris[0].to_string(), uris[1].to_string()]);
        // the membership, another repo, another fk and global rows are kept
        assert!(!b.forget(&RecKey::of_uri(&uris[0])).await.unwrap());
        b.commit().await.unwrap();
        assert!(store.get(&uris[0]).await.unwrap().is_none());
        assert!(store.tombstone_of(&uris[1]).await.unwrap().is_none());
        for u in [&uris[2], &uris[3], &uris[4], &uris[5], &uris[6]] {
            assert!(store.get(u).await.unwrap().is_some(), "{u}");
        }
        // a forgotten uri admits any later observation, even an older one
        let mut b = store.batch();
        let r = b
            .put_version(version(&uris[0], 0, "3l2a", 1))
            .await
            .unwrap();
        assert_eq!(r.admission, Admission::Inserted);
    });
}

#[test]
fn absent_requires_a_tombstone_or_a_complete_listing() {
    let kv = MemKv::new();
    let p = |did: &str, k: &str| uri_in(did, "post", &format!("{FK}-{k}")).to_string();
    let stored = p(DID, "3l2s5xxv2za2c");
    let tombed = p(DID, "3l2s5xxv2zb2c");
    let missing = p(DID, "3l2s5xxv2zc2c");
    let below_cap = p(DID, "3l2s5xxv2z22c");
    let unlisted_repo = p(DID2, "3l2s5xxv2zc2c");
    let other_fk = uri_in(DID, "post", &format!("{FK2}-3l2s5xxv2zc2c")).to_string();
    let not_ranged = uri_in(DID, "forum", "rationality").to_string();
    let no_coll = uri_in(DID, "sequence", &format!("{FK}-3l2s5xxv2zc2c")).to_string();
    let capped_comment =
        uri_in(DID, "comment", &format!("{FK}-3l2s5xxv2zc2c-3l2s5xxv2zc2c")).to_string();
    put(
        &kv,
        vec![version(&AtUri::parse(&stored).unwrap(), 1, "3l2a", 10)],
    );
    block_on(async {
        let store = RecordStore::new(&kv);
        let mut b = store.batch();
        b.tombstone(&AtUri::parse(&tombed).unwrap(), del("3l2a", 10))
            .await
            .unwrap();
        let mut r = RepoRow::new("https://pds.example");
        r.colls = vec![coll("post"), coll("comment")];
        r.apply_stable_pass(
            "3l2a",
            content("x", 0).1,
            [(
                FK,
                BTreeMap::from([
                    (coll("post"), format!("{FK}-3l2s5xxv2z32c")),
                    (coll("comment"), format!("{FK}-zzzz")),
                ]),
            )],
        );
        b.put_repo(DID, &r).unwrap();
        // DID2 was never completely listed for FK
        let mut r2 = RepoRow::new("https://pds2.example");
        r2.colls = vec![coll("post")];
        r2.apply_stable_pass("3l2a", content("x", 0).1, [(FK2, BTreeMap::new())]);
        b.put_repo(DID2, &r2).unwrap();
        b.commit().await.unwrap();

        let subjects = [
            &stored,
            &tombed,
            &missing,
            &below_cap,
            &unlisted_repo,
            &other_fk,
            &not_ranged,
            &no_coll,
            &capped_comment,
            &"at://handle.example/x/y".to_owned(),
        ];
        let absent = store
            .absent(FK, subjects.iter().map(|s| s.as_str()))
            .await
            .unwrap();
        let want: BTreeSet<String> = [tombed.clone(), missing.clone(), no_coll.clone()].into();
        assert_eq!(absent, want);
    });
}

#[test]
fn obs_map_has_both_entries() {
    let u = post_uri(8);
    let (s, _) = admit(
        Slot::Empty,
        version_with(
            &u,
            1,
            OrderKey::new("3l2a", 1),
            V {
                first_seen: 100,
                ..V::default()
            },
        ),
    );
    let (s, _) = admit(
        s,
        version_with(
            &u,
            2,
            OrderKey::new("3l2b", 2),
            V {
                first_seen: 300,
                ..V::default()
            },
        ),
    );
    let sr = StoredRecord {
        key: RecKey::of_uri(&u),
        row: rec(&s).clone(),
    };
    let m = obs_map([&sr]);
    let c = coll("post");
    assert_eq!(
        m,
        BTreeMap::from([
            ((u.to_string(), content(&c, 1).1.to_string()), 100),
            ((u.to_string(), content(&c, 2).1.to_string()), 300),
        ])
    );
    // with a single version both entries coincide
    let (s1, _) = admit(Slot::Empty, version(&u, 1, "3l2a", 1));
    let sr1 = StoredRecord {
        key: RecKey::of_uri(&u),
        row: rec(&s1).clone(),
    };
    assert_eq!(obs_map([&sr1]).len(), 1);
}

#[test]
fn note_first_seen_and_gc() {
    let kv = MemKv::new();
    let u = post_uri(9);
    let w = post_uri(10);
    put(&kv, vec![version(&u, 1, "3l2a", 10)]);
    block_on(async {
        let store = RecordStore::new(&kv);
        let cid = content(&coll("post"), 1).1;
        let mut b = store.batch();
        assert!(b.note_first_seen(&u, &cid, 5).await.unwrap());
        assert!(
            !b.note_first_seen(&u, &cid, 50).await.unwrap(),
            "never moved later"
        );
        assert!(
            !b.note_first_seen(&w, &cid, 1).await.unwrap(),
            "unknown uri: kept in memory by the caller"
        );
        b.tombstone(
            &w,
            Delete {
                at_ms: 1_000,
                ..del("3l2a", 10)
            },
        )
        .await
        .unwrap();
        b.commit().await.unwrap();
        let r = store.get(&u).await.unwrap().unwrap();
        assert_eq!((r.first_seen, r.first_seen_uri), (5, 5));
        // tombstones live at least 30 days, whatever retention is asked for
        let day = 24 * 60 * 60 * 1000;
        assert_eq!(store.gc_tombstones(1_000 + 29 * day, 0).await.unwrap(), 0);
        assert_eq!(
            store
                .gc_tombstones(1_000 + 31 * day, 40 * day)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .gc_tombstones(1_000 + 41 * day, 40 * day)
                .await
                .unwrap(),
            1
        );
        assert!(store.tombstone_of(&w).await.unwrap().is_none());
        assert!(store.get(&u).await.unwrap().is_some());
    });
}

#[test]
fn side_rows() {
    let kv = MemKv::new();
    block_on(async {
        let store = RecordStore::new(&kv);
        let mut b = store.batch();
        assert!(b.repo(DID).await.unwrap().is_none());
        let mut r = RepoRow::new("https://pds.example");
        r.fails = 2;
        b.put_repo(DID, &r).unwrap();
        assert_eq!(b.repo(DID).await.unwrap(), Some(r.clone()));
        let mut h = HostRow::default();
        h.set_range_no(5);
        b.put_host_caps("https://pds.example", &h).unwrap();
        assert_eq!(
            b.host_caps("https://pds.example").await.unwrap(),
            Some(h.clone())
        );
        assert_eq!(b.adjust_blob_refs("bafk", 1).await.unwrap(), None);
        b.put_blob("bafk", &BlobRow::new(DID, "image/png", 3, 1))
            .unwrap();
        assert_eq!(b.adjust_blob_refs("bafk", 2).await.unwrap(), Some(2));
        assert_eq!(b.adjust_blob_refs("bafk", -5).await.unwrap(), Some(0));
        b.put_cursor("jetstream", &CursorRow::new(4)).unwrap();
        b.put_forum(FK, &ForumRow::new("at://x/y/z", 9)).unwrap();
        b.commit().await.unwrap();
        assert_eq!(store.repo(DID).await.unwrap(), Some(r));
        assert_eq!(store.repos(&[DID, DID2]).await.unwrap()[1], None);
        assert_eq!(
            store.host_caps("https://pds.example").await.unwrap(),
            Some(h)
        );
        assert_eq!(store.blob("bafk").await.unwrap().unwrap().refs, 0);
        assert_eq!(store.cursor("jetstream").await.unwrap().unwrap().at, 4);
        assert_eq!(store.forum(FK).await.unwrap().unwrap().last_viewed, 9);
        // rows of an unknown version read as absent
        kv.write(vec![KvOp::Put(
            schema::repo_key(DID2),
            row::encode(&RepoRow {
                v: ROW_VERSION + 1,
                ..RepoRow::new("x")
            })
            .unwrap(),
        )])
        .await
        .unwrap();
        assert_eq!(store.repo(DID2).await.unwrap(), None);
    });
}

#[test]
fn eligibility_and_fold_rows() {
    use crate::store::repo::AccountStatus;
    let u = post_uri(11);
    let (s, _) = admit(Slot::Empty, version(&u, 1, "3l2a", 1));
    let mut row = rec(&s).clone();
    let mut repo = RepoRow::new("https://pds.example");
    assert!(eligible(&row, None));
    assert!(eligible(&row, Some(&repo)));
    repo.status = AccountStatus::Temporary;
    assert!(eligible(&row, Some(&repo)));
    repo.status = AccountStatus::Inactive;
    assert!(!eligible(&row, Some(&repo)));
    row.id_hint = true;
    assert!(!eligible(&row, None));

    // caps per (did, collection), descending rkey; the membership row; nothing else
    let kv = MemKv::new();
    let mut vs = Vec::new();
    for (i, k) in ["a", "b", "c", "d"].iter().enumerate() {
        vs.push(version(
            &uri_in(DID, "post", &format!("{FK}-{k}")),
            i as u64,
            "3l2a",
            1,
        ));
        vs.push(version(
            &uri_in(DID2, "post", &format!("{FK}-{k}")),
            10 + i as u64,
            "3l2a",
            1,
        ));
    }
    vs.push(version(
        &uri_in(DID, "comment", &format!("{FK}-a-b")),
        20,
        "3l2a",
        1,
    ));
    vs.push(version(&uri_in(DID, "membership", FK), 21, "3l2a", 1));
    // in the fk scope but outside the range: a ranged key equal to the fk
    vs.push(version(&uri_in(DID, "post", FK), 22, "3l2a", 1));
    put(&kv, vs);
    let rows = block_on(RecordStore::new(&kv).scan_forum(FK, None).collect()).unwrap();
    assert_eq!(rows.len(), 11);
    let kept = fold_rows(
        FK,
        rows.clone(),
        |c| if c == coll("post") { 2 } else { 100 },
    );
    let names: Vec<String> = kept
        .iter()
        .map(|r| {
            format!(
                "{}:{}:{}",
                &r.key.collection[NS.len() + 1..],
                &r.key.did[8..10],
                r.key.rkey
            )
        })
        .collect();
    assert_eq!(
        names,
        vec![
            format!("comment:ew:{FK}-a-b"),
            format!("membership:ew:{FK}"),
            format!("post:ab:{FK}-c"),
            format!("post:ab:{FK}-d"),
            format!("post:ew:{FK}-c"),
            format!("post:ew:{FK}-d"),
        ]
    );
    assert_eq!(fold_rows_capped(FK, rows).len(), 10);
}

// ---------------------------------------------------------------------------------------------
// Property tests

/// One generated observation of a uri.
#[derive(Clone, Debug)]
enum Ev {
    Put {
        uri: usize,
        content: u64,
        rev: u32,
        sent: i64,
        exact: bool,
        level: Level,
        src: u8,
        id_hint: bool,
        first_seen: i64,
    },
    Del {
        uri: usize,
        rev: u32,
        sent: i64,
        ev: DeleteEvidence,
    },
}

const URIS: usize = 3;

fn uris() -> Vec<AtUri> {
    (0..URIS as u32).map(|i| post_uri(20 + i)).collect()
}

fn rev_str(r: u32) -> String {
    if r == 0 {
        String::new()
    } else {
        format!("3l{r:04}")
    }
}

fn src_of(s: u8) -> Source {
    match s % 4 {
        0 => Source::Get,
        1 => Source::List,
        2 => Source::Proof,
        _ => Source::Own,
    }
}

fn to_version(us: &[AtUri], e: &Ev) -> Option<Version> {
    let Ev::Put {
        uri,
        content: c,
        rev,
        sent,
        exact,
        level,
        src,
        id_hint,
        first_seen,
    } = e
    else {
        return None;
    };
    Some(version_with(
        &us[*uri],
        *c,
        OrderKey::new(rev_str(*rev), *sent),
        V {
            level: *level,
            exact: *exact,
            src: src_of(*src),
            id_hint: *id_hint,
            first_seen: *first_seen,
        },
    ))
}

fn to_delete(e: &Ev) -> Option<(usize, Delete)> {
    let Ev::Del { uri, rev, sent, ev } = e else {
        return None;
    };
    Some((
        *uri,
        Delete {
            key: OrderKey::new(rev_str(*rev), *sent),
            evidence: *ev,
            at_ms: *sent,
        },
    ))
}

/// Applies events with the pure rules; returns the final slots.
fn run_pure(events: &[Ev]) -> Vec<Slot> {
    let us = uris();
    let mut slots = vec![Slot::Empty; URIS];
    for e in events {
        match e {
            Ev::Put { uri, .. } => {
                let s = std::mem::replace(&mut slots[*uri], Slot::Empty);
                slots[*uri] = admit_version(s, to_version(&us, e).unwrap()).0;
            }
            Ev::Del { .. } => {
                let (u, d) = to_delete(e).unwrap();
                let s = std::mem::replace(&mut slots[u], Slot::Empty);
                slots[u] = apply_delete(s, &d).0;
            }
        }
    }
    slots
}

/// An honest repo: each uri has a history over revs 1..=R in which every change creates a new
/// content id (no reverts) or deletes the record. Observations read that history:
/// - exact ones (listings, proofs, own writes, complete-listing deletes) at the rev they saw;
/// - with `inexact`, also point fetches and `RecordNotFound` whose rev is a lower bound.
///
/// Send times grow with the rev seen, and are unique.
fn honest_events(inexact: bool) -> impl Strategy<Value = Vec<Ev>> {
    const R: u32 = 12;
    let history = proptest::collection::vec(
        // per uri: for each rev, keep / new content / delete
        proptest::collection::vec(0u8..6, R as usize),
        URIS,
    );
    let obs = proptest::collection::vec(
        (
            0..URIS,
            1..=R,
            any::<bool>(),
            0..R,
            any::<u8>(),
            any::<bool>(),
            any::<bool>(),
            0i64..5_000,
        ),
        1..40,
    );
    (history, obs).prop_map(move |(history, obs)| {
        // state[u][r]: Some(content) or None (absent); content ids are unique per change
        let mut next_id = 0u64;
        let states: Vec<Vec<Option<u64>>> = history
            .iter()
            .map(|changes| {
                let mut cur = None;
                let mut out = vec![None];
                for &c in changes {
                    match c {
                        0 | 1 => {
                            next_id += 1;
                            cur = Some(next_id);
                        }
                        2 => cur = None,
                        _ => {}
                    }
                    out.push(cur);
                }
                out
            })
            .collect();
        obs.into_iter()
            .enumerate()
            .map(|(i, (u, r, exact, back, src, proven, id_hint, fs))| {
                let exact = exact || !inexact;
                // unique send time, growing with the rev actually seen
                let sent = i64::from(r) * 1_000 + i as i64;
                let rev_lo = if exact { r } else { r.saturating_sub(back) };
                match states[u][r as usize] {
                    Some(c) => Ev::Put {
                        uri: u,
                        content: c,
                        rev: rev_lo,
                        sent,
                        exact,
                        level: if proven && exact {
                            Level::Proven
                        } else {
                            Level::Pds
                        },
                        src,
                        id_hint,
                        first_seen: sent * 1_000 - fs,
                    },
                    None => Ev::Del {
                        uri: u,
                        rev: rev_lo,
                        sent,
                        ev: if exact {
                            DeleteEvidence::CompleteListing
                        } else {
                            DeleteEvidence::RecordNotFound
                        },
                    },
                }
            })
            .collect()
    })
}

/// Arbitrary (possibly dishonest) observations: conflicting CIDs at the same rev, equal keys,
/// keys out of order.
fn arbitrary_events() -> impl Strategy<Value = Vec<Ev>> {
    let put = (
        0..URIS,
        0u64..4,
        0u32..5,
        0i64..6,
        any::<bool>(),
        any::<bool>(),
        any::<u8>(),
        any::<bool>(),
        0i64..100,
    )
        .prop_map(
            |(uri, content, rev, sent, exact, proven, src, id_hint, first_seen)| Ev::Put {
                uri,
                content,
                rev,
                sent,
                exact,
                level: if proven { Level::Proven } else { Level::Pds },
                src,
                id_hint,
                first_seen,
            },
        );
    let del = (0..URIS, 0u32..5, 0i64..6, any::<bool>()).prop_map(|(uri, rev, sent, nf)| Ev::Del {
        uri,
        rev,
        sent,
        ev: if nf {
            DeleteEvidence::RecordNotFound
        } else {
            DeleteEvidence::CompleteListing
        },
    });
    proptest::collection::vec(prop_oneof![3 => put, 1 => del], 0..40)
}

/// Presence (0 empty, 1 row, 2 tombstone), CID, key, the uri's first-seen.
type Projection = (u8, Option<Cid>, Option<OrderKey>, Option<i64>);

/// What is order-independent in any honest world.
fn projection(slots: &[Slot]) -> Vec<Projection> {
    slots
        .iter()
        .map(|s| {
            let kind = match s {
                Slot::Empty => 0,
                Slot::Rec(_) => 1,
                Slot::Tomb(_) => 2,
            };
            (kind, s.cid(), s.key(), s.first_seen_uri())
        })
        .collect()
}

/// Runs events through `Batch`es over `MemKv`, committing after the events whose index is in
/// `commit_at`; returns the final slots read back from the store.
fn run_store(events: &[Ev], commit_at: &BTreeSet<usize>) -> (MemKv, Vec<Slot>) {
    let us = uris();
    let kv = MemKv::new();
    block_on(async {
        let store = RecordStore::new(&kv);
        let mut b = store.batch();
        for (i, e) in events.iter().enumerate() {
            match e {
                Ev::Put { .. } => {
                    b.put_version(to_version(&us, e).unwrap()).await.unwrap();
                }
                Ev::Del { .. } => {
                    let (u, d) = to_delete(e).unwrap();
                    b.tombstone(&us[u], d).await.unwrap();
                }
            }
            if commit_at.contains(&i) {
                b.commit().await.unwrap();
                b = store.batch();
            }
        }
        b.commit().await.unwrap();
        let mut slots = Vec::new();
        for u in &us {
            slots.push(store.slot(&RecKey::of_uri(u)).await.unwrap());
        }
        (kv.clone(), slots)
    })
}

/// Structural store invariants: every key decodes into its own uri's position, a uri never has
/// both a row and a tombstone, every row hashes to its CID (I2).
fn check_store(kv: &MemKv) -> Result<(), TestCaseError> {
    let all = block_on(kv.scan(KeyRange::prefix(b""), Dir::Forward, 10_000)).unwrap();
    let mut recs = BTreeSet::new();
    let mut tombs = BTreeSet::new();
    for (k, v) in all {
        if let Some(rk) = RecKey::parse(&k, tag::REC) {
            let r: RecordRow = row::decode(&v).unwrap();
            prop_assert!(lessdugin_atp::cid::verify(&r.cid, &r.cbor).is_ok());
            prop_assert_eq!(&rk, &RecKey::new(&rk.collection, &rk.did, &rk.rkey));
            prop_assert!(r.first_seen_uri <= r.first_seen);
            recs.insert(rk.uri());
        } else if let Some(rk) = RecKey::parse(&k, tag::TOMB) {
            let _: TombRow = row::decode(&v).unwrap();
            tombs.insert(rk.uri());
        } else {
            prop_assert!(false, "unexpected key {:?}", k);
        }
    }
    prop_assert!(recs.is_disjoint(&tombs));
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// Exact observations of an honest repo: the final store is the same whatever the arrival
    /// order, down to merged levels, sources and first-seen times.
    #[test]
    fn exact_observations_commute(
        (events, shuffled) in honest_events(false).prop_flat_map(|e| (Just(e.clone()), Just(e).prop_shuffle()))
    ) {
        prop_assert_eq!(run_pure(&events), run_pure(&shuffled));
    }

    /// With point fetches and `RecordNotFound` (lower-bound keys), the current version, its key
    /// and the uri's first-seen time still do not depend on arrival order, and the result is the
    /// observation with the greatest key (a version wins an equal-key tie with a tombstone).
    #[test]
    fn honest_observations_commute(
        (events, shuffled) in honest_events(true).prop_flat_map(|e| (Just(e.clone()), Just(e).prop_shuffle()))
    ) {
        let a = run_pure(&events);
        prop_assert_eq!(projection(&a), projection(&run_pure(&shuffled)));
        for (u, slot) in a.iter().enumerate() {
            let best = events
                .iter()
                .filter_map(|e| match e {
                    Ev::Put { uri, rev, sent, .. } if *uri == u => Some((OrderKey::new(rev_str(*rev), *sent), 1)),
                    Ev::Del { uri, rev, sent, .. } if *uri == u => Some((OrderKey::new(rev_str(*rev), *sent), 0)),
                    _ => None,
                })
                .max();
            prop_assert_eq!(slot.key(), best.as_ref().map(|b| b.0.clone()));
            prop_assert_eq!(matches!(slot, Slot::Rec(_)), best.is_some_and(|b| b.1 == 1));
            let first = events
                .iter()
                .filter_map(|e| match e {
                    Ev::Put { uri, first_seen, .. } if *uri == u => Some(*first_seen),
                    _ => None,
                })
                .min();
            prop_assert_eq!(slot.first_seen_uri(), first);
        }
    }

    /// Any observations at all: the per-uri key never decreases (I3), a tombstone suppresses
    /// smaller keys (I4), the level of a current (uri, cid) never decreases (I5), first-seen
    /// times never move later, and the store agrees with the pure rules whatever the batching.
    #[test]
    fn invariants_hold_for_arbitrary_observations(
        events in arbitrary_events(),
        commit_at in proptest::collection::btree_set(0usize..40, 0..10),
    ) {
        let us = uris();
        let mut slots = vec![Slot::Empty; URIS];
        for e in &events {
            let u = match e { Ev::Put { uri, .. } | Ev::Del { uri, .. } => *uri };
            let before = slots[u].clone();
            let after = match e {
                Ev::Put { .. } => {
                    let v = to_version(&us, e).unwrap();
                    let (cid, key) = (v.cid, v.key.clone());
                    let (s, a, changed) = admit_version(before.clone(), v);
                    prop_assert_eq!(changed, s != before);
                    prop_assert_eq!(a.is_current(), s.cid() == Some(cid));
                    if let Slot::Tomb(t) = &before
                        && key < t.key()
                    {
                        prop_assert_eq!(&a, &Admission::Suppressed);
                    }
                    if let (Slot::Rec(b), Slot::Rec(r)) = (&before, &s)
                        && b.cid == r.cid
                    {
                        prop_assert!(r.level >= b.level);
                    }
                    if let Admission::Conflict(c) = &a {
                        // the kept version has the higher rank
                        let r = rec(&s);
                        prop_assert!(r.level >= rec(&before).level);
                        prop_assert_eq!(c.replaced, r.cid == cid);
                    }
                    s
                }
                Ev::Del { .. } => {
                    let (_, d) = to_delete(e).unwrap();
                    let (s, _, changed) = apply_delete(before.clone(), &d);
                    prop_assert_eq!(changed, s != before);
                    s
                }
            };
            prop_assert!(after.key() >= before.key());
            if let (Some(b), Some(a)) = (before.first_seen_uri(), after.first_seen_uri()) {
                prop_assert!(a <= b);
            }
            slots[u] = after;
        }
        prop_assert_eq!(&run_pure(&events), &slots);
        let (kv, stored) = run_store(&events, &commit_at);
        prop_assert_eq!(&stored, &slots);
        check_store(&kv)?;
    }

    /// The store matches the pure model for honest histories, too.
    #[test]
    fn store_matches_model_on_honest_histories(
        events in honest_events(true),
        commit_at in proptest::collection::btree_set(0usize..40, 0..10),
    ) {
        let (kv, stored) = run_store(&events, &commit_at);
        prop_assert_eq!(stored, run_pure(&events));
        check_store(&kv)?;
    }
}
