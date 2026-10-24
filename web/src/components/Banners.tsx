// SPDX-License-Identifier: AGPL-3.0-or-later
// The `banners` of a ForumView, PostList, ThreadView or DiscussionList (social.md §4.11,
// engine-api.md): `moderationUnavailable` (a degraded view: items whose moderation cannot be
// checked stay hidden) and `newerRules` (the forum's rules changed after items were judged).
// Unknown codes render their message as-is; every banner keeps its `code` as `data-banner` so
// the e2e specs and styles can address it.

import type { Banner } from "@lessdugin/engine/types";
import { cx } from "../lib/cx.ts";
import styles from "./Banners.module.css";

/** The banner kinds the engine emits, and what each means for the reader. */
export const BANNER_KIND: Record<string, { tone: "warn" | "info"; hint: string }> = {
  moderationUnavailable: {
    tone: "warn",
    hint: "Some items stay hidden until the moderation data loads.",
  },
  newerRules: {
    tone: "info",
    hint: "Some items were judged under an earlier version of this forum's rules.",
  },
  relayDependent: {
    tone: "info",
    hint: "",
  },
};

export function Banners({ banners }: { banners: readonly Banner[] | undefined }) {
  if (banners === undefined || banners.length === 0) return null;
  return (
    <div className={styles.banners} role="note">
      {banners.map((b) => {
        const kind = BANNER_KIND[b.code];
        return (
          <p
            key={b.code}
            className={cx(styles.banner, kind?.tone === "warn" ? styles.warn : styles.info)}
            data-banner={b.code}
          >
            {b.message}
            {kind?.hint !== undefined && kind.hint !== "" && (
              <span className={styles.hint}> {kind.hint}</span>
            )}
          </p>
        );
      })}
    </div>
  );
}
