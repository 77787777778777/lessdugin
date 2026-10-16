<!-- SPDX-License-Identifier: CC0-1.0 -->

# lessdugin sync, discovery, verification and storage (normative)

**Status:** draft for the `.temp` namespace (M0). It is frozen with the lexicons at M7, except for the operational numbers marked *tunable* (timeouts, intervals, concurrency), which may change in any release without a new behaviour ID.
**Sources:** `docs/design/synthesis-v1.md` §1.2–§1.4, §3, §6.3, §6.4 and §6.14, with every fix from `docs/design/completeness-review.md` and the approved plan applied, plus the accepted spike ADRs 0003 (Jetstream, relay, Constellation), 0004 (`listRecords` cursors, read budgets), 0005 (write semantics, blobs) and 0006 (devnet).

## 0. About this document

### 0.1 Key words

MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are used as in RFC 2119 and RFC 8174.

### 0.2 Contents

This document defines:
- trust levels, badges, source rules, dependency tiers and settings (§1, §2);
- identity resolution and the login binding (§3);
- forum resolution, participant discovery and the closure (§4);
- per-repo sync (L1), including the cursor-range probe and the normative fetch caps (§5);
- live and incremental updates (§6);
- the sync side of the write path (§7);
- the `proofs/1` verification algorithm and the currency rule for third-party proofs (§8);
- the scheduler and budgets (§9);
- the cache and state stores, invariants I1–I9, the key schema and eviction (§10);
- blobs and media display (§11);
- the `Host` and `KvStore` contract (§12) and the `ForumModel` (§13);
- the cost model and indexer thresholds (§14);
- the optional indexer: ingest, serving and client audit (§15).

### 0.3 Companion specifications

| Document | Covers |
|---|---|
| `data-model.md` | lexicons, record keys (`fk/1`, `sk/1`), record validity (§7), fetch caps (§7.4), the write protocol (§8), indexer lexicons (§11) |
| `social.md` | the fold (`FoldInput`, `inputDigest`, `degraded`), time, participation, `mod/1`, `karma/1`, `magic/1`, visibility |
| `lfm-1.md` | rendering, `short_did`, `BADGE_GLYPHS`, `term_sanitize`, media placeholders |
| `limits.md` | every byte, count and depth cap (this document repeats the ones it uses) |
| `threat-model.md`, `glossary.md` | adversaries and measures; terms |

### 0.4 Behaviour IDs

This document defines **`proofs/1`** (§8): CAR parsing, commit verification, the MST walk, and the indexer proof-bundle manifest. Its vectors live in `testdata/vectors/proofs/`. It implements the normative fetch caps of `data-model.md` §7.4 (§5.4) and produces the `records`, `absent`, `obs` and `repoStatus` inputs of the fold (`social.md` §1.1).

### 0.5 Conventions

| Term | Meaning |
|---|---|
| `NS` | the build's namespace prefix (`data-model.md` §1.3). `.temp` builds substitute it everywhere, including Jetstream patterns. |
| `OURS` | the ten record collections `NS.{forum, membership, post, comment, revision, opinion, modAction, wikiRevision, sequence, profile}` |
| `RANGED` | `NS.{post, comment, revision, opinion, modAction, wikiRevision, sequence}`: the collections listed by `{fk}-` key range |
| PDS of D | the `serviceEndpoint` of the `#atproto_pds` service in D's current DID document (§3.2) |
| host | the origin (scheme, host, port) of a URL. Budgets, breakers and the cursor-range flag are kept per host. |
| rev | a repo revision, a TID. Revs are compared as strings, bytewise, which is time order. `""` (no rev) sorts below every TID. |
| head | the `{cid, rev}` returned by `com.atproto.sync.getLatestCommit` from the PDS of the repo |
| `getRecord`, `listRecords`, `describeRepo`, `applyWrites`, `putRecord`, `deleteRecord`, `uploadBlob` | `com.atproto.repo.*` |
| `getLatestCommit`, `sync.getRecord`, `getRepoStatus`, `getBlob`, `listReposByCollection` | `com.atproto.sync.*` |
| `uniform(a, b)` | a random value in [a, b] from `Host::fill_random`. Randomness only schedules work and never reaches a fold input or an output. |
| µs, ms | integer microseconds (fold time) and milliseconds (host clock, `Host::now_ms`) |
| *tunable* | an operational constant that may change without a new behaviour ID |
| **Reserved (Pn)** | specified only far enough to keep the design open; not implemented before phase n |

**Precedence.** `data-model.md` wins on record semantics and the write protocol. This document wins on everything listed in §0.2. Where an accepted ADR refines the synthesis, this document follows the ADR and says so.

---

## 1. Trust model

### 1.1 Content trust levels

| Level | A record version (uri, cid, value) has this level if and only if |
|---|---|
| `hint` | it came from any source that does not meet `pds` or `proven`: Jetstream, Constellation, the relay, Slingshot, indexer JSON (`#recordRef.value`, `#postView`, tallies), or a proof that failed. |
| `pds` | **all** of: (a) it was returned by an XRPC GET (`getRecord` or `listRecords`) sent to the PDS of `did(uri)`, taken from a DID document at identity level `doc` (§1.3); (b) over TLS (`https`; `http` only in devnet mode, §3.7); (c) no redirect was followed; (d) the response's JSON `value`, converted to DAG-CBOR (§5.5), gives a CIDv1 (dag-cbor `0x71`, sha2-256) equal to the `cid` in the same response. |
| `proven@R` | `verify_proof` (§8) returned `Proven{rev: R, cid}` for `(did, collection, rkey)` of the uri, using the `#atproto` key of a DID document at level `doc`. For a proof that did not come from the author's PDS (a **third-party proof**), that document was fetched at most 1 hour before verification. |

**Order.** `hint < pds < proven`. For the same `(uri, cid)`, a stored level only ever increases (I5).

**Own writes.** A version written by this client and confirmed by the PDS response (§5.8) has level `pds`.

### 1.2 Currency of third-party proofs

A third-party proof can be valid and still old: a signed commit at an old rev stays verifiable forever, so an indexer could replay a deleted or edited record. Therefore:

```
current(proof p of uri u at rev R) ⇔
    p came from the author's PDS (sync.getRecord at PDS(did(u)))                       // always current
  ∨ ∃ head observation h of PDS(did(u)) with R ≥ h.rev ∧ h.sentAt ≥ p.receivedAt − 60 s  // third-party
```

- A head observation is any `getLatestCommit` response from the PDS of the repo, recorded as `repo.headSeen = {rev, cid, sentAt}` (§5.9).
- A third-party proof that is not current is **`[stale]`**:
  - it is not a fold input;
  - it is held in memory only (it is never written to the record store);
  - it MAY be displayed as a provisional item (§1.5);
  - the engine schedules a head check of the author (§1.2.1).
- Head checks are **batched per author**: one `getLatestCommit` covers every pending proof of that DID. Authors on screen get P0; others P2.

#### 1.2.1 Resolving a stale proof

```
on head observation h for did:
  for each stale proof p (uri u, rev R, cid C) of did:
    if R ≥ h.rev:  admit p as proven@R, current, with order key (R, h.sentAt)          (§5.6)
    else:          schedule a point fetch getRecord(u) at the author's PDS (P0 if on screen, else P2)
on that point fetch returning version v (level pds):
    admit v (§5.6)
    if v.cid = C:  the stored version's level becomes proven@R (same (uri, cid): level only increases)
    drop p
on RecordNotFound: tombstone (§5.6); drop p
```

**Never advance repo state from a third-party proof.** `repo.rev`, `repo.listed` and `repo.headSeen` change only from responses of the author's PDS (§5.9).

### 1.3 Identity levels and handle states

| Kind | Level | Definition |
|---|---|---|
| Identity (per DID) | `hint` | the document or mini-doc came from Slingshot, an AppView or an indexer |
| | `doc` | the DID document came over TLS from the PLC directory (`did:plc`) or from `https://<host>/.well-known/did.json` (`did:web`), and passed §3.2 |
| | `audited` | **Reserved (P7):** `doc`, plus the PLC audit log verified from genesis |
| Handle | `verified` | the DID document's claimed handle resolves back to the same DID (§3.1) |
| | `unverified` | there is no claimed handle, or it does not resolve back (no answer, another DID, transport failure) |
| | `invalid` | the claimed handle fails handle syntax, or DNS returned more than one distinct DID (§3.1) |

### 1.4 Source rules

| Source | Text shown? | Discovery and scheduling? | Fold input (moderation, karma, visibility)? |
|---|---|---|---|
| Author PDS (`pds`) | yes | yes | yes |
| Proof CAR from the author's PDS (`proven`) | yes | yes | yes |
| Third-party proof CAR (`proven`), current (§1.2) | yes | yes | yes |
| Third-party proof CAR, `[stale]` | provisional only (§1.5) | yes | **no** |
| Records of a DID whose identity is `hint` | provisional only (§1.5), with `[identity unverified]` | yes | **no** |
| Jetstream | no (counts and placeholders only) | yes (dirty marks, point fetches, newcomer candidates) | no |
| Constellation, relay, Slingshot | no | yes | no |
| Indexer JSON and aggregates | no; scores MAY show as `~n` until recomputed locally | yes | no |

**Rules for hints**
- Hints MAY: discover DIDs and uris; schedule work; order placeholders; supply approximate counts (`~n`); supply first-seen times (§5.6.5).
- Hints MUST NOT: supply any text attributed to a DID (titles, bodies, names, reasons); feed moderation, karma, gates or visibility; be written to the record store (I5).
- A placeholder MAY show: the author's handle if verified by our own resolution (else `short_did`), a time, and counts. Links inside hint data are never rendered.
- Indexer fields such as `#postView.state`, `tier` and `rank` are ignored by the fold. They only order placeholders.

### 1.5 Fold-input eligibility and provisional items

```
eligible(v) ⇔ (v.level = pds ∨ (v.level = proven ∧ current(v)))
              ∧ ¬v.idHint                                   // identity level doc (§1.3)
              ∧ status(did(v.uri)) ≠ inactive                // I8, §3.4
```

The `records` input of the fold (`social.md` §1.1) is built from eligible stored versions only, after the fetch caps (§5.4). Invalid records are eligible and are fold inputs (the fold excludes them in V1).

**Provisional items.** A post, comment, sequence or wikiRevision that is not eligible **only** because it is a `[stale]` third-party proof or because its author's identity is `hint` MAY be displayed as a provisional item:
- it is shown with its badge (`[stale]` or `[identity unverified]`);
- its visibility is computed by **provisional evaluation**: the visibility rules V1–V10 (`social.md` §10.1) are applied to it as a leaf, using the fold result computed *without* it. It contributes nothing else: no score, karma, gate, thread position for other items, notification, search hit or aggregate;
- only `Visible` or `Collapsed(*)` provisional items are shown, and an identity-hint item is always shown `Collapsed`;
- nothing is shown provisionally while the forum view is `degraded` (`social.md` §4.11);
- opinions, modActions, memberships, revisions, declarations and profiles are never provisional.

### 1.6 Badges

| Badge | Shown on | Condition |
|---|---|---|
| (none) | item | level `pds`, identity `doc` |
| `[verified]` | item | level `proven`, current, identity `doc` |
| `[stale]` | item | a `[stale]` third-party proof shown provisionally (§1.2); or an eligible item whose author's `repoStatus` is not `Fresh` and whose last sync attempt failed (shown from cache) |
| `[partial k/n]` | list, thread, score, karma | completeness fails (§4.5); k of n completeness repos are complete |
| `[partial]` | user card, sync status | that repo has a capped or byte-capped range in this forum (§5.4) |
| `[future-dated]` | item | `future(x)` (`social.md` §2.2) |
| `[identity unverified]` | item, name | the author's identity level is `hint` |
| `[edited]` | item | the current version has `updatedAt`, or a valid revision with `subject.uri` = the item is in `records` |
| `[hint]` | placeholder | the row exists only because of a hint |
| `[pending]` | own item, own opinion | an outbox write for it is not `done` (§7.4) |

**Rendering.** Badges are drawn only in the fixed badge column, as the bracketed ASCII words above or glyphs from `BADGE_GLYPHS` (`lfm-1.md` §9.1). `term_sanitize(Name)` removes `BADGE_GLYPHS` from user-supplied names and titles, so user text cannot imitate a badge.

---

## 2. Dependency tiers and settings

### 2.1 Tiers

| Tier | Services |
|---|---|
| **CORE** | author PDSes; the PLC directory and did:web hosts; DNS over HTTPS (DoH; the provider is configurable) |
| **CORE+** | relay `listReposByCollection`; Jetstream; Constellation; Slingshot |
| **EXT** | lessdugin indexers |

### 2.2 Features by tier

| Feature | CORE | CORE+ | EXT |
|---|---|---|---|
| Read and write every content type | ✓ | | accelerates |
| Approval-mode participant set, complete | ✓ | | |
| Open mode: granted and established users (closure, §4.3) | ✓ | | |
| Open mode: brand-new newcomers; approval-mode applications | | ✓ | ✓ |
| Forum directory | | ✓ relay | ✓ `listForums` |
| Live updates | polling (§6.7) | ✓ Jetstream | ✓ `getChanges` |
| Notifications in followed forums | ✓ at sync time | ✓ live | offline and cross-forum: **Reserved (P2)** (`getActivity`) |
| Search | ✓ local, core in-memory index (§13) | | global `searchPosts`: **Reserved (P2)** |
| Karma, scores, moderation | ✓ | | hints only |
| Verification | ✓ `verify` | | ✓ proof CARs |
| Browser cold start above ~300 participants | slow | | ✓ |

**Open mode depends on CORE+.** In `sources = direct` an open forum never admits newcomers: seeds never see their content, so the closure never grows. The UI labels open mode "relies on public relay infrastructure (Bluesky relay, Jetstream, Constellation)" (`data-model.md` §6.1). Approval mode is complete at CORE.

### 2.3 Settings

| Setting | Values | Default | Meaning |
|---|---|---|---|
| `sources` | `direct`, `assisted` | `assisted` | `direct` uses CORE only. `assisted` adds CORE+ and every enabled indexer (§2.4). |
| `verify` | `pds`, `proof` | `pds` | `pds`: content is admitted at level `pds`. `proof` (verify every displayed item in the background): **Reserved (P2)**. On-demand `lessdugin verify` (§8.8) is always available. |
| `network.dns` | `doh`, `system` | `doh` | native only; `system` answers are flagged `dns-unauthenticated` (§3.1) |
| `network.doh` | list of DoH JSON endpoints | `cloudflare-dns.com`, then `dns.google` | |
| `network.plc` | URL | `https://plc.directory` | |
| `network.jetstream` | list of hosts | `jetstream.us-east.bsky.network`, `jetstream.us-west.bsky.network` | |
| `network.relays` | list of URLs | `https://bsky.network`, `https://relay1.us-east.bsky.network` | |
| `network.constellation` | URL | `https://constellation.microcosm.blue` | |
| `network.slingshot` | URL | `https://slingshot.microcosm.blue` | |
| `indexer` | `forum`, `none`, or a list of URLs | `forum` | which indexers to use (§2.4); CLI `--indexer <url\|none\|forum>` |
| `media.hosts` | list of hostnames | empty | the user's media autoload allowlist (§11.3) |

Settings live in the `state` store (`loc`, §10.4). CLI flags override them per invocation.

### 2.4 Endpoints and consent

- An indexer comes from the user's configuration or from the forum's `discovery.indexers` (`data-model.md` FO3).
- A forum-declared endpoint is used only after **one-time user consent** for that exact URL, stored in `state`. Only `https:` URLs with a hostname (not an IP literal) and the default port are eligible.
- An indexer is reached at its URL, or at the `serviceEndpoint` of service `#lessdugin_index` (type `LessduginIndex`) in its `did:web` document.
- In `direct` mode no CORE+ or EXT endpoint is contacted, including Slingshot.

---

## 3. Identity

### 3.1 Handles: resolution, verification and display

**Syntax.** A handle is lowercased, then MUST match the atproto handle syntax. Handles ending in `.alt`, `.arpa`, `.example`, `.internal`, `.invalid`, `.local`, `.localhost` or `.onion` are invalid; `.test` is invalid outside devnet mode (§3.7). `xn--` labels are never decoded for display.

**Resolution** (identical on every platform, so that clients reach the same verdicts):

```
resolve_handle(h) -> Did(d, method) | NoAnswer | Invalid
  h = ascii_lowercase(h); if ¬handle_syntax(h): return Invalid
  // 1. DNS TXT, authoritative
  for p in dns_providers:                         // network.doh in order; or the system resolver (§2.3)
     r = TXT query "_atproto." + h via p
     if r is a transport failure: continue        // network error, timeout, HTTP ≠ 200, bad JSON,
                                                  // DNS Status ∉ {0 NOERROR, 3 NXDOMAIN}
     ds = { s[4..] : s ∈ txt_strings(r), s starts with "did=", s[4..] is a valid DID (data-model §2.3) }
     if |ds| = 1: return Did(the element, p.method)
     if |ds| > 1: return Invalid
     goto WELLKNOWN                               // authoritative empty answer
  WELLKNOWN:                                      // reached if DNS answered empty or every provider failed
  r = GET https://<h>/.well-known/atproto-did     // Anon; ≤ 3 redirects, https only; ≤ 2,048 bytes; 20 s
  if r.status = 200 ∧ trim_ascii_ws(r.body) is a valid DID: return Did(it, wellknown)
  return NoAnswer                                 // a browser CORS failure is a transport failure
```

- **DoH JSON.** Requests are `GET <endpoint>?name=<qname>&type=TXT` with `accept: application/dns-json` (CORS-safelisted). `txt_strings(r)` takes every `Answer[]` entry with `type = 16`; if its `data` begins with `"`, it is parsed as a sequence of quoted character-strings (handling `\"`, `\\` and `\DDD`), which are concatenated; otherwise `data` is taken as is.
- **System DNS** (native, `network.dns = system`): `Host::resolve_txt_system`. Its verdicts are flagged `dns-unauthenticated` in `resolve` and `doctor` output.
- **Slingshot** is a last resort, only in `assisted` mode, only after `NoAnswer` (never after `Invalid`): `GET <slingshot>/xrpc/com.bad-example.identity.resolveMiniDoc?identifier=<h>`. A DID found this way has identity level `hint` until §3.2 succeeds.

**Bidirectional verification.**

```
handle_state(did) =
  let h = claimed handle of the doc (§3.2)
  h absent                               → unverified
  ¬handle_syntax(h)                      → invalid
  resolve_handle(h) = Invalid            → invalid
  resolve_handle(h) = Did(did, _)        → verified        // the resolution must not be a hint
  otherwise                              → unverified
```

**Display**
- A handle is displayed only if it is `verified`.
- `unverified`: show `short_did(did)` (`lfm-1.md` §5.4) and the claimed handle struck through.
- `invalid`: show `short_did(did)` only; details say "handle invalid".
- Stored references are always DIDs (`data-model.md` W7).
- **Confusables.** A UTS #39 skeleton check compares participants' display names and handles with those of the owner and the moderators. A different DID with an equal skeleton gets a warning marker (`threat-model.md`). Checks against frequent posters: **Reserved (P2)**.
- Owners and moderators whose DID is `did:web` are warned about: domain expiry means takeover (`data-model.md` §6.1, succession).

### 3.2 DID documents

**Fetch**
- `did:plc`: `GET <network.plc>/<did>`.
- `did:web:<host>`: `GET https://<host>/.well-known/did.json`. Forms with a port or a path are rejected (`data-model.md` §2.3).
- Both are Anon requests with at most 3 redirects, `https` only, at most 65,536 bytes and 20 s (*tunable*).

**Parsing.** The document is accepted at level `doc` only if:
1. it is a JSON object whose `id` equals the DID;
2. **PDS:** exactly one `service` entry has `id` = `#atproto_pds` or `<did>#atproto_pds` and `type` = `AtprotoPersonalDataServer`. Its `serviceEndpoint` is a URL with scheme `https` (`http` only in devnet mode), a hostname that is not an IP literal (except in devnet mode), no userinfo, query or fragment, and a path that is empty or `/`;
3. **Key:** exactly one `verificationMethod` entry has `id` = `#atproto` or `<did>#atproto` and `type` = `Multikey`, with a `publicKeyMultibase` that decodes (base58btc, multibase prefix `z`) to the multicodec `secp256k1-pub` (`0xe7`) or `p256-pub` (`0x1200`) followed by a 33-byte compressed point. Other key types leave the key **unavailable**: proofs fail with `UnsupportedKey`, and `pds`-level content is unaffected.

**Claimed handle.** The first `alsoKnownAs` entry that starts with `at://`, without that prefix, lowercased.

**Failure.** A DID whose document cannot be fetched or parsed has no PDS: its content cannot be admitted. In the browser, a CORS failure on a `did:web` document falls back (assisted mode only) to the Slingshot mini-doc `{did, handle, pds, signing_key}`. That identity is `hint`; its content is admitted only provisionally (§1.5) and shown collapsed with `[identity unverified]`. An authority with a `hint` identity counts as never listed (`Unavailable`), so the view degrades, which fails closed.

### 3.3 Lazy resolution, caching and invalidation

**What gets resolved** (F16). Identities are resolved only for:
- authorities and candidates that are being synced (§4);
- authors of items on screen;
- mentions: the first 16 distinct participant DIDs per displayed body, eagerly (`lfm-1.md` §5.4);
- anything else only on hover, focus or click.

DIDs that merely appear in records are never resolved eagerly.

**Cache durations** (*tunable*): handle → DID 6 h; DID document 24 h; handle state is recomputed when either expires.

**Invalidation.** Both the handle and the document of a DID are invalidated by any of:
- a Jetstream `identity` event for a known DID (§6.4);
- a proof signature failure: re-fetch the document once and retry; if the retry also fails, the proof fails;
- an XRPC error `RepoNotFound`, `RepoDeactivated` or `RepoTakendown` from the PDS: re-fetch the document; if the PDS changed, this is a **migration**: cached records stay valid, the repo is marked dirty at P1 and synced at the new PDS; otherwise check the account status (§3.4).

**Third-party proofs** require a document fetched at most 1 hour before verification (§1.1). An older cached document is re-fetched first.

### 3.4 Account status

- Status comes from `getRepoStatus?did=<did>` at the repo's PDS, and is checked:
  - on a Jetstream `account` event for a known DID;
  - after the errors listed in §3.3;
  - by the daily reconcile (§6.8).
- The DID is **inactive** if `active = false` and `status` ∈ {`takendown`, `suspended`, `deleted`, `deactivated`} or `status` is absent. `desynchronized` and `throttled` are temporary: the repo is treated as failing (its cached records stay, `[stale]` badge), not as inactive.
- Records of an inactive DID are excluded from every fold input (I8) and hidden. They stay in the cache for 7 days (*tunable*), then are purged. If the account is active again within that time, a P1 sync restores them.
- Status changes are never taken from a hint: a Jetstream `account` event only triggers the check.

### 3.5 PLC pacing

Requests to the PLC directory: at most 8 in flight and at most `plcRate` requests per second, where `plcRate` starts at 20. On 429: honour `Retry-After`, and set `plcRate = max(1, plcRate / 2)`. After every 60 s without a 429: `plcRate = min(20, plcRate + 2)`. PLC sends no rate-limit headers (ADR 0004).

### 3.6 Login binding

These checks apply to every OAuth session (`data-model.md` §10.5) before it is used for any write:

```
bind_login(token_response, intended?):       // intended = the DID the user asked to log in as, if any
  sub = token_response.sub;  iss = token_response.iss (the authorization server issuer)
  if intended ∧ sub ≠ intended: fail("logged in as a different account")
  doc = fresh DID document of sub (§3.2, level doc; never a hint)
  pds = PDS of doc
  prm = GET <pds>/.well-known/oauth-protected-resource      // Anon, no redirects, ≤ 65,536 bytes
  require iss ∈ prm.authorization_servers                     // exact string comparison
  pin = state.loc["as_pin", sub]
  if pin absent: show iss; store pin = iss                    // trust on first use
  elif pin ≠ iss: warn ("authorization server changed: <pin> → <iss>"); require explicit confirmation; then re-pin
```

- The check uses the PDS's protected-resource metadata, so entryway setups work: a `bsky.social` user's issuer is `https://bsky.social`, while their PDS is a `*.host.bsky.network` host that lists it.
- In the SPA, the core resolves the handle (§3.1) and the main thread calls `signIn(did)`, so the OAuth library's own handle resolver is unused. Every client sends `response_mode=query` (`data-model.md` §10.5).
- `UserPds` requests (§12.1) go only to the PDS bound at login. If a later document fetch shows a different PDS (migration), the session is marked `needs-relogin`.
- **App-password sessions** (`createSession` at the PDS from the DID document) have no issuer. The session's `did` MUST equal the resolved DID.

### 3.7 Devnet mode

Devnet mode (`--dev` or a config profile; never enabled implicitly; ADR 0006):
- handles resolve through the PDS's `com.atproto.identity.resolveHandle` (method `devnet`), and `.test` handles are allowed;
- the PLC base URL is configurable, and `http://` PDS, PLC and Jetstream endpoints and IP-literal hosts are allowed;
- the SSRF guard (§12.1) allows loopback.

---

## 4. Forum resolution and participant discovery

### 4.1 Forum references

A forum reference is one of:
- a canonical forum at-uri `at://<did>/NS.forum/<slug>`, or the same with a handle authority (resolved to a DID first, §3.1);
- an SPA URL `…#/f/<owner>/<slug>`, where `<owner>` is a handle or DID;
- `<handle|did>/<slug>`.

All of them reduce to the canonical at-uri F, and `fk = fk(F)` (`data-model.md` §3.1).

### 4.2 Opening a forum

```
open_forum(ref):
  F = canonical uri (§4.1); owner = did(F); fk = fk(F)
  id = identity(owner)                        // §3.2; a hint identity → the forum is unavailable (fail closed)
  decl = getRecord(owner PDS, NS.forum, slug(F))            // level pds
     RecordNotFound → tombstone; "forum not found"
     invalid (data-model §7) → "forum invalid"; the forum cannot be opened (fail closed, FO4)
  if no proof of the declaration is cached:
     p = sync.getRecord(owner, NS.forum, slug(F)) → verify_proof (§8)
     Proven{cid = decl.cid} → the declaration's level becomes proven@rev
     otherwise → warning "declaration proof failed" in the forum header; decl stays at level pds
  A = {owner} ∪ { m.did : m a valid moderators entry (FO2) }              // authorities
  for a ∈ A: enqueue sync_repo(a, {fk}, P0, "authority")                 // §5; full {fk}- ranges
  — then, as authority repos complete (the fold re-runs each time) —
  G, B, N, S from the fold over the available records (social.md §3.1)
  candidates =
    approval: A ∪ G
    open:     A ∪ G ∪ Closure (§4.3) ∪ Newcomers (§4.4)
  for c ∈ candidates ∖ A: confirm(c)                                      // §4.2.1
  schedule the confirmed candidates in the order of §4.5
  the fold re-runs as repos complete; aggregates show [partial k/n] until §4.5 holds
```

The view state is `degraded` while some authority is `Unavailable` (never completely listed, §5.9) (`social.md` §4.11).

#### 4.2.1 Confirming a candidate

```
confirm(d):                                   // per (forum, candidate)
  identity(d) at level doc, else: stays a candidate; retried with the identity backoff
  m = getRecord(PDS(d), NS.membership, fk)    // level pds; usually served from cache after a sync
  valid m (ME1) with m.forum = F → confirmed: sync_repo(d, {fk}, priority per §4.5)
  RecordNotFound or invalid → rejected (disc row, §10.4); re-checked only when a new hint names d
```

Authorities need no confirmation: they are always synced. A confirmed candidate becomes a fold participant only through the rules of `social.md` §3.

### 4.3 Closure (CORE)

The closure lets open forums be complete for S ∪ E without CORE+ services:

```
Closure(F) = { did(u) : u ∈ refs } ∖ (A ∪ G)
refs       = ⋃ over records r in records with author(r) ∈ S and r.forum = F:
               r is an opinion → { r.subject.uri }
               r is a comment  → { r.root.uri } ∪ { r.parent.uri if present }
```

- The closure is one level deep. It is recomputed whenever the records of S change.
- Each closure DID is confirmed (§4.2.1) before it is synced: only DIDs with a valid membership record are synced (F16).
- **Closure property** (informative; `social.md` §3.3). E depends only on seeds' votes, and anyone who earns seed upvotes is referenced by a seed opinion. So K, scores and the visibility of other users are the same with or without CORE+ discovery. `unweighted` counts and the collapsed content of newcomers do differ.

### 4.4 Newcomers and applications (CORE+, EXT)

Newcomers are participants found only through hints. They are discovered in `assisted` mode only, and each one is confirmed (§4.2.1):

| Source | Exact use |
|---|---|
| Constellation | `GET <constellation>/xrpc/blue.microcosm.links.getBacklinks?subject=<F>&source=<NS>.membership:forum&limit=100[&cursor=…]`. Keep paging while `cursor ≠ null`: a short page does not mean the end. Entries `{did, collection, rkey}` with `rkey ≠ fk` are dropped without a fetch. `getBacklinksCount` with the same parameters gives the "N members (hint)" badge. (ADR 0003) |
| Jetstream | commit events of `NS.membership` with `rkey = fk` (§6.4) |
| Indexer | `listParticipants(forum = F)`, paging by cursor |
| Relay | `listReposByCollection(collection = NS.membership, limit = 500)` across `network.relays`, only while the union of returned DIDs is ≤ 5,000. Above that, this source is disabled for the session. |

- **Budget.** At most 500 newcomer candidates per forum are confirmed and synced (*tunable*). Candidates are taken most recent first: by Jetstream `time_us`, then Constellation order (newest first), then indexer order; ties by DID ascending. The rest stay as `disc` rows.
- **Applications (approval mode).** Applicants (`member(d) ∧ access(d) = none`) are found in the same way, so moderators see them only through CORE+ or out of band. The mod queue says so.

### 4.5 Sync order and completeness

**Order** (each step's repos are enqueued at the priority shown; the queue is §6.1):

| Step | Repos | Priority |
|---|---|---|
| 1 | authorities A | P0 |
| 2 | authors of items on screen | P0 |
| 3 | recently active authors (assisted): Constellation `getBacklinks(subject = F, source = <NS>.post:forum, limit = 100)` newest first, and the DIDs of commits in the `{fk}-` ranges seen in a Jetstream replay from `now − 36 h` (§6.3) | P1 |
| 4 | S | P1 |
| 5 | confirmed closure DIDs | P1 |
| 6 | confirmed newcomers, most recent first, within the budget | P2 |

**Completeness set**

```
C(F) = approval: A ∪ G
       open:     A ∪ S ∪ ConfirmedClosure
complete(d, fk) ⇔ repoStatus(d, fk) = Fresh      (§5.9), and no byte cap was hit in the last listing
n = |C(F)|;  k = |{ d ∈ C(F) : complete(d, fk) }|
```

Lists, threads, scores and karma show `[partial k/n]` while `k < n`. A repo whose range was capped by the normative fetch caps (§5.4) counts as complete: every client excludes the same records. Its user card shows `[partial]`.

**Time budget.** Interactive CLI commands wait for sync for at most `--time-budget` (default 10 s), then print with `[partial …]` and exit with code 6 if incomplete. `--max-requests` caps requests per command; `--offline` sends none.

### 4.6 Fail closed

- An authority repo that was never completely listed is `Unavailable`, and the view is `degraded` (`social.md` §4.11): only content by S is shown, under a banner.
- A failing authority repo that *was* listed before is `Stale`: its cached assertions apply, and moderation never vanishes because a PDS is down.
- A forum whose owner identity is `hint`, whose declaration is missing or whose declaration is invalid cannot be opened.

### 4.7 Forum directory

```
directory():
  owners = ⋃ over r ∈ network.relays: listReposByCollection(r, collection = NS.forum, limit = 500), paged until no cursor
  for d ∈ owners: listRecords(PDS(d), NS.forum), full collection                 // level pds
  ∪ indexer listForums (hint: each forum is fetched from its owner's PDS before it is displayed)
  group by owner: the verified handle, else short_did (F9); sort groups by handle, then DID
```

`listReposByCollection` takes one exact NSID; wildcards are rejected (ADR 0003). Its entries carry only `did`. The directory is CORE+ (relay) or EXT; a forum can always be opened directly by reference.

### 4.8 Followed forums

- The followed list is local (`state`). Clients SHOULD seed it from the user's own membership records: on login and after each sync of the own repo, every valid own `NS.membership` record's `forum` is added to Home, unless the user unfollowed it locally (a `loc` "unfollowed" mark).
- The own repo is synced with scope `OWN` (§5.1), which lists every own collection fully.
- Followed forums define the Jetstream relevance set (§6.4) and the reconcile set (§6.8).

---

## 5. Per-repo sync (L1)

### 5.1 Scopes

| Scope | Meaning |
|---|---|
| `fk` | the forum key of a followed or open forum: the `{fk}-` range of each collection in `RANGED`, plus the membership record with rkey `fk` |
| `GLOBAL` | `NS.profile/self` (falling back to `app.bsky.actor.profile/self`, `data-model.md` §6.10), plus the declaration of every followed forum owned by this DID |
| `OWN` | the user's own repo: every collection in `OURS` listed fully |

Sync windows (time-bounded ranges) are **Reserved (P7)**. In v1 every range is listed completely, up to the fetch caps.

### 5.2 `sync_repo`

```
sync_repo(did, scopes, reason) -> Clean | Applied | Unstable | Rollback | Failed(e)
  take the repo lock (§10.6)
  id   = identity(did)                                    // level doc, else every record is idHint (§1.5)
  h1   = getLatestCommit(did)                             // record repo.headSeen = {h1.rev, h1.cid, sentAt}
         RepoNotFound | RepoDeactivated | RepoTakendown → §3.3, §3.4; Failed
  if repo.rev ≠ "" ∧ h1.rev < repo.rev: return Rollback   // §5.9
  if h1 = (repo.rev, repo.commit) ∧ ∀ s ∈ scopes: repo.listed[s] = repo.rev ∧ repo.marks = ∅:
     return Clean
  colls = (now − repo.collsAt ≤ 24 h ∧ reason ≠ reconcile ∧ ¬repo.collsDirty)
          ? repo.colls
          : describeRepo(did).collections ∩ (OURS ∪ {app.bsky.actor.profile})
  for each s ∈ scopes, c ∈ RANGED ∩ colls:          res[s, c] = list_range(did, c, s)      // §5.3; parallel
  for each fk-scope s:  mem[s] = NS.membership ∈ colls ? getRecord(did, NS.membership, s) : Absent
  GLOBAL:  profile and declarations by getRecord;   OWN: list every c ∈ OURS ∩ colls fully
  every returned record passes admission (§5.5): a CID mismatch drops it (and strikes the host);
     an invalid record is kept with valid = false
  h2     = getLatestCommit(did)
  stable = (h2.rev = h1.rev ∧ h2.cid = h1.cid)
  batch  = []
  for each (s, c):
     for each listed row: batch += admit(row, key = (h1.rev, sentAt of its page), exact = stable)      // §5.6
     if stable ∧ res[s, c].complete:
        boundary = res[s, c].capped ? the smallest listed rkey : none
        for each stored row of (s, c, did) not listed:
           if boundary = none ∨ rkey > boundary: batch += tombstone(uri, key = (h1.rev, sentAt of the first page))
           else:                                  batch += delete the row (beyond the cap; no tombstone)
  for each fk-scope s: mem[s] present → admit; RecordNotFound → tombstone if a row is stored
  if stable:
     repo.rev = h1.rev; repo.commit = h1.cid
     for each s whose ranges were all complete:
        repo.listed[s] = h1.rev; repo.capped[s] = { c ↦ boundary of res[s, c] : res[s, c].capped }
     remove every mark with rev ≤ h1.rev (§6.5)
  repo.colls/collsAt/collsDirty updated if describeRepo ran; repo.checkedAt = now; repo.fails = 0; repo.lastOk = now
  KvStore::write(cache, batch ++ [put repo row])                                    // one atomic batch
  if ¬stable: enqueue(did, minRev = h2.rev, P1, "unstable"); return Unstable
  emit Changed; return Applied
```

**Rules**
- An unstable pass (the head moved during listing) still admits what it listed, but writes no tombstones and advances no `listed` entry. After 3 consecutive unstable passes the repo is re-queued at P3 with the repo backoff (§9.4).
- **`describeRepo` freshness.** `repo.colls` is reused for up to 24 h. It is refreshed early when `repo.collsDirty` is set: a hint or an own write named a collection in `OURS` that is not in `repo.colls`. A collection created less than 24 h ago and named by no hint can therefore be missed until the next reconcile, which always refreshes. If `describeRepo` fails, `colls = OURS`.
- **Partial results.** A range that failed or hit a byte cap leaves its scope unlisted; the other ranges are still applied.

### 5.3 Range listing and the cursor-range probe

`listRecords` has no range parameter, and cursors are opaque by spec. The reference PDS and rsky-pds both treat the cursor as an exclusive rkey bound, list in descending bytewise order, and return the last key as the cursor (ADR 0004). `list_range` relies on this when the host allows it, and checks it on every page.

```
list_range(did, c, fk) -> {rows, complete, capped, mode}
  N = (c = NS.opinion) ? 100000 : 20000                // data-model §7.4
  P = fk + "-";  B = fk + "."                          // "." (0x2E) is the successor of "-" (0x2D)
  h = host(PDS(did))
  if h.rangeCursor = No ∧ now − h.rcAt < 7 days: goto FULL
  RANGE:
  rows = []; cursor = B; prev = none; first = true; bytes = 0
  loop:
    r = listRecords(repo = did, collection = c, limit = 100, cursor = cursor)      // reverse unset
    if first ∧ r is HTTP 400: set No; goto FULL
    if r failed: return error (retried per §9.3)
    ks = rkeys of r.records (each uri MUST be at://did/c/<rkey> canonical, else the page fails)
    violation ⇔ ∃ k ∈ ks: k ≥ B
              ∨ ks is not strictly descending
              ∨ (prev ≠ none ∧ ks ≠ [] ∧ ks[0] ≥ prev)
              ∨ (ks ≠ [] ∧ r.cursor present ∧ r.cursor ≠ last(ks))
    if violation: set No; goto FULL                      // discard rows
    first = false
    for each record x in page order:
       if ¬rkey(x).startsWith(P): set Yes; return {rows, complete: true, capped: false, RANGE}
       rows += x
       if |rows| = N: set Yes; return {rows, complete: true, capped: true, RANGE}
    bytes += decompressed page bytes; if bytes > 50 MB: return {rows, complete: false, capped: false, RANGE}
    if ks = [] ∨ r.cursor absent ∨ (|ks| < 100 ∧ h.shortPageEnds): set Yes; return {rows, complete: true, capped: false, RANGE}
    prev = last(ks); cursor = r.cursor
  FULL:                                                  // whole collection, filtered by prefix
  all = []; seen = {}; cursor = none; pages = 0; bytes = 0
  loop:
    r = listRecords(repo = did, collection = c, limit = 100, cursor = cursor)
    pages += 1; bytes += decompressed page bytes
    all += { x ∈ r.records : rkey(x).startsWith(P) }
    if r.records = [] ∨ r.cursor absent: break
    if r.cursor ∈ seen ∨ pages > 5,000 ∨ bytes > 200 MB: return {top N of all, complete: false, capped: false, FULL}
    seen += r.cursor; cursor = r.cursor
  if two entries in all share an rkey: return error (the listing is inconsistent; retried)
  sort all by rkey descending
  return {first N of all, complete: true, capped: |all| ≥ N, FULL}

set Yes: if h.rangeCursor = Unknown: h.rangeCursor = Yes, h.rcAt = now
set No:  h.rangeCursor = No, h.rcAt = now
```

**Notes**
- The probe is the first page with `cursor = "{fk}."`. A host that ignores the bound, uses opaque cursors, rejects the cursor (400) or orders differently fails at least one check on its first page (ADR 0004). Every later page of every listing is checked again, and any violation switches the host to `No` and redoes that listing in `FULL` mode.
- A host that ignores the cursor but whose collection holds only keys below `B` returns exactly the range listing, so the result is still correct.
- `No` is re-probed after 7 days (*tunable*). `Yes` stays until a violation.
- **Short pages.** A page with fewer than 100 records ends a `RANGE` listing (both known implementations return SQL-limited pages). `h.shortPageEnds` is `true` by default; `lessdugin doctor` sets it to `false` for a host on which it observes a non-empty page after a short one, and listing then continues until an empty page or a missing cursor. `FULL` mode never uses the short-page rule.
- The membership key `{fk}` (no dash) is outside the range and is fetched with `getRecord` (§5.2). Neighbouring fks and `{fk}.…` keys fall outside it too.
- Ascending listing (`reverse=true&cursor={fk}`) behaves identically and MAY be used for oldest-first replays; it is never used for the normative fetch cap.

### 5.4 Normative fetch caps

`data-model.md` §7.4 is normative. Restated:

```
for each (repo, collection c ∈ RANGED, fk):
  N(c) = 100000 if c = NS.opinion, else 20000
  input(repo, c, fk) = the first N(c) keys of the "{fk}-" range in DESCENDING rkey byte order
```

- **Every key counts**, including keys of invalid records, so the boundary depends only on the repo's key set.
- `RANGE` mode stops after the N-th key; `FULL` mode sorts and truncates. Both give the same set.
- A capped range is **complete** for completeness (§4.5) and `repoStatus` (§5.9), because every client excludes the same records. It sets `repo.capped[fk]`, and the user card shows `[partial]`. A range of exactly N keys is reported as capped.
- Rows below the boundary are removed from the store without a tombstone (§5.2). `ForumModel` applies the cap again over stored rows (§13), so a point-fetched row beyond the boundary never enters the fold.
- **Byte caps are safety limits, not fold rules:** 50 MB of decompressed listing per `(repo, collection, fk)` range, 4 MB per `listRecords` page, 1 MB per record. A byte-capped range is not complete: it advances no `listed` entry and makes no completeness claim. Honest repos stay far below these caps (100,000 typical opinions are about 35 MB). An authority whose range hits a byte cap keeps the view `degraded` until it was once listed completely, which fails closed.

### 5.5 Admission checks

Every record returned by `getRecord` or `listRecords` passes these checks before it is stored:

1. **Shape.** The uri is canonical (`data-model.md` §2.3), its DID is the repo's, its collection is the requested one (or `app.bsky.actor.profile` for the fallback), and the `value` is a JSON object of at most 1,000,000 bytes in its response encoding.
2. **JSON → DAG-CBOR** (atproto data model):
   - objects become maps, with keys sorted by DAG-CBOR order (shorter first, then bytewise); duplicate keys are rejected;
   - an object whose only key is `$link` (a string CID) becomes a CID (tag 42); an object whose only key is `$bytes` (base64) becomes a byte string;
   - integers must be within ±(2^53 − 1); any number with a fraction or exponent is rejected (no floats);
   - nesting depth ≤ 32;
   - strings, booleans and null map directly.
3. **CID.** The returned `cid` parses as CIDv1, codec dag-cbor (`0x71`), multihash sha2-256, and equals the CID of the encoding. Comparison is on the binary CID.
4. On failure of 1–3: the record is dropped, logged with its source, and counted as a strike against the host. It is never stored.
5. **Validity.** `data-model.md` §7 runs on the admitted record. An invalid record is stored with `valid = false` and the first failing rule ID (raw view only).
6. **Level.** `pds` if the identity is `doc`; otherwise the row is stored with `idHint = true` (§1.5).

The conversion and CID rules are tested with the vendored atproto interop vectors (data model JSON ⇄ CBOR ⇄ CID).

### 5.6 Version order, tombstones, `absent` and `obs`

#### 5.6.1 Order key

Each admitted observation of a uri carries an **order key** `(revLo, sentAt)`:

| Observation | `revLo` | `sentAt` | exact |
|---|---|---|---|
| a row of a listing pass | `h1.rev` of the pass | send time of the page's request | if the pass was stable |
| `getRecord` (point fetch) | `repo.headSeen.rev` when the request was sent (a lower bound) | send time | no |
| own write (§5.8) | the response's `commit.rev` (else a fresh head's rev) | send time of the write | yes |
| proof from the author's PDS | the commit rev | send time of `sync.getRecord` | yes |
| current third-party proof | the commit rev | `sentAt` of the head observation that made it current | yes |

Keys compare by `revLo` (bytewise), then `sentAt`.

#### 5.6.2 Admission into the store

```
admit(u, o):                                  // o: an eligible-or-idHint observation (never a hint, never stale)
  t = tomb[u];  s = rec[u]
  if t ∧ key(o) < key(t): drop o                          // suppressed by a newer delete (I4)
  if s ∧ s.cid = o.cid:                                    // same version
     s.level = max(s.level, o.level); s.revLo = max; s.exact = s.exact ∨ o.exact
     s.firstSeen = min(s.firstSeen, o.firstSeen); write s if changed
  elif s = none ∨ key(o) > key(s): rec[u] = o; delete tomb[u]
  elif key(o) = key(s) ∧ o.exact ∧ s.exact:                // same rev, different CID (I3)
     keep the higher level; at equal level keep s; log (u, rev, both CIDs, both sources); enqueue(did, P1)
  else: drop o                                             // older
```

The store keeps one current version per uri. Old versions survive only as revision snapshots.

#### 5.6.3 Tombstones

A tombstone `tomb[u] = {revLo, sentAt}` records a confirmed delete. It is written only on:
- a **complete, stable listing** of u's range that does not contain u (key: the pass's `h1.rev`, and the send time of its first page);
- a `getRecord` answered `RecordNotFound` by the author's PDS (key as for a point fetch);
- an own delete (§5.8).

A tombstone removes `rec[u]`, suppresses every observation with a smaller key (I4), and is removed when a newer observation is admitted. Tombstones are kept at least 30 days (*tunable* upward only), then garbage-collected. Jetstream deletes and indexer `#change{op: delete}` are hints: they only schedule a `getRecord` (§6.4).

#### 5.6.4 `absent`

The fold's `absent` input (`social.md` §1.1) is computed per forum:

```
absent(F) = { u : u = v.subject.uri for some opinion v ∈ records,  no rec row exists for u,  and
                  ( tomb[u] exists
                  ∨ ( fkOf(rkey(u)) = fk(F) ∧ r = repo(did(u)) ∧ r.listed[fk(F)] ≠ ""
                      ∧ ( coll(u) ∉ r.colls
                        ∨ coll(u) ∉ keys(r.capped[fk(F)])
                        ∨ rkey(u) > r.capped[fk(F)][coll(u)] ) ) ) }        // above the cap boundary
```

So a subject counts as absent only if a delete was confirmed, or its range was completely listed (above the cap boundary) without it. An unlisted repo never yields `absent`, and neither does a stored row that is merely ineligible (an inactive or identity-hint author).

#### 5.6.5 `obs` (first-seen times)

- `firstSeen(u, cid)` is the earliest time (µs) at which this client observed that exact version from **any** source, hints included: a Jetstream commit event carrying `(u, cid)`, an indexer `#recordRef` or `#change`, or the admission itself. Hint times are kept in memory and applied when the version is admitted.
- In assisted mode, an indexer's `#recordRef.firstSeen` is also a candidate, and the minimum is taken.
- Each stored row keeps `firstSeen` for its current CID, plus `firstCid` and `firstSeenUri` for the earliest version ever admitted. The fold's `obs` therefore contains `(u, cid) → firstSeen` and `(u, firstCid) → firstSeenUri`, so `t_obs` is the record's first-seen time across edits (`social.md` §2.2).
- First-seen values are never moved later.

### 5.7 Point fetches

A point fetch is `getRecord(did, collection, rkey)` at the author's PDS, admitted at level `pds` with the point-fetch order key (§5.6.1). It is used for:
- Jetstream and indexer change events (§6.4);
- dangling strongRefs on screen (a comment's root or parent, an opinion subject being displayed);
- confirming candidates (§4.2.1) and resolving stale proofs (§1.2.1);
- the declaration at forum open (§4.2).

A point fetch never makes a range complete. It can satisfy a dirty mark (§6.5). `sync.getRecord` is used instead when a proof is wanted: `verify`, the first open of a declaration, and the indexer (§15).

### 5.8 Own writes

When the outbox gets a successful response (§7):
- each created or updated record is admitted with the exact value that was sent, at level `pds`, source `own`, if the CID of its DAG-CBOR encoding equals the result's `cid`. Otherwise it is not stored, and the uri gets a P0 point fetch;
- each deleted record gets a tombstone;
- the order key's `revLo` is `commit.rev`. If the response has no `commit` (a no-op `putRecord`, ADR 0005), the client calls `getLatestCommit` for it;
- `repo.headSeen` is updated, `repo.rev` and `repo.listed` are not, and the own repo is marked dirty at P1, so a listing confirms the write;
- a write into a collection not in `repo.colls` sets `repo.collsDirty`.

### 5.9 Repo state and `repoStatus`

**`repoStatus(d, fk)`**, the fold input (`social.md` §1.1):

```
latestKnown(d) = max(repo.headSeen.rev, max { m.rev : m ∈ repo.marks })     // marks: §6.5
repoStatus(d, fk) =
  Fresh        if repo.listed[fk] ≠ "" ∧ repo.listed[fk] ≥ latestKnown(d) ∧ the last listing hit no byte cap
  Stale        if repo.listed[fk] ≠ "" otherwise
  Unavailable  if repo.listed[fk] = "" (never completely listed)
```

It is defined for every DID that owns a record in `records` and for every authority; any other DID is `Unavailable`.

**Repo classes** (*tunable*), used by polling (§6.7):

| Class | Condition |
|---|---|
| `authority` | an authority of a forum that is open in a view |
| `hot` | `repo.lastChangeAt` (the last time a sync changed a stored row of this repo) ≤ 1 day ago |
| `warm` | ≤ 30 days ago |
| `cold` | otherwise |

**Rollback.** If `h1.rev < repo.rev`, nothing is applied and the cache is kept. The repo follows the repo backoff (§9.4) and `repo.rollbackSince` is set. After 24 h of continuous rollback, the author's items carry a warning "repository rolled back". `lessdugin sync --full <forum>` discards that repo's cached state and lists it again, which accepts the PDS state: the PDS is the database.

**Failures.** A failed sync sets `repo.fails += 1` and schedules the next attempt by the repo backoff (§9.4). Cached rows stay (unknown is not absent); items from a repo whose last attempt failed show `[stale]`.

### 5.10 Safety caps (`limits.md`)

| What | Limit |
|---|---|
| Record JSON | 1 MB |
| `listRecords` page | 4 MB, decompressed |
| Range, per `(repo, collection, fk)` | 50 MB, decompressed (§5.4) |
| `FULL` listing of one collection | 5,000 pages and 200 MB |
| Proof CAR (`sync.getRecord`) | 1 MB, ≤ 2,000 blocks |
| Indexer proof bundle (`getRecordProofs`) | 8 MB, ≤ 20,000 blocks |
| DID document, `.well-known/atproto-did`, protected-resource metadata | 65,536 / 2,048 / 65,536 bytes |
| Jetstream and WebSocket message | 1,200,000 bytes |
| Blob | 300 MB hard limit; 5 MB automatic load (images) |

Every response is byte-counted while streaming, after decompression; the host aborts at the cap (§12.1). Cursors must strictly advance (§5.3).

---

## 6. Live and incremental updates

### 6.1 The dirty queue

Every change source feeds one queue of items `(did, minRev?, priority, reasons, uris?)`:
- Jetstream (§6.2–§6.4);
- the indexer change feed (§6.6);
- polling (§6.7);
- reconcile (§6.8);
- own writes (§5.8);
- discovery (§4);
- opening a view (§6.7).

**Coalescing.** The queue holds at most one item per DID. Merging takes the highest priority (P0 highest), the greatest `minRev`, the union of reasons and uris, and the earliest ready time.

| Priority | Content | Ready after (debounce plus jitter, *tunable*) |
|---|---|---|
| P0 | content on screen | `uniform(0, 5 s)` |
| P1 | posts, comments, declarations, modActions, memberships, revisions, sequences, wikiRevisions | `uniform(2 s, 30 s)` |
| P2 | opinions | `uniform(30 s, 600 s)` |
| P3 | reconcile | its scheduled time |

Jitter spreads the load when many readers react to the same write. An item carrying `uris` and no other reason is served by point fetches (§5.7); any other item runs `sync_repo` for the DID's scopes. Ready items are dispatched by `(priority, ready time, did)` within the limits of §9.

### 6.2 Jetstream connections

Jetstream is used only in `assisted` mode. It is a **"this repo changed" signal**: its records and CIDs are hints and are never displayed. The leader (§10.6) holds the connections. The endpoints follow ADR 0003, which replaces the synthesis's endpoint line.

**Wires**

| Wire | URL | Notes |
|---|---|---|
| v2 (default) | `wss://<jetstream host>/xrpc/network.bsky.jetstream.subscribeEvents?collections=<NS>.*[&dids=<did>…][&kinds=commit]&cursor=<c>`, subprotocol `xrpc.v1.json` | every frame is `{"$type":"message","payload":{…}}`; payload types `#commit`, `#identity`, `#account`, `#sync`, `#info`; the cursor is `seq` (inclusive) or a µs timestamp (≥ 10^15) |
| v1 | `wss://<jetstream host>/subscribe?wantedCollections=<NS>.*&requireHello=true&cursor=<time_us>`, then `{"type":"options_update","payload":{"wantedCollections":["<NS>.*"],"wantedDids":[…],"maxMessageSizeBytes":1200000}}` | used only for DID filters of more than 150 DIDs |

**Modes**

| Mode | When | Connection(s) |
|---|---|---|
| `wildcard` | native, always; browser until participants are known | v2, `collections=<NS>.*`, no DID filter |
| `filtered` | browser, once participants are known and D holds between 1 and 10,000 DIDs | at most 150 DIDs: v2 with `dids=` for each DID in D; more than 150: v1 with `options_update{wantedDids: D}` (the request-line limit makes 400 DIDs fail with 414, ADR 0003). Plus, if `F_new ≠ ∅`, a side connection: v2 `collections=<NS>.membership&kinds=commit` with no DID filter. |

- `D` = the own account DIDs ∪, over every followed or open forum, its authorities, confirmed participants and candidates being synced.
- "Participants are known" means every followed or open forum has finished one discovery pass (§4.2). The browser then switches to `filtered`. When D changes it reconnects with the new set, at most once per 10 minutes (*tunable*), and DIDs newly added to D get a P2 `sync_repo` to cover the events they missed. If `|D| > 10,000`, the browser stays in `wildcard`.
- **Reserved (P2):** switching the native client to `filtered` automatically when lessdugin-wide traffic exceeds about 50 events/s.
- `<NS>.*` also matches `<NS>.temp.*` and `<NS>.index.*` collections; events are filtered by exact collection (§6.4).
- Compression: v1 MAY negotiate `permessage-deflate`. Byte caps apply after decompression.

**Endpoints and reconnect** (*tunable*)
- Connect to the first host in `network.jetstream`. Reconnect after any close or error with delay `min(60 s, 1 s · 2^(n−1)) · uniform(0.8, 1.2)` for the n-th consecutive failure. After 3 consecutive failures, fail over to the next host.
- A message over 1,200,000 bytes closes the connection. If two consecutive connections fail on oversize at the same cursor, the client resumes at `timeUs + 1 s` and schedules a full reconcile (§6.8) at P3.
- While no live source (Jetstream, or an indexer change feed) has been healthy for 2 minutes, polling (§6.7) runs.

### 6.3 Cursor handling

- **Persisted** in `cur/jetstream` (§10.4) every 5 s or every 1,000 events: `{wire, instance, seq?, timeUs}`. On v2, `seq` is the payload's `seq` and `timeUs` is `µs(witnessedAt)`; on v1, `timeUs` is `time_us`.
- **Resume.** Same instance and wire v2: `cursor = seq` (inclusive, so the first event repeats). Otherwise (another instance, or a change of wire): `cursor = timeUs − 5,000,000`.
- **Deduplication.** An LRU of 10,000 keys (*tunable*): `(did, rev, collection, rkey, operation)` for commits, `(did, kind, inner seq)` for identity and account events.
- **Gaps.** `#info OutdatedCursor`, or a persisted cursor older than 36 h, triggers a full reconcile (§6.8): every participant of every followed forum is enqueued at P3 and every authority at P1; the cursor restarts at `now − 5 s`. `#info FutureCursor` restarts the cursor at `now − 5 s`.
- **Replay at forum open** (§4.5 step 3, assisted only). When a forum is opened for the first time, the client MAY open a temporary v2 connection with `cursor = now − 36 h` and close it when it reaches the live cursor. It collects only the DIDs of commits in that forum's `{fk}-` ranges, as recently-active hints.

### 6.4 Event filtering and handling

```
F     = { fk(f) : f followed or open }
F_new = { fk(f) ∈ F : f is open-mode, or the viewer is an authority of f }      // newcomer and application discovery
KNOWN(did) ⇔ the cache has an identity row for did (§10.4)

on event e:
  unknown payload $type → log; ignore
  #commit:
    require e.collection ∈ OURS (exact match)
    relevant ⇔ ∃ f ∈ F: e.rkey = f ∨ e.rkey starts with f + "-"
             ∨ (e.collection = NS.forum ∧ (e.did, e.rkey) is a followed forum's declaration)
             ∨ (e.collection = NS.profile ∧ KNOWN(e.did))
             ∨ e.did is an own account
    if ¬relevant: drop
    if ¬KNOWN(e.did):
       if e.collection = NS.membership ∧ e.rkey ∈ F_new: newcomer candidate (§4.4); else drop
    else: dedup; record firstSeen(uri, e.cid) (§5.6.5); add a dirty mark (§6.5); handle per the table
  #identity, #account, #sync:
    if ¬KNOWN(e.did): drop                                      // network-wide noise (ADR 0003)
    else: handle per the table
  #info: §6.3
```

| Event | Action |
|---|---|
| commit create or update | point fetch of the uri. With an active indexer that advertises `getRecordProofs`, the uri joins a proof batch (≤ 25 uris, flushed after `uniform(1 s, 3 s)`); the proofs are then subject to the currency rule (§1.2), whose head checks are batched per author. Otherwise `getRecord` at the author's PDS when the item's priority makes it ready (§6.1). |
| commit delete | `getRecord` at the author's PDS: `RecordNotFound` → tombstone (§5.6.3); a record → admit it |
| commit on `NS.membership` with `rkey ∈ F` by a known DID | as above, and the candidate state is refreshed (§4.2.1) |
| identity | invalidate the handle and document caches (§3.3). Authorities are re-resolved at P1; others lazily. |
| account | `getRepoStatus` at P1 (§3.4) |
| sync (v2 `#sync`) | `enqueue(did, P1, "sync")` for every scope: a full re-list |

Events MAY also drive counts and placeholders ("3 new comments"), which never move the cursor in the TUI and never supply text.

### 6.5 Dirty marks

- A mark `(uri, rev)` records that a hint (Jetstream commit, indexer `#change`) reported a change of `uri` at `rev`. Marks are stored in `repo.marks` (at most 1,000 per repo; on overflow `repo.marksOverflow` is set and the repo is enqueued for a full sync at P1).
- A mark is **satisfied** and removed when:
  - a point fetch of the uri sent after the mark was added admits a version whose CID equals the hint's CID, or (for a delete) gets `RecordNotFound`; or
  - a stable listing pass with `h1.rev ≥ rev` completes (§5.2).
- Unsatisfied marks hold the repo below `Fresh` (§5.9). A mark that is still unsatisfied after 2 stable, complete listing passes that started after it was added is dropped and counted as an unconfirmed hint against its source. A listing at `h1.rev ≥ rev` always satisfies a mark, so this drops only marks whose rev the PDS never reached, such as a lying hint's.

### 6.6 Indexer change feed

With an active indexer that advertises `getChanges` (assisted mode):
- poll `getChanges(forum = F, since = cursor)` every 10 s while a view of F is open and every 60 s otherwise (*tunable*), per followed forum the indexer serves;
- each `#change` is handled like a Jetstream commit for that uri (§6.4), deduplicated with the same LRU;
- the cursor (`cur/indexer:<did>:<fk>`) is the returned `cursor`;
- `CursorTooOld`: run a full reconcile of F (§6.8) and restart from the `asOf.seq` of that response.

### 6.7 Polling

Polling runs in `direct` mode, and in `assisted` mode while no live source has been healthy for 2 minutes:

| Repo class (§5.9) | Interval, ±20% jitter (*tunable*) |
|---|---|
| `authority` | 2 min |
| `hot` | 10 min |
| `warm` | 1 h |
| `cold` | 12 h |

A poll is `getLatestCommit`. If the head differs from `(repo.rev, repo.commit)`, the repo is enqueued at P1 (P2 for `cold`).

**View open.** Opening a view triggers a P0 head check for every author on screen whose last head observation is more than 5 minutes old.

### 6.8 Reconcile

Reconcile always runs, whatever the sources:
- every participant (confirmed candidate) of every followed forum: one `sync_repo` per 24 h, with `describeRepo` refreshed, scheduled at `lastReconcile + 24 h · uniform(0.9, 1.1)` at P3;
- every authority of a followed forum: hourly, at P1;
- the account status of each such DID: daily (§3.4).

This bounds what a lossy Jetstream, an omitting indexer or an unseen writer can hide to about 24 hours.

---

## 7. Write path (outbox)

The record-level protocols are `data-model.md` §8 (edit, delete, opinion, moderation and blob protocols, the size check and the writer rules), refined by ADR 0005 where noted below. This section defines how the engine runs them.

### 7.1 Serialisation

- Each `PendingWrite` (`data-model.md` §8.2) is persisted in the `state` store before any network call.
- **One write in flight per account.** The pump for account `a` runs only while holding the lock `outbox:<a>` (§10.6): native `fd-lock` on `<data dir>/locks/outbox-<h>.lock`, browser Web Lock `lessdugin-outbox-<a>` (with `h` = the first 16 characters of `b32s(sha256(a))`). It processes the account's writes in `seq` order (FIFO). Different accounts run independently.
- In `lessdugin web`, the bridge takes the same native lock for each `/bridge/xrpc` write, so the CLI, the TUI and the bridged SPA share one serialisation.
- A browser tab that is not the sync leader persists its `PendingWrite` and notifies the leader over the BroadcastChannel (§10.6); whichever tab holds `outbox:<a>` pumps it.

### 7.2 The pump

```
pump(a):
  hold lock outbox:<a>
  loop:
    w = the first PendingWrite of a with state ∈ {queued, held, sending, throttled}, by seq
    if none: return
    if w.state = held ∧ now < w.heldUntil: sleep until then (new intents on the same subject coalesce into w); continue
    if w.state = sending: recover(w) (data-model §8.2: compare each target's current CID with the intended one)
    if the write budget (§7.3) would be exceeded: w.state = throttled until it fits; continue
    w.state = sending (persisted)
    r = run w's protocol (data-model §8.3–§8.8):
          every swapCommit is the cid of a getLatestCommit sent immediately before that request,
          never a cached head or an earlier write's result
    on success: apply the results to the cache (§5.8); w.state = done
    on InvalidSwap / swap failure: the protocol's own retry (≤ 3, uniform(200 ms, 800 ms)); then conflict or failed
    on 429: w.state = throttled until max(Retry-After, RateLimit-Reset)
    on HTTP 500 for a write: Ambiguous → re-read the targets (and the head) before deciding; never resend blindly (ADR 0005)
    persist w; emit Write{id, state}
```

**ADR 0005 refinements** (to be folded into `data-model.md` §8):
- **Create-if-absent** (opinions, modActions, memberships) uses `putRecord{rkey, swapRecord: null}`, whose `InvalidSwap` is a typed signal. On a host that answers a null swap with a 400 other than `InvalidSwap`, the client falls back to `createRecord(rkey)`, and on a 5xx calls `getRecord` to tell "exists" from "failed". The fallback is remembered per host.
- **First content in a forum** includes the membership create only when a fresh `getRecord` shows it absent.
- **No-op updates** are skipped: the client compares the new value's CID with the current one before sending.

**Deletes** are chunked with the target first (`data-model.md` §8.7): chunk 1 is `[delete target] ++` up to 199 of its revisions, later chunks hold up to 200 revisions, and the chunks are sent in order. A partial failure therefore leaves only orphaned revisions, which the persisted write keeps retrying.

### 7.3 Write budget and rate limits

- Per account, sliding windows of 5,000 points per hour and 35,000 points per day; create 3, update 2, delete 1, counting every op of an `applyWrites`. The record of spent points lives in `state` (`out.budget`, §10.4).
- 429 responses and the `RateLimit-*` headers of write responses are honoured as in §9.2.
- A write stamped with a different account DID than the session is refused (`data-model.md` W9).

### 7.4 Undo and display

- Opinion intents are held 4 s (`held`) so they can be undone or coalesced. `undo(write_id)` removes a held write; after it is sent, undo issues the inverse intent as a new write.
- Pending writes are shown only in the viewer's lens (L7, `social.md` §10.3): the viewer sees their own pending content and opinion values with `[pending]`. Aggregates, karma and other viewers' state change only when the write is confirmed and admitted (§5.8). The fold never sees pending writes.

### 7.5 Blobs

- Blobs are uploaded only when the write that references them is sent, in the same `PendingWrite` (`data-model.md` §8.8). The record embeds the blob object returned by `uploadBlob` verbatim. A local CID is for preview only and MUST equal the returned `ref.$link`.
- The declared `mimeType` sent to `uploadBlob` is normalised from the sniffed type (`data-model.md` §8.8), which keeps audio within `blob:audio/*`.
- On 413 the client downscales images or refuses. `InvalidMimeType`, `InvalidSize` and `BlobNotFound` on a write map to `MediaRejected` (ADR 0005).

---

## 8. Verification (`proofs/1`)

### 8.1 Interface

```
verify_proof(car: bytes, did, collection, rkey, key: PublicKey, now) ->
    Proven{rev, commitCid, cid, value} | ProvenAbsent{rev, commitCid} | Err(kind)
```

- `key` is the `#atproto` key of a DID document at level `doc` (§3.2). A key from a `hint` identity MUST NOT be used.
- `now` is an explicit argument, so vectors are reproducible.
- `Err` kinds: `CarMalformed`, `CarTooLarge`, `TooManyBlocks`, `BlockHashMismatch`, `MissingBlock`, `NonCanonical`, `BadCommit`, `DidMismatch`, `FutureRev`, `BadSignature`, `UnsupportedKey`, `MstInvalid`, `MstTooDeep`, `RecordMismatch`.

### 8.2 CAR parsing

lessdugin uses its own CARv1 reader:

1. **Caps** (checked before allocation; nothing is preallocated from a declared length): total size ≤ 1,000,000 bytes for a `sync.getRecord` CAR (≤ 8,000,000 for an indexer bundle, §8.6); ≤ 2,000 blocks (≤ 20,000 for a bundle). Exceeding a cap → `CarTooLarge` or `TooManyBlocks`.
2. **Header.** An unsigned LEB128 varint length (at most 9 bytes, minimal encoding, not exceeding the remaining bytes), then a DAG-CBOR map `{version: 1, roots: [CID]}` with exactly one root.
3. **Blocks.** Repeated until the end: a varint length L (minimal, `0 < L ≤` remaining), then a CID and data occupying exactly L bytes. The CID MUST be CIDv1 with codec dag-cbor (`0x71`) or raw (`0x55`) and multihash sha2-256 (`0x12`, length 32). `sha256(data)` MUST equal the multihash digest (`BlockHashMismatch`).
4. Duplicate blocks are allowed only with identical data. Trailing bytes that do not form a block → `CarMalformed`.

### 8.3 Strict DAG-CBOR

Commit, MST node and record blocks are decoded strictly; any violation is `NonCanonical`:
- definite lengths only; integers and lengths in minimal encoding;
- map keys are text strings, unique, in DAG-CBOR order (shorter first, then bytewise);
- tag 42 only, and only for CIDs (byte string with a leading `0x00`);
- no floats, no `undefined`, no other simple values than `true`, `false`, `null`;
- nesting depth ≤ 32.

### 8.4 Commit verification

```
commit = strict_decode(block(root))
require commit is a map with: did (text), version (int), data (CID), rev (text), sig (bytes);
        prev (CID or null) MAY be present; other keys are allowed and are covered by the signature
require commit.version = 3                                     else BadCommit
require commit.did = did                                       else DidMismatch
require commit.rev is TID syntax                               else BadCommit
require tidTime(commit.rev) ≤ now + 300,000,000 µs             else FutureRev
unsigned = canonical DAG-CBOR encoding of commit without the "sig" key
require |sig| = 64 (r ‖ s, big-endian)                         else BadSignature
require s ≤ n/2 for the key's curve (low-S; high-S is rejected) else BadSignature
require ECDSA-verify(key, sha256(unsigned), r, s)              else BadSignature
        // curve: secp256k1 for a 0xe7 multikey, NIST P-256 for 0x1200 (§3.2)
```

### 8.5 MST walk

**Keys.** The target key is `collection + "/" + rkey` as bytes. Every MST key MUST be at most 1,024 bytes, contain exactly one `/`, and use only `[A-Za-z0-9._:~-]` besides it; otherwise `MstInvalid`.

**Layer.** `layer(k)` = ⌊(number of leading zero bits of `sha256(k)`) / 2⌋.

**Node format** (atproto repository v3): `{l: CID | null, e: [{p: int, k: bytes, v: CID, t: CID | null}]}`.

```
walk(cid, target, depth, lo, hi, ceiling) -> Found(v) | Absent
  require depth ≤ 32                                         else MstTooDeep
  node = strict_decode(block(cid))                           // missing → MissingBlock
  keys: k_0 = e_0.k with e_0.p = 0;  k_i = k_{i−1}[0 .. e_i.p] ‖ e_i.k  with e_i.p ≤ |k_{i−1}|
  require every k_i is a valid MST key; k_0 < k_1 < … strictly (bytewise); lo < k_i < hi for all i
  if node has entries:
     λ = layer(k_0); require layer(k_i) = λ for all i; require λ < ceiling;  ceiling' = λ
  else:
     require node.l ≠ null unless depth = 0 (the root of an empty repo);   ceiling' = ceiling
  if ∃ i: k_i = target: return Found(e_i.v)
  j = |{ i : k_i < target }|
  (child, lo', hi') = j = 0 ? (node.l, lo, k_0 or hi) : (e_{j−1}.t, k_{j−1}, k_j or hi)
  if child = null: return Absent
  return walk(child, target, depth + 1, lo', hi', ceiling')

verify_proof(…):
  commit verified (§8.4)
  w = walk(commit.data, target, 0, −∞, +∞, +∞)
  Absent   → ProvenAbsent{rev: commit.rev, commitCid: root}
  Found(v) → require v is CIDv1 dag-cbor sha2-256; value = strict_decode(block(v)) (MissingBlock if absent)
             → Proven{rev: commit.rev, commitCid: root, cid: v, value}
```

- The interval and layer checks make absence proofs sound: a present key would have to lie on this search path.
- Only the blocks on the search path are needed. Unused blocks in the CAR are ignored.
- The caller checks validity (`data-model.md` §7) of `value` separately.
- `jacquard-repo` and `@atproto/repo` are test oracles. Vectors cover tampered blocks, wrong DIDs, high-S signatures, future revs, unsorted keys, wrong layers, depth overflow and absence proofs.

### 8.6 Indexer proof bundles

`getRecordProofs(uris ≤ 25)` returns `application/vnd.ipld.car` (`data-model.md` §11.3):
- the root block is a DAG-CBOR manifest `{v: 1, items: [{uri, cid: CID | null, commit: CID}]}`, followed by deduplicated blocks;
- the manifest's `v` MUST be 1; unknown keys are ignored; `items` ≤ 25.

```
for each item (independently):
  key = #atproto key of did(item.uri) from a doc fetched ≤ 1 h ago (§1.1)
  r = verify_proof over the bundle's blocks with root = item.commit for (did, coll, rkey) of item.uri
  item.cid ≠ null: require r = Proven with r.cid = item.cid
  item.cid = null: require r = ProvenAbsent
  pass → a third-party proof (§1.2): current or [stale]
  fail → drop the item; count a failure against the indexer (§15.4)
```

A `ProvenAbsent` that is current produces a tombstone (key `(rev, sentAt of the head check)`). A stale one is ignored.

### 8.7 Key rotation (`stale-proof`)

If a proof fails `BadSignature` against the current key, and its rev is lower than the author's current PDS head:
- it is classified `stale-proof` (the key rotated after it was signed, for example after a PDS migration);
- a fresh proof is fetched from the author's PDS (`sync.getRecord`);
- it is **not** counted against the source.

A `BadSignature` at a rev ≥ the PDS head is a real failure and counts against the source. Verification against historical keys from the PLC audit log is **Reserved (P7)**.

### 8.8 `lessdugin verify <ref> [--deep]`

```
verify(uri, deep) -> VerifyReport
  doc = DID document of did(uri), fetched now (level doc)
  car = sync.getRecord(PDS, did, coll, rkey) → verify_proof
  report { uri, result: Proven{rev, cid} | ProvenAbsent{rev} | Failed{kind},
           key: {method: "plc" | "web", id: "#atproto", type: "k256" | "p256"},
           local: {cid, level} if stored, agrees: local.cid = result.cid }
  if deep:
    for each strongRef in the value (comment.root, comment.parent, opinion.subject, revision.subject,
                                     wikiRevision.parents[], modAction.adopts, #tag.head):
        verify(ref.uri, false); report whether its current cid = ref.cid ("on an earlier version" otherwise)
    revision: require cid(DAG-CBOR(record)) = subject.cid (data-model RV5)
    for each blob ref in the value (media[].blob, textBlob, icon, avatar, thumb, cover.blob):
        fetch_blob (§11.1) and report the CID check
```

Proof verification never changes the store except to raise the level of a matching stored `(uri, cid)` to `proven`.

**Equivocation** (F15: recording `(did, rev, commit cid)` and comparing with Jetstream commits and indexer heads): **Reserved (P2)**. In v1, conflicting exact observations are only logged (§5.6.2).

---

## 9. Scheduler and budgets

### 9.1 Concurrency (*tunable*)

| Limit | Native | Browser |
|---|---|---|
| Requests in flight, global | 32 | 16 |
| Per host | 6 | 6 |
| PLC | 8 (and §3.5) | 8 (and §3.5) |
| DoH | 8 | 8 |

A request is dispatched when every applicable limit has room, its host's breaker allows it (§9.4) and its host budget allows it (§9.2). Waiting requests are served by priority (P0 first), then enqueue time. Outbox writes count as P0.

### 9.2 Per-host read budgets

Budgets are **per host** (ADR 0004).

```
on a response from host h carrying RateLimit-Limit L, RateLimit-Remaining R, RateLimit-Reset T:
   h.limit = L; h.est = R; h.resetAt = (T > 10^9 ? T seconds since the epoch : now + T seconds)
on sending a request to h: h.est −= 1
when now ≥ h.resetAt: h.est = h.limit
without headers: hosts matching *.bsky.network or bsky.social use the static budget 3,000 per 300 s;
                 other hosts have no budget (concurrency and 429 handling only)
dispatch to h allowed ⇔ h has no budget ∨ (priority = P0 ? h.est > h.limit / 5 : h.est > h.limit / 2)
on 429: h.est = 0 until max(Retry-After, h.resetAt)
```

Background work may thus use at most 50% of a host's window, and P0 work up to 80%. The headers are CORS-exposed by Bluesky PDSes, so the SPA reads them too.

### 9.3 Retries and timeouts (*tunable*)

- **GET retries:** 3 retries on network errors, timeouts, 5xx and 429, with full-jitter exponential backoff: `delay = max(uniform(0, min(8 s, 250 ms · 2^attempt)), Retry-After)`. Writes are never retried by the transport; the outbox protocols decide (§7.2).
- **Timeouts:** connect 5 s; request 20 s; CAR responses 60 s; blobs 20 s without progress and 300 s in total.

### 9.4 Breakers and repo backoff (*tunable*)

- **Circuit breaker per host:** opens after 10 consecutive failures (network error, timeout, 5xx). It stays open 60 s, then lets one half-open probe through. A failed probe doubles the open time, up to 1 h; a success closes it.
- **Repo backoff:** after a failed `sync_repo`, `repo.nextCheck = now + min(30 s · 2^(fails−1), 24 h) · uniform(0.8, 1.2)`. A success resets `fails`.

### 9.5 Request headers

- Cross-origin GETs from the browser carry no custom headers, so there are no CORS preflights. (`accept: application/dns-json` is CORS-safelisted.)
- Native requests send `User-Agent: lessdugin/<version> (+https://lessdugin.7778777.online)`.

### 9.6 User-visible budgets

- CLI: `--time-budget` (default 10 s for interactive commands), `--max-requests`, `--offline` (§4.5).
- SPA: before a cold start, the engine estimates 8 requests per candidate repo (§14.1). Above about 1,500 it offers the forum's declared indexer (§2.4), if any, and shows the estimate.

---

## 10. Cache and store

### 10.1 Stores

| Store | Durability | Contents |
|---|---|---|
| `cache` | disposable | records, tombstones, repo state, identities, discovery state, blob metadata, cursors, host capabilities, forum access times |
| `state` | durable | outbox, write budget, drafts, read marks, subscriptions, follows and unfollows, bookmarks, mutes, settings, consents, auth-server pins, notification watermarks and snapshots, quarantine list, schema version |
| sessions | secret | native: the OS keyring; browser: IndexedDB with non-extractable keys |

Native paths: `~/.cache/lessdugin/{cache.sqlite, blobs/}`, `~/.local/share/lessdugin/{state.sqlite, drafts/, locks/}`. Browser: IndexedDB databases `lessdugin-cache-v<N>` and `lessdugin-state`.

### 10.2 Invariants

| # | Invariant |
|---|---|
| I1 | Deleting `cache` loses only time. Nothing in `cache` is needed to recover `state`. |
| I2 | Every stored record and blob hashes to its CID; stored bytes are immutable. |
| I3 | Per uri, the current version only moves forward by order key (§5.6.1). The same exact rev with a different CID is a conflict: keep the higher level, log the source (§5.6.2). |
| I4 | A tombstone with key `t` suppresses every observation with a smaller key, and is kept at least 30 days. |
| I5 | The record store holds only `pds` and `proven` versions (and `idHint` rows, marked). Hints, `[stale]` proofs and pending writes live only in memory or in the outbox, and are never persisted as records. For the same `(uri, cid)`, the level only increases. |
| I6 | A completeness claim requires every repo in the completeness set to be `Fresh` for that forum (§4.5, §5.9); otherwise the view shows `[partial k/n]`. |
| I7 | Moderation inputs are read only from the record store, through the eligibility filter (§1.5). |
| I8 | Records of DIDs that are not active (§3.4) are excluded from every fold input. |
| I9 | One background-sync leader per cache (§10.6). Other processes and tabs do only P0 fetches, under per-repo locks. All writes are idempotent and order-key-monotonic (§5.6.2). Changes are broadcast (§10.6). |

### 10.3 Key encoding

Keys are tuples, encoded so that byte order equals tuple order:

| Element type | Encoding |
|---|---|
| string | `0x02`, then the UTF-8 bytes with every `0x00` written as `0x00 0xFF`, then `0x00` |
| bytes | `0x01`, then the bytes escaped as for strings, then `0x00` |
| integer (i64) | `0x15`, then 8 bytes big-endian of `value XOR 0x8000000000000000` (sign bit flipped) |

The key of a tuple is the concatenation of its elements. A prefix scan over `(e1, …, ek)` covers every key that starts with their encodings. Within one position every key uses the same element type.

### 10.4 Key schema

| Store | Key | Value | Notes |
|---|---|---|---|
| cache | `("rec", scope, coll, did, rkey)` | `RecordRow` | `scope = rkey[0..12]` if `coll` ∈ `RANGED ∪ {NS.membership}` and `rkey` matches `^FK$` or `^FK-`; otherwise `"_"`. A forum is one prefix scan `("rec", fk)`. There is no secondary index. |
| cache | `("tomb", scope, coll, did, rkey)` | `TombRow` | same scope rule |
| cache | `("repo", did)` | `RepoRow` | |
| cache | `("id", "did", did)` | `IdentityRow` | |
| cache | `("id", "handle", handle)` | `HandleRow` | |
| cache | `("disc", fk, did)` | `DiscRow` | discovery candidates; not records |
| cache | `("blob", cid)` | `BlobRow` | the bytes live in the host's blob storage (§12.2) |
| cache | `("cur", source)` | `CursorRow` | `source` = `"jetstream"` or `"indexer:" + did + ":" + fk` |
| cache | `("host", origin)` | `HostRow` | |
| cache | `("forum", fk)` | `ForumRow` | `{uri, lastViewed}` for eviction |
| cache | `("meta", "schema")` | integer | a mismatch drops the whole cache (I1) |
| state | `("out", account, seq)` | `PendingWrite` | `seq` is an i64 increasing per account |
| state | `("out.budget", account)` | `BudgetRow` | spent write points with timestamps, for the two sliding windows |
| state | `("loc", kind, key…)` | per kind | kinds: `draft`, `read`, `sub`, `follow`, `unfollow`, `bookmark`, `mute`, `setting`, `alias`, `last_listing`, `consent`, `media_host`, `as_pin`, `quarantine` |
| state | `("notif", account, ts, id)` | notification | |
| state | `("meta", "schema")` | integer | `state` is migrated, never dropped |

### 10.5 Row schemas

Values are CBOR maps (serde field names below), each with a version field `v: 1`.

```
RecordRow  { cid: CID, cbor: bytes (DAG-CBOR of the value), level: "pds" | "proven", rev?: TID (proven),
             revLo: TID | "", sentAt: ms, exact: bool, src: "list" | "get" | "proof" | "own" | "indexer:<did>",
             idHint: bool, valid: bool, rule?: string, firstSeen: µs, firstCid: CID, firstSeenUri: µs, fetchedAt: ms }
TombRow    { revLo: TID | "", sentAt: ms, at: ms }
RepoRow    { pds: url, rev: TID | "", commit?: CID, headSeen?: {rev, cid, sentAt},
             colls: [nsid], collsAt: ms, collsDirty: bool,
             listed: {scope → TID}, capped: {scope → {nsid → boundary rkey}}, byteCapped: {scope → bool},
             marks: {uri → TID} (≤ 1000), marksOverflow: bool,
             checkedAt, lastOk, nextCheck: ms, fails: int, unstable: int, class: string,
             status: "active" | "inactive" | "temporary", statusDetail?: string, statusAt: ms,
             rollbackSince?: ms, lastChangeAt: ms, lastReconcile: ms }
IdentityRow{ doc: bytes (JSON), level: "doc" | "hint", src: "plc" | "web" | "slingshot" | "devnet",
             pds?: url, key?: string (multikey), aka?: handle, hstate: "verified" | "unverified" | "invalid",
             hAt: ms, docAt: ms }
HandleRow  { result: "did" | "none" | "invalid", did?: did, method: "doh" | "system" | "wellknown" | "hint" | "devnet", at: ms }
DiscRow    { src: [string], first: ms, state: "candidate" | "confirmed" | "rejected", at: ms }
BlobRow    { did, mime: string (declared), sniff?: string, size: int, stored: bool, refs: int, at: ms, used: ms }
CursorRow  { wire?: "v1" | "v2", instance?: host, seq?: int, timeUs?: µs, at: ms }
HostRow    { rangeCursor: "unknown" | "yes" | "no", rcAt: ms, shortPageEnds: bool, nullSwap?: bool,
             breaker: {state, failures, until, openMs}, budget?: {limit, est, resetAt}, strikes: int }
```

### 10.6 Leader, locks and change notification

| Lock | Native | Browser | Holder does |
|---|---|---|---|
| sync leader | exclusive `fd-lock` on `<cache dir>/cache.lock`, tried without waiting | Web Lock `lessdugin-sync-leader`, requested once and held for the tab's lifetime | live sources, polling, reconcile, discovery, P1–P3 sync, eviction |
| repo `d` | `fd-lock` on `<cache dir>/locks/repo-<h(d)>.lock` | Web Lock `lessdugin-repo-<d>` | the read-diff-write of `sync_repo`, point fetches and admissions for d |
| outbox `a` | `fd-lock` on `<data dir>/locks/outbox-<h(a)>.lock` | Web Lock `lessdugin-outbox-<a>` | the pump for account a (§7.1) |

`h(x)` = the first 16 characters of `b32s(sha256(UTF8(x)))` (`data-model.md` §3.1).

- Processes and tabs that are not the leader read the cache and do only P0 fetches, each under the repo lock.
- **Change notification.** Browser: every committed batch's `Changed{topics}` is posted on BroadcastChannel `lessdugin-changes`. Native: followers (for example a TUI next to a CLI) poll SQLite `PRAGMA data_version` every 2 s and rescan. This is a host concern and not part of the trait.
- When the leader exits, another process or tab takes the lock and resumes from the persisted cursors and queue state.

### 10.7 Eviction

- **Budget** (*tunable*): 2 GB native, 500 MB browser, measured as SQLite page count × page size plus the blob directory (native) or `navigator.storage.estimate().usage` (browser).
- Above the budget, whole forums are evicted, least recently viewed (`ForumRow.lastViewed`) first, until usage is below 80% of the budget. Evicting forum `fk` deletes `("rec", fk, …)`, `("tomb", fk, …)` and `("disc", fk, …)`, resets `listed[fk]` in every repo row, and drops blob references held by the deleted rows.
- **Never evicted:** identity rows; the user's own repo rows; the repos of authorities of followed forums. Evicting a forum skips their `(fk, coll, did)` sub-ranges.
- Blobs whose `refs` falls to 0 are deleted with `Host::blob_delete` (§11.4).
- The SPA calls `navigator.storage.persist()` at startup.

---

## 11. Blobs and media display

### 11.1 Fetch and verify

```
fetch_media(did, blobRef, policy):
  require identity(did) is doc (else click-through only, never automatic)
  size = blobRef.size; cid = blobRef.ref.$link
  require cid is CIDv1, codec raw (0x55), sha2-256                       // else "unsupported media"
  max  = min(300 MB, policy limit)                                        // images: 5 MB for automatic load
  if size > max: click-to-load (or refused above 300 MB)
  h = Host::fetch_blob({pds: PDS(did), did, cid, max_bytes: max, …})      // streamed and hashed by the host
     CidMismatch → discard; strike against the host; show "media failed verification"
     HTTP 400 "Blob not found" → missing: not retried; purge any cached copy (ADR 0005)
  sniff = sniff_type(Host::blob_read(h.cid, 0, 4096))
  require normalise(declared blobRef.mimeType) = normalise(sniff) ∧ that type is on the display allowlist (§11.2)
          else: download card only
```

- **Streaming** (completeness fix P0-4). Blob bytes never pass through wasm memory in bulk. The JS host streams the response with a byte cap, keeps the bytes in JS (an `ArrayBuffer` or `Blob`) and checks them with `crypto.subtle.digest('SHA-256')` (an incremental JS SHA-256 MAY be used instead). The native host streams to a temporary file while hashing with sha256, then renames it to its final path. The core only handles `BlobHandle`s and reads at most 1 MiB at a time through `blob_read` (enough for sniffing and for `textBlob` bodies of at most 1 MB).
- `normalise` is the MIME table of `data-model.md` §8.8 (for example `audio/x-m4a` and the M4A brand both give `audio/mp4`; `application/ogg` and `OggS` both give `audio/ogg`).
- `textBlob` bodies use the same path with `max_bytes = 1,000,000`, then the decoding rules of `data-model.md` §5.2.

### 11.2 Rendering

| Normalised type | Display |
|---|---|
| `image/png`, `image/jpeg`, `image/gif`, `image/webp` | inline image |
| `image/avif` | inline in the SPA only; elsewhere a download card |
| `video/mp4`, `video/webm`, `audio/mp4`, `audio/mpeg`, `audio/ogg` | click to load: full download, verify, then play |
| `image/svg+xml` | never inline in v1: download card |
| anything else, including `application/pdf` | download card; saved as `<cid>.<ext>` with the extension from the sniffed type |

- **Browser:** `new Blob([bytes], {type: T})`, where T is the constant string of the allowlist entry (never the declared string), then `URL.createObjectURL`, revoked when the element unmounts. CSP allows only `img-src 'self' blob:` and `media-src blob:`.
- **Native:** files at `<cache dir>/blobs/<cid[−2..]>/<cid>` (the last two characters of the CID string shard the directory). The TUI draws images only under `lfm-1.md` §9.4.
- Video and audio streaming with verification on completion (Service Worker) is **Reserved (P7)**. Thumbnails are **Reserved (P2)**.

### 11.3 Autoload policy (F29)

Media loads automatically only if all of these hold:
1. the author is in S ∪ E of the forum (`social.md` §3.1);
2. the item's visibility is `Visible`, or `Collapsed(*)` and expanded by the user;
3. the item has no `sexual`, `nudity`, `porn` or `graphic-media` self-label; for a comment, the root post's labels also count;
4. the PDS host is trusted: it serves at least 5 participants of the forum (by the PDS in their identity rows), or matches `*.host.bsky.network`, or is in `media.hosts`;
5. it is an inline image type (§11.2) of at most 5 MB.

Otherwise the user clicks through a notice: "loads from <host>, which will see your IP". Avatars and forum icons follow the same rules, with the profile or declaration as the item.

### 11.4 Caching and purging

- Hidden, provisional and pending content is never prefetched.
- `BlobRow.refs` counts the stored, eligible records that reference the CID and are not `Hidden(*)` in the current fold (the ForumModel updates it). When an item becomes hidden or is deleted, its blobs lose a reference; at 0 the bytes are deleted.
- Blobs are also deleted with their forum on eviction (§10.7), and when `getBlob` reports them missing.

---

## 12. `Host` and `KvStore` contract

The core never spawns tasks, reads a clock or does I/O except through these traits. Long-lived futures (`live_tail`, `outbox_pump`, `background_verify`) are returned to the host, which runs them with `tokio::spawn` (native) or `spawn_local` (wasm). Native implementations: `NativeHost` + `SqliteKv`; browser: `JsHost` (in a Web Worker) + IndexedDB; tests: `MemHost`.

### 12.1 HTTP and WebSocket

```rust
#[cfg(not(target_arch = "wasm32"))] pub trait MaybeSend: Send {}       // blanket impls
#[cfg(target_arch = "wasm32")]      pub trait MaybeSend {}
#[cfg(not(target_arch = "wasm32"))] pub trait MaybeSendSync: Send + Sync {}
#[cfg(target_arch = "wasm32")]      pub trait MaybeSendSync {}

pub enum Target {
    Anon(url::Url),                                     // unauthenticated
    UserPds { account: Did, path_and_query: String },   // the host adds auth for the PDS bound at login
}
pub enum Redirects { None, Follow(u8) }                 // UserPds is always None
pub struct HttpRequest {
    pub method: http::Method, pub target: Target, pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>, pub max_bytes: u64, pub timeout_ms: u32, pub redirects: Redirects,
}
pub struct HttpResponse {
    pub status: u16, pub headers: Vec<(String, String)>, pub body: Vec<u8>,
    pub sent_at_ms: i64,                                 // when the request was sent (order keys, §5.6.1)
}
pub enum HostError { Network, Timeout, TooLarge, Tls, Blocked, Cors, Aborted, Unsupported, Other(String) }
pub enum WsMsg { Text(String), Binary(Vec<u8>) }
pub trait WsStream: MaybeSend {
    fn recv(&mut self) -> impl Future<Output = Option<Result<WsMsg, HostError>>> + MaybeSend;
    fn send_text(&mut self, s: String) -> impl Future<Output = Result<(), HostError>> + MaybeSend;
    fn close(self);
}
```

**Obligations**
- `max_bytes` is enforced while streaming, after decompression; exceeding it aborts with `TooLarge`.
- `UserPds` requests go only to the PDS bound at login, carry DPoP or Bearer authentication added by the host (natively behind the OAuth facade; in the browser through the AuthBridge, which allowlists `com.atproto.repo.{createRecord, putRecord, deleteRecord, applyWrites, uploadBlob}` on NS collections plus `com.atproto.server.getSession`), and never follow redirects. Tokens and DPoP proofs are never logged.
- `Anon` requests follow at most the given number of redirects, `https` only (`http` only in devnet mode). Natively, an SSRF guard checks every hop: the resolved IP is pinned, and loopback, private, link-local, CGNAT (`100.64/10`), ULA (`fc00::/7`), `169.254/16` and IPv4-mapped addresses are blocked (`Blocked`), except loopback in devnet mode.
- In the browser, a failed cross-origin fetch is reported as `Cors` when it cannot be told apart from a network error.
- WebSocket messages larger than `max_msg` close the stream with `TooLarge`.

### 12.2 Blobs

```rust
pub struct BlobRequest { pub pds: url::Url, pub did: Did, pub cid: Cid, pub max_bytes: u64,
                         pub idle_timeout_ms: u32, pub total_timeout_ms: u32 }
pub struct BlobHandle  { pub cid: Cid, pub size: u64 }
pub enum BlobError     { Host(HostError), Status(u16, Option<String>), TooLarge, CidMismatch, UnsupportedCid }
```

- `fetch_blob` sends `GET <pds>/xrpc/com.atproto.sync.getBlob?did=<did>&cid=<cid>` (Anon, no redirects), streams the body, counts bytes against `max_bytes`, hashes it with sha256, compares the digest with the raw-codec CID, and only on a match stores the bytes under the CID in the host's blob storage and returns a handle. On any failure nothing is stored. An already stored CID returns its handle without a request.
- `blob_open` returns the handle of a stored CID; `blob_read` returns at most 1,048,576 bytes from an offset; `blob_delete` removes the stored bytes.
- The browser UI obtains object URLs from the host's blob storage by CID, outside the core.

### 12.3 The `Host` trait

```rust
pub trait Host: MaybeSendSync + 'static {
    type Kv: KvStore;
    type Ws: WsStream;
    type Lock: MaybeSend;                                // dropping it releases the lock
    fn cache(&self) -> &Self::Kv;
    fn state(&self) -> &Self::Kv;
    fn http(&self, r: HttpRequest) -> impl Future<Output = Result<HttpResponse, HostError>> + MaybeSend;
    fn fetch_blob(&self, r: BlobRequest) -> impl Future<Output = Result<BlobHandle, BlobError>> + MaybeSend;
    fn blob_open(&self, cid: &Cid) -> impl Future<Output = Result<Option<BlobHandle>, HostError>> + MaybeSend;
    fn blob_read(&self, cid: &Cid, offset: u64, len: u32) -> impl Future<Output = Result<Vec<u8>, HostError>> + MaybeSend;
    fn blob_delete(&self, cid: &Cid) -> impl Future<Output = Result<(), HostError>> + MaybeSend;
    fn websocket(&self, url: &str, protocols: &[&str], max_msg: u32)
        -> impl Future<Output = Result<Self::Ws, HostError>> + MaybeSend;
    fn resolve_txt_system(&self, name: &str)
        -> impl Future<Output = Result<Option<Vec<String>>, HostError>> + MaybeSend;   // Ok(None) on wasm
    fn lock(&self, name: &str, wait: bool)
        -> impl Future<Output = Result<Option<Self::Lock>, HostError>> + MaybeSend;    // None: busy and !wait
    fn sleep(&self, d: Duration) -> impl Future<Output = ()> + MaybeSend;
    fn now_ms(&self) -> i64;
    fn fill_random(&self, b: &mut [u8]);
}
```

- Lock names are `sync-leader`, `repo:<did>` and `outbox:<did>`; the host maps them as in §10.6.
- CI checks that every engine future is `Send` natively (`assert_send`) and that no mutex guard is held across `.await`. `clippy.toml` bans `SystemTime::now`, `Instant::now`, `thread::sleep` and `Utc::now` in the wasm-safe crates.

### 12.4 `KvStore`

```rust
pub enum Dir { Fwd, Rev }
pub struct KeyRange { pub start: Vec<u8>, pub end: Vec<u8> }            // [start, end)
pub enum KvOp { Put(Vec<u8>, Vec<u8>), Delete(Vec<u8>), DeleteRange(Vec<u8>, Vec<u8>) }   // ranges are [a, b)
pub trait KvStore: MaybeSendSync {                                      // IndexedDB-compatible: no interactive transactions
    fn get_many(&self, keys: Vec<Vec<u8>>) -> impl Future<Output = Result<Vec<Option<Vec<u8>>>, KvError>> + MaybeSend;
    fn scan(&self, r: KeyRange, d: Dir, limit: u32) -> impl Future<Output = Result<Vec<(Vec<u8>, Vec<u8>)>, KvError>> + MaybeSend;
    fn write(&self, batch: Vec<KvOp>) -> impl Future<Output = Result<(), KvError>> + MaybeSend;
}
```

**Contract**
- Keys compare as unsigned bytes. Keys are at most 2,048 bytes and values at most 2 MiB.
- `get_many` returns one entry per requested key, in request order.
- `scan` returns at most `limit` (≤ 10,000) pairs in range, ascending for `Fwd` and descending for `Rev`. Callers page by continuing from the last key.
- `write` is atomic (all or nothing) and applies its ops in order. After it resolves, reads in the same process observe it.
- Durability: `state` writes are durable when `write` resolves (SQLite `synchronous=FULL`; IndexedDB durability `strict`). `cache` writes MAY be relaxed (`synchronous=NORMAL`; `relaxed`).
- Native: one table `kv(k BLOB PRIMARY KEY, v BLOB) WITHOUT ROWID` per store, WAL mode, `busy_timeout`; each batch is one transaction. Browser: one object store with `ArrayBuffer` keys per database; each batch is one `readwrite` transaction; `Rev` scans use a `prev` cursor.
- Every implementation passes the shared store-invariant tests in `lessdugin-testkit`.

---

## 13. `ForumModel`

The `ForumModel` is the in-memory model of one forum. It builds the fold input and caches skeletons.

```
build(F):
  fk   = fk(F)
  rows = scan(cache, ("rec", fk))                                       // one prefix scan
       ∪ the declaration row ("rec", "_", NS.forum, owner, slug)
       ∪ profile rows ("rec", "_", NS.profile | app.bsky.actor.profile, d, "self") for displayed DIDs
  caps: for each (did, coll ∈ RANGED): keep the first N(coll) rows by rkey descending (§5.4)
  FoldInput (social.md §1.1):
    decl       = the declaration row, if eligible and valid
    records    = { uri → (cid, value) : row eligible (§1.5), membership rows with rkey fk, and RANGED rows after caps }
    absent     = §5.6.4
    obs        = { (uri, cid) → firstSeen, (uri, firstCid) → firstSeenUri }
    repoStatus = { d → repoStatus(d, fk) (§5.9) : d owns a row in records, or d ∈ A }
    now        = injected (tests, parity) or Host::now_ms() · 1000
```

- **Skeletons.** For each record the model keeps what the fold and list views need: uri, cid, author, collection, rkey, references (forum, root, parent, subject, adopts, head, parents, chapter entries), opinion values, modAction fields, kind, title, tags, self-labels, `createdAt`, `updatedAt`, TID time, `obs`, validity and rule ID. Bodies are dropped and re-read with `get_many` when a stage or a view needs them (quote and react anchors, rendering, search indexing), then released.
- **Incremental updates** on `Changed` events (own commits and broadcasts) re-run the affected layers. Incremental results MUST be property-tested equal to the batch fold (`social.md` §1.3).
- **Provisional items** (§1.5) are evaluated against the model's current result and never enter `records`.
- **Search.** Each client builds the core's in-memory inverted index per forum from titles and the plain-text projection P (`lfm-1.md` §10) of listed items. SQLite fts5 is used only inside the indexer (§15).
- **Rendering.** Render output is cached in an in-memory LRU keyed by `(cid, RENDERER_VERSION)`.
- The model also keeps `BlobRow.refs` current (§11.4) and records `ForumRow.lastViewed`.

---

## 14. Cost model and indexer thresholds

### 14.1 Assumptions (informative)

Per participant per year: about 80 records and 40 KB (1.5 posts, 15 comments, 60 opinions). A cold sync costs about 8 requests per participant: PLC, two `getLatestCommit` (§5.2), `describeRepo`, about 3 range listings (one each, thanks to the short-page rule, for a participant of one forum) and the membership `getRecord`. DoH lookups happen only for authors on screen. The synthesis's estimate of 6 per participant omitted the head re-check; the request column below uses 8, and the other columns are unchanged.

| Participants | Cold requests | Wire | Native cold | Browser cold | Warm, assisted | Direct polling |
|---|---|---|---|---|---|---|
| 50 | ~400 | ~2.5 MB | ~2 s | ~4 s | one point fetch per write, plus 50 reconciles/day | ~80 req/h |
| 500 | ~4,000 | ~25 MB | ~15 s | ~40 s | writes + 500/day | ~1.4k req/h |
| 5,000 | ~40,000 | ~250 MB | 3–50 min (PLC pacing dominates) | indexer needed | writes + 5k/day | not recommended |
| 50,000 | ~400,000 | ~2.5 GB | indexer required | indexer required | n/a | n/a |

Read budgets are per host (ADR 0004), so a cold sync spread over many Bluesky PDS hosts is bounded by per-host concurrency and latency. A single host holding more than 1,500 participants uses half its 3,000-request window.

**Fan-out at the author's PDS.** Without an indexer, every write causes about one point fetch per online reader at the author's PDS. With an indexer, readers fetch proofs from it instead; because of the currency rule (§1.2) each reader still sends one small `getLatestCommit` per changed author to that PDS, batched per author, but no record bytes.

### 14.2 Thresholds

| Size | Guidance |
|---|---|
| ≤ 300 participants | serverless everywhere |
| 300–3,000 | native is fine; the forum SHOULD declare an indexer for SPA first visits |
| > 3,000 participants, or > 10k DIDs, or > 300 concurrent readers, or > 1M records | the forum SHOULD declare at least one indexer (preferably two); the SPA defaults to the indexer for cold starts (after consent); direct mode stays available for audit |

### 14.3 Cold start with an indexer

1. Forum resolution: about 3 requests (PLC, declaration, declaration proof).
2. `listPosts`: placeholders render (`[hint]`, no text).
3. `getRecordProofs` for the visible posts (about 30), in 1–2 requests; each item is verified (§8.6).
4. One `getLatestCommit` per author on screen, in parallel: current proofs are admitted and their text appears `[verified]`; stale ones are point-fetched from the PDS (§1.2.1).
5. Authorities sync at P0 in parallel; the view is `degraded` until they complete. Text appears in about 1–2 s.

---

## 15. Optional indexer (`lessdugin-indexer`)

The indexer is the same core engine (native host, `SqliteKv`) running continuously for every forum it serves, plus the `NS.index.*` XRPC methods (`data-model.md` §11). It returns uri + cid with hint values, and verifiable proof CARs.

### 15.1 Ingest

- **Live:** Jetstream `wildcard` mode (§6.2) from two endpoints at once, deduplicated, with the cursor rewound 10 s on restart. `identity`, `account` and `sync` events for DIDs outside its participant and candidate sets are dropped; the rest are handled as in §6.4 and §3.4.
- **Proofs:** for each relevant commit event, one `sync.getRecord` at the author's PDS; the verified proof is stored in `proof_cache` keyed by `(uri, rev)`. Proofs of backfilled records are fetched lazily on the first `getRecordProofs` request and cached per rev.
- **Backfill:** every 6 h, `listReposByCollection` for each collection in `OURS` across `network.relays` (one NSID per call); then `sync_repo` of each forum owner and participant (level `pds`).
- **Reconcile:** daily `getLatestCommit` for every indexed repo (§6.8).
- **Scope and quotas:** only forum declarations and confirmed forum participants are indexed, with the normative fetch caps per range (§5.4), and per-host politeness of at most 5 requests/s and 4 in flight per PDS host, 200 in flight globally (*tunable*).
- `--poll <pds>…` replaces Jetstream with polling of the listed PDSes, for the devnet and air-gapped setups.

### 15.2 Storage

`SqliteKv` with the client schema (§10.4), plus SQL side tables:
- `change_log(seq, forum, uri, cid | null, did, rev, op, at)`, kept 30 days, which serves `getChanges`;
- `post_stats`, `karma` (derived from the same fold);
- `proof_cache(uri, rev, car)`;
- `backlink` and `fts5`: only with `listBacklinks` and `searchPosts`, **Reserved (P2)**.

It is comfortable up to about 50M records; a Postgres adapter can come later.

### 15.3 Serving

- **First pass (M6):** `describeIndexer`, `listForums`, `listParticipants`, `getRepoHeads`, `getChanges`, `listPosts`, `listRecentDiscussion`, `getThread`, `getRecordProofs`. **Reserved (P2):** `searchPosts` (fts5), `listBacklinks`, `getActivity`, `getKarma`, `getTallies`, `getRecords`. `describeIndexer.features` lists exactly the served methods, and clients call nothing else.
- Every method is an unauthenticated GET with `Access-Control-Allow-Origin: *`. JSON responses carry `asOf` and `indexer`; lists are `Cache-Control: max-age=5` with ETags; nothing is `immutable`.
- `listPosts`, `getThread` and `getKarma` carry the fold's `inputDigest` (`social.md` §1.2), which includes `absent`.
- Rate limit 20 requests/s per client IP (HTTP 429 `RateLimitExceeded`); per-actor limits on `getActivity` when it ships.
- `/metrics` (Prometheus), `/_health`, and `/` (a status page with the source link, AGPL §13).
- **Operations.** An operator blocklist of DIDs, uris, CIDs and hosts is applied at query time. Takedowns and deactivations reported by the relay are honoured within minutes. No blobs are stored. It ships as a distroless image on GHCR.

### 15.4 Client rules and audit

**Use.** In `assisted` mode with an enabled indexer (§2.4), clients use it as:
- an ordering source (`listPosts`, `listRecentDiscussion`, `getThread`) for placeholders;
- a discovery source (`listParticipants`, `listForums`);
- a change source (`getChanges`, §6.6);
- a proof source (`getRecordProofs`), under the currency rule (§1.2).

Displayed values are always recomputed locally from admitted records. Indexer aggregates MAY show as `~n` until then. `getRepoHeads` results are hints: they can mark repos dirty but never count as head observations.

**Audit.** `lessdugin sync <forum> --audit` (and an SPA settings action) cross-checks one indexer for one forum:

| Check | Procedure | Finding |
|---|---|---|
| A1 heads | `getRepoHeads` for every repo in the completeness set (batches of 50), then `getLatestCommit` at each PDS | the indexer's rev is above the PDS head → contradiction; below it by more than `asOf.lagMs + 2 min` → lag |
| A2 changes | for every change the client saw from Jetstream during the session, more than 2 minutes old, with a matching fk | missing from `getChanges` → omission |
| A3 records | the client's eligible, listed posts (complete repos only, older than 2 minutes) against `listPosts(sort = new)`, all pages up to 10,000 | missing → omission; present at the indexer but confirmed absent at the PDS → contradiction |
| A4 digest and aggregates | when the local view is complete: `inputDigest` and every `#tally` (score, agreement, unweighted) against local recomputation | digest mismatch, or equal digests with unequal tallies → contradiction |
| A5 proofs | `getRecordProofs` for up to 25 sampled uris | an item that fails `proofs/1` (other than `stale-proof`) → contradiction |

The report lists findings with counts, and the command exits with code 6 if any contradiction or omission was found. Every failed proof item (§8.6) is also counted against the indexer in `HostRow.strikes`.

**Reserved (P2):** the automatic 1%-per-hour audit sample, and automatic demotion of an indexer above the thresholds (more than 2% contradictions, or more than 3 omissions per day) with a warning.

---

## 16. Reserved and deferred items (summary)

| Item | Phase |
|---|---|
| `verify = proof` background verification of displayed items | P2 |
| Automatic native switch of the Jetstream DID filter above ~50 events/s | P2 |
| Equivocation recording and fork evidence (F15) | P2 |
| UTS #39 checks beyond the owner and moderators | P2 |
| `/bridge/fetch` (anonymous read proxy in `lessdugin web`) | P2 |
| Indexer `searchPosts`/fts5, `listBacklinks`, `getActivity`, `getKarma`, `getTallies`, `getRecords`; automatic audit and demotion | P2 |
| Thumbnails | P2 |
| Identity level `audited` (PLC audit log); proofs against historical keys | P7 |
| Verified MST range sync (`sync.getRecord` + `getBlocks`), sync windows, indexer range proofs (`getRepoRanges`) and set digests | P7 |
| Media mirrors; Service Worker media streaming | P7 |
| `karmaSnapshot` checkpoints | P7 |

---

## 17. Decisions made while writing

These choices fill gaps or resolve ambiguities in the sources. Each takes the simplest safe option, and all are open to review before M7.

1. **Jetstream wire (§6.2).** ADR 0003 replaces the synthesis's endpoint line: the v2 endpoint is the default, `dids=` covers filters of up to 150 DIDs, and the v1 wire with `options_update` covers larger ones. Cursors persist `seq` and `witnessedAt`; `#sync` triggers a full re-list.
2. **Which events "unknown DID" applies to (§6.4).** `identity`, `account` and `sync` events for unknown DIDs are dropped. Commits are filtered by fk prefix whatever the DID. From an unknown DID only membership commits that can introduce a newcomer are kept. `KNOWN` means "has an identity row", as in ADR 0003.
3. **Browser DID filter (§6.2).** Applied once every followed forum has finished one discovery pass. Reconnects for a changed set are rate-limited to one per 10 minutes, and newly added DIDs get one catch-up sync. A membership-only side connection with `kinds=commit` keeps newcomer discovery working. The traffic-triggered native switch stays deferred, per the completeness review's cut list.
4. **`verify = proof`** is Reserved (P2), following the completeness review's cut list ("background `--verify proof`"). On-demand `verify` remains.
5. **Stale proofs (§1.2).** "Current" is tied to a head observation sent no earlier than 60 s before the proof arrived. Stale proofs are held in memory only. A stale proof becomes `proven` when the PDS later confirms the same CID. They MAY be shown as *provisional items*, a concept introduced here so that cold-start text can appear without entering the fold.
6. **Identity-hint content (§1.5, §3.2)** (did:web documents that fail CORS in the browser) is not a fold input either: the PDS itself came from a hint. It is shown provisionally and collapsed, and an authority in that state degrades the view.
7. **Version order (§5.6.1).** Order keys `(revLo, sentAt)` replace the bare "rev" of I3, because `getRecord` returns no rev. Point fetches use the last observed head as a lower bound; conflicts are declared only between exact observations.
8. **Tombstone keys** are lower bounds observed from the PDS, never hint revs. Otherwise a lying Jetstream could suppress a re-created deterministic-key record such as an opinion.
9. **Unstable listings (§5.2)** (the head moved during the pass) admit records but write no tombstones and advance no `listed` entry.
10. **Cursor probe (§5.3)** follows ADR 0004: the two-part probe, the short-page rule (disabled per host by `doctor`), a 7-day recheck of `No`, and continuous checking of every page. `FULL` mode gets its own safety caps (5,000 pages, 200 MB).
11. **Fetch caps (§5.4).** Every key counts toward N, including invalid ones. A capped range is complete for `repoStatus` and completeness, and is flagged `[partial]` on the user card. A range of exactly N keys is reported as capped.
12. **`absent` (§5.6.4)** is defined for `social.md`: it requires a tombstone, or a complete listing above the cap boundary.
13. **`obs` (§5.6.5).** The first local observation from any source, including hints, is used, and the indexer's `firstSeen` is folded in by minimum. A hostile, consented indexer can thus affect only ranking and the `[future-dated]` badge.
14. **Dirty marks (§6.5).** A point fetch whose CID matches the hint satisfies the mark, so live updates do not leave repos `Stale` until the next listing. Marks are capped at 1,000 per repo.
15. **`describeRepo` caching (§5.2).** It keeps the synthesis's 24 h cache, refreshed early when a hint or own write names a new collection. A brand-new collection with no hint can be missed until the next daily reconcile; this is documented rather than paying one more request per sync.
16. **Account status (§3.4).** `desynchronized` and `throttled` are temporary, not inactive.
17. **Declaration proof at first open (§4.2).** A failure is a warning. The `pds`-level declaration still applies, since TLS from the owner's PDS is already authoritative.
18. **Per-host budgets (§9.2)** follow ADR 0004 and replace the synthesis's shared "rate domain". The 50%/80% fractions are read as "background stops when half of the host's window is used, P0 when 80% is used".
19. **Outbox locking (§7.1).** "One in flight per account" is enforced across processes and tabs by a named lock. `Host::lock` is added to the trait for this and for I9's per-repo locks. The `web` bridge takes the native lock.
20. **ADR 0005** refinements (create-if-absent through `putRecord` with `swapRecord: null`; a 500 means "ambiguous, re-read"; no-op updates skipped) are applied in the pump and flagged for `data-model.md` §8.
21. **Pending writes** appear only in the viewer's lens, so the fold and parity output never depend on unconfirmed writes.
22. **`Host` additions (§12).** `redirects`, `sent_at_ms`, `protocols` on `websocket`, `lock`, and blob functions keyed by CID (`fetch_blob`, `blob_open`, `blob_read`, `blob_delete`) in place of a single opaque handle. The completeness fix names `BlobHandle`; here it is `{cid, size}`.
23. **Blob directory sharding (§11.2)** uses the last two characters of the CID. The first two are always `ba` for base32 CIDv1, which the synthesis's `cid[0..2]` would have used.
24. **Concrete constants** not given by the sources (*tunable* unless stated): PLC adaptive pacing; DID-document and protected-resource caps of 64 KiB; proof-bundle block caps (2,000 per proof CAR, 20,000 per bundle); blob timeouts (20 s without progress, 300 s total); LRU size 10,000; newcomer ordering; relay defaults; indexer `getChanges` polling every 10 s or 60 s.
25. **Autoload (§11.3).** Comments inherit their root post's self-labels, and an expanded `Collapsed` item counts as visible.
26. **Audit (§15.4).** The flag is `lessdugin sync <forum> --audit`, and checks A1–A5 make the M6 acceptance criteria ("catches an injected omission and a forged proof") concrete. Automatic sampling and demotion are deferred, per the cut list.
27. **Closure (§4.3)** follows opinion subjects and comment roots and parents of seed records, one level deep and unbounded. Each DID is confirmed by its membership record before it is synced (F16).
28. **Badges (§1.6).** `[stale]` also marks items served from cache after a failed sync. `[pending]` (outbox) is added, from the SPA's "pending-write badges". `[edited]` is defined from `updatedAt` or a known revision.

**Open questions**
1. `data-model.md` §8.4–§8.5 still specify `createRecord` for create-if-absent. ADR 0005 decides `putRecord` with `swapRecord: null`, and this document follows the ADR. The data model should be amended.
2. **The currency rule makes indexer proofs stale for any author with later activity, including Bluesky activity**, because a repo has only one head. Each reader therefore still contacts each changed author's PDS (with small requests). Is this acceptable for the > 3,000-participant guidance, or should P7 range proofs be pulled forward?
3. Does Jetstream's DID filter also filter `identity` and `account` events? This document drops unknown DIDs client-side either way. M5 should measure it, because it changes browser bandwidth.
4. ADR 0004's per-host verdict is statistical (rotating egress IPs). The M1 fixed-IP check may still show a shared budget, which would change §9.2 and §14.
5. ADR 0003 asks whether the public v1 Jetstream hosts will be retired. If they are, filters of more than 150 DIDs need several v2 connections of at most 150 DIDs each.
