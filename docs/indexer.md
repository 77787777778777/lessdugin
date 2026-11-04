# Operating the optional indexer (M6)

`lessdugin-indexer` is a native, unauthenticated read service. It uses the same core admission,
record store and full social fold as the native client. It is **not** an authority for forum
content: every JSON field is a discovery/ordering hint, and only the CAR proofs are independently
verifiable. Clients must recompute displayed values and must not display hint text as attributed
content. The service does not fetch or store blobs and does not serve P2 search, backlink, activity,
karma, tally or `getRecords` methods.

## Configuration

```sh
lessdugin-indexer \
  --listen 127.0.0.1:2584 \
  --data-dir /var/lib/lessdugin-indexer \
  --did did:web:indexer.example.org \
  --source-url https://code.example.org/lessdugin/tree/v0.0.1 \
  --relay https://relay.example.org \
  --jetstream wss://jetstream1.example.org \
  --jetstream wss://jetstream2.example.org \
  --blocklist /etc/lessdugin/indexer-blocklist
```

Use the ordinary native `config.toml` in the data directory for PLC/devnet settings. Repeat
`--forum <at-uri>` to seed declarations directly. `--poll <pds-url>` enables the durable polling
loop (`--poll-interval-ms`, default 5000; 250 ms is useful for a loopback devnet) and is
particularly useful without a relay or Jetstream. `--poll` selects a source, never a network: it
keeps the configured profile, and outside devnet mode the URL must be `https`. Devnet mode needs
`--dev` (with `--poll` naming the devnet PDS) or a devnet `config.toml` profile, and a
development build (a debug build, or `--features devnet`); release builds refuse both.

Relay backfill (discovery) runs every six hours, scanning forum declarations and membership
records; a failed discovery retries after one minute, doubling up to six hours. A membership is
only a forum-discovery hint; normal sync must still admit it before that author becomes a
participant. Durable candidate rows retain those hints across restarts; they never become
participant rows without PDS confirmation. Hints are capped per DID and per PDS host (32 forum
seeds per DID, 2,000 per host, 500 pending membership hints per forum and host), so one account
cannot fill the index with seeds. A live membership hint counts against the host named by the
newcomer's indexed head or cached DID document (no identity lookup on the live path); hints of
hosts not known yet share one 500-hint quota per forum. An indexed forum counts against its
owner's host. A forum whose sync fails backs off on its own (one minute,
doubling up to six hours) without re-running discovery. A seed whose declaration is proven
absent is quarantined: it is no longer listed or synced, is rechecked once a day (a failed
recheck also waits a day), or as soon as a live commit shows the declaration again, and is
dropped a week after it was first found absent. A forum that was indexed
and whose declaration is deleted logs a delete for each of its records. Only `RecordNotFound`
for the declaration, from the PDS that a freshly fetched DID document names, counts as absence:
a deactivated, taken-down or suspended owner, a bare 404 or an owner DID that does not resolve
only backs the forum off, and its records stay served. A seed found through a hint that has
never synced is dropped after a week of failed syncs; `--forum` seeds are kept.

Daily reconcile makes one `getLatestCommit` head check per unique indexed owner/participant DID and
refreshes only forums containing a repo whose head moved; an individual head failure is logged and
does not stop checks of other repos. The same daily pass drops tombstones older than 30 days from
the record store, and cached proofs of records no forum indexes any more.

Two configured Jetstream endpoints are consumed concurrently with a shared deduplicator and
independent durable cursors; reconnects rewind ten seconds. A live commit is point-fetched and
the affected forums are re-projected from the cache, so a write is served within seconds without
a forum-wide sync; only membership and declaration changes request an ordinary forum sync, debounced
by two seconds and spaced by 200 ms per participant after the last one (ten minutes at 3,000
participants), so membership writes cannot keep a large forum in back-to-back syncs. Fetches of different frames run concurrently, each bounded to 30 seconds. Cursors advance
over the completed prefix of frames: a frame is complete when it was projected or, after any
failure, when its repository was handed to the durable per-repo retry (two seconds, doubling up
to six hours). One failing record, PDS or deleted forum therefore never stalls the stream. Live
proofs are fetched in the background; a proof that cannot be fetched is simply not cached. An
oversized message is skipped after two connections fail on it at one position (the stream
resumes one second later and a reconcile is scheduled); a connection with no frame for two
minutes is replaced.

The blocklist contains one DID, at-uri, CID or host per line; blank lines and `#` comments are
ignored. Host entries also match `host:port` and exact origins, and apply to every method: the
host's records, participant entries and repo heads are hidden, and forums owned on it are no
longer listed. It is applied when answering queries, not as a content-admission or moderation
mechanism. Restart after editing it. Outbound HTTP is limited to five requests/second and four
requests in flight per host, with 200 in flight globally (a request waits for its host's budget
before taking a global slot). Inbound requests are limited to 20/second/IP and use XRPC
`RateLimitExceeded` errors.

The state database contains forum/change/cursor/proof side tables. Change entries are retained for
30 days; `getChanges` reports `CursorTooOld` below the durable retention floor. Back up the data
directory for gap-free restarts. The cache can be rebuilt from authors' PDSes.

## HTTP surface

The nine GET methods are under
`/xrpc/online.7778777.lessdugin.index.` (or the build's staging namespace):

- `describeIndexer`, `listForums`, `listParticipants`, `getRepoHeads`, `getChanges`
- `listPosts`, `listRecentDiscussion`, `getThread`, `getRecordProofs`

JSON responses include `asOf` and `indexer`. List responses have `Cache-Control: max-age=5` and an
ETag. All routes allow CORS `*`. Absent optional fields are left out, never `null`. `listForums`
and `describeIndexer` list only synced forums. `listPosts` and `getThread` expose the core fold's
real `inputDigest`; their values and tallies remain hints. Queries read each forum's last
folded model. It is fresh for at least 60 s or 30 times its fold cost; past that it is still
served while one background fold refreshes it (one query fold at a time, never in the way of live
projections), so a request folds only a forum with no model yet, after a restart or an eviction
(metrics `query_model_builds_total`, `query_model_refreshes_total` and
`query_model_refresh_failures_total`; a failed refresh is retried after the age bound). `getRecordProofs` accepts at most 25 `uris`
and returns an `application/vnd.ipld.car` manifest bundle. It serves cached proofs and fetches at
most two missing ones at a time (a request waits at most five seconds in all for a slot); failed or
not-yet-available items are omitted; absence proofs have `cid: null`.

Operational endpoints:

- `GET /_health`: process and index snapshot status.
- `GET /metrics`: Prometheus text counters (sync, live, proof, HTTP and rate-limit activity).
- `GET /`: trust notice, health/metrics links and the operator-configured corresponding-source URL.

Set `--source-url` to the exact corresponding AGPL source for the running build, not merely a
project home page. To make the service discoverable, an operator may publish a `did:web` document
with a `#lessdugin_index` service of type `LessduginIndex`; publication and deployment are outside
M6.

## Container

`crates/indexer/Dockerfile` is a multi-stage build whose runtime is distroless `cc-debian12` and
whose user is non-root. Build it with the repository root as context and mount persistent storage
at `/data`. Pass `SOURCE_URL` as a build argument so `/` links to the exact corresponding source.

`.github/workflows/indexer.yml` builds and smoke-tests the image without publishing on pull
requests and on pushes to `claude/**` branches. An explicit workflow dispatch publishes `ghcr.io/<owner>/lessdugin-indexer:sha-<full
commit>`; a push to the default branch also publishes `latest`. This is the M6 image pipeline,
not the M7 release workflow. The workflow has only `contents: read` and `packages: write`.

## Checks

```sh
nix develop --impure -c cargo test -p lessdugin-indexer
nix develop --impure -c cargo clippy -p lessdugin-indexer --all-targets -- -D warnings
```

The integration tests use signed repositories, real loopback HTTP PDS/PLC fixtures and the
testkit mini-Jetstream. They cover all nine methods, live ingest on two endpoints within five
seconds (also in a 200-participant forum, without a forum-wide sync), failing fetches, missing
proofs and a deleted declaration that do not stall the stream, a restart that resumes from the
persisted cursors, durable poll convergence, a slow origin that cannot hold the global request
budget, proof manifest verification, omission and forged-bundle detection. Store tests count
SQLite VM steps, so lookup cost is checked without timing. They do not contact production
services.
