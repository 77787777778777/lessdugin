# SPDX-License-Identifier: CC0-1.0
"""L5: tags and wiki (social.md §13, karma/1).

    compute_tags(view) -> Tags
        Reads L0–L4 and the V1–V5 states only: view.l0 (A), view.l1 (the "tag:" subjects:
        tag.head, tag.removed; restrict), view.l3 (counting opinions, KP, E), view.passes (L4
        gates createTag and wikiEdit), view.policy (tagCreation), records and CIDs, and
        visibility.mod_state() for wikiRevisions (rules V1–V5; V6 and V7 concern comments only).

    Tags.slugs              sorted slugs the projection lists: every slug of a wikiRevision
                            that V1 does not exclude, and every slug whose "tag:" subject has a
                            forum-tier assertion (resolved or in conflict)
    Tags.page(slug)         the PageView of §13.2 (canonical, candidates, depth, edges, flags)
    Tags.exists(slug), Tags.applicable(slug)                       §13.2
    Tags.rel(p, slug), Tags.applies(p, slug)                        §13.3
    Tags.rels(p)            {slug: rel} with every non-zero rel of post p

§13.2, as modelled (readings in ../NOTES.md, "Tags and wiki"):

    page(slug) = { r : a wikiRevision, not Excluded (V1: valid, forum = F) and not Hidden by
                   V2–V5, with r.slug = slug }; if |page| > 1000 keep the 1000 with the greatest
                   (TID of rkey(r) desc, then uri asc) and flag the page partial
    par(r)     = { q.uri : q ∈ r.parents } ∩ uris(page)        (duplicate parents collapse: WR2)
    genesisOK(r) ⇔ tagCreation = moderators ? author(r) ∈ A : passes(author(r), createTag)
    editOK(r)    ⇔ passes(author(r), wikiEdit)
    Cand       = least fixpoint of r ∈ page ∧ (genesisOK(r) ∨ (editOK(r) ∧ par(r) ∩ Cand ≠ ∅))
    Edges      = { (r, q) : r ∈ Cand, q ∈ par(r) ∩ Cand }, then the SCC cycle cut: while a cycle
                 remains, for every SCC C with |C| > 1 or a self-loop, remove every edge (m, q)
                 with q ∈ C, m = the greatest-uri member of C
    depth(r)   = 0 without an outgoing edge, else 1 + max depth over its edges
    canonical  = headRef if headRef ≠ none ∧ headRef.uri ∈ Cand ∧ headRef.cid = cid(records[..])
                 else the candidate with (depth desc, K[author] desc, uri asc); none if Cand = ∅
    flags      parentUnavailable(r) ⇔ r ∈ Cand, r.parents non-empty, par(r) ∩ Cand = ∅
               altered(r, q) for an edge (after the cut) whose stored parents entry for q.uri
               has a CID ≠ cid(q) (any entry, when q.uri is listed more than once); CIDs
               compare by canonical form (§0.4, §20.1 item 45)
    history    from canonical, at each step the edge to the parent with the greatest depth,
               ties by the smallest uri
    unmerged   |{ r ∈ Cand ∖ {canonical} : no edge (x, r) }|

§13.3:
    own   = slug ∈ p.tags (set, PO7) ∧ author(p) not restricted from tag
              ? max(1, small(max(KP[author(p)], 0))) : 0
    votes = Σ over counting opinions v on p with voter ∈ E and the tags axis enabled (§5:
            subject a post in records, voter not restricted from tag), with an entry for slug
            after OP3 (an unknown value discards the entry) and then OP4 (entries for one slug
            collapse if their values are equal, else all are ignored): power(KP[voter], value)
    applies(p, slug) ⇔ applicable(slug) ∧ rel(p, slug) > 0
"""

from __future__ import annotations

from dataclasses import dataclass, field

from . import cids, opinions
from .common import sat_add, tid_int, ubytes
from .karma import power, small

PAGE_CAP = 1000
TAG_VOTE_VALUES = ("up", "down", "strongUp", "strongDown")


@dataclass(frozen=True)
class PageView:
    slug: str
    canonical: str | None
    candidates: tuple            # uris of Cand, sorted by bytes
    depth: dict                  # uri -> depth, for every candidate
    edges: dict                  # uri -> tuple of parent uris (after the cycle cut), sorted
    altered: dict                # uri -> tuple of parent uris whose stored CID differs, sorted
    parent_unavailable: frozenset
    partial: bool
    history: tuple
    unmerged: int
    exists: bool
    applicable: bool


@dataclass(frozen=True)
class Tags:
    pages: dict = field(default_factory=dict)        # slug -> PageView
    rel_map: dict = field(default_factory=dict)      # post uri -> {slug: rel} (non-zero only)

    @property
    def slugs(self) -> list:
        return sorted(self.pages, key=ubytes)

    def page(self, slug: str):
        return self.pages.get(slug)

    @property
    def canonical(self) -> dict:
        return {s: p.canonical for s, p in self.pages.items()}

    def candidates(self, slug: str) -> frozenset:
        p = self.pages.get(slug)
        return frozenset(p.candidates) if p else frozenset()

    def exists(self, slug: str) -> bool:
        p = self.pages.get(slug)
        return p is not None and p.exists

    def applicable(self, slug: str) -> bool:
        p = self.pages.get(slug)
        return p is not None and p.applicable

    def rels(self, p: str) -> dict:
        return dict(self.rel_map.get(p, {}))

    def rel(self, p: str, slug: str) -> int:
        return self.rel_map.get(p, {}).get(slug, 0)

    def applies(self, p: str, slug: str) -> bool:
        return self.applicable(slug) and self.rel(p, slug) > 0


# --------------------------------------------------------------------------- graph helpers

def _sccs(nodes: list, edges: dict) -> list:
    """Strongly connected components (iterative Tarjan); nodes and successors in byte order."""
    index, low, on, stack, out = {}, {}, set(), [], []
    counter = 0
    for root in nodes:
        if root in index:
            continue
        work = [(root, iter(edges.get(root, ())))]
        index[root] = low[root] = counter
        counter += 1
        stack.append(root)
        on.add(root)
        while work:
            v, it = work[-1]
            advanced = False
            for w in it:
                if w not in index:
                    index[w] = low[w] = counter
                    counter += 1
                    stack.append(w)
                    on.add(w)
                    work.append((w, iter(edges.get(w, ()))))
                    advanced = True
                    break
                if w in on:
                    low[v] = min(low[v], index[w])
            if advanced:
                continue
            work.pop()
            if work:
                u = work[-1][0]
                low[u] = min(low[u], low[v])
            if low[v] == index[v]:
                comp = []
                while True:
                    w = stack.pop()
                    on.discard(w)
                    comp.append(w)
                    if w == v:
                        break
                out.append(comp)
    return out


def _cycle_cut(nodes: list, edges: dict) -> dict:
    """§13.2 cycle cut; edges: uri -> set of parent uris (both ends in nodes)."""
    edges = {r: set(qs) for r, qs in edges.items()}
    while True:
        cyclic = [c for c in _sccs(nodes, {r: sorted(qs, key=ubytes) for r, qs in edges.items()})
                  if len(c) > 1 or c[0] in edges.get(c[0], ())]
        if not cyclic:
            return edges
        for comp in cyclic:
            cset = set(comp)
            m = max(comp, key=ubytes)
            edges[m] = {q for q in edges.get(m, ()) if q not in cset}


def _depths(nodes: list, edges: dict) -> dict:
    """depth(r) over an acyclic edge set, iteratively (chains may be 1000 long)."""
    depth = {}
    for root in nodes:
        if root in depth:
            continue
        stack = [(root, False)]
        while stack:
            r, done = stack.pop()
            if r in depth:
                continue
            qs = edges.get(r, ())
            if done or not qs:
                depth[r] = 1 + max(depth[q] for q in qs) if qs else 0
                continue
            stack.append((r, True))
            stack.extend((q, False) for q in qs if q not in depth)
    return depth


# --------------------------------------------------------------------------- §13.2

def _page_view(view, slug: str, members: list) -> PageView:
    records = view.input.records
    l1 = view.l1
    pol = view.policy
    partial = len(members) > PAGE_CAP
    if partial:
        members = sorted(members, key=lambda u: (-tid_int(records[u].rkey[13:]), ubytes(u)))[:PAGE_CAP]
    page = sorted(members, key=ubytes)
    pset = set(page)

    par, entries = {}, {}
    for r in page:
        ps = records[r].value.get("parents", [])
        entries[r] = ps
        par[r] = {p["uri"] for p in ps} & pset

    def author(u):
        return records[u].did

    def genesis_ok(r):
        if pol.tagCreation == "moderators":
            return author(r) in view.l0.A
        return view.passes(author(r), "createTag")

    cand = {r for r in page if genesis_ok(r)}
    edit_ok = {r for r in page if view.passes(author(r), "wikiEdit")}
    changed = True
    while changed:
        changed = False
        for r in page:
            if r not in cand and r in edit_ok and par[r] & cand:
                cand.add(r)
                changed = True
    cands = sorted(cand, key=ubytes)
    edges = _cycle_cut(cands, {r: par[r] & cand for r in cands})
    depth = _depths(cands, edges)
    K = view.l3.K

    head = l1.effective("tag:" + slug, "tag.head")
    canonical = None
    if head is not None and head[0] in cand and cids.same(head[1], records[head[0]].cid):
        canonical = head[0]
    elif cands:
        canonical = min(cands, key=lambda r: (-depth[r], -K.get(author(r), 0), ubytes(r)))

    altered = {}
    for r in cands:
        alt = {p["uri"] for p in entries[r]
               if p["uri"] in edges[r] and not cids.same(p["cid"], records[p["uri"]].cid)}
        if alt:
            altered[r] = tuple(sorted(alt, key=ubytes))
    unavailable = frozenset(r for r in cands if entries[r] and not (par[r] & cand))

    history = []
    x = canonical
    while x is not None:
        history.append(x)
        nxt = sorted(edges[x], key=lambda q: (-depth[q], ubytes(q)))
        x = nxt[0] if nxt else None
    incoming = {q for r in cands for q in edges[r]}
    unmerged = sum(1 for r in cands if r != canonical and r not in incoming)

    exists = canonical is not None and l1.effective("tag:" + slug, "tag.removed") is False
    applicable = exists and records[canonical].value.get("isTag", True) is True
    return PageView(slug=slug, canonical=canonical, candidates=tuple(cands), depth=depth,
                    edges={r: tuple(sorted(edges[r], key=ubytes)) for r in cands}, altered=altered,
                    parent_unavailable=unavailable, partial=partial, history=tuple(history),
                    unmerged=unmerged, exists=exists, applicable=applicable)


# --------------------------------------------------------------------------- §13.3

def _vote_entries(value: dict) -> dict:
    """slug -> value of an opinion's tag votes after OP3 (unknown value: entry discarded) and
    OP4 (equal entries collapse; conflicting entries for one slug are all ignored)."""
    seen: dict = {}
    for e in value.get("tags", []):
        if e["value"] not in TAG_VOTE_VALUES:
            continue                                            # OP3
        seen.setdefault(e["tag"], set()).add(e["value"])
    return {slug: next(iter(vs)) for slug, vs in seen.items() if len(vs) == 1}   # OP4


def _rels(view) -> dict:
    records = view.input.records
    l3 = view.l3
    KP, E = l3.KP, l3.E
    out: dict = {}
    for u, rec in view.items.items():
        if rec.coll != "post" or view.excluded[u]:
            continue
        a = rec.did
        if "tags" in rec.value and not opinions.restricted(view, a, "tag"):
            own = max(1, small(max(KP.get(a, 0), 0)))
            for slug in set(rec.value["tags"]):                  # PO7: set semantics
                out.setdefault(u, {})[slug] = own
    for c in l3.counting.values():
        if c.voter not in E or not opinions.axis_enabled(view, c, "tags"):
            continue
        for slug, value in _vote_entries(records[c.uri].value).items():
            m = out.setdefault(c.subject, {})
            m[slug] = sat_add(m.get(slug, 0), power(KP.get(c.voter, 0), value))
    return {p: {s: m[s] for s in sorted(m, key=ubytes) if m[s] != 0}
            for p, m in out.items() if any(m.values())}


def compute_tags(view) -> Tags:
    from .visibility import mod_state            # V1–V5 states (§1.4: L5 may use them)
    by_slug: dict = {}
    for u, rec in view.items.items():
        if rec.coll != "wikiRevision" or view.excluded[u]:
            continue
        by_slug.setdefault(rec.value["slug"], [])
        if mod_state(view, u) is None:                           # not Hidden by V2–V5
            by_slug[rec.value["slug"]].append(u)
    for (T, d, _res) in view.l1.resolved_items():
        if T.startswith("tag:") and d in ("tag.removed", "tag.head"):
            by_slug.setdefault(T[4:], [])
    pages = {slug: _page_view(view, slug, members) for slug, members in by_slug.items()}
    return Tags(pages=pages, rel_map=_rels(view))


__all__ = ["Tags", "PageView", "compute_tags", "PAGE_CAP"]
