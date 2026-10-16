// SPDX-License-Identifier: AGPL-3.0-or-later
// Table of contents from `RenderedBody.toc` (lfm-1.md §10.6). Entries scroll inside the post's
// own rendered root; content ids are never looked up document-wide (§4.6).

import type { TocEntry } from "@lessdugin/engine/types";
import { cleanName } from "../lib/text.ts";
import styles from "../pages/pages.module.css";

export function Toc({
  entries,
  onSelect,
}: {
  entries: TocEntry[];
  onSelect: (id: string) => void;
}) {
  const min = Math.min(...entries.map((e) => e.level));
  return (
    <nav className={styles.toc} aria-label="Table of contents">
      <p className={styles.tocTitle}>Contents</p>
      <ol className={styles.tocList}>
        {entries.map((e) => {
          const rel = e.level - min;
          const cls = rel === 0 ? styles.tocItem : rel === 1 ? styles.tocLevel3 : styles.tocLevel4;
          return (
            <li key={e.id} className={cls}>
              <button type="button" className={styles.tocButton} onClick={() => onSelect(e.id)}>
                {cleanName(e.text, 120)}
              </button>
            </li>
          );
        })}
      </ol>
    </nav>
  );
}
