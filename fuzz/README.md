<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->

# Fuzzing

[cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) (libFuzzer) targets for every surface that
parses untrusted bytes: repo CARs and proofs, DAG-CBOR, commits, MST walks, lexicon records and
LFM/1 bodies. They cover the M4 acceptance item "a 10-minute fuzz run (CAR, CBOR, MST, lexicon
validator, renderer) without panics" (`docs/plan.md`).

This directory is its own Cargo workspace, outside the main one: cargo-fuzz needs a nightly
toolchain and sanitizer flags. `Cargo.lock` started as a copy of the main lock, so the crates
under test build against the same dependency versions.

## Running

```sh
rustup toolchain install nightly --profile minimal
cargo install cargo-fuzz --locked

cargo xtask fuzz                       # every target, 600 s each (the acceptance run)
cargo xtask fuzz render --seconds 60   # one target
cargo xtask fuzz mst car --jobs 2      # libFuzzer -fork=2
cargo xtask fuzz --list
cargo xtask fuzz dagcbor -- -runs=0    # extra libFuzzer flags after --
```

`cargo xtask fuzz` first runs the seed generator (`seeds/`). It writes seeds from the vectors and
fixtures to `corpus/<target>/` and adds the kept reproducers from `regressions/<target>/`. Then it
runs each target with `cargo +nightly fuzz run` (set `LESSDUGIN_FUZZ_TOOLCHAIN` to use another
nightly; CI pins one). The build goes to `$CARGO_TARGET_DIR/fuzz` (default `target/fuzz`). The
command fails if any target stops on a crash, a timeout or an out-of-memory error. libFuzzer
writes the input to `artifacts/<target>/`.

The corpora and artifacts are local working state (git-ignored).
`.github/workflows/fuzz.yml` runs every target for 10 minutes each night, one runner per target,
and uploads the artifacts when a target fails.

## Targets and contracts

Every target is **panic-free by contract**. Errors are fine. Any of the following is a bug in the
crate under test:

- a panic, including an arithmetic overflow, since fuzz builds keep debug assertions and overflow
  checks;
- a stack overflow (the parsers and the renderer run on a 1 MiB stack, wasm32's default);
- a timeout or an out-of-memory error above the limits below.

Some targets also assert properties the crates document.

| Target | Input | Exercises | Asserted besides "no panic" |
|---|---|---|---|
| `car` | raw CAR bytes | `car::read_car` (proof and bundle caps); `proof::verify_proof` for the keys the CAR's own commit lists and for fixed keys; `ProofBundle::parse` and `verify_item`; `mst::list_entries` | every block verifies against its CID and fits the caps; write → read round trip |
| `dagcbor` | raw bytes | `dagcbor::{decode_with, validate_with}` under the default, strict and tiny limits; the raw iterators; `datamodel` (JSON ⇄ IPLD, records, blobs); `Commit::decode`; `mst::Node::decode` | `validate_with` gives exactly `decode_with`'s verdict; only canonical bytes decode (re-encoding gives the input back); IPLD → JSON → IPLD is the identity; a commit's unsigned bytes are strict DAG-CBOR |
| `commit` | a raw commit block | `Commit::decode`, `Commit::verify` (several DIDs, keys and clocks) | a decoded commit, re-signed over its reconstructed unsigned bytes, never fails the signature check, and verifies when its version, DID and rev are well formed |
| `mst` | structured (`arbitrary`) | a repo built from the input with every link computed: the canonical tree of a key set (`mst::build`) or hand-built nodes (wrong layers, prefixes and field types; dangling and wrong-codec links; chains past the depth limit). A signed commit, or a broken one. Dropped, corrupted and duplicated blocks. `mst::prove`, `mst::list_entries`, `verify_proof`, `verify_proof_in` with read caps, and indexer bundles | for a clean canonical repo, the walk is exact: listing, presence and absence, the proven record CID, and bundle claims. Read caps only ever turn a verdict into a cap error |
| `lexicon` | raw JSON (a record) | `validate::Validator` (record, defs, defaults, capped options); `validity::check`; `Record` (de)serialisation; the record's data model and CID. The collection comes from `$type` (either namespace) or from the input | the `Options` caps hold; an encoded record decodes back to the same value |
| `render` | raw body bytes (a `textBlob`) | `render` (and `render_error` for invalid UTF-8); math-core and Temml modes; the printer (colour, plain and narrow; with handles); `term_sanitize`; `derive_metadata`; `plain_projection`; `excerpt`; `build_quote` / `resolve_quote` / `qkey`; `safe_href`, `short_did`, `osc8_uri`. Media items are created for every `cid:` reference in the input | the HTML passes `check_html` and `ammonia(html) == html`; P and the excerpt hold no controls or non-space whitespace; the excerpt is at most 500 graphemes and 5,000 bytes; no terminal control appears in the `StyledDoc` or the `TermText`; printed output passes `check_printer_output`, and has no escape at all without colour; `derive_metadata` equals `render`; a quote built from a selection of P resolves to that selection |

Limits, set per target by `cargo xtask fuzz`:

| Target | `-max_len` | `-timeout` (s per input) |
|---|---|---|
| `car` | 16,384 | 10 |
| `commit` | 4,096 | 10 |
| `dagcbor` | 16,384 | 10 |
| `lexicon` | 16,384 | 10 |
| `mst` | 16,384 | 10 |
| `render` | 32,768 | 10 |

`-rss_limit_mb=2048` applies to every target, and with it `-malloc_limit_mb`, so under
AddressSanitizer a single allocation over 2 GiB is reported at once. That makes the
"no preallocation from declared lengths" rule of the decoders observable. The timeouts are
generous for these sizes. The slowest legitimate inputs (worst-case math, `lfm-1.md` §7.2) take
milliseconds natively, so anything near 10 s under instrumentation is super-linear work.

## Seeds and dictionaries

| Target | Seeds (`seeds/src/main.rs`) |
|---|---|
| `car` | the captured live proofs (`testdata/interop/live`); generated present and absent proof CARs; an indexer bundle |
| `commit` | the commit of every seed CAR; generated commits (unknown fields, `prev` forms, wrong version) |
| `dagcbor` | the interop data-model fixtures (CBOR, and the valid JSON encoded); every block of the seed CARs |
| `lexicon` | the records in the shared vectors, in their own and the build namespace (up to 3,000), and the live record |
| `render` | every input of the `lfm1/`, `xss/`, `terminal/` and `mathml/` vectors (bodies, TeX, URLs, quote targets, names) |
| `mst` | none: the input is structured, and libFuzzer finds the shapes |

`dicts/<target>.dict` holds the tokens of each format: CBOR heads, CID prefixes and field names;
JSON tokens and the lexicons' property names, NSIDs and known values; LFM/1 constructs.
`car`, `commit` and `dagcbor` start from a valid commit signed by a fixed test key
(`src/common.rs`). Generated repos verify under that key, and the live proof under its own.

## When a target fails

1. Reproduce: `cargo +nightly fuzz run --fuzz-dir fuzz <target> fuzz/artifacts/<target>/<file>`.
2. Minimise: `cargo +nightly fuzz tmin --fuzz-dir fuzz <target> <file>`.
3. Fix the cause in the crate (`crates/atp`, `crates/lexicon`, `crates/markdown`), not in the
   harness, unless the harness asserted something the spec does not promise. Add a regression
   test there with the minimised input.
4. Keep the minimised input as `regressions/<target>/<short-name>`. Seeding copies it into the
   corpus, so every later run replays it first. `regressions/README.md` lists each one with its
   test.
