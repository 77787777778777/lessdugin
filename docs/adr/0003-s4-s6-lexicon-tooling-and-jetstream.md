<!-- SPDX-License-Identifier: CC0-1.0 -->
# ADR 0003 — S4/S6: digit NSID tooling, Jetstream wildcard, relay and Constellation discovery

- Status: Accepted
- Date: 2026-10-16
- Spikes: S4 (NSID through tooling; Jetstream wildcard; relay `listReposByCollection`;
  Constellation `source=`) and S6 (Jetstream wildcard plus rkey-prefix filtering). The S4 items about
  `validate`-unset writes, nested blob refs and blob GC are in ADR 0005.

## Context

- The namespace is `online.7778777.lessdugin` (synthesis §2.1). Its second segment is all digits.
  NSID syntax forbids a leading digit only in the first segment, but every tool on our path has to
  agree, including `goat lex lint`/`check-dns` in CI and the TypeScript and Go validators.
- Assisted mode (§3.4) subscribes to Jetstream with `wantedCollections=NS.*` and filters by the fk
  prefix of the rkey on the client. We need to know whether the wildcard works for a group nobody
  has written to yet, what the event shapes are, and whether `identity`/`account` events get through
  a collection filter.
- Discovery (§3.2) uses relay `com.atproto.sync.listReposByCollection` (forum directory, newcomers)
  and Constellation `getBacklinks(subject=forum, source=NS.membership:forum)`. The SPA calls both
  cross-origin, so CORS matters.

## Method

All network requests were read-only and sent with
`User-Agent: lessdugin-spike/0.0 (+https://lessdugin.7778777.online)`. Totals: 10 Jetstream
connections (each ≤ 8 s), 7 relay requests, 10 Constellation requests. Throwaway scripts lived in the
session scratch directory. Tool versions: `@atproto/syntax` 0.7.7, `@atproto/lexicon` 0.7.16,
`goat` v0.2.6, Node 22.22.0, `ws` 8.22.0 (through the sandbox's HTTPS proxy).

**(a) NSID**

```js
import { isValidNsid, ensureValidNsid, NSID, isValidRecordKey } from '@atproto/syntax'
import { Lexicons } from '@atproto/lexicon'
// for each id: isValidNsid(id), ensureValidNsid(id), NSID.parse(id).authority
// new Lexicons().add({ lexicon:1, id:'online.7778777.lessdugin.temp.post', defs:{ main:{ type:'record', key:'any', … } } })
// then lex.validate(...) and lex.assertValidRecord(...)
```

```sh
goat syntax nsid check online.7778777.lessdugin.post            # and the ids in the table below
goat syntax rkey check 'aaaaaaaaaaaa.'                          # range cursor string (ADR 0004)
goat syntax at-uri check at://did:plc:abc/online.7778777.lessdugin.post/aaaaaaaaaaaa-3lzz
# lexicons/online/7778777/lessdugin/temp/{post,defs}.json: a record with a ref to defs#media{blob}
goat lex parse lexicons/online/7778777/lessdugin/temp/*.json
goat lex lint                       # and: goat lex lint --json --lint-level info
goat lex validate -c lexicons rec.json
goat lex check-dns
goat lex resolve online.7778777.lessdugin.temp.post
```

**(b) Jetstream.** A Node `ws` client recorded the upgrade response, counted events by kind and
collection, and kept the first event of each kind:

```
wss://jetstream.us-east.bsky.network/subscribe?wantedCollections=online.7778777.lessdugin.*         8 s
wss://jetstream.us-east.bsky.network/subscribe?wantedCollections=app.bsky.feed.*                    5 s
wss://jetstream.us-east.bsky.network/subscribe?wantedCollections=online.7778777.lessdugin.*&requireHello=true&cursor=<now−5 s µs>
    then after 2.5 s: {"type":"options_update","payload":{"wantedCollections":["app.bsky.feed.post"],"wantedDids":[],"maxMessageSizeBytes":0}}
wss://jetstream.us-east.bsky.network/xrpc/network.bsky.jetstream.subscribeEvents?collections=app.bsky.feed.*                 5 s
wss://jetstream.us-east.bsky.network/xrpc/network.bsky.jetstream.subscribeEvents?collections=online.7778777.lessdugin.*      8 s ×3 (with and without subprotocol xrpc.v1.json)
…/subscribeEvents?collections=online.7778777.lessdugin.*&cursor=<now−30 s, µs>   (subprotocol xrpc.v1.json)     6 s
…/subscribeEvents?collections=online.7778777.lessdugin.*&kinds=commit&dids=…×150 | ×400                          3 s
```

The v2 lexicon (`lexicons/network/bsky/jetstream/subscribeEvents.json` in
github.com/bluesky-social/jetstream) and https://bsky.network/docs/jetstream/ were read to check the
results.

**(c) Relay**

```sh
curl -sS -A "$UA" -H 'Origin: https://lessdugin.7778777.online' -D - \
  'https://relay1.us-east.bsky.network/xrpc/com.atproto.sync.listReposByCollection?collection=site.standard.document&limit=5'
# same against https://bsky.network; also collection=online.7778777.lessdugin.temp.post,
# page 2 via cursor=, collection=site.standard.* and limit=2001
```

**(d) Constellation.** The subject was the bsky.app post
`at://did:plc:z72i7hdynmk6r22z27h6tvur/app.bsky.feed.post/3mxuhaky3rc23`.

```sh
C=https://constellation.microcosm.blue
curl -sS -A "$UA" -H 'Origin: https://lessdugin.7778777.online' -D - \
  "$C/xrpc/blue.microcosm.links.getBacklinks?subject=$U&source=app.bsky.feed.like:subject.uri&limit=3"
curl … "$C/xrpc/blue.microcosm.links.getBacklinksCount?subject=$U&source=app.bsky.feed.like:subject.uri"
# variants: percent-encoded subject and source; cursor=<page 1 cursor>; did=<one liker>; reverse=true;
# source=app.bsky.feed.like:.subject.uri (leading dot); limit=101;
# subject=at://…/online.7778777.lessdugin.temp.forum/test&source=online.7778777.lessdugin.temp.membership:forum
# legacy REST: /links/count?target=$U&collection=app.bsky.feed.like&path=.subject.uri
```

## Results

### (a) Digit NSID

| Input | `@atproto/syntax` `isValidNsid` / `ensureValidNsid` | `goat syntax nsid check` |
|---|---|---|
| `online.7778777.lessdugin.post` | `true` / ok; authority `lessdugin.7778777.online` | `valid` |
| `online.7778777.lessdugin.temp.post` | `true` / ok; authority `temp.lessdugin.7778777.online` | `valid` |
| `online.7778777.lessdugin.index.listPosts` | `true` / ok | `valid` |
| `7778777.online.post` (control) | `false` / `InvalidNsidError: NSID first part may not start with a digit` | `error: NSID syntax didn't validate via regex` (exit 255) |
| `online.7778777.lessdugin.*` (Jetstream pattern) | `false` / `Disallowed characters in NSID (ASCII letters, digits, dashes, periods only)` | `error: … didn't validate via regex` |

- `@atproto/lexicon`: `Lexicons.add` of `online.7778777.lessdugin.temp.post` and `…temp.defs`
  succeeded. `validate()` returned `{"success":true,…}` and `assertValidRecord()` passed.
- goat: `lex parse` → `success` for both files. `lex lint` → 🟢 for both, exit 0, and no findings
  even at `--lint-level info`. `lex validate` → `valid online.7778777.lessdugin.temp.post record`.
- `goat lex check-dns` derived the right TXT name:
  `_lexicon.temp.lessdugin.7778777.online TXT "did=did:web:lex.example.com"`. `lex resolve` →
  `error: NSID not associated with a DID`, as expected because nothing is published.
- Rkeys and range cursors: `aaaaaaaaaaaa-3lzzzzzzzzz2a`, `aaaaaaaaaaaa.`, `aaaaaaaaaaaa-~-x`,
  `~` and `3l.` are all valid record keys in both `@atproto/syntax` and `goat syntax rkey check`.

### (b) Jetstream

Upgrades: every connection got `101`. The `/subscribe` (v1 wire) path negotiated
`sec-websocket-extensions: permessage-deflate`. The v2 path did not, and it echoed
`sec-websocket-protocol: xrpc.v1.json` when that subprotocol was requested. Server: `openresty`.

| Connection | Duration | Events | Breakdown |
|---|---|---|---|
| v1 `wantedCollections=online.7778777.lessdugin.*` | 8.1 s | 5 (1,094 B) | identity 3, account 2, commits 0 |
| v1 `wantedCollections=app.bsky.feed.*` | 5.1 s | 1,278 (751,696 B) | create 1,256, delete 22; like 914, post 192, repost 152, threadgate 13, postgate 7 |
| v1 `…lessdugin.*&requireHello=true&cursor=now−5 s`, then `options_update` → `app.bsky.feed.post` | 5 s | 0 before the hello (2.5 s); 239 after | post 236, identity 1, account 2. The first event's `time_us` was 36 ms after the cursor |
| v2 `collections=app.bsky.feed.*` | 5.1 s | 1,219 (841,870 B) | all `{"$type":"message",…}` |
| v2 `collections=online.7778777.lessdugin.*` (×3, 8 s each) | 8 s | 0, 5, 7 | identity and account only |
| v2 `…lessdugin.*&cursor=<now−30 s µs>` | 6.3 s | 26 | identity 10, account 16 (replayed from the cursor) |
| v2 `…&kinds=commit` plus 150 `dids=` (URL 5,832 B) | 3 s | 0 | upgrade 101 |
| v2 `…&kinds=commit` plus 400 `dids=` (URL 15,332 B) | – | – | **`414 Request-URI Too Large`** (openresty), before the upgrade |

- **The wildcard works** for a group that has never been written. The upgrade succeeds, and
  `app.bsky.feed.*` matches every `app.bsky.feed.<name>` collection (5 seen).
- **`identity` and `account` events get through the collection filter** on both wires, at about
  0.6–1.2 events/s network-wide. They are not limited to matching DIDs. The v2 lexicon says so too:
  "constrains which commit events are delivered. Non-commit kinds are unaffected — combine with
  kinds=commit". One 8 s v2 run saw 0 such events, which is burstiness (three reruns saw 5, 7 and 26).

v1 wire shapes (served by the v2 hosts, which add a `cursor` = v2 `seq`):

```json
{"did":"did:plc:5ds5pid26ny3unsnys5szt3r","time_us":1792148046475800,"cursor":26085885128,"kind":"commit",
 "commit":{"rev":"3mxyfxuzrma2k","operation":"create","collection":"app.bsky.feed.like","rkey":"3mxyfxuzisy2k",
           "record":{"$type":"app.bsky.feed.like","createdAt":"…","subject":{"cid":"bafyrei…","uri":"at://…"}},
           "cid":"bafyreifzg3yjby545nkudj5pwsjd2pppbddnv6a76k3ygbb5bpwdtpxrxm"}}
{"did":"did:plc:u2gyoqc3qkqgtwgyierpao4c","time_us":1792148046973949,"cursor":26085885285,"kind":"commit",
 "commit":{"rev":"3mxyfxvdamw2r","operation":"delete","collection":"app.bsky.feed.like","rkey":"3mxq6ijh5t42r"}}
{"did":"did:plc:5rynjuxfutdvpng7zmqwpvuo","time_us":1792148032672828,"cursor":26085881067,"kind":"identity",
 "identity":{"did":"did:plc:5rynjuxfutdvpng7zmqwpvuo","seq":34538408283,"time":"2026-10-16T10:53:52.491Z"}}
{"did":"did:plc:5rynjuxfutdvpng7zmqwpvuo","time_us":1792148032704964,"cursor":26085881089,"kind":"account",
 "account":{"active":true,"did":"did:plc:5rynjuxfutdvpng7zmqwpvuo","seq":34538408284,"time":"2026-10-16T10:53:52.491Z"}}
```

v2 (`network.bsky.jetstream.subscribeEvents`) shapes. Every frame is an envelope, and `seq` is the
cursor:

```json
{"$type":"message","payload":{"$type":"network.bsky.jetstream.subscribeEvents#commit",
  "cid":"bafyreiarbommdbe24mq6atxsq4ms6dzmkh2nlyagyn35bh7zdaardjmr2q","collection":"app.bsky.feed.like",
  "did":"did:plc:fryfdw4hqwzesnjl6km6aygs","operation":"create","record":{…},"rev":"3mxyfycmgv32r",
  "rkey":"3mxyfycm63t2r","seq":26085889470,"time":"2026-10-16T10:54:20.801615Z","witnessedAt":"2026-10-16T10:54:20.801615Z"}}
{"$type":"message","payload":{"$type":"network.bsky.jetstream.subscribeEvents#identity","did":"did:plc:crph7fbsr3itpsxwxl75gbco",
  "identity":{"did":"did:plc:crph7fbsr3itpsxwxl75gbco","seq":34538432224,"time":"2026-10-16T10:55:14.135Z"},
  "seq":26085905372,"time":"2026-10-16T10:55:14.272973Z","witnessedAt":"2026-10-16T10:55:14.272973Z"}}
{"$type":"message","payload":{"$type":"network.bsky.jetstream.subscribeEvents#account",
  "account":{"active":true,"did":"did:plc:crph7fbsr3itpsxwxl75gbco","seq":34538432227,"time":"2026-10-16T10:55:14.135Z"},
  "did":"did:plc:crph7fbsr3itpsxwxl75gbco","seq":26085905407,"time":"2026-10-16T10:55:14.300303Z","witnessedAt":"…"}}
```

From the v2 lexicon:

- **Message union:** `#commit | #identity | #account | #sync | #info`. `#sync` means a broken commit
  chain and that the consumer should resync the repo; it is never sent on the v1 wire. `#info`
  (`OutdatedCursor`, `FutureCursor`) carries no seq.
- **Cursor:** inclusive. A value ≥ 1e15 is read as a unix-µs timestamp and translated to the first
  seq witnessed at or after it.
- **Filter limits:** at most 100 collections and 10,000 DIDs, enforced before the upgrade.
- **Changing filters:** v2 has no `options_update`. To change filters, reconnect.
- **Compression:** optional dict-zstd (`zstdDictionary`).
- **Endpoints:** v2 is `jetstream.{us-east,us-west}.bsky.network`. Legacy v1 is
  `jetstream1.us-west` and `jetstream2.us-east`. The v2 hosts also serve `/subscribe`, as tested above.

### (c) Relay `listReposByCollection`

| Host | Collection | Status, time | Body |
|---|---|---|---|
| relay1.us-east.bsky.network | `site.standard.document`, limit 5 | 200, 0.31 s | `{"cursor":"QQAADocAAAGbfNeg32RpZDpw…","repos":[{"did":"did:plc:5i57akaxijt2xu7d3nmfbr5w"},…5]}` |
| relay1.us-east.bsky.network | `online.7778777.lessdugin.temp.post` | 200, 0.57 s | `{"repos":[]}` (no cursor) |
| bsky.network | `site.standard.document`, limit 5 | 200, 0.33 s | the same 5 DIDs; cursor `QQAADtcAAAGbfNeg5WRpZDpw…` |
| bsky.network | `online.7778777.lessdugin.temp.post` | 200, 0.30 s | `{"repos":[]}` |
| bsky.network | page 2 (`cursor=`) | 200 | 5 new DIDs and a new cursor |
| bsky.network | `site.standard.*` | **400** | `{"error":"BadRequest","message":"bad collection nsid, NSID syntax didn't validate via regex"}` |
| bsky.network | `limit=2001` | 200 | **2,001** repos (7 of them did:web). The lexicon maximum is not enforced |

- Each repo entry carries only `did`: no `rev` and no `active`.
- CORS on every response: `access-control-allow-origin: *` and `vary: Origin`.
- No `RateLimit-*` headers.

### (d) Constellation

- `getBacklinks?subject=<at-uri>&source=app.bsky.feed.like:subject.uri&limit=3` → 200:
  `{"total":2705,"records":[{"did":"did:plc:s3zjqmg3j73hbosw5dbel7hc","collection":"app.bsky.feed.like","rkey":"3mxyf6jwwtw2p"},{"did":"did:plc:avqcdzhb7wa2h3fc2bu7eqty",…,"rkey":"3mxyejux2dt2y"}],"cursor":"fbc40a33fbc10a"}`.
  Only **2** records came back for `limit=3`. Page 2 (`cursor=`) returned 3 records and a new cursor.
  `limit=101` was accepted and returned **98** records. Pages can be short even when more exist.
  Order is newest first.
- `getBacklinksCount` (same params, no limit) → `{"total":2705}`. The legacy
  `/links/count?target=…&collection=app.bsky.feed.like&path=.subject.uri` → `{"total":2705}`.
- A percent-encoded `subject` and `source` (`%3A`) return the same results as raw `at://…` and `:`.
- `did=did:plc:s3zjqmg3j73hbosw5dbel7hc` → `{"total":1,"records":[…that one like…],"cursor":null}`.
  This filters by the linking repo.
- `reverse=true` → oldest first (`3mxuhb527gl26`, `3mxuhbb5hbp2n`).
- `source=app.bsky.feed.like:.subject.uri` (leading dot) → **400 with an empty body**. In the XRPC
  API the path has no leading dot; the legacy API uses one.
- `source=online.7778777.lessdugin.temp.membership:forum` with a forum at-uri subject →
  `{"total":0,"records":[],"cursor":null}`. The digit NSID is accepted.
- CORS: `access-control-allow-origin: *`, `-methods: *`, `-headers: *`, `-expose-headers: *`,
  `-max-age: 86400`. The headers are present on 400 responses too. Server: nginx.
- No rate-limit headers.

## Decision

1. **Keep `online.7778777.lessdugin`.** Every tool on our path accepts the digit segment: the
   TypeScript syntax and lexicon packages; goat syntax, parse, lint, validate and check-dns; the
   relay's NSID regex; Jetstream; and Constellation. Our Rust NSID validator (crates/atp) gets
   these vectors:
   - `online.7778777.lessdugin.post` → valid;
   - `7778777.online.post` → invalid;
   - `online.7778777.lessdugin.*` → invalid as an NSID.

   Jetstream patterns get their own type (`NsidPattern = Nsid | "<nsid-prefix>.*"`). They never pass
   through the NSID parser.
2. **Jetstream: v2 endpoint by default, with the v1 wire as the DID-filtered mode.** This replaces
   the endpoint line in §3.4.
   - **Default:**
     `wss://jetstream.{us-east,us-west}.bsky.network/xrpc/network.bsky.jetstream.subscribeEvents?collections=NS.*&cursor=…`
     with subprotocol `xrpc.v1.json`. Parse the `{"$type":"message","payload":{…}}` envelope, and
     log and ignore unknown payload `$type`s.
   - **Cursor:** persist `seq`, and also `witnessedAt` as µs. On the same instance, resume with
     `cursor=seq`; it is inclusive, so deduplicate. On failover to the other instance, resume with
     `cursor = µs(witnessedAt) − 5 s`, because `seq` is per instance. Handle `#info OutdatedCursor`
     with a full reconcile; this replaces the "gap > 36 h" heuristic when it fires.
   - **New event row in §3.4:** `sync` → mark the repo dirty at P1 (full re-list).
   - **identity/account:** they arrive network-wide (about 1/s) regardless of the collection
     filter. The client keeps those whose DID is in the identity cache and drops the rest. This
     costs about 0.3 KB/s, so no separate connection is needed.
   - **DID-filtered mode (> 50 events/s):** v2 has no `options_update`, and the request line is
     capped by openresty (150 DIDs at 5.8 KB pass; 400 DIDs at 15.3 KB get 414). So:
     - up to 150 DIDs: v2 with `dids=` in the query;
     - more than 150 DIDs: the v1 wire
       `/subscribe?wantedCollections=NS.*&requireHello=true&cursor=<time_us>` plus
       `options_update{wantedDids ≤ 10k}` (verified: nothing arrives before the hello, and the
       filter applies immediately after it).

     Both wires go behind one `LiveSource` trait.
   - **S6 confirmed:** commit events carry `collection` and `rkey`, so the fk-prefix filter
     (`rkey == fk || rkey.starts_with(fk + "-")`) runs before any fetch. Our wildcard sees no
     traffic today, so the "> 50 events/s" switch is far off.
   - The v2 commit carries `record` and `cid`, but these stay unverified hints (§1.3). Point
     fetches still go to the PDS or the indexer as specified.
3. **Relay directory:**
   - Call `listReposByCollection` directly from the SPA. CORS is `*` and it is a simple GET, so
     there is no preflight.
   - Send `limit=500`. Do not rely on server-side clamping (2,001 was honoured).
   - Treat a missing `cursor` as the end.
   - Entries carry only `did`, so the existing `listRecords(NS.forum)` follow-up stays.
   - Wildcards are rejected: query the forum NSID explicitly per relay.
4. **Constellation newcomers:** the exact call is

   ```
   GET https://constellation.microcosm.blue/xrpc/blue.microcosm.links.getBacklinks?subject=<forum at-uri>&source=<NS>.membership:forum&limit=100[&cursor=…]
   ```

   - The path has no leading dot; nested fields use dots (`subject.uri`).
   - Keep paging while `cursor != null`. A short page does not mean the end.
   - Use `getBacklinksCount` for the "N members (hint)" badge.
   - Records come as `{did, collection, rkey}` with no CID, so each one is only a candidate,
     confirmed by `getRecord(NS.membership, rkey=fk)` at the PDS (as in §3.2). A record whose `rkey`
     is not `fk(forum)` is dropped without a fetch.
   - Use `did=` filters for "is X a member (hint)" checks.
5. **No custom headers in browser calls** to these services; the relay, Constellation and Jetstream
   need none.

## Consequences

- M1 adds NSID and pattern vectors (decision 1) to `testdata/vectors`.
- M5's mini-Jetstream used in devnet tests must speak both wires: the v2 envelope with `seq`,
  `#sync` and `#info`, and v1 `/subscribe` with `requireHello`/`options_update`. Its fixtures
  include identity/account events for unrelated DIDs.
- The fake Constellation must return short pages that still have a cursor, so the client's paging
  loop is exercised.
- Open: whether the public v1 hosts will be retired. If they are, the > 150-DID mode becomes
  several v2 connections of ≤ 150 DIDs each, or relies on the indexer.
