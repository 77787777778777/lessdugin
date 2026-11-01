// SPDX-License-Identifier: AGPL-3.0-or-later
//! Durable indexer side tables. They share the native durable SQLite database but never replace
//! or duplicate the core record store.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use lessdugin_atp::syntax::AtUri;
use lessdugin_core::model::ForumModel;
use rusqlite::{Connection, OptionalExtension, params};

#[derive(Clone)]
pub struct Store(Arc<Mutex<Connection>>);

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

impl Store {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        migrate_candidates(&conn)?;
        Ok(Self(Arc::new(Mutex::new(conn))))
    }

    pub fn in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        migrate_candidates(&conn)?;
        Ok(Self(Arc::new(Mutex::new(conn))))
    }

    fn db(&self) -> MutexGuard<'_, Connection> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn add_forum_seed(&self, forum: &str) -> rusqlite::Result<()> {
        let uri = AtUri::parse(forum)
            .map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))?;
        self.db().execute(
            "INSERT INTO forums(uri,did,rkey,name,created_at,cid,synced_ms,input_digest) VALUES(?1,?2,?3,'','','',0,'') ON CONFLICT(uri) DO NOTHING",
            params![uri.to_string(), uri.did().as_str(), uri.rkey().as_str()],
        )?;
        Ok(())
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
        lessdugin_core::discovery::ForumKey::of_uri(
            &AtUri::parse(forum)
                .map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))?,
        )
        .map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))?;
        lessdugin_atp::syntax::Did::parse(did)
            .map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))?;
        self.db().execute(
            "INSERT INTO forum_candidates(forum,did,cid,created_ms,state,observed_ms) VALUES(?1,?2,?3,?4,'pending',0) ON CONFLICT(forum,did) DO UPDATE SET cid=excluded.cid,created_ms=excluded.created_ms,state=CASE WHEN forum_candidates.state='rejected' AND forum_candidates.cid=excluded.cid THEN 'rejected' WHEN forum_candidates.cid<>excluded.cid OR forum_candidates.state='confirmed' THEN 'pending' ELSE forum_candidates.state END WHERE forum_candidates.cid<>excluded.cid OR forum_candidates.created_ms<excluded.created_ms OR forum_candidates.state='confirmed'",
            params![forum, did, cid, created_ms],
        )?;
        Ok(())
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

    pub fn forums(&self, q: Option<&str>) -> Vec<ForumRow> {
        let db = self.db();
        let pattern = q.map(|s| format!("%{}%", s.replace(['%', '_'], "")));
        let sql = "SELECT uri,did,name,description,created_at,cid,synced_ms,input_digest FROM forums WHERE (?1 IS NULL OR uri LIKE ?1 OR name LIKE ?1) ORDER BY uri";
        let mut q = db.prepare(sql).unwrap();
        q.query_map([&pattern], |r| {
            let uri: String = r.get(0)?;
            let participants = db
                .query_row(
                    "SELECT COUNT(*) FROM participants WHERE forum=?1",
                    [&uri],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap_or(0) as usize;
            let posts = db
                .query_row(
                    "SELECT COUNT(*) FROM record_state WHERE forum=?1 AND uri LIKE '%.post/%'",
                    [&uri],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap_or(0) as usize;
            Ok(ForumRow {
                uri,
                did: r.get(1)?,
                name: r.get(2)?,
                description: r.get(3)?,
                created_at: r.get(4)?,
                cid: r.get(5)?,
                synced_ms: r.get(6)?,
                input_digest: r.get(7)?,
                participants,
                posts,
            })
        })
        .unwrap()
        .filter_map(Result::ok)
        .collect()
    }

    pub fn forum(&self, uri: &str) -> Option<ForumRow> {
        self.forums(None)
            .into_iter()
            .find(|f| f.uri == uri && !f.synced_ms.eq(&0))
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
        self.db().execute("INSERT INTO repo_heads(did,rev,commit_cid,pds,checked_ms,status) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(did) DO UPDATE SET rev=excluded.rev,commit_cid=excluded.commit_cid,pds=excluded.pds,checked_ms=excluded.checked_ms,status=excluded.status", params![row.did,row.rev,row.commit,row.pds,row.checked_ms,row.status])?;
        Ok(())
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

    /// Atomically replaces the derived forum snapshot and appends its differences to the log.
    pub fn apply_model(
        &self,
        m: &ForumModel,
        now_ms: i64,
        repo_revs: &BTreeMap<String, String>,
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
        let mut new: BTreeMap<String, String> = m
            .input
            .records
            .iter()
            .map(|(u, s)| (u.clone(), s.cid.clone()))
            .collect();
        new.insert(m.key.uri.to_string(), m.input.decl_cid.clone());
        tx.execute("DELETE FROM record_state WHERE forum=?1", [&forum])?;
        tx.execute("DELETE FROM participants WHERE forum=?1", [&forum])?;
        {
            let mut ins = tx.prepare("INSERT INTO record_state(forum,uri,cid) VALUES(?1,?2,?3)")?;
            for (u, c) in &new {
                ins.execute(params![forum, u, c])?;
            }
        }
        {
            let mut ins = tx.prepare("INSERT INTO participants(forum,did,status,rev,last_seen_ms) VALUES(?1,?2,?3,NULL,?4)")?;
            for did in m.participants() {
                let status = match m.rank(did) {
                    3 => "owner",
                    2 => "admin",
                    1 => "moderator",
                    _ if m.view.participation.s.contains(did) => "established",
                    _ if m.view.participation.members.contains(did) => "member",
                    _ => "newcomer",
                };
                ins.execute(params![forum, did, status, now_ms])?;
            }
        }
        let mut changes = 0;
        let mut log = tx.prepare(
            "INSERT INTO change_log(forum,uri,cid,did,rev,op,at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)",
        )?;
        for (u, c) in &new {
            if old.get(u) != Some(c) {
                let did = AtUri::parse(u)
                    .map(|u| u.did().to_string())
                    .unwrap_or_default();
                let op = if old.contains_key(u) {
                    "update"
                } else {
                    "create"
                };
                let rev = repo_revs.get(&did).cloned().unwrap_or_default();
                log.execute(params![forum, u, c, did, rev, op, now_ms])?;
                changes += 1;
            }
        }
        for u in old.keys().filter(|u| !new.contains_key(*u)) {
            let did = AtUri::parse(u)
                .map(|u| u.did().to_string())
                .unwrap_or_default();
            let rev = repo_revs.get(&did).cloned().unwrap_or_default();
            log.execute(params![
                forum,
                u,
                Option::<String>::None,
                did,
                rev,
                "delete",
                now_ms
            ])?;
            changes += 1;
        }
        drop(log);
        tx.execute("INSERT INTO forums(uri,did,rkey,name,description,created_at,cid,synced_ms,input_digest) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(uri) DO UPDATE SET name=excluded.name,description=excluded.description,created_at=excluded.created_at,cid=excluded.cid,synced_ms=excluded.synced_ms,input_digest=excluded.input_digest", params![forum,m.key.owner.as_str(),m.key.uri.rkey().as_str(),m.decl.name,m.decl.description,m.decl.created_at.as_str(),m.input.decl_cid,now_ms,m.view.input_digest])?;
        tx.commit()?;
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
        let did =
            AtUri::parse(uri).map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))?;
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
}

fn migrate_candidates(conn: &Connection) -> rusqlite::Result<()> {
    let columns = conn
        .prepare("PRAGMA table_info(forum_candidates)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    for (name, declaration) in [
        ("cid", "TEXT NOT NULL DEFAULT ''"),
        ("created_ms", "INTEGER NOT NULL DEFAULT 0"),
        ("state", "TEXT NOT NULL DEFAULT 'pending'"),
        ("observed_ms", "INTEGER NOT NULL DEFAULT 0"),
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
CREATE TABLE IF NOT EXISTS repo_heads(
 did TEXT PRIMARY KEY,rev TEXT,commit_cid TEXT,pds TEXT,checked_ms INTEGER NOT NULL,status TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS record_state(forum TEXT NOT NULL,uri TEXT NOT NULL,cid TEXT NOT NULL,PRIMARY KEY(forum,uri));
CREATE TABLE IF NOT EXISTS change_log(
 seq INTEGER PRIMARY KEY AUTOINCREMENT,forum TEXT NOT NULL,uri TEXT NOT NULL,cid TEXT,did TEXT NOT NULL,
 rev TEXT NOT NULL,op TEXT NOT NULL,at_ms INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS change_log_forum ON change_log(forum,seq);
CREATE INDEX IF NOT EXISTS change_log_time ON change_log(at_ms);
CREATE TABLE IF NOT EXISTS proof_cache(
 uri TEXT PRIMARY KEY,rev TEXT NOT NULL,car BLOB NOT NULL,commit_cid TEXT NOT NULL,cid TEXT,verified_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS cursors(name TEXT PRIMARY KEY,value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS schedule(name TEXT PRIMARY KEY,value INTEGER NOT NULL);
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
}
