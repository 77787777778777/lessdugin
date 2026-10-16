// SPDX-License-Identifier: AGPL-3.0-or-later
// Accounts carry only did/handle/pds (`AccountView`); names come from the identity cache.

import type { AccountView, ActorView } from "@lessdugin/engine/types";
import { useQuery } from "@tanstack/react-query";
import { useEngine } from "../engine/context.tsx";
import { qk } from "../engine/queries.ts";

/** lfm-1.md §5.4 `short_did`. */
export function shortDid(did: string): string {
  if (did.startsWith("did:plc:")) return `did:plc:${did.slice(8, 16)}…`;
  return did.length <= 40 ? did : `${did.slice(0, 39)}…`;
}

function fallbackActor(a: AccountView): ActorView {
  const v: ActorView = {
    did: a.did,
    handleState: a.handle !== undefined ? "verified" : "unverified",
    shortDid: shortDid(a.did),
  };
  if (a.handle !== undefined) v.handle = a.handle;
  return v;
}

/** The account's identity for display (resolved lazily; the account's own data meanwhile). */
export function useAccountActor(account: AccountView | null | undefined): ActorView | null {
  const engine = useEngine();
  const did = account?.did ?? "";
  const q = useQuery({
    queryKey: qk.actor(did),
    queryFn: () => engine.resolveActor(did),
    enabled: did !== "",
    staleTime: 5 * 60_000,
  });
  if (!account) return null;
  return q.data ?? fallbackActor(account);
}
