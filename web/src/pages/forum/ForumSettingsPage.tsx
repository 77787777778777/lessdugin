// SPDX-License-Identifier: AGPL-3.0-or-later
// The forum declaration as the fold reads it: about, moderators and the effective policy. The
// owner can edit the declaration (`forum_update`, starting from their own record) and add or
// remove moderators (`mods_set`; a removed moderator's actions are adopted first, social.md
// §4.10).

import type { ForumDraft, ModRole, WriteTicket } from "@lessdugin/engine/types";
import { useQuery } from "@tanstack/react-query";
import { Fragment, useId, useState } from "react";
import { ActorName } from "../../components/ActorName.tsx";
import { ForumForm, forumDraftOf } from "../../components/ForumForm.tsx";
import { usePublicAck } from "../../components/PublicAck.tsx";
import { RenderedHtml } from "../../components/RenderedHtml.tsx";
import { ErrorView, errorMessage, Loading } from "../../components/Status.tsx";
import { toasts } from "../../components/Toast.tsx";
import { useEngine } from "../../engine/context.tsx";
import { qk } from "../../engine/queries.ts";
import { writeStore } from "../../engine/writes.ts";
import { cleanName } from "../../lib/text.ts";
import { parseAtUri } from "../../lib/uri.ts";
import ui from "../../styles/ui.module.css";
import styles from "../pages.module.css";
import { useForumView } from "./forumContext.tsx";

/** Runs a write and waits for its outcome; true when it was done. */
async function runWrite(
  start: () => Promise<WriteTicket>,
  subject: string,
  done: string,
): Promise<boolean> {
  try {
    const ticket = await start();
    writeStore.track(ticket, subject);
    const final = await writeStore.wait(ticket.id);
    if (final.kind === "failed") throw final.error;
    if (final.kind !== "done") return false;
    toasts.show({ message: done, tone: "success" });
    return true;
  } catch (e) {
    toasts.show({ message: errorMessage(e), tone: "error" });
    return false;
  }
}

function EditDeclaration({ uri, onDone }: { uri: string; onDone: () => void }) {
  const engine = useEngine();
  const raw = useQuery({ queryKey: qk.raw(uri), queryFn: () => engine.raw(uri) });
  const tags = useQuery({ queryKey: qk.tags(uri), queryFn: () => engine.tags(uri) });
  if (raw.isPending) return <Loading label="Loading the declaration…" />;
  if (raw.isError) return <ErrorView error={raw.error} />;
  const initial = forumDraftOf(raw.data.record, parseAtUri(uri)?.rkey ?? "");
  if (!initial) return <ErrorView error={new Error("The forum declaration is not readable.")} />;
  const save = async (draft: ForumDraft): Promise<boolean> => {
    const ok = await runWrite(
      () => engine.forumUpdate(uri, draft),
      uri,
      "Forum updated (the declaration is public)",
    );
    if (ok) onDone();
    return ok;
  };
  return (
    <ForumForm
      initial={initial}
      creating={false}
      tags={tags.data}
      submitLabel="Save the declaration"
      onSubmit={save}
      onCancel={onDone}
    />
  );
}

function AddModerator({ uri }: { uri: string }) {
  const engine = useEngine();
  const requestAck = usePublicAck();
  const ids = { who: useId(), role: useId(), title: useId() };
  const [who, setWho] = useState("");
  const [role, setRole] = useState<ModRole>("moderator");
  const [title, setTitle] = useState("");
  const [busy, setBusy] = useState(false);
  const add = async (): Promise<void> => {
    if (who.trim() === "" || busy) return;
    if (!(await requestAck("modAction"))) return;
    setBusy(true);
    try {
      const actor = await engine.resolveActor(who.trim());
      const t = title.trim();
      const ok = await runWrite(
        () =>
          engine.modsSet(uri, {
            kind: "add",
            did: actor.did,
            role,
            ...(t !== "" ? { title: t } : {}),
          }),
        uri,
        "Moderator added",
      );
      if (ok) {
        setWho("");
        setTitle("");
      }
    } catch (e) {
      toasts.show({ message: errorMessage(e), tone: "error" });
    } finally {
      setBusy(false);
    }
  };
  return (
    <form
      className={ui.row}
      aria-label="Add a moderator"
      onSubmit={(e) => {
        e.preventDefault();
        void add();
      }}
    >
      <label className="visually-hidden" htmlFor={ids.who}>
        Handle or DID
      </label>
      <input
        id={ids.who}
        className={ui.input}
        placeholder="handle or did:…"
        value={who}
        autoCapitalize="none"
        spellCheck={false}
        onChange={(e) => setWho(e.target.value)}
      />
      <label className="visually-hidden" htmlFor={ids.role}>
        Role
      </label>
      <select
        id={ids.role}
        className={ui.input}
        value={role}
        onChange={(e) => setRole(e.target.value === "admin" ? "admin" : "moderator")}
      >
        <option value="moderator">Moderator</option>
        <option value="admin">Admin</option>
      </select>
      <label className="visually-hidden" htmlFor={ids.title}>
        Title (optional)
      </label>
      <input
        id={ids.title}
        className={ui.input}
        placeholder="Title (optional)"
        value={title}
        maxLength={64}
        onChange={(e) => setTitle(e.target.value)}
      />
      <button type="submit" className={ui.button} disabled={busy || who.trim() === ""}>
        Add moderator
      </button>
    </form>
  );
}

export function ForumSettingsPage() {
  const engine = useEngine();
  const requestAck = usePublicAck();
  const view = useForumView();
  const uri = view.forum.uri;
  const owner = view.myRole === "owner";
  const [editing, setEditing] = useState(false);
  const removeModerator = async (did: string): Promise<void> => {
    if (!(await requestAck("modAction"))) return;
    await runWrite(
      () => engine.modsSet(uri, { kind: "remove", did, skipAdopt: false }),
      uri,
      "Moderator removed (their actions were adopted)",
    );
  };
  return (
    <section className={ui.page} aria-label="About this forum">
      {owner && (
        <div className={ui.row}>
          {!editing && (
            <button type="button" className={ui.button} onClick={() => setEditing(true)}>
              Edit the forum
            </button>
          )}
        </div>
      )}
      {owner && editing && (
        <>
          <h2 className={ui.sectionTitle}>Edit the declaration</h2>
          <EditDeclaration uri={uri} onDone={() => setEditing(false)} />
        </>
      )}
      {view.about && (
        <>
          <h2 className={ui.sectionTitle}>About</h2>
          <div className={ui.card}>
            <RenderedHtml html={view.about.html} />
          </div>
        </>
      )}
      <h2 className={ui.sectionTitle}>Moderators</h2>
      <ul className={ui.list}>
        <li>
          <ActorName actor={view.forum.owner} showIdent /> <span className={ui.muted}>owner</span>
        </li>
        {view.moderators.map((m) => (
          <li key={m.actor.did}>
            <ActorName actor={m.actor} showIdent />{" "}
            <span className={ui.muted}>{m.title ? <bdi>{cleanName(m.title)}</bdi> : m.role}</span>
            {!m.shown && <span className={ui.muted}> (not a member yet)</span>}
            {owner && (
              <>
                {" "}
                <button
                  type="button"
                  className={ui.link}
                  onClick={() => void removeModerator(m.actor.did)}
                >
                  Remove
                </button>
              </>
            )}
          </li>
        ))}
      </ul>
      {owner && <AddModerator uri={uri} />}
      <h2 className={ui.sectionTitle}>Policy</h2>
      <p className={ui.small}>
        The fold always reads the current declaration, so every policy change applies retroactively.
      </p>
      <dl className={styles.kv}>
        <dt>Moderation</dt>
        <dd>{view.policy.moderation}</dd>
        <dt>Karma</dt>
        <dd>{view.policy.karma}</dd>
        <dt>Ranking</dt>
        <dd>{view.policy.ranking}</dd>
        <dt>React palette</dt>
        <dd>{view.policy.reactPalette}</dd>
        {view.policy.params.map(([k, v]) => (
          <Fragment key={k}>
            <dt>{k}</dt>
            <dd>{v}</dd>
          </Fragment>
        ))}
      </dl>
    </section>
  );
}
