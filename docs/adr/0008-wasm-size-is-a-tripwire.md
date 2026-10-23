<!-- SPDX-License-Identifier: CC0-1.0 -->
# ADR 0008 — The wasm size limit is a tripwire, not a budget

- Status: Accepted
- Date: 2026-10-23
- Supersedes: ADR 0002 decision 4 (the 900 KB gz target and the 1.1 MB gz CI failure line) and
  the 900 KB condition of ADR 0002 rule 5

## Context

ADR 0002 kept the synthesis' "900 KB gzipped target, CI fails above 1.1 MB" after estimating that
the engine's own code would take about 490 KB gz. The M2 engine came in larger: the release module
is 3.2 MB raw and 1.23 MB gzipped after wasm-opt, with the `wasm` profile already at opt-level z,
fat LTO, one codegen unit and `panic = "abort"`. About 545 KB gz of it is engine code, 100 KB serde
and 104 KB markdown and math.

The number was never derived from a requirement. Its only purpose is the SPA's cold start, and
that cost is small: the module is compiled with streaming compilation in a Web Worker, the UI
paints its placeholder first, and the browser caches the module after the first visit (about one
second extra on a mobile 4G connection, once). Cutting the core to fit the old line would cost
real design effort (fewer serde and BTreeMap instantiations, hand-written encoders) for little
user-visible benefit.

## Decision

1. CI always reports the raw and gzipped size of the release module (after wasm-opt).
2. `cargo xtask wasm` **warns** above 1.5 MB gzipped or 4 MB raw.
3. `cargo xtask wasm --budget` (and therefore `cargo xtask ci`) **fails** only above 2.5 MB
   gzipped. That line catches something linked in by mistake (tokio, reqwest, ammonia/html5ever,
   debug info), not organic growth.
4. ADR 0002 rule 6 (size hygiene in the wasm-safe crates) still applies to new code.
5. ADR 0002 rule 5 (a lazily loaded `verify.wasm`) is evaluated only if the module passes the
   warning line and `verify`'s marginal size is at least 100 KB gz.

## Consequences

- The M2 acceptance criterion "CI reports the wasm size" holds with these limits
  (`docs/plan.md` M2).
- Size regressions stay visible in every CI run; a deliberate cut can be scheduled later if real
  users report slow first loads.
