// SPDX-License-Identifier: AGPL-3.0-or-later
// The mod queue (authorities) and the public mod log (social.md §4.12).

import { useQuery } from "@tanstack/react-query";
import { ActorName } from "../../components/ActorName.tsx";
import { RecordLink } from "../../components/RecordLink.tsx";
import { ErrorView, Loading } from "../../components/Status.tsx";
import { useEngine } from "../../engine/context.tsx";
import { qk } from "../../engine/queries.ts";
import { fullDate } from "../../lib/time.ts";
import { isDid, parseAtUri } from "../../lib/uri.ts";
import ui from "../../styles/ui.module.css";
import { isAuthorityRole, useForumView } from "./forumContext.tsx";

function Subject({ subject }: { subject: string }) {
  if (parseAtUri(subject)) return <RecordLink uri={subject}>{subject.split("/").pop()}</RecordLink>;
  if (isDid(subject)) return <span className={ui.mono}>{subject}</span>;
  return <span className={ui.mono}>{subject}</span>;
}

export function ForumModPage() {
  const engine = useEngine();
  const view = useForumView();
  const uri = view.forum.uri;
  const authority = isAuthorityRole(view.myRole);
  const queue = useQuery({
    queryKey: qk.modQueue(uri),
    queryFn: () => engine.modQueue(uri),
    enabled: authority,
  });
  const log = useQuery({ queryKey: qk.modLog(uri), queryFn: () => engine.modLog(uri) });
  return (
    <section className={ui.pageWide} aria-label="Moderation">
      {authority && (
        <>
          <h2 className={ui.sectionTitle}>Mod queue</h2>
          {queue.isPending && <Loading />}
          {queue.isError && <ErrorView error={queue.error} />}
          {queue.data && queue.data.items.length === 0 && (
            <p className={ui.muted}>Nothing to review.</p>
          )}
          {queue.data && queue.data.items.length > 0 && (
            <table className={ui.table}>
              <thead>
                <tr>
                  <th scope="col">Kind</th>
                  <th scope="col">Subject</th>
                  <th scope="col">Reason</th>
                  <th scope="col">Detail</th>
                </tr>
              </thead>
              <tbody>
                {queue.data.items.map((item) => (
                  <tr key={`${item.kind}:${item.subject}`}>
                    <td>{item.kind}</td>
                    <td>
                      <Subject subject={item.subject} />
                    </td>
                    <td>{item.reason}</td>
                    <td className={ui.muted}>{item.detail}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </>
      )}
      <h2 className={ui.sectionTitle}>Mod log</h2>
      <p className={ui.small}>
        Every moderation action is a public record; lapsed entries are by former moderators.
      </p>
      {log.isPending && <Loading />}
      {log.isError && <ErrorView error={log.error} />}
      {log.data && (
        <table className={ui.table}>
          <thead>
            <tr>
              <th scope="col">When</th>
              <th scope="col">Moderator</th>
              <th scope="col">Action</th>
              <th scope="col">Subject</th>
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
                </td>
                <td>
                  {e.action}: {e.value}
                  {e.adoptedFrom && <span className={ui.muted}> (adopted)</span>}
                </td>
                <td>
                  <Subject subject={e.subject} />
                </td>
                <td className={ui.muted}>{e.reason}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}
