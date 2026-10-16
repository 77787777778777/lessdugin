// SPDX-License-Identifier: AGPL-3.0-or-later
// Trust and status badges (synthesis §1.2; lfm-1.md §9.1). They render only in this fixed
// column, as ASCII bracketed words, optionally preceded by a glyph from BADGE_GLYPHS.

import type { Badges as BadgeFlags, Completeness } from "@lessdugin/engine/types";
import { type BadgeGlyph, GLYPH } from "../lib/badges.ts";
import { cx } from "../lib/cx.ts";
import styles from "./Badges.module.css";

export interface BadgeDef {
  key: string;
  label: string;
  glyph?: BadgeGlyph;
  description: string;
  tone: "good" | "warn" | "neutral";
}

/** The badges to show, in a fixed order. */
export function badgeList(
  badges: BadgeFlags | undefined,
  completeness?: Completeness | undefined,
): BadgeDef[] {
  const out: BadgeDef[] = [];
  const b = badges ?? {};
  if (b.verified) {
    out.push({
      key: "verified",
      label: "[verified]",
      glyph: GLYPH.check,
      description: "Proven by a signed commit, with a verified identity document.",
      tone: "good",
    });
  }
  if (b.edited) {
    out.push({
      key: "edited",
      label: "[edited]",
      description: "Edited after it was first published.",
      tone: "neutral",
    });
  }
  if (b.stale) {
    out.push({
      key: "stale",
      label: "[stale]",
      glyph: GLYPH.warning,
      description: "The author's server could not be reached; showing the last copy fetched.",
      tone: "warn",
    });
  }
  if (completeness?.partial) {
    out.push({
      key: "partial",
      label: `[partial ${completeness.reposComplete}/${completeness.reposTotal}]`,
      glyph: GLYPH.warning,
      description: `Only ${completeness.reposComplete} of ${completeness.reposTotal} participants' repositories are synced.`,
      tone: "warn",
    });
  }
  if (b.futureDated) {
    out.push({
      key: "futureDated",
      label: "[future-dated]",
      glyph: GLYPH.warning,
      description:
        "The record claims a creation time in the future; it is ranked by when it was first seen.",
      tone: "warn",
    });
  }
  if (b.identityUnverified) {
    out.push({
      key: "identityUnverified",
      label: "[identity unverified]",
      glyph: GLYPH.cross,
      description: "The author's identity could not be verified.",
      tone: "warn",
    });
  }
  if (b.inconsistentTimestamps) {
    out.push({
      key: "inconsistentTimestamps",
      label: "[inconsistent timestamps]",
      glyph: GLYPH.warning,
      description: "The record's timestamps disagree with each other.",
      tone: "warn",
    });
  }
  if (b.hint) {
    out.push({
      key: "hint",
      label: "[hint]",
      description: "A placeholder from a non-authoritative source; its text is not shown.",
      tone: "neutral",
    });
  }
  return out;
}

export function Badges({
  badges,
  completeness,
  className,
}: {
  badges?: BadgeFlags | undefined;
  completeness?: Completeness | undefined;
  className?: string | undefined;
}) {
  const list = badgeList(badges, completeness);
  if (list.length === 0) return null;
  return (
    <span className={cx(styles.column, className)}>
      {list.map((b) => (
        <span
          key={b.key}
          className={cx(
            styles.badge,
            b.tone === "good" && styles.good,
            b.tone === "warn" && styles.warn,
          )}
          title={b.description}
          data-badge={b.key}
        >
          {b.glyph && (
            <span className={styles.glyph} aria-hidden="true">
              {b.glyph}
            </span>
          )}
          {b.label}
        </span>
      ))}
    </span>
  );
}
