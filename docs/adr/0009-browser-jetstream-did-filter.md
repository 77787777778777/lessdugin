<!-- SPDX-License-Identifier: CC0-1.0 -->
# ADR 0009 — The browser subscribes to Jetstream with a DID filter, and says so

- Status: Accepted
- Date: 2026-11-03
- Amends: `docs/spec/threat-model.md` PRIV-11 and L2; records decision U-D1 in `docs/spec/sync.md`
  §6.2

## Context

`sync.md` §6.2 has two Jetstream modes. Native clients always use `wildcard`: one v2 connection
for every `<NS>.*` commit of the network, filtered by forum key on the client. The browser uses
`wildcard` only until the participants of its forums are known, then switches to `filtered`: the
connection names the DIDs of D (the viewer's own accounts and, over every followed or open forum,
its authorities, confirmed participants and candidates being synced). Up to 150 DIDs go into the
URL as `dids=` parameters; 151 to 10,000 go in a v1 `options_update` frame; above 10,000 the
browser stays in `wildcard`. While some followed or open forum is open-mode, or the viewer is one
of its authorities (`F_new ≠ ∅`), a side connection without a DID filter carries the membership
commits of every DID, so newcomers are still found.

The filter keeps a tab's traffic in proportion to the forums it reads instead of to all lessdugin
traffic, which a browser worker cannot afford for long. But the filter is data the Jetstream
operator receives. `threat-model.md` PRIV-11 still said that the subscription has no DID filter
and does not reveal which forum is read, which is true only of `wildcard`. Nothing told the user.

The user decided (U-D1, 2026-10-27) to keep the filter as the specification and the plan describe
it, and to disclose what it reveals. The user's own relay does not support `dids=` yet (U-D5);
against it the browser keeps receiving the unfiltered stream, so nothing below depends on it.

## Decision

1. The browser in `assisted` mode uses `filtered` as `sync.md` §6.2 specifies (U-D1). Native
   clients keep `wildcard`; automatic native switching stays Reserved (P2).
2. The disclosure is stated in `threat-model.md` PRIV-11 and L2: once participants are known, the
   Jetstream operator, and anything that can read the request on its path, learns D. A set of
   authorities and participants identifies the forums being read, and D also holds the viewer's
   own DIDs, so the operator can link the viewer's accounts, the forums and the connecting IP.
   The membership side connection names no DID; it only shows that the client wants lessdugin
   membership commits.
3. When it happens: only in `assisted` mode, only in the browser, only after every followed or
   open forum has finished one discovery pass, and again whenever D changes (at most once per 10
   minutes, `sync.md` §6.2). `direct` mode opens no Jetstream connection (decision O-3), so it
   discloses nothing.
4. The SPA states this next to the `Assisted` option of Settings → Data sources, in the user's
   language, where the choice is made.

## Consequences

- `threat-model.md`: PRIV-11 is rewritten and L2 names the filter; `sync.md` §6.2 points here.
- The SPA carries the disclosure (spa package, M5). A user who does not want the filter sent can
  choose `direct`; a per-device option to stay in `wildcard` would trade bandwidth for privacy and
  is not specified.
- A Jetstream operator that logs filters can tell which forums a browser user reads. Hosting a
  Jetstream (or a relay that filters by DID, U-D5) under the user's own control removes that
  third party.
- Open question for the U7 review: whether the browser should offer that `wildcard` option, or
  pad D with other forums' participants. Neither is planned.
