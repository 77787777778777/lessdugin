<!-- SPDX-License-Identifier: CC0-1.0 -->
# ADR NNNN — <decision, stated as its outcome>

- Status: Proposed | Accepted | Accepted, pending user run | Rejected | Superseded by ADR NNNN
- Date: YYYY-MM-DD
- Spikes: S<n> (spike ADRs only; omit otherwise)
- Amends: D<n> in ADR 0001, ADR NNNN, `docs/spec/<doc>.md` §<n> (omit if none)

<!--
How to use this template (delete this comment in a real ADR):

- Copy to docs/adr/NNNN-<short-kebab-title>.md. NNNN is the next free number, assigned in order of
  acceptance. 0000 is this template and 0001 is the v1 decision log.
- One decision per ADR. Keep it short: an ADR records why, the spec records what.
- Required sections: Context, Decision, Consequences. Spike ADRs also have Method and Results.
  "Options considered" is optional; use it when more than one option was seriously weighed.
- A decision that changes behaviour lands in the same commit as the spec change and the re-blessed
  vectors (docs/plan.md §2.2). If it changes the meaning of a released behaviour ID, it must
  introduce a new ID instead (docs/spec/README.md §3).
- An accepted ADR is not rewritten. Later changes go in a new ADR that supersedes or amends it.
  Only the Status line, and errata marked as such, may be edited afterwards.
- "Accepted, pending user run" means: decided and built against the devnet, with a step that needs
  the user (real accounts, a device, DNS, publishing) still to run. List that step under
  Consequences and in docs/plan.md §8.
- Write normatively where the ADR states rules (MUST, SHOULD, MAY as in RFC 2119 / RFC 8174), and
  cite spec sections, critique IDs (F*, H*, M*, X*, C*) and decision IDs (D*) instead of repeating
  them.
-->

## Context

What forces the decision: the problem, the constraints, the spec sections, decisions and critique
IDs involved, and what is unknown.

## Options considered

1. **Option A.** One line on what it is; pros; cons.
2. **Option B.** …

## Method

*(Spikes only.)* What was run, against what (devnet, read-only public services, production
accounts), with which tool versions, and the request totals for anything that touched public
infrastructure. Throwaway scripts stay outside the repository.

## Results

*(Spikes only.)* Measurements and observed behaviour, as tables where possible. Facts only.

## Decision

The chosen option, as numbered, testable statements. Each one names the spec section it changes.

## Consequences

- What changes in the spec, the code, the vectors and CI, and in which milestone.
- What becomes easier or harder.
- What remains open, including any pending user run.
