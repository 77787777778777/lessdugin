// SPDX-License-Identifier: AGPL-3.0-or-later
import { startEngine } from "@lessdugin/engine/client";
import { useEffect, useState } from "react";

const engine = startEngine();

export function App() {
  const [version, setVersion] = useState<string | null>(null);
  useEffect(() => {
    engine.version().then(setVersion, (e: unknown) => setVersion(`error: ${String(e)}`));
  }, []);
  return (
    <main>
      <h1>lessdugin</h1>
      <p>A serverless markdown and media forum on atproto.</p>
      <p data-testid="engine-version">engine: {version ?? "loading…"}</p>
    </main>
  );
}
