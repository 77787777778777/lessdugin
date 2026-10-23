// SPDX-License-Identifier: AGPL-3.0-or-later
// The engine worker's own logic against a stand-in wasm module (no build needed): the online
// wake of the leader's pump, the leader's re-entrant `sync-leader` host lock, and a failed
// start that leaves nothing open.
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { afterEach, describe, expect, it } from "vitest";
import { NS } from "./auth-bridge.ts";
import type { JsHostImports } from "./host.ts";
import { LocalLocks, webLockName } from "./locks.ts";
import { type AuthBridgePort, LEADER_LOCK } from "./protocol.ts";
import { type WasmModule, WorkerEngine, type WorkerEnv } from "./worker-core.ts";

const tick = (ms = 0) => new Promise<void>((r) => setTimeout(r, ms));

interface Fake {
  module: WasmModule;
  calls: string[];
  host: () => JsHostImports;
}

function fakeWasm(opts: { failConstruct?: boolean } = {}): Fake {
  const calls: string[] = [];
  let host: JsHostImports | null = null;
  class WasmEngine {
    constructor(_config: string, h: JsHostImports) {
      if (opts.failConstruct) throw '{"code":"invalid_input","message":"bad","retryable":false}';
      host = h;
    }
    async call(op: string): Promise<string> {
      calls.push(op);
      if (op === "accounts" || op === "writes") return "[]";
      return "null";
    }
    subscribe(): number {
      return 1;
    }
    unsubscribe(): boolean {
      return true;
    }
  }
  return {
    module: {
      WasmEngine,
      engineVersion: () => "0.0.0-test",
      engineNamespace: () => NS,
    },
    calls,
    host: () => {
      if (!host) throw new Error("no host");
      return host;
    },
  };
}

const NO_BRIDGE: AuthBridgePort = {
  request: async () => {
    throw { kind: "authRequired", message: "none" };
  },
};

const open: WorkerEngine[] = [];
afterEach(async () => {
  for (const e of open.splice(0)) await e.close();
});

function env(over: Partial<WorkerEnv> = {}): WorkerEnv {
  return {
    fetch: (async () => {
      throw new TypeError("offline");
    }) as typeof fetch,
    locks: new LocalLocks(),
    idb: null,
    warn: () => {},
    ...over,
  };
}

describe("WorkerEngine (stand-in wasm)", () => {
  it("wakes the leader's outbox pump when the device comes back online", async () => {
    const online: (() => void)[] = [];
    const onOnline = (cb: () => void) => {
      online.push(cb);
      return () => online.splice(online.indexOf(cb), 1);
    };
    const locks = new LocalLocks();
    const leaderWasm = fakeWasm();
    const leader = new WorkerEngine(leaderWasm.module, env({ locks, onOnline }));
    open.push(leader);
    await leader.init({ config: {}, namespace: NS }, NO_BRIDGE, () => {});
    const followerWasm = fakeWasm();
    const follower = new WorkerEngine(followerWasm.module, env({ locks, onOnline }));
    open.push(follower);
    await follower.init({ config: {}, namespace: NS }, NO_BRIDGE, () => {});
    await tick(10);
    expect(leader.isLeader).toBe(true);
    expect(follower.isLeader).toBe(false);
    for (const cb of [...online]) cb();
    await tick(10);
    expect(leaderWasm.calls.filter((c) => c === "outboxWake")).toHaveLength(1);
    expect(followerWasm.calls).not.toContain("outboxWake");
    await leader.close();
    // closing unsubscribes
    expect(online).toHaveLength(1);
  });

  it("gives the leader's engine the sync-leader host lock without waiting for itself", async () => {
    expect(webLockName("sync-leader")).toBe(LEADER_LOCK);
    const locks = new LocalLocks();
    const w = fakeWasm();
    const leader = new WorkerEngine(w.module, env({ locks }));
    open.push(leader);
    await leader.init({ config: {}, namespace: NS }, NO_BRIDGE, () => {});
    await tick(10);
    expect(leader.isLeader).toBe(true);
    const h = await Promise.race([
      w.host().lock("sync-leader", true),
      tick(200).then(() => "deadlock" as const),
    ]);
    expect(h).not.toBe("deadlock");
    expect(h).not.toBeNull();
    (h as { release(): void }).release();
    // a non-leader does not get it while the leader holds it
    const w2 = fakeWasm();
    const other = new WorkerEngine(w2.module, env({ locks }));
    open.push(other);
    await other.init({ config: {}, namespace: NS }, NO_BRIDGE, () => {});
    expect(await w2.host().lock("sync-leader", false)).toBeNull();
  });

  it("closes what it opened when the start fails, and can start again", async () => {
    const factory = new IDBFactory();
    const idb = { factory, keyRange: IDBKeyRange };
    const bad = fakeWasm({ failConstruct: true });
    const e = new WorkerEngine(bad.module, env({ idb }));
    await expect(e.init({ config: {}, namespace: NS }, NO_BRIDGE, () => {})).rejects.toMatchObject({
      code: "invalid_input",
    });
    // no connection was left open: deleting the stores is not blocked
    for (const name of ["lessdugin-cache", "lessdugin-state", "lessdugin-blobs"]) {
      await new Promise<void>((resolve, reject) => {
        const r = factory.deleteDatabase(name);
        r.onsuccess = () => resolve();
        r.onerror = () => reject(r.error);
        r.onblocked = () => reject(new Error(`${name} is still open`));
      });
    }
    const good = new WorkerEngine(fakeWasm().module, env({ idb }));
    open.push(good);
    await expect(
      good.init({ config: {}, namespace: NS }, NO_BRIDGE, () => {}),
    ).resolves.toMatchObject({ namespace: NS });
  });
});
