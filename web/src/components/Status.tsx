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

export function ErrorView({
  error,
  title = "Something went wrong",
}: {
  error: unknown;
  title?: string | undefined;
}) {
  const code = isEngineError(error) ? error.code : null;
  return (
    <div className={ui.error} role="alert">
      <strong>{title}</strong>
      <p>{errorMessage(error)}</p>
      {code && <p className={ui.mono}>code: {code}</p>}
    </div>
  );
}
