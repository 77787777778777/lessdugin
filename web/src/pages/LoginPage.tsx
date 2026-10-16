// SPDX-License-Identifier: AGPL-3.0-or-later
// Sign-in: the engine resolves the handle (DoH, both directions), then the host-specific login
// runs (OAuth in the hosted app; a placeholder with the mock engine) and calls `addAccount`.

import type { ActorView } from "@lessdugin/engine/types";
import { useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useId, useState } from "react";
import { ActorName } from "../components/ActorName.tsx";
import { errorMessage } from "../components/Status.tsx";
import { toasts } from "../components/Toast.tsx";
import { useEngineHandle } from "../engine/context.tsx";
import ui from "../styles/ui.module.css";

export function LoginPage() {
  const handle = useEngineHandle();
  const qc = useQueryClient();
  const navigate = useNavigate();
  const inputId = useId();
  const [input, setInput] = useState("");
  const [actor, setActor] = useState<ActorView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const resolve = async (): Promise<void> => {
    setBusy(true);
    setError(null);
    setActor(null);
    try {
      setActor(await handle.api.resolveActor(input.trim()));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const signIn = async (a: ActorView): Promise<void> => {
    setBusy(true);
    setError(null);
    try {
      await handle.login.login(a);
      await qc.invalidateQueries();
      toasts.show({ message: "Signed in", tone: "success" });
      void navigate({ to: "/" });
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className={ui.page}>
      <h1 className={ui.pageTitle}>Sign in</h1>
      <p>{handle.login.description}</p>
      <form
        className={ui.stack}
        onSubmit={(e) => {
          e.preventDefault();
          void resolve();
        }}
      >
        <div className={ui.field}>
          <label className={ui.label} htmlFor={inputId}>
            Your handle or DID
          </label>
          <input
            id={inputId}
            className={ui.input}
            autoComplete="username"
            spellCheck={false}
            placeholder="alice.example.com"
            value={input}
            onChange={(e) => setInput(e.target.value)}
          />
          <span className={ui.hint}>
            You will never be asked for your main password. App passwords are accepted only by the
            command-line client.
          </span>
        </div>
        <div className={ui.row}>
          <button type="submit" className={ui.primary} disabled={busy || input.trim() === ""}>
            Continue
          </button>
        </div>
      </form>
      {error && (
        <p className={ui.error} role="alert">
          {error}
        </p>
      )}
      {actor && (
        <div className={ui.card}>
          <p>
            Sign in as <ActorName actor={actor} showIdent link={false} />?
          </p>
          <p className={ui.mono}>{actor.did}</p>
          <button
            type="button"
            className={ui.primary}
            disabled={busy}
            onClick={() => void signIn(actor)}
          >
            Sign in
          </button>
        </div>
      )}
      <p className={ui.small}>
        The hosted app is trusted while you use it. For a zero-trust setup run{" "}
        <code>lessdugin web</code>, which keeps your login in the command-line client.
      </p>
    </div>
  );
}
