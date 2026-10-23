// SPDX-License-Identifier: AGPL-3.0-or-later
//! L5: tags and wiki (`karma/1`, `social.md` §13): head selection, candidates and flags (§13.2),
//! relevance and application (§13.3).
//!
//! ```text
//! page(slug) = valid wikiRevisions r with r.forum = F, r.slug = slug, not Hidden by V1–V5
//!              (> 1000: keep the greatest (TID, then smallest uri), flag partial)
//! par(r)     = { q.uri : q ∈ r.parents } ∩ uris(page)
//! genesisOK  = tagCreation = moderators ? author ∈ A : passes(author, createTag)
//! editOK     = passes(author, wikiEdit)
//! Cand       = least set: r ∈ page ∧ (genesisOK(r) ∨ (editOK(r) ∧ par(r) ∩ Cand ≠ ∅))
//! Edges      = { (r, q) : r ∈ Cand, q ∈ par(r) ∩ Cand }, cut per round: every cyclic SCC loses
//!              the edges from its greatest-uri member into the SCC
//! depth(r)   = 0 without outgoing edge, else 1 + max depth(q)
//! canonical  = the moderator head if it names a candidate with its current CID,
//!              else argmax (depth desc, K[author] desc, uri asc)
//! exists     ⇔ canonical ≠ none ∧ ¬effective(tag:slug, tag.removed);  applicable ⇔ exists ∧ isTag
//! rel(p, s)  = own + votes;  own = s ∈ p.tags ∧ author not restricted from tag ? max(1, small(max(KP, 0))) : 0
//!              votes = Σ counting v on p, voter ∈ E, tags axis, entry for s (OP3 then OP4): power(KP[voter], value)
//! applies    ⇔ applicable(s) ∧ rel(p, s) > 0
//! ```
//!
//! Inputs: `cx` (wikiRevision values, post tags, CIDs, policy `tagCreation`), `a` (L0), `l1`
//! (`tag.removed`, `tag.head`, restrictions), `v1_v5` (the page rule), `karma` (E, K, KP,
//! counting opinions with the tags axis), `gates` (the L4 stage; `createTag`, `wikiEdit`).
//! Every graph walk is iterative (no recursion depth limit) over ordered collections.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use lessdugin_atp::tid::Tid;
use lessdugin_lexicon::Collection;
use lessdugin_lexicon::values::{Capability, TagCreation, VoteValue};
use serde_json::Value;

use crate::social::full::cids;
use crate::social::full::cx::Cx;
use crate::social::full::gates::{Gate, Gates, passes};
use crate::social::full::karma::{Karma, small, vote_power};
use crate::social::full::moderation::Moderation;
use crate::social::full::view::{Dim, Stage, State};
use crate::social::input::SkelData;
use crate::social::time;

/// The page cap of §13.2 (`limits.md` `WIKI_WALK_MAX`): at most this many revisions per page.
pub const WIKI_WALK_MAX: usize = 1000;

/// A wiki candidate (r ∈ Cand) and its display values.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WikiCandidate {
    /// `depth(r)` over the edges left by the cycle cut.
    pub depth: u32,
    /// The edge targets after the cycle cut.
    pub parents: BTreeSet<String>,
    /// The edge targets whose stored `parents` CID differs from their current CID.
    pub altered: BTreeSet<String>,
    /// `Root(ParentUnavailable)`: a non-empty `parents` field with no candidate among them.
    pub parent_unavailable: bool,
}

/// One slug's page (§13.2).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WikiPage {
    /// The canonical revision.
    pub canonical: Option<String>,
    /// `exists(slug)`.
    pub exists: bool,
    /// `applicable(slug)`.
    pub applicable: bool,
    /// The page held more than 1,000 revisions.
    pub partial: bool,
    /// The history chain from `canonical`.
    pub history: Vec<String>,
    /// "N unmerged edits".
    pub unmerged: u32,
    /// Every candidate.
    pub candidates: BTreeMap<String, WikiCandidate>,
}

/// L5 tag and wiki results.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Wiki {
    /// Pages of every slug of a wikiRevision not Excluded by V1, and of every slug whose `tag:`
    /// subject has a forum-tier `tag.removed` or `tag.head` assertion.
    pub pages: BTreeMap<String, WikiPage>,
    /// The non-zero `rel(p, slug)` of every post not Excluded.
    pub rel: BTreeMap<String, BTreeMap<String, i64>>,
    /// `applies(p, slug)` (posts with at least one slug only).
    pub applies: BTreeMap<String, BTreeSet<String>>,
}

impl Wiki {
    /// `applicable(slug)` (`false` for a slug without a page).
    pub fn applicable(&self, slug: &str) -> bool {
        self.pages.get(slug).is_some_and(|p| p.applicable)
    }

    /// `applies(p, slug)`.
    pub fn applies_to(&self, post: &str, slug: &str) -> bool {
        self.applies.get(post).is_some_and(|s| s.contains(slug))
    }
}

/// A revision on a page, read from its value.
struct Rev<'a> {
    uri: &'a str,
    /// Its CID in the fold input.
    cid: &'a str,
    author: &'a str,
    /// The TID of its rkey (the page cap's order).
    tid: u64,
    /// `parents` as written: `(uri, cid)`.
    parents: Vec<(&'a str, &'a str)>,
    /// `isTag` (default `true`).
    is_tag: bool,
}

fn str_at<'v>(v: &'v Value, key: &str) -> Option<&'v str> {
    v.get(key)?.as_str()
}

/// Not visited (or outside the node set being decomposed).
const NONE: usize = usize::MAX;

/// Scratch space of [`cyclic_sccs`], indexed by node and reused across calls, so that a cut
/// allocates only its result. Every entry is back at its initial value between calls.
struct Scratch {
    /// Tarjan's visit index, [`NONE`] when unvisited.
    index: Vec<usize>,
    low: Vec<usize>,
    on_stack: Vec<bool>,
    /// Membership of the node set being decomposed (and, in [`cut_cycles`], of a component).
    member: Vec<bool>,
    stack: Vec<usize>,
    call: Vec<(usize, usize)>,
}

impl Scratch {
    fn new(n: usize) -> Self {
        Scratch {
            index: vec![NONE; n],
            low: vec![0; n],
            on_stack: vec![false; n],
            member: vec![false; n],
            stack: Vec::new(),
            call: Vec::new(),
        }
    }
}

/// The cyclic strongly connected components (size > 1, or a self-loop) of the subgraph of
/// `adj` induced by `nodes` (iterative Tarjan; edges leaving `nodes` are ignored), in
/// O(|nodes| + their edges) without allocating beyond the result.
fn cyclic_sccs(adj: &[Vec<usize>], nodes: &[usize], s: &mut Scratch) -> Vec<Vec<usize>> {
    for &g in nodes {
        s.member[g] = true;
    }
    let (mut out, mut next) = (Vec::new(), 0usize);
    for &root in nodes {
        if s.index[root] != NONE {
            continue;
        }
        s.index[root] = next;
        s.low[root] = next;
        next += 1;
        s.stack.push(root);
        s.on_stack[root] = true;
        s.call.push((root, 0));
        while let Some(&mut (v, ref mut i)) = s.call.last_mut() {
            if let Some(&w) = adj[v].get(*i) {
                *i += 1;
                if !s.member[w] {
                    continue;
                }
                if s.index[w] == NONE {
                    s.index[w] = next;
                    s.low[w] = next;
                    next += 1;
                    s.stack.push(w);
                    s.on_stack[w] = true;
                    s.call.push((w, 0));
                } else if s.on_stack[w] {
                    s.low[v] = s.low[v].min(s.index[w]);
                }
            } else {
                s.call.pop();
                if let Some(&(p, _)) = s.call.last() {
                    s.low[p] = s.low[p].min(s.low[v]);
                }
                if s.low[v] == s.index[v] {
                    let mut comp = Vec::new();
                    while let Some(w) = s.stack.pop() {
                        s.on_stack[w] = false;
                        comp.push(w);
                        if w == v {
                            break;
                        }
                    }
                    if comp.len() > 1 || adj[v].contains(&v) {
                        out.push(comp);
                    }
                }
            }
        }
    }
    for &g in nodes {
        s.member[g] = false;
        s.index[g] = NONE;
    }
    out
}

/// The cycle cut of §13.2 over index adjacency (a greater index is a greater uri): while a cycle
/// remains, each cyclic SCC (size > 1, or a self-loop), computed once per round, loses every
/// edge from its greatest-uri member into the SCC.
///
/// Cutting edges inside an SCC only splits it, and edges between SCCs are never cut, so the
/// SCCs of a round are the SCCs of the previous round's components, each decomposed on its own
/// remaining edges. Each cyclic component is therefore cut and re-decomposed on its own, from a
/// work list, which gives the rounds' result in O(|C| + |E_C|) per cut instead of a pass over
/// the whole page (`social_perf.rs` `wiki_cycle_cut_budget`; SOC-10).
fn cut_cycles(adj: &mut [Vec<usize>]) {
    let mut s = Scratch::new(adj.len());
    let all: Vec<usize> = (0..adj.len()).collect();
    let mut work = cyclic_sccs(adj, &all, &mut s);
    while let Some(comp) = work.pop() {
        let Some(&m) = comp.iter().max() else {
            continue;
        };
        for &g in &comp {
            s.member[g] = true;
        }
        adj[m].retain(|&q| !s.member[q]);
        for &g in &comp {
            s.member[g] = false;
        }
        work.extend(cyclic_sccs(adj, &comp, &mut s));
    }
}

/// `depth(r)` over an acyclic edge set (iterative post-order; an edge back into the current
/// path, which the cycle cut leaves none of, counts as depth 0).
fn depths(adj: &[Vec<usize>]) -> Vec<u32> {
    let n = adj.len();
    let mut depth = vec![0u32; n];
    // 0 unvisited, 1 on the current path, 2 done
    let mut mark = vec![0u8; n];
    for s in 0..n {
        if mark[s] != 0 {
            continue;
        }
        mark[s] = 1;
        let mut stack: Vec<(usize, usize)> = vec![(s, 0)];
        while let Some(&mut (u, ref mut i)) = stack.last_mut() {
            if let Some(&q) = adj[u].get(*i) {
                *i += 1;
                if mark[q] == 0 {
                    mark[q] = 1;
                    stack.push((q, 0));
                }
            } else {
                depth[u] = adj[u]
                    .iter()
                    .filter(|&&q| mark[q] == 2)
                    .map(|&q| depth[q].saturating_add(1))
                    .max()
                    .unwrap_or(0);
                mark[u] = 2;
                stack.pop();
            }
        }
    }
    depth
}

/// One slug's page (§13.2) from its revisions (V1–V5 already applied).
fn page(
    cx: &Cx<'_>,
    a: &BTreeMap<String, u8>,
    l1: &Moderation,
    k: &Karma,
    slug: &str,
    mut revs: Vec<Rev<'_>>,
) -> WikiPage {
    let partial = revs.len() > WIKI_WALK_MAX;
    if partial {
        revs.sort_by(|x, y| y.tid.cmp(&x.tid).then_with(|| x.uri.cmp(y.uri)));
        revs.truncate(WIKI_WALK_MAX);
    }
    // index order is uri byte order: "greatest uri" is the greatest index, ties by uri compare
    // indices
    revs.sort_by(|x, y| x.uri.cmp(y.uri));
    let n = revs.len();
    let index: BTreeMap<&str, usize> = revs.iter().enumerate().map(|(i, r)| (r.uri, i)).collect();
    let pass = |d: &str, g: Gate| passes(&cx.policy, a, k, d, g);
    let genesis_ok = |r: &Rev<'_>| match cx.policy.tag_creation {
        TagCreation::Moderators => a.contains_key(r.author),
        TagCreation::Open => pass(r.author, Gate::CreateTag),
    };
    // par(r) and its inverse
    let par: Vec<Vec<usize>> = revs
        .iter()
        .map(|r| {
            let mut ps: Vec<usize> = r
                .parents
                .iter()
                .filter_map(|(u, _)| index.get(u).copied())
                .collect();
            ps.sort_unstable();
            ps.dedup();
            ps
        })
        .collect();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (r, ps) in par.iter().enumerate() {
        for &q in ps {
            children[q].push(r);
        }
    }
    // Cand: the least fixed point, by a worklist from the genesis-eligible revisions
    let mut cand = vec![false; n];
    let mut work: Vec<usize> = Vec::new();
    for (i, r) in revs.iter().enumerate() {
        if genesis_ok(r) {
            cand[i] = true;
            work.push(i);
        }
    }
    while let Some(q) = work.pop() {
        for &r in &children[q] {
            if !cand[r] && pass(revs[r].author, Gate::WikiEdit) {
                cand[r] = true;
                work.push(r);
            }
        }
    }
    let nodes: Vec<usize> = (0..n).filter(|&r| cand[r]).collect();
    let mut adj: Vec<Vec<usize>> = (0..n)
        .map(|r| {
            if cand[r] {
                par[r].iter().copied().filter(|&q| cand[q]).collect()
            } else {
                Vec::new()
            }
        })
        .collect();
    let unavailable: Vec<bool> = (0..n)
        .map(|r| cand[r] && !revs[r].parents.is_empty() && adj[r].is_empty())
        .collect();
    cut_cycles(&mut adj);
    let depth = depths(&adj);

    let head = l1.tag_head(slug).and_then(|(u, c)| {
        let i = *index.get(u)?;
        (cand[i] && cids::same(c, revs[i].cid)).then_some(i)
    });
    let canonical: Option<usize> = head.or_else(|| {
        nodes
            .iter()
            .copied()
            .min_by_key(|&r| (Reverse(depth[r]), Reverse(k.k_of(revs[r].author)), r))
    });
    let mut history = Vec::new();
    let mut cur = canonical;
    while let Some(r) = cur {
        history.push(revs[r].uri.to_owned());
        cur = adj[r]
            .iter()
            .copied()
            .min_by_key(|&q| (Reverse(depth[q]), q));
    }
    let mut targeted = vec![false; n];
    for &q in adj.iter().flatten() {
        targeted[q] = true;
    }
    let unmerged = nodes
        .iter()
        .filter(|&&r| Some(r) != canonical && !targeted[r])
        .count();
    let candidates = nodes
        .iter()
        .map(|&r| {
            let altered = adj[r]
                .iter()
                .filter(|&&q| {
                    revs[r]
                        .parents
                        .iter()
                        .any(|(u, c)| *u == revs[q].uri && !cids::same(c, revs[q].cid))
                })
                .map(|&q| revs[q].uri.to_owned())
                .collect();
            let c = WikiCandidate {
                depth: depth[r],
                parents: adj[r].iter().map(|&q| revs[q].uri.to_owned()).collect(),
                altered,
                parent_unavailable: unavailable[r],
            };
            (revs[r].uri.to_owned(), c)
        })
        .collect();
    let exists = canonical.is_some() && !l1.tag_removed(slug);
    let applicable = exists && canonical.is_some_and(|c| revs[c].is_tag);
    WikiPage {
        canonical: canonical.map(|c| revs[c].uri.to_owned()),
        exists,
        applicable,
        partial,
        history,
        unmerged: u32::try_from(unmerged).unwrap_or(u32::MAX),
        candidates,
    }
}

/// OP3 then OP4 for the tag votes of one opinion: per slug, the value if every known entry
/// agrees (equal entries collapse), none if they conflict.
fn tag_votes(value: &Value) -> BTreeMap<&str, VoteValue> {
    let mut agree: BTreeMap<&str, Option<VoteValue>> = BTreeMap::new();
    for e in value
        .get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let (Some(tag), Some(v)) = (
            str_at(e, "tag"),
            str_at(e, "value").and_then(VoteValue::parse),
        ) else {
            continue;
        };
        agree
            .entry(tag)
            .and_modify(|seen| {
                if *seen != Some(v) {
                    *seen = None;
                }
            })
            .or_insert(Some(v));
    }
    agree
        .into_iter()
        .filter_map(|(t, v)| Some((t, v?)))
        .collect()
}

/// L5 (`social.md` §13): wiki pages, tag relevance and application.
pub fn wiki(
    cx: &Cx<'_>,
    a: &BTreeMap<String, u8>,
    l1: &Moderation,
    v1_v5: &BTreeMap<String, Option<State>>,
    karma: &Stage<Karma>,
    gates: &Stage<Gates>,
) -> Stage<Wiki> {
    let k = karma.as_ref().map_err(|p| *p)?;
    gates.as_ref().map_err(|p| *p)?;

    // §13.2 pages: the slugs of wikiRevisions that V1 does not exclude, and of `tag:` subjects
    // with a forum-tier tag.removed or tag.head assertion (resolved or in conflict)
    let mut slugs: BTreeMap<String, Vec<Rev<'_>>> = BTreeMap::new();
    for x in cx
        .records()
        .filter(|x| x.coll == Some(Collection::WikiRevision) && cx.live(&x.uri))
    {
        let Some(v) = cx.value(&x.uri) else { continue };
        let Some(slug) = str_at(v, "slug") else {
            continue;
        };
        let revs = slugs.entry(slug.to_owned()).or_default();
        // the page: V2–V5 do not hide it either
        if !matches!(v1_v5.get(&x.uri), Some(None)) {
            continue;
        }
        let parents = v
            .get("parents")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|p| Some((str_at(p, "uri")?, str_at(p, "cid").unwrap_or_default())))
            .collect();
        revs.push(Rev {
            uri: &x.uri,
            cid: &x.cid,
            author: &x.did,
            tid: time::rkey_tid(&x.rkey)
                .and_then(|t| Tid::parse(t).ok())
                .map_or(0, |t| t.to_u64()),
            parents,
            is_tag: v.get("isTag").and_then(Value::as_bool).unwrap_or(true),
        });
    }
    let asserted = l1
        .resolved
        .iter()
        .filter(|(_, dims)| dims.contains_key(&Dim::TagRemoved) || dims.contains_key(&Dim::TagHead))
        .map(|(s, _)| s)
        .chain(l1.conflicts.keys());
    for s in asserted {
        if let Some(slug) = s.strip_prefix("tag:") {
            slugs.entry(slug.to_owned()).or_default();
        }
    }
    let pages: BTreeMap<String, WikiPage> = slugs
        .into_iter()
        .map(|(slug, revs)| {
            let p = page(cx, a, l1, k, &slug, revs);
            (slug, p)
        })
        .collect();

    // §13.3 relevance of every post that V1 does not exclude
    let mut votes_on: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (uri, o) in &k.opinions {
        if o.tags && k.e.contains(&o.voter) {
            votes_on.entry(o.subject.as_str()).or_default().push(uri);
        }
    }
    let mut rel: BTreeMap<String, BTreeMap<String, i64>> = BTreeMap::new();
    let mut applies: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for x in cx
        .records()
        .filter(|x| x.coll == Some(Collection::Post) && cx.live(&x.uri))
    {
        let mut r: BTreeMap<String, i64> = BTreeMap::new();
        if let SkelData::Post { tags, .. } = &x.data
            && !l1.restricted(&x.did, Capability::Tag)
        {
            let own = small(k.kp_of(&x.did).max(0)).max(1);
            for t in tags.iter().collect::<BTreeSet<_>>() {
                r.insert(t.clone(), own);
            }
        }
        for uri in votes_on.get(x.uri.as_str()).into_iter().flatten() {
            let (Some(o), Some(v)) = (k.opinions.get(*uri), cx.skel(uri).and_then(|s| s.value()))
            else {
                continue;
            };
            let kp = k.kp_of(&o.voter);
            for (t, val) in tag_votes(v) {
                let e = r.entry(t.to_owned()).or_insert(0);
                *e = e.saturating_add(vote_power(kp, val));
            }
        }
        r.retain(|_, v| *v != 0);
        let ap: BTreeSet<String> = r
            .iter()
            .filter(|(t, v)| **v > 0 && pages.get(*t).is_some_and(|p| p.applicable))
            .map(|(t, _)| t.clone())
            .collect();
        if !ap.is_empty() {
            applies.insert(x.uri.clone(), ap);
        }
        if !r.is_empty() {
            rel.insert(x.uri.clone(), r);
        }
    }
    Ok(Wiki {
        pages,
        rel,
        applies,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Index adjacency of `es` over `n` nodes (sorted, deduplicated).
    fn graph(n: usize, es: &[(usize, usize)]) -> Vec<Vec<usize>> {
        let mut adj = vec![Vec::new(); n];
        for &(a, b) in es {
            adj[a].push(b);
        }
        for out in &mut adj {
            out.sort_unstable();
            out.dedup();
        }
        adj
    }

    #[test]
    fn cycle_cut_and_depth() {
        // a=0, b=1, c=2, d=3: a → c → b → a (a 3-cycle) and d → d (a self-loop)
        let mut e = graph(4, &[(0, 2), (2, 1), (1, 0), (3, 3)]);
        cut_cycles(&mut e);
        // c, the greatest uri of the cycle, loses its edge into it; d loses its self-loop
        assert_eq!(e, [vec![2], vec![0], vec![], vec![]]);
        let d = depths(&e);
        assert_eq!((d[1], d[0], d[2], d[3]), (2, 1, 0, 0));
    }

    #[test]
    fn nested_cycles_take_several_rounds() {
        // c ⇄ b and c → a → c: one SCC {a, b, c}; c loses both edges into it
        let mut e = graph(3, &[(2, 1), (1, 2), (2, 0), (0, 2), (1, 0)]);
        cut_cycles(&mut e);
        assert!(e[2].is_empty());
        let d = depths(&e);
        assert_eq!((d[0], d[1]), (1, 2));
    }

    /// The round-based definition of §13.2, verbatim: per round, the SCCs of the whole graph;
    /// every cyclic one loses its greatest member's edges into it.
    fn cut_by_rounds(adj: &mut [Vec<usize>]) -> usize {
        let mut rounds = 0;
        let mut s = Scratch::new(adj.len());
        let all: Vec<usize> = (0..adj.len()).collect();
        loop {
            let comps = cyclic_sccs(adj, &all, &mut s);
            if comps.is_empty() {
                return rounds;
            }
            rounds += 1;
            for comp in comps {
                let m = *comp.iter().max().unwrap();
                adj[m].retain(|q| !comp.contains(q));
            }
        }
    }

    /// The work-list cut equals the round-based definition on random graphs (dense ones with
    /// nested cycles, self-loops and up to 8 parents per node, as WR2 allows).
    #[test]
    fn work_list_cut_equals_rounds() {
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let mut multi_round = 0;
        for case in 0..2000 {
            let n = 1 + usize::try_from(next(14)).unwrap();
            let mut es = Vec::new();
            for a in 0..n {
                for _ in 0..next(if case % 2 == 0 { 3 } else { 9 }) {
                    es.push((a, usize::try_from(next(n as u64)).unwrap()));
                }
            }
            let mut x = graph(n, &es);
            let mut y = x.clone();
            cut_cycles(&mut x);
            if cut_by_rounds(&mut y) > 1 {
                multi_round += 1;
            }
            assert_eq!(x, y, "case {case}: {es:?}");
            assert!(cyclic_sccs(&x, &(0..n).collect::<Vec<_>>(), &mut Scratch::new(n)).is_empty());
        }
        assert!(
            multi_round > 100,
            "{multi_round} cases needed several rounds"
        );
    }

    /// The adversarial page of the review (1,000 revisions, parents i±1 to i±4): one cut per
    /// revision, each on a shrinking component.
    #[test]
    fn long_bidirectional_chain() {
        let n = 1000;
        let mut es = Vec::new();
        for i in 0..n {
            for d in 1..=4 {
                if i >= d {
                    es.push((i, i - d));
                }
                if i + d < n {
                    es.push((i, i + d));
                }
            }
        }
        let mut x = graph(n, &es);
        cut_cycles(&mut x);
        // the greatest member is cut first, then the next greatest of the rest, ...: each keeps
        // only its edges to the greater (already cut) revisions
        for (i, out) in x.iter().enumerate() {
            assert!(out.iter().all(|&q| q > i), "{i}: {out:?}");
        }
        let d = depths(&x);
        assert_eq!(d[0], u32::try_from(n - 1).unwrap());
        let mut y = graph(n, &es);
        cut_by_rounds(&mut y);
        assert_eq!(x, y);
    }

    #[test]
    fn tag_votes_op3_then_op4() {
        let v = serde_json::json!({"tags": [
            {"tag": "a", "value": "up"}, {"tag": "a", "value": "up"},
            {"tag": "b", "value": "up"}, {"tag": "b", "value": "down"},
            {"tag": "c", "value": "superUp"}, {"tag": "c", "value": "strongDown"},
        ]});
        let t: Vec<(&str, VoteValue)> = tag_votes(&v).into_iter().collect();
        assert_eq!(t, [("a", VoteValue::Up), ("c", VoteValue::StrongDown)]);
    }
}
