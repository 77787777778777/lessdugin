// SPDX-License-Identifier: AGPL-3.0-or-later
// The forum declaration as the fold reads it: about, moderators and the effective policy.

import { Fragment } from "react";
import { ActorName } from "../../components/ActorName.tsx";
import { RenderedHtml } from "../../components/RenderedHtml.tsx";
import ui from "../../styles/ui.module.css";
import styles from "../pages.module.css";
import { useForumView } from "./forumContext.tsx";

export function ForumSettingsPage() {
  const view = useForumView();
  return (
    <section className={ui.page} aria-label="About this forum">
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
            <span className={ui.muted}>{m.title ?? m.role}</span>
            {!m.shown && <span className={ui.muted}> (not a member yet)</span>}
          </li>
        ))}
      </ul>
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
