// SPDX-License-Identifier: AGPL-3.0-or-later
//! Building a [`ForumModel`] from the record store (`sync.md` §13) and folding it with the full
//! social fold (`social.md` §1, [`crate::social::full`]).
//!
//! ```text
//! rows   = scan(cache, ("rec", fk))                     // one prefix scan
//!        ∪ the declaration row ("rec", "_", NS.forum, owner, slug)
//! caps   : fold_rows_capped (the first N(coll) keys by rkey descending per (did, coll)), and
//!          nothing below a listing's recorded cap boundary (below_cap_boundary)
//! input  : FoldInput (§1.1), well-formed by construction:
//!          decl      = (F, cid, value) of the admitted, valid declaration row
//!          records   = eligible rows (§1.5): memberships with rkey fk ∪ the forum-scoped
//!                      collections of the "{fk}-" range; values without bodies or revision
//!                      snapshots (no layer reads them; `inputDigest` reads only (uri, cid))
//!          absent    = §5.6.4 for the subjects of the opinions in records, minus records
//!          obs       = the stored first-seen times (obs_entries), or the injected parity time
//!          repoStatus= repoStatus(d, fk) for every record owner and authority
//!          now       = injected (tests, parity mode), or the host clock
//! fold   : Cx::new with the *stored* validity (each row's `valid`/`rule`, decided at admission;
//!          not recomputed), then fold_cx(cx, Query::default()); the model keeps cx's skeletons
//!          and typed declaration, and re-ranks other queries with full::reorder (no refold)
//! ```

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_atp::syntax::Did;
use lessdugin_lexicon::Collection;
use lessdugin_lexicon::ns::NS;
use lessdugin_lexicon::records::Forum;
use serde_json::Value;

use crate::discovery::forum_ref::ForumKey;
use crate::discovery::roster;
use crate::host::KvStore;
use crate::social::FoldError;
use crate::social::full::gates::{Gate, passes};
use crate::social::full::ranking::Orderings;
use crate::social::full::view::{Pending, State};
use crate::social::full::{self, Cx, ForumView, Query};
use crate::social::input::{DeclInput, FoldInput, ForumInput, RecordInput, Skel, SkelData};
use crate::store::records::{
    Level, RecordStore, StoreError, below_cap_boundary, eligible, fold_rows_capped, obs_entries,
};
use crate::store::repo::{RepoRow, RepoStatus};

/// Why a model could not be built.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum ModelError {
    /// The store failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The declaration is not in the cache (open the forum first).
    #[error("forum {0} is not in the cache")]
    NoDeclaration(String),
    /// The cached declaration is invalid.
    #[error("forum {uri} is invalid ({rule})")]
    InvalidDeclaration {
        /// F.
        uri: String,
        /// The first failing rule.
        rule: String,
    },
    /// The fold failed (a malformed input, which the builder never produces).
    #[error(transparent)]
    Fold(#[from] FoldError),
}

/// Store metadata of one record the views need.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowMeta {
    /// The trust level.
    pub level: Level,
}

/// The in-memory model of one forum: the fold input in skeleton form and the full fold's view
/// for the base query ([`Query::default`]: no post list or thread; Recent Discussion included).
/// Immutable once built; the engine rebuilds it after a `Changed` event for the forum, and
/// answers other queries with [`ForumModel::orderings`] over the cached layers.
#[derive(Clone, Debug, PartialEq)]
pub struct ForumModel {
    /// F, owner, slug, fk.
    pub key: ForumKey,
    /// The fold input in skeleton form (the prepared [`Cx`]'s): validity, references, times
    /// (`Skel::times.obs` is `t_obs`, µs), and the small records' values (memberships, opinions,
    /// modActions).
    pub input: ForumInput,
    /// The full fold (`social.md` §1.1) for [`Query::default`].
    pub view: ForumView,
    /// The declaration, typed.
    pub decl: Forum,
    /// The declaration's trust level.
    pub decl_level: Level,
    /// Store metadata per record uri.
    pub rows: BTreeMap<String, RowMeta>,
    /// Repos whose last sync attempt failed (the `[stale]` badge with a non-`Fresh` status).
    pub failing: BTreeSet<String>,
}

/// A value as the fold input carries it: what the skeletons and the layers read, without the
/// bodies (`body` becomes `{}`, or `{"textBlob": {}}` when it is spilled) and without revision
/// snapshots (`record` becomes `{}`). No layer reads either (`social.md` §1.4), and
/// `inputDigest` reads only `(uri, cid)`.
fn fold_value(coll: Option<Collection>, mut v: Value) -> Value {
    match coll {
        Some(Collection::Post | Collection::Comment | Collection::WikiRevision) => {
            if let Some(b) = v.get_mut("body") {
                let spilled = b.get("textBlob").is_some();
                *b = if spilled {
                    serde_json::json!({ "textBlob": {} })
                } else {
                    serde_json::json!({})
                };
            }
        }
        Some(Collection::Revision) => {
            if let Some(r) = v.get_mut("record")
                && r.is_object()
            {
                *r = serde_json::json!({});
            }
        }
        _ => {}
    }
    v
}

/// The fold input's membership condition (§1.1 "records"): a membership with rkey fk, or a
/// forum-scoped collection in the `{fk}-` range.
fn in_input_sets(coll: Option<Collection>, rkey: &str, fk: &str) -> bool {
    match coll {
        Some(Collection::Membership) => rkey == fk,
        Some(c) => {
            c.is_forum_scoped()
                && rkey.len() > fk.len()
                && rkey.starts_with(fk)
                && rkey.as_bytes()[fk.len()] == b'-'
        }
        None => false,
    }
}

impl ForumModel {
    /// Builds the model of forum `key` from the cache at `now_us`.
    pub async fn build<K: KvStore>(
        kv: &K,
        key: &ForumKey,
        now_us: i64,
    ) -> Result<Self, ModelError> {
        Self::build_with(kv, key, now_us, None).await
    }

    /// [`ForumModel::build`], with every admitted version's (and the declaration's) first-seen
    /// time replaced by `fixed_obs_us` when it is given (parity mode, `engine-api.md` §2.1):
    /// two caches holding the same records then fold to the same result whenever they saw them.
    pub async fn build_with<K: KvStore>(
        kv: &K,
        key: &ForumKey,
        now_us: i64,
        fixed_obs_us: Option<i64>,
    ) -> Result<Self, ModelError> {
        let store = RecordStore::new(kv);
        let decl_row = store
            .get(&key.uri)
            .await?
            .ok_or_else(|| ModelError::NoDeclaration(key.uri.to_string()))?;
        let invalid = |rule: String| ModelError::InvalidDeclaration {
            uri: key.uri.to_string(),
            rule,
        };
        if !decl_row.valid {
            return Err(invalid(decl_row.rule.clone().unwrap_or_default()));
        }
        let decl_value = decl_row.value_json().map_err(|e| invalid(e.to_string()))?;
        let decl: Forum =
            serde_json::from_value(decl_value.clone()).map_err(|e| invalid(e.to_string()))?;
        let authorities = roster::authorities(&key.owner, &decl);

        let rows = fold_rows_capped(&key.fk, store.scan_forum(&key.fk, None).collect().await?);
        let dids: BTreeSet<&str> = rows
            .iter()
            .map(|r| r.key.did.as_str())
            .chain(authorities.keys().map(String::as_str))
            .collect();
        let dids: Vec<&str> = dids.into_iter().collect();
        let repos: BTreeMap<String, RepoRow> = dids
            .iter()
            .zip(store.repos(&dids).await?)
            .filter_map(|(d, r)| Some(((*d).to_owned(), r?)))
            .collect();

        let mut records: BTreeMap<String, RecordInput> = BTreeMap::new();
        let mut validity: BTreeMap<String, (bool, Option<String>)> = BTreeMap::new();
        let mut obs: BTreeMap<(String, String), i64> = BTreeMap::new();
        let mut meta: BTreeMap<String, RowMeta> = BTreeMap::new();
        let mut subjects: BTreeSet<String> = BTreeSet::new();
        for r in &rows {
            let repo = repos.get(&r.key.did);
            if !eligible(&r.row, repo) || below_cap_boundary(&key.fk, &r.key, repo) {
                continue;
            }
            let uri = r.uri();
            let coll = Collection::from_nsid_in(NS, &r.key.collection);
            if !in_input_sets(coll, &r.key.rkey, &key.fk) {
                continue;
            }
            let Ok(value) = r.row.value_json() else {
                continue;
            };
            if coll == Some(Collection::Opinion)
                && let Some(s) = value
                    .get("subject")
                    .and_then(|s| s.get("uri"))
                    .and_then(Value::as_str)
            {
                subjects.insert(s.to_owned());
            }
            for (k, t) in obs_entries(&uri, &r.row) {
                let t = fixed_obs_us.unwrap_or(t);
                obs.entry(k).and_modify(|e| *e = (*e).min(t)).or_insert(t);
            }
            validity.insert(uri.clone(), (r.row.valid, r.row.rule.clone()));
            meta.insert(uri.clone(), RowMeta { level: r.row.level });
            records.insert(
                uri,
                RecordInput {
                    cid: r.row.cid.to_string(),
                    value: fold_value(coll, value),
                },
            );
        }
        // §1.1: confirmed absent, subjects of opinions in records, not themselves in records
        let absent: BTreeSet<String> = store
            .absent(&key.fk, subjects.iter().map(String::as_str))
            .await?
            .into_iter()
            .filter(|u| !records.contains_key(u) && subjects.contains(u))
            .collect();

        let owners: BTreeSet<&str> = records
            .keys()
            .filter_map(|u| crate::social::input::split_uri(u).map(|(d, _, _)| d))
            .chain(authorities.keys().map(String::as_str))
            .collect();
        let repo_status: BTreeMap<String, RepoStatus> = owners
            .iter()
            .map(|d| {
                let st = repos
                    .get(*d)
                    .map_or(RepoStatus::Unavailable, |r| r.repo_status(&key.fk));
                ((*d).to_owned(), st)
            })
            .collect();
        let failing = repos
            .iter()
            .filter(|(_, r)| r.fails > 0)
            .map(|(d, _)| d.clone())
            .collect();

        let decl_cid = decl_row.cid.to_string();
        for (k, t) in obs_entries(key.uri.as_str(), &decl_row) {
            let t = fixed_obs_us.unwrap_or(t);
            obs.entry(k).and_modify(|e| *e = (*e).min(t)).or_insert(t);
        }
        validity.insert(key.uri.to_string(), (true, None));
        let input = FoldInput {
            decl: DeclInput {
                uri: key.uri.to_string(),
                cid: decl_cid,
                value: decl_value,
            },
            records,
            absent,
            obs,
            repo_status,
            now: now_us,
            algorithm_ids: None,
        };
        let stored = |uri: &str, _: &Value| -> (bool, Option<String>) {
            validity
                .get(uri)
                .cloned()
                .unwrap_or_else(|| (false, Some("missing".into())))
        };
        Self::from_contract(key.clone(), &input, &stored, decl_row.level, meta, failing)
    }

    /// A model over an explicit fold input in contract form with record validity `validity`
    /// (tests and parity scenarios inject `now` and `obs` this way; the engine passes the
    /// stored validity).
    pub fn from_contract(
        key: ForumKey,
        input: &FoldInput,
        validity: full::Validity<'_>,
        decl_level: Level,
        rows: BTreeMap<String, RowMeta>,
        failing: BTreeSet<String>,
    ) -> Result<Self, ModelError> {
        let cx = Cx::new(input, NS, validity)?;
        let view = full::fold_cx(&cx, &Query::default())?;
        let Cx { skel, decl, .. } = cx;
        Ok(Self {
            key,
            input: skel,
            view,
            decl,
            decl_level,
            rows,
            failing,
        })
    }

    /// The orderings of `query` (`social.md` §12) from the cached layers, without folding
    /// again ([`full::reorder`]): the same result as a fold with `query`.
    pub fn orderings(&self, query: &Query) -> Result<Orderings, Pending> {
        full::reorder(&self.view, &self.input, query)
    }

    /// The owner DID.
    pub fn owner(&self) -> &Did {
        &self.key.owner
    }

    /// The skeleton of `uri`.
    pub fn skel(&self, uri: &str) -> Option<&Skel> {
        self.input.records.get(uri)
    }

    /// `now` of the fold (µs).
    pub fn now(&self) -> i64 {
        self.input.now
    }

    /// `t_obs(uri)` (µs): the first-seen time of the record, or the injected parity time.
    pub fn obs(&self, uri: &str) -> Option<i64> {
        self.skel(uri)?.times.obs
    }

    /// `true` if `did` has a valid membership record of the forum (`member(d)`, §3.1).
    pub fn is_member(&self, did: &str) -> bool {
        self.view.participation.members.contains(did)
    }

    /// `true` if `did` is an authority (L0).
    pub fn is_authority(&self, did: &str) -> bool {
        self.view.is_authority(did)
    }

    /// The authority rank of `did` (0: none; 1 moderator, 2 admin, 3 owner).
    pub fn rank(&self, did: &str) -> u8 {
        self.view.authorities.get(did).copied().unwrap_or(0)
    }

    /// P, the participants (§3.1).
    pub fn participants(&self) -> &BTreeSet<String> {
        &self.view.participation.p
    }

    /// The displayed state of a candidate item after the thread pass (§10.2); `None` when the
    /// item is unknown or the visibility layer is pending.
    pub fn displayed(&self, uri: &str) -> Option<&State> {
        self.view.visibility.as_ref().ok()?.displayed.get(uri)
    }

    /// The state of a candidate item before the thread pass (V1–V10).
    pub fn state(&self, uri: &str) -> Option<&State> {
        self.view.visibility.as_ref().ok()?.states.get(uri)
    }

    /// `K[did]` (0 when unknown or pending).
    pub fn k(&self, did: &str) -> i64 {
        self.view.karma.as_ref().map_or(0, |k| k.k_of(did))
    }

    /// `did ∈ E`, the eligible voters (§6.2).
    pub fn in_e(&self, did: &str) -> bool {
        self.view.karma.as_ref().is_ok_and(|k| k.e.contains(did))
    }

    /// `passes(did, g)` (§8) for any DID (`false` while karma is pending, unless `did` is an
    /// authority).
    pub fn passes(&self, did: &str, g: Gate) -> bool {
        match &self.view.karma {
            Ok(k) => passes(&self.view.policy, &self.view.authorities, k, did, g),
            Err(_) => self.is_authority(did),
        }
    }

    /// The `[stale]` condition for an author (`sync.md` §1.6): not `Fresh`, and the last sync
    /// attempt failed.
    pub fn stale(&self, did: &str) -> bool {
        self.input.status(did) != RepoStatus::Fresh && self.failing.contains(did)
    }

    /// The comment's root post uri as written (its record's `root`), if `uri` is a comment.
    pub fn comment_root(&self, uri: &str) -> Option<&str> {
        match &self.skel(uri)?.data {
            SkelData::Comment { root: Some(r), .. } => Some(r.uri.as_str()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_values_drop_bodies_and_snapshots_only() {
        let post = serde_json::json!({"title": "t", "body": {"text": "secret"}, "tags": ["a"]});
        assert_eq!(
            fold_value(Some(Collection::Post), post),
            serde_json::json!({"title": "t", "body": {}, "tags": ["a"]})
        );
        let spilled = serde_json::json!({"body": {"textBlob": {"ref": "x"}}});
        assert_eq!(
            fold_value(Some(Collection::Comment), spilled),
            serde_json::json!({"body": {"textBlob": {}}})
        );
        let rev = serde_json::json!({"op": "update", "record": {"body": "old"}});
        assert_eq!(
            fold_value(Some(Collection::Revision), rev),
            serde_json::json!({"op": "update", "record": {}})
        );
        let op = serde_json::json!({"karma": "up", "body": "kept"});
        assert_eq!(fold_value(Some(Collection::Opinion), op.clone()), op);
    }

    #[test]
    fn input_sets() {
        assert!(in_input_sets(Some(Collection::Membership), "abc", "abc"));
        assert!(!in_input_sets(Some(Collection::Membership), "abc-x", "abc"));
        assert!(in_input_sets(Some(Collection::Post), "abc-3l2", "abc"));
        assert!(!in_input_sets(Some(Collection::Post), "abcd-3l2", "abc"));
        assert!(!in_input_sets(Some(Collection::Post), "abc", "abc"));
        assert!(!in_input_sets(None, "abc-3l2", "abc"));
    }
}
