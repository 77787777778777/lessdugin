<!-- SPDX-License-Identifier: CC0-1.0 -->

# lessdugin threat model (normative)

**Status:** draft for the `.temp` namespace (M0). It is re-reviewed in the M7 security pass, and every MUST in §5–§8 is a release blocker for 0.1.
**Sources:** `docs/design/synthesis-v1.md` §5.1–§5.2 and §6.13 (with the measures it places in §1, §3, §6 and §10), every fix from `docs/design/completeness-review.md`, and the approved plan. Where they differ, the plan wins. Critique IDs (F1–F36, H*, M*, X*, C*) refer to `docs/design/archive/critiques.md`.

## 0. About this document

**Key words.** MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are used as in RFC 2119 and RFC 8174.

**"Reserved (Pn)"** marks a measure that phase Pn will define. "Reserved (deferred)" marks one that the completeness review cut from the first pass without assigning a phase. Until then, v1 clients MUST NOT rely on it, and the residual risk is listed in §9.

**Measure IDs.** Every measure has an ID of the form `AREA-n` (for example `TERM-3`). Tests, reviews and the traceability table (§11) cite these IDs. An ID never changes meaning; a withdrawn measure keeps its ID with the note "withdrawn".

**Companion specifications**

| Document | Defines what this document relies on |
|---|---|
| `data-model.md` | Record validity (G\*, PO\*, …), admission levels (§7.1), the write protocol and MIME normalisation (§8), scopes and client metadata (§10), the login binding (§10.5) |
| `lfm-1.md` | The HTML contract and allowlist (§8), `safe_href` (§5.1), media rendering (§6), the TeX pre-scan and MathML filter (§7), `TermText`/`term_sanitize` (§9.1), printer obligations (§9.4), quote resolution (§11) |
| `social.md` | `mod/1`, `karma/1`, gates, reacts, visibility (`Hidden(Restricted)` and the others), threads, fail closed |
| `sync.md` | Trust levels and admission, identity resolution, per-repo sync, Jetstream, the outbox pump, `proofs/1`, store invariants, media fetching and display |
| `limits.md` | Every numeric cap named here. Values in this document are repeated for readability; `limits.md` wins on any difference |

---

## 1. Assets and security goals

| ID | Goal |
|---|---|
| G1 | **Attribution integrity.** Text displayed as written by DID *d* was published by *d*: it was admitted at level `pds` or `proven` (`data-model.md` §7.1). No third party can put words in anyone's mouth. |
| G2 | **Moderation integrity.** Given the records a client holds, it never shows content that the forum's current authorities have hidden. If authority data is missing, the view degrades instead of opening (fail closed). |
| G3 | **Vote and karma integrity.** DIDs outside E cannot change karma, scores or other users' visibility, beyond the bounds stated in `social.md` (P5, P7). |
| G4 | **Account safety.** A lessdugin bug, a compromised hosted SPA or an XSS cannot write outside the `NS` collections, cannot manage the account, and cannot take a durable credential off the device. |
| G5 | **Local machine safety.** Content cannot execute code, control the terminal, read or publish local files, or write outside lessdugin's data directories. |
| G6 | **Reader privacy.** Reading discloses nothing beyond the inherent limits of §4, and public actions are announced before their first use. |
| G7 | **Availability.** Hostile content or hosts cause bounded, isolated failures. They cannot crash-loop a client, exhaust memory, or make one forum unusable because of another. |
| G8 | **Distribution integrity.** Users can check which code they run, and they have a path (`lessdugin web`) that does not trust the hosted site. |

---

## 2. Trust boundaries

### 2.1 Content sources

| Source | Text shown? | Discovery and scheduling? | Fold input (moderation, karma)? |
|---|---|---|---|
| Author PDS from the DID document, over TLS, CID recomputed (`pds`) | yes | yes | yes |
| Proof CAR from any source, verified by `proofs/1` with an identity document ≤ 1 h old, and current (rev ≥ the PDS head) (`proven`) | yes | yes | yes |
| A third-party proof not (yet) confirmed current (`[stale]`, ID-10) | yes, badged `[stale]`, until a current fetch replaces it | yes | no |
| Jetstream (commit `record` and `cid` included) | no; counts and placeholders only | yes (marks repos dirty) | no |
| Constellation, relay, Slingshot | no | yes | no |
| Indexer JSON and aggregates | no; scores may show as `~n` until recomputed locally | yes | no |

The full admission rules are in `sync.md` and `data-model.md` §7.1. There is no option in v1 to "trust an indexer like a PDS" (F13: the exception it proposed is rejected).

### 2.2 Components

| Component | Trusted for | Not trusted for |
|---|---|---|
| The author's PDS (as named in the DID document) | serving that author's own records and blobs | other DIDs' content; consistency across readers (it may equivocate or cloak, §4 L10); timestamps; availability; response sizes |
| The author's PDS operator | — | It holds the signing key of hosted accounts and can write as them (§4 L6) |
| PLC directory, did:web hosts | the current DID document (`doc` level). PLC audit verification is Reserved (P7) | anything else |
| DoH provider (default `cloudflare-dns.com`, then `dns.google`) | answering `_atproto` TXT queries honestly | privacy of the queried names (§4 L2) |
| Relay, Jetstream, Constellation, Slingshot, indexers | hints only (§2.1) | any displayed text, any fold input |
| Forum authorities (owner, admins, moderators) | moderation within their rank (`social.md` §4) | anything outside their forum |
| The hosted SPA origin and its domain | the code it serves, while you use it (§4 L5) | — (verify with the published build hash, SUP-4) |
| `lessdugin web` | the code embedded in the binary you installed | — |
| The local OS user account and its browser profile | everything (out of scope, §3.2) | — |
| Other local users and processes | nothing | — |

---

## 3. Adversaries

### 3.1 In scope

| ID | Adversary | Capabilities | Goals | Main measures |
|---|---|---|---|---|
| A1 | **Hostile participant**, including one who self-hosts a PDS | Writes any record in their own repo with any `createdAt` and TID. Edits after curation. Serves different commits to different IPs (equivocation, cloaking). Serves oversized, slow, compressed-bomb or malformed responses and cursor loops. Hosts media and did:web documents, so learns reader IPs. Mentions or links many DIDs and hosts. | impersonation, spam that survives moderation, deanonymising readers, crashing clients, amplification | SOC-\*, INFRA-\*, PILL-\*, PRIV-1, ID-\* |
| A2 | **Sybil farm** (many DIDs; a self-hosted PDS is free) | Thousands of memberships, votes and reacts | karma, scores, gates, visibility, react spam | SOC-1 to SOC-4, SOC-10 |
| A3 | **Lying third party**: relay, Jetstream, Constellation, Slingshot, indexer, DoH provider | Fabricated, withheld, stale or replayed data. Records with self-consistent CIDs. Stale signed proofs. | putting words in mouths, resurrecting deleted or edited content, hiding content, steering clients at victims | INFRA-1 to INFRA-12, ID-9 to ID-11 |
| A4 | **Network attacker** (café Wi-Fi, captive portal, on-path) | Spoofs plain DNS, intercepts plaintext | handle spoofing, login phishing, content injection | ID-1, ID-2, AUTH-\*, TLS everywhere (INFRA-9) |
| A5 | **Malicious markdown and media** (the payload of A1) | Escape sequences, bidi tricks, deep nesting, TeX bombs, mXSS, SVG, polyglots, metadata, local-path references | code execution, terminal takeover, XSS, crash loops, file exfiltration | TERM-\*, FILE-\*, SPA-\*, PILL-\* |
| A6 | **Compromised hosted SPA or domain**: DNS, registrar, static host, CI, publisher account | Serves modified code to hosted users; changes `_lexicon` TXT and permission-set text | token theft, mass writes | AUTH-1 to AUTH-4, SUP-\*, the zero-trust `lessdugin web` path (§7) |
| A7 | **XSS or malicious code through dependencies** | Runs script in the SPA origin, or code in the native process | the same as A6, plus local secrets | SPA-1 to SPA-13, AUTH-4, AUTH-6, SUP-6 to SUP-10 |
| A8 | **Another local user or process** on the same machine | Binds `127.0.0.1:47778` while lessdugin is not running (port squatting); reads world-readable files; races argv exposure | session theft, draft theft | §7, AUTH-7, AUTH-8, TERM-9 |

**A rogue or former authority** acts under A1 with rank. `mod/1` bounds it: assertions lapse when the declaration drops the actor, higher ranks override, and every action is in the public mod log (`social.md` §4.10, §4.12).

### 3.2 Out of scope

- A compromised OS account, kernel, browser binary or browser profile of the user.
- A compromised PLC directory (P7 adds audit-log verification, `strict` mode).
- The user's own PDS operator acting against them (§4 L6).
- Breaking SHA-256, secp256k1 or P-256.
- Legal compulsion of any party.

---

## 4. Inherent limits

These are not bugs. Each MUST be stated where the "Stated in" column says, in user-facing words, and in the user guide.

| ID | Limit | Stated in |
|---|---|---|
| L1 | **Public means public and permanent.** Votes, agreement, reacts, tag votes, memberships (with their notes), mod actions (with their reasons), posts, comments and revisions are public records. Relays, Jetstream replay, Constellation and indexers archive them, so deleting a record does not erase copies. | First-use interstitial per action type (PRIV-9); the editor (PRIV-5); the join and apply dialog (membership `note`) |
| L2 | **Reading reveals your IP.** It is revealed to every PDS you sync (including self-hosted ones), to media hosts when media loads, to PLC and did:web hosts, to the DoH provider (which also sees the handle names you look up), and in assisted mode to Jetstream, Constellation, relays and indexers. | Settings → Privacy; the media click-through notice (PRIV-1) |
| L3 | **Signed content is non-repudiable.** A saved copy of a deleted record, together with its proof, stays attributable to its author forever. | The editor's delete dialog; the user guide |
| L4 | **Time-based rules are best-effort.** Timestamps and TIDs are claims. Backdating can only hurt its author (`social.md` §2), and rate limits are advisory (Reserved (P2), `ratelimit/1`). | The user guide; moderator help |
| L5 | **The hosted SPA is trusted while you use it.** Whoever controls `lessdugin.7778777.online` controls the code hosted users run. `lessdugin web` is the zero-trust path. | Settings → About (with the build hash, SUP-4); the login page |
| L6 | **Your PDS operator can write as you.** On a hosted PDS the operator holds your repo signing key. | The user guide |
| L7 | **Open mode relies on public relay infrastructure.** Only approval mode is complete without third-party servers. Open mode finds brand-new members only through the Bluesky relay, Jetstream and Constellation (or an indexer). | The forum header and `forum create --mode open`: "Open mode relies on Bluesky relay, Jetstream and Constellation" |
| L8 | **A forum is bound to its owner DID forever.** `fk` hashes the forum uri, which contains the owner DID. Losing the owner's keys loses the forum, and a takeover of the owner account (or, for did:web, of its domain) takes over the forum. | `forum create` (succession guidance, ID-12) |
| L9 | **Edit history is verifiable, not complete.** Authors can redact revisions, and `--no-history` edits leave none. | The history view ("history redacted"; "on an unavailable version") |
| L10 | **Self-hosting participants can equivocate or cloak**, showing different content to different readers. v1 does not detect this (INFRA-11 is Reserved). | The user guide; moderator help |
| L11 | **Moderation is a view, not deletion.** Hidden content stays in its author's repo, readable in the raw view and by other software. | The raw view banner; moderator help |

---

## 5. Measures

Each subsection covers one area of synthesis §5.2. The rule texts are normative; where another specification holds the details, the row names it, and that specification is normative for the details.

### 5.1 Terminal (F1, F2)

| ID | Measure |
|---|---|
| TERM-1 | **`TermText` only.** Every string that reaches a terminal is a `TermText`, which only `term_sanitize` can construct (`lfm-1.md` §9.1). This covers the CLI printers, `StyledDoc` spans, the ratatui adapters, error messages (including XRPC `message` strings from remote servers), `resolve` and `doctor` output (DID-document `alsoKnownAs` and service strings), indexer snippets, Jetstream payloads, and log lines. Nothing relies on filtering by ratatui or `less`. |
| TERM-2 | **Sanitisation.** C0 controls (except structural `\n` and `\t`), DEL and C1 become visible escapes (`^[`, `^?`, `\u{85}`) in a distinct style. U+2028 and U+2029 become a space. Bidi controls are stripped from prose and names, and shown as visible markers such as `⟨RLO⟩` in code (Trojan Source). Names also lose zero-width characters and badge glyphs, and are capped in graphemes (`limits.md` §13.3). The exact table is `lfm-1.md` §9.1. |
| TERM-3 | **Escape sequences.** Printers emit only SGR and OSC 8, with the parameter set in `lfm-1.md` §9.4. Content never sets the terminal title (no OSC 0/1/2), never writes OSC 52 (clipboard), and never moves the cursor. |
| TERM-4 | **OSC 8 links** are emitted only for URLs that `safe_href` accepted as `External` or `Mailto`, only on a TTY with colour enabled, percent-encoded outside 0x21–0x7E and `;`, and only up to 2,048 bytes. The real host is printed dimmed after the link text, as ` ⟨host⟩`, whenever the text differs from the URL (`lfm-1.md` §9.4). |
| TERM-5 | **Badges** appear only in the fixed badge column and style. Badge glyphs are stripped from user text (`Name` context), so content cannot forge `[verified]` or a moderator notice. |
| TERM-6 | **Logs.** The `tracing` subscriber writes through a sanitising writer that applies `term_sanitize(Prose)` to every field, and redacts secrets (AUTH-10). |
| TERM-7 | **Machine output on a TTY.** When stdout is a TTY, `--json` writes every C1 code point, U+2028, U+2029 and bidi control as `\uXXXX`, and `--raw` passes every line through `term_sanitize(Code)`. When stdout is not a TTY, both are byte-exact. |
| TERM-8 | **Editor templates** (F2). The template written for `$EDITOR`:<br>• ends with at least 6 fixed lines after any quoted or remote text, so remote text never sits in the last 5 lines (vim modelines);<br>• prefixes every quoted line with `# `;<br>• neutralises modelines inside quoted lines: a U+0020 is inserted before the `:` of every match of `(?i)(^\|\s)(vi\|vim\|ex)([<=>]?[0-9]*)?:` and of `(?i)local variables:`, and every `-*-` becomes `- * -`;<br>• writes validation and server errors as single-line YAML comments (`# ` + `term_sanitize(Prose)` with every line break replaced by U+0020). |
| TERM-9 | **Template files** are created with mode 0600 inside a directory with mode 0700, with `O_NOFOLLOW` and `O_EXCL`. On Windows they are created under `%LOCALAPPDATA%`, whose ACL excludes other users. |
| TERM-10 | **Frontmatter parsing.** Drafts' YAML frontmatter is parsed by serde-saphyr with aliases and anchors rejected, nesting ≤ 32 and size ≤ 65,536 bytes (`limits.md` §6). A draft that breaks a limit is refused with a one-line error. |

### 5.2 Local files (F3, F4)

| ID | Measure |
|---|---|
| FILE-1 | **Draft image paths.** A local image reference in a draft (`![](path)`) is accepted only if all of these hold:<br>• the path is relative: not absolute (`/…`, `C:…`, `\\…`), not starting with `~`, and with no `..` component;<br>• after canonicalisation (following symlinks), it lies inside the canonical directory that contains the draft, compared by path components;<br>• it names a regular file (not a FIFO, device, socket or directory), checked with `fstat` on the file opened with `O_NOFOLLOW` on the final component;<br>• its sniffed type is on the upload allowlist (PRIV-6).<br>Anything else is an error that names the reference. |
| FILE-2 | **Publish confirmation.** Before publishing, the summary lists every local file that will be uploaded, with its absolute path, size and sniffed type, and asks `[y/N/e]`. `--yes` does not skip this confirmation for a **foreign draft**: any file passed to `post <file>` that is not under `$DATA/drafts/`. |
| FILE-3 | **Downloads** (`media get`) are saved as `<cid>.<ext>`. `ext` comes from a fixed map of the **sniffed** type, which MUST agree with the declared type (`sync.md`); an unknown type saves as `.bin`. `#media.name` is display-only and is never used as a file name. Without `-o`, the file goes to the current directory, and an existing file is never overwritten (`O_EXCL`). |
| FILE-4 | **`media open`** works only for image, video and audio types. Everything else is save-only. The file path is passed to the platform opener as a single argv element, never through a shell. On Windows the file gets a Mark-of-the-Web (`Zone.Identifier` stream with `ZoneId=3`). |
| FILE-5 | **No shell, anywhere.** External programs (`$EDITOR`, `less`, the platform opener, the browser) are started with an argv array. `$VISUAL`/`$EDITOR` are split into words like a POSIX shell would, without expansion, and the file path is appended as a separate element. |
| FILE-6 | **Data paths.** lessdugin writes only under its config, data and cache directories (`~/.config/lessdugin`, `~/.local/share/lessdugin`, `~/.cache/lessdugin`, or the platform equivalents), the per-user temp directory, and files the user names with `-o` or `--out`. |

### 5.3 SPA (F5–F8)

| ID | Measure |
|---|---|
| SPA-1 | **CSP and response headers** exactly as in §6. |
| SPA-2 | **Trusted Types.** Exactly three policies exist, created once at startup:<br>• `lessdugin-md`: `createHTML(html)` returns the string `DOMPurify.sanitize(html, LFM_CONFIG)`, which the policy wraps as `TrustedHTML`. It is the only way rendered content reaches the DOM.<br>• `lessdugin-worker`: `createScriptURL(u)` returns `u` only if it equals, as an absolute URL string, one of the build's worker script URLs resolved against `location.origin`; otherwise it throws.<br>• `dompurify`: created by DOMPurify itself.<br>No `default` policy exists, and no policy implements `createScript`. |
| SPA-3 | **DOMPurify configuration** is generated from `lfm-1.md` §8.2 and §7.3 (allowed tags and attributes, `ALLOW_DATA_ATTR: false`, the URI regexp, and a hook for class tokens and value grammars), and contains nothing else. Vectors assert `DOMPurify.sanitize(html) === html` for every renderer output (`lfm-1.md` §8.5). |
| SPA-4 | **Links.** Every `href` outside rendered content goes through `<SafeLink>`, which calls the core's `safeHref` (`lfm-1.md` §5.1). CI fails on any JSX `href` attribute outside `SafeLink`. External links open with `noopener,noreferrer`. `data-at-uri` and `data-did` are re-parsed by the core before use. |
| SPA-5 | **Names and direction.** User-supplied names render as React text inside `<bdi>`. Rendered roots carry `dir="auto"`, and the stylesheet sets `unicode-bidi: isolate` on them. |
| SPA-6 | **Math.** Disabled TeX macros and the MathML allowlist (`lfm-1.md` §7.1, §7.3). In Temml mode, Temml runs with `trust: false`, and its output passes DOMPurify's MathML profile generated from the same allowlist. |
| SPA-7 | **Media elements.**<br>• Object URLs are built as `new Blob([bytes], {type: T})`, where T is a constant from the display allowlist chosen by the sniffed type, never the declared string. They are revoked when their element unmounts.<br>• SVG is never displayed inline in v1.<br>• Download cards use `application/octet-stream` with a `download` attribute of `<cid>.<ext>`.<br>• No `src` comes from content: only the SPA sets it, after `Host::fetch_blob` verified the bytes (INFRA-6). |
| SPA-8 | **Fragments and routing.**<br>• In-content `#frag` links become `data-anchor` and scroll inside their own rendered root (`lfm-1.md` §4.6). Content never assigns `location.hash`.<br>• The OAuth callback (`response_mode=query`, AUTH-5) and, in `web` mode, the bridge capability (§7.2) are read and removed with `history.replaceState` **before** the hash router mounts. |
| SPA-9 | **Referrer.** `Referrer-Policy: no-referrer` as a header, and `<meta name="referrer" content="no-referrer">` in `index.html`. |
| SPA-10 | **Frame guard.** Before rendering anything, the SPA checks `window.top === window.self`. If not, it clears the document and stops. This duplicates `frame-ancestors 'none'`, which a `<meta>` CSP cannot express (§6.3). |
| SPA-11 | **Wasm boundary.** Maps crossing serde-wasm-bindgen are JS `Map` objects, never plain objects, so record keys cannot pollute prototypes (F24). |
| SPA-12 | **No third-party code.** Scripts, styles, fonts and wasm load only from the SPA's own origin. There are no analytics, CDNs or remote fonts. The only network requests are the data requests the engine makes (DoH, PLC, PDSes, and the configured CORE+ and EXT services). |
| SPA-13 | **Worker isolation of parsing.** All parsing of network data (JSON, DAG-CBOR, CAR, markdown, media sniffing) runs in the engine worker. The main thread holds the OAuth client and receives only DTOs and rendered HTML (which still passes `lessdugin-md`). |

### 5.4 Poison pills (F8)

| ID | Measure |
|---|---|
| PILL-1 | **Iterative walkers.** No code path recurses in proportion to input nesting: JSON and DAG-CBOR decoding, CAR reading, the MST walk, the lexicon validator, every LFM stage (`lfm-1.md` §1.5), the MathML filter, thread and wiki walks, media container walks. |
| PILL-2 | **Pre-parse limits.** Nesting depth is checked before or during tokenisation, before any tree is built: JSON and DAG-CBOR ≤ 36 levels for a whole document (deeper: the response is rejected) and ≤ 32 inside a record value (deeper: the record is invalid, `data-model.md` G10); markdown output depth ≤ 64 and ≤ 2^20 parser events (`lfm-1.md` §3, §8.4); the TeX pre-scan (`lfm-1.md` §7.1). Values in `limits.md` §6. |
| PILL-3 | **No preallocation from declared lengths.** CAR varints, CBOR array, map and string lengths, `Content-Length`, MP4 box sizes and image dimensions never size an allocation directly: a buffer grows only as bytes actually arrive, and never beyond the remaining input or the applicable cap. |
| PILL-4 | **Native isolation.** Per-record work (decode, validate, render, media processing) runs under `catch_unwind`. A panic quarantines that `(uri, cid)` and is logged. Native stack overflow cannot be caught, so PILL-1 and PILL-2 are the defence there. |
| PILL-5 | **Browser quarantine.** Before the worker processes a batch of records (decode, validate or render), it writes the batch's `(uri, cid, RENDERER_VERSION)` triples to the IndexedDB `state` store as in-flight markers, and deletes them after the batch completes. When the supervisor (`packages/engine` `client.ts`) restarts a crashed worker:<br>1. leftover markers become **suspects**;<br>2. suspects are re-processed one at a time, each under its own marker;<br>3. a suspect that crashes again is **quarantined**: it is skipped and displayed as "failed to render" until `RENDERER_VERSION` changes;<br>4. restarts back off from 1 s, doubling to 60 s (`limits.md` §7). |
| PILL-6 | **Fuzzing.** cargo-fuzz targets: CAR reader, DAG-CBOR decoder, JSON → DAG-CBOR, MST proof walk, commit verification, lexicon validator, LFM renderer (every output), `term_sanitize`, `safe_href`, the media strippers (JPEG, PNG, WebP, GIF, MP4) and the frontmatter parser. Nightly CI runs them; M4 requires a 10-minute run per target without a panic. |

### 5.5 Identity (F9–F11)

| ID | Measure |
|---|---|
| ID-1 | **DoH by default** on every platform, using the DoH JSON API over TLS: `cloudflare-dns.com`, falling back to `dns.google` (configurable). Native clients may opt into the system resolver (`network.dns=system`); handles verified only through it are flagged `dns-unauthenticated`. |
| ID-2 | **Bidirectional handles.** A handle is displayed only if DNS (or, when DNS has no answer, `https://<handle>/.well-known/atproto-did`) names the DID **and** the DID document claims the handle. Otherwise the UI shows the short DID with the claimed handle struck through. Stored references are always DIDs. Slingshot answers are hints. |
| ID-3 | **Punycode** labels (`xn--`) are never decoded for display. |
| ID-4 | **Confusables.** Display names and handles are compared with a UTS #39 skeleton against the forum owner's and moderators' names. A match from a different DID shows a warning ("looks like @x"). Comparing against frequent posters is Reserved (deferred). |
| ID-5 | **Bylines** always show the verified handle or the short DID; the display name is secondary. The forum header always shows the owner's handle, and the forum directory groups forums by owner. A listed moderator is shown with the moderator role only if they have a membership record in the forum (F9); their authority does not depend on it. |
| ID-6 | **Mentions and link text.** Mention styling applies only to `[@x](at://<did>)` links, and the displayed name is the verified handle or the short DID (`lfm-1.md` §4.4, §5.4). When link text looks like a host and differs from the target host, the real host is shown (`lfm-1.md` §5.5, §9.4). |
| ID-7 | **did:web.** Forms with a port or path are rejected. Owners and moderators whose DID is did:web are flagged in the forum header and in `forum create`/`mods add` ("domain expiry means forum takeover"). |
| ID-8 | **Identity freshness.** Handles are cached 6 h and DID documents 24 h. Both are invalidated by a Jetstream `identity` event for a known DID, by a signature failure (retry once with a fresh document), and by `RepoNotFound`, `RepoDeactivated` or `RepoTakendown` (refresh; if the PDS changed, follow the migration). |
| ID-9 | **Third-party proofs** earn `proven` only if the DID document was fetched from PLC or did:web within the last hour, and the commit `rev` is ≤ now + 5 min. |
| ID-10 | **Stale proofs.** A third-party proof is *current* only if its rev is ≥ the head returned by the author PDS's `getLatestCommit`. Clients batch these checks per author on screen. A proof not yet confirmed current is `[stale]`: it MAY be displayed with the `[stale]` badge, it is never a fold input, and once it is confirmed below the head the client point-fetches the record from the PDS, whose result (or tombstone) replaces it. A stored rev never advances past the head observed at the PDS on the strength of a third-party proof. A proof that fails against the current key but is older than the PDS head is `stale-proof`: the client fetches a fresh proof from the PDS and does not count it against the source (`sync.md`). |
| ID-11 | **Login binding and TOFU.** At login (`data-model.md` §10.5): the PDS comes from the DID document of the token's `sub`; that PDS's `/.well-known/oauth-protected-resource` `authorization_servers` MUST contain the token's `iss`. The authorization server is shown at first login and pinned per account DID. A later login with a different `iss` requires an explicit confirmation that names both servers (a PDS migration is the honest cause). Login accepts a DID as well as a handle. |
| ID-12 | **Forum succession** (L8). `forum create` and the guide recommend a dedicated did:plc account for each forum, whose rotation keys are held by at least two admins. A `forum.successor` or `aliases` field is Reserved (P2). |

### 5.6 Infrastructure (F13–F17)

| ID | Measure |
|---|---|
| INFRA-1 | **Hints never become text** (§2.1). Hint sources may discover, schedule, order placeholders and supply approximate counts. Text attributed to a DID needs a `pds` or `proven` record; a point fetch is one request. The fold ignores indexer `state`, `tier`, `score` and similar fields; moderation inputs come only from the record store (`sync.md` I7). |
| INFRA-2 | **Writer-derived fields are hints.** `markdown.mentions` and `markdown.links` are never used for rendering, notifications or the fold. Mentions and pingbacks are computed from the rendered bodies of synced records (`lfm-1.md` §12). |
| INFRA-3 | **Byte caps** are counted while streaming, **after** decompression, and the transfer is aborted at the first byte over the cap. `Content-Length` is never trusted. Every cap is in `limits.md` §5. |
| INFRA-4 | **Listing integrity.** Cursors must strictly advance: within a descending listing, keys strictly decrease, every key is below the request cursor, and each returned cursor is below the previous one. A violation in range mode discards the pages read so far, marks the host range-incapable, and redoes the listing as a full-collection listing with a prefix filter (`sync.md` §5.3); a repeated cursor or a safety cap in that mode stops it, and the range is `[partial]` (no tombstones). Fetch caps are normative: the first N keys in descending rkey order per `{fk}-` range and collection, N = 100,000 for `opinion` and 20,000 otherwise (`data-model.md` §7.4). |
| INFRA-5 | **Jetstream.** Every message is capped at 1,200,000 bytes: natively through the WebSocket library's message and frame limits; on the v1 wire also through `maxMessageSizeBytes`. A browser cannot cap a WebSocket message before it arrives, so it drops an oversized message on receipt and fails over to the other endpoint. `identity` and `account` events for DIDs that are not in the identity cache are dropped before any further processing. A commit's `record` and `cid` are hints that schedule a point fetch. |
| INFRA-6 | **Blobs stream outside wasm memory.** Blobs are fetched with `Host::fetch_blob(did, cid, max) -> BlobHandle` (§5.6.1). The core handles only handles, so no blob bytes enter wasm linear memory except text bodies ≤ 1,000,000 bytes and media the user is preparing for upload, up to `MEDIA_WASM_WINDOW_BYTES` (`limits.md` §6). |
| INFRA-7 | **Fan-out limits** (F16).<br>• The closure follows only DIDs referenced by seeds' records in the forum; identities are resolved lazily, only for participants and authors on screen.<br>• Non-participant identities (mentions, profile links) resolve only on hover, focus or click; until then the short DID is shown. At most 16 mention DIDs per body resolve eagerly, and only participants.<br>• Forum-declared endpoints (`discovery.indexers`) need one-time consent per endpoint, `https`, a hostname (no IP literal, no `localhost`) and the default port, and remain hints.<br>• Per-host concurrency, and a per-domain cap for identity requests (`limits.md` §8), bound what any one forum can make clients send to one host. |
| INFRA-8 | **SSRF guard** for every outbound request of the native host (CLI, TUI, `lessdugin web`) and of the indexer (§5.6.2). |
| INFRA-9 | **TLS everywhere.** Release builds connect only over `https:` and `wss:`. `http:` and `ws:` are allowed only in `--dev` builds for the devnet (ADR 0006), never implicitly. |
| INFRA-10 | **Rate-limit hygiene.** Clients honour 429, `Retry-After` and `RateLimit-*`, keep a budget per host, use full-jitter retries and a per-host circuit breaker (`limits.md` §7, §8). Cross-origin browser GETs carry no custom headers, so they need no CORS preflight. Native requests send `User-Agent: lessdugin/<ver> (+https://lessdugin.7778777.online)`. |
| INFRA-11 | **Equivocation recording** (record `(did, rev, commit cid)` and compare with Jetstream commits; warn on forks) is Reserved (deferred); fork evidence attached to a hide is Reserved (P2). Meanwhile, moderators MAY sync through an indexer or another network path to defeat IP-targeted cloaking. |
| INFRA-12 | **Indexer obligations** (server side, F17):<br>• full-text search: every user token is quoted, at most 16 terms, prefix terms need ≥ 3 characters (when `searchPosts` ships);<br>• snippets are plain text with highlight ranges, never HTML;<br>• a takedown blocklist of `(did, uri, cid, host)` entries is applied at query time; relay takedowns and account deactivations are honoured on receipt of the event and at the latest at the next reconcile;<br>• no response containing record values or blocks is cacheable as `immutable`; lists are `max-age=5`;<br>• no blob storage;<br>• only forum declarations and forum participants are indexed, with quotas per DID and per PDS host;<br>• per-IP request limits, and per-actor limits on `getActivity` (when it ships);<br>• the status page publishes a legal contact and the source link;<br>• the SSRF guard (INFRA-8) applies to every outbound fetch. |
| INFRA-13 | **Indexer use by clients.** Clients use only indexers from user configuration or, after one-time consent, from `forum.discovery.indexers`. `lessdugin … --audit` cross-checks indexer heads, changes and `inputDigest` against PDSes and local recomputation. The 1 %-per-hour automatic audit and automatic demotion are Reserved (deferred). |

#### 5.6.1 `Host::fetch_blob`

```
fetch_blob(did, cid, max) -> BlobHandle | Err
  require cid is a CIDv1 with codec raw (0x55) and multihash sha2-256
  max  = min(max, declared size from the verified record, BLOB_HARD_MAX_BYTES)       // limits.md §5
  url  = pds(did) + "/xrpc/com.atproto.sync.getBlob?did=" + did + "&cid=" + cid     // the PDS from the DID doc
  stream the body; n += len(chunk) after decompression; if n > max: abort, Err(TooLarge)
     native: write chunks to a 0600 temp file in the cache directory while feeding SHA-256
     JS:     append chunks to a list; at the end build one Blob from them
  digest = native: the streamed SHA-256;  JS: crypto.subtle.digest("SHA-256", await blob.arrayBuffer())
  if digest ≠ cid.multihash.digest: discard; Err(CidMismatch)                       // counts against the host
  native: rename the temp file to blobs/<cid[−2..]>/<cid> (the last two characters shard the directory);
  JS:     keep the Blob in the host's blob storage, keyed by CID
  return BlobHandle { cid, size: n }                                                // sync.md §12.2
```

- The core reads the first ≤ 4,096 bytes with `Host::blob_read` (`BLOB_SNIFF_PREFIX_BYTES`), sniffs the type, checks agreement with the declared type, and decides display (`sync.md` §11.1). `sync.md` §12 is normative for the exact `Host` signatures; this pseudocode shows the security obligations. The host turns a handle into an object URL, a file path, or (for `text/*` bodies only, ≤ 1,000,000 bytes) a string for the renderer.
- Handles are reference-counted by referencing records, and released (and their storage purged) when the last referencing record is hidden or deleted (PRIV-2).

#### 5.6.2 SSRF guard

```
guard(url, purpose) -> PinnedTarget | Err            // purpose ∈ {PdsRead, UserPds, Identity, ForumEndpoint, Media, DevNet}
  1. scheme is "https" (or "wss"); "http"/"ws" only when purpose = DevNet in a --dev build
  2. no userinfo; the host is a DNS name. IP literals are rejected (DevNet: loopback literals allowed)
  3. purpose = ForumEndpoint: no explicit port. Other purposes: any port 1–65535
  4. resolve A and AAAA with the resolver the connection will use
  5. reject if ANY resolved address is in BLOCKED (DevNet: loopback allowed)
  6. connect only to the checked addresses (no second resolution: DNS-rebinding pin)
  7. redirects: UserPds never follows one. Other purposes follow at most 5 (limits.md §7); each Location is
     resolved against the current URL and re-checked from step 1; an https → http change is rejected
```

`BLOCKED` is every address the IANA IPv4 and IPv6 Special-Purpose Address Registries mark as not globally reachable, plus every address that embeds an IPv4 address:

| Family | Ranges |
|---|---|
| IPv4 | `0.0.0.0/8`, `10.0.0.0/8`, `100.64.0.0/10`, `127.0.0.0/8`, `169.254.0.0/16`, `172.16.0.0/12`, `192.0.0.0/24`, `192.0.2.0/24`, `192.88.99.0/24`, `192.168.0.0/16`, `198.18.0.0/15`, `198.51.100.0/24`, `203.0.113.0/24`, `224.0.0.0/4`, `240.0.0.0/4` |
| IPv6 | `::/128`, `::1/128`, `::ffff:0:0/96` (v4-mapped), `64:ff9b::/96` and `64:ff9b:1::/48` (NAT64), `100::/64`, `2001::/23` (includes Teredo `2001::/32`), `2001:db8::/32`, `2002::/16` (6to4), `fc00::/7`, `fe80::/10`, `fec0::/10`, `ff00::/8` |

**In the browser** the guard cannot see addresses. For URLs that come from data (PDS endpoints in DID documents, did:web hosts, forum endpoints, external images), the SPA applies steps 1–3 and also rejects hostnames that are `localhost`, end in `.localhost`, `.local`, `.internal` or `.home.arpa`, or have a single label. The browser's own private-network protections are the remaining defence.

### 5.7 Social (F18–F24)

The rules live in `social.md`; this table lists what they defend against.

| ID | Measure |
|---|---|
| SOC-1 | **Seeded `karma/1`.** Only seeds S (authorities and granted members, minus banned and nullified) start the karma computation. In open mode, becoming an established voter takes upvotes from at least `establishSeeds` (default 2) distinct seeds (F18). Sybil property P7 (restricted to K, scores and other users' visibility) and the pair bound P5 are property-tested. |
| SOC-2 | **Explicit grants only.** Discovery confers no rights. Membership grants (`#access member`) are human decisions; there is no automatic sweep. The batch-grant UI shows each applicant's account age and activity (F19). |
| SOC-3 | **CID-bound positive assertions.** Tier ≥ frontpage, pins and keeps take effect only while the subject's current CID equals the asserted `subjectCid`. A stale positive creates a mod-queue item ("edited since <action>"), and authors are warned before editing such an item (F20). Votes show "N of M votes were cast on an earlier version". |
| SOC-4 | **Backdating-proof bans and restrictions** (F21). A resolved forum-tier ban hides all of the user's in-forum content; a resolved `#restrict` capability hides every in-forum item of that kind by the user, retroactively (`Hidden(Restricted)`, rule 4b). Item-level keeps at a sufficient rank override both; `until` lapses them. The moderation UI says that both are retroactive. |
| SOC-5 | **Locks carry allow-lists.** Comments on a locked post are hidden unless listed in `allowed` or written by an authority. The client refuses to write a lock that would list more than 5,000 comments and tells the moderator to use hide. A forum-wide (forum-uri) lock is Reserved (P2) and has no effect in v1. |
| SOC-6 | **Fail closed.** If an authority repo has never synced and nothing is cached, the view is `degraded`: only content by S derived from the authorities that did sync is shown, under a banner. Unknown is never treated as absent, so moderation never vanishes because a PDS is down (F22). |
| SOC-7 | **Authority lapse and adoption.** Only the current declaration confers authority. `mods rm` runs `mod adopt` first and only then edits the declaration; the CLI enforces the order (`data-model.md` §8.6). `tag:` subjects have author rank 0. modAction updates and removals go through the revision protocol, so the mod log keeps every prior state. |
| SOC-8 | **React limits** (F23): one opinion per (voter, subject), ≤ 32 reacts per opinion, deduplicated by (react, `qkey` of the quote), never by resolved range (`social.md` §9.2); a for and against pair in one opinion cancels both. React gates use K. |
| SOC-9 | **Canonical references and deleted subjects** (F24): non-canonical at-uris invalidate the record; votes on a confirmed-absent subject count toward karma only from ≥ 2 distinct eligible voters, and never toward a score. |
| SOC-10 | **Bounded walks.** Thread depth ≤ 1,000 (deeper nodes re-attach to the root), wiki walks ≤ 1,000 revisions per page, at most 50 comments per author per thread expanded by default. |
| SOC-11 | **Time.** `t_rank` clamps future-dated items to their first-seen time and badges them `[future-dated]`; the causal lower bound means backdating only hurts its author. Notifications use first-seen time, never `createdAt`, so backdating cannot hide an item from them. |
| SOC-12 | **Forum identity checks.** Every fold rule compares the record's `forum` with the forum uri, not only the key's fk, so a 60-bit fk collision never leaks records between forums (`social.md` §20.1 item 23). |
| SOC-13 | **Membership default.** The lexicon default for `forum.membership` is `approval`, and `forum create` always writes the mode explicitly. Open mode carries the L7 label. |
| SOC-14 | **Stale proofs and fetch caps** never alter the fold: stale proofs are excluded (ID-10) and every client truncates identically (INFRA-4). |

### 5.8 Authentication (F25–F28)

| ID | Measure |
|---|---|
| AUTH-1 | **No `transition:generic`**, ever. The default scope is `atproto include:NS.authFull` plus enumerated blob scopes; the fallback, chosen at PAR time on `invalid_scope` before the browser opens, is the enumerated `repo?collection=…` form (`data-model.md` §10.3). If both fail, login fails. |
| AUTH-2 | **Enumerated blob scopes:** `blob:image/* blob:video/* blob:audio/* blob:text/* blob:application/pdf`, never `blob:*/*`. Declared upload MIME types are normalised from the sniffed type (`data-model.md` §8.8) so that `.ogg` and `.m4a` uploads fall inside these scopes. |
| AUTH-3 | **No main passwords.** Password login accepts only the app-password shape `^[A-Za-z0-9]{4}-[A-Za-z0-9]{4}-[A-Za-z0-9]{4}-[A-Za-z0-9]{4}$`; anything else is refused before any network call, and `authFactorToken` is never sent. App passwords are read from an interactive prompt (no echo), stdin, or the `LESSDUGIN_APP_PASSWORD` environment variable, never from argv, and are never persisted (only the resulting session is). |
| AUTH-4 | **Write allowlist (AuthBridge).** Every authenticated request from the SPA, in both hosted and `web` modes, passes a main-thread allowlist: `com.atproto.repo.{createRecord, putRecord, deleteRecord, applyWrites, uploadBlob}`, where every `repo` is the session DID and every `collection` (each op of an `applyWrites`) is one of the 10 `NS` record collections, plus `com.atproto.server.getSession`. Anything else is refused locally. The native CLI applies the same allowlist to its own writes. |
| AUTH-5 | **OAuth flows.** Clients send `response_mode=query`; callback handlers accept `?query` and `#fragment` responses; the SPA handles the callback before the hash router mounts and removes the parameters with `history.replaceState`. PKCE and `state` are always checked, including on the `--no-browser` paste-back path. |
| AUTH-6 | **Browser keys.** DPoP keys are non-extractable WebCrypto `CryptoKeyPair`s stored in IndexedDB (structured clone preserves non-extractability; spike S2 confirms the library does this). Refresh runs under the Web Lock `lessdugin-oauth-refresh:<did>`, because refresh tokens are single-use across tabs. An XSS can then act only while a tab is open, and cannot take a usable credential off the device: the refresh token is DPoP-bound to a key it cannot export. |
| AUTH-7 | **Native sessions** live in the OS keyring (`keyring` crate). The file fallback: a 0700 directory and a 0600 file written with `O_NOFOLLOW` and `O_EXCL` to a temp name, then renamed atomically; lessdugin refuses to read it if it is a symlink or if its mode grants any group or world permission. Refresh runs under an exclusive `fd-lock`, because the CLI, TUI and `web` may run at once. |
| AUTH-8 | **PDS binding of writes.** `Target::UserPds` requests go only to the PDS bound at login and never follow redirects. A DID-document change of the PDS triggers a re-login, never silent re-targeting. Each `PendingWrite` is stamped with its account DID, and the outbox pump refuses to send it with any other account's session (`data-model.md` W9). |
| AUTH-9 | **Write path integrity.** The outbox is serialised per account with one write in flight, and every `swapCommit` comes from a fresh `getLatestCommit` (`data-model.md` §8.2). Blob refs are embedded verbatim from `uploadBlob`; a local CID that differs from the returned `ref.$link` fails the write. |
| AUTH-10 | **Secret hygiene.** `authorization`, `dpop`, `cookie`, `refresh*`, `password`, `code`, `code_verifier`, bridge capabilities and bridge tokens are redacted in every log and error. Sessions are capped by the server (about 14 days for public clients); users are warned 3 days ahead and can re-login inline in the TUI. |
| AUTH-11 | **`logout`** revokes the session at the authorization server when it supports revocation, and deletes it from the keyring or IndexedDB. `logout --purge` also deletes that account's local `state` (drafts, outbox, read marks, subscriptions, bookmarks, mutes, notification state) after a confirmation that names what will be lost. |
| AUTH-12 | **`web` mode keeps no secrets in the browser** (§7). |

### 5.9 Privacy (F29–F34)

| ID | Measure |
|---|---|
| PRIV-1 | **Media autoload policy** (F29). Media loads without a click only if all of these hold:<br>• the author is in S ∪ E;<br>• the item resolves `Visible`;<br>• neither the item nor (for a comment) its root post carries a `sexual`, `nudity`, `porn` or `graphic-media` self-label;<br>• the blob is an inline image type and ≤ 5 MiB (video and audio are always click-to-load);<br>• the author's PDS host serves ≥ 5 participants of the forum, or matches `*.host.bsky.network`, or is on the user's allowlist.<br>Otherwise the user clicks through a notice: "loads from <host>, which will see your IP". |
| PRIV-2 | **No prefetch of hidden or pending content.** Cached blobs are reference-counted by their referencing records and purged when the last one is hidden or deleted. |
| PRIV-3 | **External images** are never fetched by default. A per-forum opt-in (off by default) fetches them with `credentials: 'omit'` and no referrer, ≤ 5 MiB, sniffed as an inline image type, and shows them through a `blob:` URL (`lfm-1.md` §6.4). Natively they pass the SSRF guard. |
| PRIV-4 | **Metadata stripping** by allowlist rebuild, in the core, before upload (§5.9.1). `#media.name` is opt-in, basename only. HEIC is not accepted natively; the SPA converts it through a canvas. |
| PRIV-5 | **History and redaction** (F30). The editor warns that the previous version stays public. It offers "replace without history" (`--no-history`) and "redact revisions" (deleting revision records, so their blobs are garbage-collected). A missing snapshot shows "history redacted", and quote anchoring degrades gracefully. |
| PRIV-6 | **Upload allowlist.** Only the sniffed types JPEG, PNG, WebP, GIF, MP4 (video or audio), WebM, MP3, Ogg and PDF, and spilled `text/markdown`, are uploaded (`data-model.md` §8.8). |
| PRIV-7 | **Other files are uploaded as is.** v1 strips only images and MP4. Before attaching WebM, MP3, Ogg or PDF, the UI warns that embedded metadata (for example PDF author fields or audio tags) is published unchanged. |
| PRIV-8 | **Export** defaults to `--mine`. Forum exports exclude deleted and hidden content. |
| PRIV-9 | **Public-action interstitials** (F34). Before the first use of each public action type (vote, agreement, react, tag vote, join or apply with a note, mod action with a reason), the UI explains that it is public and permanent (L1). The "what's public about me" view and the `sensitive` forum flag are Reserved (P2); the private-ballot tally extension is Reserved (P8+). |
| PRIV-10 | **Private state stays local.** Drafts, read marks, subscriptions, bookmarks, mutes, notification state, tag filters and karma history live in the local `state` store, exportable as JSON. |
| PRIV-11 | **Reading privacy on Jetstream.** The default subscription has no DID filter and filters by fk on the client, so the subscription itself does not reveal which forum you read. |
| PRIV-12 | **Reports.** The MVP offers only `forum.reportContact`. Sealed reports (no forum, subject or recipient fields; padded; published after a random 0–6 h delay) are Reserved (P2), as is the `showReason`-sealed variant. The UI says that a membership `note` and a mod-action `reason` are public. |

#### 5.9.1 Metadata stripping

The output keeps only what the table allows, in the original order; everything else, including any data after the end marker, is dropped. Walks are iterative and bounded (`limits.md` §6). Images whose EXIF orientation is not 1 are first physically rotated and re-encoded (the media pipeline, `sync.md`), which drops all metadata.

| Format | Kept | Dropped (examples) |
|---|---|---|
| JPEG | SOI; SOF*n*, DHT, DAC, DQT, DRI; SOS with its entropy-coded data and RST*n* markers; APP2 segments whose identifier is `ICC_PROFILE\0`; APP14 `Adobe` (it changes colour decoding); EOI | APP0 (JFIF, may hold a thumbnail), APP1 (EXIF, XMP), APP13 (IPTC), other APP*n*, COM, MPF APP2, anything after EOI (MPF images, appended "motion photo" video) |
| PNG | IHDR, PLTE, tRNS, cHRM, gAMA, iCCP, sBIT, sRGB, cICP, mDCV, cLLI, bKGD, pHYs, acTL, fcTL, IDAT, fdAT, IEND | tEXt, iTXt, zTXt, eXIf, tIME, every other chunk, anything after IEND |
| WebP | RIFF header (size recomputed); VP8, VP8L, ALPH, ANIM, ANMF, ICCP; VP8X with its EXIF and XMP flag bits cleared | EXIF, XMP, every other chunk |
| GIF | header, logical screen descriptor, global colour table, graphic control extensions, image descriptors with local colour tables and image data, the `NETSCAPE2.0` application extension (loop count), trailer | comment and plain-text extensions, other application extensions (XMP), anything after the trailer |
| MP4 / MOV (`video/mp4`, `audio/mp4`) | every byte; offsets never shift | every `udta` and `meta` box, at any depth, has its type overwritten with `free` and its payload zeroed |

### 5.10 Supply chain (F35, F36)

| ID | Measure |
|---|---|
| SUP-1 | **Domain hardening** for `7778777.online`: registrar lock, multi-year auto-renew, DNSSEC and CAA. |
| SUP-2 | **Publisher account**: a did:plc account (`lessdugin.7778777.online`) with an offline rotation key. Lexicons are published only at M7, after `goat lex lint`, `goat lex breaking` and `goat lex check-dns`. |
| SUP-3 | **Lexicons are compiled in** and never fetched at runtime. |
| SUP-4 | **Reproducible builds.** The SPA dist and the wasm are reproducible; each release publishes their SHA-256 (in `SHA256SUMS` and the release notes) with a build-provenance attestation. Settings → About shows the running build's hash. Release binaries embed the same SPA dist. `lessdugin verify-site <url>` is Reserved (deferred). |
| SUP-5 | **`lessdugin web` is the recommended path** for users who do not want to trust the hosted site (L5). |
| SUP-6 | **npm/pnpm.** `minimumReleaseAge` ≥ 7 days (10,080 minutes); `onlyBuiltDependencies` is an explicit allowlist that MUST NOT contain any package bundled into the SPA or the engine (today it holds only the devnet's `better-sqlite3`); exclusions from `minimumReleaseAge` MUST NOT cover bundled packages; `--frozen-lockfile`, lockfile-lint and osv-scanner in CI. Dependencies on the XSS-to-token path (React, TanStack, Radix, CodeMirror, Comlink, idb, DOMPurify, the OAuth client) are kept to the listed set. |
| SUP-7 | **Cargo.** `cargo deny` (licences and advisories) and `cargo vet`. Exact `=` pins for the parsing and crypto crates: `k256`, `p256`, `ecdsa`, `cid`, `serde_ipld_dagcbor`, `pulldown-cmark`, `math-core`, `img-parts`, `imagesize`, `ammonia`, `wasm-bindgen`, `jacquard-oauth`. Release binaries are built with `cargo auditable`. |
| SUP-8 | **GitHub Actions** pinned by commit SHA; default `permissions: contents: read`; protected `release` and Pages environments; no `pull_request_target`. |
| SUP-9 | **Image decoding** in any process that holds tokens uses pure-Rust decoders only (PNG, JPEG, GIF, WebP), never AVIF or HEIC C libraries. Decoding is limited to 64 MiB of input, 8,192 pixels per side, 200 GIF frames and a 256 MiB allocation, and runs off the UI thread with a 10 s timeout (`limits.md` §6). |
| SUP-10 | **No runtime code loading.** No `eval`, no `new Function`, no remote scripts (CSP-enforced in the SPA); natively, no plugins and no dynamic libraries loaded from data directories. |

---

## 6. SPA Content Security Policy and response headers

### 6.1 Production policy

Sent as a `Content-Security-Policy` response header by `lessdugin web` and by the hosted site (Cloudflare Pages or Netlify `_headers`, `deploy/site/_headers`), on every response including the worker script:

```
default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; style-src 'self'; font-src 'self';
img-src 'self' blob:; media-src blob:; connect-src 'self' https: wss:; object-src 'none'; base-uri 'none';
form-action 'none'; frame-ancestors 'none'; require-trusted-types-for 'script';
trusted-types lessdugin-md lessdugin-worker dompurify
```

The header value is this text on one line, with each line break above replaced by a single space. There are no reporting directives: there is no server to receive reports.

| Directive | Why |
|---|---|
| `script-src 'self' 'wasm-unsafe-eval'` | the bundle and the engine wasm; no inline script, no `eval` |
| `worker-src 'self'` | the engine worker |
| `style-src 'self'` | no inline styles; tables use alignment classes (F5) |
| `img-src 'self' blob:` / `media-src blob:` | media appears only after CID verification, as object URLs; no remote image or media URL can load (F5, PRIV-1) |
| `connect-src 'self' https: wss:` | PDSes, PLC, DoH, did:web, Jetstream, Constellation, relays and indexers are arbitrary HTTPS hosts |
| `object-src 'none'`, `base-uri 'none'`, `form-action 'none'`, `frame-ancestors 'none'` | no plugins, no base hijack, no form posts, no framing |
| `require-trusted-types-for 'script'`; `trusted-types …` | every DOM injection sink needs a Trusted Type from one of the three policies of SPA-2 |

### 6.2 Development policy

`--dev` builds and the e2e harness against the devnet (ADR 0006) use the production policy with `connect-src 'self' https: wss: http: ws:`. Release builds MUST NOT serve it.

### 6.3 Other headers

| Header | Value | Scope |
|---|---|---|
| `Referrer-Policy` | `no-referrer` | every response |
| `X-Content-Type-Options` | `nosniff` | every response |
| `Cross-Origin-Opener-Policy` | `same-origin` | every response |
| `Permissions-Policy` | `camera=(), microphone=(), geolocation=(), payment=()` | every response |
| `Cache-Control` | `no-cache` | `index.html`, `lessdugin-config.json`, `oauth/*.json` (so that security fixes propagate) |
| `Content-Type`, `Access-Control-Allow-Origin` | `application/json`, `*` | `oauth/*.json` (hosted site only) |
| `Cache-Control` | `no-store` | every `lessdugin web` bridge response (§7.3) |

**GitHub Pages fallback.** Pages cannot send headers, so the policy is delivered as the first element of `<head>`: `<meta http-equiv="Content-Security-Policy" content="…">` with the §6.1 text minus `frame-ancestors 'none'` (which a `<meta>` policy ignores). This loses, compared with the header: `frame-ancestors` (SPA-10 compensates), the policy on the worker script (a worker created from a URL takes its policy from its own response headers), and every header in the table above except the referrer policy (SPA-9's `<meta>` covers it). Pages is therefore a fallback only; the default host is Cloudflare Pages or Netlify.

---

## 7. `lessdugin web` (the zero-trust path)

`lessdugin web` serves the embedded SPA from the local machine, and the CLI keeps the session. The browser holds no long-lived secret: no OAuth tokens, no DPoP keys, no app passwords, and no drafts (they stay in native `state.sqlite`).

### 7.1 Listener

| ID | Rule |
|---|---|
| WEB-1 | The server is axum, bound to TCP `127.0.0.1:<port>` only: never `0.0.0.0`, `::`, or any non-loopback address. |
| WEB-2 | `<port>` defaults to 47778 and is persisted in the config (`web.port`); `--port` overrides it. If the port is busy, `web` exits with an error. It never picks another port, because the port is part of the origin. |
| WEB-3 | Every request (static or bridge) whose `Host` header is not exactly `127.0.0.1:<port>` or `[::1]:<port>` is answered `403` with an empty body, before routing. This defeats DNS rebinding. |
| WEB-4 | Every response carries the §6.1 CSP (the §6.2 policy under `--dev`) and the §6.3 headers. No response carries any `Access-Control-*` header, and `OPTIONS` is answered `405`, so cross-origin preflights fail. |
| WEB-5 | `build.rs` embeds `web/dist`; if it is missing, it embeds a placeholder page that explains how to build it. Release binaries MUST embed the real SPA, whose hash equals the published SPA hash (SUP-4). `lessdugin-config.json` is served as `{"oauth": "bridge"}`. |

### 7.2 Capability and session

```
on launch, and for every further link the process opens or prints:
  cap = base64url_nopad(32 random bytes from the OS CSPRNG)          // 256 bits, 43 characters
  unused += cap
  url = "http://127.0.0.1:" + port + "/#lessdugin-cap=" + cap
  open url in the browser (unless --no-open) and print it to the terminal

POST /bridge/session  {"cap": c}
  if c ∈ unused:  unused −= c; used[c] = token = base64url_nopad(32 random bytes)
                  sessions += token; return 200 {"token": token}
  if c ∈ used:    sessions −= used[c]                                // revoke: the link was used twice
                  warn on the terminal: "a lessdugin web link was used twice; its session was revoked"
                  return 403
  otherwise:      return 403

SPA startup in bridge mode, before the router mounts:
  read lessdugin-cap from location.hash; history.replaceState to "#/"
  POST /bridge/session; keep the token in sessionStorage under "lessdugin-bridge-token"
  on 403: show "this link was already used or has expired; open a fresh link from the terminal"
```

- Capabilities, tokens and the session table live only in the process's memory and die with it. Comparisons are constant-time.
- Further links are minted only by the serving process, on a local action: the TUI's `o` key when the TUI hosts the bridge, or Enter in the `web` terminal. A process that does not serve the bridge cannot mint one.
- A link travels through the browser's argv, where another local user may race to read it (F27). Single use plus revoke-on-reuse turns a successful race into a visible failure for the legitimate tab.

### 7.3 Request rules

Every `/bridge/*` request is checked in this order; the first failing check answers:

| # | Check | On failure |
|---|---|---|
| 1 | WEB-3 Host check | 403 |
| 2 | Method is `POST` (the only exception is `GET /callback`, §7.4) | 405 |
| 3 | `Origin` is present and is exactly `http://127.0.0.1:<port>` or `http://[::1]:<port>` (`GET /callback` is exempt) | 403 |
| 4 | `Authorization: Bearer <token>` names a live session (`/bridge/session` and `GET /callback` are exempt) | 401 |
| 5 | `Content-Type` is `application/json`, except `uploadBlob` (§7.4) | 415 |
| 6 | The body is within its limit (`limits.md` §18) | 413 |

Every bridge response carries `Cache-Control: no-store`. Capabilities and tokens are never logged (AUTH-10). A bearer token can only be attached by script on the bridge's own origin, and the custom `Authorization` header forces a preflight that check 2 and WEB-4 reject, so other origins cannot drive the bridge.

### 7.4 Endpoints

| Endpoint | Rule |
|---|---|
| `POST /bridge/session` | §7.2. |
| `POST /bridge/xrpc/<nsid>?account=<did>` | Forwards one XRPC call through the CLI's session for `<did>`, which must be a logged-in account.<br>• `<nsid>` ∈ the AUTH-4 allowlist; the AUTH-4 `repo` and `collection` checks are re-applied here on the parsed body, whatever the SPA checked;<br>• `uploadBlob`: the body is the raw blob, its `Content-Type` must be a declared type from `data-model.md` §8.8, and its size ≤ the blob limit;<br>• the response is the PDS's status and body, with only the `Content-Type`, `Retry-After` and `RateLimit-*` headers passed through. |
| `POST /bridge/login` | `{"actor": <handle or DID>}`. Starts native loopback OAuth with the redirect URI `http://127.0.0.1:<port>/callback` (this same server), and returns `{"url": <authorization URL>}` for the tab to navigate to. At most one login is pending, and it expires after 10 minutes. |
| `GET /callback` | The OAuth redirect. Valid only for the pending login: `state` and PKCE are checked, the login binding (ID-11) runs, the session goes to the keyring, and the response is a `303` to `/#/`. The tab keeps its `sessionStorage`, because the origin is unchanged. |
| `POST /bridge/kv` | `get_many`, `scan` and `write` on the native `state` store, limited to keys whose first tuple element (`sync.md` §10.3–§10.4) is `out`, `out.budget`, `loc`, `notif` or `meta`. Sessions are never stored in `state`, so they are unreachable here. |
| `POST /bridge/fetch` | Reserved (deferred): an anonymous read proxy for PDSes without CORS, limited to allowlisted `com.atproto.{repo,sync}` GETs behind the SSRF guard. In v1 the route does not exist (404). |

**Residual risk.** In `web` mode the browser keeps the `cache` store (public records the user has read) in IndexedDB on the bridge's origin. A local user who later squats the port can read it, and so learn reading history, but nothing secret (§9).

---

## 8. Hosted static site

| ID | Rule |
|---|---|
| SITE-1 | The canonical site is `https://lessdugin.7778777.online/`, hosted by default on Cloudflare Pages or Netlify, so that §6 is sent as headers. GitHub Pages is a fallback (§6.3). |
| SITE-2 | It ships `lessdugin-config.json` `{"oauth": "hosted"}`, the web and native OAuth client metadata under `/oauth/` (`data-model.md` §10.4), a visible link to the source and licence (AGPL-3.0 §13), and source maps. |
| SITE-3 | Each release's dist is reproducible, and its hash is published (SUP-4). |
| SITE-4 | Self-hosted copies are generated with `xtask site --origin <url>`, which rewrites the client metadata for that origin. They are trusted exactly like the canonical site. |
| SITE-5 | The site runs the same engine, policies and allowlists as `lessdugin web`, differing only in where the session lives: the hosted OAuth client keeps non-extractable keys in IndexedDB (AUTH-6). |

---

## 9. Residual risks and deferred measures

| Risk | Status in v1 | Planned |
|---|---|---|
| Equivocation and cloaking by self-hosting participants (F15) | not detected (L10); moderators may sync through an indexer | INFRA-11: Reserved (deferred); fork evidence Reserved (P2) |
| A compromised hosted site serves malicious code (F35) | build hash shown; `lessdugin web` available | `verify-site`: Reserved (deferred) |
| Raids that outpace moderators' write budget (F22) | per-item and per-user actions; `degraded` view when authorities are missing | bulk actions `#banList`, `#raid`, `#banHost`: Reserved (P2) |
| Rate limiting of abusive posters (F21) | advisory only (L4) | `ratelimit/1`: Reserved (P2) |
| Reporter exposure (F32) | `reportContact` only | sealed reports: Reserved (P2) |
| Confusable names from non-moderators (F9) | checks against owner and moderators only | frequent posters: Reserved (deferred) |
| Background proof verification of every displayed item | `verify <ref>` on demand; indexer content needs proofs | `--verify proof` in the background: Reserved (deferred) |
| PDSes without CORS are unreadable from the browser | content shown as unavailable; native clients read it | `/bridge/fetch`: Reserved (deferred) |
| Reading history in the `web` origin's IndexedDB cache (§7.4) | readable by a later port squatter on a shared machine | none planned; documented |
| Metadata in WebM, MP3, Ogg and PDF uploads | warned before upload (PRIV-7) | none planned |
| A `<meta>` CSP on GitHub Pages | no frame-ancestors, no worker policy, no other headers | default host sends headers (SITE-1) |
| A ban hides past content, unlike ForumMagnum | item-level keep | `#access.keep` snapshots: Reserved (P2) |
| PLC directory compromise | trusted for current documents | PLC audit verification, `strict` mode: Reserved (P7) |

---

## 10. Verification

| What | How | Measures |
|---|---|---|
| Terminal escapes | `testdata/vectors/terminal/` escape corpus; snapshot tests assert that no ESC appears in output except our own SGR and OSC 8 sequences; TTY and non-TTY variants of `--json` and `--raw` | TERM-1 to TERM-7 |
| Editor templates | template tests with modeline and `Local Variables:` payloads in the quoted parent; YAML frontmatter with aliases, depth 33 and 65,537 bytes | TERM-8 to TERM-10 |
| Local files | `assert_cmd` cases: absolute, `~`, `..`, symlink escaping the draft directory, FIFO, key file with a non-image type, foreign draft with `--yes` | FILE-1 to FILE-5 |
| XSS and mXSS | `testdata/vectors/{xss,mathml}/`; DOMPurify and ammonia equality over every LFM vector; property test that both sanitisers leave renderer output unchanged | SPA-3, SPA-6 |
| CSP and Trusted Types | Playwright runs every e2e scenario against the built SPA with the §6 headers; any `securitypolicyviolation` event fails the test; a test asserts the exact header text in `deploy/site/_headers` and in `lessdugin web` responses | SPA-1, SPA-2, §6 |
| Raw `href` | CI check that fails on a JSX `href` outside `SafeLink` | SPA-4 |
| Poison pills | the PILL-6 fuzz targets; depth 33/37 JSON and CBOR vectors in `testdata/vectors/limits/`; a crash-loop test that injects a panicking record and checks that exactly that record is quarantined | PILL-1 to PILL-6 |
| Identity | vectors for DoH answers, unidirectional handles, `xn--`, confusables against moderators, did:web with a port, proof with an old document, rev > now + 5 min, stale proof below the PDS head | ID-\* |
| Infrastructure | FakePds fault modes: compressed bombs, oversized pages, cursor loops and regressions, wrong CIDs, slow bodies; mini-Jetstream oversize messages and identity noise; SSRF guard tests for every BLOCKED range, v4-mapped and NAT64 forms, redirects to blocked hosts, and DNS rebinding | INFRA-\* |
| Social | the `social.md` §19 vectors and property tests (sybil, pair bound, keep vs ban, restrict, lock refusal, fail closed with a moderator PDS down) | SOC-\* |
| Auth | main-password shapes refused; argv password refused; keyring file fallback refuses mode 0644 and symlinks; AuthBridge allowlist and `repo`/`collection` checks; `response_mode=query`; parallel CLI refreshes never break a session | AUTH-\* |
| Media privacy | fixtures with EXIF and GPS, IPTC, XMP, MPF, appended video, PNG text chunks, WebP EXIF, GIF comments, MP4 `udta` GPS; orientation preserved after rotation | PRIV-4, §5.9.1 |
| Bridge | Host and Origin mismatches, missing or wrong token, GET on a POST route, OPTIONS, double use of a capability (revocation), disallowed NSID or collection, `repo` of another DID, body limits | §7 |
| Supply chain | CI jobs for pnpm, cargo deny and vet, osv-scanner, action pinning lint, reproducible-build check of the SPA and wasm | SUP-\* |

---

## 11. Traceability

| Critique | Measures |
|---|---|
| F1 | TERM-1 to TERM-7 |
| F2 | TERM-8 to TERM-10 |
| F3 | FILE-1, FILE-2 |
| F4 | FILE-3 to FILE-5 |
| F5 | SPA-1, SPA-3, SPA-5, SPA-7, SPA-8, SPA-9, §6 |
| F6 | SPA-6 |
| F7 | SPA-4 |
| F8 | PILL-1 to PILL-6 |
| F9 | ID-3 to ID-7 |
| F10 | ID-1, ID-2, ID-11 |
| F11 | ID-9, ID-10 |
| F13 | INFRA-1, INFRA-2, §2.1 |
| F14 | INFRA-3 to INFRA-6, `limits.md` |
| F15 | INFRA-11, L10 |
| F16 | INFRA-7, INFRA-8 |
| F17 | INFRA-12 |
| F18 | SOC-1 |
| F19 | SOC-2 |
| F20 | SOC-3 |
| F21 | SOC-4, SOC-5, L4 |
| F22 | SOC-6, §9 |
| F23 | SOC-8 |
| F24 | SOC-9, SOC-10, SPA-11 |
| F25 | AUTH-1 to AUTH-4 |
| F26 | AUTH-6 |
| F27 | §7, AUTH-12 |
| F28 | AUTH-7, AUTH-8, AUTH-10, AUTH-11 |
| F29 | PRIV-1 to PRIV-3, L2 |
| F30 | PRIV-5, L9 |
| F31 | PRIV-2, PRIV-8, INFRA-12, L3 |
| F32 | PRIV-12 |
| F33 | PRIV-4, PRIV-7, §5.9.1 |
| F34 | PRIV-9, L1 |
| F35 | SUP-1 to SUP-5, §8 |
| F36 | SUP-6 to SUP-10 |
| Completeness review P0.3 (stale proofs) | ID-10, SOC-14 |
| Completeness review P0.4 (blobs in wasm memory) | INFRA-6, §5.6.1 |
| Completeness review P0.5 (fetch caps) | INFRA-4, SOC-14 |
| Completeness review P0.7 (outbox races) | AUTH-9 |
| Completeness review P2 (login issuer, Jetstream noise, audio MIME, `response_mode`, CSP header) | ID-11, INFRA-5, AUTH-2, AUTH-5, §6 |

---

## 12. Decisions made while writing

These fill gaps in the sources or resolve ambiguities. Each takes the simplest safe option, and all are open to review before M7.

1. **A third Trusted Types policy, `lessdugin-worker`** (SPA-2, §6.1). Under `require-trusted-types-for 'script'`, the `Worker` constructor is a Trusted Types sink, so the synthesis policy list (`lessdugin-md dompurify`) would make `new Worker(new URL(…))` in `packages/engine/src/client.ts` throw. The policy accepts only the build's own worker URLs. This is the only change to the synthesis CSP, and `deploy/site/_headers` must be updated to match.
2. **The development CSP adds `ws:` as well as `http:`**, because the devnet's mini-Jetstream is a plain `ws://` endpoint.
3. **The other response headers** (`X-Content-Type-Options`, `Cross-Origin-Opener-Policy`, `Permissions-Policy`) are taken from `deploy/site/_headers` and made normative, with `no-cache` on the entry files and `no-store` on bridge responses. No reporting directive is used, because there is no server to receive reports.
4. **Deferred items without a phase** (equivocation recording, `verify-site`, background `--verify proof`, `/bridge/fetch`, UTS #39 beyond owner and moderators, automatic Jetstream DID-filter switching, automatic indexer audit and demotion) are marked "Reserved (deferred)" and listed in §9.
5. **Bridge details the synthesis left open:** the fragment parameter name `lessdugin-cap`; the capability is single-use and is exchanged for a separate per-tab bearer token; reusing a capability revokes the session minted from it; further links are minted only by the serving process on a local action; `GET /callback` is the one non-POST route and is exempt from the Origin and bearer checks; failure status codes; `/bridge/xrpc` carries the NSID in the path and the account in the query, and re-applies the AuthBridge checks server-side; `/bridge/kv` is limited to the `state` prefixes; one pending login, expiring after 10 minutes.
6. **SSRF guard.** BLOCKED is the IANA "not globally reachable" set plus every IPv4-embedding IPv6 form; any blocked address rejects the whole name; at most 5 redirects for anonymous requests. Forum endpoints need the default port, PDS endpoints may use any port. The browser applies a hostname-only subset.
7. **Metadata stripping.** JPEG keeps APP14 `Adobe` (needed to decode Adobe CMYK and YCCK images correctly) and drops APP0 (JFIF may carry a thumbnail) and anything after EOI (motion-photo video, MPF images). PNG, WebP and GIF use explicit allowlists. MP4 neutralises only `udta` and `meta`, as in the synthesis. Other uploadable types are not stripped in v1 and carry a warning (PRIV-7).
8. **Autoload labels.** A comment inherits the self-labels of its root post, because comments have no `labels` field.
9. **TOFU pin.** The pin is the authorization server issuer per account DID; a different issuer at a later login needs an explicit confirmation rather than a hard failure, because PDS migration is legitimate.
10. **Names the synthesis left open:** the app-password shape `[A-Za-z0-9]{4}` × 4 joined by `-`; the environment variable `LESSDUGIN_APP_PASSWORD`; the refresh Web Lock `lessdugin-oauth-refresh:<did>`.
11. **Jetstream oversize in the browser:** drop the message and fail over to the other endpoint, since a browser cannot cap a WebSocket message before receipt.
12. **Poison-pill quarantine** retries suspects one at a time before quarantining, so a crash in a large batch quarantines only the culprit.
13. **Supply chain.** `onlyBuiltDependencies` is an explicit allowlist, not an empty list, because the devnet needs `better-sqlite3` (ADR 0006). Neither it nor the `minimumReleaseAge` exclusions may cover bundled packages.
14. **Moderation is a view** (L11) and **the PDS operator can write as you** (L6) are added to the inherent limits; both follow from atproto and were implicit in the synthesis.
15. **The web origin's cache** (§7.4) is an accepted residual risk: the synthesis moved only `state` behind the bridge.
16. **Consistency review.** Aligned with the documents that define the procedures: INFRA-4 now falls back to a full-collection listing on a range-cursor violation (`sync.md` §5.3) instead of ending as `[partial]`; §5.6.1 uses `sync.md` §12's `BlobHandle {cid, size}`, sniffing through `blob_read`, and the `<cid[−2..]>` directory shard; SOC-8 deduplicates reacts by `qkey`, never by resolved range (`social.md` §9.2); `/bridge/kv` names its allowed keys by first tuple element; PILL-2 points at validity rule G10. PRIV-1 (only `Visible` items autoload) was kept, and `sync.md` §11.3 was changed to match it.

**Open questions**
- **`pnpm-workspace.yaml`** excludes `@atproto/*` from `minimumReleaseAge`. That pattern also matches `@atproto/oauth-client-browser`, which is bundled into the SPA and sits on the token path, so it conflicts with SUP-6. The exclusion should be narrowed to the devnet packages (`@atproto/dev-env` and its dependencies).
- **Worker Trusted Types sink.** Decision 1 should be confirmed by the M2 Playwright run (Chromium enforces Trusted Types; Firefox and WebKit may not yet, and then the policy is unused there).
- **PWA manifest.** If the Safari mitigation (S11) ships a web app manifest, the CSP needs `manifest-src 'self'`, added in the same change to `_headers` and `lessdugin web`.
- *(Resolved in the consistency review: a record value deeper than 32 that can still be decoded is invalid under `data-model.md` G10 and is stored as an invalid version; a response deeper than 36 is rejected whole and changes nothing, `sync.md` §5.5.)*
