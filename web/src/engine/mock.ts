// SPDX-License-Identifier: AGPL-3.0-or-later
import { MockEngine } from "@lessdugin/engine/mock";
import type { EngineHandle } from "./handle.ts";

export function createMockHandle(engine: MockEngine = new MockEngine()): EngineHandle {
  return {
    kind: "mock",
    api: engine,
    login: {
      description:
        "Mock sign-in: any well-formed handle resolves to a demo identity, and no password or OAuth is involved.",
      async login(actor) {
        await engine.addAccount({
          did: actor.did,
          pds: "https://pds.example.test",
          auth: "oauth",
        });
        await engine.setActiveAccount(actor.did);
      },
    },
    async version() {
      return "mock (in-memory fixture)";
    },
  };
}
