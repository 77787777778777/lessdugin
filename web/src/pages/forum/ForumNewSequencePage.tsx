// SPDX-License-Identifier: AGPL-3.0-or-later
// Creating a sequence in the forum (engine-api.md §3.5 `seq_create`).

import type { SequenceDraft } from "@lessdugin/engine/types";
import { useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { SequenceFormView } from "../../components/SequenceForm.tsx";
import { toasts } from "../../components/Toast.tsx";
import { useEngine } from "../../engine/context.tsx";
import { qk, useActiveAccount } from "../../engine/queries.ts";
import { useSendWrite } from "../../engine/queued.ts";
import { EMPTY_SEQUENCE_FORM } from "../../lib/sequences.ts";
import { parseAtUri } from "../../lib/uri.ts";
import ui from "../../styles/ui.module.css";
import { useForumView } from "./forumContext.tsx";

export function ForumNewSequencePage() {
  const engine = useEngine();
  const qc = useQueryClient();
  const navigate = useNavigate();
  const send = useSendWrite();
  const { forum } = useForumView();
  const active = useActiveAccount();
  if (active.data === null) return <p className={ui.notice}>Sign in to create a sequence.</p>;
  const create = async (draft: SequenceDraft): Promise<void> => {
    const uri = await send(await engine.seqCreate(draft), "Your sequence");
    void qc.invalidateQueries({ queryKey: qk.sequences(forum.uri) });
    const p = uri !== null ? parseAtUri(uri) : null;
    if (!p) {
      void navigate({ to: "/outbox" });
      return;
    }
    toasts.show({ message: "Sequence created (public)", tone: "success" });
    void navigate({ to: "/s/$author/$rkey", params: { author: p.did, rkey: p.rkey } });
  };
  return (
    <section className={ui.page} aria-label="New sequence">
      <h2 className={ui.sectionTitle}>New sequence</h2>
      <SequenceFormView
        forum={forum.uri}
        initial={EMPTY_SEQUENCE_FORM}
        submitLabel="Create sequence"
        onSubmit={create}
      />
    </section>
  );
}
