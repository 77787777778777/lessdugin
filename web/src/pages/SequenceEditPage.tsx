// SPDX-License-Identifier: AGPL-3.0-or-later
// Editing an own sequence (engine-api.md §3.5 `seq_edit`): the form starts from the record
// (`raw`), so entries the sequence view leaves out (hidden or unavailable posts) are kept, and
// the save replaces the title, introduction and chapters as one write.

import type { SequenceDraft } from "@lessdugin/engine/types";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { getRouteApi, useNavigate } from "@tanstack/react-router";
import { SequenceFormView } from "../components/SequenceForm.tsx";
import { ErrorView, Loading } from "../components/Status.tsx";
import { toasts } from "../components/Toast.tsx";
import { useEngine } from "../engine/context.tsx";
import { qk, useActiveAccount, useParsedRef } from "../engine/queries.ts";
import { useSendWrite } from "../engine/queued.ts";
import { stringField } from "../lib/records.ts";
import { sequenceFormOf } from "../lib/sequences.ts";
import { parseAtUri } from "../lib/uri.ts";
import ui from "../styles/ui.module.css";

const route = getRouteApi("/s/$author/$rkey/edit");

/** The editor for the sequence `uri` (exported for its tests). */
export function EditSequence({ uri }: { uri: string }) {
  const engine = useEngine();
  const qc = useQueryClient();
  const navigate = useNavigate();
  const send = useSendWrite();
  const active = useActiveAccount();
  const raw = useQuery({ queryKey: qk.raw(uri), queryFn: () => engine.raw(uri) });
  if (raw.isPending || active.isPending) return <Loading />;
  if (raw.isError) return <ErrorView page error={raw.error} title="Sequence not found" />;
  const p = parseAtUri(uri);
  const forum = stringField(raw.data.record, "forum");
  const form = sequenceFormOf(raw.data.record);
  const why =
    p === null || p.did !== active.data?.did
      ? "Only the author can edit a sequence."
      : !forum || !form
        ? "This sequence's record is not one this app can edit."
        : null;
  if (why !== null || !forum || !form || !p) {
    return <ErrorView page error={new Error(why ?? "")} title="This sequence cannot be edited" />;
  }
  const save = async (draft: SequenceDraft): Promise<void> => {
    const done = await send(await engine.seqEdit(uri, draft), "Your sequence edit", uri);
    void qc.invalidateQueries({ queryKey: qk.sequence(uri) });
    void qc.invalidateQueries({ queryKey: qk.raw(uri) });
    void qc.invalidateQueries({ queryKey: qk.sequences(forum) });
    if (done === null) {
      void navigate({ to: "/outbox" });
      return;
    }
    toasts.show({ message: "Sequence saved (public)", tone: "success" });
    void navigate({ to: "/s/$author/$rkey", params: { author: p.did, rkey: p.rkey } });
  };
  return (
    <section className={ui.page} aria-label="Edit sequence">
      <h1 className={ui.pageTitle}>Edit sequence</h1>
      <SequenceFormView forum={forum} initial={form} submitLabel="Save sequence" onSubmit={save} />
    </section>
  );
}

export function SequenceEditPage() {
  const { author, rkey } = route.useParams();
  const ref = useParsedRef(`#/s/${author}/${rkey}`);
  if (ref.isPending) return <Loading />;
  if (ref.isError) return <ErrorView page error={ref.error} />;
  if (ref.data.kind !== "sequence")
    return <ErrorView page error={new Error("This address is not a sequence.")} />;
  return <EditSequence uri={ref.data.uri} />;
}
