// SPDX-License-Identifier: AGPL-3.0-or-later
// Tag votes on a post (social.md §13.3): anyone but the author votes on whether a tag applies,
// which is how a reader applies a tag (there are no separate tag-relation records). A vote is a
// `#tagVote` entry in the viewer's public opinion record, held for the undo window like any
// opinion. The engine's views do not report the viewer's own tag votes, so the buttons show the
// votes made on this page; the tags shown as applied are the post's resolved tags.

import { OPINION_UNDO_WINDOW_MS, type TagVoteValue } from "@lessdugin/engine/types";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useId, useState } from "react";
import { useEngine } from "../engine/context.tsx";
import { qk } from "../engine/queries.ts";
import { writeStore } from "../engine/writes.ts";
import { cleanName } from "../lib/text.ts";
import ui from "../styles/ui.module.css";
import { usePublicAck } from "./PublicAck.tsx";
import { errorMessage } from "./Status.tsx";
import styles from "./TagVotes.module.css";
import { toasts } from "./Toast.tsx";

const CHOICES: { value: TagVoteValue; label: string; glyph: string }[] = [
  { value: "up", label: "Applies", glyph: "▲" },
  { value: "down", label: "Does not apply", glyph: "▼" },
];

export function TagVotes({
  post,
  forum,
  applied,
  signedIn,
}: {
  post: string;
  forum: string;
  /** The post's resolved tags (slugs). */
  applied: readonly string[];
  signedIn: boolean;
}) {
  const engine = useEngine();
  const navigate = useNavigate();
  const requestAck = usePublicAck();
  const selectId = useId();
  const [open, setOpen] = useState(false);
  const [mine, setMine] = useState<Record<string, TagVoteValue | null>>({});
  /** Slugs with a vote in flight (one at a time per tag; other tags stay usable). */
  const [busy, setBusy] = useState<ReadonlySet<string>>(() => new Set());
  const [extra, setExtra] = useState("");
  const tags = useQuery({
    queryKey: qk.tags(forum),
    queryFn: () => engine.tags(forum),
    enabled: open,
  });
  const nameOf = (slug: string): string => {
    const t = tags.data?.find((x) => x.slug === slug);
    return t ? cleanName(t.name) : slug;
  };
  const shown = [...new Set([...applied, ...Object.keys(mine)])];
  const others = (tags.data ?? []).filter((t) => !shown.includes(t.slug));

  const vote = async (slug: string, value: TagVoteValue | null): Promise<void> => {
    if (!signedIn) {
      void navigate({ to: "/login" });
      return;
    }
    if (busy.has(slug) || !(await requestAck("tagVote"))) return;
    setBusy((b) => new Set(b).add(slug));
    const before = mine[slug] ?? null;
    setMine((m) => ({ ...m, [slug]: value }));
    try {
      const ticket = await engine.opinion(post, {
        kind: "tag",
        slug,
        ...(value !== null ? { value } : {}),
      });
      writeStore.track(ticket, post);
      const toastId = toasts.show({
        message:
          value === null ? `Tag vote on “${nameOf(slug)}” removed` : "Tag vote recorded (public)",
        durationMs: OPINION_UNDO_WINDOW_MS,
        action: {
          label: "Undo",
          onAction: () => {
            engine
              .undo(ticket.id)
              .catch((e: unknown) => toasts.show({ message: errorMessage(e), tone: "error" }));
          },
        },
      });
      const final = await writeStore.wait(ticket.id);
      if (final.kind !== "done") {
        setMine((m) => ({ ...m, [slug]: before }));
        toasts.dismiss(toastId);
        toasts.show(
          final.kind === "failed"
            ? { message: `Tag vote failed: ${final.error.message}`, tone: "error" }
            : { message: "Tag vote undone" },
        );
      }
    } catch (e) {
      setMine((m) => ({ ...m, [slug]: before }));
      toasts.show({ message: errorMessage(e), tone: "error" });
    } finally {
      setBusy((b) => {
        const next = new Set(b);
        next.delete(slug);
        return next;
      });
    }
  };

  return (
    <div className={styles.wrap}>
      <button
        type="button"
        className={ui.ghost}
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
      >
        Vote on tags
      </button>
      {open && (
        <section className={styles.panel} aria-label="Tag votes">
          <p className={ui.small}>
            Vote on whether each tag applies to this post. Tag votes are public records in your
            repository.
          </p>
          {shown.length === 0 && <p className={ui.muted}>This post has no tags yet.</p>}
          <ul className={styles.list}>
            {shown.map((slug) => {
              const current = mine[slug] ?? null;
              return (
                <li key={slug} className={styles.item}>
                  <span className={styles.name}>
                    <bdi>{nameOf(slug)}</bdi>
                  </span>
                  {CHOICES.map((c) => (
                    <button
                      key={c.value}
                      type="button"
                      className={current === c.value ? ui.chipActive : ui.chip}
                      aria-pressed={current === c.value}
                      disabled={busy.has(slug)}
                      onClick={() => void vote(slug, current === c.value ? null : c.value)}
                    >
                      <span aria-hidden="true">{c.glyph}</span> {c.label}
                    </button>
                  ))}
                </li>
              );
            })}
          </ul>
          {others.length > 0 && (
            <form
              className={ui.row}
              onSubmit={(e) => {
                e.preventDefault();
                if (extra === "") return;
                setExtra("");
                void vote(extra, "up");
              }}
            >
              <label htmlFor={selectId} className={ui.small}>
                Apply another tag
              </label>
              <select
                id={selectId}
                className={ui.input}
                value={extra}
                onChange={(e) => setExtra(e.target.value)}
              >
                <option value="">Choose a tag…</option>
                {others.map((t) => (
                  <option key={t.slug} value={t.slug}>
                    {cleanName(t.name)}
                  </option>
                ))}
              </select>
              <button
                type="submit"
                className={ui.button}
                disabled={extra === "" || busy.has(extra)}
              >
                Vote to apply
              </button>
            </form>
          )}
        </section>
      )}
    </div>
  );
}
