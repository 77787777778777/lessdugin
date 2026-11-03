#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Real signed-loopback M6 cold-start acceptance. Pre-indexing is intentionally outside the
# browser timing and can take several minutes. No production endpoint is configured.
# Run by `cargo xtask e2e --cold-start` (and nightly by .github/workflows/e2e.yml, job
# cold-start); see packages/e2e/README.md.
set -euo pipefail

# Waits until the fixture (pid $1) is ready: it writes its JSON ($2) completely and only then
# prints its ready line to its log ($3), so neither a file left by an earlier run nor one still
# being written is taken. Bounded by the fixture's own progress, not a claimed browser time
# budget. (A timed `read` from /dev/null returns at once at end of file, so it never waited.)
wait_for_fixture() {
  local pid=$1 fixture=$2 log=$3
  for _ in $(seq 1 900); do
    if ! kill -0 "$pid" 2>/dev/null; then
      cat "$log" >&2
      return 1
    fi
    if grep -q '^M6 fixture ready:' "$log" && [ -s "$fixture" ]; then
      return 0
    fi
    sleep 1
  done
  cat "$log" >&2
  echo "signed fixture was not ready after 15 minutes" >&2
  return 1
}
# Sourced (xtask's test of wait_for_fixture): define the function only.
[[ "${BASH_SOURCE[0]}" == "$0" ]] || return 0

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
# The state directory persists between local runs: an earlier run's fixture names endpoints
# that are gone.
rm -f "$fixture"
./target/release/examples/cold_start_devnet --json "$fixture" >"$state/fixture.log" 2>&1 &
pid=$!
cleanup() {
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}
trap cleanup EXIT
wait_for_fixture "$pid" "$fixture" "$state/fixture.log"
cargo xtask wasm
pnpm --filter @lessdugin/web run build:e2e
pnpm --filter @lessdugin/web exec playwright test -c e2e/cold-start/cold-start.config.ts
printf 'M6 cold-start evidence: %s\n' "$results"
