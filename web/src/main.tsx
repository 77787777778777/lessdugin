// SPDX-License-Identifier: AGPL-3.0-or-later
import "./styles/global.css";
import { workerUrlPolicy } from "@lessdugin/engine/worker-url";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App.tsx";
import { chooseEngine, loadEngine } from "./engine/select.ts";
import { takeBootParams } from "./lib/bootParams.ts";
import { lessduginPolicy } from "./lib/sanitize.ts";
import { setStorageNamespace } from "./lib/storage.ts";

function start(): void {
  const rootEl = document.getElementById("root");
  if (!rootEl) throw new Error("missing #root");
  // The Trusted Types policies are created once, at startup (threat-model SPA-2): `lessdugin-md`
  // here, `lessdugin-worker` for the engine worker, and `dompurify` by DOMPurify itself.
  lessduginPolicy();
  workerUrlPolicy();
  // OAuth callback and bridge capability leave the address bar before the router mounts (SPA-8).
  takeBootParams();
  void navigator.storage?.persist?.().catch(() => false);
  const choice = chooseEngine(window.location, import.meta.env);
  setStorageNamespace(choice);
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
      p.setAttribute("role", "alert");
      // A `lessdugin web` tab without a bridge session gets the §7.2 text as it is.
      p.textContent =
        error instanceof Error && error.name === "BridgeLinkError"
          ? error.message
          : `The forum engine failed to start: ${error instanceof Error ? error.message : String(error)}`;
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
