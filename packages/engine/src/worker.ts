// SPDX-License-Identifier: AGPL-3.0-or-later
// The engine worker: loads the Rust engine (wasm) and exposes it over Comlink.
// Later milestones add the JsHost (fetch, IndexedDB, WebSocket, DoH) here.

import * as Comlink from "comlink";
import init, { engineVersion } from "../pkg/lessdugin_engine.js";

const ready = init();

const api = {
  async version(): Promise<string> {
    await ready;
    return engineVersion();
  },
};

export type EngineWorkerApi = typeof api;

Comlink.expose(api);
