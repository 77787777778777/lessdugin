<!-- SPDX-License-Identifier: CC0-1.0 -->
# ADR 0004 — S7/S9: `listRecords` range cursors and PDS read limits

- Status: Accepted
- Date: 2026-10-16
- Spikes: S7 (`listRecords` cursor-range behaviour on Bluesky and non-reference PDSes) and S9 (PDS
  read limits per host vs shared; PLC headers)

## Context

- **Range listing.** Every lessdugin record key in a forum starts with `{fk}-` (synthesis §2.3).
  `list_range` (§3.3) wants to list one forum's key range per collection without paging the whole
  collection. It relies on a "range cursor trick": the reference PDS treats `cursor` as an
  exclusive rkey bound, so listing descending from cursor `{fk}.` returns keys `< "{fk}."` and
  the `{fk}-…` keys come first (`-` is 0x2D and `.` is 0x2E). The spec calls cursors opaque, so we
  need to know which servers behave this way and how to detect those that don't.
- **Read budgets.** The cost model (§3.7, §3.10) depends on whether the Bluesky PDS read limit
  (3,000 requests per 5 min per IP) is shared across all `*.host.bsky.network` hosts or counted per
  host.

## Method

All public requests were read-only and sent with
`User-Agent: lessdugin-spike/0.0 (+https://lessdugin.7778777.online)`.

| Service | Requests |
|---|---|
| puffball | 19 |
| enoki | 10 |
| blacksky.app | 9 |
| plc.directory | 2 |
| bsky.social | 2 |

**(a) Bluesky PDS.** Resolve and locate the repo:

```sh
curl -A "$UA" 'https://bsky.social/xrpc/com.atproto.identity.resolveHandle?handle=bsky.app'
#   → {"did":"did:plc:z72i7hdynmk6r22z27h6tvur"}
curl -A "$UA" https://plc.directory/did:plc:z72i7hdynmk6r22z27h6tvur
#   → serviceEndpoint https://puffball.us-east.host.bsky.network
#      (_health: {"version":"27acf280b5ccf1704c892db710a25d8f55083090"})
```

Then list with these probes:

```sh
B='https://puffball.us-east.host.bsky.network/xrpc/com.atproto.repo.listRecords?repo=did:plc:z72i7hdynmk6r22z27h6tvur&collection=app.bsky.feed.post'
$B&limit=5
$B&limit=3&cursor=3mxu6dfxuwk2o          # an existing rkey from the middle of page 1
$B&limit=3&cursor=3mxu4z                 # arbitrary, not an rkey
$B&limit=3&cursor=3mxu4eydsmk2v.         # existing rkey + "."
$B&limit=3&cursor=3mxu4                  # bare prefix
$B&limit=3&reverse=true
$B&limit=3&reverse=true&cursor=3mxu4
$B&limit=3&reverse=true&cursor=3mxu4eydsmk2v
```

**Non-reference PDS.** The same probes ran on `https://blacksky.app` for
`did:plc:w4xbfzo7kqfes5zb7r6qv3rw`. That host answers with `server: Rocket` and
`x-pds-backend: rsky`, and `_health` returns
`{"version":"1.14.13","gatekeeper_version":"1.0.2-blacksky"}`, so it is rsky-pds and not the
TypeScript reference. The account was taken from `com.atproto.sync.listRepos?limit=5` on that host.

**(b) Devnet.** `node packages/e2e/devnet.mjs --json devnet.json` started `@atproto/dev-env` 0.6.20
(`@atproto/pds` 0.5.39). Then:

1. `com.atproto.server.createSession` as alice with her app password.
2. `com.atproto.repo.putRecord` into `online.7778777.lessdugin.temp.post` with these rkeys:

   ```
   aaaaaaaaaaa7-3mxygf7onkc2b                 (lower neighbouring fk)
   aaaaaaaaaaaa                               (bare fk, as a membership key would be)
   aaaaaaaaaaaa-3mxygf7onkd2b
   aaaaaaaaaaaa-3mxygf7onke2b
   aaaaaaaaaaaa-3mxygf7onkf2b-3mxygf7onkg2b   (comment shape)
   aaaaaaaaaaaa-~-zzzzzzzzzzzzzzzz            (opinion shape with stid "~")
   aaaaaaaaaaaa.x                             (outside our grammar; tests the upper boundary)
   aaaaaaaaaaab-3mxygf7onkh2b                 (higher neighbouring fk)
   bbbbbbbbbbbb-3mxygf7onki2b
   bbbbbbbbbbbb-3mxygf7onkj2b
   ```

3. `listRecords` with: no cursor; `reverse=true`; `cursor=aaaaaaaaaaaa.`; `cursor=aaaaaaaaaaaa/`;
   paged with `limit=2`; `reverse=true&cursor=aaaaaaaaaaaa-`; `reverse=true&cursor=aaaaaaaaaaaa`;
   `cursor=bbbbbbbbbbbb.`; `cursor=cccccccccccc.`; `cursor=aaaaaaaaaaaa/#?`; `cursor=` (empty);
   `limit=101`; and end-of-range cursors.

**(c) Rate limits.**

- A second account on another host: `plc.directory/did:plc:ewvi7nxzyoun6zhxrhs64oiz`
  (atproto.com) → `https://enoki.us-east.host.bsky.network`.
- Ten rounds of: `GET com.atproto.sync.getLatestCommit` on puffball (for bsky.app), wait 1.2 s,
  the same on enoki (for atproto.com), wait 1.2 s. `RateLimit-*` headers were recorded for these
  and for every other request.
- Egress check: `curl https://checkip.amazonaws.com/` four times on separate connections, then
  three times on one keep-alive connection.

## Results

### (a) Bluesky PDS (puffball, reference implementation)

| Request | Returned rkeys (in order) | `cursor` |
|---|---|---|
| `limit=5` | `3mxuhakymek23 3mxuhaky3rc23 3mxu6dfxuwk2o 3mxu4eydsmk2v 3mxu3zex7y22v` (createdAt descending) | `3mxu3zex7y22v` (= last key) |
| `cursor=3mxu6dfxuwk2o` | `3mxu4eydsmk2v 3mxu3zex7y22v 3mxu3hsnebc2v` (strictly below) | `3mxu3hsnebc2v` |
| `cursor=3mxu4z` (not an rkey) | `3mxu4eydsmk2v 3mxu3zex7y22v 3mxu3hsnebc2v` | `3mxu3hsnebc2v` |
| `cursor=3mxu4eydsmk2v.` | `3mxu4eydsmk2v 3mxu3zex7y22v 3mxu3hsnebc2v` (the key itself is included, since `k < k+"."`) | `3mxu3hsnebc2v` |
| `cursor=3mxu4` | `3mxu3zex7y22v 3mxu3hsnebc2v 3mxp32vwd4c24` (the `3mxu4…` keys sort above `3mxu4`) | `3mxp32vwd4c24` |
| `reverse=true` | `3jt6walwmos2y 3jt6wgfuuvs23 3jtdugdgcb52s` (oldest first) | `3jtdugdgcb52s` |
| `reverse=true&cursor=3mxu4` | `3mxu4eydsmk2v 3mxu6dfxuwk2o 3mxuhaky3rc23` (strictly above) | `3mxuhaky3rc23` |
| `reverse=true&cursor=3mxu4eydsmk2v` | `3mxu6dfxuwk2o 3mxuhaky3rc23 3mxuhakymek23` | `3mxuhakymek23` |

### rsky-pds (blacksky.app), same probes

| Request | Returned rkeys | `cursor` |
|---|---|---|
| `limit=4` | `3mxwwdmaq622s 3mxwruueclk2d 3mxuikr4whc2v 3mxuikqxklc2v` | `3mxuikqxklc2v` |
| `cursor=3mxwruueclk2d` | `3mxuikr4whc2v 3mxuikqxklc2v 3mxgv4w7h4s23` | `3mxgv4w7h4s23` |
| `cursor=3mxv` (not an rkey) | same three | same |
| `cursor=3mxuikr4whc2v.` | same three | same |
| `reverse=true&cursor=3mxv` | `3mxwruueclk2d 3mxwwdmaq622s` (a short page, end of collection) | `3mxwwdmaq622s` (still present) |
| `reverse=true` | `3kgwpun632i2z 3kkkqmwxbd22x 3klaf2dhsxt2g` | `3klaf2dhsxt2g` |

### (b) Devnet (reference PDS 0.5.39)

- No cursor: `bbbb…j2b, bbbb…i2b, aaaaaaaaaaab-…, aaaaaaaaaaaa.x, aaaaaaaaaaaa-~-zzz…, aaaaaaaaaaaa-…f2b-…g2b, aaaaaaaaaaaa-…e2b, aaaaaaaaaaaa-…d2b, aaaaaaaaaaaa, aaaaaaaaaaa7-…`.
  This is bytewise descending: `-` < `.` < digits < letters < `~`. `reverse=true` gives exactly the
  reverse.
- `cursor=aaaaaaaaaaaa.` →

  ```
  aaaaaaaaaaaa-~-zzzzzzzzzzzzzzzz
  aaaaaaaaaaaa-3mxygf7onkf2b-3mxygf7onkg2b
  aaaaaaaaaaaa-3mxygf7onke2b
  aaaaaaaaaaaa-3mxygf7onkd2b
  aaaaaaaaaaaa                ← first key outside the range (bare fk), then the lower fk
  aaaaaaaaaaa7-3mxygf7onkc2b
  ```

  So the **range is isolated**: `aaaaaaaaaaaa.x`, `aaaaaaaaaaab-…` and `bbbb…` are all excluded.
- `cursor=aaaaaaaaaaaa/` (`/` = 0x2F) also returns `aaaaaaaaaaaa.x`. This confirms the bound is
  a plain string comparison.
- Paged with `limit=2` from `aaaaaaaaaaaa.`:

  | Page | Keys | Next cursor |
  |---|---|---|
  | p0 | `…-~-zzz…`, `…-f2b-…g2b` | `aaaaaaaaaaaa-3mxygf7onkf2b-3mxygf7onkg2b` |
  | p1 | `…-e2b`, `…-d2b` | `aaaaaaaaaaaa-3mxygf7onkd2b` |
  | p2 | `aaaaaaaaaaaa`, `aaaaaaaaaaa7-…` | – (left the range) |

  In-range result: the 4 `aaaaaaaaaaaa-` keys, with nothing missed or duplicated.
- Ascending: `reverse=true&cursor=aaaaaaaaaaaa-` and `reverse=true&cursor=aaaaaaaaaaaa` both start
  at `aaaaaaaaaaaa-3mxygf7onkd2b`. Neither includes the bare `aaaaaaaaaaaa`.
- `cursor=bbbbbbbbbbbb.` lists `bbbb…` first. `cursor=cccccccccccc.` (no such range) lists from
  the top of the collection, so the first key is already outside the range: zero rows, one request.
- `cursor=aaaaaaaaaaaa/#?` (characters that are illegal in an rkey) → 200; the cursor is not
  validated.
- `cursor=` (empty) → treated as no cursor.
- `limit=101` → `400 {"error":"InvalidRequest","message":"Invalid com.atproto.repo.listRecords params: integer too big (maximum 100, got 101)"}`.
- End of collection: a non-empty page **always** carries `cursor` = its last key, even when it is
  short (10 records with `limit=100` → `cursor=aaaaaaaaaaa7-3mxygf7onkc2b`). The follow-up request
  returns `{"records":[]}` with no cursor. rsky does the same (the short reverse page above still
  had a cursor).

### (c) Rate-limit headers

| Service | Headers |
|---|---|
| puffball, enoki (`*.host.bsky.network`), bsky.social entryway | `ratelimit-limit: 3000`, `ratelimit-policy: 3000;w=300`, `ratelimit-remaining`, `ratelimit-reset` (unix s = window start + 300), plus `access-control-expose-headers: RateLimit-Limit, RateLimit-Reset, RateLimit-Remaining, RateLimit-Policy` and `access-control-allow-origin: *` |
| rsky (blacksky.app) | none (CORS `*`) |
| plc.directory | none. Only `content-type: application/did+ld+json`, `x-powered-by: Express`, `access-control-allow-origin: *`, `etag` |
| relay, Constellation, Jetstream (ADR 0003) | none |

**Egress caveat.** Requests left the sandbox through a rotating pool of IPs. Four connections to
checkip came from `160.79.106.131`, `.135`, `.141` and `.140`; three requests on one keep-alive
connection all came from `.131`. A fresh connection therefore usually uses a different IP. Each
server-side counter was identified by its `ratelimit-reset` value, which fixes its window start.

All 28 PDS requests (puffball 18, enoki 10), grouped by counter:

| Window start (reset − 300) | Host | `remaining` per hit |
|---|---|---|
| 10:57:11 | puffball | 2999, 2998, 2997, 2996 |
| 10:57:28 | puffball | 2999, 2998, 2997, 2996 |
| 10:57:29 (two counters opened in the same second) | puffball | 2999, 2999 |
| 10:57:30 | puffball | 2999, 2998, 2997 |
| 10:58:56, 10:59:04, :07, :09, :15 | puffball | 2999 each |
| 10:58:54 | enoki | 2999, 2998, 2997, 2996 |
| 10:58:57 | enoki | 2999, 2998 |
| 10:59:03, :05, :08, :13 | enoki | 2999 each |
| 10:56:58 | bsky.social (resolveHandle) | 2999 |

- 16 PDS counters in total.
- 12 of the 28 requests landed in a counter already opened **on the same host**.
- **None** landed in a counter opened on the other host, even though ≥ 5 puffball counters were
  open (opened 10:57:11–10:57:30, expiring ≥ 11:02:11) throughout the enoki sequence and the hosts
  were interleaved 1.2 s apart.

If the budget were one per-IP counter shared across hosts, the observed IP reuse (16 counters in
28 requests, so a pool of about 20 IPs) predicts cross-host hits. The chance of seeing 0 of 19
eligible cross-host hits is about 0.2–1%.

## Decision

1. **`list_range` stays as designed.** It is descending from `cursor="{fk}."` with `limit=100`.
   This is verified on the reference PDS (devnet 0.5.39 and Bluesky's production PDS) and on
   rsky-pds. Concretely:
   - Keep every row whose key starts with `{fk}-`. Stop at the first row that does not (that row
     is discarded). Stop also at an empty page or a missing cursor.
   - A short page (`len < limit`) ends the listing. Both implementations return SQL-limited pages,
     and this saves one request per range that sits at the bottom of a collection, which is the
     common case for single-forum users.
   - **Probe rule (unchanged):** a key `≥ "{fk}."` in the first page means the host does not treat
     cursors as bounds. Set `host.rangeCursor = No` (remembered per host, rechecked after 7 days)
     and fall back to listing the full collection with a prefix filter.
   - **Add a second check:** if the cursor returned by the server is not a key that was just
     returned, or the request with `cursor={fk}.` gets a 400, also set `rangeCursor = No`. A server
     with opaque cursors fails at least one of these checks on its first page.
   - `{fk}.` is itself a syntactically valid rkey (ADR 0003), so servers that validate cursors as
     rkeys still accept it.
   - Ascending (`reverse=true&cursor={fk}`) works identically and is kept for oldest-first replays.
     It also skips the bare `{fk}` key.
2. **Key-grammar check.** In both implementations the order is plain bytewise, and the `~` stid
   (0x7E) and comment keys stay inside the `{fk}-` range. The bare `{fk}` membership key and
   neighbouring fks fall outside it. No grammar change is needed.
3. **Budgets are per host.**
   - The scheduler keeps a `RateBudget` per host, fed from `ratelimit-remaining`/`-reset` when
     present. The headers are CORS-exposed, so the SPA can read them too.
   - Without headers it uses the static default `3000/300 s` for `*.bsky.network` and
     `bsky.social`, and only concurrency plus 429/`Retry-After` handling elsewhere.
   - The 50% (background) and 80% (P0) fractions in §3.7 apply per host.
   - **Cost table (§3.10):** drop the "if Bluesky hosts share one budget" (†) caveat. A cold sync
     of 500 participants spread over Bluesky's many PDS hosts is bounded by per-host concurrency (6)
     and latency, not by a single 3,000-request window. The (†) footnote is replaced by "a single
     host with > 1,500 participants uses half its window".
4. **PLC** sends no rate-limit headers. Keep the designed pacing (≤ 20 req/s, 8 in flight) and
   adapt on 429/`Retry-After`.

## Consequences

- `crates/sync`: `list_range` implements the stop and short-page rules and the two-part probe. A
  per-host `RangeCursor{Yes,No,Unknown}` is stored with `checkedAt`. FakePds gets an
  "opaque cursor" mode and a "cursor ignores bound" mode so the fallback is tested.
- Completeness (I6) depends on the short-page rule. `lessdugin doctor` reports hosts whose short
  pages were later contradicted (a non-empty page after a short one) and turns the rule off for
  them.
- **Remaining work for S9:**
  - **Fixed-IP confirmation.** This sandbox exits through rotating IPs, so the per-host verdict is
    statistical. M1 CI should repeat it from one IP: 3 requests to host A, then 1 to host B. The
    budget is per host if B reports `remaining: 2999`.
  - **Not measured here:** the full "cold sync of 200 real repos" and sustained PLC throughput at
    20 req/s. Both need well over 100 requests per service, which is beyond the good-citizen limit
    of this spike. They move to the M2 benchmark, which runs against devnet/FakePds plus a small
    opt-in production sample.
