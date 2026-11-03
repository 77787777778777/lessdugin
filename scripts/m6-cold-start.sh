#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Real signed-loopback M6 cold-start acceptance. Pre-indexing is intentionally outside the
# browser timing and can take several minutes. No production endpoint is configured.
# Run by `cargo xtask e2e --cold-start` (and nightly by .github/workflows/e2e.yml, job
# cold-start); see packages/e2e/README.md.
set -euo pipefail
cd "$(dirname "$0")/.."
export LD_LIBRARY_PATH="$(dirname "$(gcc -print-file-name=libstdc++.so.6)")${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
state="${LESSDUGIN_COLD_START_STATE:-${TMPDIR:-/tmp}/lessdugin-m6-cold-start}"
mkdir -p "$state"
fixture="$state/fixture.json"
results="${LESSDUGIN_COLD_START_RESULTS:-$state/results.json}"
export LESSDUGIN_COLD_START_JSON="$fixture"
export LESSDUGIN_COLD_START_RESULTS="$results"
export LESSDUGIN_COLD_START_COMMAND="scripts/m6-cold-start.sh"
cargo build --release -p lessdugin-indexer --features devnet --example cold_start_devnet
./target/release/examples/cold_start_devnet --json "$fixture" >"$state/fixture.log" 2>&1 &
pid=$!
cleanup() {
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}
trap cleanup EXIT
for _ in $(seq 1 900); do
  if ! kill -0 "$pid" 2>/dev/null; then
    cat "$state/fixture.log" >&2
    exit 1
  fi
  [ -s "$fixture" ] && break
  # This wait is bounded by the fixture's own progress, not a claimed browser time budget.
  # (A timed `read` from /dev/null returns at once at end of file, so it never waited.)
  sleep 1
done
if [ ! -s "$fixture" ]; then
  cat "$state/fixture.log" >&2
  echo "signed fixture was not ready after 15 minutes" >&2
  exit 1
fi
cargo xtask wasm
pnpm --filter @lessdugin/web run build:e2e
pnpm --filter @lessdugin/web exec playwright test -c e2e/cold-start/cold-start.config.ts
printf 'M6 cold-start evidence: %s\n' "$results"
