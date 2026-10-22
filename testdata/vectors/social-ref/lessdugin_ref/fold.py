# SPDX-License-Identifier: CC0-1.0
"""The fold (social.md §1): ForumView = fold(FoldInput, Query).

ForumView computes its layers lazily (functools.cached_property), each from the layers below
it only (§1.3 rule 8, §1.4). A vector that projects only L0–L2 fields never touches a stub.

    attribute       layer  module            computes                                        state
    ---------       -----  ------            --------                                        -----
    validity        –      validity.py       data-model.md §7, per record                    final
    policy          –      policy.py         §1.6 effective policy and algorithm IDs         final
    l0              L0     authorities.py    A and ranks (§4.1)                              final
    l1              L1     modfold.py        forum-tier resolution (§4.2–§4.8, §4.10)        final
    degraded        L1     modfold.py        §4.11                                           final
    l2              L2     participation.py  P, G, B, N, S, members, applicants (§3)         final
    l3              L3     karma.py          counting opinions (opinions.py), K1, seedUp, E,  final
                                             KP, K (§5–§6)
    newcomers       L3     (here)            P ∖ (S ∪ E) (§3.1)                              final
    l35             L3.5   scores.py         score, agreement, unweighted, earlier (§7)      final
    passes(u, g)    L4     gates.py          §8                                              final
    l5_author       L5     authormod.py      author tier (§4.9)                              final
    l5_reacts       L5     reacts.py         react aggregation (§9.2)                        final
    l5_tags         L5     tags.py           tags and wiki (§13)                             final
    l5_sequences    L5     sequences.py      sequence library (§14)                          STUB
    excluded        V1     visibility.py     per candidate item                              final
    threads         –      threads.py        §11 placement (steps 1–4), per post uri         final
    times           –      time.py           §2 t_decl, per-item times, t_rank               final
    states          L6     visibility.py     V1–V10 per candidate item (before the pass)     final
    displayed       L6     visibility.py     after the thread pass (§10.2)                   final
    (rank.py)       L6     rank.py           §12 sorts, filters, hot, Recent Discussion      final
    mod_log         L1+L5  (here)            §4.12                                           final
    mod_queue       L1+L2  (here)            §4.12 (stale, conflict, applicant)              final
    input_digest    –      digest.py         §1.2                                            final

L7 (the lens, §10.3) is local presentation: visibility.lens() applies mutes to `displayed` and
never changes a fold output; no vector asserts it.
"""

from __future__ import annotations

from functools import cached_property

from . import authormod, digest, gates, karma, reacts, scores, sequences, tags, validity
from .authorities import compute_authorities
from .common import ITEM_COLLECTIONS, InputError, coll_of, fk, ubytes
from .input import parse_vector_input
from .modfold import ModFold, degraded, mod_time
from .participation import compute_participation
from .policy import effective_policy
from .threads import place_thread
from .time import compute_times
from . import visibility


class ForumView:
    def __init__(self, inp, query=None):
        self.input = inp
        self.query = query or {}
        self.F = inp.decl.uri
        self.fk = fk(self.F)
        dv = validity.check(inp.decl.uri, inp.decl.value)
        if not dv.valid:
            raise InputError(f"decl is not valid ({dv.rule}: {dv.detail}); the forum cannot be opened")
        self.policy = effective_policy(inp.decl.value)
        if inp.algorithm_ids is not None and inp.algorithm_ids != self.policy.algs:
            raise InputError(f"algorithmIds {inp.algorithm_ids} differ from the effective IDs {self.policy.algs}")
        self.validity = {u: validity.check(u, r.value) for u, r in inp.records.items()}

    # ---------------------------------------------------------------- L0–L2
    @cached_property
    def l0(self):
        return compute_authorities(self.F, self.input.decl.value)

    @cached_property
    def l1(self):
        return ModFold(self.input.records, self.validity, self.l0, self.policy, self.F, self.input.now)

    @cached_property
    def degraded(self) -> bool:
        return degraded(self.l0, self.input)

    @cached_property
    def l2(self):
        return compute_participation(self.input.records, self.validity, self.l0, self.l1,
                                     self.policy, self.F)

    # ---------------------------------------------------------------- L3–L5
    @cached_property
    def l3(self):
        return karma.compute_karma(self)

    @cached_property
    def newcomers(self) -> frozenset:
        return self.l2.P - (self.l2.S | self.l3.E)

    @cached_property
    def l35(self):
        return scores.compute_scores(self)

    def passes(self, u: str, g: str) -> bool:
        return gates.passes(self, u, g)

    @cached_property
    def l5_author(self):
        return authormod.compute_author_tier(self)

    @cached_property
    def l5_reacts(self):
        return reacts.compute_reacts(self)

    @cached_property
    def l5_tags(self):
        return tags.compute_tags(self)

    @cached_property
    def l5_sequences(self):
        return sequences.compute_sequences(self)

    # ---------------------------------------------------------------- items, threads, time
    @cached_property
    def items(self) -> dict:
        """Every candidate item (§10.1): uri -> Record, in uri order."""
        return {u: r for u, r in self.input.records.items() if r.coll in ITEM_COLLECTIONS}

    @cached_property
    def excluded(self) -> dict:
        return {u: visibility.v1_excluded(self, u) for u in self.items}

    @cached_property
    def threads(self) -> dict:
        """post uri -> Thread, for every non-Excluded post and every root of a non-Excluded comment."""
        by_root, roots = {}, set()
        for u, r in self.items.items():
            if self.excluded[u]:
                continue
            if r.coll == "post":
                roots.add(u)
            elif r.coll == "comment":
                p = r.value["root"]["uri"]
                roots.add(p)
                by_root.setdefault(p, {})[u] = r.value
        out = {}
        for p in sorted(roots, key=ubytes):
            present = p in self.items and not self.excluded[p] and self.items[p].coll == "post"
            out[p] = place_thread(p, by_root.get(p, {}), present)
        return out

    @cached_property
    def times(self):
        live = {u: r for u, r in self.items.items() if not self.excluded[u]}
        return compute_times(live, self.threads, self.input.decl, self.input.obs)

    # ---------------------------------------------------------------- L6 visibility
    def mod_state(self, uri: str):
        return visibility.mod_state(self, uri)

    @cached_property
    def states(self) -> dict:
        return {u: visibility.state(self, u) for u in self.items}

    @cached_property
    def displayed(self) -> dict:
        return visibility.thread_pass(self, self.states)

    # ---------------------------------------------------------------- mod log, queue (§4.12)
    @cached_property
    def mod_log(self) -> list:
        entries = []
        for uri, m in self.l1.mods.items():
            if m.scope == "forum" or m.no_effect is not None:
                st = self.l1.forum_status(m)
            else:
                st = self.l5_author.status(uri)
            entries.append((uri, st))
        for uri, rec in self.input.records.items():
            if rec.coll == "revision" and self.validity[uri].valid:
                sub = rec.value["subject"]["uri"]
                if coll_of(sub) == "modAction":
                    entries.append((uri, ("edited" if rec.value["op"] == "update" else "removed", None)))
        none_last = -(2 ** 70)

        def key(e):
            t = mod_time(self.input.records[e[0]])
            return (-(t if t is not None else none_last), ubytes(e[0]))
        return sorted(entries, key=key)

    @cached_property
    def mod_queue(self) -> list:
        q = []
        l1 = self.l1
        for uri, m in l1.mods.items():
            if m.scope != "forum" or m.no_effect is not None:
                continue
            if m.status == "stale" and m.subject in self.input.records:
                q.append(("stale", uri))
            elif m.stale_head and m.value["action"]["head"]["uri"] in self.input.records:
                q.append(("stale", uri))
        for (T, d, res) in l1.resolved_items():
            if res == "conflict":
                q.append(("conflict", T))
        if self.policy.membership == "approval":
            for d in self.l2.applicants:
                q.append(("applicant", d))
        return sorted(q, key=lambda e: (e[0], ubytes(e[1])))

    # ---------------------------------------------------------------- §1.2
    @cached_property
    def input_digest(self) -> str:
        return digest.input_digest(self.input, self.policy)


def fold(inp, query=None) -> ForumView:
    return ForumView(inp, query)


def fold_vector_input(obj) -> ForumView:
    inp, q = parse_vector_input(obj)
    return ForumView(inp, q)
