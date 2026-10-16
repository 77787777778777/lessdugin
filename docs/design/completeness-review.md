## P0: Blockers (an implementer would have to invent semantics)

1. **`#restrict` has no visibility rule.** §4.10 never checks `restrict{post,comment,sequence,tag}`. Only vote, react and wiki restrictions are wired in (§4.5, §4.13). Because time is self-asserted, "from now on" can't be defined. **Fix:** add rule 4b. A restricted capability hides every in-forum item of that kind by that user as `Hidden(Restricted)`. This is retroactive, like a ban. Item-level keeps override it and `until` lapses it. The mod UI should say it is retroactive.

2. **The forum-scope lock is broken.**
   - `allowed ≤5000` can't list a whole forum.
   - The §4.4 rank rule (`rank(X) > rank(author(T))`) stops an admin (rank 2) from locking a forum subject owned by the owner (rank 3). The action table says admins can.
   - **Fix:** cut forum lock from v1 and reserve `lock` on forum uris. Owners can archive by switching to approval mode.
   - Also define the post-lock case with more than 5000 comments: refuse the lock and tell the mod to use hide.

3. **Stale third-party proofs can bring back deleted or edited content.** A signed proof at an old rev stays valid forever, and an SPA that cold-starts from the indexer has no tombstones. **Fix:** treat a proof as current only if its rev is at least the head returned by the author PDS's `getLatestCommit`. Batch these checks per author on screen. Until then, mark it `[stale]` and keep it out of the fold input.

4. **300 MB blobs would pass through wasm linear memory.** `HttpResponse.body: Vec<u8>` is the only blob path. **Fix:** add `Host::fetch_blob(did, cid, max) -> BlobHandle`.
   - In JS: stream the bytes and check them with `crypto.subtle.digest`.
   - Natively: stream to a file while hashing with sha256.
   - The core only handles the handles.

5. **Fetch caps break determinism.** The 20k records per repo per forum cap is a fetch limit. Heavy ForumMagnum-style voters will pass it over the years, and clients that truncate at different points fold different inputs. **Fix:** make the cap normative in `limits`: "the first N keys in descending rkey order of the `{fk}-` range, per collection". Every client then excludes the same excess and shows `[partial]`. Raise N for `opinion` (for example to 100k).

6. **Required fields per post kind are unspecified.** `title` is optional for every kind, and `link` is optional even when `kind=link`. **Fix:** add core validity rules:
   - `title` is required unless the post is a `quickTake`.
   - `link` is required for `kind=link`.
   - A title on a `quickTake` is ignored.

7. **The outbox races `swapCommit`.** Any write to the repo invalidates `swapCommit`: an opinion from our own pump, the Bluesky app, or a second device. **Fix:** serialise the outbox per account with one write in flight. Take the swap head from a fresh `getLatestCommit`, never from the cache.

## P1: Internal inconsistencies

- **Time clamp.** D12 says `firstSeen + 5 min`; §4.2 clamps to `t_obs`. Keep §4.2 and correct D12.
- **modAction write path.** §3.5 writes modActions "as for an opinion" (`putRecord`). But the mod log shows "revisions of modActions", and `revision.record` includes modAction. **Fix:** modAction updates and deletes use the edit protocol, writing an `op=delete` revision on removal. Content deletes purge revisions. Then `op:delete` exists only for modActions; say so.
- **Removing a moderator.** Their grants lapse at once, so approval-mode members vanish. **Fix:** `mods rm` must run `mod adopt` first and only then update the declaration. The CLI enforces this order.
- **`tag:` subjects have no author rank.** Define it as 0.
- **`establishSeeds` "approval 1".** In approval mode, S is all participants, so this setting has no effect. Delete the remark.
- **Sybil theorem.** It claims records outside E change no output, but `unweighted` counts and newcomer collapse do change. Restrict the theorem to K, scores and visibility of other users.
- **Search.** Native lists `fts5` while core has an in-memory index. State that the CLI uses the core index and fts5 is for the indexer only.
- **M4 module-name mangling.** It is moot with hand-written structs plus tsify. Drop it, or tie it explicitly to a TS lex-cli build.

## P2: atproto factual corrections

- **Login check (§3.1).** "sub, PDS and issuer agree" fails for every bsky.social user. Their issuer is the entryway `bsky.social`, while their PDS is `*.host.bsky.network`. **Fix:**
  1. Take the PDS from the DID doc of `sub`.
  2. Fetch that PDS's `/.well-known/oauth-protected-resource`.
  3. Require `authorization_servers` to contain the token `iss`.
- **Jetstream filters.** Collection filters don't apply to identity, account or sync events. A connection with no DID filter therefore receives identity and account traffic for the whole network, so "cheap" is wrong. **Fix:** drop events for unknown DIDs immediately. In the browser, switch to `wantedDids` (≤10k) once participants are known.
- **Chunked deletes are not atomic.** Deletes of more than 200 ops span several batches. Delete the target in the first chunk so a partial failure leaves only orphaned revisions.
- **`revision.record`.** A union of record-type defs is likely to trip goat lint and codegen. Use `unknown` (with a mandatory `$type`) and validate in core. Verify in S4.
- **Audio mime types.** `.ogg` usually sniffs as `application/ogg`, and `.m4a` as `audio/x-m4a`. Neither is reliably covered by `blob:audio/*`. **Fix:** normalise the declared mime before upload, or add `blob:application/ogg`; test in S4.
- **OAuth callback vs hash routing.** `@atproto/oauth-client-browser` defaults to `response_mode=fragment`, which collides with the hash router. **Fix:** force `response_mode=query` and handle the callback before the router mounts.
- **CSP on GitHub Pages.** A `<meta>` CSP ignores `frame-ancestors`, and Pages cannot send headers. **Fix:** make the default host one that sends the CSP as a header (Cloudflare Pages or Netlify), with GitHub Pages as a fallback.

## P3: Gaps against the request and the ForumMagnum inventory

- **"Nobody runs anything server-side."** Only approval mode is complete without third-party servers. In `sources=direct`, an open forum can never admit newcomers: seeds never see their content, so the closure never grows. **Fix:** make the lexicon default `membership=approval` before it is frozen (or have the CLI always write the mode explicitly). Label open mode in the UI as "relies on Bluesky relay, Jetstream and Constellation".
- **Forum succession.** A forum is bound to its owner DID forever, because fk embeds that DID. **Fix:**
  - The guide should recommend a dedicated did:plc forum account whose rotation keys are held by at least two admins.
  - Reserve an optional `forum.successor`/`aliases` field for P2.
- **Followed forums across devices.** The followed list is local. Seed Home from the user's own `membership` records.
- **Pingbacks.** `markdown.links` is hint-only. Define pingbacks as computed from the rendered bodies of synced posts, with indexer backlinks as hints.
- **ForumMagnum features in neither the MVP nor the deferred list.** Each needs an explicit entry in §8 (deferred or never):
  - recommendations;
  - shareable drafts and comments on drafts;
  - authorOnly tags;
  - the 10-karma removal of nofollow;
  - `lockedToAccountsCreatedAfter` and `repliesBlockedUntil`;
  - karma from votes on tag revisions.
- **`.temp` dogfood data.** It is orphaned at 0.1. Say so, or ship `migrate-temp`: the rkey grammar is identical, so it only copies records.
- **Magic cutoff.** Specify that items older than `cutoffDays` are excluded from magic but still appear in top, new and old.
- **Wiki.** A deleted parent revision becomes a missing edge. The child is treated as a root and flagged.

## Cut from the first pass

- **Forum lock** (see P0.2).
- **Wiki forks, merges and `aliasOf`.** The MVP keeps a linear history, with the head being the latest revision by an eligible author or the mod's `#tag.head`. Keep the `parents[]` schema.
- **Indexer (M6), keep only:** describeIndexer, listForums, listParticipants, getRepoHeads, getChanges, listPosts, getThread, getRecordProofs.
- **Indexer, defer:** searchPosts/fts5, getActivity, getKarma, listBacklinks, and automatic demotion. Keep the manual `--audit`.
- **Also defer:**
  - automatic switching of the Jetstream DID filter;
  - equivocation recording;
  - UTS #39 checks beyond owner and moderator names;
  - `/bridge/fetch`;
  - background `--verify proof`;
  - `verify-site`;
  - the STIX font subset;
  - running both tsgo and tsc in CI.

### Critical Files for Implementation
- /home/user/lessdugin/docs/spec/social.md
- /home/user/lessdugin/docs/spec/data-model.md
- /home/user/lessdugin/lexicons/online/7778777/lessdugin/modAction.json
- /home/user/lessdugin/crates/core/src/social/fold.rs
- /home/user/lessdugin/crates/core/src/host.rs

Sources: [Bluesky Jetstream SDK docs](https://bsky.network/ja/docs/jetstream-sdk), [crates.io aws-lc-sys](https://crates.io/api/v1/crates/aws-lc-sys)