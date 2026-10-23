// SPDX-License-Identifier: AGPL-3.0-or-later
//! The fold input (`social.md` §1.1) in its contract form ([`FoldInput`]: `(uri → (cid, value))`)
//! and in the skeleton form the fold reads ([`ForumInput`]: one [`Skel`] per record, bodies
//! dropped, validity decided).
//!
//! ```text
//! FoldInput { decl: (uri, cid, value), records: uri → (cid, value), absent: {uri},
//!             obs: (uri, cid) → µs, repoStatus: did → Fresh|Stale|Unavailable, now: µs,
//!             algorithmIds? }
//! ```
//!
//! Skeletons keep what the fold and the list views need (`sync.md` §13): uri, cid, author,
//! collection, rkey, validity, `forum`, references, kind, title, tags, labels, `createdAt`,
//! `updatedAt`, the TID time and `t_obs`; the small records the fold reads whole (memberships,
//! opinions, modActions) keep their value. Bodies are re-read from the store when a view needs
//! them.
//!
//! The namespace prefix is explicit ([`ForumInput::ns`]): the engine passes this build's `NS`,
//! vector runners the file's `ns`. Skeletons are extracted from the JSON value without typed
//! parsing, so they do not depend on the build namespace.

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_lexicon::Collection;
use serde_json::Value;

use crate::social::time::{self, ItemTimes};
use crate::store::repo::RepoStatus;

/// A record of the contract form.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordInput {
    /// Its CID (canonical string).
    pub cid: String,
    /// Its value (atproto JSON).
    pub value: Value,
}

/// The current declaration.
#[derive(Clone, Debug, PartialEq)]
pub struct DeclInput {
    /// F.
    pub uri: String,
    /// Its CID.
    pub cid: String,
    /// Its value.
    pub value: Value,
}

/// `algorithmIds` (§1.1): the effective behaviour IDs (§1.6) an input was built for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlgorithmIds {
    /// `moderation` (`mod/1`).
    pub moderation: String,
    /// `karma` (`karma/1`).
    pub karma: String,
    /// `ranking` (`magic/1`).
    pub ranking: String,
    /// `reactPalette` (`default/1`).
    pub react_palette: String,
}

/// `FoldInput` (§1.1), contract form.
#[derive(Clone, Debug, PartialEq)]
pub struct FoldInput {
    /// The declaration (admitted and valid).
    pub decl: DeclInput,
    /// One current version per uri.
    pub records: BTreeMap<String, RecordInput>,
    /// Confirmed-absent opinion subjects.
    pub absent: BTreeSet<String>,
    /// First-seen times: `(uri, cid) → µs`.
    pub obs: BTreeMap<(String, String), i64>,
    /// Repo states (a missing DID is `Unavailable`).
    pub repo_status: BTreeMap<String, RepoStatus>,
    /// Now (µs).
    pub now: i64,
    /// `algorithmIds`, when the producer states them; they must equal the effective IDs derived
    /// from `decl` (§1.6), or the input is malformed. `None`: derived from `decl` only.
    pub algorithm_ids: Option<AlgorithmIds>,
}

fn s<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)?.as_str()
}

fn status_of(s: &str) -> Option<RepoStatus> {
    Some(match s {
        "fresh" => RepoStatus::Fresh,
        "stale" => RepoStatus::Stale,
        "unavailable" => RepoStatus::Unavailable,
        _ => return None,
    })
}

impl FoldInput {
    /// Parses the vector JSON form (`testdata/vectors/README.md`, "Social fold vectors").
    pub fn from_vector_json(v: &Value) -> Result<Self, String> {
        let decl = v.get("decl").ok_or("missing decl")?;
        let decl = DeclInput {
            uri: s(decl, "uri").ok_or("decl.uri")?.to_owned(),
            cid: s(decl, "cid").ok_or("decl.cid")?.to_owned(),
            value: decl.get("value").cloned().ok_or("decl.value")?,
        };
        let mut records = BTreeMap::new();
        for r in v
            .get("records")
            .and_then(Value::as_array)
            .ok_or("records")?
        {
            records.insert(
                s(r, "uri").ok_or("record uri")?.to_owned(),
                RecordInput {
                    cid: s(r, "cid").ok_or("record cid")?.to_owned(),
                    value: r.get("value").cloned().ok_or("record value")?,
                },
            );
        }
        let absent = v
            .get("absent")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let mut obs = BTreeMap::new();
        for o in v.get("obs").and_then(Value::as_array).into_iter().flatten() {
            let key = (
                s(o, "uri").ok_or("obs uri")?.to_owned(),
                s(o, "cid").ok_or("obs cid")?.to_owned(),
            );
            let t = o.get("us").and_then(Value::as_i64).ok_or("obs us")?;
            obs.insert(key, t);
        }
        let repo_status = v
            .get("repoStatus")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(d, st)| Some((d.clone(), status_of(st.as_str()?)?)))
                    .collect()
            })
            .unwrap_or_default();
        let now = v.get("now").and_then(Value::as_i64).ok_or("now")?;
        let algorithm_ids = match v.get("algorithmIds") {
            None | Some(Value::Null) => None,
            Some(a) => {
                let id = |k: &str| {
                    s(a, k)
                        .map(str::to_owned)
                        .ok_or(format!("algorithmIds.{k}"))
                };
                Some(AlgorithmIds {
                    moderation: id("moderation")?,
                    karma: id("karma")?,
                    ranking: id("ranking")?,
                    react_palette: id("reactPalette")?,
                })
            }
        };
        Ok(Self {
            decl,
            records,
            absent,
            obs,
            repo_status,
            now,
            algorithm_ids,
        })
    }
}

/// A strong reference `{uri, cid}` read from a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefSkel {
    /// The uri.
    pub uri: String,
    /// The CID.
    pub cid: String,
}

impl RefSkel {
    fn of(v: Option<&Value>) -> Option<Self> {
        let v = v?;
        Some(Self {
            uri: s(v, "uri")?.to_owned(),
            cid: s(v, "cid").unwrap_or_default().to_owned(),
        })
    }
}

/// A post link card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkSkel {
    /// The URL.
    pub uri: String,
    /// Title.
    pub title: Option<String>,
    /// Description.
    pub description: Option<String>,
}

/// What a skeleton keeps beyond the common fields.
#[derive(Clone, Debug, PartialEq)]
pub enum SkelData {
    /// A post.
    Post {
        /// `kind` as written.
        kind: Option<String>,
        /// `title`.
        title: Option<String>,
        /// The author's own tags.
        tags: Vec<String>,
        /// Self-label values.
        labels: Vec<String>,
        /// The link card.
        link: Option<LinkSkel>,
        /// `excerpt` (read when the body is spilled).
        excerpt: Option<String>,
        /// The body is a `textBlob`.
        spilled: bool,
        /// The record has a body.
        has_body: bool,
    },
    /// A comment.
    Comment {
        /// `root`.
        root: Option<RefSkel>,
        /// `parent`.
        parent: Option<RefSkel>,
        /// `retracted`.
        retracted: bool,
        /// `moderatorHat`.
        moderator_hat: bool,
        /// `quote` (a `defs#textQuote` value).
        quote: Option<Value>,
    },
    /// A revision.
    Revision {
        /// `subject`.
        subject: Option<RefSkel>,
        /// `op` as written.
        op: Option<String>,
        /// `summary`.
        summary: Option<String>,
        /// The snapshot `record` is present.
        has_record: bool,
    },
    /// A small record the fold reads whole (membership, opinion, modAction).
    Value(Value),
    /// Nothing more (wikiRevisions and sequences until M5, other collections).
    None,
}

/// A record skeleton.
#[derive(Clone, Debug, PartialEq)]
pub struct Skel {
    /// The uri.
    pub uri: String,
    /// The CID.
    pub cid: String,
    /// `did(uri)`.
    pub did: String,
    /// The collection NSID.
    pub collection: String,
    /// The collection, if it is one of the namespace's.
    pub coll: Option<Collection>,
    /// The rkey.
    pub rkey: String,
    /// Passed `data-model.md` §7.
    pub valid: bool,
    /// The first failing rule.
    pub rule: Option<String>,
    /// `value.forum`.
    pub forum: Option<String>,
    /// `createdAt` as written.
    pub created_at: Option<String>,
    /// `updatedAt` as written.
    pub updated_at: Option<String>,
    /// `t_claim`, `t_tid`, `t_obs`.
    pub times: ItemTimes,
    /// The rest.
    pub data: SkelData,
}

/// Splits `at://did/collection/rkey`.
pub fn split_uri(uri: &str) -> Option<(&str, &str, &str)> {
    lessdugin_lexicon::keys::split_uri(uri).ok()
}

impl Skel {
    /// The skeleton of the record `uri` (in namespace `ns`) with `value`, validity
    /// `(valid, rule)` and first-seen time `obs`.
    pub fn new(
        ns: &str,
        uri: &str,
        cid: &str,
        value: &Value,
        valid: bool,
        rule: Option<String>,
        obs: Option<i64>,
    ) -> Self {
        let (did, collection, rkey) = split_uri(uri).unwrap_or(("", "", ""));
        let coll = Collection::from_nsid_in(ns, collection);
        let created_at = s(value, "createdAt").map(str::to_owned);
        let times = ItemTimes {
            claim: created_at.as_deref().and_then(time::us),
            tid: time::rkey_tid(rkey).and_then(time::tid_time),
            obs,
        };
        let strings = |v: Option<&Value>| -> Vec<String> {
            v.and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        let data = match coll {
            Some(Collection::Post) => {
                let body = value.get("body");
                let link = value.get("link").and_then(|l| {
                    Some(LinkSkel {
                        uri: s(l, "uri")?.to_owned(),
                        title: s(l, "title").map(str::to_owned),
                        description: s(l, "description").map(str::to_owned),
                    })
                });
                let labels = value
                    .get("labels")
                    .and_then(|l| l.get("values"))
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| s(x, "val").map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                SkelData::Post {
                    kind: s(value, "kind").map(str::to_owned),
                    title: s(value, "title").map(str::to_owned),
                    tags: strings(value.get("tags")),
                    labels,
                    link,
                    excerpt: s(value, "excerpt").map(str::to_owned),
                    spilled: body.is_some_and(|b| b.get("textBlob").is_some()),
                    has_body: body.is_some(),
                }
            }
            Some(Collection::Comment) => SkelData::Comment {
                root: RefSkel::of(value.get("root")),
                parent: RefSkel::of(value.get("parent")),
                retracted: value.get("retracted").and_then(Value::as_bool) == Some(true),
                moderator_hat: value.get("moderatorHat").and_then(Value::as_bool) == Some(true),
                quote: value.get("quote").cloned(),
            },
            Some(Collection::Revision) => SkelData::Revision {
                subject: RefSkel::of(value.get("subject")),
                op: s(value, "op").map(str::to_owned),
                summary: s(value, "summary").map(str::to_owned),
                has_record: value.get("record").is_some_and(Value::is_object),
            },
            Some(Collection::Membership | Collection::Opinion | Collection::ModAction) => {
                SkelData::Value(value.clone())
            }
            _ => SkelData::None,
        };
        Self {
            uri: uri.to_owned(),
            cid: cid.to_owned(),
            did: did.to_owned(),
            collection: collection.to_owned(),
            coll,
            rkey: rkey.to_owned(),
            valid,
            rule,
            forum: s(value, "forum").map(str::to_owned),
            created_at,
            updated_at: s(value, "updatedAt").map(str::to_owned),
            times,
            data,
        }
    }

    /// `true` for the candidate items of the visibility rules (post, comment, wikiRevision,
    /// sequence).
    pub fn is_item(&self) -> bool {
        matches!(
            self.coll,
            Some(
                Collection::Post
                    | Collection::Comment
                    | Collection::WikiRevision
                    | Collection::Sequence
            )
        )
    }

    /// The comment's `root` and `parent`, if this is a comment.
    pub fn comment_refs(&self) -> Option<(Option<&RefSkel>, Option<&RefSkel>)> {
        match &self.data {
            SkelData::Comment { root, parent, .. } => Some((root.as_ref(), parent.as_ref())),
            _ => None,
        }
    }

    /// The value kept whole (memberships, opinions, modActions).
    pub fn value(&self) -> Option<&Value> {
        match &self.data {
            SkelData::Value(v) => Some(v),
            _ => None,
        }
    }

    /// `updatedAt` later than `createdAt` (the `[edited]` badge, §11 "Display").
    pub fn updated_later(&self) -> bool {
        match (
            self.updated_at.as_deref().and_then(time::us),
            self.times.claim,
        ) {
            (Some(u), Some(c)) => u > c,
            (Some(_), None) => true,
            _ => false,
        }
    }
}

/// The fold input in skeleton form.
#[derive(Clone, Debug, PartialEq)]
pub struct ForumInput {
    /// The NSID prefix the records are in.
    pub ns: String,
    /// F.
    pub forum: String,
    /// `did(F)`.
    pub owner: String,
    /// `fk(F)`.
    pub fk: String,
    /// The declaration's CID.
    pub decl_cid: String,
    /// The declaration's value.
    pub decl: Value,
    /// `t_obs(decl)`.
    pub decl_obs: Option<i64>,
    /// The records, by uri.
    pub records: BTreeMap<String, Skel>,
    /// `absent`.
    pub absent: BTreeSet<String>,
    /// `repoStatus`.
    pub repo_status: BTreeMap<String, RepoStatus>,
    /// Now (µs).
    pub now: i64,
}

/// `t_obs` per uri: the minimum over its versions.
pub fn obs_by_uri(obs: &BTreeMap<(String, String), i64>) -> BTreeMap<String, i64> {
    let mut out: BTreeMap<String, i64> = BTreeMap::new();
    for ((u, _), t) in obs {
        out.entry(u.clone())
            .and_modify(|e| *e = (*e).min(*t))
            .or_insert(*t);
    }
    out
}

impl ForumInput {
    /// The skeleton form of `input` (records in namespace `ns`); `validity(uri, value)` decides
    /// `data-model.md` §7 for each record: `(valid, first failing rule)`.
    pub fn from_contract(
        input: &FoldInput,
        ns: &str,
        validity: impl Fn(&str, &Value) -> (bool, Option<String>),
    ) -> Result<Self, String> {
        let (owner, _, _) = split_uri(&input.decl.uri).ok_or("decl uri")?;
        let fk = lessdugin_lexicon::keys::fk(&input.decl.uri).map_err(|e| e.to_string())?;
        let obs = obs_by_uri(&input.obs);
        let records = input
            .records
            .iter()
            .map(|(u, r)| {
                let (valid, rule) = validity(u, &r.value);
                let sk = Skel::new(ns, u, &r.cid, &r.value, valid, rule, obs.get(u).copied());
                (u.clone(), sk)
            })
            .collect();
        Ok(Self {
            ns: ns.to_owned(),
            forum: input.decl.uri.clone(),
            owner: owner.to_owned(),
            fk,
            decl_cid: input.decl.cid.clone(),
            decl: input.decl.value.clone(),
            decl_obs: obs.get(&input.decl.uri).copied(),
            records,
            absent: input.absent.clone(),
            repo_status: input.repo_status.clone(),
            now: input.now,
        })
    }

    /// The repo status of `did` (missing: `Unavailable`).
    pub fn status(&self, did: &str) -> RepoStatus {
        self.repo_status
            .get(did)
            .copied()
            .unwrap_or(RepoStatus::Unavailable)
    }
}
