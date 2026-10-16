// SPDX-License-Identifier: AGPL-3.0-or-later
import { isEngineError } from "@lessdugin/engine/types";
import ui from "../styles/ui.module.css";

export function Loading({ label = "Loading…" }: { label?: string | undefined }) {
  return (
    <p className={ui.muted} role="status">
      {label}
    </p>
  );
}

export function errorMessage(error: unknown): string {
  if (isEngineError(error)) return error.message;
  if (error instanceof Error) return error.message;
  return String(error);
}

/** An error box. With `page`, it stands for the whole page: its title is the page's <h1>, so
 * route focus and the document outline still find a heading. */
export function ErrorView({
  error,
  title = "Something went wrong",
  page = false,
}: {
  error: unknown;
  title?: string | undefined;
  page?: boolean | undefined;
}) {
  const code = isEngineError(error) ? error.code : null;
  return (
    <div className={ui.errorView} role="alert">
      {page ? <h1 className={ui.errorTitle}>{title}</h1> : <strong>{title}</strong>}
      <p>{errorMessage(error)}</p>
      {code && <p className={ui.mono}>code: {code}</p>}
    </div>
  );
}
