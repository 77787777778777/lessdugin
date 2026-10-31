// SPDX-License-Identifier: AGPL-3.0-or-later
// The `banners` of a ForumView, PostList, ThreadView or DiscussionList (social.md §4.11,
// engine-api.md): `moderationUnavailable` (a degraded view: items whose moderation cannot be
// checked stay hidden), `newerRules` (the declaration has values this client does not know,
// §1.6), `authorityInactive` (sync.md §3.4) and `relayDependent`, the open-mode label of
// threat-model L7, which the forum header adds itself when the view does not carry it.
// Unknown codes render their message as-is; every banner keeps its `code` as `data-banner` so
// the e2e specs and styles can address it.

import type { Banner, MembershipMode } from "@lessdugin/engine/types";
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
    hint: "Settings this client does not know take their safe fallbacks; a newer client shows the forum as its owner wrote it.",
  },
  authorityInactive: {
    tone: "warn",
    hint: "A moderator's account is inactive; their actions still count for a while.",
  },
  relayDependent: {
    tone: "info",
    hint: "Only approval mode is complete without third-party servers: brand-new members are found through them, which may delay or miss their content.",
  },
};

/** The open-mode label (threat-model L7, social.md §3.2). */
export const OPEN_MODE_LABEL = "Open mode relies on Bluesky relay, Jetstream and Constellation.";

/** `banners` with the open-mode label for an open forum, unless the engine already sent one. */
export function withOpenModeLabel(
  banners: readonly Banner[] | undefined,
  membership: MembershipMode,
): readonly Banner[] | undefined {
  if (membership !== "open" || banners?.some((b) => b.code === "relayDependent")) return banners;
  return [...(banners ?? []), { code: "relayDependent", message: OPEN_MODE_LABEL }];
}

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
