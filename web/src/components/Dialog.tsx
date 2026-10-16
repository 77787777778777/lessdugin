// SPDX-License-Identifier: AGPL-3.0-or-later
// A modal <dialog>. `onClose` reports a close the user asked for (Escape, or a form inside the
// dialog); closing it because `open` became false does not call it. The native `close` event
// fires in a later task, by when `onClose` may already belong to the next request shown in the
// same dialog (PublicAck), so a programmatic close must not reach it.
import { type ReactNode, useEffect, useId, useRef } from "react";
import styles from "./Dialog.module.css";

export function Dialog({
  open,
  onClose,
  title,
  children,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  children: ReactNode;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  const closingItself = useRef(false);
  useEffect(() => {
    const d = ref.current;
    if (!d) return;
    if (open && !d.open) {
      if (typeof d.showModal === "function") d.showModal();
      else d.setAttribute("open", "");
    } else if (!open && d.open) {
      if (typeof d.close === "function") {
        closingItself.current = true;
        d.close();
      } else d.removeAttribute("open");
    }
  }, [open]);
  return (
    <dialog
      ref={ref}
      className={styles.dialog}
      aria-labelledby={titleId}
      onClose={() => {
        if (closingItself.current) {
          closingItself.current = false;
          return;
        }
        onClose();
      }}
    >
      {open && (
        <>
          <h2 id={titleId} className={styles.title}>
            {title}
          </h2>
          {children}
        </>
      )}
    </dialog>
  );
}
