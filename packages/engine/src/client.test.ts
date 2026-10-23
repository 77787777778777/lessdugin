// SPDX-License-Identifier: AGPL-3.0-or-later
// The main-thread client over Comlink, against a stand-in worker on a MessageChannel: lazy
// start, method forwarding, events, and the AuthBridge proxy the worker calls back into.
import * as Comlink from "comlink";
import { afterEach, describe, expect, it } from "vitest";
import { NS } from "./auth-bridge.ts";
import { type EngineClient, startEngine } from "./client.ts";
import type { AuthBridgePort, WorkerInit } from "./protocol.ts";

interface FakeWorker {
  inits: WorkerInit[];
  calls: [string, unknown[]][];
  bridge: AuthBridgePort | null;
  emit: ((json: string) => void) | null;
  closed: boolean;
}

function fakeWorker(): { worker: () => Worker; state: FakeWorker } {
  const state: FakeWorker = { inits: [], calls: [], bridge: null, emit: null, closed: false };
  const api = {
    version: async () => "0.0.1",
    async init(init: WorkerInit, bridge: AuthBridgePort, onEvent: (json: string) => void) {
      state.inits.push(init);
      state.bridge = bridge;
      state.emit = onEvent;
      return { version: "0.0.1", namespace: init.namespace, id: "w" };
    },
    async call(op: string, args: unknown[]) {
      state.calls.push([op, args]);
      if (op === "grant") return [{ writeId: "w1" }, { writeId: "w2" }];
      if (op === "fail") throw { code: "network", message: "down", retryable: true };
      return { op };
    },
    async callJson(op: string, argsJson: string) {
      return JSON.stringify({ op, args: JSON.parse(argsJson) });
    },
    isLeader: () => true,
    async close() {
      state.closed = true;
    },
  };
  const { port1, port2 } = new MessageChannel();
  Comlink.expose(api, port2);
  const worker = () =>
    Object.assign(port1, { terminate: () => port1.close() }) as unknown as Worker;
  return { worker, state };
}

const clients: EngineClient[] = [];
afterEach(async () => {
  for (const c of clients.splice(0)) await c.terminate();
});

function start(opts: Parameters<typeof startEngine>[0] = {}) {
  const c = startEngine(opts);
  clients.push(c);
  return c;
}

const tick = () => new Promise<void>((r) => setTimeout(r, 10));

describe("startEngine", () => {
  it("starts the worker lazily with this build's namespace", async () => {
    const { worker, state } = fakeWorker();
    const c = start({ worker, init: { config: { verify: "on" } as never, elect: false } });
    expect(await c.version()).toBe("0.0.1");
    expect(state.inits).toEqual([]);
    expect(await c.ready).toEqual({ version: "0.0.1", namespace: NS, id: "w" });
    expect(state.inits).toEqual([{ config: { verify: "on" }, elect: false, namespace: NS }]);
    await c.accounts();
    expect(state.inits).toHaveLength(1);
  });

  it("forwards methods, special-cases grant and the leader-only loops", async () => {
    const { worker, state } = fakeWorker();
    const c = start({ worker });
    expect(await c.getPost("at://x")).toEqual({ op: "getPost" });
    expect(state.calls.at(-1)).toEqual(["getPost", ["at://x"]]);
    expect(await c.grant("at://f", ["did:plc:a", "did:plc:b"])).toEqual({
      writeId: "w1",
    });
    await c.outboxPump();
    await c.liveTail();
    expect(state.calls.map(([op]) => op)).toEqual(["getPost", "grant"]);
    expect(await c.callJson("x", [1, null])).toBe('{"op":"x","args":[1,null]}');
    expect(await c.isLeader()).toBe(true);
    await expect((c as unknown as { fail(): Promise<void> }).fail()).rejects.toMatchObject({
      code: "network",
    });
  });

  it("sends omitted optional trailing arguments as null (the wasm side reads fixed tuples)", async () => {
    const { worker, state } = fakeWorker();
    const c = start({ worker });
    await c.join("at://f");
    await c.forumFollow("at://f");
    await c.getThread("at://p", { sort: "new", unreadOnly: false });
    expect(state.calls).toEqual([
      ["join", ["at://f", null]],
      ["forumFollow", ["at://f", null]],
      ["getThread", ["at://p", { sort: "new", unreadOnly: false }]],
    ]);
    expect(await c.callJson("syncThread", ["at://p"])).toBe(
      '{"op":"syncThread","args":["at://p",null]}',
    );
  });

  it("is not a thenable", async () => {
    const { worker } = fakeWorker();
    const c = start({ worker });
    expect((c as unknown as { then?: unknown }).then).toBeUndefined();
    // awaiting the client itself must not call the worker
    expect(await Promise.resolve(c)).toBe(c);
  });

  it("delivers events in order to every listener until unsubscribed", async () => {
    const { worker, state } = fakeWorker();
    const c = start({ worker });
    const a: unknown[] = [];
    const b: unknown[] = [];
    const offA = c.subscribe((e) => a.push(e));
    c.subscribe((e) => {
      b.push(e);
      throw new Error("a throwing listener does not stop the others");
    });
    await c.ready;
    const ev1 = { kind: "changed", topics: [{ kind: "outbox" }] };
    const ev2 = { kind: "write", id: "w", state: { kind: "pending" } };
    state.emit?.(JSON.stringify(ev1));
    state.emit?.("not json");
    await tick();
    offA();
    state.emit?.(JSON.stringify(ev2));
    await tick();
    expect(a).toEqual([ev1]);
    expect(b).toEqual([ev1, ev2]);
  });

  it("hands the worker an AuthBridge over the SPA's sessions", async () => {
    const { worker, state } = fakeWorker();
    const sent: string[] = [];
    const c = start({
      worker,
      sessions: {
        session: async (did) => ({
          did,
          fetch: async (path) => {
            sent.push(path);
            return Response.json({ did });
          },
        }),
      },
    });
    await c.ready;
    const bridge = state.bridge as AuthBridgePort;
    const r = await bridge.request({
      account: "did:plc:a",
      method: "GET",
      path: "/xrpc/com.atproto.server.getSession",
      headers: [],
      maxBytes: 1024,
      timeoutMs: 1000,
    });
    expect(r.status).toBe(200);
    await expect(
      bridge.request({
        account: "did:plc:a",
        method: "GET",
        path: "/xrpc/com.atproto.repo.listRecords?repo=did:plc:a",
        headers: [],
        maxBytes: 1024,
        timeoutMs: 1000,
      }),
    ).rejects.toMatchObject({ kind: "blocked" });
    expect(sent).toEqual(["/xrpc/com.atproto.server.getSession"]);
  });

  it("closes the worker's engine on terminate", async () => {
    const { worker, state } = fakeWorker();
    const c = startEngine({ worker });
    await c.ready;
    await c.terminate();
    expect(state.closed).toBe(true);
  });
});
