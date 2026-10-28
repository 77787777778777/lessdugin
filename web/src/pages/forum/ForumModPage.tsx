// SPDX-License-Identifier: AGPL-3.0-or-later
// The mod queue (authorities) and the public mod log (social.md §4.12). The queue is
// authorities-only: the engine answers `blocked` for anyone else, which this page shows as an
// explanation rather than an error. Each queue item takes one action, with the reason typed
// above the queue: approve or reject an applicant, keep or hide a newcomer's item, assert a
// stale positive assertion (or `#tag.head`) again on the item's new version, open the tag of a
// tag-head conflict. Log entries carry their status
// (effective, overridden, lapsed, ...), the winning rank of an override and whether the
// assertion was author-scope; the log pages through the engine's cursor, and the viewer's own
// assertions can be removed from it. Authorities can also adopt a former moderator's
// assertions in effect (§4.10).

import {
  isEngineError,
  type ModLogEntry,
  type ModLogStatus,
  type ModQueueItem,
  type WriteTicket,
} from "@lessdugin/engine/types";
import { useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useState } from "react";
import { ActorName } from "../../components/ActorName.tsx";
import { useSendModAction } from "../../components/ModMenu.tsx";
import { usePublicAck } from "../../components/PublicAck.tsx";
import { RecordLink } from "../../components/RecordLink.tsx";
import { ErrorView, errorMessage, Loading } from "../../components/Status.tsx";
import { toasts } from "../../components/Toast.tsx";
import { useEngine } from "../../engine/context.tsx";
import { qk, useActiveAccount } from "../../engine/queries.ts";
import { writeStore } from "../../engine/writes.ts";
import { entryActionKey, recordActionValue, tagSlug } from "../../lib/modActions.ts";
import { fullDate } from "../../lib/time.ts";
import { isDid, parseAtUri } from "../../lib/uri.ts";
import ui from "../../styles/ui.module.css";
import { isAuthorityRole, useForumView } from "./forumContext.tsx";

/** The status labels of a mod-log entry (`ModLogEntry.status`, social.md §4.12). */
export const MOD_STATUS_LABEL: Record<ModLogStatus, string> = {
  effective: "effective",
  overridden: "overridden",
  lapsed: "lapsed",
  expired: "expired",
  stale: "stale",
  unauthorised: "unauthorised",
  reservedUnknown: "unknown status",
  edited: "edited",
  removed: "removed",
};

function Subject({ subject }: { subject: string }) {
  if (parseAtUri(subject)) return <RecordLink uri={subject}>{subject.split("/").pop()}</RecordLink>;
  if (isDid(subject)) return <span className={ui.mono}>{subject}</span>;
  return <span className={ui.mono}>{subject}</span>;
}

/** Whether the engine refused a query with `blocked` (the queue's authorities-only answer). */
export function isBlocked(e: unknown): boolean {
  return isEngineError(e) && e.code === "blocked";
}

/** Waits for each write and says how it ended. */
async function settle(tickets: WriteTicket[], done: string): Promise<void> {
  for (const t of tickets) {
    writeStore.track(t);
    const final = await writeStore.wait(t.id);
    if (final.kind === "failed") throw final.error;
  }
  toasts.show({ message: done, tone: "success" });
}

/** The one action of a queue item (social.md §4.12). */
function QueueAction({
  forum,
  item,
  reason,
}: {
  forum: string;
  item: ModQueueItem;
  reason: string;
}) {
  const engine = useEngine();
  const requestAck = usePublicAck();
  const send = useSendModAction();
  const [busy, setBusy] = useState(false);
  const run = (f: () => Promise<void>) => (): void => {
    setBusy(true);
    f()
      .catch((e: unknown) => toasts.show({ message: errorMessage(e), tone: "error" }))
      .finally(() => setBusy(false));
  };
  const why = reason.trim() !== "" ? { reason: reason.trim() } : {};
  const hide = (hidden: boolean, label: string) =>
    run(async () => {
      await send(
        {
          kind: "assert",
          forum,
          scope: "forum",
          subject: item.subject,
          action: { kind: "hide", hidden, spam: false, showReason: true },
          ...why,
        },
        label,
      );
    });
  switch (item.kind) {
    case "applicant":
      return (
        <span className={ui.row}>
          <button
            type="button"
            className={ui.ghost}
            disabled={busy}
            onClick={run(async () => {
              // approving writes a public `#access` record; rejecting is local
              if (!(await requestAck("modAction"))) return;
              await settle(await engine.approve(forum, [item.subject]), "Applicant approved");
            })}
          >
            Approve
          </button>
          <button
            type="button"
            className={ui.ghost}
            disabled={busy}
            onClick={run(() =>
              engine.reject(forum, [item.subject]).then((t) => settle(t, "Applicant rejected")),
            )}
          >
            Reject
          </button>
        </span>
      );
    case "newcomer":
      return (
        <span className={ui.row}>
          <button type="button" className={ui.ghost} disabled={busy} onClick={hide(false, "Keep")}>
            Keep
          </button>
          <button type="button" className={ui.ghost} disabled={busy} onClick={hide(true, "Hide")}>
            Hide
          </button>
        </span>
      );
    case "stale":
      // the assertion again, on the item as it is now (the engine binds it to the cached CID)
      return item.uri === undefined ? null : (
        <button
          type="button"
          className={ui.ghost}
          disabled={busy}
          onClick={run(async () => {
            const raw = await engine.raw(item.uri ?? "");
            const action = recordActionValue(raw.record);
            if (!action) throw new Error("this action cannot be asserted again from here");
            await send(
              { kind: "assert", forum, scope: "forum", subject: item.subject, action, ...why },
              "Asserted again",
            );
          })}
        >
          Assert again on this version
        </button>
      );
    case "conflict": {
      // unordered `#tag.head` assertions at the top rank (social.md §4.5). Resolving it means
      // naming one revision in an own `#tag.head`, which takes the tag's wiki history (M5);
      // the queue opens the tag
      const slug = tagSlug(item.subject);
      const f = parseAtUri(forum);
      return slug === null || !f ? null : (
        <Link
          to="/f/$owner/$slug/t/$tag"
          params={{ owner: f.did, slug: f.rkey, tag: slug }}
          className={ui.link}
        >
          Open the tag
        </Link>
      );
    }
    default:
      return null;
  }
}

function ModQueue({ uri, authority }: { uri: string; authority: boolean }) {
  const engine = useEngine();
  const [reason, setReason] = useState("");
  const queue = useQuery({
    queryKey: qk.modQueue(uri),
    queryFn: () => engine.modQueue(uri),
    // The queue is authorities-only; for anyone else the engine answers `blocked`, which is
    // rendered below rather than as a failed query.
    enabled: authority,
    retry: (failureCount, error) => !isBlocked(error) && failureCount < 1,
  });
  if (!authority) {
    return (
      <>
        <h2 className={ui.sectionTitle}>Mod queue</h2>
        <p className={ui.muted}>
          The mod queue is available to this forum's moderators and admins only.
        </p>
      </>
    );
  }
  return (
    <>
      <h2 className={ui.sectionTitle}>Mod queue</h2>
      <label className={ui.field}>
        <span className={ui.label}>Reason for the actions below (public)</span>
        <input className={ui.input} value={reason} onChange={(e) => setReason(e.target.value)} />
      </label>
      {queue.isPending && <Loading />}
      {queue.isError &&
        (isBlocked(queue.error) ? (
          <p className={ui.muted}>{errorMessage(queue.error)}</p>
        ) : (
          <ErrorView error={queue.error} />
        ))}
      {queue.data && queue.data.items.length === 0 && (
        <p className={ui.muted}>Nothing to review.</p>
      )}
      {queue.data && queue.data.items.length > 0 && (
        <div className={ui.tableScroll}>
          <table className={ui.table}>
            <thead>
              <tr>
                <th scope="col">Kind</th>
                <th scope="col">Subject</th>
                <th scope="col">Who</th>
                <th scope="col">When</th>
                <th scope="col">Reason</th>
                <th scope="col">Detail</th>
                <th scope="col">Action</th>
              </tr>
            </thead>
            <tbody>
              {queue.data.items.map((item) => (
                <tr key={`${item.kind}:${item.subject}:${item.uri ?? ""}:${item.atMs ?? ""}`}>
                  <td>{item.kind}</td>
                  <td>
                    <Subject subject={item.subject} />
                    {item.uri !== undefined && item.uri !== item.subject && (
                      <span className={ui.small}>
                        {" "}
                        about <RecordLink uri={item.uri}>this action</RecordLink>
                      </span>
                    )}
                  </td>
                  <td>{item.actor !== undefined && <ActorName actor={item.actor} />}</td>
                  <td className={ui.muted}>{item.atMs !== undefined && fullDate(item.atMs)}</td>
                  <td>{item.reason}</td>
                  <td className={ui.muted}>{item.detail}</td>
                  <td>
                    <QueueAction forum={uri} item={item} reason={reason} />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </>
  );
}

/** Re-issues a former moderator's assertions that are in effect (social.md §4.10). */
function Adopt({ uri }: { uri: string }) {
  const engine = useEngine();
  const requestAck = usePublicAck();
  const [former, setFormer] = useState("");
  const [busy, setBusy] = useState(false);
  const adopt = async (): Promise<void> => {
    // the copies are the adopter's own public modActions
    if (!(await requestAck("modAction"))) return;
    setBusy(true);
    try {
      await settle([await engine.modAdopt(uri, former.trim())], "Assertions adopted");
      setFormer("");
    } catch (e) {
      toasts.show({ message: errorMessage(e), tone: "error" });
    } finally {
      setBusy(false);
    }
  };
  return (
    <>
      <h2 className={ui.sectionTitle}>Adopt a former moderator's actions</h2>
      <p className={ui.small}>
        A moderator's actions lapse when they leave the moderators list. Adopting re-issues the ones
        in effect as your own records.
      </p>
      <div className={ui.row}>
        <input
          className={ui.input}
          aria-label="Former moderator (handle or DID)"
          placeholder="handle or DID"
          value={former}
          onChange={(e) => setFormer(e.target.value)}
        />
        <button
          type="button"
          className={ui.button}
          disabled={busy || former.trim() === ""}
          onClick={() => void adopt()}
        >
          Adopt
        </button>
      </div>
    </>
  );
}

/** Removes the viewer's own assertion of a mod-log entry (data-model.md §8.4). */
function RemoveOwn({ uri, entry }: { uri: string; entry: ModLogEntry }) {
  const send = useSendModAction();
  const qc = useQueryClient();
  const key = entryActionKey(entry);
  if (key === null || entry.status === "removed") return null;
  const remove = async (): Promise<void> => {
    try {
      const ok = await send(
        {
          kind: "remove",
          forum: uri,
          scope: entry.authorScope === true ? "author" : "forum",
          subject: entry.subject,
          actionKey: key,
        },
        "Removed",
      );
      if (ok) await qc.invalidateQueries({ queryKey: qk.modLog(uri) });
    } catch (e) {
      toasts.show({ message: errorMessage(e), tone: "error" });
    }
  };
  return (
    <button type="button" className={ui.ghost} onClick={() => void remove()}>
      Remove
    </button>
  );
}

export function ForumModPage() {
  const engine = useEngine();
  const view = useForumView();
  const active = useActiveAccount();
  const me = active.data?.did ?? null;
  const uri = view.forum.uri;
  const authority = view.viewer !== undefined ? view.viewer.moderate : isAuthorityRole(view.myRole);
  // every page of the log, through the engine's cursor (the CLI's `mod log -n` reaches as far)
  const log = useInfiniteQuery({
    queryKey: qk.modLog(uri),
    queryFn: ({ pageParam }) => engine.modLog(uri, pageParam),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.cursor,
  });
  const entries = log.data?.pages.flatMap((p) => p.entries) ?? [];
  return (
    <section className={ui.pageWide} aria-label="Moderation">
      <ModQueue uri={uri} authority={authority} />
      {authority && <Adopt uri={uri} />}
      <h2 className={ui.sectionTitle}>Mod log</h2>
      <p className={ui.small}>
        Every moderation action is a public record; lapsed entries are by former moderators.
      </p>
      {log.isPending && <Loading />}
      {log.isError && <ErrorView error={log.error} />}
      {log.data && (
        <div className={ui.tableScroll}>
          <table className={ui.table}>
            <thead>
              <tr>
                <th scope="col">When</th>
                <th scope="col">Moderator</th>
                <th scope="col">Action</th>
                <th scope="col">Subject</th>
                <th scope="col">Status</th>
                <th scope="col">Reason</th>
                {me !== null && <th scope="col">Yours</th>}
              </tr>
            </thead>
            <tbody>
              {entries.map((e) => (
                <tr key={e.uri}>
                  <td>{fullDate(e.atMs)}</td>
                  <td>
                    <ActorName actor={e.actor} />
                    {e.lapsed && <span className={ui.muted}> (lapsed)</span>}
                    {e.authorScope === true && (
                      <span className={ui.muted} title="An author-scope assertion (social.md §4.9)">
                        {" "}
                        (author-scope)
                      </span>
                    )}
                  </td>
                  <td>
                    {e.action}: {e.value}
                    {e.adoptedFrom && <span className={ui.muted}> (adopted)</span>}
                  </td>
                  <td>
                    <Subject subject={e.subject} />
                  </td>
                  <td>
                    <span data-status={e.status}>
                      {MOD_STATUS_LABEL[e.status] ?? e.status}
                      {e.status === "overridden" && e.rank !== undefined && (
                        <span className={ui.muted}> by rank {e.rank}</span>
                      )}
                    </span>
                  </td>
                  {/* A `#hide` reason with `showReason: false` reaches non-authorities without
                    `reason`; only authorities ever see one (social.md §4.12). */}
                  <td className={ui.muted}>{e.reason}</td>
                  {me !== null && (
                    <td>{e.actor.did === me && <RemoveOwn uri={uri} entry={e} />}</td>
                  )}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {log.hasNextPage && (
        <button
          type="button"
          className={ui.button}
          disabled={log.isFetchingNextPage}
          onClick={() => void log.fetchNextPage()}
        >
          {log.isFetchingNextPage ? "Loading…" : "Load more"}
        </button>
      )}
    </section>
  );
}
