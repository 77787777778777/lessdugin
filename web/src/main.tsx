// SPDX-License-Identifier: AGPL-3.0-or-later
import "./styles/global.css";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App.tsx";
import { chooseEngine, loadEngine } from "./engine/select.ts";
import { lessduginPolicy } from "./lib/sanitize.ts";

function start(): void {
  const rootEl = document.getElementById("root");
  if (!rootEl) throw new Error("missing #root");
  // The Trusted Types policy is created once, at startup (threat-model SPA-2).
  lessduginPolicy();
  void navigator.storage?.persist?.().catch(() => false);
  const choice = chooseEngine(window.location, import.meta.env);
  loadEngine(choice).then(
    (engine) => {
      createRoot(rootEl).render(
        <StrictMode>
          <App engine={engine} />
        </StrictMode>,
      );
    },
    (error: unknown) => {
      const p = document.createElement("p");
      p.textContent = `The forum engine failed to start: ${String(error)}`;
      rootEl.replaceChildren(p);
    },
  );
}

// Frame guard (threat-model SPA-10): never render inside a frame.
if (window.top !== window.self) {
  document.documentElement.replaceChildren();
} else {
  start();
}
