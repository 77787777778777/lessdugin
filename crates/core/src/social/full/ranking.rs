// SPDX-License-Identifier: AGPL-3.0-or-later
//! L6: ranking, sorts, filters and Recent Discussion (`magic/1`, `social.md` §12). The sort
//! keys and `hot` are [`crate::social::rank`]'s (`hot`, `order_posts`, `cmp_comments`).
//!
//! ```text
//! b(p)      = score(p) + (tier = curated ? curatedBonus : 0) + (tier ≥ frontpage ? frontpageBonus : 0)
//! posts     = listed posts (displayed Visible or Collapsed) passing the filter (frontpage: tier ≥
//!             frontpage ∧ kind ≠ quickTake; all; curated: tier = curated ∧ kind ≠ quickTake), the
//!             tag (applies), kind (PO2) and author filters, in the sort's order (§12.2); magic:
//!             pins first and the cutoff (pinned posts exempt); hotBits for b > 0
//! activity  = max({t_rank(p)} ∪ {t_rank(c) : c in p's placed thread, state Visible})
//! comments  = pre-order of the displayed nodes of query.comments.thread (listed or Tombstone
//!             comments, placeholders with a listed descendant); siblings: thread pins first
//!             (forum tier, else author tier), then the sort key, then uri; placeholders last
//! latest(c) = max({t_rank(c)} ∪ {t_rank(d) : d a descendant of c, state Visible})
//! recent    = listed posts with ≥ 1 Visible comment in their placed thread, by (newest Visible
//!             comment t_rank desc, uri asc), each with its 3 newest Visible comments
//! ```
//!
//! **Dependencies follow the query.** `wiki` is read only when `query.posts.tag` is set, and
//! `author` only when `query.comments` is set, so a pending L5 layer makes pending only the
//! queries that read it.

use std::collections::BTreeMap;

use lessdugin_lexicon::Collection;
use lessdugin_lexicon::values::{PostKind, Tier};

use crate::dto;
use crate::social::full::author::AuthorTier;
use crate::social::full::cx::Cx;
use crate::social::full::moderation::Moderation;
use crate::social::full::query::{
    CommentQuery, CommentSort, PostFilter, PostQuery, PostSort, Query,
};
use crate::social::full::scores::Score;
use crate::social::full::view::{ItemTime, Stage};
use crate::social::full::visibility::Visibility;
use crate::social::full::wiki::Wiki;
use crate::social::input::SkelData;
use crate::social::rank::{self, CommentKey, PostKey};
use crate::social::threads::{Node, Placement};

/// The number of comments Recent Discussion shows per post (§12.5).
pub const RECENT_COMMENTS: usize = 3;

/// One Recent Discussion entry (§12.5).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecentPost {
    /// The post uri.
    pub post: String,
    /// Its up to 3 newest `Visible` comments, by (t_rank desc, uri asc).
    pub comments: Vec<String>,
}

/// L6 orderings for one query.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Orderings {
    /// The post list of `query.posts` (`None` without it).
    pub post_order: Option<Vec<String>>,
    /// `hot(b(p), t_rank(p))` bit patterns of the listed posts with b > 0 (`magic` only).
    pub hot_bits: BTreeMap<String, u64>,
    /// The comment order of `query.comments` (`None` without it).
    pub comment_order: Option<Vec<String>>,
    /// Recent Discussion (no limit).
    pub recent_discussion: Vec<RecentPost>,
}

fn post_sort(s: PostSort) -> dto::PostSort {
    match s {
        PostSort::Magic => dto::PostSort::Magic,
        PostSort::Top => dto::PostSort::Top,
        PostSort::New => dto::PostSort::New,
        PostSort::Old => dto::PostSort::Old,
        PostSort::RecentComments => dto::PostSort::RecentComments,
    }
}

fn comment_sort(s: CommentSort) -> dto::CommentSort {
    match s {
        CommentSort::Magic => dto::CommentSort::Magic,
        CommentSort::Top => dto::CommentSort::Top,
        CommentSort::New => dto::CommentSort::New,
        CommentSort::Old => dto::CommentSort::Old,
        CommentSort::LatestReply => dto::CommentSort::LatestReply,
    }
}

/// What the orderings read.
struct Rank<'c, 'a> {
    cx: &'c Cx<'a>,
    l1: &'c Moderation,
    times: &'c BTreeMap<String, ItemTime>,
    threads: &'c BTreeMap<String, Placement>,
    vis: &'c Visibility,
    scores: &'c BTreeMap<String, Score>,
}

impl Rank<'_, '_> {
    /// `t_rank(x)` of a non-Excluded item.
    fn t(&self, uri: &str) -> i64 {
        self.times.get(uri).map_or(0, |t| t.t_rank)
    }

    /// `score(x)` of a non-Excluded item.
    fn score(&self, uri: &str) -> i64 {
        self.scores.get(uri).map_or(0, |s| s.score)
    }

    /// The `Visible` comments of the placed thread of `post` (every comment of the tree,
    /// including those under placeholders and re-attached ones).
    fn visible_comments(&self, post: &str) -> Vec<&str> {
        self.threads.get(post).map_or_else(Vec::new, |pl| {
            pl.parent
                .keys()
                .map(String::as_str)
                .filter(|c| self.vis.visible(c))
                .collect()
        })
    }

    /// `b(p)` (§12.2).
    fn b(&self, post: &str) -> i64 {
        let tier = self.l1.tier(post);
        let p = &self.cx.policy;
        let curated = if tier == Tier::Curated {
            p.curated_bonus
        } else {
            0
        };
        let frontpage = if tier >= Tier::Frontpage {
            p.frontpage_bonus
        } else {
            0
        };
        self.score(post)
            .saturating_add(curated)
            .saturating_add(frontpage)
    }

    /// The listed posts, by uri.
    fn listed_posts(&self) -> impl Iterator<Item = &str> {
        self.cx
            .items()
            .filter(|x| x.coll == Some(Collection::Post) && self.vis.listed(&x.uri))
            .map(|x| x.uri.as_str())
    }

    /// §12.2, §12.3: the post list and the `hot` bits of its posts with b > 0 (magic only).
    fn posts(&self, q: &PostQuery, wiki: Option<&Wiki>) -> (Vec<String>, BTreeMap<String, u64>) {
        let keys: Vec<PostKey> = self
            .listed_posts()
            .filter(|p| self.passes(p, q, wiki))
            .map(|p| PostKey {
                uri: p.to_owned(),
                t_rank: self.t(p),
                score: self.score(p),
                b: self.b(p),
                pinned: self.l1.pinned_forum(p),
                activity: self
                    .visible_comments(p)
                    .into_iter()
                    .map(|c| self.t(c))
                    .fold(self.t(p), i64::max),
            })
            .collect();
        let now = self.cx.now();
        let policy = &self.cx.policy;
        let ordered = rank::order_posts(keys, post_sort(q.sort), now, policy);
        let hot_bits = if q.sort == PostSort::Magic {
            ordered
                .iter()
                .filter(|k| k.b > 0)
                .map(|k| {
                    let h = rank::hot(
                        k.b,
                        k.t_rank,
                        now,
                        policy.offset_hours,
                        policy.exponent_milli,
                    );
                    (k.uri.clone(), h.to_bits())
                })
                .collect()
        } else {
            BTreeMap::new()
        };
        (ordered.into_iter().map(|k| k.uri).collect(), hot_bits)
    }

    /// The list filter (§12.3) and the tag, kind and author filters of `q`.
    fn passes(&self, post: &str, q: &PostQuery, wiki: Option<&Wiki>) -> bool {
        let Some(x) = self.cx.skel(post) else {
            return false;
        };
        let kind = match &x.data {
            SkelData::Post { kind, .. } => PostKind::effective(kind.as_deref()).value,
            _ => PostKind::Article,
        };
        let tier = self.l1.tier(post);
        let filter = match q.filter {
            PostFilter::Frontpage => tier >= Tier::Frontpage && kind != PostKind::QuickTake,
            PostFilter::All => true,
            PostFilter::Curated => tier == Tier::Curated && kind != PostKind::QuickTake,
        };
        let tag = q.tag.as_ref().is_none_or(|t| {
            wiki.and_then(|w| w.applies.get(post))
                .is_some_and(|s| s.contains(t))
        });
        let kind_ok = q.kind.as_deref().is_none_or(|k| k == kind.as_str());
        let author = q.author.as_deref().is_none_or(|d| d == x.did);
        filter && tag && kind_ok && author
    }

    /// §12.4: the displayed nodes of the thread of `q.thread` in pre-order.
    fn comments(&self, q: &CommentQuery, author: &AuthorTier) -> Vec<String> {
        let Some(pl) = self.threads.get(&q.thread) else {
            return Vec::new();
        };
        let shown_missing = self.vis.shown_missing.get(&q.thread);
        // latest(c): the newest Visible descendant, children before parents
        let mut below: BTreeMap<&str, i64> = BTreeMap::new();
        let order = pl.top_down();
        for node in order.iter().rev() {
            let Node::Comment(c) = node else {
                continue;
            };
            let newest = pl
                .children_of(node)
                .iter()
                .filter_map(|ch| match ch {
                    Node::Comment(d) => {
                        let own = self.vis.visible(d).then(|| self.t(d));
                        own.max(below.get(d.as_str()).copied())
                    }
                    _ => None,
                })
                .max();
            if let Some(t) = newest {
                below.insert(c, t);
            }
        }
        let key = |c: &str| CommentKey {
            uri: c.to_owned(),
            t_rank: self.t(c),
            score: self.score(c),
            pinned: self
                .l1
                .pin_thread(c)
                .or_else(|| author.thread_pins.get(c).copied())
                .unwrap_or(false),
            latest: below.get(c).copied().unwrap_or(i64::MIN).max(self.t(c)),
        };
        let sort = comment_sort(q.sort);
        let now = self.cx.now();
        let policy = &self.cx.policy;
        let sorted_children = |node: &Node| -> Vec<Node> {
            let kids = pl.children_of(node);
            let mut comments: Vec<CommentKey> = kids
                .iter()
                .filter_map(|n| match n {
                    Node::Comment(c) if self.vis.shown(c) => Some(key(c)),
                    _ => None,
                })
                .collect();
            comments.sort_by(|a, b| rank::cmp_comments(a, b, sort, now, policy));
            let mut out: Vec<Node> = comments.into_iter().map(|k| Node::Comment(k.uri)).collect();
            out.extend(
                kids.iter()
                    .filter(|n| {
                        matches!(n, Node::Missing(m) if shown_missing.is_some_and(|s| s.contains(m)))
                    })
                    .cloned(),
            );
            out
        };
        // pre-order, iteratively (threads are up to 1,000 deep)
        let mut out = Vec::new();
        let mut stack: Vec<Node> = sorted_children(&Node::Root);
        stack.reverse();
        while let Some(node) = stack.pop() {
            if let Some(u) = node.uri() {
                out.push(u.to_owned());
            }
            let mut kids = sorted_children(&node);
            kids.reverse();
            stack.extend(kids);
        }
        out
    }

    /// §12.5: Recent Discussion.
    fn recent(&self) -> Vec<RecentPost> {
        let mut entries: Vec<(i64, RecentPost)> = self
            .listed_posts()
            .filter_map(|p| {
                let mut cs: Vec<(i64, &str)> = self
                    .visible_comments(p)
                    .into_iter()
                    .map(|c| (self.t(c), c))
                    .collect();
                cs.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
                let newest = cs.first()?.0;
                let comments = cs
                    .iter()
                    .take(RECENT_COMMENTS)
                    .map(|(_, c)| (*c).to_owned())
                    .collect();
                Some((
                    newest,
                    RecentPost {
                        post: p.to_owned(),
                        comments,
                    },
                ))
            })
            .collect();
        entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.post.cmp(&b.1.post)));
        entries.into_iter().map(|(_, r)| r).collect()
    }
}

/// L6 (`social.md` §12): the orderings `query` asks for, and Recent Discussion.
#[allow(clippy::too_many_arguments)]
pub fn orderings(
    cx: &Cx<'_>,
    query: &Query,
    l1: &Moderation,
    times: &BTreeMap<String, ItemTime>,
    threads: &BTreeMap<String, Placement>,
    visibility: &Stage<Visibility>,
    scores: &Stage<BTreeMap<String, Score>>,
    wiki: &Stage<Wiki>,
    author: &Stage<AuthorTier>,
) -> Stage<Orderings> {
    let vis = visibility.as_ref().map_err(|p| *p)?;
    let scores = scores.as_ref().map_err(|p| *p)?;
    let r = Rank {
        cx,
        l1,
        times,
        threads,
        vis,
        scores,
    };
    let mut out = Orderings::default();
    if let Some(q) = &query.posts {
        let wiki = match &q.tag {
            Some(_) => Some(wiki.as_ref().map_err(|p| *p)?),
            None => None,
        };
        let (order, bits) = r.posts(q, wiki);
        out.post_order = Some(order);
        out.hot_bits = bits;
    }
    if let Some(q) = &query.comments {
        let author = author.as_ref().map_err(|p| *p)?;
        out.comment_order = Some(r.comments(q, author));
    }
    out.recent_discussion = r.recent();
    Ok(out)
}
