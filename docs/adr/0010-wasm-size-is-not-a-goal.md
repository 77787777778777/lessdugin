<!-- SPDX-License-Identifier: CC0-1.0 -->
# ADR 0010 — The wasm module's size is reported, not optimised for

- Status: Accepted
- Date: 2026-11-03
- Supersedes: ADR 0008, and ADR 0002 rules 5 and 6

## Context

ADR 0008 turned ADR 0002's 1.1 MB gzipped budget into a tripwire: a warning above 1.5 MB gzipped
or 4 MB raw and a CI failure above 2.5 MB gzipped. Those lines were never derived from a
requirement either. Lines like these invite work that makes the product worse for a number:
hand-written code where a maintained crate does the job, features held back, or a build tuned
for size over speed. The user rejected that trade (2026-11-03): this is not a demoscene demo,
and only a hard limit on the size justifies effort spent on it.

There is one hard limit. Cloudflare Pages, the default static host (`threat-model.md` SITE-1)
and the host of the preview, serves no file above 25 MiB. Browsers set no limit on a module
compiled asynchronously in a Web Worker, which is how the SPA loads the engine. What the user
actually notices, load and paint time, is gated directly by acceptance criteria: a cached forum
renders in under 1.5 s (M5), and the SPA cold-paints a 2,000-participant forum in under 2 s
(M6).

## Decision

1. CI reports the raw and gzipped size of the release module, as M2 requires, with no target and
   no warning.
2. `cargo xtask wasm --budget` (and so `cargo xtask ci`) fails only when the module is larger
   than the host's 25 MiB per-file limit.
3. Size is not a reason to trade code quality, clarity, features or runtime speed. ADR 0002 rule
   6 (size hygiene in the wasm-safe crates) and rule 5 (a lazily loaded `verify.wasm`) are
   retired. Choices made under them stay where they have other reasons, for example the
   ASCII-only IDNA adapter: `safe_href` rejects non-ASCII hosts, which avoids homograph links
   (`lfm-1.md` §14 item 8).
4. Native-only dependencies (ammonia, html5ever) stay out of the wasm build for architectural
   reasons: the SPA sanitises with DOMPurify, and ammonia is the native sanitiser (`lfm-1.md`
   §8.5).
5. The `wasm` profile is built for runtime speed: `opt-level = 3` (it was `"z"`) and `wasm-opt
   -O3` (it was `-Oz`), keeping fat LTO, one codegen unit and `panic = "abort"`. Measured on the
   integrated tree on 2026-11-03, in node with the release module's `render` operation (two runs
   each, on a shared machine): 424 LFM vector inputs ×5 took 71–82 ms against 184–304 ms, and
   20 long posts with math ×5 took 398–459 ms against 1,057–1,157 ms, about 2.5 times faster.
   Compilation took 11–15 ms for both. The module grew from 5,465,859 to 8,259,684 bytes raw
   and from 2,064,066 to 3,000,275 bytes gzipped: about 0.9 MB more on a first visit, after
   which the browser caches it.

## Consequences

- An agent never shrinks the module to meet a number. A load or paint regression shows up in
  the acceptance criteria above, and is fixed where it arises.
- The size stays visible in every CI run and in the milestone status.
