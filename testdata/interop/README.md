<!-- SPDX-License-Identifier: CC0-1.0 -->

# Vendored atproto interop tests

These files come from the official atproto interop test repository:

- **Source:** <https://github.com/bluesky-social/atproto-interop-tests>
- **Commit:** `056e5741bb330757205d6b16db5266fffcae937b` (2026-07-01), cloned with `--depth 1`
- **License:** Creative Commons Zero 1.0 Universal (CC0-1.0), see [`LICENSE-CC0`](LICENSE-CC0).
  The upstream README is kept as [`UPSTREAM-README.md`](UPSTREAM-README.md).

Only the directories that `crates/atp` tests use were copied, unmodified:

| Directory | Used by | Contents |
|---|---|---|
| `syntax/` | `crates/atp/tests/interop_syntax.rs` | valid/invalid lists for handle, DID, NSID, AT URI, record key, TID, datetime, at-identifier, CID (plus unused `uri` and `language` lists) |
| `data-model/` | `crates/atp/tests/interop_datamodel.rs` | JSON → DAG-CBOR → CID fixtures; valid/invalid data-model values |
| `crypto/` | `crates/atp/tests/interop_crypto.rs` | signature fixtures (incl. high-S and DER rejection), did:key derivation |
| `mst/` | `crates/atp/tests/interop_mst.rs` | key heights, common prefix lengths (plus upstream `gen_keys.py` / `example_keys.txt`) |
| `firehose/` | `crates/atp/tests/interop_mst.rs` | MST commit-proof fixtures (root CIDs before/after, blocks in proof) |

Not vendored: `lexicon/` (lexicon validation is not part of `lessdugin-atp`).

When updating, re-clone, copy the same directories, and update the commit hash above.

## `live/` (captured from the network, not part of the upstream repository)

A real `com.atproto.sync.getRecord` proof captured on 2026-10-16 for the public account
`bsky.app` (`did:plc:z72i7hdynmk6r22z27h6tvur`, PDS `https://puffball.us-east.host.bsky.network`):

| File | Source |
|---|---|
| `bsky.app-did.json` | `https://plc.directory/did:plc:z72i7hdynmk6r22z27h6tvur` |
| `bsky.app-profile-self.car` | `GET /xrpc/com.atproto.sync.getRecord?did=…&collection=app.bsky.actor.profile&rkey=self` (commit rev `3mxxmh55uhk2l`) |
| `bsky.app-profile-self.record.json` | `GET /xrpc/com.atproto.repo.getRecord?repo=…&collection=app.bsky.actor.profile&rkey=self` |
| `bsky.app-profile-absent.car` | `GET /xrpc/com.atproto.sync.getRecord?did=…&collection=app.bsky.actor.profile&rkey=lessdugin-absent` (proof of absence, same commit) |

These are public, signed repository data, used unmodified as test fixtures by
`crates/atp/tests/live_proof.rs` (the proofs must verify as present / absent; any 1-byte change must fail).
