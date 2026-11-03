# M6 assisted indexer client contract

The implementation is `lessdugin_core::indexer`; the transport shapes are in
`indexer::contract`. The normative source remains `docs/spec/data-model.md` §11 and
`docs/spec/sync.md` §§1.2, 2.4, 6.6, 8.6 and 15.4.

## Selection and transport

An engine constructs a client only when `sources=assisted` and a stored or configured consent
matches. Forum-scoped consent matches the complete forum AT URI; a global consent is usable for
any forum. Production endpoints are HTTPS on the default port, without credentials, query or
fragment. Explicit devnet mode additionally permits HTTP on `localhost` or a loopback IP.
`describeIndexer` establishes the DID and feature set. Features are the short method names listed
in the normative contract, not full NSIDs. Every other call is refused unless its method name is
advertised. Direct mode returns before `describeIndexer`, so it generates no indexer traffic.

All methods are anonymous GETs under
`/xrpc/online.7778777.lessdugin.index.<method>`, with no redirects, scheduled host budgets and a
4 MiB JSON body cap. Array parameters are repeated query parameters. `getRecordProofs` accepts at
most 25 URIs and uses an 8 MiB CAR cap and `Accept: application/vnd.ipld.car`. Its body is parsed
only by `lessdugin_atp::proof::ProofBundle` (`proofs/1`), never by JSON or an Engine DTO shortcut.

## Trust boundary

`listForums`, `listParticipants`, `getRepoHeads` and `getChanges` are discovery/change hints.
Head hints can queue a PDS sync but cannot write `headSeen`, `listed`, or a repo revision.
`getChanges` validates ascending exclusive sequence numbers, persists its integer cursor under
`indexer:<did>:<fk>`, writes ordinary dirty marks, and enqueues point fetches. A malformed page
does not advance the cursor. `CursorTooOld` queues reconciliation and restarts at the current
`asOf.seq`.

`listPosts`, `listRecentDiscussion` and `getThread` expose `OrderingHint`: AT URIs, an opaque
cursor and (where defined) `inputDigest`. Hint text, state, badges, rank and tallies never enter
the local model. `inputDigest` is retained only for A4 comparison. Visible content and values
come from the ordinary local fold.

A proof response is checked item-by-item against a fresh document-level signing key and the
author's current PDS head. A proof is admitted only when its commit revision is at least that
head and the head observation is no more than 60 seconds older than bundle receipt. Current
proofs use the normal record validator and `Source::Indexer`; stale, missing, mismatched and
failed items are never fold inputs and fall back to `point_fetch`. Bundle failures add persistent
host strikes, with one breaker failure per bundle. A request that returns no bundle
(`Unavailable`) and a single URI still over the cap (`TooLarge`) also fall back to the PDS, but
are not strikes and are not audit contradictions.

`LiveProofBatch` holds at most 25 URIs and flushes a non-full batch after a host-selected 1–3
second delay. Core does not spawn a task: the native service agent owns the timer and calls
`Engine::hydrate_indexer_records`. The same method accepts a cold-start visible page, producing
one request per 25 visible URIs (two for 26–50).

## Audit and service integration

`Engine::audit_indexer` is the shared A1–A5 implementation. `lessdugin sync <forum> --audit`
prints per-check and total counts and exits 6 for any contradiction or omission. A2 uses bounded
session-local Jetstream observations; it intentionally does not invent a persistent Jetstream
history. Audit does not automatically demote an indexer (P2).

The eventual service agent should reuse `poll_indexer_changes`, the ordering methods,
`hydrate_indexer_records`, `LiveProofBatch`, and `audit_indexer`. It must schedule the returned
futures, drain the existing dirty queue, and invalidate/read local views after hydration. It
must not add a second CAR verifier, persist hint DTOs as records, or call an endpoint before
engine selection. No production endpoint or service publication is part of M6.
