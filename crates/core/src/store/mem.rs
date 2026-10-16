// SPDX-License-Identifier: AGPL-3.0-or-later
//! [`MemKv`]: an in-memory [`KvStore`] (tests, ephemeral sessions, and the browser before
//! IndexedDB opens).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::host::{Dir, HostError, KeyRange, KvOp, KvStore};

/// An in-memory ordered store. Cloning shares the same data.
#[derive(Clone, Debug, Default)]
pub struct MemKv {
    inner: Arc<Mutex<BTreeMap<Vec<u8>, Vec<u8>>>>,
}

impl MemKv {
    /// A new empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of keys (tests).
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// True if empty.
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<Vec<u8>, Vec<u8>>> {
        // A poisoned lock only means a panic elsewhere in a test; the map itself is consistent
        // because every mutation below is applied fully or not at all.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl KvStore for MemKv {
    async fn get_many(&self, keys: Vec<Vec<u8>>) -> Result<Vec<Option<Vec<u8>>>, HostError> {
        let map = self.lock();
        Ok(keys.iter().map(|k| map.get(k).cloned()).collect())
    }

    async fn scan(
        &self,
        range: KeyRange,
        dir: Dir,
        limit: u32,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>, HostError> {
        let map = self.lock();
        let iter = map.range((range.start.clone(), range.end.clone()));
        let take = limit as usize;
        Ok(match dir {
            Dir::Forward => iter.take(take).map(|(k, v)| (k.clone(), v.clone())).collect(),
            Dir::Reverse => iter.rev().take(take).map(|(k, v)| (k.clone(), v.clone())).collect(),
        })
    }

    async fn write(&self, batch: Vec<KvOp>) -> Result<(), HostError> {
        let mut map = self.lock();
        for op in batch {
            match op {
                KvOp::Put(k, v) => {
                    map.insert(k, v);
                }
                KvOp::Delete(k) => {
                    map.remove(&k);
                }
                KvOp::DeleteRange(r) => {
                    let doomed: Vec<Vec<u8>> =
                        map.range((r.start, r.end)).map(|(k, _)| k.clone()).collect();
                    for k in doomed {
                        map.remove(&k);
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;

    #[test]
    fn put_scan_delete() {
        let kv = MemKv::new();
        block_on(async {
            kv.write(vec![
                KvOp::Put(b"a1".to_vec(), b"x".to_vec()),
                KvOp::Put(b"a2".to_vec(), b"y".to_vec()),
                KvOp::Put(b"b1".to_vec(), b"z".to_vec()),
            ])
            .await
            .unwrap();
            let fwd = kv.scan(KeyRange::prefix(b"a"), Dir::Forward, 10).await.unwrap();
            assert_eq!(fwd.len(), 2);
            let rev = kv.scan(KeyRange::prefix(b"a"), Dir::Reverse, 1).await.unwrap();
            assert_eq!(rev[0].0, b"a2");
            kv.write(vec![KvOp::DeleteRange(KeyRange::prefix(b"a"))]).await.unwrap();
            assert_eq!(kv.len(), 1);
            let got = kv.get_many(vec![b"b1".to_vec(), b"zz".to_vec()]).await.unwrap();
            assert_eq!(got, vec![Some(b"z".to_vec()), None]);
        });
    }
}
