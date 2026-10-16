// SPDX-License-Identifier: AGPL-3.0-or-later
//! SqliteKv: the KvStore implementation on SQLite (WAL).
//!
//! One table per store file, `kv(k BLOB PRIMARY KEY, v BLOB) WITHOUT ROWID` (`sync.md` §12.4).
//! SQLite compares BLOBs with `memcmp` and then by length, which is exactly the unsigned
//! lexicographic order of the store contract.
//!
//! - WAL mode, `busy_timeout` 5 s; `synchronous=FULL` for `state` ([`Durability::Full`]) and
//!   `NORMAL` for the disposable `cache` ([`Durability::Normal`]).
//! - Each batch is one `BEGIN IMMEDIATE` transaction, applied in order.
//! - SQLite calls are blocking: they run on `tokio::task::spawn_blocking` (inline when there is
//!   no Tokio runtime). One writer connection plus a small pool of reader connections; WAL lets
//!   readers run beside the writer, also across processes.
//! - New database files are created with mode 0600 in a 0700 directory, never through a
//!   symlink (`SQLITE_OPEN_NOFOLLOW`).

use std::ops::Bound;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use lessdugin_core::host::{Dir, HostError, KeyRange, KvOp, KvStore};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params_from_iter};

use crate::fsutil;

/// How hard SQLite works to make a committed batch survive a power loss.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Durability {
    /// `synchronous=FULL`: durable when `write` resolves (the `state` store).
    Full,
    /// `synchronous=NORMAL`: may lose the last batches on power loss, never corrupts (`cache`).
    Normal,
}

/// Busy timeout for lock contention between connections and processes.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// Idle reader connections kept open.
const MAX_IDLE_READERS: usize = 4;

/// A [`KvStore`] in one SQLite file. Cloning shares the connections.
#[derive(Clone)]
pub struct SqliteKv {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for SqliteKv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteKv")
            .field("path", &self.inner.path)
            .field("durability", &self.inner.durability)
            .finish()
    }
}

struct Inner {
    /// `None` for an in-memory store (one connection serves reads too).
    path: Option<PathBuf>,
    durability: Durability,
    writer: Mutex<Connection>,
    readers: Mutex<Vec<Connection>>,
}

fn storage(e: impl std::fmt::Display) -> HostError {
    HostError::Storage(e.to_string())
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic while holding a connection leaves SQLite itself consistent (an open transaction
    // is rolled back when its `Transaction` drops), so a poisoned lock is still usable.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl SqliteKv {
    /// Opens (creating if needed) the store at `path`.
    pub fn open(path: &Path, durability: Durability) -> Result<Self, HostError> {
        // Create the file ourselves so that it gets mode 0600 and is never a symlink; SQLite
        // gives the -wal and -shm files the same permissions as the database.
        fsutil::open_or_create_private(path)
            .map_err(|e| HostError::Storage(format!("{}: {e}", path.display())))?;
        let writer = open_conn(path, durability)?;
        writer
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS kv (k BLOB PRIMARY KEY NOT NULL, v BLOB NOT NULL) WITHOUT ROWID;",
            )
            .map_err(storage)?;
        Ok(Self {
            inner: Arc::new(Inner {
                path: Some(path.to_path_buf()),
                durability,
                writer: Mutex::new(writer),
                readers: Mutex::new(Vec::new()),
            }),
        })
    }

    /// A private in-memory store (tests, ephemeral sessions).
    pub fn open_in_memory() -> Result<Self, HostError> {
        let conn = Connection::open_in_memory().map_err(storage)?;
        conn.execute_batch(
            "CREATE TABLE kv (k BLOB PRIMARY KEY NOT NULL, v BLOB NOT NULL) WITHOUT ROWID;",
        )
        .map_err(storage)?;
        Ok(Self {
            inner: Arc::new(Inner {
                path: None,
                durability: Durability::Normal,
                writer: Mutex::new(conn),
                readers: Mutex::new(Vec::new()),
            }),
        })
    }

    /// The database file, if any.
    pub fn path(&self) -> Option<&Path> {
        self.inner.path.as_deref()
    }

    /// The durability this store was opened with.
    pub fn durability(&self) -> Durability {
        self.inner.durability
    }

    /// Runs `f` on a blocking thread when a Tokio runtime is available, inline otherwise.
    async fn run<T, F>(&self, f: F) -> Result<T, HostError>
    where
        T: Send + 'static,
        F: FnOnce(&Inner) -> Result<T, HostError> + Send + 'static,
    {
        let inner = Arc::clone(&self.inner);
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => rt
                .spawn_blocking(move || f(&inner))
                .await
                .map_err(|e| HostError::Storage(format!("storage task failed: {e}")))?,
            Err(_) => f(&inner),
        }
    }
}

fn open_conn(path: &Path, durability: Durability) -> Result<Connection, HostError> {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW
        | OpenFlags::SQLITE_OPEN_URI;
    let conn = Connection::open_with_flags(path, flags).map_err(storage)?;
    conn.busy_timeout(BUSY_TIMEOUT).map_err(storage)?;
    let mode: String = conn
        .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))
        .map_err(storage)?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(HostError::Storage(format!(
            "cannot enable WAL mode (got {mode})"
        )));
    }
    let sync = match durability {
        Durability::Full => "FULL",
        Durability::Normal => "NORMAL",
    };
    conn.pragma_update(None, "synchronous", sync)
        .map_err(storage)?;
    conn.set_prepared_statement_cache_capacity(64);
    Ok(conn)
}

impl Inner {
    /// Runs `f` with a reader connection (the writer for in-memory stores).
    fn with_reader<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T, HostError>,
    ) -> Result<T, HostError> {
        let Some(path) = &self.path else {
            return f(&mut lock(&self.writer));
        };
        let pooled = lock(&self.readers).pop();
        let mut conn = match pooled {
            Some(c) => c,
            None => open_conn(path, self.durability)?,
        };
        let out = f(&mut conn);
        let mut pool = lock(&self.readers);
        if pool.len() < MAX_IDLE_READERS {
            pool.push(conn);
        }
        out
    }

    fn get_many(&self, keys: Vec<Vec<u8>>) -> Result<Vec<Option<Vec<u8>>>, HostError> {
        self.with_reader(|conn| {
            // One read transaction: every key is read from the same snapshot.
            let tx = conn.transaction().map_err(storage)?;
            let mut out = Vec::with_capacity(keys.len());
            {
                let mut stmt = tx
                    .prepare_cached("SELECT v FROM kv WHERE k = ?1")
                    .map_err(storage)?;
                for k in &keys {
                    out.push(
                        stmt.query_row([k], |r| r.get(0))
                            .optional()
                            .map_err(storage)?,
                    );
                }
            }
            tx.commit().map_err(storage)?;
            Ok(out)
        })
    }

    fn scan(&self, range: KeyRange, dir: Dir, limit: u32) -> Result<Pairs, HostError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let (cond, mut params) = range_sql(&range);
        let order = match dir {
            Dir::Forward => "ASC",
            Dir::Reverse => "DESC",
        };
        let sql = format!("SELECT k, v FROM kv WHERE {cond} ORDER BY k {order} LIMIT ?");
        params.push(Param::Int(i64::from(limit)));
        self.with_reader(|conn| {
            let mut stmt = conn.prepare_cached(&sql).map_err(storage)?;
            let rows = stmt
                .query_map(params_from_iter(params.iter()), |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .map_err(storage)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(storage)
        })
    }

    fn write(&self, batch: Vec<KvOp>) -> Result<(), HostError> {
        if batch.is_empty() {
            return Ok(());
        }
        let mut conn = lock(&self.writer);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        for op in batch {
            match op {
                KvOp::Put(k, v) => {
                    tx.prepare_cached("INSERT OR REPLACE INTO kv (k, v) VALUES (?1, ?2)")
                        .and_then(|mut s| s.execute((k, v)))
                        .map_err(storage)?;
                }
                KvOp::Delete(k) => {
                    tx.prepare_cached("DELETE FROM kv WHERE k = ?1")
                        .and_then(|mut s| s.execute([k]))
                        .map_err(storage)?;
                }
                KvOp::DeleteRange(range) => {
                    let (cond, params) = range_sql(&range);
                    tx.prepare_cached(&format!("DELETE FROM kv WHERE {cond}"))
                        .and_then(|mut s| s.execute(params_from_iter(params.iter())))
                        .map_err(storage)?;
                }
            }
        }
        // Dropping `tx` on an early return above rolls the whole batch back.
        tx.commit().map_err(storage)
    }
}

/// Key-value pairs returned by a scan.
type Pairs = Vec<(Vec<u8>, Vec<u8>)>;

/// A bound SQL parameter.
enum Param {
    Blob(Vec<u8>),
    Int(i64),
}

impl rusqlite::ToSql for Param {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        match self {
            Param::Blob(b) => b.to_sql(),
            Param::Int(i) => i.to_sql(),
        }
    }
}

/// The `WHERE` condition and parameters selecting `range`.
fn range_sql(range: &KeyRange) -> (String, Vec<Param>) {
    let mut conds = Vec::new();
    let mut params = Vec::new();
    match &range.start {
        Bound::Included(s) => {
            conds.push("k >= ?");
            params.push(Param::Blob(s.clone()));
        }
        Bound::Excluded(s) => {
            conds.push("k > ?");
            params.push(Param::Blob(s.clone()));
        }
        Bound::Unbounded => {}
    }
    match &range.end {
        Bound::Included(e) => {
            conds.push("k <= ?");
            params.push(Param::Blob(e.clone()));
        }
        Bound::Excluded(e) => {
            conds.push("k < ?");
            params.push(Param::Blob(e.clone()));
        }
        Bound::Unbounded => {}
    }
    let cond = if conds.is_empty() {
        "1".to_owned()
    } else {
        conds.join(" AND ")
    };
    (cond, params)
}

impl KvStore for SqliteKv {
    async fn get_many(&self, keys: Vec<Vec<u8>>) -> Result<Vec<Option<Vec<u8>>>, HostError> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        self.run(move |inner| inner.get_many(keys)).await
    }

    async fn scan(
        &self,
        range: KeyRange,
        dir: Dir,
        limit: u32,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>, HostError> {
        self.run(move |inner| inner.scan(range, dir, limit)).await
    }

    async fn write(&self, batch: Vec<KvOp>) -> Result<(), HostError> {
        self.run(move |inner| inner.write(batch)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn conformance_on_file() {
        let dir = tempfile::tempdir().unwrap();
        let kv = SqliteKv::open(&dir.path().join("cache.sqlite"), Durability::Normal).unwrap();
        lessdugin_testkit::kv::kv_conformance(&kv).await;
    }

    #[tokio::test]
    async fn conformance_on_file_full_durability() {
        let dir = tempfile::tempdir().unwrap();
        let kv = SqliteKv::open(&dir.path().join("state.sqlite"), Durability::Full).unwrap();
        lessdugin_testkit::kv::kv_conformance(&kv).await;
    }

    #[tokio::test]
    async fn conformance_in_memory() {
        let kv = SqliteKv::open_in_memory().unwrap();
        lessdugin_testkit::kv::kv_conformance(&kv).await;
    }

    #[test]
    fn conformance_without_runtime() {
        // Outside Tokio the store runs SQLite inline.
        let dir = tempfile::tempdir().unwrap();
        let kv = SqliteKv::open(&dir.path().join("x.sqlite"), Durability::Normal).unwrap();
        futures::executor::block_on(lessdugin_testkit::kv::kv_conformance(&kv));
    }

    #[tokio::test]
    async fn persists_across_reopen_and_processes_share_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/state.sqlite");
        let a = SqliteKv::open(&path, Durability::Full).unwrap();
        a.write(vec![KvOp::Put(b"k".to_vec(), b"v1".to_vec())])
            .await
            .unwrap();
        // A second handle on the same file (as another process would open it) sees the write,
        // and its writes are seen by the first.
        let b = SqliteKv::open(&path, Durability::Full).unwrap();
        assert_eq!(
            b.get_many(vec![b"k".to_vec()]).await.unwrap(),
            vec![Some(b"v1".to_vec())]
        );
        b.write(vec![KvOp::Put(b"k".to_vec(), b"v2".to_vec())])
            .await
            .unwrap();
        assert_eq!(
            a.get_many(vec![b"k".to_vec()]).await.unwrap(),
            vec![Some(b"v2".to_vec())]
        );
        drop((a, b));
        let c = SqliteKv::open(&path, Durability::Full).unwrap();
        assert_eq!(
            c.get_many(vec![b"k".to_vec()]).await.unwrap(),
            vec![Some(b"v2".to_vec())]
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
            let dmode = std::fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(dmode & 0o777, 0o700);
            let wal = std::fs::metadata(dir.path().join("sub/state.sqlite-wal")).unwrap();
            assert_eq!(
                wal.permissions().mode() & 0o077,
                0,
                "WAL file is private too"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinked_database() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.sqlite");
        SqliteKv::open(&real, Durability::Normal).unwrap();
        let link = dir.path().join("link.sqlite");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(matches!(
            SqliteKv::open(&link, Durability::Normal),
            Err(HostError::Storage(_))
        ));
    }

    #[tokio::test]
    async fn concurrent_readers_and_writers() {
        let dir = tempfile::tempdir().unwrap();
        let kv = SqliteKv::open(&dir.path().join("c.sqlite"), Durability::Normal).unwrap();
        let mut tasks = Vec::new();
        for t in 0..8u8 {
            let kv = kv.clone();
            tasks.push(tokio::spawn(async move {
                for i in 0..50u8 {
                    kv.write(vec![KvOp::Put(vec![t, i], vec![i])])
                        .await
                        .unwrap();
                    let got = kv
                        .scan(KeyRange::prefix(&[t]), Dir::Reverse, 1)
                        .await
                        .unwrap();
                    assert_eq!(
                        got[0].0,
                        vec![t, i],
                        "a reader sees the latest committed write"
                    );
                }
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }
        let all = kv
            .scan(KeyRange::prefix(b""), Dir::Forward, 10_000)
            .await
            .unwrap();
        assert_eq!(all.len(), 400);
    }

    #[tokio::test]
    async fn failed_batch_is_rolled_back() {
        let kv = SqliteKv::open_in_memory().unwrap();
        kv.write(vec![KvOp::Put(b"a".to_vec(), b"1".to_vec())])
            .await
            .unwrap();
        // Make the table reject one specific insert to force a mid-batch failure.
        {
            let conn = lock(&kv.inner.writer);
            conn.execute_batch(
                "CREATE TRIGGER no_bad BEFORE INSERT ON kv WHEN NEW.k = x'626164' \
                 BEGIN SELECT RAISE(ABORT, 'bad key'); END;",
            )
            .unwrap();
        }
        let err = kv
            .write(vec![
                KvOp::Delete(b"a".to_vec()),
                KvOp::Put(b"b".to_vec(), b"2".to_vec()),
                KvOp::Put(b"bad".to_vec(), b"3".to_vec()),
            ])
            .await
            .unwrap_err();
        assert!(matches!(err, HostError::Storage(_)));
        let got = kv
            .get_many(vec![b"a".to_vec(), b"b".to_vec()])
            .await
            .unwrap();
        assert_eq!(
            got,
            vec![Some(b"1".to_vec()), None],
            "nothing of the failed batch applied"
        );
    }
}
