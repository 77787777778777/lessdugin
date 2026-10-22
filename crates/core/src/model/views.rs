// SPDX-License-Identifier: AGPL-3.0-or-later
//! DTO assembly (`engine-api.md` §4) from a [`ForumModel`]: post summaries and views, threads
//! (pre-order with depth, siblings sorted per `social.md` §12.4), history, members and the forum
//! header.
//!
//! Views are pure functions of the model, the record values that carry the bodies to render
//! (re-read from the store by the caller), the authors' actor views (from the identity cache) and
//! local state; the same inputs give byte-identical JSON on every platform (`engine-api.md` §1).
//! Hidden content is never rendered: tombstones and placeholders carry an empty body.

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_lexicon::Collection;
use lessdugin_lexicon::records::Forum;
use lessdugin_lexicon::values::{MembershipMode, PostKind as LexPostKind, Role};
use serde_json::Value;

use crate::dto::{
    ActorView, Badges, Banner, CommentNode, CommentSort, Completeness, DtoVersion,
    ExternalLinkView, ForumRole, ForumSummary, ForumView, HandleState, HistorySummary, HistoryView,
    ItemState, MemberView, MembersQuery, MembershipMode as DtoMode, ModRole, ModeratorView,
    PostKind, PostSummary, PostView, RenderedBody, ScoreView, TextQuote, ThreadRoot, ThreadView,
    VersionEntry, VersionRef,
};
use crate::identity::short_did;
use crate::model::build::ForumModel;
use crate::model::render::{RenderCache, empty_body, render_md};
use crate::social::fold::{ItemFold, State};
use crate::social::input::{Skel, SkelData};
use crate::social::rank::{CommentKey, cmp_comments};
use crate::social::threads::Node;
use crate::social::time::ms;
use crate::store::records::Level;

/// An actor view with only the DID (no cached identity).
pub fn bare_actor(did: &str) -> ActorView {
    ActorView {
        did: did.to_owned(),
        handle: None,
        handle_state: HandleState::Unverified,
        claimed_handle: None,
        display_name: None,
        avatar: None,
        short_did: short_did(did),
    }
}

/// The DTO post kind of an effective lexicon kind.
pub fn post_kind(k: LexPostKind) -> PostKind {
    match k {
        LexPostKind::Article => PostKind::Article,
        LexPostKind::Link => PostKind::Link,
        LexPostKind::Question => PostKind::Question,
        LexPostKind::QuickTake => PostKind::QuickTake,
    }
}

/// The host of a URL (for link cards).
fn host_of(uri: &str) -> String {
    url::Url::parse(uri)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_default()
}

fn text_quote(v: &Value) -> Option<TextQuote> {
    Some(TextQuote {
        exact: v.get("exact")?.as_str()?.to_owned(),
        prefix: v.get("prefix").and_then(Value::as_str).map(str::to_owned),
        suffix: v.get("suffix").and_then(Value::as_str).map(str::to_owned),
        start: v
            .get("start")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok()),
    })
}

/// Everything a view needs besides the model.
pub struct Views<'a> {
    /// The forum model.
    pub model: &'a ForumModel,
    /// Actor views by DID (missing: [`bare_actor`]).
    pub actors: &'a BTreeMap<String, ActorView>,
    /// PDS origins by DID (media attribution).
    pub pds: &'a BTreeMap<String, String>,
    /// Record values of the items whose bodies are shown.
    pub values: &'a BTreeMap<String, Value>,
    /// The render cache.
    pub renders: &'a RenderCache,
    /// Own uris with an outbox write that is not done.
    pub pending: &'a BTreeSet<String>,
    /// Completeness of the forum.
    pub completeness: &'a Completeness,
    /// The query time (µs) for time-dependent ordering (`magic`, `hot`); the fold's own states
    /// are as of the model's build ([`ForumModel::now`], at most
    /// [`crate::limits::MODEL_MAX_AGE_MS`] old in the engine).
    pub now: i64,
}

impl Views<'_> {
    /// The actor view of `did`.
    pub fn actor(&self, did: &str) -> ActorView {
        self.actors
            .get(did)
            .cloned()
            .unwrap_or_else(|| bare_actor(did))
    }

    fn badges(&self, x: &Skel, i: &ItemFold) -> Badges {
        Badges {
            verified: self
                .model
                .rows
                .get(&x.uri)
                .is_some_and(|r| r.level == Level::Proven),
            edited: i.edited,
            future_dated: i.future,
            identity_unverified: false,
            stale: self.model.stale(&x.did),
            hint: false,
            inconsistent_timestamps: i.inconsistent,
            pending: self.pending.contains(&x.uri),
        }
    }

    fn score(i: &ItemFold) -> ScoreView {
        ScoreView {
            score: i.score.score,
            agreement: i.score.agreement,
            unweighted: i.score.unweighted,
            earlier_version_votes: i.score.earlier,
            mine: None,
        }
    }

    /// The displayed state of an item (after the thread pass; hidden items keep their state).
    pub fn state(i: &ItemFold) -> ItemState {
        i.displayed
            .clone()
            .or_else(|| i.state.item_state())
            .unwrap_or(ItemState::Visible)
    }

    fn created_ms(x: &Skel) -> i64 {
        x.times.claim.or_else(|| x.times.eff()).map_or(0, ms)
    }

    /// The rendered body of item `x` (empty unless the item is listed and its value was
    /// read).
    pub fn body(&self, x: &Skel, i: &ItemFold) -> RenderedBody {
        if !i.state.listed() {
            return empty_body();
        }
        let Some(md) = self.values.get(&x.uri).and_then(|v| v.get("body")) else {
            return empty_body();
        };
        let excerpt = match &x.data {
            SkelData::Post { excerpt, .. } => excerpt.as_deref(),
            _ => None,
        };
        let pds = self.pds.get(&x.did).map_or("", String::as_str);
        self.renders
            .body(&x.cid, md, &self.model.input.ns, excerpt, &x.did, pds)
    }

    fn last_comment_ms(&self, post: &str) -> Option<i64> {
        let th = self.model.fold.threads.get(post)?;
        th.placement
            .parent
            .keys()
            .filter_map(|c| self.model.fold.item(c))
            .filter(|i| i.state == State::Visible)
            .filter_map(|i| i.t_rank)
            .max()
            .map(ms)
    }

    /// The summary of post `uri` (`None` unless it is a non-Excluded post).
    pub fn post_summary(&self, uri: &str) -> Option<PostSummary> {
        let x = self.model.skel(uri)?;
        let i = self.model.fold.item(uri)?;
        let SkelData::Post {
            kind,
            title,
            tags,
            labels,
            link,
            excerpt,
            spilled,
            ..
        } = &x.data
        else {
            return None;
        };
        if i.state == State::Excluded {
            return None;
        }
        let kind = post_kind(LexPostKind::effective(kind.as_deref()).value);
        // a hidden or tombstoned post shows none of its author's content (title, link, tags and
        // labels included), like its body
        let listed = i.state.listed();
        let excerpt = if !listed {
            String::new()
        } else if *spilled {
            excerpt.clone().unwrap_or_default()
        } else {
            self.body(x, i).excerpt
        };
        let th = self.model.fold.threads.get(uri);
        Some(PostSummary {
            uri: uri.to_owned(),
            cid: x.cid.clone(),
            forum: self.model.key.uri.to_string(),
            author: self.actor(&x.did),
            post_kind: kind,
            // a quickTake's title is ignored (PO4)
            title: if kind == PostKind::QuickTake || !listed {
                None
            } else {
                title.clone()
            },
            excerpt,
            tags: if listed { tags.clone() } else { Vec::new() },
            tier: i.tier,
            pinned: i.pinned,
            locked: i.locked,
            created_at_ms: Self::created_ms(x),
            t_rank_ms: i.t_rank.map_or(0, ms),
            comment_count: th.map_or(0, |t| t.listed),
            last_comment_at_ms: self.last_comment_ms(uri),
            score: Self::score(i),
            state: Self::state(i),
            badges: self.badges(x, i),
            unread_comments: None,
            link: if kind == PostKind::Link && listed {
                link.as_ref().map(|l| ExternalLinkView {
                    uri: l.uri.clone(),
                    host: host_of(&l.uri),
                    title: l.title.clone(),
                    description: l.description.clone(),
                })
            } else {
                None
            },
            labels: if listed { labels.clone() } else { Vec::new() },
        })
    }

    /// Valid `op: update` revisions of `uri`, in TID order.
    fn revisions(&self, uri: &str) -> Vec<&Skel> {
        self.model
            .fold
            .revisions
            .get(uri)
            .map(|v| v.iter().filter_map(|r| self.model.skel(r)).collect())
            .unwrap_or_default()
    }

    /// History summary of `uri`. "Redacted" when the item says it was edited but no revision
    /// of it is left.
    pub fn history_summary(&self, uri: &str) -> HistorySummary {
        let revs = self.revisions(uri).len() as u32;
        let edited_later = self.model.skel(uri).is_some_and(Skel::updated_later);
        HistorySummary {
            revisions: revs,
            redacted: revs == 0 && edited_later,
        }
    }

    /// The view of post `uri` with its rendered body (TODO(M3): reacts; M5: sequences).
    pub fn post_view(&self, uri: &str) -> Option<PostView> {
        let post = self.post_summary(uri)?;
        let x = self.model.skel(uri)?;
        let i = self.model.fold.item(uri)?;
        Some(PostView {
            dto: DtoVersion::default(),
            post,
            body: self.body(x, i),
            reacts: Vec::new(),
            history: self.history_summary(uri),
            sequences: Vec::new(),
        })
    }

    /// "On revision n" / "on an unavailable version" (§11 step 6): `None` when `cid` is the
    /// target's current CID or the target is not in the fold input.
    pub fn version_ref(&self, target: &str, cid: &str) -> Option<VersionRef> {
        let t = self.model.skel(target)?;
        if t.cid == cid {
            return None;
        }
        let n = self.revisions(target).iter().position(|r| match &r.data {
            SkelData::Revision {
                subject: Some(s), ..
            } => s.cid == cid,
            _ => false,
        });
        Some(match n {
            Some(n) => VersionRef::Revision {
                revision: n as u32 + 1,
            },
            None => VersionRef::Unavailable { unavailable: true },
        })
    }

    /// The node of comment `uri` in the thread of `root` (`None` unless it is placed there).
    pub fn comment_node(&self, uri: &str, root: &str) -> Option<CommentNode> {
        let x = self.model.skel(uri)?;
        let i = self.model.fold.item(uri)?;
        let th = self.model.fold.threads.get(root)?;
        let pl = &th.placement;
        let SkelData::Comment {
            root: r,
            parent,
            retracted,
            moderator_hat,
            quote,
        } = &x.data
        else {
            return None;
        };
        let parent_uri = match pl.parent.get(uri)? {
            crate::social::threads::Parent::Root => None,
            crate::social::threads::Parent::Comment(p)
            | crate::social::threads::Parent::Missing(p) => Some(p.clone()),
        };
        let listed = i.state.listed();
        let version = match (parent, r) {
            (Some(p), _) => self.version_ref(&p.uri, &p.cid),
            (None, Some(r)) => self.version_ref(&r.uri, &r.cid),
            _ => None,
        };
        Some(CommentNode {
            uri: uri.to_owned(),
            cid: x.cid.clone(),
            root: root.to_owned(),
            parent: parent_uri,
            depth: pl.depth.get(uri).copied().unwrap_or(1),
            author: self.actor(&x.did),
            body: self.body(x, i),
            quote: if listed {
                quote.as_ref().and_then(text_quote)
            } else {
                None
            },
            // TODO(M3): resolve the quote against P of the target (`lfm-1.md` §11)
            quote_anchor: None,
            created_at_ms: Self::created_ms(x),
            t_rank_ms: i.t_rank.map_or(0, ms),
            score: Self::score(i),
            // TODO(M3): reacts
            reacts: Vec::new(),
            state: Self::state(i),
            badges: self.badges(x, i),
            retracted: *retracted,
            moderator_hat: *moderator_hat && self.model.fold.is_authority(&x.did),
            version: if listed { version } else { None },
            orphaned: pl.orphaned.contains(uri),
            missing: false,
            unread: false,
            pinned: i.pinned,
        })
    }

    fn placeholder_node(&self, uri: &str, root: &str) -> CommentNode {
        let did = crate::social::input::split_uri(uri).map_or("", |(d, _, _)| d);
        CommentNode {
            uri: uri.to_owned(),
            cid: String::new(),
            root: root.to_owned(),
            parent: None,
            depth: 1,
            author: self.actor(did),
            body: empty_body(),
            quote: None,
            quote_anchor: None,
            created_at_ms: 0,
            t_rank_ms: 0,
            score: ScoreView::default(),
            reacts: Vec::new(),
            state: ItemState::Visible,
            badges: Badges::default(),
            retracted: false,
            moderator_hat: false,
            version: None,
            orphaned: false,
            missing: true,
            unread: false,
            pinned: false,
        }
    }

    /// The displayed comments of the thread of `root` in pre-order, siblings sorted by `sort`
    /// (pins first; placeholders after the comments, by uri).
    pub fn thread_comments(&self, root: &str, sort: CommentSort) -> Vec<CommentNode> {
        let Some(th) = self.model.fold.threads.get(root) else {
            return Vec::new();
        };
        let pl = &th.placement;
        // latest(c) = max({t_rank(c)} ∪ {t_rank(d) : d a Visible descendant}) (latestReply);
        // `visible[n]` is the greatest t_rank of a Visible node in n's subtree
        let mut visible: BTreeMap<Node, i64> = BTreeMap::new();
        let mut latest: BTreeMap<String, i64> = BTreeMap::new();
        for node in pl.top_down().iter().rev() {
            let below = pl
                .children_of(node)
                .iter()
                .filter_map(|k| visible.get(k))
                .copied()
                .max()
                .unwrap_or(i64::MIN);
            let mut own_visible = i64::MIN;
            if let Node::Comment(c) = node
                && let Some(i) = self.model.fold.item(c)
            {
                let t = i.t_rank.unwrap_or(i64::MIN);
                if i.state == State::Visible {
                    own_visible = t;
                }
                latest.insert(c.clone(), t.max(below));
            }
            visible.insert(node.clone(), own_visible.max(below));
        }
        let now = self.now;
        let policy = &self.model.fold.policy;
        let mut out = Vec::new();
        let mut stack: Vec<Node> = vec![Node::Root];
        // iterative pre-order: push children in reverse sorted order
        while let Some(node) = stack.pop() {
            match &node {
                Node::Comment(c) => {
                    if let Some(n) = self.comment_node(c, root) {
                        out.push(n);
                    }
                }
                Node::Missing(m) => out.push(self.placeholder_node(m, root)),
                Node::Root => {}
            }
            let mut comments: Vec<(CommentKey, Node)> = Vec::new();
            let mut missing: Vec<Node> = Vec::new();
            for k in pl.children_of(&node) {
                match k {
                    Node::Comment(c) => {
                        let Some(i) = self.model.fold.item(c) else {
                            continue;
                        };
                        if i.displayed.is_none() {
                            continue;
                        }
                        comments.push((
                            CommentKey {
                                uri: c.clone(),
                                t_rank: i.t_rank.unwrap_or(0),
                                score: i.score.score,
                                pinned: i.pinned,
                                latest: latest.get(c).copied().unwrap_or(0),
                            },
                            k.clone(),
                        ));
                    }
                    Node::Missing(m) if th.shown_missing.contains(m) => missing.push(k.clone()),
                    _ => {}
                }
            }
            comments.sort_by(|a, b| cmp_comments(&a.0, &b.0, sort, now, policy));
            let ordered: Vec<Node> = comments
                .into_iter()
                .map(|(_, n)| n)
                .chain(missing)
                .collect();
            stack.extend(ordered.into_iter().rev());
        }
        out
    }

    /// The thread of post `root`.
    pub fn thread_view(&self, root: &str, sort: CommentSort) -> ThreadView {
        let post = match self.post_view(root) {
            Some(p) => ThreadRoot::Post(Box::new(p)),
            None => ThreadRoot::Missing {
                missing: root.to_owned(),
            },
        };
        ThreadView {
            dto: DtoVersion::default(),
            post,
            comments: self.thread_comments(root, sort),
            sort,
            completeness: self.completeness.clone(),
        }
    }

    /// The version history of `uri` (§11 step 6, `data-model.md` §6.5).
    pub fn history(&self, uri: &str) -> Option<HistoryView> {
        let x = self.model.skel(uri)?;
        let revs = self.revisions(uri);
        let rev_meta = |r: &Skel| match &r.data {
            SkelData::Revision {
                subject,
                summary,
                has_record,
                ..
            } => (
                subject.as_ref().map(|s| s.cid.clone()).unwrap_or_default(),
                summary.clone(),
                *has_record,
            ),
            _ => (String::new(), None, false),
        };
        let mut versions = Vec::with_capacity(revs.len() + 1);
        let mut at = Self::created_ms(x);
        let mut summary: Option<String> = None;
        for (n, r) in revs.iter().enumerate() {
            let (cid, s, available) = rev_meta(r);
            versions.push(VersionEntry {
                n: n as u32 + 1,
                cid,
                created_at_ms: at,
                summary: summary.take(),
                available,
            });
            at = Self::created_ms(r);
            summary = s;
        }
        // an edit without a revision (`--no-history`, a redaction) dates the current version
        // by its later `updatedAt`
        let updated = x
            .updated_at
            .as_deref()
            .and_then(crate::social::time::us)
            .map(ms);
        let at = updated.filter(|u| *u > at).unwrap_or(at);
        versions.push(VersionEntry {
            n: revs.len() as u32 + 1,
            cid: x.cid.clone(),
            created_at_ms: at,
            summary,
            available: true,
        });
        Some(HistoryView {
            dto: DtoVersion::default(),
            uri: uri.to_owned(),
            versions,
        })
    }

    /// The forum's role of `did` (TODO(M3): `established` and `newcomer` need karma).
    pub fn role_of(&self, did: &str) -> ForumRole {
        let f = &self.model.fold;
        match f.authorities.get(did) {
            Some(3) => ForumRole::Owner,
            Some(2) => ForumRole::Admin,
            Some(_) => ForumRole::Moderator,
            None if f.participants.contains(did) => ForumRole::Member,
            None => ForumRole::None,
        }
    }

    /// `members` (basic): every DID with a valid membership record, by DID.
    pub fn members(&self, q: &MembersQuery) -> Vec<MemberView> {
        let f = &self.model.fold;
        let approval = f.policy.membership == MembershipMode::Approval;
        let mut out = Vec::new();
        for x in self.model.input.records.values() {
            if x.coll != Some(Collection::Membership)
                || !x.valid
                || !f.roster.members.contains(&x.did)
            {
                continue;
            }
            let applicant = approval
                && !f.authorities.contains_key(&x.did)
                && !f.roster.granted.contains(&x.did)
                && !f.roster.banned.contains(&x.did);
            if q.pending == Some(true) && !applicant {
                continue;
            }
            let status = self.role_of(&x.did);
            if q.status.is_some_and(|s| s != status) {
                continue;
            }
            out.push(MemberView {
                actor: self.actor(&x.did),
                status,
                granted: f.roster.granted.contains(&x.did),
                membership_note: x
                    .value()
                    .and_then(|v| v.get("note"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                joined_at_ms: Self::created_ms(x),
            });
        }
        out.sort_by(|a, b| a.actor.did.cmp(&b.actor.did));
        out
    }

    /// The forum's summary.
    pub fn forum_summary(
        &self,
        me: Option<&str>,
        followed: bool,
        alias: Option<String>,
    ) -> ForumSummary {
        let d: &Forum = &self.model.fold.decl;
        ForumSummary {
            uri: self.model.key.uri.to_string(),
            owner: self.actor(self.model.key.owner.as_str()),
            name: d.name.clone(),
            description: d.description.clone(),
            membership: match self.model.fold.policy.membership {
                MembershipMode::Open => DtoMode::Open,
                MembershipMode::Approval => DtoMode::Approval,
            },
            followed,
            member: me.is_some_and(|m| self.model.is_member(m)),
            alias,
            report_contact: d.report_contact.clone(),
        }
    }

    /// The forum header (`forum_open`).
    pub fn forum_view(
        &self,
        me: Option<&str>,
        followed: bool,
        alias: Option<String>,
        warnings: &[String],
    ) -> ForumView {
        let f = &self.model.fold;
        let about = f.decl.about.as_ref().and_then(|md| {
            let v = serde_json::to_value(md).ok()?;
            Some(render_md(&v, &self.model.input.ns, None))
        });
        let moderators =
            lessdugin_lexicon::validity::entry::moderators(&f.decl, &self.model.key.owner)
                .into_iter()
                .map(|(m, role)| ModeratorView {
                    actor: self.actor(m.did.as_str()),
                    role: match role {
                        Role::Admin => ModRole::Admin,
                        Role::Moderator => ModRole::Moderator,
                    },
                    title: m.title.clone(),
                    shown: self.model.is_member(m.did.as_str()),
                })
                .collect();
        let mut banners: Vec<Banner> = f
            .banners
            .iter()
            .map(|(code, message)| Banner {
                code: code.clone(),
                message: message.clone(),
            })
            .collect();
        banners.extend(warnings.iter().map(|w| Banner {
            code: "warning".into(),
            message: w.clone(),
        }));
        ForumView {
            dto: DtoVersion::default(),
            forum: self.forum_summary(me, followed, alias),
            about,
            moderators,
            policy: f.policy.dto(),
            my_role: me.map_or(ForumRole::None, |m| self.role_of(m)),
            // TODO(M3): karma
            my_karma: None,
            banners,
            completeness: self.completeness.clone(),
        }
    }
}
