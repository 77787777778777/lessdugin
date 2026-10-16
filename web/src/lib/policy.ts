// SPDX-License-Identifier: AGPL-3.0-or-later
import type { EffectivePolicy } from "@lessdugin/engine/types";

export type VotingMode = "karmaOnly" | "twoAxis" | "reacts";

/** A value from the effective policy's name/value pairs. */
export function policyParam(policy: EffectivePolicy | undefined, name: string): string | undefined {
  return policy?.params.find(([k]) => k === name)?.[1];
}

/** `policy.voting` (data-model.md §6.1; default `reacts`). */
export function votingMode(policy: EffectivePolicy | undefined): VotingMode {
  const v = policyParam(policy, "voting");
  return v === "karmaOnly" || v === "twoAxis" ? v : "reacts";
}
