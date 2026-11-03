<!-- SPDX-License-Identifier: CC0-1.0 -->
# End-to-end tests on the local devnet

`devnet.mjs` starts a local atproto network for the end-to-end tests (ADR 0006): a PLC
directory and one PDS from `@atproto/dev-env` (`TestNetworkNoAppView`; lessdugin needs no
AppView), with the accounts `alice.test`, `bob.test` and `carol.test`, each with an app password.

```sh
node packages/e2e/devnet.mjs [--accounts alice,bob,carol] [--json devnet.json]
```

It prints (and with `--json` writes) `{pds, plc, accounts: {<name>: {handle, did, password,
appPassword}}, bootMs}` and runs until SIGINT or SIGTERM. Both services listen on
`http://localhost:<random port>`, and handles resolve through the PDS. The clients reach it
only in devnet mode (`lessdugin --dev`; ADR 0006, threat-model INFRA-9).

## One command

```sh
cargo xtask e2e            # everything below: --rust, then --web
cargo xtask e2e --rust     # the Rust e2e tests
cargo xtask e2e --web      # the SPA's devnet Playwright suite, including the parity scenario
cargo xtask e2e --parity   # only the M2 parity scenario
cargo xtask e2e --web --no-build   # skip the builds (they must be current)
cargo xtask e2e --cold-start   # opt in: the M6 cold paint (scripts/m6-cold-start.sh)
```

You need `pnpm install`, Chromium for Playwright and, for `--web` and `--parity`, the wasm-bindgen
CLI at the workspace pin (`cargo install wasm-bindgen-cli --version =0.2.129 --locked`).
Chromium comes from `PLAYWRIGHT_BROWSERS_PATH`, or `/opt/pw-browsers` when that is unset.
CI (`.github/workflows/e2e.yml`) runs `cargo xtask e2e --rust` and `cargo xtask e2e --web` as
two jobs and uploads `test-results/web-e2e-devnet`.

Every test starts its own devnet and stops it at the end. To reuse a running devnet, set
`LESSDUGIN_DEVNET_JSON=<the file devnet.mjs wrote>`. A fresh devnet is safer for the Rust
tests, which use fixed forum slugs.

### `--rust`

`LESSDUGIN_E2E=1` turns on the Rust tests that need the devnet. Without it, `cargo test` skips
them, so it stays offline. The list is `RUST_E2E` in `xtask/src/main.rs`:

| Package | Test | What |
|---|---|---|
| `lessdugin-testkit` | `e2e_devnet` | identity resolution, the login binding, `verify` |
| `lessdugin-testkit` (feature `http`) | `e2e_devnet_services` | the devnet's CORE+ services (below) against real writes: short Constellation and relay pages that still carry a cursor, both Jetstream wires, the noise dropped by the core's filter |
| `lessdugin-native` | `devnet` | the native host against the reference PDS |
| `lessdugin` | `e2e_devnet` | app-password and OAuth logins (headless Chromium approves at the PDS), concurrent refresh |
| `lessdugin` | `e2e_forum` | the CLI forum scenario, and parity between two CLI devices |
| `lessdugin` | `web_e2e_devnet` | `lessdugin web`: browser sign-in through `/bridge/login`, then writes through `/bridge/xrpc` |
| `lessdugin-tui` | `e2e_devnet` | carol replies through `$EDITOR` in the headless TUI |

The CLI's OAuth approver (`crates/cli/tests/support/oauth_approve.mjs`) loads Playwright from
`PLAYWRIGHT_MODULE`. pnpm keeps Playwright out of the root `node_modules`, so `cargo xtask e2e`
points that variable at `web/node_modules/@playwright/test`.

### `--web`

Runs the Playwright suite in `web/e2e/devnet` (`playwright.config.ts`) against the e2e build of
the SPA (`VITE_E2E=1`, `web/dist-e2e`). The pages are served with the development CSP of
threat-model §6.2, and Trusted Types are enforced. Any CSP or Trusted Types violation, page
error or console error fails a test.

| Spec | What |
|---|---|
| `hosted.spec.ts` | anonymous read; OAuth sign-in; join, comment, post, and an edit that writes a revision |
| `parity.spec.ts` | parity mode in the SPA: two fresh profiles read the same bytes |
| `bridge.spec.ts` | `lessdugin web` serving the SPA (needs a CLI that embeds the e2e build) |
| `scenario.spec.ts` | the M2 parity scenario (below) |

First, `cargo xtask e2e --web` builds these:

1. the engine wasm (`cargo xtask wasm`);
2. the e2e SPA (`pnpm --filter @lessdugin/web run build:e2e`);
3. the CLI, with `LESSDUGIN_WEB_DIST=web/dist-e2e` so that it embeds the e2e SPA and
   `bridge.spec.ts` runs. The next plain `cargo build -p lessdugin` embeds `web/dist` again;
4. the TUI parity driver (`cargo build -p lessdugin-tui --example parity_driver`);
5. the devnet's CORE+ services (`cargo build -p lessdugin-testkit --features http --example
   devnet_services`) and `lessdugin-indexer`, for the parity scenario's variants.

## The devnet's CORE+ services

`crates/testkit/examples/devnet_services.rs --pds <url> --json <file>` follows the devnet PDS's
firehose (`com.atproto.sync.subscribeRepos` from sequence 0) and serves, on loopback:

- a mini-Jetstream with both wires on one listener (v1 `/subscribe` with `requireHello` and
  `options_update`, v2 `subscribeEvents`), whose log also carries identity and account events of
  three unrelated DIDs (`NOISE_DIDS`), every 500 ms;
- a Constellation (`getBacklinks`, `getBacklinksCount`) and a relay (`listReposByCollection`)
  whose pages hold one entry and still carry a cursor, the last one to an empty final page.

It logs every request (`GET /_devnet/log`), so a test can tell which client contacted which
service; `GET /_devnet/record?uri=` says when the mirror holds a record. The services give hints
only: no frame carries record text.

## The M2 parity scenario

`web/e2e/devnet/scenario.spec.ts` covers `docs/plan.md` §4.2, text subset, which is an M2
acceptance item. Three users work on three clients, and every client runs in parity mode with
the same fixed time `t` (`engine-api.md` §2.1: `LESSDUGIN_PARITY_NOW_MS` for the CLI and the TUI,
`parityNowMs` for the e2e SPA build).

| Step | Who | Client | Action |
|---|---|---|---|
| 1 | alice | CLI | `forum create --mode approval --frontpage`, then `approve bob.test carol.test` |
| 2 | bob | SPA | OAuth sign-in, Join (the public-membership dialog), a new post |
| 3 | carol | TUI | opens the forum, then the post, presses `c` and writes the comment in `$EDITOR` |
| 4 | carol | TUI | votes, agrees and reacts through the keys |
| 5 | alice | CLI | hides carol's comment and pins the post |
| 6 | bob | SPA | edits the title and body with a summary: `applyWrites [revision, update]` |
| 6c, 6b | alice | SPA, CLI | applies the wiki-backed tag; pins the edited post again |
| 6d | a newcomer | PDS API | a fresh account joins and comments on bob's post |

The scenario runs four times: approval and open mode, each with `sources` direct and assisted.
Every client of both source variants is configured with the same CORE+ services above (the
relay, the Constellation and the devnet profile's Jetstream in `config.toml`; the SPA's source
options); only `sources` differs. In assisted mode every reader must contact them; in direct
mode nothing may, during the whole run. The newcomer is what only CORE+ can introduce: in open
mode its comment is in every assisted reader's thread (collapsed, as a new user's) and in no
direct reader's; in approval mode it is an applicant, whose content no reader is shown.

In the assisted variants `lessdugin-indexer --poll <pds> --forum <forum>` follows the devnet from
step 1. After the readers above, three more fresh readers (CLI, TUI, SPA) are configured with it
and open the forum with no prior sync: the TUI and the SPA hydrate their cold page from its
ordering and proof bundles (`listPosts`, `getRecordProofs`), and every reader's forum sync
polls its change feed (`getChanges`) before its ordinary PDS sync. The native readers reach it
through a recording proxy each, so the scenario requires those methods per client. The SPA's
cold page is read before that PDS sync can change it: until then the browser holds every
`describeRepo` and `listRecords` of a repo other than the forum owner's (the indexer, head
checks, DID documents, point fetches and the owner's sync go through, sync.md §14.3), and the
posts it lists must be the indexer-off list's, in its order, a proof-admitted one at the same
CID. Then the listings are released, and the converged views must equal the indexer-off views
byte for byte once the provenance badges (`verified`, `hint`, `stale`; decision U-D2) are
removed.

The TUI runs as `crates/tui/examples/parity_driver.rs`, which drives the headless TUI
(`lessdugin_tui::Headless`). It opens the engine the way `lessdugin --dev tui` does: the data
directory, the devnet and parity mode come from the same environment variables, it uses the
session that carol's `lessdugin --dev login` stored, and it writes through `$EDITOR`.

Then each client reads the forum as an anonymous reader, with a fresh data directory or browser
profile, and syncs it the way it does when the forum is opened:

| Client | Post list | Thread |
|---|---|---|
| CLI | `lessdugin --json ls <forum> --sort new --all` | `lessdugin --json read <post>` |
| TUI | the Forum screen's `PostList` (`parity_driver read`) | the Thread screen's `ThreadView` (`capture_thread`) |
| SPA | forum page, then the post page; the debug hook's `listPosts` | the debug hook's `getThread` (sort `magic`) |

All three outputs must be byte-identical:

- The SPA hook returns the compact dto/1 text, as `serde_json::to_string` writes it and the wasm
  boundary carries it.
- The TUI driver writes that same compact form, and also the pretty form exactly as the CLI
  prints it (`to_string_pretty` and a newline).
- The test checks three equalities for both the post list and the thread:
  - SPA == TUI compact;
  - CLI == TUI pretty;
  - CLI with its layout whitespace removed == SPA. The minify is lexical and leaves strings
    alone.

It also checks the content, so that equal outputs cannot all be empty:

- the edited title and body, by `bob.test`, with one revision;
- carol's comment, by `carol.test`;
- `completeness` of 3 of 3 repositories.

The outputs go to `test-results/web-e2e-devnet/<test>/parity/`:

- `cli.*.json`, `spa.*.json` and `tui/`, which holds the JSON and the TUI screens as text;
- `<name>.stderr.txt` for every process;
- `timings.json`;
- `result.json`, with the uris and whether the outputs are identical.

On a mismatch, the test writes unified diffs (`thread-cli-vs-tui.diff`, and so on) and attaches
them to the report. The SPA side is re-indented for reading; the comparison itself is on the
bytes. The step timings also go to the GitHub job summary.

To run the scenario by hand against a running devnet:

```sh
cargo build -p lessdugin && cargo build -p lessdugin-tui --example parity_driver
cargo build -p lessdugin-testkit --features http --example devnet_services
cargo build -p lessdugin-indexer --bin lessdugin-indexer
cargo xtask wasm && pnpm --filter @lessdugin/web run build:e2e
cd web && LESSDUGIN_DEVNET_JSON=/path/to/devnet.json PLAYWRIGHT_BROWSERS_PATH=/opt/pw-browsers \
  pnpm exec playwright test -c e2e/devnet/playwright.config.ts scenario.spec.ts
```

`LESSDUGIN_BIN`, `LESSDUGIN_TUI_DRIVER`, `LESSDUGIN_DEVNET_SERVICES` and `LESSDUGIN_INDEXER_BIN`
override the binaries. Their defaults are `target/debug/lessdugin`,
`target/debug/examples/parity_driver`, `target/debug/examples/devnet_services` and
`target/debug/lessdugin-indexer`. When a binary is missing,
the scenario is skipped, but it fails instead under CI or with `LESSDUGIN_PARITY_REQUIRED=1`,
which `cargo xtask e2e` sets.

## The M6 cold start (`--cold-start`)

`cargo xtask e2e --cold-start` runs `scripts/m6-cold-start.sh`: the signed loopback fixture
(`crates/indexer/examples/cold_start_devnet.rs`: 2,000 participants on 40 FakeNet PDSes, the
owner's post and one post by each of 30 members) is pre-indexed (minutes; outside the timing),
then `web/e2e/cold-start/cold-start.spec.ts` has three fresh Chromium contexts paint the forum's
first page (20 posts by 20 distinct participants) and requires each under 2 s, with a proof for
every post, a head check and a DID document for every author on screen, and no post point
fetch. The results go to `test-results/m6-cold-start/results.json`. It is not part of a bare
`cargo xtask e2e`; `.github/workflows/e2e.yml` runs it nightly (job `cold-start`).

## Other M2 acceptance checks outside this package

- **A 50-member FakePds forum cold-syncs natively in under 5 s:**
  - The test is `crates/cli/tests/cold_sync.rs`. It runs `lessdugin --dev sync` as an anonymous
    reader against FakePds over HTTP.
  - It runs in plain `cargo test`, and it prints and records the time to the job summary and to
    `LESSDUGIN_TIMINGS_FILE`.
  - On CI the bound is 3 × 5 s, because shared runners are slow and test builds are not
    optimised; a time over 5 s is then a warning.
- **The outbox survives `kill -9`:** `crates/cli/tests/outbox_kill.rs`.
- **Sync and outbox scenarios on MemHost and FakePds:** `crates/core/tests/sync.rs` and
  `outbox.rs`.
- **The wasm size report and budget (ADR 0002):** `cargo xtask wasm --budget --report`, which
  runs in the web job of `.github/workflows/ci.yml`.
