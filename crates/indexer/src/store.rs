// SPDX-License-Identifier: AGPL-3.0-or-later
//! Durable indexer side tables. They share the native durable SQLite database but never replace
//! or duplicate the core record store.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use lessdugin_atp::syntax::AtUri;
use lessdugin_core::model::ForumModel;
use rusqlite::{Connection, OptionalExtension, params};

/// At most this many forum seeds are attributed to one DID (its own declarations, or the forums
/// its memberships name before they are verified), `threat-model.md` INFRA-12.
pub const SEEDS_PER_DID: i64 = 32;
/// At most this many forum seeds are attributed to the DIDs of one PDS host (INFRA-12).
pub const SEEDS_PER_HOST: i64 = 2_000;
/// At most this many pending relay membership hints per forum from the DIDs of one PDS host.
pub const CANDIDATES_PER_HOST: i64 = 500;
/// At most this many pending membership hints per forum in all.
pub const CANDIDATES_PER_FORUM: i64 = 10_000;
/// A forum whose sync failed is retried after `RETRY_BASE_MS · 2^(failures-1)`, at most
/// [`RETRY_MAX_MS`], independently of the six-hour discovery cadence.
pub const RETRY_BASE_MS: i64 = 60_000;
/// The longest per-forum or per-repo back-off.
pub const RETRY_MAX_MS: i64 = 6 * 60 * 60 * 1000;
/// The first retry of a repository whose live event could not be applied.
pub const REPO_RETRY_BASE_MS: i64 = 2_000;
/// A seed whose declaration is proven absent is rechecked at most this often.
pub const ABSENT_RECHECK_MS: i64 = 24 * 60 * 60 * 1000;
/// A seed whose declaration has been absent this long (and never indexed since) is dropped.
pub const ABSENT_DROP_MS: i64 = 7 * 24 * 60 * 60 * 1000;

#[derive(Clone)]
pub struct Store(Arc<Inner>);

struct Inner {
    conn: Mutex<Connection>,
    /// Incremented whenever the forum set, a forum's identity or its participants change, so the
    /// live relevance context is rebuilt only then.
    generation: AtomicU64,
}

#[derive(Clone, Debug)]
pub struct ForumRow {
    pub uri: String,
    pub did: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: String,
    pub cid: String,
    pub participants: usize,
    pub posts: usize,
    pub synced_ms: i64,
    pub input_digest: String,
    pub fk: String,
    pub failures: i64,
    pub retry_ms: i64,
    pub absent_ms: i64,
    pub sync_due_ms: i64,
}

#[derive(Clone, Debug)]
pub struct ParticipantRow {
    pub did: String,
    pub status: String,
    pub rev: Option<String>,
    pub last_seen_ms: i64,
}

#[derive(Clone, Debug)]
pub struct HeadRow {
    pub did: String,
    pub rev: Option<String>,
    pub commit: Option<String>,
    pub pds: Option<String>,
    pub checked_ms: i64,
    pub status: String,
}

#[derive(Clone, Debug)]
pub struct ChangeRow {
    pub seq: i64,
    pub uri: String,
    pub cid: Option<String>,
    pub did: String,
    pub rev: String,
    pub op: String,
}

#[derive(Clone, Debug)]
pub struct ProofRow {
    pub uri: String,
    pub rev: String,
    pub car: Vec<u8>,
    pub commit: String,
    pub cid: Option<String>,
}

/// A Jetstream commit applied by live ingest: its claimed rev, and its CID (absent for a
/// delete). The change log records this rev when the projection admitted that same version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveChange {
    pub rev: String,
    pub cid: Option<String>,
}

/// A repository whose live event could not be applied, waiting for its per-repo retry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoRetry {
    pub did: String,
    pub forum: String,
    pub attempts: i64,
    pub next_ms: i64,
}

fn invalid(e: impl std::fmt::Display) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(e.to_string())
}

/// `base · 2^(attempts-1)`, at most [`RETRY_MAX_MS`].
pub fn backoff_ms(base: i64, attempts: i64) -> i64 {
    let shift = attempts.saturating_sub(1).clamp(0, 30) as u32;
    base.saturating_mul(1i64 << shift).min(RETRY_MAX_MS)
}

impl Store {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        migrate_candidates(&conn)?;
        migrate_forums(&conn)?;
        Ok(Self::wrap(conn))
    }

    pub fn in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        migrate_candidates(&conn)?;
        migrate_forums(&conn)?;
        Ok(Self::wrap(conn))
    }

    fn wrap(conn: Connection) -> Self {
        Self(Arc::new(Inner {
            conn: Mutex::new(conn),
            generation: AtomicU64::new(1),
        }))
    }

    fn db(&self) -> MutexGuard<'_, Connection> {
        self.0.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn bump(&self) {
        self.0.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Changes whenever a forum is added, removed or re-indexed with other participants.
    pub fn generation(&self) -> u64 {
        self.0.generation.load(Ordering::Relaxed)
    }

    /// An operator seed (`--forum`): never subject to the hint quotas.
    pub fn add_forum_seed(&self, forum: &str) -> rusqlite::Result<()> {
        self.add_seed(forum, "", "").map(|_| ())
    }

    /// A forum hint from a declaration or a membership found by discovery, attributed to the DID
    /// whose record named it and that DID's PDS host. Returns false when the hint was refused by
    /// the per-DID or per-host quota (`threat-model.md` INFRA-12). A known forum is never
    /// refused, and does not count again.
    pub fn add_forum_hint(&self, forum: &str, via: &str, via_host: &str) -> rusqlite::Result<bool> {
        let known = self
            .db()
            .query_row("SELECT 1 FROM forums WHERE uri=?1", [forum], |_| Ok(()))
            .optional()?
            .is_some();
        if known {
            return Ok(true);
        }
        let (by_did, by_host): (i64, i64) = {
            let db = self.db();
            let by_did = db.query_row("SELECT COUNT(*) FROM forums WHERE via=?1", [via], |r| {
                r.get(0)
            })?;
            let by_host = if via_host.is_empty() {
                0
            } else {
                db.query_row(
                    "SELECT COUNT(*) FROM forums WHERE via_host=?1",
                    [via_host],
                    |r| r.get(0),
                )?
            };
            (by_did, by_host)
        };
        if by_did >= SEEDS_PER_DID || by_host >= SEEDS_PER_HOST {
            return Ok(false);
        }
        self.add_seed(forum, via, via_host)
    }

    fn add_seed(&self, forum: &str, via: &str, via_host: &str) -> rusqlite::Result<bool> {
        let uri = AtUri::parse(forum).map_err(invalid)?;
        let fk = lessdugin_lexicon::keys::fk(uri.as_str()).map_err(invalid)?;
        let inserted = self.db().execute(
            "INSERT INTO forums(uri,did,rkey,name,created_at,cid,synced_ms,input_digest,fk,via,via_host) VALUES(?1,?2,?3,'','','',0,'',?4,?5,?6) ON CONFLICT(uri) DO NOTHING",
            params![
                uri.to_string(),
                uri.did().as_str(),
                uri.rkey().as_str(),
                fk,
                via,
                via_host
            ],
        )?;
        if inserted > 0 {
            self.bump();
        }
        Ok(true)
    }

    /// Persist a relay membership hint, never a confirmed participant. Ordinary forum sync
    /// checks the author's own signed membership before admitting the repository. A changed
    /// membership CID is a new hint; a repeated listing does not reset rejection back-off.
    pub fn add_candidate(
        &self,
        forum: &str,
        did: &str,
        cid: &str,
        created_ms: i64,
    ) -> rusqlite::Result<()> {
        self.add_candidate_from(forum, did, cid, created_ms, "")
            .map(|_| ())
    }

    /// [`Store::add_candidate`] for a hint from a DID on PDS host `host` (empty when unknown).
    /// A new hint is refused (false) beyond [`CANDIDATES_PER_HOST`] pending hints of that host
    /// or [`CANDIDATES_PER_FORUM`] pending hints in all, for this forum.
    pub fn add_candidate_from(
        &self,
        forum: &str,
        did: &str,
        cid: &str,
        created_ms: i64,
        host: &str,
    ) -> rusqlite::Result<bool> {
        lessdugin_core::discovery::ForumKey::of_uri(&AtUri::parse(forum).map_err(invalid)?)
            .map_err(invalid)?;
        lessdugin_atp::syntax::Did::parse(did).map_err(invalid)?;
        let db = self.db();
        let known = db
            .query_row(
                "SELECT 1 FROM forum_candidates WHERE forum=?1 AND did=?2",
                params![forum, did],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !known {
            let pending: i64 = db.query_row(
                "SELECT COUNT(*) FROM forum_candidates WHERE forum=?1 AND state='pending'",
                [forum],
                |r| r.get(0),
            )?;
            let from_host: i64 = if host.is_empty() {
                0
            } else {
                db.query_row(
                    "SELECT COUNT(*) FROM forum_candidates WHERE forum=?1 AND host=?2 AND state='pending'",
                    params![forum, host],
                    |r| r.get(0),
                )?
            };
            if pending >= CANDIDATES_PER_FORUM || from_host >= CANDIDATES_PER_HOST {
                return Ok(false);
            }
        }
        db.execute(
            "INSERT INTO forum_candidates(forum,did,cid,created_ms,state,observed_ms,host) VALUES(?1,?2,?3,?4,'pending',0,?5) ON CONFLICT(forum,did) DO UPDATE SET cid=excluded.cid,created_ms=excluded.created_ms,state=CASE WHEN forum_candidates.state='rejected' AND forum_candidates.cid=excluded.cid THEN 'rejected' WHEN forum_candidates.cid<>excluded.cid OR forum_candidates.state='confirmed' THEN 'pending' ELSE forum_candidates.state END WHERE forum_candidates.cid<>excluded.cid OR forum_candidates.created_ms<excluded.created_ms OR forum_candidates.state='confirmed'",
            params![forum, did, cid, created_ms, host],
        )?;
        Ok(true)
    }

    /// At most 500 unconfirmed candidates, most recently declared first within this relay
    /// enumeration source. Rejected hints join only after their 24-hour back-off.
    pub fn candidates(&self, forum: &str, now_ms: i64) -> Vec<(String, String, i64)> {
        let db = self.db();
        let mut q = db
            .prepare("SELECT did,cid,created_ms FROM forum_candidates WHERE forum=?1 AND (state='pending' OR (state='rejected' AND observed_ms<=?2)) ORDER BY created_ms DESC,did DESC LIMIT 500",
            )
            .unwrap();
        q.query_map(
            params![
                forum,
                now_ms - lessdugin_core::discovery::state::REJECTED_RECHECK_MS
            ],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
        .filter_map(Result::ok)
        .collect()
    }

    pub fn set_candidate_state(
        &self,
        forum: &str,
        did: &str,
        state: &str,
        now_ms: i64,
    ) -> rusqlite::Result<()> {
        if !matches!(state, "confirmed" | "rejected") {
            return Err(rusqlite::Error::InvalidParameterName(state.into()));
        }
        self.db().execute(
            "UPDATE forum_candidates SET state=?3,observed_ms=?4 WHERE forum=?1 AND did=?2",
            params![forum, did, state, now_ms],
        )?;
        Ok(())
    }

    pub fn forum_seeds(&self) -> Vec<String> {
        let db = self.db();
        let mut q = db.prepare("SELECT uri FROM forums ORDER BY uri").unwrap();
        q.query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect()
    }

    /// The seeds a sync pass visits at `now_ms`: neither in a failure back-off nor quarantined
    /// as absent (an absent seed is rechecked after [`ABSENT_RECHECK_MS`]).
    pub fn forums_for_pass(&self, now_ms: i64) -> Vec<String> {
        let db = self.db();
        let mut q = db
            .prepare("SELECT uri FROM forums WHERE (failures=0 OR retry_ms<=?1) AND (absent_ms=0 OR absent_ms<=?2) ORDER BY uri")
            .unwrap();
        q.query_map(params![now_ms, now_ms - ABSENT_RECHECK_MS], |r| r.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect()
    }

    /// Forums whose own retry or newcomer sync is due, independently of the discovery cadence.
    pub fn forums_due(&self, now_ms: i64) -> Vec<String> {
        let db = self.db();
        let mut q = db
            .prepare("SELECT uri FROM forums WHERE absent_ms=0 AND ((failures>0 AND retry_ms<=?1) OR (sync_due_ms>0 AND sync_due_ms<=?1 AND (failures=0 OR retry_ms<=?1))) ORDER BY uri")
            .unwrap();
        q.query_map([now_ms], |r| r.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect()
    }

    /// Asks for an ordinary sync of `forum` (newcomer confirmation, a changed roster) no later
    /// than `at_ms`. An earlier request is kept.
    pub fn schedule_forum_sync(&self, forum: &str, at_ms: i64) -> rusqlite::Result<()> {
        self.db().execute(
            "UPDATE forums SET sync_due_ms=?2 WHERE uri=?1 AND (sync_due_ms=0 OR sync_due_ms>?2)",
            params![forum, at_ms],
        )?;
        Ok(())
    }

    /// A live commit showed the declaration again: the quarantine ends and a sync is due.
    pub fn lift_quarantine(&self, forum: &str, at_ms: i64) -> rusqlite::Result<()> {
        self.db().execute(
            "UPDATE forums SET absent_ms=0,sync_due_ms=?2 WHERE uri=?1 AND absent_ms>0",
            params![forum, at_ms],
        )?;
        Ok(())
    }

    /// A successful ordinary sync: clears the back-off, quarantine and pending sync request.
    pub fn note_forum_synced(&self, forum: &str) -> rusqlite::Result<()> {
        self.db().execute(
            "UPDATE forums SET failures=0,retry_ms=0,absent_ms=0,sync_due_ms=0,error='' WHERE uri=?1",
            [forum],
        )?;
        Ok(())
    }

    /// A failed sync: exponential per-forum back-off. Returns the retry time.
    pub fn note_forum_failed(
        &self,
        forum: &str,
        now_ms: i64,
        error: &str,
    ) -> rusqlite::Result<i64> {
        let mut db = self.db();
        let tx = db.transaction()?;
        let failures: i64 = tx
            .query_row("SELECT failures FROM forums WHERE uri=?1", [forum], |r| {
                r.get(0)
            })
            .optional()?
            .unwrap_or(0i64)
            .saturating_add(1);
        let at = now_ms.saturating_add(backoff_ms(RETRY_BASE_MS, failures));
        tx.execute(
            "UPDATE forums SET failures=?2,retry_ms=?3,sync_due_ms=0,error=?4 WHERE uri=?1",
            params![forum, failures, at, error],
        )?;
        tx.commit()?;
        Ok(at)
    }

    /// The declaration is proven absent: the forum's snapshot is replaced by an empty one (each
    /// indexed record is logged as deleted) and the seed is quarantined. A seed absent for
    /// [`ABSENT_DROP_MS`] is dropped with its candidates. Returns the number of logged deletes.
    pub fn note_forum_absent(&self, forum: &str, now_ms: i64) -> rusqlite::Result<usize> {
        let mut db = self.db();
        let tx = db.transaction()?;
        let since: Option<i64> = tx
            .query_row("SELECT absent_ms FROM forums WHERE uri=?1", [forum], |r| {
                r.get(0)
            })
            .optional()?;
        let Some(since) = since else {
            return Ok(0);
        };
        let mut deletes = 0;
        {
            let mut q = tx.prepare("SELECT uri FROM record_state WHERE forum=?1")?;
            let gone = q
                .query_map([forum], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            let mut log = tx.prepare(
                "INSERT INTO change_log(forum,uri,cid,did,rev,op,at_ms) VALUES(?1,?2,NULL,?3,'','delete',?4)",
            )?;
            for u in gone {
                let did = AtUri::parse(&u)
                    .map(|u| u.did().to_string())
                    .unwrap_or_default();
                log.execute(params![forum, u, did, now_ms])?;
                deletes += 1;
            }
        }
        tx.execute("DELETE FROM record_state WHERE forum=?1", [forum])?;
        tx.execute("DELETE FROM participants WHERE forum=?1", [forum])?;
        if since > 0 && now_ms.saturating_sub(since) >= ABSENT_DROP_MS {
            tx.execute("DELETE FROM forum_candidates WHERE forum=?1", [forum])?;
            tx.execute("DELETE FROM repo_retry WHERE forum=?1", [forum])?;
            tx.execute("DELETE FROM forums WHERE uri=?1", [forum])?;
        } else {
            tx.execute(
                "UPDATE forums SET synced_ms=0,cid='',input_digest='',participant_count=0,post_count=0,failures=0,retry_ms=0,sync_due_ms=0,error='absent',absent_ms=CASE WHEN absent_ms=0 THEN ?2 ELSE absent_ms END WHERE uri=?1",
                params![forum, now_ms],
            )?;
        }
        tx.commit()?;
        self.bump();
        Ok(deletes)
    }

    pub fn latest_seq(&self) -> i64 {
        self.db()
            .query_row("SELECT COALESCE(MAX(seq),0) FROM change_log", [], |r| {
                r.get(0)
            })
            .unwrap_or(0)
    }

    pub fn change_floor(&self) -> i64 {
        self.cursor("change_floor")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    }

    pub fn snapshot_ms(&self) -> i64 {
        self.db().query_row("SELECT MAX(t) FROM (SELECT MAX(at_ms) AS t FROM change_log UNION ALL SELECT MAX(synced_ms) AS t FROM forums)", [], |r|r.get::<_,Option<i64>>(0)).ok().flatten().unwrap_or(0)
    }

    pub fn oldest_seq(&self) -> i64 {
        self.db()
            .query_row("SELECT COALESCE(MIN(seq),0) FROM change_log", [], |r| {
                r.get(0)
            })
            .unwrap_or(0)
    }

    pub fn cursor(&self, source: &str) -> Option<String> {
        self.db()
            .query_row("SELECT value FROM cursors WHERE name=?1", [source], |r| {
                r.get(0)
            })
            .optional()
            .ok()
            .flatten()
    }

    pub fn set_cursor(&self, source: &str, value: &str) -> rusqlite::Result<()> {
        self.db().execute("INSERT INTO cursors(name,value) VALUES(?1,?2) ON CONFLICT(name) DO UPDATE SET value=excluded.value", params![source,value])?;
        Ok(())
    }

    /// Moves a numeric cursor forward only: a replayed (rewound) frame never moves it back.
    pub fn advance_cursor(&self, source: &str, value: i64) -> rusqlite::Result<()> {
        self.db().execute("INSERT INTO cursors(name,value) VALUES(?1,?2) ON CONFLICT(name) DO UPDATE SET value=excluded.value WHERE CAST(cursors.value AS INTEGER)<CAST(excluded.value AS INTEGER)", params![source,value.to_string()])?;
        Ok(())
    }

    pub fn scheduled_at(&self, name: &str) -> Option<i64> {
        self.db()
            .query_row("SELECT value FROM schedule WHERE name=?1", [name], |r| {
                r.get(0)
            })
            .optional()
            .ok()
            .flatten()
    }

    pub fn schedule_due(&self, name: &str, now_ms: i64) -> bool {
        self.db()
            .query_row("SELECT value FROM schedule WHERE name=?1", [name], |r| {
                let at: i64 = r.get(0)?;
                Ok(at <= now_ms)
            })
            .unwrap_or(true)
    }

    pub fn schedule_next(&self, name: &str, at_ms: i64) -> rusqlite::Result<()> {
        self.db().execute("INSERT INTO schedule(name,value) VALUES(?1,?2) ON CONFLICT(name) DO UPDATE SET value=excluded.value", params![name,at_ms])?;
        Ok(())
    }

    fn forum_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ForumRow> {
        Ok(ForumRow {
            uri: r.get(0)?,
            did: r.get(1)?,
            name: r.get(2)?,
            description: r.get(3)?,
            created_at: r.get(4)?,
            cid: r.get(5)?,
            synced_ms: r.get(6)?,
            input_digest: r.get(7)?,
            participants: r.get::<_, i64>(8)? as usize,
            posts: r.get::<_, i64>(9)? as usize,
            fk: r.get(10)?,
            failures: r.get(11)?,
            retry_ms: r.get(12)?,
            absent_ms: r.get(13)?,
            sync_due_ms: r.get(14)?,
        })
    }

    /// Every forum row, seeds included. Counts are maintained columns, so this costs one pass
    /// over `forums`, never a scan of the indexed records.
    pub fn forums(&self, q: Option<&str>) -> Vec<ForumRow> {
        self.forums_where(q, false)
    }

    /// The forums served by `listForums` and `describeIndexer`: synced, with a declaration CID,
    /// not quarantined.
    pub fn indexed_forums(&self, q: Option<&str>) -> Vec<ForumRow> {
        self.forums_where(q, true)
    }

    fn forums_where(&self, q: Option<&str>, indexed: bool) -> Vec<ForumRow> {
        let db = self.db();
        let pattern = q.map(|s| format!("%{}%", s.replace(['%', '_'], "")));
        let sql = format!(
            "SELECT {FORUM_COLUMNS} FROM forums WHERE (?1 IS NULL OR uri LIKE ?1 OR name LIKE ?1){} ORDER BY uri",
            if indexed {
                " AND synced_ms>0 AND cid<>'' AND absent_ms=0"
            } else {
                ""
            }
        );
        let mut q = db.prepare(&sql).unwrap();
        q.query_map([&pattern], Self::forum_from_row)
            .unwrap()
            .filter_map(Result::ok)
            .collect()
    }

    pub fn forum_count(&self) -> i64 {
        self.db()
            .query_row("SELECT COUNT(*) FROM forums", [], |r| r.get(0))
            .unwrap_or(0)
    }

    /// One synced forum by its declaration URI (a primary-key lookup).
    pub fn forum(&self, uri: &str) -> Option<ForumRow> {
        self.forum_row(uri)
            .filter(|f| f.synced_ms != 0 && !f.cid.is_empty() && f.absent_ms == 0)
    }

    /// One forum row (a seed too) by its declaration URI.
    pub fn forum_row(&self, uri: &str) -> Option<ForumRow> {
        self.db()
            .query_row(
                &format!("SELECT {FORUM_COLUMNS} FROM forums WHERE uri=?1"),
                [uri],
                Self::forum_from_row,
            )
            .optional()
            .ok()
            .flatten()
    }

    /// The forum seeds whose key is `fk` (an indexed lookup; at most a handful).
    pub fn forums_by_fk(&self, fk: &str) -> Vec<String> {
        let db = self.db();
        let mut q = db
            .prepare("SELECT uri FROM forums WHERE fk=?1 ORDER BY uri")
            .unwrap();
        q.query_map([fk], |r| r.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect()
    }

    /// Forums that `did` owns or participates in (indexed lookups).
    pub fn did_forums(&self, did: &str) -> Vec<String> {
        let db = self.db();
        let mut q = db
            .prepare("SELECT uri FROM forums WHERE did=?1 UNION SELECT forum FROM participants WHERE did=?1 ORDER BY 1")
            .unwrap();
        q.query_map([did], |r| r.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect()
    }

    /// Every participant DID of every forum, once.
    pub fn participant_dids(&self) -> BTreeSet<String> {
        let db = self.db();
        let mut q = db.prepare("SELECT DISTINCT did FROM participants").unwrap();
        q.query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect()
    }

    pub fn participants(&self, forum: &str, status: Option<&str>) -> Vec<ParticipantRow> {
        let db = self.db();
        let mut q = db.prepare("SELECT did,status,rev,last_seen_ms FROM participants WHERE forum=?1 AND (?2 IS NULL OR status=?2) ORDER BY did").unwrap();
        q.query_map(params![forum, status], |r| {
            Ok(ParticipantRow {
                did: r.get(0)?,
                status: r.get(1)?,
                rev: r.get(2)?,
                last_seen_ms: r.get(3)?,
            })
        })
        .unwrap()
        .filter_map(Result::ok)
        .collect()
    }

    pub fn put_head(&self, row: &HeadRow) -> rusqlite::Result<()> {
        self.put_heads(std::slice::from_ref(row))
    }

    /// Upserts head rows in one transaction; unchanged rows are not rewritten.
    pub fn put_heads(&self, rows: &[HeadRow]) -> rusqlite::Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let mut db = self.db();
        let tx = db.transaction()?;
        {
            let mut up = tx.prepare("INSERT INTO repo_heads(did,rev,commit_cid,pds,checked_ms,status) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(did) DO UPDATE SET rev=excluded.rev,commit_cid=excluded.commit_cid,pds=excluded.pds,checked_ms=excluded.checked_ms,status=excluded.status WHERE repo_heads.rev IS NOT excluded.rev OR repo_heads.commit_cid IS NOT excluded.commit_cid OR repo_heads.pds IS NOT excluded.pds OR repo_heads.checked_ms<>excluded.checked_ms OR repo_heads.status<>excluded.status")?;
            for row in rows {
                up.execute(params![
                    row.did,
                    row.rev,
                    row.commit,
                    row.pds,
                    row.checked_ms,
                    row.status
                ])?;
            }
        }
        tx.commit()
    }

    pub fn heads(&self, dids: &[String]) -> Vec<HeadRow> {
        let db = self.db();
        let mut out = Vec::new();
        for did in dids {
            if let Ok(Some(row)) = db
                .query_row(
                    "SELECT did,rev,commit_cid,pds,checked_ms,status FROM repo_heads WHERE did=?1",
                    [did],
                    |r| {
                        Ok(HeadRow {
                            did: r.get(0)?,
                            rev: r.get(1)?,
                            commit: r.get(2)?,
                            pds: r.get(3)?,
                            checked_ms: r.get(4)?,
                            status: r.get(5)?,
                        })
                    },
                )
                .optional()
            {
                out.push(row);
            }
        }
        out
    }

    pub fn changes(&self, forum: Option<&str>, since: i64, limit: usize) -> Vec<ChangeRow> {
        let db = self.db();
        let mut q = db.prepare("SELECT seq,uri,cid,did,rev,op FROM change_log WHERE seq>?1 AND (?2 IS NULL OR forum=?2) ORDER BY seq LIMIT ?3").unwrap();
        q.query_map(params![since, forum, limit as i64], |r| {
            Ok(ChangeRow {
                seq: r.get(0)?,
                uri: r.get(1)?,
                cid: r.get(2)?,
                did: r.get(3)?,
                rev: r.get(4)?,
                op: r.get(5)?,
            })
        })
        .unwrap()
        .filter_map(Result::ok)
        .collect()
    }

    pub fn prune_changes(&self, cutoff_ms: i64) -> rusqlite::Result<()> {
        let mut db = self.db();
        let tx = db.transaction()?;
        let floor = tx.query_row(
            "SELECT COALESCE(MAX(seq),0) FROM change_log WHERE at_ms<?1",
            [cutoff_ms],
            |r| r.get::<_, i64>(0),
        )?;
        tx.execute("DELETE FROM change_log WHERE at_ms<?1", [cutoff_ms])?;
        if floor > 0 {
            tx.execute("INSERT INTO cursors(name,value) VALUES('change_floor',?1) ON CONFLICT(name) DO UPDATE SET value=excluded.value WHERE CAST(cursors.value AS INTEGER)<CAST(excluded.value AS INTEGER)",[floor])?;
        }
        tx.commit()
    }

    /// Drops cached proofs verified before `cutoff_ms` whose record no forum indexes any more
    /// (deleted, or its forum dropped). Returns the count.
    pub fn prune_proofs(&self, cutoff_ms: i64) -> rusqlite::Result<usize> {
        self.db().execute(
            "DELETE FROM proof_cache WHERE verified_ms<?1 AND NOT EXISTS(SELECT 1 FROM record_state WHERE record_state.uri=proof_cache.uri)",
            [cutoff_ms],
        )
    }

    pub fn proof(&self, uri: &str) -> Option<ProofRow> {
        self.db()
            .query_row(
                "SELECT uri,rev,car,commit_cid,cid FROM proof_cache WHERE uri=?1",
                [uri],
                |r| {
                    Ok(ProofRow {
                        uri: r.get(0)?,
                        rev: r.get(1)?,
                        car: r.get(2)?,
                        commit: r.get(3)?,
                        cid: r.get(4)?,
                    })
                },
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn put_proof(&self, row: &ProofRow, now_ms: i64) -> rusqlite::Result<()> {
        self.db().execute("INSERT INTO proof_cache(uri,rev,car,commit_cid,cid,verified_ms) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(uri) DO UPDATE SET rev=excluded.rev,car=excluded.car,commit_cid=excluded.commit_cid,cid=excluded.cid,verified_ms=excluded.verified_ms WHERE excluded.rev>=proof_cache.rev", params![row.uri,row.rev,row.car,row.commit,row.cid,now_ms])?;
        Ok(())
    }

    /// The repositories of the records whose indexed CID differs from `m`'s (created, updated or
    /// deleted): the rows [`Store::apply_model`] would log.
    pub fn changed_dids(&self, m: &ForumModel) -> rusqlite::Result<BTreeSet<String>> {
        let old = self.record_state(m.key.uri.as_str())?;
        let new = snapshot(m);
        Ok(new
            .iter()
            .filter(|(u, c)| old.get(*u) != Some(*c))
            .map(|(u, _)| u.clone())
            .chain(old.keys().filter(|u| !new.contains_key(*u)).cloned())
            .filter_map(|u| AtUri::parse(&u).ok().map(|u| u.did().to_string()))
            .collect())
    }

    fn record_state(&self, forum: &str) -> rusqlite::Result<BTreeMap<String, String>> {
        let db = self.db();
        let mut q = db.prepare("SELECT uri,cid FROM record_state WHERE forum=?1")?;
        q.query_map([forum], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect()
    }

    /// Atomically brings the derived forum snapshot to `m` and appends its differences to the
    /// log. Only changed rows are written. A change matching a `live` commit (same CID, or a
    /// delete) is logged with that commit's rev; other changes with the repo's synced head rev.
    pub fn apply_model(
        &self,
        m: &ForumModel,
        now_ms: i64,
        repo_revs: &BTreeMap<String, String>,
        live: &BTreeMap<String, LiveChange>,
    ) -> rusqlite::Result<usize> {
        let mut db = self.db();
        let tx = db.transaction()?;
        let forum = m.key.uri.to_string();
        let mut old = BTreeMap::new();
        {
            let mut q = tx.prepare("SELECT uri,cid FROM record_state WHERE forum=?1")?;
            for row in q.query_map([&forum], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })? {
                let (u, c) = row?;
                old.insert(u, c);
            }
        }
        let new = snapshot(m);
        let mut old_participants: BTreeMap<String, (String, Option<String>, i64)> = BTreeMap::new();
        {
            let mut q =
                tx.prepare("SELECT did,status,rev,last_seen_ms FROM participants WHERE forum=?1")?;
            for row in q.query_map([&forum], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    (r.get::<_, String>(1)?, r.get(2)?, r.get(3)?),
                ))
            })? {
                let (d, p) = row?;
                old_participants.insert(d, p);
            }
        }
        let mut changes = 0;
        let mut touched: BTreeSet<String> = BTreeSet::new();
        {
            let mut put = tx.prepare(
                "INSERT INTO record_state(forum,uri,cid) VALUES(?1,?2,?3) ON CONFLICT(forum,uri) DO UPDATE SET cid=excluded.cid",
            )?;
            let mut del = tx.prepare("DELETE FROM record_state WHERE forum=?1 AND uri=?2")?;
            let mut log = tx.prepare(
                "INSERT INTO change_log(forum,uri,cid,did,rev,op,at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            )?;
            for (u, c) in &new {
                if old.get(u) != Some(c) {
                    put.execute(params![forum, u, c])?;
                    let did = AtUri::parse(u)
                        .map(|u| u.did().to_string())
                        .unwrap_or_default();
                    let op = if old.contains_key(u) {
                        "update"
                    } else {
                        "create"
                    };
                    let rev = live
                        .get(u)
                        .filter(|l| l.cid.as_deref() == Some(c.as_str()) && !l.rev.is_empty())
                        .map(|l| l.rev.clone())
                        .or_else(|| repo_revs.get(&did).cloned())
                        .unwrap_or_default();
                    log.execute(params![forum, u, c, did, rev, op, now_ms])?;
                    touched.insert(did);
                    changes += 1;
                }
            }
            for u in old.keys().filter(|u| !new.contains_key(*u)) {
                del.execute(params![forum, u])?;
                let did = AtUri::parse(u)
                    .map(|u| u.did().to_string())
                    .unwrap_or_default();
                let rev = live
                    .get(u)
                    .filter(|l| l.cid.is_none() && !l.rev.is_empty())
                    .map(|l| l.rev.clone())
                    .or_else(|| repo_revs.get(&did).cloned())
                    .unwrap_or_default();
                log.execute(params![
                    forum,
                    u,
                    Option::<String>::None,
                    did,
                    rev,
                    "delete",
                    now_ms
                ])?;
                touched.insert(did);
                changes += 1;
            }
        }
        let mut roster_changed = false;
        {
            let mut put = tx.prepare("INSERT INTO participants(forum,did,status,rev,last_seen_ms) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(forum,did) DO UPDATE SET status=excluded.status,rev=excluded.rev,last_seen_ms=excluded.last_seen_ms")?;
            let mut del = tx.prepare("DELETE FROM participants WHERE forum=?1 AND did=?2")?;
            for did in m.participants() {
                let status = match m.rank(did) {
                    3 => "owner",
                    2 => "admin",
                    1 => "moderator",
                    _ if m.view.participation.s.contains(did) => "established",
                    _ if m.view.participation.members.contains(did) => "member",
                    _ => "newcomer",
                };
                let previous = old_participants.get(did);
                let rev = repo_revs
                    .get(did)
                    .cloned()
                    .or_else(|| previous.and_then(|p| p.1.clone()));
                // When the participant's repository last changed, as far as this index saw.
                let last_seen = match previous {
                    Some((_, _, at)) if !touched.contains(did) => *at,
                    _ => now_ms,
                };
                if previous.is_none_or(|(s, r, at)| s != status || *r != rev || *at != last_seen) {
                    put.execute(params![forum, did, status, rev, last_seen])?;
                    roster_changed |= previous.is_none_or(|(s, _, _)| s != status);
                }
            }
            for did in old_participants
                .keys()
                .filter(|d| !m.participants().contains(*d))
            {
                del.execute(params![forum, did])?;
                roster_changed = true;
            }
        }
        let posts = new
            .keys()
            .filter(|u| {
                m.input
                    .records
                    .get(*u)
                    .is_some_and(|s| s.collection == lessdugin_lexicon::nsid::POST)
            })
            .count() as i64;
        let previous_cid: Option<String> = tx
            .query_row("SELECT cid FROM forums WHERE uri=?1", [&forum], |r| {
                r.get(0)
            })
            .optional()?;
        tx.execute("INSERT INTO forums(uri,did,rkey,name,description,created_at,cid,synced_ms,input_digest,fk,participant_count,post_count,absent_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,0) ON CONFLICT(uri) DO UPDATE SET name=excluded.name,description=excluded.description,created_at=excluded.created_at,cid=excluded.cid,synced_ms=excluded.synced_ms,input_digest=excluded.input_digest,fk=excluded.fk,participant_count=excluded.participant_count,post_count=excluded.post_count,absent_ms=0", params![forum,m.key.owner.as_str(),m.key.uri.rkey().as_str(),m.decl.name,m.decl.description,m.decl.created_at.as_str(),m.input.decl_cid,now_ms,m.view.input_digest,m.key.fk,m.participants().len() as i64,posts])?;
        tx.commit()?;
        if roster_changed || previous_cid.as_deref() != Some(m.input.decl_cid.as_str()) {
            self.bump();
        }
        Ok(changes)
    }

    pub fn record_change(
        &self,
        forum: &str,
        uri: &str,
        cid: Option<&str>,
        rev: &str,
        op: &str,
        now_ms: i64,
    ) -> rusqlite::Result<()> {
        let did = AtUri::parse(uri).map_err(invalid)?;
        self.db().execute(
            "INSERT INTO change_log(forum,uri,cid,did,rev,op,at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![forum, uri, cid, did.did().as_str(), rev, op, now_ms],
        )?;
        Ok(())
    }

    pub fn record_cid(&self, forum: &str, uri: &str) -> Option<String> {
        self.db()
            .query_row(
                "SELECT cid FROM record_state WHERE forum=?1 AND uri=?2",
                params![forum, uri],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    pub(crate) fn indexed_record_cids(&self, uri: &str) -> Vec<String> {
        let db = self.db();
        let mut q = db
            .prepare("SELECT DISTINCT cid FROM record_state WHERE uri=?1")
            .unwrap();
        q.query_map([uri], |r| r.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect()
    }

    /// The URI is an indexed projection and this CID belongs to one of its current snapshots.
    /// Different forums may still hold different revisions. PDS-head currency is checked by the
    /// client, not by choosing whichever forum happened to sync most recently (sync.md §8.6).
    pub fn indexed_record_has_cid(&self, uri: &str, cid: &str) -> bool {
        self.db()
            .query_row(
                "SELECT 1 FROM record_state WHERE uri=?1 AND cid=?2 LIMIT 1",
                params![uri, cid],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .is_ok_and(|present| present.is_some())
    }

    pub fn record_pds(&self, did: &str) -> Option<String> {
        self.db()
            .query_row("SELECT pds FROM repo_heads WHERE did=?1", [did], |r| {
                r.get(0)
            })
            .optional()
            .ok()
            .flatten()
    }

    /// Hands a repository whose live event failed to the per-repo retry path: exponential
    /// back-off from [`REPO_RETRY_BASE_MS`]. Returns the retry time.
    pub fn schedule_repo_retry(
        &self,
        did: &str,
        forum: &str,
        now_ms: i64,
        error: &str,
    ) -> rusqlite::Result<i64> {
        let mut db = self.db();
        let tx = db.transaction()?;
        let attempts: i64 = tx
            .query_row(
                "SELECT attempts FROM repo_retry WHERE did=?1 AND forum=?2",
                params![did, forum],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0i64)
            .saturating_add(1);
        let at = now_ms.saturating_add(backoff_ms(REPO_RETRY_BASE_MS, attempts));
        tx.execute("INSERT INTO repo_retry(did,forum,attempts,next_ms,error) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(did,forum) DO UPDATE SET attempts=excluded.attempts,next_ms=excluded.next_ms,error=excluded.error", params![did, forum, attempts, at, error])?;
        tx.commit()?;
        Ok(at)
    }

    /// Due per-repo retries, oldest first.
    pub fn due_repo_retries(&self, now_ms: i64, limit: usize) -> Vec<RepoRetry> {
        let db = self.db();
        let mut q = db
            .prepare("SELECT did,forum,attempts,next_ms FROM repo_retry WHERE next_ms<=?1 ORDER BY next_ms,did,forum LIMIT ?2")
            .unwrap();
        q.query_map(params![now_ms, limit as i64], |r| {
            Ok(RepoRetry {
                did: r.get(0)?,
                forum: r.get(1)?,
                attempts: r.get(2)?,
                next_ms: r.get(3)?,
            })
        })
        .unwrap()
        .filter_map(Result::ok)
        .collect()
    }

    /// Every pending per-repo retry (diagnostics and tests).
    pub fn repo_retries(&self) -> Vec<RepoRetry> {
        self.due_repo_retries(i64::MAX, usize::MAX >> 1)
    }

    pub fn clear_repo_retry(&self, did: &str, forum: &str) -> rusqlite::Result<()> {
        self.db().execute(
            "DELETE FROM repo_retry WHERE did=?1 AND forum=?2",
            params![did, forum],
        )?;
        Ok(())
    }
}

const FORUM_COLUMNS: &str = "uri,did,name,description,created_at,cid,synced_ms,input_digest,participant_count,post_count,fk,failures,retry_ms,absent_ms,sync_due_ms";

/// The snapshot rows of `m`: every fold input record and the declaration.
fn snapshot(m: &ForumModel) -> BTreeMap<String, String> {
    let mut new: BTreeMap<String, String> = m
        .input
        .records
        .iter()
        .map(|(u, s)| (u.clone(), s.cid.clone()))
        .collect();
    new.insert(m.key.uri.to_string(), m.input.decl_cid.clone());
    new
}

fn columns(conn: &Connection, table: &str) -> rusqlite::Result<Vec<String>> {
    conn.prepare(&format!("PRAGMA table_info({table})"))?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect()
}

fn migrate_candidates(conn: &Connection) -> rusqlite::Result<()> {
    let columns = columns(conn, "forum_candidates")?;
    for (name, declaration) in [
        ("cid", "TEXT NOT NULL DEFAULT ''"),
        ("created_ms", "INTEGER NOT NULL DEFAULT 0"),
        ("state", "TEXT NOT NULL DEFAULT 'pending'"),
        ("observed_ms", "INTEGER NOT NULL DEFAULT 0"),
        ("host", "TEXT NOT NULL DEFAULT ''"),
    ] {
        if !columns.iter().any(|column| column == name) {
            conn.execute(
                &format!("ALTER TABLE forum_candidates ADD COLUMN {name} {declaration}"),
                [],
            )?;
        }
    }
    // Pre-migration hints must go through ordinary PDS confirmation once, not bypass it forever.
    conn.execute(
        "UPDATE forum_candidates SET state='confirmed' WHERE state='pending' AND EXISTS(SELECT 1 FROM participants WHERE participants.forum=forum_candidates.forum AND participants.did=forum_candidates.did)",
        [],
    )?;
    Ok(())
}

/// Maintained counts, the forum key and the per-forum back-off and quota columns. Older
/// databases get them on open, with the counts computed once.
fn migrate_forums(conn: &Connection) -> rusqlite::Result<()> {
    let columns = columns(conn, "forums")?;
    let mut counted = true;
    for (name, declaration) in [
        ("fk", "TEXT NOT NULL DEFAULT ''"),
        ("participant_count", "INTEGER NOT NULL DEFAULT 0"),
        ("post_count", "INTEGER NOT NULL DEFAULT 0"),
        ("failures", "INTEGER NOT NULL DEFAULT 0"),
        ("retry_ms", "INTEGER NOT NULL DEFAULT 0"),
        ("absent_ms", "INTEGER NOT NULL DEFAULT 0"),
        ("sync_due_ms", "INTEGER NOT NULL DEFAULT 0"),
        ("error", "TEXT NOT NULL DEFAULT ''"),
        ("via", "TEXT NOT NULL DEFAULT ''"),
        ("via_host", "TEXT NOT NULL DEFAULT ''"),
    ] {
        if !columns.iter().any(|column| column == name) {
            conn.execute(
                &format!("ALTER TABLE forums ADD COLUMN {name} {declaration}"),
                [],
            )?;
            counted &= name != "participant_count";
        }
    }
    if !counted {
        conn.execute(
            "UPDATE forums SET participant_count=(SELECT COUNT(*) FROM participants WHERE participants.forum=forums.uri),post_count=(SELECT COUNT(*) FROM record_state WHERE record_state.forum=forums.uri AND record_state.uri LIKE '%.post/%')",
            [],
        )?;
    }
    let unkeyed = conn
        .prepare("SELECT uri FROM forums WHERE fk=''")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for uri in unkeyed {
        if let Ok(fk) = lessdugin_lexicon::keys::fk(&uri) {
            conn.execute("UPDATE forums SET fk=?2 WHERE uri=?1", params![uri, fk])?;
        }
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS forums_fk ON forums(fk);
         CREATE INDEX IF NOT EXISTS forums_owner ON forums(did);
         CREATE INDEX IF NOT EXISTS forums_via ON forums(via);
         CREATE INDEX IF NOT EXISTS forums_via_host ON forums(via_host);
         CREATE INDEX IF NOT EXISTS candidates_host ON forum_candidates(forum,host,state);",
    )?;
    Ok(())
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS forum_candidates(
 forum TEXT NOT NULL,did TEXT NOT NULL,
 PRIMARY KEY(forum,did));
CREATE TABLE IF NOT EXISTS forums(
 uri TEXT PRIMARY KEY,did TEXT NOT NULL,rkey TEXT NOT NULL,name TEXT NOT NULL DEFAULT '',
 description TEXT,created_at TEXT NOT NULL DEFAULT '',cid TEXT NOT NULL DEFAULT '',
 synced_ms INTEGER NOT NULL DEFAULT 0,input_digest TEXT NOT NULL DEFAULT '');
CREATE TABLE IF NOT EXISTS participants(
 forum TEXT NOT NULL,did TEXT NOT NULL,status TEXT NOT NULL,rev TEXT,last_seen_ms INTEGER NOT NULL,
 PRIMARY KEY(forum,did));
CREATE INDEX IF NOT EXISTS participants_did ON participants(did);
CREATE TABLE IF NOT EXISTS repo_heads(
 did TEXT PRIMARY KEY,rev TEXT,commit_cid TEXT,pds TEXT,checked_ms INTEGER NOT NULL,status TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS record_state(forum TEXT NOT NULL,uri TEXT NOT NULL,cid TEXT NOT NULL,PRIMARY KEY(forum,uri));
CREATE INDEX IF NOT EXISTS record_state_uri ON record_state(uri,cid);
CREATE TABLE IF NOT EXISTS change_log(
 seq INTEGER PRIMARY KEY AUTOINCREMENT,forum TEXT NOT NULL,uri TEXT NOT NULL,cid TEXT,did TEXT NOT NULL,
 rev TEXT NOT NULL,op TEXT NOT NULL,at_ms INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS change_log_forum ON change_log(forum,seq);
CREATE INDEX IF NOT EXISTS change_log_time ON change_log(at_ms);
CREATE TABLE IF NOT EXISTS proof_cache(
 uri TEXT PRIMARY KEY,rev TEXT NOT NULL,car BLOB NOT NULL,commit_cid TEXT NOT NULL,cid TEXT,verified_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS cursors(name TEXT PRIMARY KEY,value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS schedule(name TEXT PRIMARY KEY,value INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS repo_retry(
 did TEXT NOT NULL,forum TEXT NOT NULL,attempts INTEGER NOT NULL,next_ms INTEGER NOT NULL,error TEXT NOT NULL,
 PRIMARY KEY(did,forum));
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_hints_keep_recency_and_rejection_backoff() {
        let store = Store::in_memory().unwrap();
        let owner = lessdugin_atp::syntax::Did::parse("did:plc:abcdefghijklmnopqrstuvwx").unwrap();
        let forum = lessdugin_core::discovery::ForumKey::new(&owner, "forum")
            .unwrap()
            .uri
            .to_string();
        let older = "did:plc:abcdefghijklmnopqrstuvwy";
        let newer = "did:plc:abcdefghijklmnopqrstuvwz";
        store.add_candidate(&forum, older, "old", 10).unwrap();
        store.add_candidate(&forum, newer, "new", 20).unwrap();
        assert_eq!(
            store.candidates(&forum, 30),
            vec![
                (newer.into(), "new".into(), 20),
                (older.into(), "old".into(), 10)
            ]
        );

        store
            .set_candidate_state(&forum, older, "rejected", 30)
            .unwrap();
        store.add_candidate(&forum, older, "old", 10).unwrap();
        assert_eq!(
            store.candidates(&forum, 31),
            vec![(newer.into(), "new".into(), 20)]
        );

        // A changed signed membership is a new hint; repeated relay enumeration is not.
        store
            .add_candidate(&forum, older, "replacement", 11)
            .unwrap();
        assert!(
            store
                .candidates(&forum, 32)
                .iter()
                .any(|(d, _, _)| d == older)
        );
        store
            .set_candidate_state(&forum, older, "rejected", 40)
            .unwrap();
        store
            .add_candidate(&forum, older, "replacement", 999)
            .unwrap();
        assert_eq!(
            store.candidates(&forum, 41),
            vec![(newer.into(), "new".into(), 20)]
        );

        // An unchanged hint becomes eligible again after the normal rejected-candidate back-off.
        let retry = 40 + lessdugin_core::discovery::state::REJECTED_RECHECK_MS;
        assert!(
            store.candidates(&forum, retry - 1).is_empty()
                || store
                    .candidates(&forum, retry - 1)
                    .iter()
                    .all(|(d, _, _)| d != older)
        );
        assert!(
            store
                .candidates(&forum, retry)
                .iter()
                .any(|(d, _, _)| d == older)
        );
        store
            .set_candidate_state(&forum, newer, "confirmed", retry)
            .unwrap();
        assert!(
            store
                .candidates(&forum, retry)
                .iter()
                .all(|(d, _, _)| d != newer)
        );

        // Open-mode membership deletion can retain its CID when recreated. A relay sighting
        // after confirmation is the signal to confirm the new membership again.
        store.add_candidate(&forum, newer, "new", 20).unwrap();
        assert!(
            store
                .candidates(&forum, retry + 1)
                .contains(&(newer.into(), "new".into(), 20))
        );
    }

    #[test]
    fn proof_scope_accepts_any_current_projection_not_just_the_newest() {
        let store = Store::in_memory().unwrap();
        let record = "at://did:plc:aaaaaaaaaaaaaaaaaaaaaaaa/online.7778777.lessdugin.post/r";
        for (forum, synced, cid) in [
            (
                "at://did:plc:aaaaaaaaaaaaaaaaaaaaaaaa/online.7778777.lessdugin.forum/old",
                10,
                "old",
            ),
            (
                "at://did:plc:aaaaaaaaaaaaaaaaaaaaaaaa/online.7778777.lessdugin.forum/new",
                20,
                "new",
            ),
        ] {
            store.add_forum_seed(forum).unwrap();
            store
                .db()
                .execute(
                    "UPDATE forums SET synced_ms=?2 WHERE uri=?1",
                    params![forum, synced],
                )
                .unwrap();
            store
                .db()
                .execute(
                    "INSERT INTO record_state(forum,uri,cid) VALUES(?1,?2,?3)",
                    params![forum, record, cid],
                )
                .unwrap();
        }
        let cids = store.indexed_record_cids(record);
        assert!(cids.contains(&"old".to_owned()));
        assert!(cids.contains(&"new".to_owned()));
        assert!(!cids.contains(&"forged".to_owned()));
        assert!(store.indexed_record_has_cid(record, "old"));
        assert!(store.indexed_record_has_cid(record, "new"));
        assert!(!store.indexed_record_has_cid(record, "forged"));
    }

    /// SQLite VM steps spent by `f` (deterministic, unlike a timing budget).
    fn steps<T>(store: &Store, f: impl FnOnce() -> T) -> u64 {
        let counter = Arc::new(AtomicU64::new(0));
        let c = Arc::clone(&counter);
        store
            .db()
            .progress_handler(
                1,
                Some(move || {
                    c.fetch_add(1, Ordering::Relaxed);
                    false
                }),
            )
            .unwrap();
        f();
        store
            .db()
            .progress_handler(1, None::<fn() -> bool>)
            .unwrap();
        counter.load(Ordering::Relaxed)
    }

    const FORUM: &str =
        "at://did:plc:aaaaaaaaaaaaaaaaaaaaaaaa/online.7778777.lessdugin.temp.forum/big";

    fn grow(store: &Store, from: usize, n: usize) {
        let mut db = store.db();
        let tx = db.transaction().unwrap();
        for i in from..from + n {
            tx.execute(
                "INSERT INTO record_state(forum,uri,cid) VALUES(?1,?2,'c')",
                params![
                    FORUM,
                    format!(
                        "at://did:plc:aaaaaaaaaaaaaaaaaaaaaaaa/online.7778777.lessdugin.temp.post/r{i}"
                    )
                ],
            )
            .unwrap();
            tx.execute(
                "INSERT INTO participants(forum,did,status,rev,last_seen_ms) VALUES(?1,?2,'member',NULL,0)",
                params![FORUM, format!("did:plc:p{i:023}")],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }

    #[test]
    fn lookups_do_not_scan_indexed_records_or_every_forum() {
        let store = Store::in_memory().unwrap();
        store.add_forum_seed(FORUM).unwrap();
        store
            .db()
            .execute(
                "UPDATE forums SET synced_ms=1,cid='decl' WHERE uri=?1",
                [FORUM],
            )
            .unwrap();
        for i in 0..200 {
            store
                .add_forum_seed(&format!(
                    "at://did:plc:bbbbbbbbbbbbbbbbbbbbbbbb/online.7778777.lessdugin.temp.forum/s{i}"
                ))
                .unwrap();
        }
        let probe = "at://did:plc:aaaaaaaaaaaaaaaaaaaaaaaa/online.7778777.lessdugin.temp.post/r7";
        let fk = store.forum_row(FORUM).unwrap().fk;
        let cost = |store: &Store| {
            [
                steps(store, || store.forum(FORUM)),
                steps(store, || store.forum_row(FORUM)),
                steps(store, || store.indexed_record_cids(probe)),
                steps(store, || store.indexed_record_has_cid(probe, "c")),
                steps(store, || store.forums_by_fk(&fk)),
                steps(store, || {
                    store.did_forums("did:plc:p00000000000000000000007")
                }),
                steps(store, || store.forums(None)),
            ]
        };
        grow(&store, 0, 2_000);
        let small = cost(&store);
        grow(&store, 2_000, 20_000);
        let large = cost(&store);
        // Ten times the records and participants: no lookup may cost more (security-net-1,
        // robustness-5, robustness-6). The forum list grows with forums only.
        assert_eq!(small, large, "lookup cost grew with the indexed records");
        assert!(large[..6].iter().all(|s| *s < 500), "{large:?}");
    }

    #[test]
    fn forum_hints_are_capped_per_did_and_per_host() {
        let store = Store::in_memory().unwrap();
        let forum = |owner: &str, n: usize| {
            format!("at://did:plc:{owner}/online.7778777.lessdugin.temp.forum/f{n}")
        };
        let spammer = "did:plc:ssssssssssssssssssssssss";
        let mut accepted = 0;
        for n in 0..(SEEDS_PER_DID as usize + 8) {
            accepted += usize::from(
                store
                    .add_forum_hint(&forum("xxxxxxxxxxxxxxxxxxxxxxxx", n), spammer, "spam.test")
                    .unwrap(),
            );
        }
        assert_eq!(accepted as i64, SEEDS_PER_DID);
        assert_eq!(store.forum_count(), SEEDS_PER_DID);
        // An already known forum is never refused and does not count again.
        assert!(
            store
                .add_forum_hint(&forum("xxxxxxxxxxxxxxxxxxxxxxxx", 0), spammer, "spam.test")
                .unwrap()
        );
        // Many accounts of one host share the host quota.
        let mut by_host = SEEDS_PER_DID;
        for n in 0.. {
            let did = format!("did:plc:h{n:023}");
            let ok = store
                .add_forum_hint(&forum("yyyyyyyyyyyyyyyyyyyyyyyy", n), &did, "spam.test")
                .unwrap();
            if !ok {
                break;
            }
            by_host += 1;
        }
        assert_eq!(by_host, SEEDS_PER_HOST);
        // Another host is unaffected; operator seeds are never capped.
        assert!(
            store
                .add_forum_hint(
                    &forum("zzzzzzzzzzzzzzzzzzzzzzzz", 0),
                    "did:plc:o",
                    "ok.test"
                )
                .unwrap()
        );
        store
            .add_forum_seed(&forum("zzzzzzzzzzzzzzzzzzzzzzzz", 1))
            .unwrap();

        // Pending membership hints: per host, per forum.
        let real = lessdugin_core::discovery::ForumKey::new(
            &lessdugin_atp::syntax::Did::parse("did:plc:abcdefghijklmnopqrstuvwx").unwrap(),
            "real",
        )
        .unwrap()
        .uri
        .to_string();
        let mut pending = 0;
        for n in 0..(CANDIDATES_PER_HOST as usize + 5) {
            let did = format!("did:plc:c{n:023}");
            pending += usize::from(
                store
                    .add_candidate_from(&real, &did, "cid", 1, "spam.test")
                    .unwrap(),
            );
        }
        assert_eq!(pending as i64, CANDIDATES_PER_HOST);
        assert!(
            store
                .add_candidate_from(
                    &real,
                    "did:plc:dddddddddddddddddddddddd",
                    "cid",
                    1,
                    "ok.test"
                )
                .unwrap()
        );
    }

    #[test]
    fn failing_forums_back_off_alone_and_absent_ones_are_quarantined_then_dropped() {
        let store = Store::in_memory().unwrap();
        store.add_forum_seed(FORUM).unwrap();
        assert_eq!(store.forums_for_pass(0), vec![FORUM.to_owned()]);
        // Exponential back-off from one minute; the forum leaves passes until it is due.
        let first = store.note_forum_failed(FORUM, 1_000, "boom").unwrap();
        assert_eq!(first, 1_000 + RETRY_BASE_MS);
        assert!(store.forums_for_pass(1_001).is_empty());
        assert!(store.forums_due(first - 1).is_empty());
        assert_eq!(store.forums_due(first), vec![FORUM.to_owned()]);
        let second = store.note_forum_failed(FORUM, first, "boom").unwrap();
        assert_eq!(second, first + 2 * RETRY_BASE_MS);
        for _ in 0..40 {
            store.note_forum_failed(FORUM, 0, "boom").unwrap();
        }
        assert_eq!(store.forum_row(FORUM).unwrap().retry_ms, RETRY_MAX_MS);
        store.note_forum_synced(FORUM).unwrap();
        assert_eq!(store.forum_row(FORUM).unwrap().failures, 0);

        // An indexed forum whose declaration is proven absent logs a delete per record and
        // leaves listings; it is rechecked daily and dropped after a week.
        grow(&store, 0, 3);
        store
            .db()
            .execute(
                "UPDATE forums SET synced_ms=1,cid='decl' WHERE uri=?1",
                [FORUM],
            )
            .unwrap();
        assert!(store.forum(FORUM).is_some());
        let before = store.latest_seq();
        assert_eq!(store.note_forum_absent(FORUM, 10).unwrap(), 3);
        let deletes = store.changes(Some(FORUM), before, 10);
        assert!(deletes.iter().all(|c| c.op == "delete" && c.cid.is_none()));
        assert!(store.forum(FORUM).is_none());
        assert!(store.indexed_forums(None).is_empty());
        assert!(store.forums_due(i64::MAX - 1).is_empty());
        assert!(store.forums_for_pass(10 + ABSENT_RECHECK_MS - 1).is_empty());
        assert_eq!(
            store.forums_for_pass(10 + ABSENT_RECHECK_MS),
            vec![FORUM.to_owned()]
        );
        store.note_forum_absent(FORUM, 10 + ABSENT_DROP_MS).unwrap();
        assert!(
            store.forum_row(FORUM).is_none(),
            "an absent seed is dropped"
        );
    }

    #[test]
    fn repo_retries_back_off_exponentially() {
        let store = Store::in_memory().unwrap();
        let at = store
            .schedule_repo_retry("did:plc:x", FORUM, 0, "e")
            .unwrap();
        assert_eq!(at, REPO_RETRY_BASE_MS);
        assert!(store.due_repo_retries(at - 1, 10).is_empty());
        assert_eq!(store.due_repo_retries(at, 10).len(), 1);
        let again = store
            .schedule_repo_retry("did:plc:x", FORUM, at, "e")
            .unwrap();
        assert_eq!(again, at + 2 * REPO_RETRY_BASE_MS);
        store.clear_repo_retry("did:plc:x", FORUM).unwrap();
        assert!(store.repo_retries().is_empty());
    }
}
