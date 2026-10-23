// SPDX-License-Identifier: AGPL-3.0-or-later
//! Generated forums for the social fold's property, performance and parity tests
//! (`social_props.rs`, `social_perf.rs`, `social_parity.rs`; `social.md` §6.3, §19).
//!
//! A [`Spec`] (a proptest value, so failures shrink) is built into a [`Forum`]: the declaration
//! and every record of one forum, each under its canonical key (`lessdugin_lexicon::keys`) with
//! its genuine CIDv1 (dag-cbor, sha256). [`Forum::input`] turns any subset of the records into a
//! well-formed `FoldInput` (§1.1): `absent` holds exactly the confirmed-deleted uris that some
//! kept opinion names, `obs` and `repoStatus` cover exactly the kept records and the authorities.
//!
//! ```text
//! users     : 0 owner, 1 admin, 2 moderator, 3.. plain users (membership, forum #access and
//!             #restrict by an authority or a lapsed actor, repo status)
//! posts     : kinds, author tags, edits (an `op: update` revision with the old snapshot),
//!             rare invalid ones (PO3, or G9: `forum` names another forum), deleted ones
//! comments  : any root (live, deleted, never synced), any parent (cycles, cross-root, missing)
//! opinions  : karma, agreement, reacts (palette, unknown ids, stances, quotes), tag votes, on
//!             every item kind and on deleted and unsynced uris; some cite an earlier version
//! modActions: forum scope (hide, keep, access, restrict, tier, pin, lock, tag) by authorities
//!             or lapsed actors, author scope (hide, ban, thread pin, lock); stale CIDs, `until`
//! wiki      : wikiRevisions with parents (forks, cycles), sequences
//! ```
//!
//! Every record the spec does not mean to be invalid is valid under the build's lexicon
//! ([`check_validity`] asserts it), so a generator slip cannot silently weaken a property.

#![allow(dead_code)] // each test crate uses a part

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_atp::datamodel::record_cid;
use lessdugin_atp::tid::Tid;
use lessdugin_core::social::full::project::FIELDS;
use lessdugin_core::social::full::query::{
    CommentQuery, CommentSort, PostFilter, PostQuery, PostSort,
};
use lessdugin_core::social::full::{ForumView, Query, fold_view, project};
use lessdugin_core::social::input::FoldInput;
use lessdugin_lexicon::keys;
use lessdugin_lexicon::ns::NS;
use proptest::prelude::*;
use serde_json::{Map, Value, json};

/// 2026-01-01T00:00:00Z (µs).
pub const T0: i64 = 1_767_225_600_000_000;
/// One minute (µs).
pub const MINUTE: i64 = 60_000_000;
/// One day (µs).
pub const DAY: i64 = 86_400_000_000;
/// Item times stay within January 2026 ([`at`]).
pub const MAX_MINUTE: i64 = 29 * 1440;
/// Users of a generated forum: 0 owner, 1 admin, 2 moderator, the rest plain users.
pub const USERS: usize = 10;
/// Tag and wiki slugs.
pub const TAGS: [&str; 3] = ["t", "u", "w"];
/// `karma` values (the last is unknown: OP3 drops the axis).
pub const KARMA: [&str; 5] = ["up", "down", "strongUp", "strongDown", "superUp"];
/// `agreement` values (the last is unknown).
pub const AGREEMENT: [&str; 5] = ["agree", "disagree", "strongAgree", "strongDisagree", "meh"];
/// React ids: three of the `default/1` palette and one outside it.
pub const REACTS: [&str; 4] = ["insightful", "crux", "typo", "not-a-palette-id"];
/// React stances (the last is unknown).
pub const STANCES: [&str; 3] = ["for", "against", "sideways"];
/// Quotes of anchored reacts (index 0: none).
pub const QUOTES: [&str; 3] = ["", "alpha", "beta"];
/// `#restrict` capabilities.
pub const CAPS: [&str; 7] = [
    "post", "comment", "vote", "react", "tag", "wiki", "sequence",
];

const B32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// A did:plc (24 base32 characters): `tag`, then `i` in 7 base32 digits.
pub fn plc(tag: u8, i: usize) -> String {
    let mut digits = [b'a'; 7];
    let mut n = i;
    for d in digits.iter_mut().rev() {
        *d = B32[n % 32];
        n /= 32;
    }
    format!(
        "did:plc:{}{}{}",
        char::from(tag),
        std::str::from_utf8(&digits).unwrap(),
        "2".repeat(16)
    )
}

/// User `i` of a generated forum (0 is the owner).
pub fn user(i: usize) -> String {
    plc(b'u', i)
}

/// `T0 + minute` as an atproto datetime (`minute` < 31 days).
pub fn at(minute: i64) -> String {
    let m = minute.clamp(0, 30 * 1440 + 1439);
    format!(
        "2026-01-{:02}T{:02}:{:02}:00.000Z",
        m / 1440 + 1,
        (m % 1440) / 60,
        m % 60
    )
}

/// A CID that is no record's (stale `subjectCid`s, parents named before they exist).
pub const FOREIGN_CID: &str = "bafyreibox5tmitcjfmeurxwythqcq5cazsbifbkr56hu4752nd7erb6vdu";

fn cid(v: &Value) -> String {
    record_cid(v).expect("a record value encodes").to_string()
}

/// What a record is, for the properties' filters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// `membership`.
    Membership,
    /// `post`.
    Post,
    /// `comment`.
    Comment,
    /// `revision`.
    Revision,
    /// `opinion`.
    Opinion,
    /// A forum-scope `modAction`.
    ModForum,
    /// An author-scope `modAction` (an L5-only input, §6.3 P4).
    ModAuthor,
    /// `wikiRevision` (L5-only).
    WikiRevision,
    /// `sequence` (L5-only).
    Sequence,
}

impl Kind {
    /// A candidate item of the visibility rules (§10.1).
    pub fn is_item(self) -> bool {
        matches!(
            self,
            Kind::Post | Kind::Comment | Kind::WikiRevision | Kind::Sequence
        )
    }
}

/// One record of a generated forum.
#[derive(Clone, Debug, PartialEq)]
pub struct Rec {
    /// The uri.
    pub uri: String,
    /// Its CID.
    pub cid: String,
    /// The value.
    pub value: Value,
    /// `did(uri)`.
    pub author: String,
    /// What it is.
    pub kind: Kind,
    /// The generator meant it to be valid.
    pub valid: bool,
    /// Its first-seen time (µs).
    pub obs: i64,
}

impl Rec {
    /// `subject.uri` of an opinion or revision, `subject` of a modAction.
    pub fn subject(&self) -> Option<&str> {
        let s = self.value.get("subject")?;
        s.as_str().or_else(|| s.get("uri")?.as_str())
    }

    /// An opinion whose subject is a wikiRevision or a sequence (an L5-only input, P4).
    pub fn opinion_on_l5_item(&self) -> bool {
        self.kind == Kind::Opinion
            && self.subject().is_some_and(|s| {
                s.contains(&format!("/{NS}.wikiRevision/"))
                    || s.contains(&format!("/{NS}.sequence/"))
            })
    }
}

/// A forum and its records.
#[derive(Clone, Debug)]
pub struct Forum {
    /// F.
    pub uri: String,
    /// `fk(F)`.
    pub fk: String,
    /// The declaration.
    pub decl: Value,
    /// Its CID.
    pub decl_cid: String,
    /// The records (unique uris).
    pub recs: Vec<Rec>,
    index: BTreeMap<String, usize>,
    /// Uris whose absence is confirmed (deleted items): in `absent` when a kept opinion names
    /// them and no kept record has them.
    pub deleted: BTreeSet<String>,
    /// CIDs of items that are not records (deleted, never synced), for references to them.
    pub ghost_cids: BTreeMap<String, String>,
    /// Repo status per DID (missing: fresh).
    pub status: BTreeMap<String, &'static str>,
    /// `now` (µs).
    pub now: i64,
    clock: i64,
}

impl Forum {
    /// A forum owned by user 0 with `decl` (its `$type` and `createdAt` are set here).
    pub fn new(mut decl: Value, now: i64) -> Self {
        let uri = format!("at://{}/{NS}.forum/test", user(0));
        let fk = keys::fk(&uri).unwrap();
        decl["$type"] = json!(format!("{NS}.forum"));
        decl["createdAt"] = json!(at(0));
        let decl_cid = cid(&decl);
        Self {
            uri,
            fk,
            decl,
            decl_cid,
            recs: Vec::new(),
            index: BTreeMap::new(),
            deleted: BTreeSet::new(),
            ghost_cids: BTreeMap::new(),
            status: BTreeMap::new(),
            now,
            clock: 0,
        }
    }

    /// Removes every record.
    pub fn clear(&mut self) {
        self.recs.clear();
        self.index.clear();
    }

    /// The record `uri`.
    pub fn get(&self, uri: &str) -> Option<&Rec> {
        self.index.get(uri).map(|i| &self.recs[*i])
    }

    /// The current CID of `uri` (record or ghost).
    pub fn cid_of(&self, uri: &str) -> Option<&str> {
        self.get(uri)
            .map(|r| r.cid.as_str())
            .or_else(|| self.ghost_cids.get(uri).map(String::as_str))
    }

    /// A fresh TID at `T0 + minute` (unique within the forum).
    pub fn tid(&mut self, minute: i64) -> String {
        self.clock += 1;
        let t = T0 + minute.clamp(0, MAX_MINUTE) * MINUTE + self.clock;
        Tid::from_parts(u64::try_from(t).unwrap(), 0)
            .unwrap()
            .as_str()
            .to_owned()
    }

    /// A fresh `{fk}-{tid}` uri in `coll` of `author`'s repo.
    pub fn ranged_uri(&mut self, author: &str, coll: &str, minute: i64) -> String {
        let t = self.tid(minute);
        format!("at://{author}/{NS}.{coll}/{}-{t}", self.fk)
    }

    /// A fresh comment uri under `root`.
    pub fn comment_uri(&mut self, author: &str, root: &str, minute: i64) -> String {
        let t = self.tid(minute);
        let rkey = keys::comment_rkey(NS, &self.uri, root, &t).unwrap();
        format!("at://{author}/{NS}.comment/{rkey}")
    }

    /// Adds a record (its CID is computed); `None` if the uri is taken.
    pub fn put(
        &mut self,
        uri: &str,
        kind: Kind,
        value: Value,
        valid: bool,
        obs: i64,
    ) -> Option<()> {
        if self.index.contains_key(uri) {
            return None;
        }
        let (author, _, _) = keys::split_uri(uri).unwrap();
        let rec = Rec {
            uri: uri.to_owned(),
            cid: cid(&value),
            value,
            author: author.to_owned(),
            kind,
            valid,
            obs,
        };
        self.index.insert(uri.to_owned(), self.recs.len());
        self.recs.push(rec);
        Some(())
    }

    /// Replaces the value of `uri` (a new version: new CID) and returns the old CID.
    pub fn update(&mut self, uri: &str, value: Value) -> String {
        let i = self.index[uri];
        let r = &mut self.recs[i];
        let new = cid(&value);
        r.value = value;
        std::mem::replace(&mut r.cid, new)
    }

    /// Records `uri` as an item that is not in `records` with value `value` (deleted: its
    /// absence is confirmed; otherwise never synced).
    pub fn ghost(&mut self, uri: &str, value: &Value, deleted: bool) {
        self.ghost_cids.insert(uri.to_owned(), cid(value));
        if deleted {
            self.deleted.insert(uri.to_owned());
        }
    }

    /// `obs` of an item created at `minute`: seen 30 s later.
    pub fn seen(minute: i64) -> i64 {
        T0 + minute.clamp(0, MAX_MINUTE) * MINUTE + 30_000_000
    }

    /// The common fields of a record of `coll` created at `minute`: `$type`, `forum`, `createdAt`.
    pub fn base(&self, coll: &str, minute: i64) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("$type".into(), json!(format!("{NS}.{coll}")));
        m.insert("forum".into(), json!(self.uri));
        m.insert("createdAt".into(), json!(at(minute)));
        m
    }

    /// A membership of `did`.
    pub fn membership(&mut self, did: &str) {
        let uri = format!("at://{did}/{NS}.membership/{}", self.fk);
        let v = Value::Object(self.base("membership", 0));
        self.put(&uri, Kind::Membership, v, true, Self::seen(0));
    }

    /// The value of a post (`invalid`: 1 blank title (PO3), 2 `forum` names another forum (G9)).
    pub fn post_value(&self, minute: i64, kind: u8, tags: u8, invalid: u8, title: &str) -> Value {
        let mut m = self.base("post", minute);
        match kind % 4 {
            1 => {
                m.insert("kind".into(), json!("question"));
            }
            2 => {
                m.insert("kind".into(), json!("link"));
                m.insert("link".into(), json!({"uri": "https://example.com/"}));
            }
            3 => {
                m.insert("kind".into(), json!("quickTake"));
                m.insert(
                    "body".into(),
                    json!({"$type": format!("{NS}.markdown"), "text": "A quick take"}),
                );
            }
            _ => {}
        }
        if kind % 4 != 3 {
            m.insert(
                "title".into(),
                json!(if invalid == 1 { "   " } else { title }),
            );
        }
        let t: Vec<&str> = TAGS
            .iter()
            .enumerate()
            .filter(|(i, _)| tags & (1 << i) != 0)
            .map(|(_, t)| *t)
            .collect();
        if !t.is_empty() {
            m.insert("tags".into(), json!(t));
        }
        if invalid == 2 {
            m.insert(
                "forum".into(),
                json!(format!("at://{}/{NS}.forum/elsewhere", user(0))),
            );
        }
        Value::Object(m)
    }

    /// The value of a comment.
    pub fn comment_value(
        &self,
        minute: i64,
        root: (&str, &str),
        parent: Option<(&str, &str)>,
        text: &str,
        invalid: bool,
    ) -> Value {
        let mut m = self.base("comment", minute);
        m.insert("root".into(), json!({"uri": root.0, "cid": root.1}));
        if let Some((u, c)) = parent {
            m.insert("parent".into(), json!({"uri": u, "cid": c}));
        }
        m.insert(
            "body".into(),
            json!({"$type": format!("{NS}.markdown"), "text": text}),
        );
        if invalid {
            m.insert(
                "forum".into(),
                json!(format!("at://{}/{NS}.forum/elsewhere", user(0))),
            );
        }
        Value::Object(m)
    }

    /// An `op: update` revision of `subject` (a post or comment by `author`) whose snapshot is
    /// `old` (so `subject.cid` = CID(old), RV5).
    pub fn revision(&mut self, author: &str, subject: &str, old: &Value, minute: i64) {
        let t = self.tid(minute);
        let rkey = keys::revision_rkey(NS, subject, &t).unwrap();
        let uri = format!("at://{author}/{NS}.revision/{rkey}");
        let v = json!({
            "$type": format!("{NS}.revision"),
            "subject": {"uri": subject, "cid": cid(old)},
            "op": "update",
            "record": old,
            "createdAt": at(minute),
        });
        self.put(&uri, Kind::Revision, v, true, Self::seen(minute));
    }

    /// An opinion of `voter` on `subject` (cited at `subject_cid`) with `axes`; `None` if the
    /// voter already has one on that subject (one key per pair, OP2).
    pub fn opinion(
        &mut self,
        voter: &str,
        subject: &str,
        subject_cid: &str,
        axes: Map<String, Value>,
        minute: i64,
    ) -> Option<String> {
        let rkey = keys::opinion_rkey(NS, &self.uri, subject).unwrap();
        let uri = format!("at://{voter}/{NS}.opinion/{rkey}");
        let mut m = self.base("opinion", minute);
        m.insert(
            "subject".into(),
            json!({"uri": subject, "cid": subject_cid}),
        );
        m.extend(axes);
        self.put(
            &uri,
            Kind::Opinion,
            Value::Object(m),
            true,
            Self::seen(minute),
        )?;
        Some(uri)
    }

    /// A modAction of `actor`; `None` if the actor already has one with that key.
    #[allow(clippy::too_many_arguments)]
    pub fn mod_action(
        &mut self,
        actor: &str,
        author_scope: bool,
        subject: &str,
        subject_cid: Option<&str>,
        action: Value,
        until: Option<&str>,
        reason: Option<&str>,
        minute: i64,
    ) -> Option<String> {
        let scope = author_scope.then_some("author");
        let a = action.as_object().unwrap();
        let rkey = keys::mod_action_rkey(NS, &self.uri, scope, subject, a).unwrap();
        let uri = format!("at://{actor}/{NS}.modAction/{rkey}");
        let mut m = self.base("modAction", minute);
        if let Some(s) = scope {
            m.insert("scope".into(), json!(s));
        }
        m.insert("subject".into(), json!(subject));
        if let Some(c) = subject_cid {
            m.insert("subjectCid".into(), json!(c));
        }
        m.insert("action".into(), action);
        if let Some(u) = until {
            m.insert("until".into(), json!(u));
        }
        if let Some(r) = reason {
            m.insert("reason".into(), json!(r));
        }
        let kind = if author_scope {
            Kind::ModAuthor
        } else {
            Kind::ModForum
        };
        self.put(&uri, kind, Value::Object(m), true, Self::seen(minute))?;
        Some(uri)
    }

    /// The input over the records `keep` selects (§1.1), in the vector JSON form (records sorted
    /// by uri, `absent` by bytes, `obs` by (uri, cid)).
    pub fn json(&self, keep: &dyn Fn(&Rec) -> bool) -> Value {
        let mut recs: Vec<&Rec> = self.recs.iter().filter(|r| keep(r)).collect();
        recs.sort_by(|a, b| a.uri.cmp(&b.uri));
        let uris: BTreeSet<&str> = recs.iter().map(|r| r.uri.as_str()).collect();
        let named: BTreeSet<&str> = recs
            .iter()
            .filter(|r| r.kind == Kind::Opinion)
            .filter_map(|r| r.subject())
            .collect();
        let absent: Vec<&str> = self
            .deleted
            .iter()
            .map(String::as_str)
            .filter(|u| named.contains(u) && !uris.contains(u))
            .collect();
        let records: Vec<Value> = recs
            .iter()
            .map(|r| json!({"uri": r.uri, "cid": r.cid, "value": r.value}))
            .collect();
        let mut obs: Vec<Value> = vec![json!({"uri": self.uri, "cid": self.decl_cid, "us": T0})];
        obs.extend(
            recs.iter()
                .map(|r| json!({"uri": r.uri, "cid": r.cid, "us": r.obs})),
        );
        obs.sort_by(|a, b| {
            (a["uri"].as_str(), a["cid"].as_str()).cmp(&(b["uri"].as_str(), b["cid"].as_str()))
        });
        let authorities: Vec<String> = AUTHORITIES.iter().map(|i| user(*i)).collect();
        let dids: BTreeSet<&str> = recs
            .iter()
            .map(|r| r.author.as_str())
            .chain(authorities.iter().map(String::as_str))
            .collect();
        let status: Map<String, Value> = dids
            .into_iter()
            .map(|d| {
                (
                    d.to_owned(),
                    json!(self.status.get(d).copied().unwrap_or("fresh")),
                )
            })
            .collect();
        json!({
            "decl": {"uri": self.uri, "cid": self.decl_cid, "value": self.decl},
            "records": records,
            "absent": absent,
            "obs": obs,
            "repoStatus": status,
            "now": self.now,
        })
    }

    /// The `FoldInput` over the records `keep` selects.
    pub fn input_where(&self, keep: &dyn Fn(&Rec) -> bool) -> FoldInput {
        FoldInput::from_vector_json(&self.json(keep)).expect("a generated input parses")
    }

    /// The `FoldInput` over every record.
    pub fn input(&self) -> FoldInput {
        self.input_where(&|_| true)
    }

    /// Uris of the records of `kind`.
    pub fn uris(&self, kind: Kind) -> Vec<String> {
        self.recs
            .iter()
            .filter(|r| r.kind == kind)
            .map(|r| r.uri.clone())
            .collect()
    }
}

/// The authorities of every generated forum (user indices): owner, admin, moderator.
pub const AUTHORITIES: [usize; 3] = [0, 1, 2];

// ---------------------------------------------------------------------------------------------
// Specs

/// A user's standing.
#[derive(Clone, Debug)]
pub struct UserSpec {
    /// A membership record.
    pub member: bool,
    /// Forum `#access`: 0 none, 1 member, 2 banned.
    pub access: u8,
    /// Who wrote the `#access` and `#restrict` records: 0 owner, 1 admin, 2 moderator, 3 a
    /// plain user (lapsed).
    pub by: u8,
    /// `#restrict`: bit 0 `nullifyVotes`, bits 1–7 the capabilities of [`CAPS`] (0: none).
    pub restrict: u8,
    /// Repo status: 0 fresh, 1 stale, 2 unavailable.
    pub status: u8,
}

/// A post.
#[derive(Clone, Debug)]
pub struct PostSpec {
    /// Author index.
    pub author: usize,
    /// 0 article, 1 question, 2 link, 3 quickTake.
    pub kind: u8,
    /// Author tags, a bit set over [`TAGS`].
    pub tags: u8,
    /// Creation minute.
    pub minute: i64,
    /// 0 valid, 1 blank title (PO3), 2 another forum (G9).
    pub invalid: u8,
    /// Deleted (absence confirmed), not in `records`.
    pub deleted: bool,
    /// Edited once (a revision holds the old snapshot).
    pub edited: bool,
}

/// Where a comment's `parent` points.
#[derive(Clone, Copy, Debug)]
pub enum ParentSpec {
    /// No parent (a top-level comment).
    None,
    /// Comment `i % n` (any comment: cycles, self-parents and cross-root parents happen).
    Comment(usize),
    /// A comment uri that is in no input (a `Missing` placeholder).
    Ghost,
}

/// A comment.
#[derive(Clone, Debug)]
pub struct CommentSpec {
    /// Author index.
    pub author: usize,
    /// Root post `i % posts`.
    pub root: usize,
    /// The root is a post that was never synced (instead of `root`).
    pub ghost_root: bool,
    /// The parent.
    pub parent: ParentSpec,
    /// Creation minute.
    pub minute: i64,
    /// `forum` names another forum (G9).
    pub invalid: bool,
    /// Deleted (absence confirmed).
    pub deleted: bool,
    /// Edited once.
    pub edited: bool,
}

/// A wikiRevision.
#[derive(Clone, Debug)]
pub struct WikiSpec {
    /// Author index.
    pub author: usize,
    /// Slug index into [`TAGS`].
    pub slug: u8,
    /// Parents: wikiRevision `i % n` (forks and cycles happen).
    pub parents: Vec<usize>,
    /// `isTag`.
    pub is_tag: Option<bool>,
    /// Creation minute.
    pub minute: i64,
}

/// A sequence.
#[derive(Clone, Debug)]
pub struct SeqSpec {
    /// Author index.
    pub author: usize,
    /// Chapter posts `i % posts`.
    pub posts: Vec<usize>,
    /// Creation minute.
    pub minute: i64,
}

/// A modAction.
#[derive(Clone, Debug)]
pub struct ModSpec {
    /// Actor index (forum scope; author scope picks the subject's author when `own` holds).
    pub actor: usize,
    /// Author scope.
    pub author_scope: bool,
    /// Forum scope: 0 hide, 1 keep, 2 access, 3 restrict, 4 tier, 5 pin, 6 lock, 7 tag. Author
    /// scope: 0 hide, 1 ban, 2 thread pin, 3 lock (mod 4).
    pub action: u8,
    /// Subject index into the action's pool.
    pub subject: usize,
    /// Value bits (per action).
    pub variant: u8,
    /// Author scope: the actor is the author of the subject's post (else `actor`).
    pub own: bool,
    /// `subjectCid` names an earlier or unknown version.
    pub stale: bool,
    /// `until`: 0 none, 1 past, 2 future.
    pub until: u8,
    /// Creation minute.
    pub minute: i64,
}

/// An opinion.
#[derive(Clone, Debug)]
pub struct OpSpec {
    /// Voter index.
    pub voter: usize,
    /// Subject index into the opinion pool (items, deleted and unsynced uris).
    pub subject: usize,
    /// `karma` index into [`KARMA`].
    pub karma: Option<u8>,
    /// `agreement` index into [`AGREEMENT`].
    pub agreement: Option<u8>,
    /// `reacts` entries: (react, stance, quote) indices.
    pub reacts: Vec<(u8, u8, u8)>,
    /// `tags` entries: (tag, value) indices.
    pub tags: Vec<(u8, u8)>,
    /// `subject.cid` names an earlier version (or an unknown one).
    pub earlier: bool,
}

/// A whole generated forum.
#[derive(Clone, Debug)]
pub struct Spec {
    /// `membership: open` (else approval).
    pub open: bool,
    /// `policy.voting`: 0 karmaOnly, 1 twoAxis, 2 reacts, 3 absent (the default, `reacts`).
    pub voting: u8,
    /// `pairCap`.
    pub pair_cap: i64,
    /// `establish`.
    pub establish: i64,
    /// `establishSeeds`.
    pub establish_seeds: i64,
    /// `authorModeration`.
    pub author_moderation: bool,
    /// `display.newUsers`: 0 collapse, 1 hide, 2 show.
    pub new_users: u8,
    /// `display.collapseBelow`.
    pub collapse_below: i64,
    /// Small gate values, so that generated karma crosses them (else the `karma/1` defaults).
    pub low_gates: bool,
    /// `tagCreation: moderators`.
    pub tag_creation_mods: bool,
    /// `defaultTier: frontpage`.
    pub frontpage_default: bool,
    /// `now` is past the 90-day cutoff.
    pub late: bool,
    /// The users ([`USERS`]).
    pub users: Vec<UserSpec>,
    /// Posts (at least one).
    pub posts: Vec<PostSpec>,
    /// Comments.
    pub comments: Vec<CommentSpec>,
    /// WikiRevisions.
    pub wikis: Vec<WikiSpec>,
    /// Sequences.
    pub sequences: Vec<SeqSpec>,
    /// ModActions.
    pub mods: Vec<ModSpec>,
    /// Opinions.
    pub ops: Vec<OpSpec>,
}

fn minute() -> impl Strategy<Value = i64> {
    0..MAX_MINUTE
}

fn weighted(p: f64) -> impl Strategy<Value = bool> {
    proptest::bool::weighted(p)
}

fn user_spec() -> impl Strategy<Value = UserSpec> {
    (
        weighted(0.75),
        prop_oneof![5 => Just(0u8), 3 => Just(1u8), 1 => Just(2u8)],
        prop_oneof![6 => 0u8..3, 1 => Just(3u8)],
        prop_oneof![6 => Just(0u8), 1 => 1u8..=255],
        prop_oneof![40 => Just(0u8), 1 => Just(1u8), 1 => Just(2u8)],
    )
        .prop_map(|(member, access, by, restrict, status)| UserSpec {
            member,
            access,
            by,
            restrict,
            status,
        })
}

fn post_spec() -> impl Strategy<Value = PostSpec> {
    (
        0..USERS,
        prop_oneof![5 => Just(0u8), 1 => 1u8..4],
        0u8..8,
        minute(),
        prop_oneof![20 => Just(0u8), 1 => Just(1u8), 1 => Just(2u8)],
        weighted(0.08),
        weighted(0.15),
    )
        .prop_map(
            |(author, kind, tags, minute, invalid, deleted, edited)| PostSpec {
                author,
                kind,
                tags,
                minute,
                invalid,
                deleted,
                edited,
            },
        )
}

fn comment_spec() -> impl Strategy<Value = CommentSpec> {
    (
        0..USERS,
        0usize..64,
        weighted(0.05),
        prop_oneof![
            4 => Just(ParentSpec::None),
            5 => (0usize..64).prop_map(ParentSpec::Comment),
            1 => Just(ParentSpec::Ghost),
        ],
        minute(),
        weighted(0.04),
        weighted(0.06),
        weighted(0.1),
    )
        .prop_map(
            |(author, root, ghost_root, parent, minute, invalid, deleted, edited)| CommentSpec {
                author,
                root,
                ghost_root,
                parent,
                minute,
                invalid,
                deleted,
                edited,
            },
        )
}

fn wiki_spec() -> impl Strategy<Value = WikiSpec> {
    (
        0..USERS,
        0u8..3,
        proptest::collection::vec(0usize..16, 0..3),
        proptest::option::of(any::<bool>()),
        minute(),
    )
        .prop_map(|(author, slug, parents, is_tag, minute)| WikiSpec {
            author,
            slug,
            parents,
            is_tag,
            minute,
        })
}

fn seq_spec() -> impl Strategy<Value = SeqSpec> {
    (
        0..USERS,
        proptest::collection::vec(0usize..64, 0..4),
        minute(),
    )
        .prop_map(|(author, posts, minute)| SeqSpec {
            author,
            posts,
            minute,
        })
}

fn mod_spec() -> impl Strategy<Value = ModSpec> {
    (
        prop_oneof![3 => 0usize..3, 1 => 3..USERS],
        weighted(0.4),
        0u8..8,
        0usize..64,
        any::<u8>(),
        weighted(0.8),
        weighted(0.15),
        prop_oneof![8 => Just(0u8), 1 => Just(1u8), 1 => Just(2u8)],
        minute(),
    )
        .prop_map(
            |(actor, author_scope, action, subject, variant, own, stale, until, minute)| ModSpec {
                actor,
                author_scope,
                action,
                subject,
                variant,
                own,
                stale,
                until,
                minute,
            },
        )
}

fn op_spec() -> impl Strategy<Value = OpSpec> {
    (
        // authorities vote more (seed votes establish users)
        prop_oneof![1 => 0usize..3, 2 => 0..USERS],
        0usize..256,
        // mostly upvotes, so that users get established
        prop_oneof![
            4 => Just(Some(0u8)),
            2 => Just(Some(2u8)),
            2 => Just(Some(1u8)),
            1 => Just(Some(3u8)),
            1 => Just(Some(4u8)),
            1 => Just(None),
        ],
        prop_oneof![1 => (0u8..5).prop_map(Some), 2 => Just(None)],
        prop_oneof![
            3 => Just(Vec::new()),
            1 => proptest::collection::vec((0u8..4, 0u8..3, 0u8..3), 1..3),
        ],
        prop_oneof![
            3 => Just(Vec::new()),
            1 => proptest::collection::vec((0u8..3, 0u8..5), 1..3),
        ],
        weighted(0.12),
    )
        .prop_map(
            |(voter, subject, karma, agreement, reacts, tags, earlier)| OpSpec {
                voter,
                subject,
                karma,
                agreement,
                reacts,
                tags,
                earlier,
            },
        )
}

/// Size limits of a generated forum.
#[derive(Clone, Copy, Debug)]
pub struct Size {
    /// Posts (1..=posts).
    pub posts: usize,
    /// Comments.
    pub comments: usize,
    /// WikiRevisions.
    pub wikis: usize,
    /// Sequences.
    pub sequences: usize,
    /// ModActions.
    pub mods: usize,
    /// Opinions.
    pub ops: usize,
}

/// The default sizes of the property tests.
pub const SMALL: Size = Size {
    posts: 7,
    comments: 14,
    wikis: 5,
    sequences: 2,
    mods: 12,
    ops: 60,
};

/// A random forum spec.
pub fn spec(size: Size) -> impl Strategy<Value = Spec> {
    let policy = (
        weighted(0.7),
        0u8..4,
        proptest::sample::select(vec![1i64, 2, 3, 25]),
        0i64..3,
        prop_oneof![3 => Just(1i64), 2 => Just(2i64), 1 => Just(3i64)],
        weighted(0.85),
        0u8..3,
        proptest::sample::select(vec![-5i64, -1, 0, 1]),
        any::<bool>(),
        weighted(0.2),
        weighted(0.2),
        weighted(0.1),
    );
    let content = (
        proptest::collection::vec(user_spec(), USERS),
        proptest::collection::vec(post_spec(), 1..=size.posts),
        proptest::collection::vec(comment_spec(), 0..=size.comments),
        proptest::collection::vec(wiki_spec(), 0..=size.wikis),
        proptest::collection::vec(seq_spec(), 0..=size.sequences),
        proptest::collection::vec(mod_spec(), 0..=size.mods),
        proptest::collection::vec(op_spec(), 0..=size.ops),
    );
    (policy, content).prop_map(
        |(
            (
                open,
                voting,
                pair_cap,
                establish,
                establish_seeds,
                author_moderation,
                new_users,
                collapse_below,
                low_gates,
                tag_creation_mods,
                frontpage_default,
                late,
            ),
            (users, posts, comments, wikis, sequences, mods, ops),
        )| Spec {
            open,
            voting,
            pair_cap,
            establish,
            establish_seeds,
            author_moderation,
            new_users,
            collapse_below,
            low_gates,
            tag_creation_mods,
            frontpage_default,
            late,
            users,
            posts,
            comments,
            wikis,
            sequences,
            mods,
            ops,
        },
    )
}

/// A spec drawn from `rng` without proptest, with about the strategies' distributions. Its
/// draws are 64-bit on every target, so native and wasm32 builds draw the same specs (proptest's
/// `usize` ranges consume the random stream by pointer width): the parity test uses it.
pub fn random_spec(rng: &mut Rng, size: Size) -> Spec {
    fn pick(rng: &mut Rng, weights: &[u64]) -> usize {
        let total: u64 = weights.iter().sum();
        let mut x = rng.next() % total;
        for (i, w) in weights.iter().enumerate() {
            if x < *w {
                return i;
            }
            x -= w;
        }
        unreachable!()
    }
    fn small(rng: &mut Rng, n: usize) -> u8 {
        u8::try_from(rng.below(n)).unwrap()
    }
    fn minute(rng: &mut Rng) -> i64 {
        i64::try_from(rng.below(usize::try_from(MAX_MINUTE).unwrap())).unwrap()
    }
    let users = (0..USERS)
        .map(|_| UserSpec {
            member: rng.chance(3, 4),
            access: u8::try_from(pick(rng, &[5, 3, 1])).unwrap(),
            by: if rng.chance(1, 7) { 3 } else { small(rng, 3) },
            restrict: if rng.chance(1, 7) {
                1 + small(rng, 255)
            } else {
                0
            },
            status: u8::try_from(pick(rng, &[40, 1, 1])).unwrap(),
        })
        .collect();
    let posts = (0..1 + rng.below(size.posts))
        .map(|_| PostSpec {
            author: rng.below(USERS),
            kind: if rng.chance(5, 6) {
                0
            } else {
                1 + small(rng, 3)
            },
            tags: small(rng, 8),
            minute: minute(rng),
            invalid: u8::try_from(pick(rng, &[20, 1, 1])).unwrap(),
            deleted: rng.chance(8, 100),
            edited: rng.chance(15, 100),
        })
        .collect();
    let comments = (0..rng.below(size.comments + 1))
        .map(|_| CommentSpec {
            author: rng.below(USERS),
            root: rng.below(64),
            ghost_root: rng.chance(5, 100),
            parent: match pick(rng, &[4, 5, 1]) {
                0 => ParentSpec::None,
                1 => ParentSpec::Comment(rng.below(64)),
                _ => ParentSpec::Ghost,
            },
            minute: minute(rng),
            invalid: rng.chance(4, 100),
            deleted: rng.chance(6, 100),
            edited: rng.chance(10, 100),
        })
        .collect();
    let wikis = (0..rng.below(size.wikis + 1))
        .map(|_| WikiSpec {
            author: rng.below(USERS),
            slug: small(rng, 3),
            parents: (0..rng.below(3)).map(|_| rng.below(16)).collect(),
            is_tag: match rng.below(3) {
                0 => None,
                1 => Some(true),
                _ => Some(false),
            },
            minute: minute(rng),
        })
        .collect();
    let sequences = (0..rng.below(size.sequences + 1))
        .map(|_| SeqSpec {
            author: rng.below(USERS),
            posts: (0..rng.below(4)).map(|_| rng.below(64)).collect(),
            minute: minute(rng),
        })
        .collect();
    let mods = (0..rng.below(size.mods + 1))
        .map(|_| ModSpec {
            actor: if rng.chance(3, 4) {
                rng.below(3)
            } else {
                3 + rng.below(USERS - 3)
            },
            author_scope: rng.chance(2, 5),
            action: small(rng, 8),
            subject: rng.below(64),
            variant: small(rng, 256),
            own: rng.chance(4, 5),
            stale: rng.chance(15, 100),
            until: u8::try_from(pick(rng, &[8, 1, 1])).unwrap(),
            minute: minute(rng),
        })
        .collect();
    let ops = (0..rng.below(size.ops + 1))
        .map(|_| OpSpec {
            voter: if rng.chance(1, 3) {
                rng.below(3)
            } else {
                rng.below(USERS)
            },
            subject: rng.below(256),
            karma: [Some(0), Some(2), Some(1), Some(3), Some(4), None]
                [pick(rng, &[4, 2, 2, 1, 1, 1])],
            agreement: if rng.chance(1, 3) {
                Some(small(rng, 5))
            } else {
                None
            },
            reacts: if rng.chance(1, 4) {
                (0..1 + rng.below(2))
                    .map(|_| (small(rng, 4), small(rng, 3), small(rng, 3)))
                    .collect()
            } else {
                Vec::new()
            },
            tags: if rng.chance(1, 4) {
                (0..1 + rng.below(2))
                    .map(|_| (small(rng, 3), small(rng, 5)))
                    .collect()
            } else {
                Vec::new()
            },
            earlier: rng.chance(12, 100),
        })
        .collect();
    Spec {
        open: rng.chance(7, 10),
        voting: small(rng, 4),
        pair_cap: [1, 2, 3, 25][rng.below(4)],
        establish: i64::try_from(rng.below(3)).unwrap(),
        establish_seeds: [1, 2, 3][pick(rng, &[3, 2, 1])],
        author_moderation: rng.chance(85, 100),
        new_users: small(rng, 3),
        collapse_below: [-5, -1, 0, 1][rng.below(4)],
        low_gates: rng.chance(1, 2),
        tag_creation_mods: rng.chance(1, 5),
        frontpage_default: rng.chance(1, 5),
        late: rng.chance(1, 10),
        users,
        posts,
        comments,
        wikis,
        sequences,
        mods,
        ops,
    }
}

// ---------------------------------------------------------------------------------------------
// Building

/// The declaration of `spec`.
pub fn decl(spec: &Spec) -> Value {
    let new_users = ["collapse", "hide", "show"][usize::from(spec.new_users % 3)];
    let mut policy = json!({
        "karma": {
            "pairCap": spec.pair_cap,
            "establish": spec.establish,
            "establishSeeds": spec.establish_seeds,
        },
        "authorModeration": spec.author_moderation,
        "display": {
            "collapseBelow": spec.collapse_below,
            "newUsers": new_users,
        },
    });
    // voting 3: the default (`reacts`), left implicit
    if spec.voting < 3 {
        policy["voting"] = json!(["karmaOnly", "twoAxis", "reacts"][usize::from(spec.voting)]);
    }
    if spec.low_gates {
        policy["gates"] = json!({
            "reactSecond": 1, "reactCreate": 2, "reactAnti": 3,
            "authorModPersonal": 1, "authorModFrontpage": 4,
            "createTag": 1, "wikiEdit": 0, "sequence": 2,
        });
    }
    if spec.tag_creation_mods {
        policy["tagCreation"] = json!("moderators");
    }
    let mut d = json!({
        "name": "Test forum",
        "membership": if spec.open { "open" } else { "approval" },
        "moderators": [{"did": user(1), "role": "admin"}, {"did": user(2), "role": "moderator"}],
        "policy": policy,
    });
    if spec.frontpage_default {
        d["defaultTier"] = json!("frontpage");
    }
    d
}

/// An item of the built forum (live or not).
#[derive(Clone, Debug)]
pub struct Item {
    /// The uri.
    pub uri: String,
    /// The author.
    pub author: String,
    /// The current CID.
    pub cid: String,
    /// The CID of the earlier version, if edited.
    pub earlier: Option<String>,
}

/// Builds the forum of `spec`.
pub fn build(spec: &Spec) -> Forum {
    let now = T0 + if spec.late { 120 * DAY } else { 30 * DAY };
    let mut f = Forum::new(decl(spec), now);
    let users: Vec<String> = (0..USERS).map(user).collect();

    // users: memberships, statuses, forum #access and #restrict
    for (i, u) in spec.users.iter().enumerate() {
        let d = &users[i];
        if u.member {
            f.membership(d);
        }
        match u.status {
            1 => {
                f.status.insert(d.clone(), "stale");
            }
            2 => {
                f.status.insert(d.clone(), "unavailable");
            }
            _ => {}
        }
        if i == 0 {
            continue;
        }
        let by = &users[if u.by < 3 {
            usize::from(u.by)
        } else {
            3 + i % (USERS - 3)
        }];
        if by == d {
            continue;
        }
        if u.access > 0 {
            let level = if u.access == 1 { "member" } else { "banned" };
            let a = json!({"$type": format!("{NS}.modAction#access"), "level": level});
            f.mod_action(by, false, d, None, a, None, None, 1);
        }
        if u.restrict > 0 {
            let caps: Vec<&str> = CAPS
                .iter()
                .enumerate()
                .filter(|(j, _)| u.restrict & (1 << (j + 1)) != 0)
                .map(|(_, c)| *c)
                .collect();
            let mut a = json!({"$type": format!("{NS}.modAction#restrict"), "capabilities": caps});
            if u.restrict & 1 != 0 {
                a["nullifyVotes"] = json!(true);
            }
            f.mod_action(by, false, d, None, a, None, None, 2);
        }
    }

    // posts
    let mut posts: Vec<Item> = Vec::new();
    for p in &spec.posts {
        let author = &users[p.author];
        let uri = f.ranged_uri(author, "post", p.minute);
        let value = f.post_value(p.minute, p.kind, p.tags, p.invalid, "A post");
        // a quick take has no title, so a blank one cannot make it invalid
        let valid = p.invalid == 0 || (p.invalid == 1 && p.kind % 4 == 3);
        let mut item = Item {
            uri: uri.clone(),
            author: author.clone(),
            cid: cid(&value),
            earlier: None,
        };
        if p.deleted {
            f.ghost(&uri, &value, true);
        } else if p.edited {
            let mut new = f.post_value(p.minute, p.kind, p.tags, p.invalid, "A post, edited");
            new["updatedAt"] = json!(at(p.minute + 60));
            item.earlier = Some(item.cid.clone());
            item.cid = cid(&new);
            f.put(&uri, Kind::Post, new, valid, Forum::seen(p.minute));
            f.revision(author, &uri, &value, p.minute + 60);
        } else {
            f.put(&uri, Kind::Post, value, valid, Forum::seen(p.minute));
        }
        posts.push(item);
    }
    // two posts that are in no input: one never synced, one deleted
    let mut ghosts: Vec<Item> = Vec::new();
    for (k, deleted) in [(0usize, false), (1, true)] {
        let author = &users[3 + k];
        let uri = f.ranged_uri(author, "post", 10 + i64::try_from(k).unwrap());
        let value = f.post_value(10, 0, 0, 0, "A ghost");
        f.ghost(&uri, &value, deleted);
        ghosts.push(Item {
            uri: uri.clone(),
            author: author.clone(),
            cid: cid(&value),
            earlier: None,
        });
    }

    // comments: uris first, so that parents can point anywhere
    let roots: Vec<String> = spec
        .comments
        .iter()
        .map(|c| {
            if c.ghost_root {
                ghosts[0].uri.clone()
            } else {
                posts[c.root % posts.len()].uri.clone()
            }
        })
        .collect();
    let curis: Vec<String> = spec
        .comments
        .iter()
        .zip(&roots)
        .map(|(c, root)| f.comment_uri(&users[c.author], root, c.minute))
        .collect();
    let mut comments: Vec<Item> = Vec::new();
    for (i, c) in spec.comments.iter().enumerate() {
        let author = &users[c.author];
        let root = &roots[i];
        let root_cid = f.cid_of(root).unwrap_or(FOREIGN_CID).to_owned();
        let parent: Option<(String, String)> = match c.parent {
            ParentSpec::None => None,
            ParentSpec::Comment(j) => {
                let j = j % curis.len();
                let pc = if j < i {
                    comments[j].cid.clone()
                } else {
                    FOREIGN_CID.to_owned()
                };
                Some((curis[j].clone(), pc))
            }
            ParentSpec::Ghost => {
                let g = f.comment_uri(&users[3], root, 5);
                Some((g, FOREIGN_CID.to_owned()))
            }
        };
        let pref = parent.as_ref().map(|(u, c)| (u.as_str(), c.as_str()));
        let value = f.comment_value(c.minute, (root, &root_cid), pref, "A comment", c.invalid);
        let uri = curis[i].clone();
        let mut item = Item {
            uri: uri.clone(),
            author: author.clone(),
            cid: cid(&value),
            earlier: None,
        };
        if c.deleted {
            f.ghost(&uri, &value, true);
        } else if c.edited {
            let mut new = f.comment_value(c.minute, (root, &root_cid), pref, "Edited", c.invalid);
            new["updatedAt"] = json!(at(c.minute + 30));
            item.earlier = Some(item.cid.clone());
            item.cid = cid(&new);
            f.put(&uri, Kind::Comment, new, !c.invalid, Forum::seen(c.minute));
            f.revision(author, &uri, &value, c.minute + 30);
        } else {
            f.put(
                &uri,
                Kind::Comment,
                value,
                !c.invalid,
                Forum::seen(c.minute),
            );
        }
        comments.push(item);
    }

    // wikiRevisions: uris first (parents may point forward: cycles)
    let wuris: Vec<String> = spec
        .wikis
        .iter()
        .map(|w| f.ranged_uri(&users[w.author], "wikiRevision", w.minute))
        .collect();
    let mut wikis: Vec<Item> = Vec::new();
    for (i, w) in spec.wikis.iter().enumerate() {
        let mut m = f.base("wikiRevision", w.minute);
        let slug = TAGS[usize::from(w.slug % 3)];
        m.insert("slug".into(), json!(slug));
        m.insert("name".into(), json!(slug));
        let parents: Vec<Value> = w
            .parents
            .iter()
            .map(|j| {
                let j = j % wuris.len();
                let c = if j < i {
                    wikis[j].cid.clone()
                } else {
                    FOREIGN_CID.to_owned()
                };
                json!({"uri": wuris[j], "cid": c})
            })
            .collect();
        if !parents.is_empty() {
            m.insert("parents".into(), json!(parents));
        }
        if let Some(t) = w.is_tag {
            m.insert("isTag".into(), json!(t));
        }
        let v = Value::Object(m);
        wikis.push(Item {
            uri: wuris[i].clone(),
            author: users[w.author].clone(),
            cid: cid(&v),
            earlier: None,
        });
        f.put(
            &wuris[i],
            Kind::WikiRevision,
            v,
            true,
            Forum::seen(w.minute),
        );
    }

    // sequences
    let mut seqs: Vec<Item> = Vec::new();
    for s in &spec.sequences {
        let author = &users[s.author];
        let uri = f.ranged_uri(author, "sequence", s.minute);
        let mut m = f.base("sequence", s.minute);
        m.insert("title".into(), json!("A sequence"));
        let chapter: Vec<&str> = s
            .posts
            .iter()
            .map(|j| posts[j % posts.len()].uri.as_str())
            .collect();
        m.insert("chapters".into(), json!([{"posts": chapter}]));
        let v = Value::Object(m);
        seqs.push(Item {
            uri: uri.clone(),
            author: author.clone(),
            cid: cid(&v),
            earlier: None,
        });
        f.put(&uri, Kind::Sequence, v, true, Forum::seen(s.minute));
    }

    // modActions
    for m in &spec.mods {
        mod_action(&mut f, m, &users, &posts, &comments, &wikis, &seqs);
    }

    // opinions: on items, the ghost posts, deleted items
    let pool: Vec<&Item> = posts
        .iter()
        .chain(&comments)
        .chain(&wikis)
        .chain(&seqs)
        .chain(&ghosts)
        .collect();
    for o in &spec.ops {
        let s = pool[o.subject % pool.len()];
        let scid = if o.earlier {
            s.earlier.as_deref().unwrap_or(FOREIGN_CID)
        } else {
            &s.cid
        };
        f.opinion(&users[o.voter], &s.uri, scid, axes(o), 0);
    }
    f
}

/// The axes of an opinion.
pub fn axes(o: &OpSpec) -> Map<String, Value> {
    let mut m = Map::new();
    if let Some(k) = o.karma {
        m.insert("karma".into(), json!(KARMA[usize::from(k % 5)]));
    }
    if let Some(a) = o.agreement {
        m.insert("agreement".into(), json!(AGREEMENT[usize::from(a % 5)]));
    }
    if !o.reacts.is_empty() {
        let r: Vec<Value> = o
            .reacts
            .iter()
            .map(|(r, s, q)| {
                let mut e = json!({
                    "react": REACTS[usize::from(r % 4)],
                    "stance": STANCES[usize::from(s % 3)],
                });
                if q % 3 != 0 {
                    e["quote"] = json!({"exact": QUOTES[usize::from(q % 3)]});
                }
                e
            })
            .collect();
        m.insert("reacts".into(), json!(r));
    }
    if !o.tags.is_empty() {
        let t: Vec<Value> = o
            .tags
            .iter()
            .map(|(t, v)| json!({"tag": TAGS[usize::from(t % 3)], "value": KARMA[usize::from(v % 5)]}))
            .collect();
        m.insert("tags".into(), json!(t));
    }
    if m.is_empty() {
        m.insert("karma".into(), json!("up"));
    }
    m
}

fn mod_action(
    f: &mut Forum,
    m: &ModSpec,
    users: &[String],
    posts: &[Item],
    comments: &[Item],
    wikis: &[Item],
    seqs: &[Item],
) {
    let ty = |name: &str| format!("{NS}.modAction#{name}");
    let v = m.variant;
    let until = match m.until {
        1 => Some("2026-01-02T00:00:00.000Z"),
        2 => Some("2030-01-01T00:00:00.000Z"),
        _ => None,
    };
    let cid_for = |it: &Item| -> String {
        if m.stale {
            it.earlier.clone().unwrap_or_else(|| FOREIGN_CID.to_owned())
        } else {
            it.cid.clone()
        }
    };
    // the author of the post a comment hangs under (from the comment's key: its root's TID)
    let root_author = |c: &Item| -> Option<String> {
        posts
            .iter()
            .find(|p| c.uri.contains(&thread_of(&p.uri)))
            .map(|p| p.author.clone())
    };
    if m.author_scope {
        let (subject, scid, action, owner) = match m.action % 4 {
            0 | 2 if !comments.is_empty() => {
                let c = &comments[m.subject % comments.len()];
                let a = if m.action.is_multiple_of(4) {
                    json!({"$type": ty("hide")})
                } else {
                    json!({"$type": ty("pin"), "scope": "thread", "pinned": v & 1 == 0})
                };
                (c.uri.clone(), Some(cid_for(c)), a, root_author(c))
            }
            3 => {
                let p = &posts[m.subject % posts.len()];
                let allowed: Vec<&str> = comments
                    .iter()
                    .filter(|c| v & 2 != 0 && c.uri.contains(&thread_of(&p.uri)))
                    .take(2)
                    .map(|c| c.uri.as_str())
                    .collect();
                let a = json!({"$type": ty("lock"), "locked": v & 1 == 0, "allowed": allowed});
                (p.uri.clone(), Some(cid_for(p)), a, Some(p.author.clone()))
            }
            _ => {
                let d = users[m.subject % users.len()].clone();
                let a = json!({"$type": ty("access"), "level": "banned"});
                let owner = posts[usize::from(v) % posts.len()].author.clone();
                (d, None, a, Some(owner))
            }
        };
        let actor = match owner {
            Some(o) if m.own => o,
            _ => users[m.actor].clone(),
        };
        f.mod_action(
            &actor,
            true,
            &subject,
            scid.as_deref(),
            action,
            until,
            None,
            m.minute,
        );
        return;
    }
    let actor = users[m.actor].clone();
    let items: Vec<&Item> = posts
        .iter()
        .chain(comments)
        .chain(seqs)
        .chain(wikis)
        .collect();
    let reason = (v & 4 != 0).then_some("off-topic");
    let (subject, scid, action) = match m.action % 8 {
        0 | 1 => {
            let it = items[m.subject % items.len()];
            let a = if m.action.is_multiple_of(8) {
                json!({"$type": ty("hide"), "spam": v & 1 != 0, "showReason": v & 2 == 0})
            } else {
                json!({"$type": ty("hide"), "hidden": false})
            };
            (it.uri.clone(), Some(cid_for(it)), a)
        }
        2 => {
            let d = users[m.subject % users.len()].clone();
            let level = if v & 1 == 0 { "member" } else { "banned" };
            (d, None, json!({"$type": ty("access"), "level": level}))
        }
        3 => {
            let d = users[m.subject % users.len()].clone();
            let caps: Vec<&str> = CAPS
                .iter()
                .enumerate()
                .filter(|(j, _)| v & (1 << j) != 0)
                .map(|(_, c)| *c)
                .collect();
            let mut a = json!({"$type": ty("restrict"), "capabilities": caps});
            if v & 0x80 != 0 {
                a["nullifyVotes"] = json!(true);
            }
            (d, None, a)
        }
        4 => {
            let p = &posts[m.subject % posts.len()];
            let tier = ["personal", "frontpage", "curated"][usize::from(v % 3)];
            (
                p.uri.clone(),
                Some(cid_for(p)),
                json!({"$type": ty("tier"), "tier": tier}),
            )
        }
        5 => {
            let pinned = v & 2 == 0;
            if v & 1 == 0 || comments.is_empty() {
                let pool: Vec<&Item> = posts.iter().chain(seqs).collect();
                let it = pool[m.subject % pool.len()];
                let a = json!({"$type": ty("pin"), "scope": "forum", "pinned": pinned});
                (it.uri.clone(), Some(cid_for(it)), a)
            } else {
                let c = &comments[m.subject % comments.len()];
                let a = json!({"$type": ty("pin"), "scope": "thread", "pinned": pinned});
                (c.uri.clone(), Some(cid_for(c)), a)
            }
        }
        6 => {
            let p = &posts[m.subject % posts.len()];
            let allowed: Vec<&str> = comments
                .iter()
                .filter(|c| c.uri.contains(&thread_of(&p.uri)))
                .take(usize::from(v % 3))
                .map(|c| c.uri.as_str())
                .collect();
            let a = json!({"$type": ty("lock"), "locked": v & 4 == 0, "allowed": allowed});
            (p.uri.clone(), Some(cid_for(p)), a)
        }
        _ => {
            let slug = TAGS[usize::from(v % 3)];
            let a = if v & 4 != 0 && !wikis.is_empty() {
                let w = &wikis[m.subject % wikis.len()];
                json!({"$type": ty("tag"), "head": {"uri": w.uri, "cid": w.cid}})
            } else {
                json!({"$type": ty("tag"), "removed": v & 8 != 0})
            };
            (format!("tag:{slug}"), None, a)
        }
    };
    f.mod_action(
        &actor,
        false,
        &subject,
        scid.as_deref(),
        action,
        until,
        reason,
        m.minute,
    );
}

/// `-{threadTid}-`: the infix of the comment keys of post `uri`'s thread.
fn thread_of(post: &str) -> String {
    let tid = post.rsplit('-').next().unwrap();
    format!("-{tid}-")
}

// ---------------------------------------------------------------------------------------------
// Folding and comparing

/// Folds `input` (this build's namespace), which must be accepted.
pub fn fold(input: &FoldInput, query: &Query) -> ForumView {
    fold_view(input, query).expect("the fold accepts a well-formed input")
}

/// Checks that exactly the records of `input` that the generator meant to be invalid are
/// invalid in `view` (its fold).
pub fn check_validity(forum: &Forum, input: &FoldInput, view: &ForumView) {
    for (uri, rule) in &view.raw.invalid {
        let rec = forum.get(uri).expect("an invalid record of the input");
        assert!(
            !rec.valid,
            "the generator wrote an invalid record ({rule}): {uri} {}",
            rec.value
        );
    }
    for r in forum.recs.iter().filter(|r| !r.valid) {
        assert!(
            view.raw.invalid.contains_key(&r.uri) || !input.records.contains_key(&r.uri),
            "meant invalid but valid: {}",
            r.uri
        );
    }
}

/// The serialised view: the projection of every field (`testdata/vectors/README.md`; pending
/// or unknown fields as their error), then the whole `ForumView` (`Debug`, ordered maps only).
pub fn serialize(view: &ForumView) -> String {
    let mut m = Map::new();
    for f in FIELDS {
        let v = project(view, f).unwrap_or_else(|e| json!(format!("{e:?}")));
        m.insert((*f).to_owned(), v);
    }
    format!("{}\n{view:?}", Value::Object(m))
}

/// The first valid post of `f` (for comment queries).
pub fn first_post(f: &Forum) -> Option<String> {
    f.recs
        .iter()
        .find(|r| r.kind == Kind::Post && r.valid)
        .map(|r| r.uri.clone())
}

/// Queries that exercise every ordering: each post sort (every filter, a tag, a kind, an
/// author), and each comment sort of the thread of `post`.
pub fn queries(post: Option<&str>) -> Vec<Query> {
    let mut out = vec![Query::default()];
    let sorts = [
        PostSort::Magic,
        PostSort::Top,
        PostSort::New,
        PostSort::Old,
        PostSort::RecentComments,
    ];
    let filters = [PostFilter::Frontpage, PostFilter::All, PostFilter::Curated];
    for (i, sort) in sorts.into_iter().enumerate() {
        out.push(Query {
            posts: Some(PostQuery {
                sort,
                filter: filters[i % 3],
                tag: (i == 2).then(|| "t".to_owned()),
                kind: (i == 3).then(|| "question".to_owned()),
                author: (i == 4).then(|| user(3)),
            }),
            ..Query::default()
        });
    }
    if let Some(p) = post {
        for sort in [
            CommentSort::Magic,
            CommentSort::Top,
            CommentSort::New,
            CommentSort::Old,
            CommentSort::LatestReply,
        ] {
            out.push(Query {
                thread: Some(p.to_owned()),
                comments: Some(CommentQuery {
                    thread: p.to_owned(),
                    sort,
                }),
                ..Query::default()
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// A small deterministic PRNG (splitmix64): shuffles and the parity and performance inputs

/// splitmix64.
#[derive(Clone, Debug)]
pub struct Rng(pub u64);

impl Rng {
    /// The next value.
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (n > 0).
    pub fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(n).unwrap()).unwrap()
    }

    /// `true` with probability `num / den`.
    pub fn chance(&mut self, num: u64, den: u64) -> bool {
        self.next() % den < num
    }

    /// Fisher–Yates.
    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = self.below(i + 1);
            v.swap(i, j);
        }
    }
}
