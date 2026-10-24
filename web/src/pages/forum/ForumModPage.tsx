// SPDX-License-Identifier: AGPL-3.0-or-later
// The mod queue (authorities) and the public mod log (social.md §4.12). The queue is
// authorities-only: the engine answers `blocked` for anyone else, which this page shows as an
// explanation rather than an error. Log entries carry their status (effective, overridden,
// lapsed, ...), the winning rank of an override and whether the assertion was author-scope.

import type { ModLogStatus } from "@lessdugin/engine/types";
import { useQuery } from "@tanstack/react-query";
import { ActorName } from "../../components/ActorName.tsx";
import { RecordLink } from "../../components/RecordLink.tsx";
import { ErrorView, errorMessage, Loading } from "../../components/Status.tsx";
import { useEngine } from "../../engine/context.tsx";
import { qk } from "../../engine/queries.ts";
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
function isBlocked(e: unknown): boolean {
  return errorMessage(e).toLowerCase().includes("authorities");
}

function ModQueue({ uri, authority }: { uri: string; authority: boolean }) {
  const engine = useEngine();
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
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </>
  );
}

export function ForumModPage() {
  const engine = useEngine();
  const view = useForumView();
  const uri = view.forum.uri;
  const authority = view.viewer !== undefined ? view.viewer.moderate : isAuthorityRole(view.myRole);
  const log = useQuery({ queryKey: qk.modLog(uri), queryFn: () => engine.modLog(uri) });
  return (
    <section className={ui.pageWide} aria-label="Moderation">
      <ModQueue uri={uri} authority={authority} />
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
              </tr>
            </thead>
            <tbody>
              {log.data.entries.map((e) => (
                <tr key={`${e.atMs}:${e.subject}:${e.action}`}>
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
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}
