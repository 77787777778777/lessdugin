// SPDX-License-Identifier: AGPL-3.0-or-later
// Notifications (social.md §16), computed locally by the engine.

import type { NotificationReason } from "@lessdugin/engine/types";
import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { ActorName } from "../components/ActorName.tsx";
import { RecordLink } from "../components/RecordLink.tsx";
import { ErrorView, Loading } from "../components/Status.tsx";
import { useEngine } from "../engine/context.tsx";
import { qk } from "../engine/queries.ts";
import { fullDate, shortAge } from "../lib/time.ts";
import { parseAtUri, recordKind } from "../lib/uri.ts";
import ui from "../styles/ui.module.css";
import styles from "./pages.module.css";

const REASON: Record<NotificationReason, { glyph: string; text: string }> = {
  reply: { glyph: "↩", text: "replied to you" },
  mention: { glyph: "@", text: "mentioned you" },
  quote: { glyph: "❝", text: "quoted you" },
  pingback: { glyph: "🔗", text: "linked to your item" },
  newComment: { glyph: "💬", text: "commented on a post you follow" },
  newPost: { glyph: "📝", text: "posted" },
  reacted: { glyph: "☺", text: "Reacts on your item" },
  modAction: { glyph: "🛡", text: "Moderation on your item" },
  karmaChange: { glyph: "±", text: "Karma change" },
};

/** A link text for a notification without a preview. */
function subjectLabel(uri: string): string {
  const p = parseAtUri(uri);
  switch (p ? recordKind(p.collection) : "other") {
    case "post":
      return "Open the post";
    case "comment":
      return "Open the comment";
    case "sequence":
      return "Open the sequence";
    case "forum":
      return "Open the forum";
    default:
      return "Open the record";
  }
}

export function NotificationsPage() {
  const engine = useEngine();
  const [all, setAll] = useState(true);
  const q = useQuery({
    queryKey: qk.notifications(all),
    queryFn: () => engine.notifications({ all, limit: 100 }),
  });
  return (
    <div className={ui.page}>
      <h1 className={ui.pageTitle}>Notifications</h1>
      <p className={ui.row}>
        <button
          type="button"
          className={all ? ui.chipActive : ui.chip}
          aria-pressed={all}
          onClick={() => setAll(true)}
        >
          All
        </button>
        <button
          type="button"
          className={!all ? ui.chipActive : ui.chip}
          aria-pressed={!all}
          onClick={() => setAll(false)}
        >
          Unread
        </button>
      </p>
      {q.isPending && <Loading />}
      {q.isError && <ErrorView error={q.error} />}
      {q.data && q.data.items.length === 0 && <p className={ui.muted}>Nothing new.</p>}
      <ul className={ui.list}>
        {q.data?.items.map((n) => {
          const r = REASON[n.reason];
          const isRecord = parseAtUri(n.subject) !== null;
          return (
            <li key={n.id} className={n.read ? styles.notification : styles.unreadNotification}>
              <span aria-hidden="true">{r.glyph}</span>
              <div>
                <p className={ui.small}>
                  {n.actor && <ActorName actor={n.actor} />} {r.text}
                  {!n.read && <span className="visually-hidden"> (unread)</span>}
                </p>
                {isRecord ? (
                  <RecordLink uri={n.subject} className={ui.small}>
                    {n.preview ?? subjectLabel(n.subject)}
                  </RecordLink>
                ) : (
                  n.preview && <p className={ui.small}>{n.preview}</p>
                )}
              </div>
              <time
                className={ui.muted}
                dateTime={new Date(n.atMs).toISOString()}
                title={fullDate(n.atMs)}
              >
                {shortAge(n.atMs)}
              </time>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
