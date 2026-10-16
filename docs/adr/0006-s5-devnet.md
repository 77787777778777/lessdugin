<!-- SPDX-License-Identifier: CC0-1.0 -->
# ADR 0006 — S5: local devnet for end-to-end tests

- Status: Accepted
- Date: 2026-10-16

## Context

End-to-end and cross-client parity tests (docs/plan.md, M2+) need a real PDS and PLC directory
that CI can start in seconds, without touching production infrastructure. lessdugin needs no
AppView, so a PLC + PDS pair is sufficient.

## Method

`packages/e2e/devnet.mjs` starts `@atproto/dev-env` 0.6.20 `TestNetworkNoAppView` and creates
accounts with plain XRPC calls over `fetch` (`com.atproto.server.createAccount`,
`createAppPassword`). Plain XRPC was chosen after `network.pds.getClient()` turned out to have
changed shape in this dev-env release; fetch keeps the script independent of SDK churn.

```sh
node packages/e2e/devnet.mjs --accounts alice,bob,carol --json devnet.json
```

## Results

- Boot plus three accounts with app passwords: **0.9 s** locally (`bootMs: 889`).
- PDS and PLC listen on `http://localhost:<random port>`; handles are `<name>.test` and resolve
  through the PDS (`com.atproto.identity.resolveHandle`), not DNS.
- `better-sqlite3` is the only dependency needing an install script; it is allow-listed in
  `pnpm-workspace.yaml` (`onlyBuiltDependencies`).
- The workflow `.github/workflows/e2e.yml` boots the devnet on every PR and nightly.

## Consequences

- Clients need a **devnet mode** (`--dev`, or a config profile) in which handles resolve via the
  PDS's `resolveHandle`, the PLC base URL is configurable, and plain `http://` endpoints are
  allowed. It must never be enabled implicitly.
- OAuth against the devnet (http issuer) is checked as part of S1/S2 in M1; app passwords cover the
  devnet path in the meantime.
- The devnet is the target for S12 and the cursor-range probe in S7 (ADR 0005, 0004).
