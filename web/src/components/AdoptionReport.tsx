// SPDX-License-Identifier: AGPL-3.0-or-later
// The report of a done adoption (`mod adopt`, and the one a moderator's removal runs first):
// what it did not adopt lapses with the former moderator unless the adopter's own assertion
// stands, so it is listed as soon as the write is done (engine-api.md §3.5 `mod_adopt`,
// data-model.md §8.6; review finding sec-8), as the TUI's adoption panel does. The write store
// keeps each report until it is closed, so one that arrives while another is open follows it.

import { useSyncExternalStore } from "react";
import { type WriteStore, writeStore } from "../engine/writes.ts";
import { plural } from "../lib/text.ts";
import ui from "../styles/ui.module.css";
import { Dialog } from "./Dialog.tsx";

export function AdoptionReport({ store = writeStore }: { store?: WriteStore | undefined }) {
  useSyncExternalStore(store.subscribe, store.snapshot, store.snapshot);
  const report = store.pendingReport();
  const close = (): void => {
    if (report) store.dismissReport(report.id);
  };
  return (
    <Dialog open={report !== undefined} onClose={close} title="Adoption report">
      {report && (
        <>
          <p>
            Adoption sent (write {report.id}); {plural(report.lines.length, "assertion")} not
            adopted, listed below. What was not adopted lapses with the former moderator, unless
            your own assertion for it stands.
          </p>
          <ul className={ui.list} data-testid="adoption-report">
            {report.lines.map((line, i) => (
              <li key={`${i}:${line}`}>{line}</li>
            ))}
          </ul>
          <div className={ui.row}>
            <button type="button" className={ui.button} onClick={close}>
              Close
            </button>
          </div>
        </>
      )}
    </Dialog>
  );
}
