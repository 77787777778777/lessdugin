// SPDX-License-Identifier: AGPL-3.0-or-later
//! Building a [`ForumModel`] from the record store (`sync.md` §13).
//!
//! ```text
//! rows  = scan(cache, ("rec", fk))                     // one prefix scan
//!       ∪ the declaration row ("rec", "_", NS.forum, owner, slug)
//! caps  : fold_rows_capped (the first N(coll) keys by rkey descending per (did, coll)), and
//!         nothing below a listing's recorded cap boundary (below_cap_boundary)
//! input : records    = eligible rows (§1.5) → skeletons (bodies dropped)
//!         absent     = §5.6.4 for the opinion subjects
//!         obs        = t_obs per uri (firstSeenUri), or the injected parity time
//!         repoStatus = repoStatus(d, fk) for every record owner and authority
//!         now        = injected (tests, parity mode), or the host clock
//! fold  : crate::social::fold
//! ```

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_atp::syntax::Did;
use lessdugin_lexicon::ns::NS;
use lessdugin_lexicon::records::Forum;

use crate::discovery::forum_ref::ForumKey;
use crate::discovery::roster;
use crate::host::KvStore;
use crate::social::fold::{FoldError, FoldResult, fold};
use crate::social::input::{ForumInput, Skel, SkelData};
use crate::store::records::{
    Level, RecordStore, StoreError, below_cap_boundary, eligible, fold_rows_capped,
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
    /// The fold failed.
    #[error(transparent)]
    Fold(#[from] FoldError),
}

/// Store metadata of one record the views need.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowMeta {
    /// The trust level.
    pub level: Level,
}

/// The in-memory model of one forum: the fold input in skeleton form and the fold result.
/// Immutable once built; the engine rebuilds it after a `Changed` event for the forum.
#[derive(Clone, Debug, PartialEq)]
pub struct ForumModel {
    /// F, owner, slug, fk.
    pub key: ForumKey,
    /// The fold input.
    pub input: ForumInput,
    /// The fold result.
    pub fold: FoldResult,
    /// The declaration's trust level.
    pub decl_level: Level,
    /// Store metadata per record uri.
    pub rows: BTreeMap<String, RowMeta>,
    /// Repos whose last sync attempt failed (the `[stale]` badge with a non-`Fresh` status).
    pub failing: BTreeSet<String>,
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

        let mut records: BTreeMap<String, Skel> = BTreeMap::new();
        let mut meta: BTreeMap<String, RowMeta> = BTreeMap::new();
        for r in &rows {
            let repo = repos.get(&r.key.did);
            if !eligible(&r.row, repo) || below_cap_boundary(&key.fk, &r.key, repo) {
                continue;
            }
            let uri = r.uri();
            let Ok(value) = r.row.value_json() else {
                continue;
            };
            let obs = fixed_obs_us.unwrap_or_else(|| r.row.first_seen_uri.min(r.row.first_seen));
            let sk = Skel::new(
                NS,
                &uri,
                &r.row.cid.to_string(),
                &value,
                r.row.valid,
                r.row.rule.clone(),
                Some(obs),
            );
            meta.insert(uri.clone(), RowMeta { level: r.row.level });
            records.insert(uri, sk);
        }

        let subjects: Vec<String> = records
            .values()
            .filter_map(|x| match &x.data {
                SkelData::Value(v) if x.coll == Some(lessdugin_lexicon::Collection::Opinion) => {
                    v.get("subject")?.get("uri")?.as_str().map(str::to_owned)
                }
                _ => None,
            })
            .collect();
        let absent = store
            .absent(&key.fk, subjects.iter().map(String::as_str))
            .await?;

        let owners: BTreeSet<&str> = records
            .values()
            .map(|x| x.did.as_str())
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

        let input = ForumInput {
            ns: NS.to_owned(),
            forum: key.uri.to_string(),
            owner: key.owner.to_string(),
            fk: key.fk.clone(),
            decl_cid: decl_row.cid.to_string(),
            decl: decl_value,
            decl_obs: Some(
                fixed_obs_us.unwrap_or_else(|| decl_row.first_seen_uri.min(decl_row.first_seen)),
            ),
            records,
            absent,
            repo_status,
            now: now_us,
        };
        Self::from_input(key.clone(), input, decl_row.level, meta, failing)
    }

    /// A model over an explicit fold input (tests and parity scenarios inject `now` and
    /// `obs` this way).
    pub fn from_input(
        key: ForumKey,
        input: ForumInput,
        decl_level: Level,
        rows: BTreeMap<String, RowMeta>,
        failing: BTreeSet<String>,
    ) -> Result<Self, ModelError> {
        let fold = fold(&input)?;
        Ok(Self {
            key,
            input,
            fold,
            decl_level,
            rows,
            failing,
        })
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

    /// `true` if `did` has a valid membership record of the forum.
    pub fn is_member(&self, did: &str) -> bool {
        self.fold.roster.members.contains(did)
    }

    /// The `[stale]` condition for an author (`sync.md` §1.6): not `Fresh`, and the last sync
    /// attempt failed.
    pub fn stale(&self, did: &str) -> bool {
        self.input.status(did) != RepoStatus::Fresh && self.failing.contains(did)
    }
}
