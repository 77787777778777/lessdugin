<!-- SPDX-License-Identifier: CC0-1.0 -->

# lessdugin limits (normative)

**Status:** draft for the `.temp` namespace (M0). Limits of class N and L (§2) freeze at publication (M7).
**Sources:** `docs/design/synthesis-v1.md` §5.3, plus every numeric limit in §2–§6, with the fixes from `docs/design/completeness-review.md` and the approved plan applied, and the spike results in ADR 0003 and ADR 0004. Where they differ, the plan wins, then the ADRs.
**Mirror:** `crates/core/src/limits.rs` defines one constant per row that names one (§21). **Vectors:** `testdata/vectors/limits/`.

## 0. About this document

**Key words.** MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are used as in RFC 2119 and RFC 8174. "Reserved (Pn)" and "Reserved (deferred)" are used as in `social.md` §0.1 and `threat-model.md` §0.

**Scope.** Every byte, count, depth, time, concurrency and budget limit of lessdugin, in one place. Field limits of records (string lengths, array sizes, blob `maxSize`) live in `data-model.md` §4–§6 and §11; this document lists only the ones code uses outside the lexicon validator (§4).

**Precedence.** Values here win over values repeated in other documents. A rule's *meaning* is defined by the document named in the "Defined in" column; this document fixes its numbers.

**Reading the tables.** Each row gives: the constant name mirrored in `limits.rs` (or `–` if the value is not mirrored), the exact value, the class (§2), what it applies to and what happens when it is hit, and where the rule is defined.

---

## 1. Units and counting

| Term | Meaning |
|---|---|
| bytes | octets. 1 KB = 1,000; 1 MB = 1,000,000; 1 KiB = 1,024; 1 MiB = 1,048,576; 1 GiB = 1,073,741,824. Every row also states the exact integer. |
| response bytes | bytes of an HTTP or WebSocket body **after** content decoding (gzip, deflate, br, zstd), counted as they arrive |
| graphemes | extended grapheme clusters as computed by the pinned `unicode-segmentation` (`data-model.md` §2.2) |
| code points | Unicode scalar values |
| nesting depth | for JSON and DAG-CBOR: the number of containers (maps and arrays) on the path from the root to the deepest value; a root container has depth 1, a scalar root depth 0. Depth is counted on the atproto data model, not on its JSON spelling: a `{"$link": …}` or `{"$bytes": …}` object is a leaf (a CID or bytes value), so it adds no level, and the same record has the same depth in JSON and in DAG-CBOR. For markdown: `lfm-1.md` §8.4. For MathML: elements inside `math`, where `math` is 1 |
| durations | integers in milliseconds (`_MS`) unless the name says otherwise; human forms in parentheses are informative |
| ranges | "a–b" for a randomised delay means a uniform draw from the closed interval [a, b] |
| bounds | inclusive: "≤ N" admits N and rejects N + 1 |

---

## 2. Classes and change rules

| Class | Meaning | Change rule |
|---|---|---|
| **N** | Normative behaviour: part of a behaviour ID (`mod/1`, `karma/1`, `magic/1`, `lfm/1`, `default/1`, `sk/1`, `fk/1`, `proofs/1`) or of record validity. Every client (native, wasm, indexer) applies it identically, so it affects fold outputs, rendered semantics or verification results. | Never changes. A different value is a new behaviour ID (for example `proofs/2`), whose constants are added beside the old ones. |
| **L** | Lexicon field limit. Defined in `data-model.md`; enforced by the validator from the embedded lexicon JSON. | Frozen at first publication (`data-model.md` §13). |
| **W** | Writer rule: what lessdugin itself writes, and how. Readers never depend on it. | Any release. |
| **S** | Safety cap against hostile or broken peers. Hitting one makes the affected item *unavailable* or the repo `[partial]` (§3); it never yields a different view of fully fetched data. | Any release, with its vectors updated. |
| **P** | Presentation, versioned by `RENDERER_VERSION` (`lfm-1.md` §1.2). | Any release that bumps `RENDERER_VERSION`. |
| **O** | Operational: scheduling, budgets, caches, presentation outside the renderer, build gates. | Any release. |

---

## 3. Rules when a limit is hit

| ID | Rule |
|---|---|
| R1 | **Unknown is not absent.** A fetch that hits an S cap, times out or fails yields *unavailable*. It creates no tombstone and deletes nothing; the repo or item is shown `[partial]` or "unavailable" (`social.md` §1.5). |
| R2 | **No silent truncation.** Content cut by a cap carries a visible notice (`lfm-1.md` §8.6) or a `[partial k/n]` badge. A listing that stopped at a cap never claims completeness (`sync.md` I6). |
| R3 | **Streaming enforcement.** Byte caps are checked as bytes arrive, after content decoding, and the transfer is aborted at the first byte past the cap. `Content-Length` and every other declared length (CAR varints, CBOR headers, MP4 box sizes, image dimensions) are never used to size an allocation (`threat-model.md` PILL-3). |
| R4 | **Host accounting.** A response that breaks an S cap, a listing rule (§10.1) or a CID check counts as one failure toward that host's circuit breaker (§7). |
| R5 | **N before S.** N-class limits are applied identically by every client. When an S cap stops a fetch before an N cap would, the result is `[partial]`, and the client makes no completeness claim for that repo. |
| R6 | **Honest data.** S caps are set well above typical honest data. Honest data that hits one still gets R1 and R2, never a wrong view. |

---

## 4. Lexicon and validity limits

Only the limits that code uses outside the lexicon validator are listed. Every other field limit is in `data-model.md` §4 (`NS.defs`), §5 (`NS.markdown`), §6 (records) and §11 (indexer queries).

| Constant | Value | Class | Applies to; on hit | Defined in |
|---|---|---|---|---|
| `RECORD_CBOR_MAX_BYTES` | 1,000,000 | N (validity) | DAG-CBOR encoding of a record value. Larger: the record is invalid (rule G6). | `data-model.md` §7.2 |
| `MARKDOWN_TEXT_MAX_BYTES` | 100,000 | L | `markdown.text`. Writers refuse a longer non-spillable source at compose time. | `data-model.md` §5.1, §5.3 |
| `MARKDOWN_TEXTBLOB_MAX_BYTES` | 1,000,000 | L | `markdown.textBlob` `maxSize`; also the fetch cap for a body blob (§5) | `data-model.md` §5.1 |
| `SPILL_THRESHOLD_BYTES` | 65,536 (64 KiB) | W | A spillable `md` field whose UTF-8 source is longer is written as `textBlob`. | `data-model.md` §5.3 |
| `WRITE_BODY_MAX_BYTES` | 900,000 | W | The exact JSON request body of `createRecord`, `putRecord` or `applyWrites`. Larger: spill the largest spillable field and re-measure; if nothing is left to spill, refuse with `TooLarge`. | `data-model.md` §8.9 |
| `URI_MAX_BYTES` | 3,000 | L | `atu` and `url` fields; also the input limit of `safe_href` (§13.1) | `data-model.md` §2.1 |
| `KNOWN_VALUE_MAX_BYTES` | 64 | L | `kv{…}` fields, slugs | `data-model.md` §2.1 |
| `LOCK_ALLOWED_MAX` | 5,000 | L, W | `#lock.allowed`. A writer refuses a lock whose computed allow-list is longer, and tells the moderator to use hide. | `data-model.md` §6.7, §8.6 |
| `REACTS_PER_OPINION_MAX` | 32 | L | `opinion.reacts` | `data-model.md` §6.6 |
| `TAG_VOTES_PER_OPINION_MAX` | 32 | L | `opinion.tags` | `data-model.md` §6.6 |
| `MEDIA_PER_BODY_MAX` | 128 | L | `markdown.media` | `data-model.md` §5.1 |
| `INDEXER_PARAM_URIS_MAX` | 25 | L | at-uri array parameters of indexer queries | `data-model.md` §11.1 |
| `INDEXER_PARAM_DIDS_MAX` | 50 | L | DID array parameters of indexer queries | `data-model.md` §11.1 |
| `INDEXER_CURSOR_MAX_BYTES` | 512 | L | opaque indexer cursors (except `getChanges`) | `data-model.md` §11.1 |

**Policy parameters** (`pairCap` 25, `establish` 1, `establishSeeds` 2, `offsetHours` 2, `exponentMilli` 1150, `curatedBonus` 10, `frontpageBonus` 0, `cutoffDays` 90, `collapseBelow` −5, and the gate defaults) are per-forum values with lexicon ranges and defaults. They are not limits: see `data-model.md` §6.1 and `social.md` §6, §8 and §12.

---

## 5. Network byte caps

Every cap counts response bytes (§1) and follows R1–R4. A request that names no cap below uses `DEFAULT_RESPONSE_MAX_BYTES`.

| Constant | Value | Class | Applies to; on hit | Defined in |
|---|---|---|---|---|
| `DEFAULT_RESPONSE_MAX_BYTES` | 4,194,304 (4 MiB) | S | Any HTTP response not listed below. | here |
| `IDENTITY_RESPONSE_MAX_BYTES` | 65,536 (64 KiB) | S | DoH JSON answers; PLC and did:web DID documents; `/.well-known/oauth-protected-resource`; OAuth authorization-server metadata. Larger: that resolution fails (the identity is unresolved, not invalid). | here |
| `HANDLE_WELLKNOWN_MAX_BYTES` | 2,048 | S | `https://<handle>/.well-known/atproto-did` (a DID is at most 2,048 characters). Larger: the answer is `NoAnswer`. | `sync.md` §3.1 |
| `RECORD_JSON_MAX_BYTES` | 1,000,000 | S | The JSON body of one `getRecord` response (envelope and value). A conforming writer's whole request body is ≤ 900,000 bytes (`WRITE_BODY_MAX_BYTES`), so every record it wrote fits. | here |
| `LIST_PAGE_MAX_BYTES` | 4,194,304 (4 MiB) | S | One `listRecords` page. Larger: abort and retry the same cursor with `limit` 10, then 1 (`LIST_PAGE_RETRY_LIMITS`, §10.1). If a one-record page is still larger, the range is `[partial]`. | here |
| `RANGE_MAX_BYTES` | 52,428,800 (50 MiB) | S | The sum of response bytes of every page fetched by one `list_range(did, collection, fk)` call in range mode. Larger: stop; the range is `[partial]`. Applies to every ranged collection except `opinion`. The full-collection fallback uses `FULL_LIST_MAX_*` instead. | `sync.md` §5.3 |
| `RANGE_MAX_BYTES_OPINION` | 104,857,600 (100 MiB) | S | The same for `opinion`, whose fetch cap is 5× larger (§9). | `sync.md` §5.3 |
| `FULL_LIST_MAX_BYTES` | 209,715,200 (200 MiB) | S | The sum of response bytes of a full-collection listing (the fallback for hosts without range cursors), whatever the collection. Larger: stop; the range is `[partial]`. | `sync.md` §5.3 |
| `FULL_LIST_MAX_PAGES` | 5,000 | S | Pages of one full-collection listing; a repeated cursor also stops it. More: stop; the range is `[partial]`. | `sync.md` §5.3 |
| `PROOF_CAR_MAX_BYTES` | 1,000,000 | N (`proofs/1`) | A `sync.getRecord` CAR. Larger: the proof is `Err`. For an item of an indexer bundle, the bytes of the blocks its verification reads. | `sync.md` (`proofs/1`) |
| `PROOF_CAR_MAX_BLOCKS` | 2,000 | N (`proofs/1`) | Blocks in one proof CAR, or blocks one bundle item's verification reads. More: `Err`. | `sync.md` (`proofs/1`) |
| `INDEXER_BUNDLE_MAX_BYTES` | 8,388,608 (8 MiB) | S | One `getRecordProofs` response. Larger: retry with the `uris` list halved, down to one uri; an item still too large is fetched from the author's PDS instead. | here |
| `INDEXER_BUNDLE_MAX_BLOCKS` | 50,000 | S | Blocks in one `getRecordProofs` CAR (= 25 × `PROOF_CAR_MAX_BLOCKS`). More: the response is rejected. | here |
| `INDEXER_RESPONSE_MAX_BYTES` | 8,388,608 (8 MiB) | S | Every indexer JSON response. Larger: the hint is dropped. | here |
| `JETSTREAM_MESSAGE_MAX_BYTES` | 1,200,000 | S | One Jetstream WebSocket message. Natively: the WebSocket library's maximum message and frame size. On the v1 wire: also sent as `maxMessageSizeBytes`. In the browser: an oversized message is dropped on receipt and the client fails over to the other endpoint. | `threat-model.md` INFRA-5 |
| `BLOB_TEXT_MAX_BYTES` | 1,000,000 | S | A `textBlob` body fetch (equal to the lexicon `maxSize`). | `lfm-1.md` §1.3 |
| `BLOB_HARD_MAX_BYTES` | 314,572,800 (300 MiB) | S | Any blob fetch or upload. | here |
| – | min(declared `size`, the field's lexicon `maxSize`, `BLOB_HARD_MAX_BYTES`) | S | The `max` passed to `Host::fetch_blob`. A body longer than the declared size is aborted. | `threat-model.md` §5.6.1 |
| `BLOB_AUTOLOAD_MAX_BYTES` | 5,242,880 (5 MiB) | O | Inline images up to this size may load without a click (autoload policy); larger ones are click-to-load. | `threat-model.md` PRIV-1 |
| `EXTERNAL_IMAGE_MAX_BYTES` | 5,242,880 (5 MiB) | S | Opted-in external images (`lfm-1.md` §6.4 "≤ 5 MB" means this value). | `lfm-1.md` §6.4 |
| `BLOB_SNIFF_PREFIX_BYTES` | 4,096 | O | The prefix of a stored blob that the core reads with `Host::blob_read` for type sniffing. | `sync.md` §11.1 |

---

## 6. Parsing and decoding caps

| Constant | Value | Class | Applies to; on hit | Defined in |
|---|---|---|---|---|
| `NESTING_MAX_RECORD` | 32 | N (validity) | Nesting depth of a record value, in JSON or DAG-CBOR. Deeper: the record is invalid (rule G10): it is stored as an invalid version, excluded from the fold and shown in the raw view with the rule ID. | `data-model.md` §7.2 (G10); `threat-model.md` PILL-2 |
| `NESTING_MAX_DOCUMENT` | 36 | S | Nesting depth of any JSON response document or DAG-CBOR block (32 plus the deepest envelope, `listRecords`' 3 levels, plus 1). Deeper: the response is rejected whole (R1, R4): nothing in it is admitted, tombstoned or replaced. | `threat-model.md` PILL-2 |
| `MST_DEPTH_MAX` | 32 | N (`proofs/1`) | MST layers walked from the commit's `data` root to the key. Deeper: `Err`. | `sync.md` (`proofs/1`) |
| `FRONTMATTER_MAX_BYTES` | 65,536 | S | A draft's YAML frontmatter. Larger: the draft is refused. | `threat-model.md` TERM-10 |
| `FRONTMATTER_DEPTH_MAX` | 32 | S | YAML nesting depth in frontmatter. | `threat-model.md` TERM-10 |
| `FRONTMATTER_ALIASES_MAX` | 0 | S | YAML anchors and aliases are rejected. | `threat-model.md` TERM-10 |
| `IMAGE_DECODE_MAX_INPUT_BYTES` | 67,108,864 (64 MiB) | S | Encoded image bytes given to a decoder (TUI display, native upload pipeline). Larger: placeholder (display) or refusal (upload). | `threat-model.md` SUP-9 |
| `IMAGE_DECODE_MAX_SIDE` | 8,192 | S | Width and height in pixels, each. | `threat-model.md` SUP-9 |
| `IMAGE_DECODE_MAX_ALLOC` | 268,435,456 (256 MiB) | S | Decoder allocation limit (one 8,192² RGBA frame). | `threat-model.md` SUP-9 |
| `GIF_DECODE_MAX_FRAMES` | 200 | S | Frames decoded from one GIF. | `threat-model.md` SUP-9 |
| `IMAGE_DECODE_TIMEOUT_MS` | 10,000 (10 s) | S | Wall time of one off-thread decode. Longer: abort; placeholder or refusal. | `threat-model.md` SUP-9 |
| `MEDIA_CONTAINER_MAX_DEPTH` | 32 | S | Nesting of MP4 boxes and RIFF chunks walked by the sniffer and the strippers. Deeper: the file is refused for upload, or treated as an unknown type for display. | `threat-model.md` §5.9.1 |
| `MEDIA_WASM_WINDOW_BYTES` | 67,108,864 (64 MiB) | S | Blob bytes the core may copy into wasm linear memory while preparing an upload. Images above it are refused in the SPA; MP4 neutralisation works through ranged reads on a host handle and never copies the file. | `threat-model.md` INFRA-6 |

The renderer's own caps are in §13.

---

## 7. Timeouts, retries and backoff

**Full jitter.** Where a row says "full jitter", the delay before attempt k (k = 1 for the first retry) is `uniform(0, min(cap, base × 2^(k−1)))`. A server's `Retry-After` wins when it is longer.

| Constant | Value | Class | Applies to | Defined in |
|---|---|---|---|---|
| `CONNECT_TIMEOUT_MS` | 5,000 (5 s) | O | TCP and TLS connection setup | here |
| `REQUEST_TIMEOUT_MS` | 20,000 (20 s) | O | a whole request, including its body, for every non-CAR, non-blob request | here |
| `CAR_TIMEOUT_MS` | 60,000 (60 s) | O | requests whose response is a CAR | here |
| `BLOB_IDLE_TIMEOUT_MS` | 20,000 (20 s) | O | blob downloads have no total timeout (they are user-visible and cancellable); a download aborts after this long without a byte | here |
| `WS_IDLE_TIMEOUT_MS` | 60,000 (60 s) | O | a Jetstream connection that receives no frame for this long is closed and counts as a failure | here |
| `RETRY_MAX` | 3 | O | retries of a GET on a network error, 5xx or 429 | here |
| `RETRY_BASE_MS` | 250 | O | full-jitter base for those retries | here |
| `RETRY_CAP_MS` | 8,000 (8 s) | O | full-jitter cap for those retries | here |
| `BREAKER_FAILURES` | 10 | O | consecutive failures that trip a host's circuit breaker | here |
| `BREAKER_PROBE_MS` | 60,000 (60 s) | O | first probe of a tripped host; doubles after each failed probe | here |
| `BREAKER_PROBE_MAX_MS` | 3,600,000 (1 h) | O | cap of that doubling | here |
| `REDIRECTS_MAX` | 5 | S | redirects an anonymous request follows; each is re-checked by the SSRF guard. `UserPds` requests follow none. Identity requests (DID documents, `/.well-known/atproto-did`) follow at most 3 (`sync.md` §3.1–§3.2). | `threat-model.md` §5.6.2 |
| `JETSTREAM_BACKOFF_MS` | 1,000 (1 s) | O | base of the Jetstream reconnect delay; not full jitter: the n-th consecutive failure waits `min(cap, base · 2^(n−1)) · uniform(0.8, 1.2)` | `sync.md` §6.2 |
| `JETSTREAM_BACKOFF_MAX_MS` | 60,000 (60 s) | O | cap of that delay | `sync.md` §6.2 |
| `JETSTREAM_FAILOVER_AFTER` | 3 | O | consecutive failures before switching to the other endpoint | `sync.md` |
| `WORKER_RESTART_BACKOFF_MS` | 1,000 (1 s) | O | first restart delay of a crashed engine worker; doubles per consecutive crash | `threat-model.md` PILL-5 |
| `WORKER_RESTART_BACKOFF_MAX_MS` | 60,000 (60 s) | O | cap of that doubling | `threat-model.md` PILL-5 |

Write retries are in §14.

---

## 8. Concurrency and request budgets

| Constant | Value | Class | Applies to | Defined in |
|---|---|---|---|---|
| `INFLIGHT_NATIVE` | 32 | O | requests in flight per native process | `sync.md` |
| `INFLIGHT_BROWSER` | 16 | O | requests in flight per browser engine (the sync leader) | `sync.md` |
| `INFLIGHT_PER_HOST` | 6 | O | requests in flight per hostname | `sync.md` |
| `INFLIGHT_PLC` | 8 | O | requests in flight to the PLC directory | `sync.md` |
| `INFLIGHT_DOH` | 8 | O | requests in flight to DoH providers | `sync.md` |
| `INFLIGHT_IDENTITY_PER_DOMAIN` | 6 | S | identity requests (did:web documents, handle `/.well-known/atproto-did`) in flight per registrable domain, approximated as the last two DNS labels of the host (an over-approximation that only ever groups more hosts together) | `threat-model.md` INFRA-7 |
| `PLC_MAX_REQUESTS_PER_S` | 20 | O | request rate to the PLC directory. On 429, wait for `Retry-After` (or full jitter, §7) before the next PLC request. | `sync.md` |
| `RATE_BUDGET_DEFAULT_REQUESTS` | 3,000 | O | per-host read budget per window for `*.bsky.network` and `bsky.social` when no `RateLimit-*` headers are present (ADR 0004). Hosts that send the headers are budgeted from them; other hosts are limited only by concurrency, 429 and `Retry-After`. | ADR 0004 |
| `RATE_BUDGET_DEFAULT_WINDOW_MS` | 300,000 (5 min) | O | window of that budget | ADR 0004 |
| `BUDGET_BACKGROUND_PCT` | 50 | O | background work may use at most this percentage of a host's remaining budget | `sync.md` |
| `BUDGET_P0_PCT` | 80 | O | P0 work (content on screen) may use up to this percentage | `sync.md` |
| `INTERACTIVE_TIME_BUDGET_MS` | 10,000 (10 s) | O | default `--time-budget` of interactive CLI and TUI commands; after it, output is printed with `[partial …]`. `--max-requests` has no default limit. | CLI |
| `SPA_INDEXER_OFFER_REQUESTS` | 1,500 | O | when the SPA's cold-start estimate exceeds this many requests, it offers the forum's indexer | `sync.md` |

---

## 9. Fold-input and behaviour limits

These are N-class: every client applies them identically, and they are part of the named behaviour ID.

| Constant | Value | Class | Rule | Defined in |
|---|---|---|---|---|
| `FETCH_CAP_DEFAULT` | 20,000 | N (all fold IDs) | Per (repo, collection, fk): the fold input is the first N records of the `{fk}-` key range in **descending** rkey byte order, for post, comment, revision, modAction, wikiRevision and sequence. Excess records are excluded, and the repo is `[partial]`. A listing stops at the N-th in-range key, and a range that reaches N keys is reported as capped (`sync.md` §5.4), so a range of exactly N keys is also marked `[partial]`; the fold input is the same either way. | `data-model.md` §7.4 |
| `FETCH_CAP_OPINION` | 100,000 | N (all fold IDs) | The same for `opinion`. | `data-model.md` §7.4 |
| `FUTURE_SKEW_MS` | 300,000 (5 min) | N (`magic/1`, `proofs/1`) | An item whose effective time is more than this after its first-seen time is clamped to first-seen and badged `[future-dated]`; a commit whose `rev` time is more than this after `now` is rejected. | `social.md` §2; `sync.md` |
| `PROOF_DOC_MAX_AGE_MS` | 3,600,000 (1 h) | N (`proofs/1`) | A third-party proof earns `proven` only if the DID document was fetched from PLC or did:web within this time. | `threat-model.md` ID-9 |
| `THREAD_DEPTH_MAX` | 1,000 | N (`mod/1`) | A comment whose depth would exceed this is re-attached to the root and flagged `Orphaned`. | `social.md` §11 |
| `WIKI_WALK_MAX` | 1,000 | N (`karma/1`) | Revisions considered per wiki page; the ones with the greatest TIDs are kept and the page is flagged `[partial]`. | `social.md` §13.2 |
| `DELETED_SUBJECT_MIN_VOTERS` | 2 | N (`karma/1`) | Votes on a confirmed-absent subject count toward karma only from at least this many distinct eligible voters. | `social.md` §5 |
| `RECENT_DISCUSSION_COMMENTS` | 3 | N (`magic/1`) | Newest visible comments shown per post in Recent Discussion. | `social.md` §12.5 |
| `TIMESTAMP_INCONSISTENT_MS` | 3,600,000 (1 h) | O | Display-only flag "inconsistent timestamps" when `|t_claim − t_tid|` exceeds this. | `social.md` §2 |

---

## 10. Sync scheduling and live updates

### 10.1 Listing rules

```
list_range pages:   limit = LIST_RECORDS_LIMIT; on a page over LIST_PAGE_MAX_BYTES retry the same cursor
                    with each limit in LIST_PAGE_RETRY_LIMITS in turn
every page:         keys strictly descending; every key < the request cursor (when one was sent);
                    the next cursor < the previous request cursor.  Violation in range mode → the host is
                    marked range-incapable and the listing is redone as a full-collection listing (sync.md §5.3);
                    a repeated cursor or a cap in full mode → stop, range [partial]; R4 either way
range mode:         stop at the first key that does not start with "{fk}-", at an empty page, at a missing
                    cursor, or at a short page (fewer rows than limit) (ADR 0004)
fetch cap:          stop at the N-th in-range key (N from §9) and report the range capped
bytes:              stop when RANGE_MAX_BYTES (or _OPINION) is exceeded; in full mode FULL_LIST_MAX_BYTES or _PAGES
```

| Constant | Value | Class | Applies to | Defined in |
|---|---|---|---|---|
| `LIST_RECORDS_LIMIT` | 100 | O | `limit` of every `listRecords` call (the PDS maximum) | ADR 0004 |
| `LIST_PAGE_RETRY_LIMITS` | [10, 1] | S | smaller `limit`s tried after an oversized page | here |
| `RANGE_CURSOR_RECHECK_MS` | 604,800,000 (7 days) | O | a host's remembered range-cursor capability is re-probed after this | ADR 0004 |
| `DESCRIBE_REPO_TTL_MS` | 86,400,000 (24 h) | O | a repo's collection list is re-read with `describeRepo` after this | `sync.md` |
| `ROLLBACK_FLAG_AFTER_MS` | 86,400,000 (24 h) | O | a repo whose head stays below the cached rev this long is flagged | `sync.md` |

### 10.2 Dirty queue and polling

| Constant | Value | Class | Applies to | Defined in |
|---|---|---|---|---|
| `DEBOUNCE_P0_MS` | 0–5,000 | O | P0 (content on screen) | `sync.md` |
| `DEBOUNCE_P1_MS` | 2,000–30,000 | O | P1 (posts and comments) | `sync.md` |
| `DEBOUNCE_P2_MS` | 30,000–600,000 | O | P2 (opinions) | `sync.md` |
| `POLL_AUTHORITY_MS` | 120,000 (2 min) | O | authorities of an open forum, in direct mode or with Jetstream down | `sync.md` |
| `POLL_HOT_MS` | 600,000 (10 min) | O | repos changed ≤ `HOT_AGE_MS` ago | `sync.md` |
| `POLL_WARM_MS` | 3,600,000 (1 h) | O | repos changed ≤ `WARM_AGE_MS` ago | `sync.md` |
| `POLL_COLD_MS` | 43,200,000 (12 h) | O | all other repos | `sync.md` |
| `HOT_AGE_MS` | 86,400,000 (1 day) | O | class boundary | `sync.md` |
| `WARM_AGE_MS` | 2,592,000,000 (30 days) | O | class boundary | `sync.md` |
| `POLL_JITTER_PCT` | 20 | O | each interval is multiplied by `uniform(0.8, 1.2)` | `sync.md` |
| `HEAD_CHECK_STALE_MS` | 300,000 (5 min) | O | opening a view triggers a P0 head check for authors on screen whose last check is older | `sync.md` |
| `RECONCILE_PERIOD_MS` | 86,400,000 (24 h) | O | full re-list of every participant, spread evenly over the period | `sync.md` |
| `RECONCILE_AUTHORITY_MS` | 3,600,000 (1 h) | O | full re-list of authorities | `sync.md` |

### 10.3 Jetstream

| Constant | Value | Class | Applies to | Defined in |
|---|---|---|---|---|
| `JETSTREAM_CURSOR_PERSIST_MS` | 5,000 (5 s) | O | the cursor is persisted at least this often… | `sync.md` |
| `JETSTREAM_CURSOR_PERSIST_EVENTS` | 1,000 | O | …or after this many events, whichever comes first | `sync.md` |
| `JETSTREAM_REWIND_MS` | 5,000 (5 s) | O | rewind of a time cursor on reconnect (v1 wire) and on failover between instances (from `witnessedAt`); duplicates are removed through the LRU below | `sync.md`; ADR 0003 |
| `JETSTREAM_DEDUP_LRU` | 10,000 | O | entries in the duplicate-suppression LRU | here |
| `JETSTREAM_GAP_RECONCILE_MS` | 129,600,000 (36 h) | O | a gap longer than this (or an `OutdatedCursor` info message) triggers a full reconcile | `sync.md` |
| `JETSTREAM_V2_QUERY_DIDS_MAX` | 150 | O | DIDs in a v2 `dids=` query (request-line limit measured in ADR 0003) | ADR 0003 |
| `JETSTREAM_WANTED_DIDS_MAX` | 10,000 | O | DIDs in a v1 `options_update{wantedDids}` | `sync.md`; ADR 0003 |
| `JETSTREAM_DID_FILTER_EVENTS_PER_S` | 50 | O | Reserved (deferred): above this lessdugin-wide rate, switching to a DID-filtered connection. v1 never switches automatically. | `sync.md` |
| `PROOF_BATCH_MAX` | 25 | O | uris per `getRecordProofs` call made for Jetstream events | `sync.md` |
| `PROOF_BATCH_WINDOW_MS` | 1,000–3,000 | O | events are collected for a randomised window before a batch is sent | `sync.md` |
| `INDEXER_AUDIT_OMISSION_MS` | 120,000 (2 min) | O | under `--audit`, a Jetstream event missing from `getChanges` after this counts against the indexer | `sync.md` |

The 1 %-per-hour automatic audit and automatic demotion (above 2 % contradictions or 3 omissions per day) are Reserved (deferred).

---

## 11. Discovery and identity

| Constant | Value | Class | Applies to | Defined in |
|---|---|---|---|---|
| `NEWCOMER_BUDGET` | 500 | O | newcomer repos synced per forum open, most recent first | `sync.md` |
| `REJECTED_RECHECK_MS` | 86,400,000 (24 h) | O | a background pass confirms a rejected candidate (no membership) again only this long after its last check, unless a new hint names it | `sync.md` §4.2.1 |
| `REJECTED_RECHECK_BATCH` | 50 | O | rejected candidates a user-initiated `sync_forum` (also `--full`) confirms again before their back-off ends, cheapest first | `sync.md` §4.2.1 |
| `RECENT_POSTS_DISCOVERY` | 100 | O | newest `NS.post` backlinks of the forum read from Constellation to find recently active authors | `sync.md` |
| `RECENT_ACTIVITY_REPLAY_MS` | 129,600,000 (36 h) | O | Jetstream replay window used to find recently active authors | `sync.md` |
| `RELAY_ECOSYSTEM_MAX_REPOS` | 5,000 | O | relay `listReposByCollection(NS.membership)` is used for newcomers only while it lists at most this many repos | `sync.md` |
| `RELAY_PAGE_LIMIT` | 500 | O | `limit` of relay `listReposByCollection` calls | ADR 0003 |
| `CONSTELLATION_PAGE_LIMIT` | 100 | O | `limit` of Constellation `getBacklinks` calls; paging continues while a cursor is returned | ADR 0003 |
| `HANDLE_TTL_MS` | 21,600,000 (6 h) | O | handle → DID cache | `sync.md` |
| `DID_DOC_TTL_MS` | 86,400,000 (24 h) | O | DID document cache | `sync.md` |
| `INACTIVE_RETENTION_MS` | 604,800,000 (7 days) | O | records of an account that is not active are hidden at once and kept this long in the cache | `sync.md` |

---

## 12. Media

| Constant | Value | Class | Applies to | Defined in |
|---|---|---|---|---|
| `IMAGE_UPLOAD_TARGET_BYTES` | 5,242,880 (5 MiB) | O | first downscale target for images (the default self-hosted PDS upload limit). On a 413, the target becomes half the size of the last encoding and the image is re-encoded, at most `IMAGE_UPLOAD_413_RETRIES` times; then the upload is refused. | `sync.md` (media pipeline) |
| `IMAGE_UPLOAD_413_RETRIES` | 3 | O | see above | here |
| `AUTOLOAD_HOST_MIN_PARTICIPANTS` | 5 | O | a PDS host qualifies for media autoload if it serves at least this many participants of the forum | `threat-model.md` PRIV-1 |
| `ASPECT_RATIO_MAX` | 20 | P | `aspectRatio` is clamped to between 1:20 and 20:1 for display | `lfm-1.md` §6.2 |

Related caps: `BLOB_AUTOLOAD_MAX_BYTES`, `BLOB_HARD_MAX_BYTES`, `EXTERNAL_IMAGE_MAX_BYTES` (§5); decode limits and `MEDIA_WASM_WINDOW_BYTES` (§6). Type allowlists are in `data-model.md` §8.8 (upload) and `lfm-1.md` §6.2 (inline display: PNG, JPEG, GIF, WebP, and AVIF in the SPA only).

---

## 13. Rendering, text and presentation

### 13.1 Rendering (`lfm/1` and presentation)

| Constant | Value | Class | Applies to; on hit | Defined in |
|---|---|---|---|---|
| `LFM_SOURCE_MAX_BYTES` | 1,000,000 | N (`lfm/1`) | markdown source. Larger: not parsed, `BodyError::TooLarge` | `lfm-1.md` §1.3 |
| `LFM_EVENT_CAP` | 1,048,576 (2^20) | N (`lfm/1`) | parser events pulled by pass 0. More: truncated, `flags.truncated` and a notice | `lfm-1.md` §3 |
| `LFM_DEPTH_MAX` | 64 | P | open HTML elements around any position (not counting the root). Deeper blocks are flattened. | `lfm-1.md` §8.4 |
| `MATH_EXPR_MAX_BYTES` | 4,096 | N (`lfm/1`) | one TeX expression. Larger: math error | `lfm-1.md` §7.1 |
| `MATH_GROUP_DEPTH_MAX` | 64 | N (`lfm/1`) | TeX group depth (braces, `\left`, `\begin`) | `lfm-1.md` §7.1 |
| `MATH_MAX_EXPANSIONS` | 1,000 | P | math-core `max_expansions` and Temml `maxExpand` | `lfm-1.md` §7.2, §7.5 |
| `MATHML_DEPTH_MAX` | 64 | P | element depth inside `math` after filtering. Deeper: math error | `lfm-1.md` §7.3 |
| `MATHML_MAX_BYTES` | 65,536 | P | serialised `math` element. Larger: math error | `lfm-1.md` §7.3 |
| `SAFE_HREF_MAX_BYTES` | 3,000 | N (`lfm/1`) | input to `safe_href`. Longer: rejected | `lfm-1.md` §5.1 |
| `ANCHOR_MAX_BYTES` | 512 | N (`lfm/1`) | decoded in-document anchor | `lfm-1.md` §5.1 |
| `AUTOLINK_DOMAIN_MAX_CHARS` | 253 | N (`lfm/1`) | domain part of an extended autolink | `lfm-1.md` §4.1 |
| `CID_URL_MAX_CHARS` | 128 | N (`lfm/1`) | the CID of a `cid:` URL (`^b[a-z2-7]{1,127}$`) | `lfm-1.md` §6.1 |
| `RENDERED_LINKS_MAX` | 256 | N (`lfm/1`) | `Rendered.links` (record links, first occurrences) | `lfm-1.md` §12 |
| `RENDERED_MENTIONS_MAX` | 128 | N (`lfm/1`) | `Rendered.mentions` | `lfm-1.md` §12 |
| `CODE_LANG_MAX_CHARS` | 32 | P | `language-*` class token | `lfm-1.md` §8.2 |
| `OSC8_URI_MAX_BYTES` | 2,048 | P | encoded OSC 8 URI. Longer: no OSC 8 (the text is still printed) | `lfm-1.md` §9.4 |
| `HOVER_PREVIEW_DEPTH` | 1 | O | previews never open inside previews | `lfm-1.md` §5.3 |
| `RENDER_CACHE_MAX_BYTES` | 67,108,864 (64 MiB) | O | the in-memory render LRU keyed by `(cid, RENDERER_VERSION)`; SHOULD | here |

### 13.2 Quotes, excerpts, word counts (`lfm/1`)

| Constant | Value | Class | Applies to | Defined in |
|---|---|---|---|---|
| `QUOTE_TARGET_MAX_BYTES` | 200,000 | N (`lfm/1`) | the searched prefix T′ of the target's P | `lfm-1.md` §11 |
| `QUOTE_EXACT_MAX_BYTES` | 2,000 | N (`lfm/1`) | normalised `exact`. Longer: `Orphaned`; writers refuse longer selections | `lfm-1.md` §11 |
| `QUOTE_OCCURRENCES_MAX` | 1,000 | N (`lfm/1`) | occurrences scored, the first ones in ascending order | `lfm-1.md` §11 |
| `QUOTE_CONTEXT_MAX_GRAPHEMES` | 32 | W | `prefix` and `suffix` written | `lfm-1.md` §11 |
| `QUOTE_CONTEXT_MAX_BYTES` | 640 | L | `prefix` and `suffix` (lexicon) | `data-model.md` §4 |
| `EXCERPT_MAX_GRAPHEMES` | 500 | N (`lfm/1`) | excerpt, including the `…` (499 + 1 when cut) | `lfm-1.md` §10.4 |
| `EXCERPT_MAX_BYTES` | 5,000 | N (`lfm/1`) | excerpt, including the `…` (4,997 + 3 when cut) | `lfm-1.md` §10.4 |
| `READ_WORDS_PER_MINUTE` | 250 | N (`lfm/1`) | `read_minutes` | `lfm-1.md` §10.5 |

### 13.3 Names and presentation

`term_sanitize(Name{max})` (`lfm-1.md` §9.1) uses these grapheme caps:

| Constant | Value | Class | Applies to |
|---|---|---|---|
| `NAME_MAX_DISPLAY_NAME` | 64 | O | display names (`profile.displayName`, `app.bsky.actor.profile` fallback) |
| `NAME_MAX_FORUM` | 100 | O | forum names |
| `NAME_MAX_TITLE` | 300 | O | post, sequence and chapter titles |
| `NAME_MAX_TAG` | 64 | O | tag names, react labels |
| `NAME_MAX_MEDIA` | 64 | O | `#media.name` |
| – | none | O | handles are never grapheme-capped: they are ASCII, ≤ 253 characters by syntax, verified, and truncating them could hide the distinguishing tail |

| Constant | Value | Class | Applies to | Defined in |
|---|---|---|---|---|
| `SHORT_DID_PLC_CHARS` | 8 | P | identifier characters shown in a short did:plc | `lfm-1.md` §5.4 |
| `SHORT_DID_WEB_MAX_CHARS` | 40 | P | a longer did:web is cut to 39 characters + `…` | `lfm-1.md` §5.4 |
| `MENTION_EAGER_MAX` | 16 | O | distinct mention DIDs per body resolved eagerly (participants only); the rest resolve on hover, focus or click | `lfm-1.md` §5.4 |
| `THREAD_AUTHOR_EXPANDED_MAX` | 50 | O | comments per author per thread expanded by default, in `(t_rank, uri)` order; the rest collapse behind "show N more by @x" | `social.md` §10.3 |
| `THREAD_INDENT_MAX` | 6 | O | indentation levels in the TUI and on mobile, then "continue thread →" | `social.md` §11 |
| `EDITOR_TRAILING_LINES_MIN` | 6 | O | fixed lines after quoted text in editor templates | `threat-model.md` TERM-8 |
| `EDITOR_PREVIEW_DEBOUNCE_MS` | 150 | O | SPA editor live-preview debounce | here |

---

## 14. Write path

| Constant | Value | Class | Applies to | Defined in |
|---|---|---|---|---|
| `OUTBOX_INFLIGHT_PER_ACCOUNT` | 1 | W | writes in flight per account; the swap head is always a fresh `getLatestCommit` | `data-model.md` §8.2 |
| `OPINION_HOLD_MS` | 4,000 (4 s) | O | opinion intents are held this long so they can be undone or coalesced | `data-model.md` §8.2 |
| `SWAP_RETRY_MAX` | 3 | W | attempts of an edit, opinion write, modAction removal or `mod adopt` batch on `InvalidSwap` or a swap failure | `data-model.md` §8.3, §8.5, §8.6 |
| `SWAP_RETRY_JITTER_MS` | 200–800 | W | delay before each such retry | `data-model.md` §8.3 |
| `APPLYWRITES_MAX_OPS` | 200 | W | ops per `applyWrites` (delete chunks, redaction, `mod adopt`) | `data-model.md` §8.6, §8.7 |
| `WRITE_BUDGET_POINTS_PER_HOUR` | 5,000 | O | per account; a write that would exceed it waits as `throttled` | `data-model.md` §8.2 |
| `WRITE_BUDGET_POINTS_PER_DAY` | 35,000 | O | per account | `data-model.md` §8.2 |
| `WRITE_COST_CREATE` | 3 | O | points per create op | `data-model.md` §8.2 |
| `WRITE_COST_UPDATE` | 2 | O | points per update op | `data-model.md` §8.2 |
| `WRITE_COST_DELETE` | 1 | O | points per delete op | `data-model.md` §8.2 |

Blobs are uploaded in the `sending` step of the `PendingWrite` that references them, immediately before the record write, because PDSes garbage-collect unreferenced blobs after about an hour (`data-model.md` §8.8). Body size and spilling: §4.

---

## 15. Store and cache

| Constant | Value | Class | Applies to | Defined in |
|---|---|---|---|---|
| `CACHE_EVICT_NATIVE_BYTES` | 2,147,483,648 (2 GiB) | O | above this, whole forums are evicted, least recently viewed first. Never evicted: identities, the user's own repos, and authority repos of followed forums. | `sync.md` |
| `CACHE_EVICT_BROWSER_BYTES` | 524,288,000 (500 MiB) | O | the same in the browser | `sync.md` |
| `TOMBSTONE_MIN_RETENTION_MS` | 2,592,000,000 (30 days) | O | tombstones are kept at least this long (`sync.md` I4) | `sync.md` |

`INACTIVE_RETENTION_MS` is in §11, `RENDER_CACHE_MAX_BYTES` in §13.1, `JETSTREAM_DEDUP_LRU` in §10.3.

**Engine runtime** (`engine-api.md`; all *tunable*):

| Constant | Value | Class | Applies to | Defined in |
|---|---|---|---|---|
| `RENDER_CACHE_ENTRIES` | 512 | O | rendered bodies kept in memory (with `RENDER_CACHE_MAX_BYTES`) | `sync.md` §13 |
| `LIST_PAGE_DEFAULT` | 30 | O | `list_posts` page size when the query gives none | `engine-api.md` §3.4 |
| `LIST_PAGE_MAX` | 500 | O | largest `list_posts` page; a larger `limit` is clamped | `engine-api.md` §3.4 |
| `MODEL_MAX_AGE_MS` | 60,000 (1 min) | O | a cached forum model older than this is rebuilt on the next query (the fold's time-dependent states follow the clock); orderings always use the query time | here |
| `MODEL_CACHE_ENTRIES` | 16 | O | forum models kept in memory, least recently built evicted first | here |
| `FOLLOW_SEED_PAGES` | 10 | O | `listRecords` pages of own `membership` records read when seeding the followed list | `sync.md` §4.8 |
| `FORUM_REOPEN_MS` | 300,000 (5 min) | O | `forum_open` within this time of the last open reuses its result (no declaration refetch) | `engine-api.md` §3.2 |
| `FORUM_OPEN_SYNC_BUDGET_MS` | 10,000 (10 s) | O | time budget of the P0 sync `forum_open` runs for authorities with no synced range | `engine-api.md` §3.2 |
| `LIVE_TAIL_IDLE_MS` | 60,000 (1 min) | O | longest sleep of `live_tail` between rounds | `engine-api.md` §3.3 |
| `LIVE_TAIL_BATCH` | 16 | O | dirty-queue items served per `live_tail` round | `sync.md` §6.1 |
| `HANDLE_VERIFY_BATCH` | 32 | O | author handles verified per `live_tail` round | `sync.md` §3.3 |
| `HANDLE_VERIFY_QUEUE_MAX` | 1,000 | O | authors waiting for handle verification; later ones wait for the next view | `sync.md` §3.3 |
| `TOMBSTONE_GC_EVERY_MS` | 86,400,000 (1 day) | O | how often `live_tail` drops tombstones past `TOMBSTONE_MIN_RETENTION_MS` | `sync.md` I4 |
| `DISCUSSION_COMMENTS` | 3 | O | latest Visible comments shown per post by `recent_discussion` | `engine-api.md` §3.4 |

---

## 16. Authentication and sessions

| Constant | Value | Class | Applies to | Defined in |
|---|---|---|---|---|
| – | ≈ 14 days | — | session lifetime of a public OAuth client, set by the server; expiry comes from `expires_in` (informative) | `sync.md` |
| `SESSION_EXPIRY_WARNING_MS` | 259,200,000 (3 days) | O | users are warned this long before a session expires | `threat-model.md` AUTH-10 |
| `OAUTH_LOGIN_TTL_MS` | 600,000 (10 min) | O | a pending loopback or bridge login (state, PKCE verifier, listener) expires after this | `threat-model.md` §7.4 |

---

## 17. SPA and build

| Constant | Value | Class | Applies to | Defined in |
|---|---|---|---|---|
| `WASM_GZIP_TARGET_BYTES` | 900,000 | O | target size of the eagerly loaded engine `.wasm`, compressed with gzip level 9 | `xtask wasm --budget` |
| `WASM_GZIP_FAIL_BYTES` | 1,100,000 | O | CI fails above this. Lazily loaded modules (for example `verify.wasm`) are reported, not gated. | `xtask wasm --budget` |
| `PNPM_MIN_RELEASE_AGE_MINUTES` | 10,080 (7 days) | O | pnpm `minimumReleaseAge` | `threat-model.md` SUP-6 |

---

## 18. `lessdugin web` bridge

| Constant | Value | Class | Applies to | Defined in |
|---|---|---|---|---|
| `WEB_PORT_DEFAULT` | 47778 | O | default listening port on `127.0.0.1` | `threat-model.md` WEB-2 |
| `BRIDGE_CAPABILITY_BYTES` | 32 (256 bits) | S | random bytes per capability, from the OS CSPRNG | `threat-model.md` §7.2 |
| `BRIDGE_TOKEN_BYTES` | 32 (256 bits) | S | random bytes per session token | `threat-model.md` §7.2 |
| `BRIDGE_JSON_MAX_BODY_BYTES` | 1,048,576 (1 MiB) | S | JSON bodies of `/bridge/session`, `/bridge/login` and `/bridge/xrpc` (a conforming write body is ≤ 900,000 bytes). Larger: 413 | `threat-model.md` §7.3 |
| `BRIDGE_KV_MAX_BODY_BYTES` | 67,108,864 (64 MiB) | S | one `/bridge/kv` request (binary values are base64). Larger: 413 | `threat-model.md` §7.3 |
| – | `BLOB_HARD_MAX_BYTES` | S | `/bridge/xrpc/com.atproto.repo.uploadBlob` bodies | `threat-model.md` §7.4 |
| `BRIDGE_CAPABILITY_TTL_MS` | 300,000 (5 min) | S | an unused capability (a printed or opened link) expires this long after it was minted | `threat-model.md` §7.2 |
| `BRIDGE_LOGIN_CONFIRM_MS` | 300,000 (5 min) | S | a browser-started sign-in waits this long for `y` in the `web` terminal, then is refused | `threat-model.md` §7.4 |
| `BRIDGE_LOCK_WAIT_MS` | 20,000 (20 s) | O | the longest one `/bridge/lock` request waits for a busy lock before answering `{"held": false}` | `threat-model.md` §7.4 |
| `BRIDGE_LOCK_HEARTBEAT_MS` | 1,000 (1 s) | O | a held `/bridge/lock` body sends a newline this often, so a closed connection releases the lock within about two beats | `threat-model.md` §7.4 |
| `WEB_SHUTDOWN_GRACE_MS` | 3,000 (3 s) | O | after Ctrl-C, open connections get this long before `lessdugin web` stops anyway (a second Ctrl-C stops it at once) | – |

`OAUTH_LOGIN_TTL_MS` (§16) bounds the pending bridge login.

---

## 19. Indexer

### 19.1 Server

| Constant | Value | Class | Applies to | Defined in |
|---|---|---|---|---|
| `INDEXER_INGEST_HOST_RPS` | 5 | O | requests per second to one PDS host | `sync.md` |
| `INDEXER_INGEST_HOST_INFLIGHT` | 4 | O | requests in flight to one PDS host | `sync.md` |
| `INDEXER_INGEST_INFLIGHT` | 200 | O | requests in flight in total | `sync.md` |
| `INDEXER_BACKFILL_PERIOD_MS` | 21,600,000 (6 h) | O | `listReposByCollection` backfill sweep | `sync.md` |
| `INDEXER_JETSTREAM_REWIND_MS` | 10,000 (10 s) | O | cursor rewind on reconnect | `sync.md` |
| `INDEXER_CHANGE_LOG_RETENTION_MS` | 2,592,000,000 (30 days) | O | `change_log`; older `getChanges` cursors get `CursorTooOld` | `data-model.md` §11.3 |
| `INDEXER_SERVE_RPS_PER_IP` | 20 | O | requests per second per client IP; above it, 429 `RateLimitExceeded` | `data-model.md` §11.1 |
| `INDEXER_LIST_MAX_AGE_S` | 5 | O | `Cache-Control: max-age` of list responses; nothing is `immutable` | `data-model.md` §11.1 |
| `INDEXER_FTS_TERMS_MAX` | 16 | S | query terms in `searchPosts` (when it ships); every term is quoted | `threat-model.md` INFRA-12 |
| `INDEXER_FTS_PREFIX_MIN_CHARS` | 3 | S | minimum length of a prefix term | `threat-model.md` INFRA-12 |

Output limits of indexer methods (page sizes, `getThread` ≤ 10,000 comments, `describeIndexer` ≤ 1,000 forums, snippets ≤ 1,000 bytes, ≤ 16 highlights) are lexicon limits in `data-model.md` §11.3.

### 19.2 Sizing guidance (informative)

| Forum size | Guidance |
|---|---|
| ≤ 300 participants | serverless everywhere |
| 300–3,000 participants | native clients are fine; the forum should declare an indexer for SPA first visits, global search and offline notifications |
| > 3,000 participants, > 10,000 DIDs, > 300 concurrent readers or > 1,000,000 records | the forum should declare at least one indexer (preferably two); the SPA defaults to the indexer for cold starts; direct mode stays available for audit |
| indexer capacity | about 50,000,000 records on SQLite |

---

## 20. Reserved limits

Fixed now so that later phases do not reopen them; no v1 code path uses them.

| Name | Value | Status | Applies to |
|---|---|---|---|
| `REPORT_CIPHERTEXT_MAX_BYTES` | 16,384 (16 KiB) | Reserved (P2) | sealed `report.ciphertext` |
| `REPORT_DELAY_MAX_MS` | 21,600,000 (6 h) | Reserved (P2) | random publication delay of sealed reports, from 0 |
| `BANLIST_SUBJECTS_MAX` | 2,000 | Reserved (P2) | `#banList.subjects` |
| `CUSTOM_REACT_ICONS_MAX` | 256 | Reserved (P2) | custom react icons, loaded lazily |
| `GET_BLOCKS_BATCH_MAX` | 50 | Reserved (P7) | CIDs per `getBlocks` call in verified range sync |
| — | — | Reserved (P2) | `ratelimit/1` advisory rate limits |
| `JETSTREAM_DID_FILTER_EVENTS_PER_S` | 50 | Reserved (deferred) | automatic switch to a DID-filtered connection (§10.3) |
| — | 1 % per hour; > 2 % contradictions; > 3 omissions per day | Reserved (deferred) | automatic indexer audit and demotion |

---

## 21. Rust mirror and vectors

1. **One module.** `crates/core/src/limits.rs` (wasm-safe) defines one `pub const` for every row of this document that names a constant, except §19 (which `crates/indexer` defines) and §20 (not defined until its phase). Each constant's value MUST equal the row's value, and its doc comment cites the section. Integer types are the implementer's choice; byte counts that can exceed 2^32 use `u64`.
2. **Ranges.** A row whose value is a range (`a–b`) or a list becomes a pair of constants with suffixes `_MIN` and `_MAX`, or a `const` slice.
3. **Lexicon limits** are enforced by the validator from the embedded lexicon JSON. The few in §4 are repeated in `limits.rs` only because other code uses them; a unit test asserts that they equal the embedded lexicon values.
4. **N-class constants are scoped to their behaviour ID.** A new behaviour ID adds new constants; the old ones stay, so older IDs remain implementable from their vectors.
5. **Vectors** (`testdata/vectors/limits/`, CC0-1.0, format `{name, input, expected}` as in synthesis §7):
   - `constants.json` lists every mirrored constant as `{name, value}`. The Rust runner (testkit) and the TS runner (wasm `runVector`) assert that the compiled constants equal it.
   - Boundary vectors cover every N-class row and every S-class row that a pure function can exercise, at the limit and one past it: fetch caps (20,000 and 20,001 in-range keys; 100,000 and 100,001 opinions), `RECORD_CBOR_MAX_BYTES`, proof CAR bytes and blocks, `MST_DEPTH_MAX`, both nesting limits (32/33 and 36/37), `FUTURE_SKEW_MS`, `THREAD_DEPTH_MAX`, `WIKI_WALK_MAX`, `LIST_PAGE_RETRY_LIMITS`, the frontmatter limits, and the §13 rendering and quote limits (which the `lfm1` vectors also cover).
   - Network caps are tested in the testkit against FakePds fault modes (oversized, compressed-bomb and slow responses; cursor regressions), not as pure vectors.

---

## 22. Decisions made while writing

These fill gaps in the sources or resolve ambiguities. Each takes the simplest safe option, and all are open to review before M7.

1. **Units.** The synthesis wrote "MB" loosely. Each row now states exact bytes. The values already in the `limits.rs` stub are kept: decimal for record, proof CAR and Jetstream caps (1,000,000 and 1,200,000 bytes), binary for page, bundle and blob caps (4 MiB, 8 MiB, 5 MiB, 300 MiB). New byte limits are binary unless they mirror a decimal lexicon or protocol value.
2. **Classes** (§2) are introduced, so that each limit has an explicit change rule. The fetch caps, proof caps and `lfm/1` limits are N-class; resource caps are S-class and only ever produce `[partial]` or "unavailable".
3. **Per-range byte caps** are per (repo, collection, fk) rather than per (repo, forum), following the completeness review's move of the record caps to per-collection fetch caps. `opinion` gets 100 MiB, in proportion to its larger record cap. Revision ranges of very long, often edited posts can approach 50 MiB; R6 applies.
4. **`RECORD_JSON_MAX_BYTES`** (1,000,000) is read as the cap on a `getRecord` response, distinct from validity rule G6 (`RECORD_CBOR_MAX_BYTES`). A conforming writer's request body is ≤ 900,000 bytes, so its records always fit.
5. **Oversized pages and bundles** are retried with smaller requests (`limit` 10 then 1; `uris` halved), so that one large honest record does not make a whole range or bundle unreadable.
6. **New safety caps the sources implied but did not size:** `IDENTITY_RESPONSE_MAX_BYTES` (64 KiB), `INDEXER_RESPONSE_MAX_BYTES` (8 MiB, as for bundles), `INDEXER_BUNDLE_MAX_BLOCKS` (25 × 2,000), `NESTING_MAX_DOCUMENT` (36, so that envelope depth never decides whether a record is readable), `MEDIA_CONTAINER_MAX_DEPTH` (32), `MEDIA_WASM_WINDOW_BYTES` (64 MiB, applying the streaming-blob fix to uploads), `REDIRECTS_MAX` (5), the bridge body caps, and `BLOB_SNIFF_PREFIX_BYTES` (4,096).
7. **Image decode limits.** "64 MB" (F36) is read as encoded input, and an allocation cap of 256 MiB (one 8,192² RGBA frame) is added, because 8,192² RGBA does not fit in 64 MB. The decode timeout is 10 s. The same limits apply to the native upload pipeline.
8. **Timeouts the sources left open:** blob downloads use an idle timeout of 20 s and no total timeout; Jetstream connections are recycled after 60 s without a frame; pending logins expire after 10 minutes.
9. **Backoff shapes.** "With jitter" is read as full jitter everywhere except Jetstream reconnects, whose shape (exponential with ±20 % jitter) `sync.md` §6.2 defines. The worker restart backoff reuses the Jetstream numbers (1 s doubling to 60 s).
10. **Per-domain identity budget.** F16's "per registrable domain" budget is approximated by the last two DNS labels, which never under-groups, so no public-suffix list is needed. It applies only to identity requests, so Bluesky's many PDS hosts still get per-host budgets (ADR 0004).
11. **Rate budgets are per host** (ADR 0004), with 3,000 requests per 5 minutes as the default for Bluesky hosts without headers.
12. **Image 413 handling.** "Fit the image when the PDS returns 413" becomes: halve the target, re-encode, at most 3 times, then refuse.
13. **Name caps** follow each field's lexicon grapheme limit; handles are never capped.
14. **Unspecified cache sizes:** the render LRU is 64 MiB (SHOULD) and the Jetstream de-duplication LRU holds 10,000 entries.
15. **Wasm budget** is measured on the eagerly loaded module with gzip level 9; lazily loaded modules are reported, not gated.
16. **`RECENT_DISCUSSION_COMMENTS`** is N-class under `magic/1`, because `social.md` §0.3 puts Recent Discussion under that ID.
17. **Consistency review.** These rows were aligned with `sync.md`, which defines the procedures: the full-collection fallback has its own caps (`FULL_LIST_MAX_BYTES`, `FULL_LIST_MAX_PAGES`), taken from `sync.md` §5.3 and stated in MiB; a listing violation in range mode falls back to a full listing rather than ending as `[partial]`; a range that reaches N keys is reported capped (no (N + 1)-th key is fetched); `/.well-known/atproto-did` keeps `sync.md`'s 2,048-byte cap (`HANDLE_WELLKNOWN_MAX_BYTES`); identity requests follow at most 3 redirects within `REDIRECTS_MAX`; the sniff prefix is read with `blob_read`, because `sync.md` §12.2's `BlobHandle` is `{cid, size}`. In the other direction, `sync.md` now uses this document's values for the per-range byte caps (100 MiB for `opinion`), the bundle caps (8 MiB, 50,000 blocks), the retry formula and the blob timeouts (no total timeout). `NESTING_MAX_RECORD` became class N, as validity rule G10.

**Open questions**
- **PDS request-body limits.** `WRITE_BODY_MAX_BYTES` (900,000) assumes the PDS accepts JSON bodies of about 1 MB. The reference PDS may enforce a much lower JSON body limit; spike S12 (or M2) should measure the largest accepted `applyWrites` body on bsky.social and the devnet. If it is lower, the writer should spill at that size, and `data-model.md` §5.3 and §8.9 need the measured value.
- *(Resolved in the consistency review: nesting depth is now validity rule G10, `data-model.md` §7.2; a too-deep but decodable record is an invalid version that replaces older ones, and an undecodable response changes nothing.)*
- **`crates/core/src/limits.rs`** currently mirrors only part of this table. Its existing values agree with this document; the remaining constants are to be added in M1.
