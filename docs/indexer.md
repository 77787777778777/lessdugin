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
loop (`--poll-interval-ms`, default 5000; 250 ms is useful for a loopback devnet). Poll mode is
particularly useful without a relay or Jetstream. Relay backfill runs every six hours, scanning
forum declarations and membership records. A membership is only a forum-discovery hint; normal
sync must still admit it before that author becomes a participant. Durable candidate rows retain
those hints across restarts; they never become participant rows without PDS confirmation. A
failed discovery or forum sync retries after one minute instead of waiting for the next six-hour
pass.
Daily reconcile makes one `getLatestCommit` head check per unique indexed owner/participant DID and
refreshes only forums containing a repo whose head moved; an individual head failure is logged and
does not stop checks of other repos. Two configured Jetstream endpoints are consumed concurrently
with a shared deduplicator and independent durable cursors. Reconnects rewind ten seconds, and
cursors advance only after admission, proof acquisition and snapshot persistence. A failed event
therefore remains unacknowledged and is replayed after reconnect; a permanently failing record can
currently stall later frames on that stream rather than being silently skipped.

The blocklist contains one DID, at-uri, CID or host per line; blank lines and `#` comments are
ignored. Host entries also match `host:port` and exact origins. It is applied when answering
queries, not as a content-admission or moderation mechanism. Restart after editing it. Outbound
HTTP is limited to five requests/second and four requests in flight per host,
with 200 in flight globally. Inbound requests are limited to 20/second/IP and use XRPC
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
ETag. All routes allow CORS `*`. `listPosts` and `getThread` expose the core fold's real
`inputDigest`; their values and tallies remain hints. `getRecordProofs` accepts at most 25 `uris`
and returns an `application/vnd.ipld.car` manifest bundle. Failed items are omitted; absence proofs
have `cid: null`.

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

`.github/workflows/indexer.yml` builds and smoke-tests the image without publishing on ordinary
branch pushes. An explicit workflow dispatch publishes `ghcr.io/<owner>/lessdugin-indexer:sha-<full
commit>`; a push to the default branch also publishes `latest`. This is the M6 image pipeline,
not the M7 release workflow. The workflow has only `contents: read` and `packages: write`.

## Checks

```sh
nix develop --impure -c cargo test -p lessdugin-indexer
nix develop --impure -c cargo clippy -p lessdugin-indexer --all-targets -- -D warnings
```

The integration tests use signed repositories and real loopback HTTP PDS/PLC fixtures. They cover
all nine methods, durable poll convergence within five seconds, restart continuity, proof manifest
verification, omission and forged-bundle detection. They do not contact production services.
