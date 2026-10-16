// SPDX-License-Identifier: AGPL-3.0-or-later
// The composer, loaded on demand: CodeMirror is most of the bundle, and reading needs none of it.

import { type ComponentProps, lazy, Suspense } from "react";
import styles from "./Composer.module.css";

const Composer = lazy(() => import("./Composer.tsx").then((m) => ({ default: m.Composer })));

export function LazyComposer(props: ComponentProps<typeof Composer>) {
  return (
    <Suspense
      fallback={
        <div className={styles.composer} role="status">
          <p className={styles.loading}>Loading the editor…</p>
        </div>
      }
    >
      <Composer {...props} />
    </Suspense>
  );
}
