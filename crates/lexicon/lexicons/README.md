<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# Vendored atproto lexicons

The lessdugin lexicons reference two `com.atproto` definitions (`data-model.md` §12):

| File | Used by |
|---|---|
| `com/atproto/repo/strongRef.json` | every `sref` field (`comment.root`, `opinion.subject`, …) |
| `com/atproto/label/defs.json` | `post.labels` (`#selfLabels`) |

They are reproduced from the `lexicons/` directory of
<https://github.com/bluesky-social/atproto> (MIT OR Apache-2.0), unmodified in substance
(descriptions only), and embedded by `src/schemas.rs` so that records can be validated without
fetching anything at runtime (`data-model.md` §1.4). The lessdugin lexicons themselves live in the
repository's top-level `lexicons/` directory and are embedded from there.
