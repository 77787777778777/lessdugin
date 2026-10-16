# lessdugin

A rich-text (markdown) and media forum on the [AT Protocol](https://atproto.com), modelled on
[ForumMagnum](https://github.com/ForumMagnum/ForumMagnum) (LessWrong, EA Forum), that needs **no
forum server, no extra database and no trusted website**.

- **Your PDS is the database.** Forums, posts, comments, votes, reacts, wiki edits, sequences and
  moderation actions are ordinary records in their authors' own atproto repositories.
- **Every client computes the same view.** Moderation, karma, scores and ranking are a pure,
  deterministic fold over public records, implemented once in Rust and shared by the command line,
  the terminal UI, the web app (via WebAssembly) and the optional indexer.
- **Nothing third-party is trusted for content.** Text is shown only if it came from the author's own
  PDS (CID recomputed) or carries a signed commit proof. Relays, Jetstream, Constellation and indexers
  are hints that speed things up.
- **Optional extensions** (an indexer, live updates, global search) make large forums fast, without
  changing what anyone sees.

> Status: **pre-release, under active construction.** Data written by development builds goes to the
> unpublished `online.7778777.lessdugin.temp.*` namespace.

## Clients

| Client | How |
|---|---|
| CLI | `lessdugin ls`, `lessdugin read 3`, `lessdugin comment 3 -m "…"`, `lessdugin post draft.md` |
| TUI | `lessdugin` (or `lessdugin tui`) |
| Web | the hosted static site, or `lessdugin web` — the same app served from `127.0.0.1`, with your login kept by the CLI, so you don't have to trust any website |

## Documentation

- [Specification](docs/spec/README.md): data model and lexicons, LFM/1 markdown, social semantics,
  sync and verification, threat model, limits.
- [Implementation plan](docs/plan.md) and [decision log](docs/adr/).
- Design provenance: [reconciled design](docs/design/synthesis-v1.md).

## Repository layout

```
lexicons/            Lexicon schemas (online.7778777.lessdugin.*)           CC0-1.0
docs/spec/           Normative specification                                CC0-1.0
testdata/vectors/    Shared test vectors (Rust + TypeScript/wasm)            CC0-1.0
crates/atp           atproto primitives (DAG-CBOR, CID, CAR, MST, commits)  MIT OR Apache-2.0
crates/lexicon       record types, validator, record keys                   MIT OR Apache-2.0
crates/markdown      LFM/1 renderer (HTML+MathML, terminal, plain text)     AGPL-3.0-or-later
crates/core          engine: identity, sync, outbox, verification, fold     AGPL-3.0-or-later
crates/native        native host (HTTP, SQLite, keyring, OAuth)             AGPL-3.0-or-later
crates/cli, tui      the `lessdugin` binary                                 AGPL-3.0-or-later
crates/wasm          wasm bindings used by the SPA                          AGPL-3.0-or-later
crates/indexer       optional indexer                                       AGPL-3.0-or-later
web/, packages/      React SPA, engine worker, e2e devnet                    AGPL-3.0-or-later
```

## Development

Requirements: Rust (stable, with `wasm32-unknown-unknown`), Node 22 + pnpm, `wasm-bindgen-cli 0.2.129`.

```sh
pnpm install
cargo xtask wasm          # build the engine for the browser
pnpm --filter @lessdugin/web dev
cargo run -p lessdugin -- --help
cargo xtask ci            # everything CI runs
node packages/e2e/devnet.mjs   # local PLC + PDS with test accounts
```

## Licensing

Applications are licensed under the **GNU AGPL v3.0 or later** (see `LICENSE`). The lexicons,
specification and test vectors are dedicated to the public domain under **CC0-1.0**, and the
`lessdugin-atp` and `lessdugin-lexicon` crates are **MIT OR Apache-2.0**, so that anyone can build
a compatible client. License texts are in `LICENSES/`.
