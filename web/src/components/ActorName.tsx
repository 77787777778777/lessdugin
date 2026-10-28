// SPDX-License-Identifier: AGPL-3.0-or-later
// A user's name: always inside <bdi> (threat-model SPA-5), cleaned of bidi controls,
// zero-width characters and badge glyphs. The handle appears only when it is verified both
// ways (the DTO omits it otherwise); else the short DID. That identity is always shown, next
// to a self-chosen display name, which is never shown alone (threat-model ID-5): a display
// name can say anything, another user's name or handle included.

import type { ActorView } from "@lessdugin/engine/types";
import { Link } from "@tanstack/react-router";
import { cx } from "../lib/cx.ts";
import { cleanName } from "../lib/text.ts";
import styles from "./ActorName.module.css";

export function actorIdent(actor: ActorView): string {
  return actor.handleState === "verified" && actor.handle ? `@${actor.handle}` : actor.shortDid;
}

export function actorLabel(actor: ActorView): string {
  const name = actor.displayName ? cleanName(actor.displayName) : "";
  return name !== "" ? name : actorIdent(actor);
}

export function ActorName({
  actor,
  link = true,
  showIdent = true,
  className,
}: {
  actor: ActorView;
  link?: boolean | undefined;
  /** Kept for callers that pass it; the identity is shown by default (ID-5). */
  showIdent?: boolean | undefined;
  className?: string | undefined;
}) {
  const name = actor.displayName ? cleanName(actor.displayName) : "";
  const ident = actorIdent(actor);
  const unverified = actor.handleState !== "verified";
  const title = unverified ? `${actor.did} (handle not verified)` : actor.did;
  const main = <bdi>{name !== "" ? name : ident}</bdi>;
  return (
    <span className={cx(styles.actor, className)}>
      {link ? (
        <Link to="/u/$actor" params={{ actor: actor.did }} className={styles.name} title={title}>
          {main}
        </Link>
      ) : (
        <span className={styles.name} title={title}>
          {main}
        </span>
      )}
      {showIdent && name !== "" && (
        <span className={styles.ident}>
          <bdi>{ident}</bdi>
        </span>
      )}
    </span>
  );
}
