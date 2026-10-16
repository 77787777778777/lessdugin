##### CRITIQUE: SECURITY #####
# Red-team review: security, privacy and abuse in the lessdugin v1 designs

Severity scale:
- **CRIT**: one cheap actor can compromise many users, or the forum's integrity.
- **HIGH**: serious harm that is cheap to cause, or one step away.
- **MED**: needs preconditions, or the harm is bounded.
- **LOW**: hygiene.

References use the form DOC §n, where DOC is PROTOCOL, SYNC, SOCIAL or ARCH.

## 0. Priority order (fix before lexicons freeze; most fixes change lexicon shape)

| # | ID | Sev | One-liner |
|---|---|---|---|
| 1 | F18 | CRIT | PROTOCOL karma/1 has no seeds. In open mode with default gates of 0, a sybil farm gets unlimited score and karma. |
| 2 | F19 | CRIT | SYNC's automatic mod sweep puts discovered DIDs on the roster, and SOCIAL treats roster members as seeds. Sybils become seeds. |
| 3 | F1 | CRIT | Terminal escape injection: no doc sanitises text for the CLI, TUI, pager or JSON-to-TTY output. |
| 4 | F25/F26 | HIGH | `transition:generic` fallback, main-password login, and exportable DPoP keys in localStorage. Any XSS or bug becomes a durable full-account takeover. |
| 5 | F8 | HIGH | Poison-pill records: one comment can crash-loop the SPA worker permanently. |
| 6 | F13 | HIGH | Unverified hint *content* is shown by default, so any indexer can put words in anyone's mouth. |
| 7 | F29 | HIGH | `cid:` media on self-hosted PDSes acts as a tracking pixel. Prefetch-all also stores illegal content on readers' disks. |
| 8 | F20 | HIGH | Bait-and-switch: curation, pins and co-author consent are not bound to a CID. |
| 9 | F21 | HIGH | Bans, locks and rate limits can be bypassed by backdating, for every cold-sync client. |
| 10 | F3/F4 | HIGH | Local-file exfiltration through `![](path)`; attachment `open` with an attacker-chosen name or extension. |
| 11 | F30 | HIGH | Public verbatim revision snapshots make redaction impossible. |
| 12 | F35 | HIGH | The hosted SPA, the domain and the lexicon publisher account are a single point of compromise. |

## 1. Cross-doc conflicts that are really security decisions

| Topic | Conflicting positions | Pick |
|---|---|---|
| Karma | PROTOCOL §7 has no seeds. SYNC D7 uses checkpoints. SOCIAL §8 is staged and seeded. ARCH §1.4 uses a fixed-point iteration. | SOCIAL karma/v1, hardened (F18). Reject the fixed point: convergence is not guaranteed and it is hard to vector-test. |
| Roster | PROTOCOL §5.13 uses `#member` per DID. SYNC §5 uses auto-swept pages. SOCIAL §4 requires mutual consent plus a grant. | Discovery pages confer no rights. Explicit grants confer trust (F19). |
| Hint display | SYNC §2 has `show_unverified=true` by default. ARCH §4.3 shows hints as "unconfirmed". | Hints may only show skeletons (F13). |
| Scope fallback | ARCH §1.11 falls back to `transition:generic`. PROTOCOL §8 uses an enumerated list. | PROTOCOL's list, and never `transition:generic` (F25). |
| Reports | PROTOCOL §6 has `forum!` and `recipients!`. SOCIAL §18 has no forum or subject. | SOCIAL, with anonymous stanzas (F32). |
| External images | PROTOCOL §4.2 says they are not fetched. ARCH §11.4 CSP has `img-src https: data:`. | Drop `https:` and `data:` (F5). |
| Inline body / blob size | PROTOCOL §3 `textBlob` is 10 MB. SYNC #9 caps inline at 64 KB. | `maxSize` ≤ 1 MB, decided now, because limits are permanent (PROTOCOL §0.7). |
| Future-dated records | PROTOCOL §0.6 and SYNC §4 hide until the time. SOCIAL §2 pins to `t_obs`. | SOCIAL. Hiding until a future time creates a scheduled-reveal channel and depends on the reader's clock. |
| Session storage | SYNC §10 uses the keyring. ARCH §5 uses a 0600 file. | Keyring by default (F28). |

## 2. Findings

### A. Injection and rendering (TUI, CLI, SPA)

**F1 [CRIT] Terminal escape injection wherever network text reaches a TTY**
- **Where:**
  - ARCH §6: StyledDoc becomes ANSI plus OSC 8.
  - ARCH §9: `less -R`, `--raw`, `--json`.
  - ARCH §10: ratatui.
  - PROTOCOL §4.3 specifies HTML only.
- **Why it is reachable:** lexicon `string` accepts U+001B. JSON `\u001b` decodes to ESC. pulldown-cmark passes ESC through in Text and Code events.
- **Injection vectors** (all are attacker-controlled):
  - titles, bodies, code blocks;
  - `displayName`;
  - `#reactDef.label` and `emoji`;
  - `#media.name`, forum `name`;
  - XRPC error `message` from a hostile PDS;
  - DID-doc `alsoKnownAs` and service strings printed by `resolve` and `doctor`;
  - indexer snippets, Jetstream payloads.
- **Attacks:**
  - OSC 52 (`ESC]52;c;<b64>BEL`) silently replaces the clipboard with `curl …|sh`.
  - OSC 8 makes link text say one URL while the target is another.
  - CSI cursor and erase sequences can forge `[verified]` badges, moderator notices, or a fake `[y/N/e]` publish prompt.
  - Title set plus title report injects input on vulnerable terminals (iTerm2 CVE-2024-38396 class).
  - `less -R` has had filter bypasses (CVE-2022-46663).
  - Over SSH, the user's local terminal is the target.
- **Fix:**
  - Add a `TermText` newtype in `lessdugin-markdown`. It is the only type accepted by StyledDoc spans, the ratatui adapters and the CLI printers, and it is built only by `term_sanitize()`:
    - C0 (except structural `\n` and `\t`), DEL and C1 (U+0080–U+009F) become a visible `^[` or `\u{…}` in a distinct style.
    - U+2028 and U+2029 become a space.
    - Bidi controls (U+061C, U+200E/F, U+202A–E, U+2066–9) are stripped from prose. Inside code they become visible `⟨RLO⟩` markers (Trojan Source; people copy commands from posts).
    - Names, handles and titles also lose zero-width characters (U+200B–D, U+2060, U+FEFF) and are capped in graphemes.
  - Emit OSC 8 only for URLs that passed `safe_href` (F7). Percent-encode bytes outside 0x21–0x7E and `;`, cap length at 2 KB, and print the host dimmed after the text when the text differs from the URL: `text ⟨evil.com⟩`.
  - Sanitise errors, logs (a sanitising `tracing` writer), and `resolve`/`doctor` output.
  - Never set the terminal title from content.
  - Badges go in a fixed column and style, and badge glyphs are stripped from user text.
  - Don't rely on ratatui's partial control-character filtering or on `less`.
  - When stdout is a TTY, `--json` and `--raw` must escape C1 and bidi characters as `\uXXXX`; serde_json passes them raw. When piping, output stays byte-exact.
  - Add an escape corpus at `testdata/vectors/terminal/`. Snapshot tests assert that no ESC appears except our own SGR and OSC 8 sequences.

**F2 [HIGH] Editor-template injection (ARCH §9: quoted parent below a scissors line; YAML-comment errors)**
- **Attack:**
  - The quoted parent sits at the end of the file, which is where vim modelines (last 5 lines) and Emacs `Local Variables:` blocks live.
  - Server-provided error text containing newlines escapes the YAML comment and injects `forum:` or `tags:` keys into the frontmatter.
- **Fix:**
  - Put at least 6 fixed lines after any quoted text.
  - Prefix quoted lines with `# `, and neutralise `vim:`, `vi:`, `ex:`, `-*-` and `Local Variables`.
  - Write error comments as single sanitised lines.
  - Create templates 0600 inside a 0700 directory.
  - Turn on serde-saphyr's alias and budget limits for imported drafts (billion-laughs).

**F3 [HIGH] Local-file exfiltration through draft image paths (ARCH §9: "local image paths … go through the media pipeline at publish time")**
- **Attack:** `lessdugin post downloaded.md` (crossposting or RSS import), or a pasted quote, contains `![](../../.ssh/id_ed25519)` or `![](/home/u/Pictures/passport.jpg)`. The file is uploaded and published publicly and permanently, under a signature.
- **Fix:**
  - Resolve only relative paths that canonicalise (after following symlinks) inside the draft's own directory. Reject absolute paths, `~`, `..` and special files.
  - The sniffed type must be in the media allowlist. That alone rejects key files.
  - The pre-publish summary lists every local file with its absolute path, size and type, and `--yes` cannot skip it for drafts not created by lessdugin.

**F4 [HIGH] Attachment download or open (SYNC §11 `blob get … open`, ARCH §9 `media open`, PROTOCOL §3 `#media.name` ≤255b, declared mime)**
- **Attack:**
  - A `name` like `../../.bashrc` gives path traversal on `-o` defaults.
  - Names like `invoice.pdf.hta`, `x.lnk`, `x.js` or `x.scr` are executed by Windows ShellExecute (Windows is a release target, ARCH §14). Similar handler abuse exists elsewhere (`.desktop`, `.terminal`, local `.html`).
- **Fix:**
  - Save as `<cid>.<ext>`, with the extension taken from a fixed mime→extension map of the *sniffed* type, which must agree with the declared type.
  - `name` is display-only (`TermText` or React text).
  - `open` is allowed only for image, video and audio. Everything else is save-only.
  - Set Mark-of-the-Web (`Zone.Identifier`) on Windows. Never go through a shell.

**F5 [MED] SPA CSP and HTML contract gaps (ARCH §11.4, PROTOCOL §4.3, SYNC §11)**
- **CSP:** `img-src … data: https:` and `media-src https:` undo PROTOCOL's no-external-fetch rule. Any rendering bug or React field becomes an IP or tracking leak.
  - Use `img-src 'self' blob:` and `media-src blob:`.
  - External images stay links (opened with `noopener noreferrer`).
  - Add `<meta name="referrer" content="no-referrer">`.
- **Table alignment:** PROTOCOL's `td[style=text-align]` is blocked by `style-src 'self'` when inserted through innerHTML. Use `ta-l`, `ta-c`, `ta-r` classes.
- **Object URLs:**
  - Build with `new Blob([bytes], {type: ALLOWLIST_CONST})`, never the declared string.
  - Exclude SVG from inline display in the MVP. SYNC allows it, but a `blob:` SVG opened in a new tab is a *same-origin document*; CSP inheritance helps, but don't depend on it.
  - Download cards use `application/octet-stream` plus `download`.
- **In-content `#fragment` links** collide with the hash router (`#/settings`, `#/at/…`, and the OAuth callback parsing in `main.tsx`). Rewrite them to `data-anchor` and scroll in a delegated handler. Never assign `location.hash` from content, and reject `#/…`.
- **Ids and classes:**
  - Footnote ids by ordinal (`fn-3`), not by label. Labels can contain quotes; that is an attribute-injection risk in a custom writer.
  - Heading slug charset `[\p{L}\p{N}_-]`.
  - `language-*` class `[a-z0-9_+-]{1,32}`.
- **`data-at-uri`:** re-parse and validate before any use.
- **User text:** every container gets `dir="auto"` and `unicode-bidi: isolate`; names use `<bdi>`.
- **Hover previews:** our collections only, depth 1, same sanitiser.

**F6 [MED] TeX and MathML (ARCH §6 math-core; PROTOCOL §4.1)**
- Disable `\href`, `\url`, `\includegraphics`, `\class`, `\style`, `\html*`, `\def`, `\gdef`, `\newcommand` and `\let`.
- Keep a MathML element and attribute allowlist in Rust: no `href`, `xlink:*`, `src`, `style` or `on*`; no `maction` or `annotation-xml`.
- Pre-scan each expression before parsing: at most 4 KB per expression and brace depth at most 64. A recursive-descent parser will otherwise overflow the stack (F8).
- DOMPurify gets the MathML profile and an mXSS corpus. Keep ARCH's `DOMPurify(html)===html` vector test.

**F7 [MED] URL fields outside markdown bypass the sanitiser**
- **Fields:** `defs#link.uri`, `#externalLink.uri`, `canonicalUrl`, `discovery.*`, `#endpoint.url`, profile links.
- **Why it matters:** lexicon `format: uri` accepts `javascript:`, `data:` and `file:`, and React has historically only warned on `javascript:` hrefs.
- **Fix:**
  - Export one `safe_href()` from core through wasm (https, http, mailto; plus internal `at://` routes).
  - Provide a `<SafeLink>` component, and add a lint rule banning raw `href={…}`.
  - The TUI's `xdg-open` path uses the same function and passes the URL as a single argv entry.

**F8 [HIGH] Poison-pill records cause permanent crash loops (ARCH §8.1: "panics trap… supervisor restarts the worker; all state is in IDB")**
- **Attack:**
  - Deep TeX or markdown nesting (pulldown-cmark is mostly iterative, but our StyledDoc, HTML and P-projection walkers may recurse), deep CBOR or JSON, or a CAR length prefix that triggers a huge `Vec::with_capacity`. Each of these traps wasm or overflows a native stack.
  - The record is persisted in IndexedDB and re-rendered on every start, so that forum is bricked for every SPA user. Native stack overflow cannot be caught.
- **Fix:**
  - All walkers are iterative.
  - Size and depth caps are checked before parsing: CBOR/JSON depth ≤ 32, markdown nesting ≤ 64, TeX per F6.
  - No preallocation from declared lengths.
  - Native wraps per-record work in `catch_unwind`.
  - The supervisor writes "in-flight batch URIs" to IDB before processing. After a crash, those `(uri, cid, RENDERER_VERSION)` are quarantined and shown as "failed to render". Restarts back off exponentially.
  - Fuzz the renderer and validator with unbounded depth.

### B. Identity and impersonation

**F9 [MED] Display-name, link and forum impersonation**
- **Display names:** `displayName` (PROTOCOL §5.12, and the `app.bsky` fallback) can be set to a moderator's name. ForumMagnum relies on unique usernames; we don't have them.
  - Bylines always show the verified handle or a short DID.
  - Run a UTS #39 confusable-skeleton check against the forum's owner, moderators and frequent posters, and warn on a match from a different DID.
- **`@` link text:** `[@alice.bsky.social](https://evil)` is not covered by PROTOCOL §4.2, which only handles `at://`.
  - Apply mention styling only to verified `at://did` mentions.
  - When link text looks like a URL, handle or domain and differs from the href host, show the real host.
- **Punycode:** never decode `xn--` handles.
- **Forum clones:** the header always shows the owner handle, and the directory groups forums by owner.
- **Moderators listed without consent** (PROTOCOL §5.1, SOCIAL §4): an owner can list a famous person as a moderator, which reads as an endorsement. Display the role only after the listed DID publishes an acceptance (for example `.membership{role}`). Authority itself is unchanged.
- **`forum.movedTo`:** require the target forum to declare a reciprocal `movedFrom`, plus user confirmation.

**F10 [MED] Native plain-DNS handle spoofing (SYNC §3, ARCH §5 use hickory with the system resolver)**
- **Attack:** an attacker on café Wi-Fi answers `_atproto.alice.com` with their own DID. Their DID doc claims `alice.com`, so the bidirectional check passes. The attacker is then shown as alice, `read alice.com/slug` opens the attacker's forum, and `login alice.com` goes to the attacker's authorisation server (password phishing).
- **Fix:**
  - Native uses DoH by default, or a DNSSEC-validating resolver. Answers from plain DNS alone are marked `dns-unauthenticated`.
  - Pin handle→DID→PDS/authorisation server per account on first use (TOFU), and show the authorisation server host on first login.
  - Accept a DID for login.

**F11 [MED] Stale DID doc plus a rotated key lets third parties forge `[verified]` (SYNC §3: 7-day doc TTL; SYNC D10 `verify_bundle`)**
- **Attack:**
  - After a key compromise and rotation, the attacker keeps the old key and feeds forged signed commits through an indexer or mirror. Clients with cached docs show them as `[verified]` for up to 7 days.
  - A commit with a far-future `rev` also freezes the repo: later honest commits look like rollbacks.
- **Fix:**
  - Proofs from third parties earn `[verified]` only if the DID doc was fetched from PLC or did:web within the last hour. Otherwise show `[pds]` or hint until refreshed.
  - Reject `rev > now + 5 min`.
  - Never advance the stored rev beyond the head observed at the PDS on the strength of a third-party bundle.

### C. Untrusted infrastructure (PDS, indexer, Jetstream)

**F13 [HIGH] Hint content is displayed by default (SYNC §2; PROTOCOL §9 `#recordRef.value`; ARCH §12 `hydrate`)**
- **Attack:**
  - A forum-declared or user-configured indexer, or Jetstream, serves fabricated records with self-consistent CIDs. They render as the victim's post or comment, or as a fake moderator notice, under a tiny `[hint]` badge.
  - Accelerated mode's first paint is built entirely from this.
- **Fix:**
  - Hints may drive scheduling, ordering and counts, and fill "N items loading" skeletons.
  - Any text attributed to a DID requires L1 or better. A point fetch is one request.
  - The only exception is per-indexer explicit trust ("treat like a PDS").
  - Links inside hint text are never clickable.
  - The fold explicitly ignores `#postView.state`, `tier` and similar indexer fields (SYNC I7 already covers moderation).

**F14 [MED] Resource exhaustion from lying PDSes and huge repos**
- **Where:** ARCH §4.3's `listRecords` path has none of SYNC §5 and §9's caps.
- **Attacks:**
  - Cursor loops.
  - 100 × 1 MB records per page.
  - gzip or zstd bombs.
  - tungstenite's default 64 MiB messages.
  - SYNC §7 `maxMessageSizeBytes: 0` (unlimited).
  - `response.arrayBuffer()` in JsHost, which ignores caps.
  - A 10 MB `textBlob` passed through quote fuzzy-matching at O(n·m).
  - One repo flooding a thread.
- **Fix:**
  - Put every cap in one `limits.rs` with vectors.
  - Count bytes while streaming (reqwest chunk loop; `ReadableStream` reader in JS), measured after decompression.
  - Cursors must strictly advance.
  - Apply SYNC's per-repo and per-forum record caps on both sync paths.
  - Jetstream and WebSocket messages ≤ 1.2 MB.
  - `textBlob` `maxSize` ≤ 1 MB.
  - Fuzzy matching only when the text is ≤ 200 KB and the quote ≤ 2 KB, using a banded algorithm.
  - Show at most 50 items per author per thread; collapse the rest.
  - Clamp aspect ratio to between 1:20 and 20:1.
  - Lazy-load the up to 256 custom react icons.

**F15 [MED] Equivocation and cloaking**
- **Attack:** a self-hosting participant holds their own signing key. They can serve different signed commits to different IPs, for example a benign version to moderators and spam to everyone else.
- **Fix:**
  - Clients and the indexer record `(did, rev, commit cid)` and compare with Jetstream's relay-observed `commit.{rev,cid}` (sync 1.1 chain) and with indexer `getRepoHeads`.
  - Two valid commits at the same rev, or a broken `prevData` chain, is transferable fork evidence. It can be cited in `#hide{reason: equivocation, evidence blob}`.
  - Moderators can sync via an indexer or a proxy to defeat IP-targeted cloaking.

**F16 [MED] Fan-out and reflection amplification**
- **Where:**
  - SOCIAL §1 reference closure.
  - PROTOCOL §4.2 mention display, which needs a resolve per mentioned DID.
  - Hover previews.
  - Up to 128 `mentions` and 256 `links` per body.
  - Forum `discovery.*` (PROTOCOL §5.1) and SYNC §4 `mediaMirrors`.
- **Attacks:**
  - A comment with 128 `did:web:<rand>.victim.com` mentions turns 1k readers into 128k requests to the victim (DDoS).
  - Attacker-owned did:web hosts get read receipts and reader IPs.
  - An owner points every client's relays or indexers at a victim host or a logging host.
- **Fix:**
  - Reference closure only follows DIDs already in the participant set. References to anyone else cannot be valid in-forum subjects anyway.
  - Resolve non-participant identities lazily on hover or click, showing a short DID until then.
  - Forum-declared endpoints need per-endpoint user consent, https, and a hostname (no IP literals, no localhost, no non-default port). They remain hints.
  - Budgets per hostname and per registrable domain.

**F17 [MED] Indexer specifics (PROTOCOL §9, SYNC §13, ARCH §12)**
- **Search:** FTS5 MATCH syntax injection (NEAR, column filters, `a*` full scans). Quote each token, cap at 16 terms, and require prefix terms of at least 3 characters.
- **Snippets:** return plain text plus highlight ranges, never HTML.
- **Quotas:** per-DID records and bytes, per-PDS-host limits. Index only forum participants and forum declarations.
- **Takedown:** an operator blocklist (did, uri, cid, host) applied at query time. Honour relay takedown and deactivation within minutes. Store no blobs by default. Publish a legal contact.
- **SSRF guard:** re-check every redirect hop and pin the resolved IP (DNS rebinding). Block 169.254/16, 100.64/10, fc00::/7 and v4-mapped addresses.
- **Caching:** no `immutable` (F31).
- **Mod queue:** `listModQueue` must not expose `reported` unless reports are meant to be public.
- **Notification endpoints:** rate-limit `listNotifications` and `getActivity` per actor; they are stalking-dashboard primitives.

### D. Social abuse: votes, karma, sybils, time

**F18 [CRIT] PROTOCOL karma/1 has no seeds (PROTOCOL §7; `forum create` writes gates of 0)**
- **Attack:**
  - 1,000 DIDs (a self-hosted PDS is free) each write a membership and a vote, about 4 writes per sybil.
  - Any item gains +1000 score. The attacker's main account reaches k0 ≥ 1000, which means strong-vote power 6 and crossing the author-moderation and sequence gates.
  - Critics can be mass-downvoted into the K < −2 rate limit of 1 comment per day.
  - Forumless personal posts use power 1 for every vote, so they are equally gameable.
- **Fix:**
  - Adopt SOCIAL karma/v1 (seeds, E, KP, pairCap).
  - Harden it: in open mode, establishment requires upvotes from **≥ 2 distinct seeds** (count distinct seed voters, not the net sum), because LLM-written comments easily earn a single seed upvote.
  - Seeds in open mode are authorities plus `trusted` only (keep `openVoting=false`).
  - Forumless posts show raw counts labelled "unweighted" and never rank in cross-forum feeds.

**F19 [CRIT] Mod sweep turns discovered DIDs into seeds (SYNC D4/§5 × SOCIAL §4)**
- **Attack:**
  - TUI and SPA run the sweep hourly for moderators. It appends every discovered membership that passes the admission policy to the roster pages.
  - SOCIAL's roster-mode seeds are authorities plus participants. So every Constellation-discovered sybil becomes a seed, and the attacker controls karma.
  - Separately, one compromised moderator can roster 1,000 DIDs with a single page write.
- **Fix:**
  - Keep two separate things:
    - `directory` pages: discovery only, auto-swept, confer nothing.
    - grants: an explicit human decision, with a batch UI showing account age and activity.
  - Participants, seeds and E come only from grants.
  - Cap grants per moderator per epoch, counted deterministically by entries. Grants beyond the cap are ignored until an admin co-signs.

**F20 [HIGH] Bait-and-switch through edits**
- **Where:**
  - PROTOCOL §5.13 `subjectCid` is audit-only.
  - SOCIAL §5 promotion, pin and content "keep" are URI-scoped.
  - SOCIAL §16 co-author acceptance is keyed on `postUri`.
  - SOCIAL §6 counts votes regardless of CID.
  - Revision snapshots are a writer convention, so a hostile client simply skips them.
- **Attack:** a post earns curation, a pin, 200 strong votes and a respected co-author's acceptance, then is edited into a scam, defamation or spam. It keeps its tier, its score, and "by X and @respected".
- **Fix:**
  - *Positive* assertions take effect only while `current cid == asserted cid`. That covers tier ≥ frontpage, pin, keep-despite-ban, wiki `approved` and co-author confirmation.
  - On a mismatch:
    - the tier falls back to the default and the pin drops;
    - the co-author is shown as "co-authored an earlier version";
    - the mod queue gets an "edited since curated" item.
  - Negative actions stay URI-scoped.
  - Show "N of M votes were cast on an earlier version".
  - Flag "edited, history unavailable" when votes or comments reference CIDs that have no matching revision.

**F21 [HIGH] Backdating bypasses bans, locks and rate limits (SOCIAL §2, §12, §13 `since`; PROTOCOL `#lock`)**
- **Attack:** a banned user keeps writing with `createdAt` and TID set before the ban. Every client without a live observer shows the content: all first-time SPA visits and all fresh CLI installs. The same works against locks and against rate limits (spread the claimed times).
- **Fix:**
  - A ban hides all of the user's in-forum content by default.
  - "Keep prior content" is a snapshot, `#ban{keep: blob(cbor [uri,cid])}`, and only listed items survive.
  - Locks work the same way: `#lock{snapshot: blob}`. Comments not in the snapshot are hidden regardless of their claimed time.
  - Rate limits are documented as advisory.
  - P2: authority-published first-seen batches (`observation{forum, [(uri,cid,t)]}` from moderator or indexer clients) feed `t_obs` in the fold. Authorities are already trusted, so this stays deterministic.

**F22 [MED] Raid economics and fail-open on moderator outage**
- **Problem:** a moderator gets about 11.6k `putRecord` calls per day (35k points), one record per subject (PROTOCOL §5.13 sk, SOCIAL §5). An attacker's self-hosted PDS has no write limit.
- **Fix, bulk actions:**
  - `#banList{subjects: [did] ≤ 2000}`, keyed by TID.
  - `#raid{until, mode: hide-non-established | hide-non-granted}` as one forum-wide write.
  - `#banHost{host}` for sybil PDS hosts, excluding a shared-host list.
- **Fix, cold clients:**
  - If an authority repo is unreachable and nothing is cached, render only content from authorities and E, with a "moderation data unavailable" banner (fail closed, extending SOCIAL's "unknown is not absent").
  - The forum may declare `modMirrors` that serve proof bundles of authority repos. Accept them only at a rev ≥ the highest rev ever seen, so mirrors cannot roll back bans.

**F23 [MED] React spam (PROTOCOL §2.3/§5.7: the react sk includes the quote)**
- **Attack:** one E user plants 500 distinct inline "wrong" or "strawman" highlights on a single post.
- **Fix:** use SOCIAL §6's single opinion record (≤ 40 reacts, deduplicated by `(react, qkey)`, quotes ≤ 500 graphemes). Otherwise, cap at 10 react records per (actor, subject), chosen by rkey order.

**F24 [LOW] Miscellaneous**
- **Non-canonical URIs:** `did:web:Example.com` vs lowercase, `%3A` ports, or an uppercase NSID authority produce two subject keys for one subject. Reject non-canonical forms at validation and deduplicate on the exact canonical string.
- **Delete to escape downvotes:** deleting downvoted comments resets karma and the "last 20 items" rate rule (SOCIAL §6). Count a missing subject's votes when ≥ 2 distinct E voters reference it; fabrication then needs collusion and is still pairCapped.
- **Wiki chain DoS:** PROTOCOL §5.9's `prev` chain lets one editor force 10k fetches. Cap the walk at 1k and limit revisions per author per page.
- **Window disagreement:** the browser syncs 90 days and native 365 (SYNC §6), so they compute different karma and therefore different gate outcomes. Use checkpoints for gates when available; otherwise mark results `partial`.
- **JS object keys:** keep serde-wasm-bindgen maps as JS `Map` (avoids prototype pollution).

### E. Authentication and secrets

**F25 [HIGH] Scope over-grant and main-password login (ARCH §1.11, §5)**
- **`transition:generic` fallback:** it grants write and delete on *every* collection, including `app.bsky.*` and other apps' records. A lessdugin bug or a compromised hosted SPA could wipe a user's Bluesky presence.
- **Main-password login:** "authFactorToken 2FA prompts" only apply to main-password `createSession`, so ARCH implicitly accepts main passwords. That grants account management (email change, deletion).
- **Fix:**
  - Never request `transition:generic`. If `include:` fails, use PROTOCOL §8's enumerated list; if that fails, error out.
  - Refuse main passwords: require the `xxxx-xxxx-xxxx-xxxx` app-password shape and drop authFactorToken.
  - Take app passwords only by prompt, stdin or environment variable, never argv (shell history, `ps`). Never persist them.
  - Narrow `blob:*/*` to `blob:image/* blob:video/* blob:audio/* blob:text/markdown`. ARCH's list is missing `text/markdown`, which `textBlob` needs.
  - Request interop scopes incrementally, with `action=create` only for shared collections such as `site.standard.*`.
  - Third-party clients use `authInteract`.
  - The main-thread AuthBridge (and the `web` bridge, F27) allows only `com.atproto.repo.{createRecord,putRecord,deleteRecord,applyWrites,uploadBlob}` on allowed collections, plus `server.getSession`.
  - In spikes S1/S2, verify that the OAuth libraries check that the token's `sub`, the DID doc's PDS and the issuer agree.

**F26 [HIGH] Browser DPoP keys are exportable in localStorage (ARCH §8.2)**
- **Fact:** atcute's README lists "no IndexedDB" and "can't use non-exportable keys".
- **Attack:** with `connect-src https:`, any XSS or malicious bundled dependency exfiltrates the DPoP private key plus the refresh token. The attacker can then refresh from their own machine for about 2 weeks and lock the user out.
- **Fix:**
  - Store a non-extractable `CryptoKeyPair` in IndexedDB; structured clone preserves non-extractability. Either use @atproto/oauth-client-browser (I believe it is IDB-backed; confirm in S2) or give atcute a custom store.
  - XSS can then act only while a tab is open and cannot take a durable credential off the device.
  - Use Web Locks around refresh, since refresh tokens are single-use across tabs.

**F27 [MED] `lessdugin web` origin squatting (ARCH §1.9: fixed port 47778)**
- **Problem:** an origin is scheme, host and port, not a process. Another local user or program that binds 127.0.0.1:47778 while lessdugin isn't running owns that origin's localStorage and IndexedDB. That includes the session, and with F26 it can still *use* even non-extractable keys while its page is open. The TUI `o` key opens this URL. On shared machines this is a cross-user account takeover.
- **Fix:**
  - In `web` mode, keep no long-lived secrets in the browser. The CLI holds the session in the keyring and acts as an auth proxy (bring ARCH's phase-2 "engine bridge" forward for auth only).
  - A per-launch 256-bit capability is passed in the URL fragment, exchanged once, and kept in sessionStorage. A local user may race the brief argv exposure, but the capability is single-use.
  - Bridge endpoints are POST-only, use `Authorization: Bearer` (which forces a CORS preflight), check Host and Origin, and enforce the F25 allowlist.

**F28 [MED] Native token hygiene (SYNC §10 vs ARCH §5)**
- Default to the OS keyring. The file fallback uses a 0700 directory, a 0600 file, `O_NOFOLLOW|O_EXCL` with atomic rename, and refuses to read the file if it is group- or world-readable (as ssh does).
- Redact `authorization`, `dpop`, `refresh*` and `password` in `tracing`.
- `Target::UserPds` requests don't follow redirects and go only to the PDS bound at login. A DID-doc change triggers a re-login, not silent re-targeting.
- Stamp each outbox `PendingWrite` with the account DID; the pump refuses a mismatch.
- Add `logout --purge`.

### F. Privacy

**F29 [HIGH] Reader IP leaks via media; illegal-content prefetch (PROTOCOL §3; SYNC §11 "native prefetches thumbnails for the whole window"; SYNC §14)**
- **Attacks:**
  - Any self-hosting participant learns the IP and time of every reader who renders their `cid:` media. A reply aimed at one person is a read receipt that can deanonymise pseudonymous readers or moderators. Sync itself also reveals reader IPs to every self-hosting participant.
  - Prefetch-all writes images from pending or sybil accounts, later-hidden content, and potentially CSAM to users' disks without them viewing anything. That is legal exposure, and the same applies to indexer media and mirrors.
- **Fix:** auto-load media only when all of these hold:
  - the author is in E or granted;
  - the item is visible;
  - it has no `sexual` or `graphic-media` self-label;
  - the PDS host is shared: it serves ≥ N distinct participants, or is on a user allowlist such as `*.host.bsky.network`.
- **Otherwise:**
  - Click-to-load, with the text "loads from <host>, which will see your IP".
  - Never prefetch hidden or pending content.
  - On hide, remove, delete or takedown, purge the blob cache immediately (refcounted by referencing records).
  - "Private reading" uses a proxy or Tor rather than prefetching everything.
  - Indexers store no blobs.
  - The threat model states plainly that reading a forum exposes your IP to self-hosting participants.

**F30 [HIGH] Public revisions defeat redaction (PROTOCOL §0.5/§5.5; SYNC #10; SOCIAL #9)**
- **Harm:** a user accidentally posts their address or a geotagged photo, then edits it out. The verbatim prior version and its blob stay public, signed and permanent, ready for harassers to diff. Mod-log snapshots of reasons have the same problem.
- **Fix:**
  - The editor warns that the previous version stays public.
  - Offer "replace without history" (no snapshot) and "redact old versions" (delete the revision records, so the blobs get garbage-collected).
  - Readers treat a missing snapshot as "history redacted", and quote anchoring degrades gracefully.
  - Mod-log entries can be redacted.
  - Drop `.profile` from the revision union.

**F31 [MED] Deleted content persists (SYNC §13 `immutable` bundles and phase-2 "immutable segments"; ARCH §9 `export [forum]`; blob LRU)**
- **Problem:** signed proofs make any saved copy of deleted content *cryptographically attributable forever*. That is inherent to atproto, but these features amplify it.
- **Fix:**
  - No `immutable` and no long max-age on any response containing record values or blocks.
  - Snapshot segments cover moderator state only.
  - Third-party proofs older than the PDS head are shown as `[stale]` and purged on an absence proof (SYNC I4).
  - Indexers answer absence for deleted URIs.
  - `export` defaults to `--mine`; forum exports exclude deleted and hidden content.
  - Purge blobs by refcount.
  - Document non-repudiation in the UI.

**F32 [MED] Reports and moderation reasons**
- **Reports (PROTOCOL §6):** the `.report{forum!, recipients!}` record publicly links the reporter, the forum and the timing, which in a small forum is enough for retaliation.
  - Use SOCIAL §18's shape: no forum or subject fields.
  - Remove the key-id `recipients` field; age X25519 stanzas are anonymous only without it.
  - Pad to fixed buckets and delay publication by a random 0–6 hours.
  - Prefer the `createReport` service when one is declared.
- **`#hide{public:false}`:** this is misleading because the record is public anyway. Rename it `showReason`, document it, and add a sealed `reasonSealed` in P2.
- **Membership `note`:** it is public; warn in the UI.

**F33 [MED] Gaps in metadata stripping (ARCH §7)**
- **Missed formats and segments:**
  - JPEG: APP13 (IPTC), COM, XMP-extension segments, MPF secondary images.
  - GIF: comment and XMP application extensions.
  - AVIF/HEIF: Exif items.
  - MP4/MOV: `udta` / `©xyz` GPS. ARCH only warns here, but phone video carries location by default.
- **Filenames:** `#media.name` publishes local filenames.
- **Fix:**
  - Use an allowlist approach: rebuild the container keeping only pixel data and ICC.
  - Rewrite MP4 boxes to drop `udta` and `meta`, or require explicit confirmation.
  - `name` is empty by default; opt-in, basename only.

**F34 [LOW] Public social graph UX**
- **Problem:** votes, reacts, memberships, applications, co-author acceptances, bans and reasons are all public and archived (relays, Jetstream replay, Constellation, indexers).
- **Fix:**
  - Show a first-use interstitial for each action type.
  - Add a "what's public about me here" view.
  - Allow a `sensitive` forum flag: wildcard Jetstream with no DID filter, and no third-party discovery.
  - Keep SOCIAL's private-ballot tally extension on the roadmap.

### G. Supply chain

**F35 [HIGH] The hosted SPA and namespace control are a single point of failure (ARCH §11.4, §16 M7)**
- **Problem:** this is the "trusted website" the user wanted to avoid. Whoever controls `7778777.online` (DNS, registrar, Pages) or the publisher account controls:
  - the SPA code, and with it every hosted user's tokens;
  - the OAuth client_id identity;
  - `_lexicon` resolution, including the permission-set text shown on consent screens.
- **Fix:**
  - Registrar lock, multi-year auto-renew, DNSSEC and CAA.
  - The publisher should be did:plc with an offline rotation key.
  - Reproducible SPA and wasm builds, published hashes, and `lessdugin verify-site <url>`. Show the build hash in settings.
  - Make `lessdugin web` the recommended path.
  - Keep lexicons compiled in; never fetch them at runtime.
  - Warn when an owner or moderator uses did:web, since domain expiry means forum takeover.

**F36 [MED] npm, cargo and Actions hygiene**
- **npm:**
  - pnpm `minimumReleaseAge` ≥ 7 days.
  - `onlyBuiltDependencies: []` (no install scripts).
  - lockfile-lint and osv-scanner.
  - Trim dependencies: radix, TanStack, CodeMirror, comlink and idb are all on the XSS→token path.
- **Cargo:**
  - cargo-vet for parsing and crypto crates: k256, p256, cid, serde_ipld_dagcbor, pulldown-cmark, math-core, img-parts, imagesize.
  - `=` pins for those crates; `cargo auditable` for releases.
- **Actions:**
  - Pin actions by SHA, including taiki-e.
  - Default `permissions: contents: read`.
  - Protected environments for release and Pages.
  - No `pull_request_target`.
- **TUI image decoding:**
  - Pure-Rust decoders only (png, zune-jpeg, gif, image-webp). No AVIF or HEIC decoding (C libraries) in a process that holds tokens.
  - Tighten SYNC §11's limits to 64 MB, 8192² pixels and ≤ 200 GIF frames.
  - Decode off the UI thread with a timeout.

## 3. Spec and lexicon deltas these findings require (before the freeze)

- **`markdown`:** `textBlob` `maxSize` ≤ 1 MB.
- **`modAction`:**
  - Positive actions require `subjectCid` and are effective only while it is current.
  - Add `#ban.keep` and `#lock.snapshot` blobs.
  - Add `#banList`, `#raid` and `#banHost`.
  - Rename `#hide.public` to `showReason`.
- **Roster:** separate `directory` (discovery) from `grant` (trust).
- **`.report`:** drop `forum` and `recipients`.
- **`revision`:** drop `.profile` from the union, and specify that missing snapshots mean "redacted".
- **`#media.name`:** optional and display-only.
- **Co-author:** acceptance is CID-bound.
- **Moderators:** an acceptance record is required for display.
- **Policy:** karma/v1 per SOCIAL, plus `establishDistinctSeeds` (default 2 in open mode).
- **New normative docs:**
  - `docs/spec/threat-model.md`: trust tiers, IP exposure, non-repudiation, best-effort timing, hosted vs local SPA.
  - `docs/spec/limits.md`.
  - `testdata/vectors/{terminal,xss,mathml,limits}`.
- **Auth policy:**
  - No `transition:generic`.
  - No main passwords.
  - Keyring by default.
  - Non-extractable IDB DPoP keys.
  - AuthBridge allowlist.
  - `web`-mode bridge with no browser-held secrets.

Sources:
- [atcute oauth browser-client README](https://github.com/mary-ext/atcute/tree/trunk/packages/oauth/browser-client)
- [InfoQ: DPoP key storage](https://infoq.com/articles/dpop-key-storage-unsolved-problem)
- [IETF DPoP draft-09](https://datatracker.ietf.org/doc/html/draft-ietf-oauth-dpop-09)

### Critical Files for Implementation
These are planned files; the repo currently holds only `LICENSE`.
- /home/user/lessdugin/crates/markdown/src/term.rs and /home/user/lessdugin/crates/markdown/src/sanitize.rs (`TermText`, `safe_href`, MathML/TeX allowlist, depth pre-scan)
- /home/user/lessdugin/crates/core/src/social/karma.rs and /home/user/lessdugin/crates/core/src/social/participation.rs (seeded karma/v1, separating directory from grant)
- /home/user/lessdugin/crates/core/src/limits.rs (all byte, depth, count and fan-out caps, plus the poison-pill quarantine)
- /home/user/lessdugin/crates/native/src/auth/mod.rs and /home/user/lessdugin/packages/engine/src/auth-bridge.ts (scope policy, keyring, non-extractable IDB keys, method allowlist, `web` bridge)
- /home/user/lessdugin/docs/spec/threat-model.md (with the `modAction`, `report` and `markdown` lexicon deltas under /home/user/lessdugin/lexicons/online/7778777/lessdugin/)

##### CRITIQUE: PROTOCOL #####
# Atproto protocol feasibility review of the lessdugin designs

**Verdict.** The overall architecture is feasible on today's atproto. The blockers are these:
- **Edit semantics.** `applyWrites` has no per-op swap and no upsert.
- **OAuth blob scope.** It is wrong in the ARCH doc.
- **"Immutable" revision and wiki chains.** Atproto records are always editable and deletable by their author.
- **Frozen limits.** Lexicon limits can never be added later, and many fields have none.
- **Rkeys.** Four incompatible rkey derivations must be reduced to one before publishing. Rkey types and limits are permanent.

The rest is medium or low.

## 1. Claims I verified (safe to build on)

- **Lexicon spec**
  - A union may point to `object` or `record` definitions. So PROTO §5.5 `revision.record: union[.post, …]` is valid.
  - The spec says designers are "strongly recommended to not use `unknown` fields in record objects", so PROTO is right to avoid it. The only risk is codegen tooling.
  - Evolution rule, verbatim: "Size and length limits (eg, `maxLength`) can not be added, removed, or changed". This is stricter than the research note ("never tighten"). PROTO §0.7 and §13 state it correctly.
- **Permission sets**
  - A set may cover its own group plus child groups only; no siblings or parents.
  - No `blob`, `account` or `identity` permissions inside a set, and no wildcards in a set.
  - An unresolvable set that is not cached makes the auth request fail.
  - Authorization servers cache sets about 24h. On token refresh, the session's permissions may be updated to the current set, so adding collections later reaches existing sessions.
  - PROTO §8 is correct, and `authFull` also covers the `.index.*` child group.
- **NSID and DNS**
  - The NSID is valid: only the TLD segment may not start with a digit.
  - `_lexicon.lessdugin.7778777.online` and `_lexicon.index.lessdugin.7778777.online` are the correct TXT names.
- **OAuth spec**
  - Loopback `client_id` is exactly `http://localhost`: no port, path empty or `/`. Redirects go to `127.0.0.1` or `[::1]`, with the port ignored when matching. `scope` defaults to `atproto`. Servers are "encouraged", not required, to support this.
  - Native clients may use an `http://127.0.0.1` redirect.
  - A native client's HTTPS redirect must have the same origin as the `client_id`. Web redirects have no same-origin rule.
  - Public clients: session and refresh token both "should" be ≤2 weeks. The reference provider has `SESSION_LIFETIME = REFRESH_LIFETIME = 2 weeks`; only confidential clients get the extended 2y / 3mo values.
  - The client must check that the `sub` DID's PDS is bound to the authorization server.
- **Repo XRPC**
  - `sync.getRecord`, per its own lexicon, proves "existence or non-existence".
  - `applyWrites` has only a top-level `swapCommit`. Its output has optional `commit{cid,rev}` and `results[{uri,cid}]`.
  - `listRecords` accepts only limit 1–100, `cursor` and `reverse`. The `rkeyStart`/`rkeyEnd` parameters are gone from the lexicon.
  - `getBlocks` takes an unbounded `cids[]` and fails with `BlockNotFound`.
- **Reference PDS source (blobs)**
  - Blob refs are found generically, including in unknown-lexicon and nested records.
  - A blob referenced by any create or update in the same `applyWrites` batch is kept (`cidsToKeep`). So verbatim snapshots keep old media alive.
  - `verifyBlob` compares the ref's `mimeType` **and** `size` with the stored blob.
- **Probes I ran today**
  - Bluesky PDS: CORS `*`, `ratelimit-policy: 3000;w=300`, RateLimit-* headers exposed.
  - bsky.social authorization server metadata: CORS `*`, `allow-headers: Content-Type,DPoP`; `response_modes_supported: query, fragment, form_post`; PAR required.
  - Other CORS `*` endpoints: `plc.directory`, `*.bsky.social/.well-known/atproto-did`, and `cardyb.bsky.app`.
- **Subject-key vectors.** The PROTO §2.3 sk/1 vectors recompute exactly: `ncepyaoj3bijw6li`, `q2swgvbea4gb2m6c`, `mbibtewaiedhx3gd`, `bawazc54xxhjkhdy`.

## 2. Incorrect or infeasible claims, by impact

### High

- **H1. ARCH §4.4 "Edits use swapRecord; conflict → WriteState::Conflict" cannot work. This also affects PROTO §2.4 and §5.5.**
  - An edit is an `applyWrites` call (snapshot create plus update), and `applyWrites` supports only batch-level `swapCommit`.
  - Any unrelated write to the repo between `getLatestCommit` and `applyWrites` makes the call fail with `InvalidSwap`. That includes the user's Bluesky likes, our own outbox votes, or another device. Heavy Bluesky users will hit this routinely.
  - **Correction:** on `InvalidSwap`, re-read the target record.
    - If its CID is unchanged, rebuild the snapshot and retry (bounded, with jitter).
    - If its CID changed, report a real conflict.
  - `applyWrites` also has **no upsert**. A `#create` on an existing rkey, or an `#update` on a missing one, fails the whole batch. This affects:
    - deterministic-key records written in batches ("membership beacon + first post", `mod adopt` in 200-op batches);
    - any batch whose contents depend on whether a record already exists.

    Either check existence first, or write those records with `putRecord`. `putRecord` has a per-record `swapRecord`, where null means "must not exist".
- **H2. ARCH §1.11 and §5 request `blob:image/* blob:video/*`.**
  - With only those scopes, these uploads fail:
    - PROTO §3 `textBlob` (`text/*`);
    - ARCH's own §1.13 snapshot stored as a `text/markdown` blob;
    - SYNC §8 checkpoint (`application/cbor`);
    - `defs#media` attachments declared as `*/*`, and audio.
  - Blob permissions cannot be placed in a set.
  - **Correction:** request `blob:*/*` directly, as PROTO §8 does. Every scope variant the client may request must appear in the metadata `scope`, and every variant must start with `atproto`. ARCH's fallback chain omits `atproto`.
- **H3. The "immutable" revision and wiki rules can be broken by the authors of the records they depend on (PROTO §5.9; SOCIAL §15).**
  - PROTO §5.9: candidates need a `prev` chain whose CIDs match. SOCIAL §15: an edge counts only if the parent's strongRef CID equals the parent's *current* CID.
  - Atproto records are always editable and deletable by their author. Under both rules, the author of any ancestor revision (typically the genesis author) can edit or delete it. That invalidates every descendant revision by every other editor: the page disappears or reverts.
  - PROTO's tag discussion, which hangs off the creation revision, is orphaned the same way.
  - **Correction:**
    - Every revision already carries full content, so a head's validity must not depend on its ancestors being intact.
    - Resolve ancestry by URI. A CID mismatch or deletion is shown as "ancestor altered", not treated as invalid.
    - The page's identity is (forum, slug). Tag discussion should anchor to the forum plus the slug, not to the genesis record.
  - The same caveat applies to `.revision`: history is hash-verifiable but not complete, because authors can delete snapshots. Votes pinned to a CID with no surviving snapshot should render "on an unavailable version".
- **H4. Many fields have no limits, and limits can never be added later.**
  - PROTO leaves many strings unbounded, including `membership`, `role`, `op`, `postPolicy`, `commentPolicy`, `style`, `tier`, `scope`, `voting`, `tagCreation`, `#autoRateLimit.action` and `#rateLimit.kind` (most of the `kv{}` fields).
  - SOCIAL and ARCH specify no limits at all.
  - Every string, array, bytes and blob field needs its final `maxLength`/`maxGraphemes`/`maxSize` at first publication. For knownValues strings, 64–128 bytes is enough.
  - Required sets are also permanent, for example `sequence.chapters!` and `wikiRevision.name!`.
- **H5. ARCH §1.6 and §4.3 assume "skip the repo if the `getLatestCommit` rev is unchanged".**
  - The rev changes on every write in the repo, including all `app.bsky.*` activity. For typical Bluesky users nearly every participant repo is therefore dirty on every poll.
  - Each dirty repo then needs a full `listRecords` of every lessdugin collection: 100 records per page, no range parameters, and deletes only detectable by re-listing everything.
  - The roughly 2k-participant serverless envelope is optimistic as a result.
  - **Correction:**
    - Use Jetstream filtered to `online.7778777.lessdugin.*` as the dirty signal, or SYNC's MST subtree check, which ignores `app.bsky` churn.
    - ARCH's P10 plan, "verified range diffs via `getRepo?since`", uses the wrong primitive. That call returns all blocks of the whole repo since the rev, and deletions cannot be interpreted without the previous MST; SYNC §0.6 is correct about this. Replace P10 with SYNC's `getRecord` + `getBlocks` range sync.
- **H6. SYNC §6's fallback ("100 per page, newest first, stop at the window's lower rkey bound") does not work with `{fk}-{tid}` keys as written.**
  - `listRecords` orders by the full rkey (the reference PDS sorts by uri descending), so the order is forum-prefix-major, not time.
  - There is no range parameter. A single-forum scan works only by crafting `cursor={fk}.`. The reference PDS treats the cursor as an rkey bound, but cursors are opaque per spec, and other PDS implementations may differ.
  - **Correction:** treat it as a reference-PDS optimisation with a runtime check (the first returned key must be ≤ the bound); otherwise do a full collection scan.
- **H7. SYNC §4 and §6 windowing by author-chosen TIDs creates a moderation blind spot.**
  - With comment keys `{fk}-{tid}`, a commenter can backdate the TID outside the 365-day (native) or 90-day (browser) window.
  - Windowed clients never fetch that comment on cold sync, yet still mark the thread `complete`. That includes moderators' clients, because participant repos are windowed.
  - Unwindowed clients, indexers and Constellation users do see it.
  - **Correction:** either
    - key comments as `{fk}-{rootTid}-{tid}`, with readers validating that the middle segment equals the root post's TID, or
    - never window comment ranges of roots that are inside the window.

### Medium

- **M1. SYNC §13 `format=car` cannot work as one method.** An XRPC lexicon method has a single `output.encoding`. Define separate methods (for example `getThreadBundle`, `getRecordsBundle`, `getRepoRanges`) with `application/vnd.ipld.car`.
- **M2. Indexer GET array parameters are too large.**
  - PROTO §9 allows `uris`/`subjects`/`actors` up to 100; SYNC allows `dids` up to 200.
  - Percent-encoded, that is about 13–25 KB of URL, past the common 8 KB proxy and CDN limits.
  - Cap at 25 at-uris (Bluesky's `getPosts` uses 25) and about 50 DIDs, or batch on the client.
- **M3. SYNC §8 `forum.checkpoint` collides with an existing name.** It becomes `online.7778777.lessdugin.forum.checkpoint`, making `forum` both a record and a group. That breaks PROTO §1's naming rule and needs its own `_lexicon` TXT record. Rename it `online.7778777.lessdugin.checkpoint` (or SOCIAL's `karmaSnapshot`).
- **M4. The digit segment `7778777` is valid, but the spec "strongly encourages" alphabetic-only authority segments.**
  - Rust modules cannot start with a digit, so `online::7778777::lessdugin` is invalid. jacquard-lexgen or the xtask generator must mangle it (for example `_7778777`). TS codegen that emits identifiers needs the same treatment.
  - Add every consumer to spike S4: goat lex lint, `@atproto/lexicon`, atcute lex-cli, jacquard-lexgen, Jetstream wildcard, relay `listReposByCollection`, Constellation `source=` parsing.
- **M5. Blob refs must be taken verbatim from `uploadBlob`.**
  - ARCH §4.4 checks only that "returned ref == local CID". The PDS compares `mimeType` and `size` with the blob it stored, and the mime type may be sniffed and differ from what the client declared.
  - Writers must embed the returned blob object verbatim. Local CIDs are for preview only.
- **M6. Proofs fail after key rotation (SYNC §2 and §13; PROTO §9).**
  - Commit signatures verify only against the `#atproto` key that was current when the commit was signed.
  - After a PDS migration or key rotation, cached L2 records and indexer bundles for older revs fail against the current DID doc.
  - SYNC's rule "invalid signature = strike" would penalise honest indexers and drop valid content.
  - **Correction:** classify these as `stale-proof` and re-anchor at the current head. Strict mode can instead verify against historical keys from the PLC audit log.
- **M7. The SPA cannot build external link cards (PROTO §3 `#externalLink` title, description, thumb) without a proxy.**
  - Arbitrary sites do not send CORS headers.
  - Options: manual entry, CLI/native fetch, or an optional extension. `cardyb.bsky.app` sends CORS `*`, but it is an unstable Bluesky service.
- **M8. The report-service scope in SOCIAL §18 needs `aud=*`.**
  - `rpc:com.atproto.moderation.createReport?aud=<did>%23atproto_labeler` needs a per-forum DID that is unknown when the metadata is written, so the metadata must declare `aud=*`.
  - A fixed-aud rpc permission cannot go in a set. Use `inheritAud`.
  - The forum's DID must also publish an `#atproto_labeler` service.
- **M9. SYNC §3 says DNS and HTTPS answers that disagree make a handle invalid. The spec does not require that, and it would make clients disagree.**
  - In the browser, `.well-known` requests often fail CORS, so the SPA and native clients would reach different verdicts.
  - **Correction:** treat DNS as authoritative and consult HTTPS only if DNS is empty.
  - What works in the browser today:
    - `*.bsky.social` well-known sends CORS `*`;
    - DoH JSON via Cloudflare or Google (the `accept: application/dns-json` header is CORS-safelisted).
  - Custom-domain HTTPS-only handles and did:web documents often lack CORS, so they can only be hints.
- **M10. A merged per-subject `opinion` record needs a swap on write.** If SOCIAL's single record is adopted, PROTO's "putRecord with no swap" loses updates when two devices (for example CLI and SPA) write concurrently. Use `putRecord` + `swapRecord` and retry.
- **M11. OAuth callback handling and web client metadata (ARCH §11.4).**
  - The spec implies query parameters (`state`, `iss`). atcute uses the fragment. bsky.social supports both, but a third-party authorization server may support only query. Handle both `location.search` and `location.hash`.
  - The web metadata must include:
    - `grant_types: ["authorization_code","refresh_token"]`;
    - `response_types: ["code"]`;
    - `scope` (the superset of every variant);
    - `dpop_bound_access_tokens: true`;
    - `token_endpoint_auth_method: "none"`;
    - `application_type: "web"`.
  - `client_uri`, if set, must have the same hostname as the `client_id`, so it cannot point at the GitHub repo.
- **M12. PROTO §3's size budget can be exceeded.**
  - It assumes JSON escaping at most doubles the 100 KB post plus its snapshot. Control characters escape to 6 bytes, so the 1,000,000-byte `applyWrites` body limit can be exceeded. Measure the serialized body and spill to `textBlob` before sending.
  - `textBlob maxSize 10,000,000` exceeds the default 5 MiB upload limit on self-hosted PDSes. Keep the limit (it is frozen) but handle 413 responses.
- **M13. OAuth sessions are short for every client.** There is no backend, so every lessdugin client is a public client with a hard ≤14-day session.
  - Headless, cron and moderator-bot use must use app passwords.
  - ARCH's `authFactorToken` 2FA prompt only applies to main-password logins; app passwords bypass email 2FA. The CLI should refuse main-password logins.
- **M14. Some publishing and testing facts follow from the permission-set rules.**
  - Publish `authFull` before any bsky.social test. On a PAR failure, retry with PROTO's enumerated `repo?collection=…` form.
  - There is no need to pre-list P2 collections, because sets are re-resolved on refresh.
  - The dev-env cannot resolve `include:`, so end-to-end tests must use the enumerated scope.

### Low

- **PROTO §9 indexer service on a did:plc** requires a PLC operation (an email-token flow on Bluesky PDSes). Prefer did:web, as SYNC §13 does.
- **PROTO §1 naming rule** is a convention; the NSID spec is silent on it.
- **PLC directory rate limits** are undocumented (it sends no headers). SYNC's adaptive pacing is needed; throughput is unverified.
- **Jetstream identity and account events** are not filtered by collection, which costs browser bandwidth when no DID filter is set.
- **Relay, Jetstream and Constellation coverage** only includes PDSes the Bluesky relay crawls. Open-mode discovery misses uncrawled self-hosted PDSes; roster mode does not.
- **`listReposByCollection`** takes one exact NSID, so backfill needs one call per collection.
- **The `applyWrites` `commit` output is optional.** SYNC §6's own-write path needs a `getLatestCommit` fallback.
- **`getBlocks` is all-or-nothing on a missing CID.** Keep batches at about 50 or fewer.
- **ARCH CSP** `connect-src https: wss:` blocks the http devnet PDS. Playwright needs a dev CSP variant.
- **`applyWrites` that dual-publishes to `community.lexicon.*`** fails the whole batch if that repo scope was not granted. Check the granted `scope` in the token response first.

## 3. Contradictions between the designs, with recommended resolutions

- **X1. Record model.**
  - The four designs use different record sets: PROTO's `vote`/`react`/`tagVote`/`modAction`/`wikiRevision`/`coauthor`; SOCIAL's `opinion`/`modState`/`authorSettings`/`coauthorAccept`/`tagRevision`/`dialogueMessage`/`karmaSnapshot`; SYNC's roster pages and `checkpoint`; ARCH's per-axis votes.
  - **Resolution:**
    - Use SOCIAL's single `opinion` record per (voter, subject). It is atomic, costs one record (about 2–3 points), gives one Constellation path, and should be written with `putRecord` + `swapRecord`.
    - Use SOCIAL's `modState` with rank plus safety-minimum, which does not depend on time. PROTO's "latest time wins" for section and tier depends on self-asserted time.
- **X2. Derived rkeys.** There are four incompatible schemes:
  - PROTO sk/1: 16 characters, domain-separated by collection with 0x1F separators;
  - SOCIAL: 26 characters, `lessdugin:kind:v1\n…`;
  - ARCH: 32 characters, `uri‖0x00‖axis`, no domain separation;
  - SYNC: `{fk}-…` layouts.

  Rkey types and canonical-key validity are frozen at publication.
  - **Resolution:**
    - Use SYNC's locality prefix: posts `{fk}-{tid}`; comments `{fk}-{rootTid}-{tid}` (fixes H7); interactions `{fk}-{stid}-{h}`; membership `{fk}`; revisions `{postRkey}-{tid}`.
    - Use PROTO's domain-separated hash for `{h}`.
    - Define a sentinel `fk` for content with no forum.
    - Change the lexicon key type to `any`, with a grammar the core validates. PROTO's `tid` key types change accordingly.
    - Adopt this layout now even if range sync waits, because rkeys are permanent.
- **X3. Indexer namespace.** PROTO uses `.index.*` with `listForumRepos`, `getWikiPage` and others; SYNC uses `.index.*` with `describe`, `getRepoHeads`, `getChanges` and others; ARCH puts the queries flat in the record group.
  - **Resolution:** use `.index.*` (its own TXT record and its own evolution), with one merged method list and separate CAR bundle methods (M1).
- **X4. Membership and roster.** PROTO uses a per-member `modAction#member`; SOCIAL uses `modState access=member` plus a required self-membership record; SYNC uses roster pages of 1,000 members.
  - SYNC's rate maths is correct: about 11.6k grants per moderator per day.
  - **Resolution:** per-member deterministic-key state records plus mutual consent as the authority. Add roster pages later as compaction for forums over 10k members. Unify the mode names (`approval|open`).
- **X5. Revision records.** PROTO uses `{subject strongRef, op, record union}`; SYNC uses `{post, cid, record}`; ARCH stores bodies as blobs above 16 KB and gives comments only `editedAt`.
  - ARCH's variant breaks hash verification and SOCIAL's quote anchoring.
  - **Resolution:** PROTO's shape with SYNC's rkey, and snapshots for comments too (5 points per edit).
- **X6. Verification and the sync primitive.** SYNC makes proofs the default via range sync in the MVP; ARCH defaults to `pds`, with ranges post-MVP via `getRepo?since`.
  - **Resolution:**
    - MVP: L1 with CID recomputation, plus **mandatory** proofs for moderation inputs (forum declaration and moderator records), per SYNC I7.
    - Range sync (`getRecord` + `getBlocks`) is the next milestone.
    - Drop `getRepo?since` except for export.
- **X7. Karma.** PROTO's karma/1 is two-pass with no seeds or ≥1-karma rule, so Sybils are open. ARCH uses fixed-point iteration, which is convergence- and order-sensitive. SOCIAL's karma/v1 is seeded, three-stage, with a pair cap.
  - Behaviour IDs are frozen once released. **Resolution:** adopt SOCIAL's and unify the ID style (`karma/1` vs `karma/v1`).
- **X8. Time.**
  - PROTO uses min(createdAt, TID, first-seen) and hides records more than 5 minutes in the future.
  - SYNC uses min(createdAt, TID) and also hides them.
  - SOCIAL uses max(causal lower bound, min(claimed time, observed time, now)).
  - ARCH clamps to the observed time.
  - **Resolution:** use SOCIAL's formula with TID time added inside the min (it can only make items older). Badge future-dated items as "scheduled" rather than hiding them; hiding also hides them from moderators.
- **X9. Vote values.** PROTO uses `int[-2..2]` (a frozen range); SOCIAL uses knownValues strings. **Resolution:** strings, which can be extended later.
- **X10. Forum declaration.**
  - PROTO allows 128 moderators with four roles; SOCIAL allows 50 with two roles.
  - SYNC needs `scorekeepers`, `indexers`, `mediaMirrors`, `syncWindowDays` and `algorithms`.
  - SOCIAL has integer tuning parameters.
  - **Resolution:** merge everything before publishing (all limits are frozen).
- **X11. Comment fields.** PROTO uses `subject`/`parent`/`moderatorHat`; SOCIAL uses `root`/`asModerator` and adds `post.acceptedAnswer`. **Resolution:** unify on one set.
- **X12. Wiki.** PROTO has a single `prev` and picks the head by latest time; SOCIAL has `parents[]≤8` (allowing merges) and picks by `revScore`. **Resolution:** SOCIAL's DAG, with the H3 fix.
- **X13. Spill thresholds.** PROTO spills at 64 KiB with a 100,000-byte maximum; ARCH uses 16 KB. **Resolution:** PROTO's.
- **X14. Scope chain.** **Resolution:** PROTO's chain (`include` → enumerated `repo?collection` + `blob:*/*`), with `transition:generic` only as a last resort.
- **X15. Open-mode discovery.** PROTO starts with the relay, which costs O(ecosystem). **Resolution:** Constellation first, relay as fallback.
- **X16. Reports (P2).** PROTO has a `forum` field and uses age; SOCIAL has no forum or subject field and uses HPKE; ARCH uses crypto_box.
  - **Resolution:** age v1 with X25519 anonymous recipients, in SOCIAL's metadata-free layout. Moderators discover reports via Jetstream `.report` events and `listReposByCollection`.
- **X17. Identity details.**
  - Cache TTLs: SYNC uses 1h for handles and 7d for DID docs; ARCH uses 24h.
  - SYNC says TS does no identity resolution, but atcute resolves identity during login.
  - **Resolution:** allow atcute's resolution for login only, and have the core re-check the session DID's PDS-to-authorization-server binding.
- **X18. Author moderation storage.** PROTO uses author-scope `modAction` records; SOCIAL uses `post.bannedUsers`, `authorSettings` and `authormod` records. **Resolution:** pick one before the lexicons freeze.

## 4. Live spikes to run before freezing the lexicons

1. Publish the schemas and TXT record, then log into bsky.social with `include:…authFull`. Capture the PAR error shape when the set cannot be resolved.
2. On bsky.social and a self-hosted PDS, write records with `validate` unset that contain a union of record types and nested blob refs. Confirm that blobs referenced only by a revision snapshot survive.
3. Run the NSID with its digit segment through every tool listed in M4.
4. Check that `getBlocks` and `sync.getRecord` exist and send CORS headers on non-reference PDSes (rsky, Caddy-fronted self-hosted). Check crafted-cursor `listRecords` behaviour there too.
5. Measure PLC directory throughput at about 20 requests/s.
6. Test OAuth query vs fragment responses, and loopback `client_id` acceptance, on a third-party authorization server.

### Critical Files for Implementation
- /home/user/lessdugin/lexicons/online/7778777/lessdugin/ (one merged record set with final limits on every field; `authFull.json`; `index/`)
- /home/user/lessdugin/docs/spec/data-model.md (the single rkey grammar, revision and wiki rules, write rules for swapCommit and no-upsert)
- /home/user/lessdugin/crates/core/src/ids.rs (rkey derivation and validation; vectors already verified)
- /home/user/lessdugin/crates/core/src/engine/outbox.rs (`applyWrites`/`swapCommit` retry, `putRecord`+`swapRecord`, verbatim blob refs)
- /home/user/lessdugin/deploy/site/oauth/client-metadata.json and cli-client-metadata.json (full required fields, superset scope with `blob:*/*`)

##### CRITIQUE: PRODUCT #####
# lessdugin product, UX and scope review

## Verdict

- The four designs are strong one at a time. Taken together they are about 2–3× the scope of the requested MVP, and they contradict each other on almost every decision that can never be changed later: rkey formats, record granularity, roster, moderation resolution, karma, time and revisions.
- Fix this first. Before any code, write **one reconciled spec with a decision log** (P0 below). Building on the current four docs guarantees churn.
- **Rule for the cut-line.** Lexicon evolution allows new optional fields, knownValues and collections, so almost every feature can be deferred. Only these must be right now:
  - NSIDs and record granularity;
  - rkey schemes;
  - required fields and size limits;
  - strongRef vs at-uri for each reference;
  - the shape of the markdown object;
  - LFM/1 syntax, which is frozen once released.

  Everything else should leave the v1 lexicons.

---

## 1. Contradictions between the docs (P0: resolve before code)

| # | Topic | PROTOCOL | SYNC | SOCIAL | ARCH | Recommendation |
|---|---|---|---|---|---|---|
| C1 | Interaction record granularity | Separate `vote` (2 axes), `react` (one per react+quote) and `tagVote` (§5.6–5.8) | One merged record per actor+subject (§4) | One `opinion` record with axes, `reacts[]` and `tags[]` (§6) | One record per axis (§1.3) | **SOCIAL's `opinion`.** It means fewer collections, so fewer `listRecords` calls per participant (see 2.1). It gives 1 write per toggle and one Constellation path. Do read-modify-write across devices with `putRecord`+`swapRecord`. |
| C2 | Derived rkey function | `sk/1`, 16 chars, 80 bits, 0x1F separators | `{fk}-{stid}-{h16}` | 26 chars, `"lessdugin:kind:v1\n…"` | 32 chars, `uri‖0x00‖axis` | One function, PROTOCOL-style domain separation, prefixed with SYNC's `fk`. Drop `stid`; nobody needs windowing by subject time in the MVP. |
| C3 | Post/comment rkey type | `tid` | `any` = `{fk}-{tid}` | tid | tid | **`{fk}-{tid}`. This is permanent, so decide now.** I checked the reference PDS (`packages/pds/src/actor-store/record/reader.ts`): `listRecords` compares the full URI against `cursor` and still honours the deprecated `rkeyStart`/`rkeyEnd`. Per-forum range listing therefore works **today at L1, without rangesync**. A forum-wide sync costs one request per collection per participant, not a fetch of everything they wrote in every forum. Other PDS implementations fall back to list-all-and-filter (spike S7). |
| C4 | Roster | `modAction#member` per member | Pages of 1,000 members plus a mod sweep | `access` dimension per DID | `roster_*` | MVP: one record per member; it is simple and carries reason and until. Pages can be added later as a new collection. The rate-limit ceiling (about 11k grants/day per mod) is not an MVP problem. |
| C5 | Membership modes | open / approval | approval / open | roster / open (+`reviewNewUsers`, LATER) | approval | Unify as one roster with two policies. **approval**: content from non-members is hidden. **open**: non-members are shown collapsed as "new user" until a mod approves them, which is FM's review queue. This merges SOCIAL's open+reviewNewUsers, SYNC's sweep and PROTOCOL's beacon. |
| C6 | Mod record shape and conflict rules | One record per action; OR for hide/ban; latest effective time wins for tier/section; 4 roles, ≤128 mods | – | One record per subject holding several dimensions; rank plus safety-minimum, no time; 2 roles, ≤50 mods | "Actions by removed mods void" | PROTOCOL's per-action records, which make undo a delete and give one mod-log line per action, combined with **SOCIAL's rank plus safety-minimum**. Time-based "latest wins" is gameable. MVP roles are owner, admin and moderator; curator and reviewer come later (unknown roles grant nothing, so adding them is safe). |
| C7 | Karma | `karma/1`: two passes, k0 = count of signs; implicit *small* self-vote | All-time data infeasible, so checkpoints | `karma/v1`: three seeded stages with pairCap; self-vote *big* on posts, small on comments | Bounded fixed-point iteration | SOCIAL `karma/v1`. Reject ARCH's fixed point: convergence and determinism are harder and it is not explainable. Pick one self-vote rule and one id spelling (`karma/1` vs `karma/v1`). Defer windowing so the MVP has complete inputs and needs no checkpoints. |
| C8 | Time | `min(createdAt, tid, firstSeen)`; future-dated content hidden | `min(createdAt, tid)`; future content "scheduled" | `t_rank` clamped to first-seen; says hide/clamp-to-now is a bug | Clamp to observed | SOCIAL `t_rank`. Any observer term makes output depend on the client, which contradicts SOCIAL §1 and ARCH M5 "byte-identical". The tests must inject `obs`. |
| C9 | Revision snapshot | Verbatim union plus `op`, rkey tid | `{post,cid,record}`, rkey `{postRkey}-{tid}` | Must be verbatim (§22.9) | **Body over 16KB moved to a blob (§1.13), which breaks the CID check**; comments get only `editedAt` | Always verbatim. Bodies over 64KB are already `textBlob` refs, so snapshots stay small. Comments get revisions too. |
| C10 | Edit concurrency | `applyWrites`+`swapCommit` (§2.4), which is **repo-wide**: one Bluesky like from another app causes a spurious conflict | – | – | "Edits use swapRecord" (§4.4); `applyWrites` has no per-op swap | Use `swapCommit`. On failure, re-read the record: if its CID is unchanged, retry against the new head automatically; otherwise it is a real conflict. |
| C11 | Markdown | `lfm/1`, ids prefixed `h-`, KaTeX/Temml-compatible math, extended-autolink pass, inline limit 100,000b with spill at 64KiB | Inline limit 64KB | – | `lessdugin-md/1`, ids prefixed `md-`, math-core to MathML, `noreferrer`, `#/at/` routes | One profile name and one id prefix. Math rendering is decided by spike S10, not by the spec. |
| C12 | MVP sync and store | Proof sync optional | **Rangesync over our own MST, `proof` as default, in the MVP**; relational SQLite plus an IDB mirror; L0/L1/L2 with strict/standard/accelerated | – | `listRecords` at `pds` level; verified sync in P10 via `getRepo?since` (which SYNC §0.6 rejects); ordered KV; hint/pds/proven/plc | MVP: ARCH's L1 path with fk ranges, and `verify` on demand. Rangesync comes in P7. The key layout from C3 keeps that option open. |
| C13 | Indexer API | `.index.*` sub-group, `listForumRepos`, `search`, `listNotifications` | `.index.*`, `listParticipants`, `searchPosts`, `getActivity`, proof bundles | – | Flat namespace, `listParticipants`, `listBacklinks`, results are hints | `.index.*` sub-group, SYNC's method names, MVP results are uri+cid plus hint values (no bundles). |
| C14 | Blob scope | `blob:*/*` | – | – | `blob:image/* blob:video/*` | ARCH's scope **breaks `textBlob` spill (`text/markdown`) and attachment cards**. Use `blob:*/*`. |
| C15 | Feature phases | Q&A, quick takes, coauthors, sections, wiki with single `prev`, `.report` with a `forum` field, `forum create` writes gates of 0 | – | Same features LATER; wiki as a merge DAG (`parents≤8`, revScore); reports must have **no** forum/subject fields; FM gate defaults in open mode | P8 for those features; `topAdjusted` in M3 | Follow section 5. |
| C16 | Posts with no forum | Allowed: `forum` absent means personal blog (§5.3) | Keys need `fk` | Everything is forum-scoped | – | **Require `forum`.** In FM, "personal blog" is a *tier* inside the site; standard.site covers real personal blogs. |
| C17 | Names | modAction, profile, coauthor, wikiRevision, turn, `moderatorHat`, `subject`, integer votes | `forum.checkpoint` (breaks PROTOCOL §1's naming rule) | modState, authorSettings, coauthorAccept, tagRevision, dialogueMessage, `asModerator`, `root`, string vote values | – | Publish a glossary in P0. |
| C18 | Crates and codegen | `lessdugin-core` monolith; jacquard-lexgen as a risk | Own MST code | – | atp, lexicon, markdown and core split; own xtask codegen | Use ARCH's split. |

**Inconsistency inside SYNC that changes the UX.**
- §9 treats all `*.host.bsky.network` hosts as one 10 req/s rate-limit domain. §12 claims 500 participants (about 3.5k requests) cold-sync in about 15 s.
- At 10 req/s, 3.5k requests take about 6 minutes, and PLC pacing alone adds 25 s or more.
- Most users are on Bluesky-hosted PDSes, so this decides whether serverless first visits take seconds or minutes. It must be measured (S9).
- Until it is measured, a progressive first paint and a time budget are mandatory UX, not polish.

---

## 2. Over-engineering to cut or defer

### 2.1 SYNC (the biggest overshoot)

Defer:
- rangesync and our own MST range walk (keep the C3 key layout);
- the `lessdugin.bundle/1` format, `getRepoRanges` and proof-serving indexers;
- multi-indexer union and digests, `getSetDigest`/`listBucket`, 1% audit sampling;
- `source_stats` strikes and demotion;
- learning rate-limit domains;
- half-open circuit breakers;
- the three profiles × three verification levels (nine combinations);
- roster pages and automatic mod sweeps;
- scorekeeper checkpoints, media mirrors, the "private reading" prefetch;
- the hint overlay table: in the MVP a hint only triggers a refetch;
- sync windows: there is no windowing in the MVP, so karma is complete.

Instead:
- **Two user-facing modes.** `direct` (PDS, PLC and DNS only) and `assisted` (adds Jetstream, Constellation, relay and indexer). Default `assisted`.
- **Three badges.** `[pds]`, `[verified]` (after `lessdugin verify` or the SPA's verify action) and `[hint]`.
- **Simple scheduler.** A per-host semaphore of 6, 3 retries with jitter, honouring `RateLimit-*`/`Retry-After`, and a 24-hour reconciliation pass.
- **Multi-tab.** Replace Web Locks leader election plus BroadcastChannel (§10 I10) with a per-tab worker and `navigator.locks.request('sync:'+did)` around each repo sync. Store writes are idempotent, monotonic-rev upserts.

Keep:
- Jetstream as a dirty signal;
- deletes applied only after confirmation;
- CID recompute on ingest;
- "unknown is not absent" (fail closed);
- the newcomer budget.

### 2.2 SOCIAL

Defer:
- the late-arrival rule: it needs a live observer, and cold-sync clients cannot apply it anyway;
- `auto/v1` rate limits (reserve the policy field);
- the author-moderation tier and moderation styles;
- the wiki merge DAG with revScore. Use PROTOCOL's single `prev` plus "latest valid head, fork warning, mod lock";
- the dimensions `acceptedAnswer`, `official`, `canonicalRevision`, `readonly` and `votesVoid` (keep `votesNullified`);
- the lens "exclude moderator X";
- personalized magic.

Keep:
- the layered DAG, seeds/E, pairCap, safety-minimum;
- the property tests: permutation, sybil, layering;
- coauthor credit text in `karma/v1`, though it could equally be `karma/v2` later.

### 2.3 PROTOCOL

Cut from the v1 forum declaration:
- `sections`/`defaultSection` (a phpBB idea, not FM; tags plus tiers cover it);
- react palette add/remove;
- `discovery`, `movedTo`, `archived`, `banner`, `langs`, `links`, `labels`.

The 14 gates become 4: vote, react, createTag, wikiEdit.

Cut from post:
- `section`, `canonicalUrl`, per-post `votingSystem`, `moderation`, `labels`, `langs`, `excerpt` (derive it).

All of these are optional fields and can be added later without breaking anything.

MVP mod actions are `hide`, `ban`, `member`, `tier`, `pin`, `lock`, and `restrict{nullifyVotes}`.

### 2.4 ARCH

- **The KV store's 15 secondary-index prefixes become in-memory indices rebuilt on load.** The SOCIAL fold needs the whole forum record set in memory anyway. Persist only `rec`, repo sync state, `id`, `out`, `loc` and blobs, and lazy-load bodies. A forum of tens of thousands of records fits easily. This removes index-migration bugs and IDB latency on hot paths.
- **Own lexicon codegen becomes handwritten owned serde types for about 10 records, plus a runtime, data-driven lexicon validator** that embeds the JSON. One conformance test per record keeps them aligned. The validator is also what the indexer needs for arbitrary versions. Revisit codegen after 1.0.
- **M3 as one mega-phase** (engine, full CLI tree and all social features before any TUI or SPA consumer) gives an engine API designed without its wasm consumer. Use vertical slices instead (section 6).
- **TS 7/tsgo, Biome 2.5, Vite 8:** fine, but keep a `tsc` fallback in CI.

---

## 3. Missing ForumMagnum must-haves for the MVP

- **Recent Discussion feed** (recent comments across posts, grouped by post). On LessWrong this is how people find new activity. Every doc has only the `recentComments` *post* sort. Add `#/f/…/activity`, `lessdugin activity` and a TUI tab.
- **Comment edits with visible history.** ARCH's MVP has `editedAt` only; PROTOCOL uses revisions. Users expect an "edited" marker and a diff.
- **LFM/1 gaps, which freeze at release:**
  - spoilers and collapsible blocks (heavily used on LessWrong): propose a post-pass on `> [!spoiler]` and `> [!details] Summary`;
  - image captions: `![alt](cid:x "caption")` becomes `figure`/`figcaption`;
  - decide them now.
- **Short refs for comments in CLI `read` output** (e.g. `c12`). ARCH §9 defines numbered refs only for listings, so replying to or voting on a comment from the CLI has no specified UX.
- **Clear UI for votes that do not count yet.** Under seeded karma, new users' votes have no weight. Say: "recorded; will count once you have 1 karma (applied retroactively)". Without this the forum feels broken in week 1.
- **Undo window on votes and reacts.** Hold them 3–5 s in the outbox (ARCH already merges toggles). They are public and rate-limited writes, and the TUI makes them single keystrokes.
- **Forum discovery in CLI and TUI** (`lessdugin forum discover`, via relay `listReposByCollection(.forum)`), and a **current forum**, so `ls` and `post` need no arguments.
- **Anonymous read path everywhere.** No login or config should be needed to read a link.
- **Mention autocomplete** in the SPA editor (from the participant set) and mention notifications.
- **Mobile layout for the SPA.** No doc mentions it, yet FM traffic is largely mobile. Mobile needs touch voting, collapsed thread indent and a bottom-sheet react palette.
- **Mod queue usability.** One key to approve, hide or ban with a reason, in the TUI and the SPA.

---

## 4. CLI first run: friction and fixes

0. **Install.**
   - Release binaries embed the SPA.
   - `cargo install` from crates.io will not include `web/dist` unless the dist is packaged (watch the 10MB crate limit). Document `cargo binstall` and the release attestations.
   - The `lessdugin web` placeholder page (ARCH §11.4) is acceptable.
1. **First command is reading a shared link**, e.g. `lessdugin read 'https://lessdugin.7778777.online/#/f/alice.example/rationality'`.
   - It must work without login or config.
   - On a cold sync, apply a default `--time-budget` of 10 s for interactive commands, then print what is available with `[partial 120/500 members — run 'lessdugin sync']`.
   - Never show a 6-minute spinner.
2. **`lessdugin login alice.bsky.social`.**
   - **SSH/headless:** loopback OAuth cannot work, because the browser is on another machine. Add `login --no-browser`, which prints the URL; the user pastes back the failed `http://127.0.0.1/...?code=` redirect URL (with a PKCE+state check). Steering everyone to app passwords is worse.
   - **Scope fallback:** the chain (`include:` → `repo?collection=` → `transition:generic`) must be resolved **at PAR time** (an `invalid_scope` error) before opening the browser. Otherwise users see up to three consent screens (S8).
   - Warn about the roughly 2-week public-client session expiry a few days ahead, and re-login inline in the TUI.
3. **Join.**
   - ARCH requires `forum add` + `forum join` + `--alias`. Make it one command: `lessdugin join <ref>` follows the forum, writes the membership, sets it as current and auto-aliases it.
   - `forum add` stays as "follow without joining" (lurk).
   - In approval mode, print: "pending — you can read; your posts are hidden until approved".
4. **Read.** `ls` shows numbered posts with unread markers. `read 3` pipes to the pager and labels comments `c1…cN`, with "new since last visit" markers.
5. **Reply.**
   - `comment c12` opens `$EDITOR` with the quoted parent below a scissors line; `-m "text"` for one-liners.
   - Editor fallback chain: `$VISUAL` → `$EDITOR` → `editor` → `nano` → `vi` (notepad on Windows).
   - After sending, show the permalink and state ("sent → confirmed on PDS").
6. **Post with an image.**
   - `lessdugin post draft.md` with frontmatter and `![](./cat.png)`.
   - Warn when alt text is missing.
   - Strip EXIF and physically apply orientation.
   - **Downscale automatically** to the PDS limit (self-hosted PDSes default to 5MiB).
   - Write the membership beacon automatically if missing.
   - Confirm with `[y/N/e]`.
   - Skip author-generated thumbnails in the MVP: the `image` crate's lossy WebP needs libwebp. Use JPEG thumbnails later.
7. **Vote.** `vote c12 up` shows a one-time warning that votes are public.

That is about 6 commands for the whole flow, which is good. The only blockers are items 2, 3 and 5.

---

## 5. TUI and SPA gaps

### TUI (ARCH §10)

- **Key conflicts:** `n` means both "new post" and "next search result". Move new post to `N`/`P` or put it behind a leader key.
- Single-key public writes need the pending/undo state from section 3.
- **Inline composer** (tui-textarea) for short replies, with `Ctrl-e` to escalate to `$EDITOR`. Many users have no `$EDITOR` set.
- **Deep threads at 80 columns:** cap indentation at about 6 levels, then "continue thread →".
- **Live updates must not move the cursor:** show a "3 new comments — R" banner.
- **tmux:** ratatui-image needs passthrough. Detect it and fall back to halfblocks.
- **Login inside the TUI:** suspend the terminal and open the browser, or use the `--no-browser` paste flow.
- **Accessibility:** a TUI is poor with screen readers. The CLI's plain `--color never` output (no box drawing) is the accessible path and should be tested.

### SPA (ARCH §11)

- No criterion for a cold first visit; only a cached forum has one (<1.5 s). Add: "50-member forum, no indexer, first post titles <3 s; 2k members with indexer <2 s".
- **The hosted SPA is a trusted website while it is being used**: its code holds the OAuth tokens. This needs to be stated clearly:
  - `lessdugin web` is the zero-trust path;
  - "self-host on any static origin": `xtask site --origin https://…` generates `client-metadata.json` per origin, because the client_id must be served as JSON at its own URL;
  - publish dist hashes per release.
- **`lessdugin web` loopback OAuth:** if a PDS rejects loopback, the fallback is the hosted *native* metadata with a `127.0.0.1` redirect. A web-type client cannot use an http redirect. Spike S2 must cover this. The double login (CLI session plus browser session) is acceptable for the MVP but should be documented.
- **Safari/iOS:** ITP's 7-day script-storage eviction can wipe drafts, read marks and the atcute localStorage session.
  - Mitigations: PWA install, `storage.persist()`, an autosave warning, and "download draft".
  - Test in S11.
- **Editor:** FM users expect a WYSIWYG editor. Markdown in CodeMirror is fine with a toolbar (bold, link, image, math, spoiler), paste/drop images, live preview and math preview.
- **Hash routing** means no OpenGraph link previews. Accept this; optionally add "share to Bluesky", which creates a link-card post.
- **wasm cost on low-end Android:** compiling about 1MB takes 1–2 s. Use streaming compile and show the skeleton UI first.

---

## 6. Implementation risks (by impact)

1. **Lexicon lock-in before learning.**
   - Once real users write `online.7778777.lessdugin.*` records, the shapes are effectively frozen, even before publication.
   - Make the NSID prefix a build-time constant.
   - Dogfood on devnet and on throwaway bsky.social accounts under `.temp.*`.
   - Switch to the real namespace only at release 0.1, after the social core has been dogfooded.
2. **Cold-sync cost against real PDS rate limits** (S9). This decides whether "no server" is pleasant at a few hundred members.
   - Mitigations: the fk-range `listRecords` path (C3), few collections (C1), persistent cache, progressive render, and the optional indexer.
3. **Scope.** All four docs add up to roughly 40–60k lines. As written, ARCH M3 is most of the project. Vertical slices surface the integration risks early: the wasm/IDB boundary, OAuth in the worker, render parity.
4. **jacquard (alpha, 0.13 breaking).**
   - ARCH's confinement to native OAuth behind a facade is right.
   - Timebox S1. If jacquard's dependency weight or API is a problem, our own PAR+PKCE+DPoP client is about 1–1.5k lines, and we already own the XRPC layer.
5. **wasm size** (budget 0.9–1.1MB gz).
   - Feature-gate `k256`/`p256` out of the default SPA build, since proof is not the default in the MVP.
   - S10: measure math-core. If it is large, Rust emits sanitized `<span data-tex>` and the SPA lazy-loads Temml (about 60KB). Sanitization stays in the core; the TUI and CLI show TeX source anyway.
6. **Third-party infrastructure in open mode.** Constellation is best-effort with a single operator. The relay fallback is O(ecosystem). Approval mode is the only truly serverless mode, and the docs must say so plainly.
7. **Cross-platform.**
   - Windows: no `ttyname` (ARCH's PID fallback is fine), keyring backends, sixel support in Windows Terminal, the `$EDITOR` chain.
   - WSL: forwarding the loopback redirect.
   - musl + rustls + bundled rusqlite: fine.
8. **tsify maintenance.** Keep `ts-rs` or hand-written `.d.ts` as a fallback. The DTO set is small.

---

## 7. Sharper MVP cut-line

### In the MVP

**Lexicons (about 10 collections):**
- forum (minimal), membership, post, comment, revision, opinion (karma, agreement, reacts with an optional quote, tag votes), modAction, wikiRevision, sequence, profile.
- Permission sets `authFull` and `authInteract`.

**Protocol:**
- Unified rkey function with the fk prefix.
- Roster with approval/open policies.
- Moderation: per-action records resolved by rank plus safety-minimum, failing closed; actions hide, ban, member, tier (personal/frontpage/curated), pin, lock, nullify.

**Social:**
- `karma/v1` with seeds and pairCap.
- Scores and agreement.
- Sorts: magic, top, new, old, recentComments.
- Recent Discussion feed.
- Reacts using the default palette only; the quote is stored but there is no margin UI.
- Tags: author tags, relevance via opinion, and a linear wiki page.
- Sequences: create, add, next/prev navigation.

**Content and media:**
- LFM/1: tables, footnotes, math, alerts, spoilers, captions, at:// hover previews, mentions.
- Images: strip, orient, downscale, CID-verify, display via object URL.
- Attachments as download cards; video and audio as click-to-load downloads.
- Edits with revisions for posts and comments.

**Local and sync:**
- Local drafts, read marks, unread highlighting, subscriptions, local notifications and local search.
- L1 sync: fk ranges, CID recompute, delete confirmation, `getLatestCommit` skip.
- Jetstream as a dirty signal; Constellation/relay discovery in open mode.
- `lessdugin verify <ref>` for an on-demand proof.

**Clients:**
- CLI core command tree.
- TUI: home, forum, post/thread, composer, notifications, mod queue.
- SPA: the same, plus the editor, `lessdugin web` and a hosted static deploy.

**Indexer:**
- describe, `listParticipants`, `listPosts`/`getThread` skeletons (values as hints), `searchPosts`, `getActivity`; `--audit`.

### Out (P7 and later)

Rangesync and proof as default; bundles; checkpoints; multi-indexer cross-checks; PLC audit; profiles beyond direct/assisted; author moderation and moderation styles; rate limits; Q&A; quick takes; coauthors and dialogues; side-comment and inline-react margin UI; sections; palette customization; tag merge DAG; reports beyond a contact link; events and polls; standard.site syndication; export to HTML; thumbnails; Service Worker media streaming; engine bridge.

Q&A and quick takes are mostly UI and could be pulled in as stretch goals.

---

## 8. Phase order with acceptance criteria

### P0: Reconcile and spike

**Deliverables**
- One `docs/spec` with a decision log closing C1–C18.
- A glossary.
- Minimal lexicons that pass `goat lex lint`.
- `sk` vectors regenerated.
- Spikes S1–S6 (ARCH) plus:
  - **S7:** `listRecords` cursor and `rkeyStart` range semantics on bsky.social production and on other PDS implementations;
  - **S8:** scope failure detectable at PAR;
  - **S9:** PDS read rate limits, per host or shared, measured by a cold sync of 200 real repos;
  - **S10:** wasm size with and without math-core and the crypto crates;
  - **S11:** Safari/iOS IDB and session persistence;
  - **S12:** the `--no-browser` paste flow on bsky.social.

**Acceptance**
- Every contradiction has one recorded decision and every spike has an ADR.
- Running `cargo xtask ci` works and devnet boots in under 30 s.

### P1: Walking skeleton (text only, all three clients)

**Scope**
- atp primitives (JSON/CBOR/CID, no MST yet), identity, XRPC subset, runtime validator.
- Login: app password plus loopback OAuth, with `--no-browser`.
- Store (records, repo state, outbox, local state) and fk-range sync.
- Thread assembly and a basic markdown renderer.
- CLI: `read` (anonymous), `login`, `forum create`, `join`, `ls`, `post`, `comment`, `edit`, `delete`, `sync`.
- TUI: read-only navigation plus reply through `$EDITOR`.
- SPA: wasm worker, anonymous read, OAuth login, comment.

**Acceptance**
- Devnet scenario: alice creates a forum (CLI), bob posts (CLI), carol replies (SPA). The thread JSON from the CLI's `--json` and from the SPA's `getThread` is byte-identical with `obs` injected, and the TUI snapshot matches.
- Production smoke test on throwaway accounts under the `.temp` NSID.
- A 50-member FakePds forum cold-syncs in under 5 s on native.
- wasm size is reported in CI.

### P2: Social core

**Scope**
- Opinion record, `karma/v1`, scores, rankings, Recent Discussion.
- Moderation fold and mod queue, approval and open policies with newcomer collapse.
- Notifications and unread highlighting.
- Votes-public warning, undo window, "vote not counted yet" UI.

**Acceptance**
- Vectors and property tests pass: permutation, sybil, layering, pair bound.
- Taking the mod's PDS offline brings no hidden spam back.
- All MVP actions are reachable in all three clients, and parity diffs are clean.

### P3: Rich content and media

**Scope**
- Images and attachments, math, footnotes and sidenotes, ToC, hover previews, spoilers and captions.
- Revisions and history view.
- SPA editor: toolbar, paste/drop, preview.
- TUI images.

**Acceptance**
- At least 150 LFM vectors plus an XSS corpus.
- DOMPurify and ammonia leave the renderer's output unchanged.
- EXIF and GPS removed with orientation preserved.
- Auto-downscale stays under 5MiB.
- No CSP or Trusted Types violations in Playwright.

### P4: Organisation, discovery and production auth

**Scope**
- Tags and sequences, local search, forum directory.
- Live Jetstream, open-mode discovery.
- `include:` scope with fallback, hosted SPA and client metadata, `lessdugin web`.

**Acceptance**
- Open-mode devnet runs with a mini-Jetstream.
- Login on bsky.social works via the CLI (loopback and hosted-native), the hosted SPA and `lessdugin web`, each with **one** consent screen.

### P5: Optional indexer

**Acceptance**
- Clients produce identical views with and without the indexer.
- A restart resumes with no gaps.
- The SPA paints a 2k-participant synthetic forum on a cold visit in under 2 s with the indexer.
- `--audit` flags an injected omission.

### P6: Release 0.1

ARCH M7's criteria, plus switching the NSID from `.temp` to the real namespace, and a "Level-0 conformance" doc (read posts, comments and mod hides) for third-party clients.

### P7 and later

Rangesync and proof as default, bundles, checkpoints, PLC audit, then the features listed under "Out" in section 7.

---

### Critical Files for Implementation

All of these are planned; the repo currently holds only LICENSE.
- /home/user/lessdugin/docs/spec/data-model.md (the reconciled spec and decision log for C1–C18)
- /home/user/lessdugin/lexicons/online/7778777/lessdugin/opinion.json (with forum.json, modAction.json, post.json and comment.json, using `{fk}-{tid}` keys)
- /home/user/lessdugin/crates/core/src/ids.rs (the single rkey and fk derivation, and the effective-time rule)
- /home/user/lessdugin/crates/core/src/social/view.rs (the fold entry point; the property-tested layering)
- /home/user/lessdugin/crates/cli/src/main.rs (the first-run flow: anonymous `read`, `login --no-browser`, `join`, comment refs, time budget)
- /home/user/lessdugin/packages/engine/src/worker.ts (the wasm/IDB/OAuth bridge, which the P1 walking skeleton must prove early)

Reference I checked for C3: https://github.com/bluesky-social/atproto/blob/main/packages/pds/src/actor-store/record/reader.ts