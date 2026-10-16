// SPDX-License-Identifier: AGPL-3.0-or-later
//! The shared [`KvStore`] contract tests (`sync.md` §12.4). Every store implementation (MemKv,
//! SqliteKv, IndexedDB) runs [`kv_conformance`] on a fresh, empty store; it panics with a
//! description of the first violation.

use std::collections::BTreeMap;
use std::ops::Bound;

use lessdugin_core::host::{Dir, KeyRange, KvOp, KvStore};

/// Every key written by the suite starts with this prefix.
const P: &[u8] = b"\x02kvc\x00";

fn k(suffix: &[u8]) -> Vec<u8> {
    let mut v = P.to_vec();
    v.extend_from_slice(suffix);
    v
}

fn all() -> KeyRange {
    KeyRange::prefix(P)
}

async fn scan_all<K: KvStore>(kv: &K, dir: Dir) -> Vec<(Vec<u8>, Vec<u8>)> {
    kv.scan(all(), dir, 10_000).await.expect("scan")
}

async fn clear<K: KvStore>(kv: &K) {
    kv.write(vec![KvOp::DeleteRange(all())])
        .await
        .expect("clear");
    assert!(
        scan_all(kv, Dir::Forward).await.is_empty(),
        "DeleteRange(prefix) left keys behind"
    );
}

/// Runs the contract on `kv` (which should start empty). Panics on the first violation.
pub async fn kv_conformance<K: KvStore>(kv: &K) {
    get_many_contract(kv).await;
    ordering_contract(kv).await;
    scan_contract(kv).await;
    batch_contract(kv).await;
    size_contract(kv).await;
    model_check(kv).await;
    clear(kv).await;
}

async fn get_many_contract<K: KvStore>(kv: &K) {
    clear(kv).await;
    assert_eq!(
        kv.get_many(vec![]).await.expect("get_many"),
        Vec::<Option<Vec<u8>>>::new()
    );
    kv.write(vec![
        KvOp::Put(k(b"a"), b"1".to_vec()),
        KvOp::Put(k(b"b"), Vec::new()),
    ])
    .await
    .expect("write");
    let got = kv
        .get_many(vec![k(b"b"), k(b"missing"), k(b"a"), k(b"b")])
        .await
        .expect("get_many");
    assert_eq!(
        got,
        vec![
            Some(Vec::new()),
            None,
            Some(b"1".to_vec()),
            Some(Vec::new())
        ],
        "get_many must return one entry per key, in request order (empty values included)"
    );
    kv.write(vec![KvOp::Put(k(b"a"), b"2".to_vec())])
        .await
        .expect("write");
    assert_eq!(
        kv.get_many(vec![k(b"a")]).await.expect("get_many"),
        vec![Some(b"2".to_vec())],
        "Put must replace an existing value"
    );
}

async fn ordering_contract<K: KvStore>(kv: &K) {
    clear(kv).await;
    // Keys compare as unsigned bytes: 0x00 < 0x01 < 0x7f < 0x80 < 0xff, and a prefix sorts first.
    let suffixes: [&[u8]; 7] = [
        b"\xff",
        b"\x80",
        b"\x7f",
        b"\x00",
        b"",
        b"\x00\x00",
        b"\x01",
    ];
    let ops = suffixes
        .iter()
        .map(|s| KvOp::Put(k(s), s.to_vec()))
        .collect();
    kv.write(ops).await.expect("write");
    let fwd: Vec<Vec<u8>> = scan_all(kv, Dir::Forward)
        .await
        .into_iter()
        .map(|(_, v)| v)
        .collect();
    let expected: Vec<Vec<u8>> = [
        &b""[..],
        b"\x00",
        b"\x00\x00",
        b"\x01",
        b"\x7f",
        b"\x80",
        b"\xff",
    ]
    .iter()
    .map(|s| s.to_vec())
    .collect();
    assert_eq!(fwd, expected, "keys must sort as unsigned bytes");
    let mut rev: Vec<Vec<u8>> = scan_all(kv, Dir::Reverse)
        .await
        .into_iter()
        .map(|(_, v)| v)
        .collect();
    rev.reverse();
    assert_eq!(
        rev, expected,
        "Reverse scans must return descending key order"
    );
}

async fn scan_contract<K: KvStore>(kv: &K) {
    clear(kv).await;
    let ops = (0u8..20)
        .map(|i| KvOp::Put(k(&[b'a' + i]), vec![i]))
        .collect();
    kv.write(ops).await.expect("write");
    let r = |s: Bound<Vec<u8>>, e: Bound<Vec<u8>>| KeyRange { start: s, end: e };
    let vals = |v: Vec<(Vec<u8>, Vec<u8>)>| v.into_iter().map(|(_, v)| v[0]).collect::<Vec<u8>>();

    let got = kv.scan(
        r(Bound::Included(k(b"c")), Bound::Excluded(k(b"f"))),
        Dir::Forward,
        100,
    );
    assert_eq!(
        vals(got.await.expect("scan")),
        vec![2, 3, 4],
        "[start, end) forward"
    );
    let got = kv.scan(
        r(Bound::Excluded(k(b"c")), Bound::Included(k(b"f"))),
        Dir::Forward,
        100,
    );
    assert_eq!(
        vals(got.await.expect("scan")),
        vec![3, 4, 5],
        "(start, end] forward"
    );
    let got = kv.scan(
        r(Bound::Included(k(b"c")), Bound::Excluded(k(b"f"))),
        Dir::Reverse,
        100,
    );
    assert_eq!(
        vals(got.await.expect("scan")),
        vec![4, 3, 2],
        "[start, end) reverse"
    );
    let got = kv.scan(all(), Dir::Forward, 3);
    assert_eq!(
        vals(got.await.expect("scan")),
        vec![0, 1, 2],
        "limit applies from the start"
    );
    let got = kv.scan(all(), Dir::Reverse, 2);
    assert_eq!(
        vals(got.await.expect("scan")),
        vec![19, 18],
        "limit applies from the end"
    );
    let got = kv.scan(all(), Dir::Forward, 0);
    assert!(
        got.await.expect("scan").is_empty(),
        "limit 0 returns nothing"
    );
    // Paging by continuing from the last key.
    let mut seen = Vec::new();
    let mut start = Bound::Included(P.to_vec());
    loop {
        let page = kv
            .scan(r(start.clone(), all().end), Dir::Forward, 7)
            .await
            .expect("scan");
        let Some((last, _)) = page.last().cloned() else {
            break;
        };
        seen.extend(vals(page));
        start = Bound::Excluded(last);
    }
    assert_eq!(
        seen,
        (0u8..20).collect::<Vec<_>>(),
        "paging must visit every key once"
    );
    // Unbounded ends.
    let got = kv.scan(
        r(Bound::Included(k(b"s")), Bound::Unbounded),
        Dir::Forward,
        100,
    );
    assert_eq!(
        vals(got.await.expect("scan")),
        vec![18, 19],
        "unbounded end"
    );
    // Empty and inverted ranges select nothing.
    let got = kv.scan(
        r(Bound::Included(k(b"f")), Bound::Excluded(k(b"c"))),
        Dir::Forward,
        100,
    );
    assert!(
        got.await.expect("scan").is_empty(),
        "an inverted range selects nothing"
    );
    let got = kv.scan(
        r(Bound::Excluded(k(b"c")), Bound::Excluded(k(b"c"))),
        Dir::Reverse,
        100,
    );
    assert!(
        got.await.expect("scan").is_empty(),
        "an empty range selects nothing"
    );
}

async fn batch_contract<K: KvStore>(kv: &K) {
    clear(kv).await;
    // Ops apply in order within one batch.
    kv.write(vec![
        KvOp::Put(k(b"x"), b"1".to_vec()),
        KvOp::Delete(k(b"x")),
        KvOp::Delete(k(b"y")),
        KvOp::Put(k(b"y"), b"2".to_vec()),
        KvOp::Put(k(b"m1"), b"3".to_vec()),
        KvOp::Put(k(b"m2"), b"4".to_vec()),
        KvOp::DeleteRange(KeyRange::prefix(&k(b"m"))),
        KvOp::Put(k(b"m3"), b"5".to_vec()),
        KvOp::Put(k(b"z"), b"6".to_vec()),
        KvOp::Put(k(b"z"), b"7".to_vec()),
    ])
    .await
    .expect("write");
    let got = kv
        .get_many(vec![
            k(b"x"),
            k(b"y"),
            k(b"m1"),
            k(b"m2"),
            k(b"m3"),
            k(b"z"),
        ])
        .await
        .expect("get_many");
    assert_eq!(
        got,
        vec![
            None,
            Some(b"2".to_vec()),
            None,
            None,
            Some(b"5".to_vec()),
            Some(b"7".to_vec())
        ],
        "batch ops must apply in order"
    );
    // Deleting absent keys and empty ranges is fine.
    kv.write(vec![
        KvOp::Delete(k(b"absent")),
        KvOp::DeleteRange(KeyRange::prefix(&k(b"nothing-here"))),
        KvOp::DeleteRange(KeyRange {
            start: Bound::Included(k(b"z")),
            end: Bound::Excluded(k(b"a")),
        }),
    ])
    .await
    .expect("write");
    assert_eq!(
        scan_all(kv, Dir::Forward).await.len(),
        3,
        "no-op deletes must not remove keys"
    );
    // An empty batch is fine.
    kv.write(Vec::new()).await.expect("empty batch");
    // DeleteRange with bounds removes exactly the range.
    kv.write(vec![KvOp::DeleteRange(KeyRange {
        start: Bound::Excluded(k(b"m3")),
        end: Bound::Included(k(b"z")),
    })])
    .await
    .expect("write");
    let keys: Vec<Vec<u8>> = scan_all(kv, Dir::Forward)
        .await
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(
        keys,
        vec![k(b"m3")],
        "DeleteRange(m3, z] must remove y and z only"
    );
}

async fn size_contract<K: KvStore>(kv: &K) {
    clear(kv).await;
    // Keys up to 2,048 bytes and values up to 2 MiB.
    let long_key = k(&vec![0xAB; 2048 - P.len()]);
    let big = (0..2 * 1024 * 1024)
        .map(|i| (i % 251) as u8)
        .collect::<Vec<u8>>();
    kv.write(vec![KvOp::Put(long_key.clone(), big.clone())])
        .await
        .expect("write big");
    let got = kv.get_many(vec![long_key]).await.expect("get_many");
    assert_eq!(
        got[0].as_deref(),
        Some(big.as_slice()),
        "a 2 MiB value under a 2 KiB key"
    );
}

/// Applies a deterministic pseudo-random sequence of batches to `kv` and to a `BTreeMap`, and
/// compares full scans in both directions after each batch.
async fn model_check<K: KvStore>(kv: &K) {
    clear(kv).await;
    let mut model: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for round in 0..60 {
        let mut batch = Vec::new();
        for _ in 0..(next() % 6 + 1) {
            let key = k(&[b'a' + (next() % 8) as u8, b'a' + (next() % 8) as u8]);
            match next() % 5 {
                0..=2 => batch.push(KvOp::Put(key, vec![round as u8, (next() % 256) as u8])),
                3 => batch.push(KvOp::Delete(key)),
                _ => batch.push(KvOp::DeleteRange(KeyRange::prefix(&key[..key.len() - 1]))),
            }
        }
        for op in &batch {
            match op {
                KvOp::Put(k, v) => {
                    model.insert(k.clone(), v.clone());
                }
                KvOp::Delete(k) => {
                    model.remove(k);
                }
                KvOp::DeleteRange(r) => model.retain(|k, _| !r.contains(k)),
            }
        }
        kv.write(batch).await.expect("write");
        let fwd = scan_all(kv, Dir::Forward).await;
        let expected: Vec<(Vec<u8>, Vec<u8>)> = model.clone().into_iter().collect();
        assert_eq!(
            fwd, expected,
            "store diverged from the model after round {round}"
        );
        let mut rev = scan_all(kv, Dir::Reverse).await;
        rev.reverse();
        assert_eq!(rev, expected, "reverse scan diverged after round {round}");
    }
}
