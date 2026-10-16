// SPDX-License-Identifier: AGPL-3.0-or-later
// The engine worker's script URL and the `lessdugin-worker` Trusted Types policy
// (threat-model.md SPA-2): `createScriptURL(u)` returns `u` only if it is exactly this build's
// worker script URL (absolute); anything else throws. Under `require-trusted-types-for
// 'script'`, `new Worker(url)` accepts only a TrustedScriptURL, so this policy is the only way
// the engine worker can start. Kept apart from `client.ts` so the app can create the policy at
// startup without loading Comlink.

import workerAsset from "./worker.ts?worker&url";

/** This build's worker script URL, absolute. */
export const WORKER_SCRIPT_URL: string = new URL(workerAsset, import.meta.url).href;

export interface WorkerUrlPolicy {
  createScriptURL(input: string): unknown;
}

interface TrustedTypesFactory {
  createPolicy(
    name: string,
    rules: { createScriptURL: (input: string) => string },
  ): WorkerUrlPolicy;
}

const POLICY_KEY = Symbol.for("lessdugin.trusted-types.lessdugin-worker");
type PolicyHolder = { [POLICY_KEY]?: WorkerUrlPolicy | null };

/** `u` if it is this build's worker script URL; throws otherwise. */
export function checkWorkerScriptUrl(u: string): string {
  if (u === WORKER_SCRIPT_URL) return u;
  throw new TypeError(`lessdugin-worker: refusing a script URL that is not the engine worker`);
}

/** Creates the `lessdugin-worker` policy once (call at startup). `null` without Trusted Types. */
export function workerUrlPolicy(): WorkerUrlPolicy | null {
  const holder = globalThis as unknown as PolicyHolder;
  const existing = holder[POLICY_KEY];
  if (existing !== undefined) return existing;
  const tt = (globalThis as unknown as { trustedTypes?: TrustedTypesFactory }).trustedTypes;
  const policy = tt?.createPolicy
    ? tt.createPolicy("lessdugin-worker", { createScriptURL: checkWorkerScriptUrl })
    : null;
  holder[POLICY_KEY] = policy;
  return policy;
}

/** The worker script URL as the Worker constructor must receive it (a TrustedScriptURL under
 * Trusted Types, the plain string otherwise). */
export function workerScriptUrl(): string {
  const policy = workerUrlPolicy();
  return (policy ? policy.createScriptURL(WORKER_SCRIPT_URL) : WORKER_SCRIPT_URL) as string;
}
