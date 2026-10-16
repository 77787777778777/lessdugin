<!-- SPDX-License-Identifier: CC0-1.0 -->
# ADR 0005 — S12 and S4 (writes): write semantics, `validate`-unset records, nested blob refs

- Status: Accepted
- Date: 2026-10-16
- Spikes: S12 (`createRecord`-with-rkey create-if-absent; `putRecord swapRecord`; `applyWrites
  swapCommit`) and the write half of S4 (`validate`-unset records containing unknown `$type`s and
  union-like values, nested blob refs, and blobs referenced only by a revision surviving GC)

## Context

The outbox (synthesis §3.5) depends on specific PDS behaviour:

- **Opinions and mod actions** use "`createRecord(rkey)` fails if it exists", `putRecord(swapRecord
  = cid)` and `deleteRecord(swapRecord)`.
- **Edits** use `applyWrites[create revision, update target]` with `swapCommit = head` and retry on
  `InvalidSwap`.
- **First content in a forum** uses `applyWrites[create membership, create post]` and expects a
  recognisable failure when the membership already exists.
- **Record shape (§2.2).** Writers never set `validate` and always include `$type`.
- **Revisions** snapshot the old record in a field of type `unknown` (`revision.record`). If the old
  record had media, the snapshot holds blob refs nested two levels deep. Those blobs must stay
  alive at the PDS for history to be viewable, and must go away when history is redacted.

## Method

- **Setup.** Local devnet only: `node packages/e2e/devnet.mjs --json devnet.json`, which starts
  `@atproto/dev-env` 0.6.20 with `@atproto/pds` 0.5.39 and runs on `http://localhost:<port>`.
  Nothing was written to any production PDS.
- **Server logs.** The devnet was restarted with `LOG_ENABLED=true LOG_LEVEL=warn` so the server-side
  causes of 500s show up in its log.
- **Requests.** Plain `fetch` XRPC calls, authenticated with
  `com.atproto.server.createSession{identifier: "<name>.test", password: <app password>}`.
- **Collections.** `online.7778777.lessdugin.temp.{opinion,post,membership,revision}`. These are
  unknown to the PDS.
- **The four probes:**
  - **(a)** `createRecord` with an explicit rkey, twice.
  - **(b)** `putRecord` with `swapRecord` = the current CID, a stale CID, `null`, and a CID on an
    absent key, plus `putRecord` of an identical value. Also `deleteRecord` with
    `swapRecord`.
  - **(c)** `getLatestCommit`, then `applyWrites` with `swapCommit` = that CID, then again with the
    now-stale CID. Also single-op `applyWrites` for create-existing, update-absent and
    delete-absent.
  - **(d)** Five 1×1 PNGs (69 B each, distinct pixels) uploaded with `uploadBlob`
    (`content-type: image/png`) and used as follows:

    | Blob | Use |
    |---|---|
    | A | in a post at `record.body.media[0].blob` |
    | B | only in a revision at `record.record.body.media[0].blob` |
    | C | never referenced |
    | D | referenced with a wrong `mimeType` and a wrong `size` |
    | E | in a post; then `applyWrites[create revision(snapshot with E), update post without E]` |

    After each step, check `com.atproto.sync.listBlobs?did=`,
    `com.atproto.repo.listMissingBlobs` and `com.atproto.sync.getBlob`. Then delete the post holding
    A and the revision holding B, and check again.

  The post in (d) also contained:

  ```json
  "body": {"$type":"online.7778777.lessdugin.temp.markdown","text":"…","media":[{"blob":<A>,"alt":"red"}]},
  "embed": {"$type":"com.example.unknown.embed#thing","nested":{"$type":"com.example.other","deep":[1,2,{"$type":"x.y.z"}]}},
  "unionish": [{"$type":"online.7778777.lessdugin.temp.defs#link","uri":"https://example.com"},
               {"$type":"online.7778777.lessdugin.temp.defs#textQuote","exact":"q"}],
  "knownTypeButInvalid": {"$type":"app.bsky.feed.post","text":12345}
  ```
- **(e)** The `validate` matrix: unset / `false` / `true`, against an unknown collection
  (`…temp.post`) and a known one (`app.bsky.feed.like`, valid and invalid).

## Results

### (a) `createRecord` with an rkey that already exists

| Step | Response |
|---|---|
| create `aaaaaaaaaaaa-3mxygf7onkd2b-aaaaaaaaaaaaaaaa` | 200 `{uri, cid:"bafyreienhjeo…", commit:{cid, rev:"3mxygibnwnk2f"}, validationStatus:"unknown"}` |
| create the same rkey again (×2) | **500** `{"error":"InternalServerError","message":"Internal Server Error"}`. The server log says `"err":{"message":"There is already a value at key: online.7778777.lessdugin.temp.opinion/aaaaaaaaaaaa-…"}` |
| `getRecord` afterwards | unchanged (`v: 1`, same CID) |

### (b) `putRecord` and `deleteRecord` with `swapRecord`

| Step | Response |
|---|---|
| put, `swapRecord` = current CID | 200, new CID and `commit` |
| put, `swapRecord` = stale CID | **400** `{"error":"InvalidSwap","message":"Record was at bafyreih457moolfer7blpw6x3c6vyzlvmegvd4ei5ir2a2eqvfmsf5osky"}` (the current CID) |
| put, `swapRecord: null`, key **exists** | **400** `{"error":"InvalidSwap","message":"Record was at bafyreih457…"}` |
| put, `swapRecord: null`, key **absent** | **200**, record created |
| put, `swapRecord` = some CID, key absent | **400** `{"error":"InvalidSwap","message":"Record was at null"}` |
| put an **identical** value (no swap, or swap = current) | 200 `{uri, cid (unchanged), validationStatus:"unknown"}`. **No `commit` field**, and `getLatestCommit` is unchanged before and after (`rev 3mxygibnwnk2f`): a no-op |
| delete, existing key, wrong `swapRecord` | **400** `InvalidSwap` "Record was at bafyreienhjeo…" |
| delete, existing key, correct `swapRecord` | 200 `{"commit":{"cid":…,"rev":"3mxygibqnks2f"}}` |
| delete, **absent** key, no swap | 200 `{}` (no commit) |
| delete, **absent** key, `swapRecord` = old CID | 200 `{}`. The swap is not checked when the record is absent |

### (c) `applyWrites` and `swapCommit`

| Step | Response |
|---|---|
| `swapCommit` = head; `[create membership aaaaaaaaaaaa, create post aaaaaaaaaaaa-<tid>]` | 200 `{"commit":{"cid","rev"},"results":[{"$type":"com.atproto.repo.applyWrites#createResult","uri","cid","validationStatus":"unknown"},…]}` |
| the same `swapCommit` again (now stale) | **400** `{"error":"InvalidSwap","message":"Commit was at bafyreiboxhfw5y3b4zectslpqbcgqe56fs44vbjaahqqxzbxgiuqpo5umu"}` (the current head) |
| `putRecord` with stale `swapCommit` | **400** `InvalidSwap` "Commit was at …" |
| `[create existing membership, create new post]` | **500** `InternalServerError`. Log: "There is already a value at key …". **Atomic**: the new post was not written |
| `[create]` on an existing key | 500 (same log) |
| `[update]` on an **absent** key | **500**. Log: `Could not find a record with key: online.7778777.lessdugin.temp.opinion/zzzzzzzzzzzz-absent` |
| `[delete]` on an **absent** key | **500** (same log) |
| `[update]` with an identical value | 200 with a **new commit** (`rev 3mxygibspxs2f`), unlike `putRecord` |
| `[update, delete]` of the same key in one batch | 200; results `#updateResult` (with cid) then `#deleteResult` (empty) |

**Other rejections**

| Write | Response |
|---|---|
| record `$type` ≠ collection | 400 `{"error":"InvalidRequest","message":"Invalid $type: expected online.7778777.lessdugin.temp.post, got online.7778777.lessdugin.temp.comment"}` |
| record **without** `$type` | 200. `getRecord` shows that the PDS **injected** `"$type":"online.7778777.lessdugin.temp.post"` |
| a float | 400 `"Expected one of null, boolean, integer, string, cid, bytes, array or object value type (got 1.5) at $.record.score"` |
| rkey `bad/rkey` | 400 `"Invalid record key (got \"bad/rkey\") at $.rkey"` |

### (d) Blobs

**`uploadBlob` response:**
`{"blob":{"$type":"blob","ref":{"$link":"bafkreifr76oi5i5hqc5nbgzunrbd2lioi2avsjuhtmmoqqozfa3wvfdgia"},"mimeType":"image/png","size":69}}`.

The PDS **sniffs** the type, and its answer can differ from what was declared:

| Bytes, declared as | Stored `mimeType` |
|---|---|
| PNG, `image/jpeg` | `image/png` |
| PNG, `application/octet-stream` | `image/png` |
| UTF-8 text, `text/plain; charset=utf-8` | `text/plain` (parameters stripped) |
| UTF-8 text, `text/markdown` | `text/markdown` |
| SVG, `image/svg+xml` | `image/svg+xml` |

| Step | `sync.listBlobs` | `getBlob` |
|---|---|---|
| after 5 uploads, before any record | `[]` | C: **400** `{"error":"InvalidRequest","message":"Blob not found"}` (temporary blobs are not served) |
| post with A at `body.media[0].blob`; revision with B **only** at `record.body.media[0].blob` | `[A, B]` | A 200 `image/png` 69 B; B 200; C 400 "Blob not found" |
| `listMissingBlobs` | – | `{"blobs":[]}` |
| post referencing D with `mimeType: "image/jpeg"` | – | **400** `{"error":"InvalidMimeType","message":"Referenced Mimetype does not match stored blob. Expected: image/png, Got: image/jpeg"}` |
| post referencing D with `size: 999` | – | **400** `{"error":"InvalidSize","message":"Referenced Size does not match stored blob. Expected: 69, Got: 999"}` |
| post referencing a CID that was never uploaded | – | **400** `{"error":"BlobNotFound","message":"Could not find blob: bafkreihdwdcef…"}` |
| `applyWrites[create revision(record.body.media[0].blob = E), update post (media: [])]` | `[A, B, E]` | E 200: **kept alive by the revision alone** |
| delete the post holding A | `[B, E]` | A **400 "Blob not found"** (collected immediately) |
| delete the revision holding B | `[E]` | B 400 "Blob not found" |

The post with unknown nested `$type`s, union-like arrays and an invalid object labelled
`$type: app.bsky.feed.post` was accepted (`validationStatus: "unknown"`). The PDS does not validate
nested objects in an unknown collection, but it does find blob refs at any depth, including inside
`unknown`-typed fields.

### (e) `validationStatus`

| `validate` | Collection | Result |
|---|---|---|
| unset | unknown (`…temp.post`) | 200, `"validationStatus":"unknown"` |
| `false` | unknown | 200, **no** `validationStatus` field |
| `true` | unknown | **400** `{"error":"InvalidRequest","message":"Unknown lexicon type: online.7778777.lessdugin.temp.post"}` |
| unset | known, valid `app.bsky.feed.like` | 200, `"valid"` |
| `false` | known, valid | 200, no field |
| `true` | known, valid | 200, `"valid"` |
| unset | known, **invalid** like | **400** `"Invalid app.bsky.feed.like record: Expected object value type (got \"not-a-strongref\") at $.record.subject"` |
| `false` | known, invalid like | 200 (written unvalidated) |

## Decision

1. **Create-if-absent uses `putRecord{rkey, swapRecord: null}`, not `createRecord`.**
   - On the reference PDS, `createRecord` with an existing rkey fails atomically but with a generic
     **500**, which cannot be told apart from a transient server error.
   - `putRecord` with `swapRecord: null` gives a typed `400 InvalidSwap` whose message carries the
     current CID.
   - **New Opinion and Mod-action rows in §3.5:** no record → `putRecord(rkey, swapRecord: null)`;
     record exists → `putRecord(swapRecord = cid)`; result empty → `deleteRecord(swapRecord = cid)`.
     On `InvalidSwap`, re-read and re-apply the intent (≤ 3 times, with 200–800 ms jitter).
   - **Compatibility:** the lexicon flags nullable `swapRecord` as possibly problematic for Go
     implementations. Treat a 400 other than `InvalidSwap` on a null swap as "unsupported"; for that
     host, fall back to `createRecord(rkey)`, and on a 5xx call `getRecord` to tell "exists" from
     "failed".
2. **`InvalidSwap` is the only conflict signal.**
   - Its message names the current CID or commit (`Record was at <cid|null>`, `Commit was at <cid>`).
     It may be parsed as a hint to skip a re-read, but must not be relied on: the format is not
     specified.
   - Edits keep `applyWrites` with `swapCommit = head` as designed. A stale head gives
     `400 InvalidSwap "Commit was at …"`.
3. **`applyWrites` is strict, atomic, and fails with 500 on missing or present keys.** Create on an
   existing key, and update or delete on an absent key, each fail the whole batch with a 500 and
   write nothing. Therefore:
   - **First content in a forum (replaces the §3.5 row):** call `getRecord(NS.membership, fk)`
     first, which is usually cached from sync. Include the membership create only when it is absent,
     and use `swapCommit = head`. On a 500, re-read the membership and head, then retry once without
     the membership if it now exists. Never treat a 500 as "membership exists" without re-reading.
   - **Delete:** build `[delete target, delete its revisions]` only from a fresh listing of the own
     revision range. On a 500, re-list and retry once. Single deletes use `deleteRecord`, which is
     idempotent on absent keys (`200 {}`, swap ignored).
   - **`mod adopt` and other batched creates:** decide between create and update from a freshly
     listed range (already designed). A 500 means "re-list".
4. **Handle no-op results.**
   - `putRecord` of an identical value returns no `commit` and does not advance the head. The outbox
     must accept a response without `commit` (§3.3: "`getLatestCommit` if commit is absent") and
     must not wait for a Jetstream event for it.
   - `applyWrites` update with an identical value does create a commit. Writers skip no-op updates
     on the client: compare the CBOR CID before sending.
5. **Validation flag (§2.2 is unchanged).** Leave `validate` unset. Unknown NSIDs then return
   `validationStatus: "unknown"`, which the outbox records but ignores.
   - Never send `validate: true`: with `.temp` and before publication it fails with "Unknown lexicon
     type".
   - Never send `validate: false`: it turns validation off entirely (an invalid
     `app.bsky.feed.like` was written), and its response lacks `validationStatus`.
   - **Risk:** a PDS validates collections it knows even when `validate` is unset. Once our
     lexicons are published and PDSes start resolving them, our records will be validated against
     them. Hence §2.11's rule that published limits never change. Conformance tests write every
     record kind with the published schema loaded into a local `Lexicons` instance.
6. **Writers send `$type`** even though the PDS injects it when missing (§2.2). A mismatched `$type`
   is a 400. Floats are rejected by the PDS, which matches our "no floats" rule; core rejects them
   earlier.
7. **Blobs.**
   - **Embed the blob object returned by `uploadBlob` verbatim** (§3.5, now confirmed). The PDS
     sniffs and normalises `mimeType` (for example `image/jpeg` → `image/png`; parameters
     stripped). A record whose `mimeType` or `size` differs from the stored blob is rejected with
     `InvalidMimeType`/`InvalidSize`, and an unknown CID with `BlobNotFound`.
   - The local preview CID must equal `ref.$link`. The local mime is a guess until the upload
     returns.
   - `textBlob` uploads use `text/plain` (stored as such). Readers accept `text/*` and decode as
     UTF-8.
   - **Nested refs are tracked at any depth**, including `revision.record.body.media[i].blob`
     inside an `unknown` field. A blob stays alive while any record references it. An edit that
     drops media from the post keeps the media for as long as the revision exists.
   - Deleting the last referencing record collects the blob immediately: `getBlob` answers
     `400 InvalidRequest "Blob not found"`, not 404. Consequences:
     - `history redact` and Delete (`[delete target, delete its revisions]`) really remove the media
       from the author's PDS.
     - The media cache maps `getBlob` 400 "Blob not found" to *missing* (not retried) and
       purges the cached copy (§3.9 reference counting).
     - Upload blobs only when publishing and reference them in the same outbox operation. An
       unreferenced upload is never served and is cleaned up by the PDS; its temporary lifetime was
       not measured.

## Consequences

- **Outbox (M2/M3).** Implement decisions 1, 3 and 4. FakePds must reproduce:
  - `createRecord` on an existing key → 500 with nothing written;
  - `applyWrites` on missing or present keys → 500;
  - `putRecord` with `swapRecord: null` → `InvalidSwap`;
  - `deleteRecord` on an absent key → `200 {}`;
  - an identical `putRecord` → no `commit`.

  The parity scenario covers an opinion race (two clients) and an edit race.
- **Engine error mapping.** `InvalidSwap` → `Conflict{current?}`; `InvalidMimeType`, `InvalidSize`
  and `BlobNotFound` on write → `MediaRejected`; 500 on a write → `Ambiguous`, resolved by a re-read
  and never retried blindly.
- **§3.5 edits** follow decisions 1 and 3; this ADR is the source.
- **Still open:**
  - Behaviour on non-reference PDSes (rsky, Go implementations) for `swapRecord: null` and for
    `applyWrites` errors. It is checked by the per-host compatibility probe in `lessdugin doctor`,
    against devnet-like test accounts only, because we never write to production PDSes we do not
    own.
  - The lifetime of temporary (unreferenced) blobs.
