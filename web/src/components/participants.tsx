// SPDX-License-Identifier: AGPL-3.0-or-later
// The forum participants known on the current screen (social.md §3.1 P), for the mention
// resolution budget of lfm-1.md §5.4 / threat-model INFRA-7 (F16): rendered bodies resolve
// mentioned identities eagerly only for participants. Every other mention keeps its short-DID
// placeholder until the reader hovers, focuses or clicks it, so an author cannot make each
// reader's browser contact a DID host of their choosing (`did:web:<unique>.attacker.example`).
//
// The set is a conservative subset of P built from what the screen already shows: the forum's
// owner and moderators (authorities) and the authors of items the fold lists (visible or
// collapsed; non-participants' items are hidden by V2). Without a provider the set is empty.

import type { ActorView, CommentNode, ForumView, ItemState } from "@lessdugin/engine/types";
import { createContext, type ReactNode, use, useMemo } from "react";

const EMPTY: ReadonlySet<string> = new Set();

const ParticipantsContext = createContext<ReadonlySet<string>>(EMPTY);

export function useParticipants(): ReadonlySet<string> {
  return use(ParticipantsContext);
}

const listed = (state: ItemState): boolean =>
  state.kind === "visible" || state.kind === "collapsed";

/** Participants evidenced by a forum view and listed items. */
export function participantsOf(
  forum: ForumView | undefined,
  items: readonly { author: ActorView; state: ItemState }[],
): Set<string> {
  const out = new Set<string>();
  if (forum) {
    out.add(forum.forum.owner.did);
    for (const m of forum.moderators) out.add(m.actor.did);
  }
  for (const item of items) if (listed(item.state)) out.add(item.author.did);
  return out;
}

export function ParticipantsProvider({
  forum,
  items,
  children,
}: {
  forum: ForumView | undefined;
  items: readonly ({ author: ActorView; state: ItemState } | CommentNode)[];
  children: ReactNode;
}) {
  const parent = useParticipants();
  const value = useMemo(() => {
    const set = participantsOf(forum, items);
    for (const did of parent) set.add(did);
    return set;
  }, [forum, items, parent]);
  return <ParticipantsContext value={value}>{children}</ParticipantsContext>;
}
