// SPDX-License-Identifier: AGPL-3.0-or-later
// The main-thread client over Comlink, against a stand-in worker on a MessageChannel: lazy
// start, method forwarding, events, and the AuthBridge port the worker calls back into.
import { resolveObjectURL } from "node:buffer";
import { readFileSync } from "node:fs";
import * as Comlink from "comlink";
import { afterEach, describe, expect, it } from "vitest";
import { NS } from "./auth-bridge.ts";
import { connectBridgePort } from "./bridge-port.ts";
import { type EngineClient, OP_ARITY, padArgs, startEngine } from "./client.ts";
import type { AuthBridgePort, WorkerInit } from "./protocol.ts";

interface FakeWorker {
  inits: WorkerInit[];
  calls: [string, unknown[]][];
  bridge: AuthBridgePort | null;
  emit: ((json: string) => void) | null;
  /** The raw port the worker got (to send what a compromised worker could). */
  port: MessagePort | null;
  closed: boolean;
}

function fakeWorker(opts: { failInits?: number } = {}): {
  worker: () => Worker;
  state: FakeWorker;
} {
  let failInits = opts.failInits ?? 0;
  const state: FakeWorker = {
    inits: [],
    calls: [],
    bridge: null,
    emit: null,
    port: null,
    closed: false,
  };
  const api = {
    version: async () => "0.0.1",
    async init(init: WorkerInit, port: MessagePort) {
      state.inits.push(init);
      if (failInits > 0) {
        failInits--;
        throw { code: "internal", message: "storage: IndexedDB is blocked", retryable: true };
      }
      const link = connectBridgePort(port);
      state.port = port;
      state.bridge = link.bridge;
      state.emit = (json) => link.emit(json);
      return { version: "0.0.1", namespace: init.namespace, id: "w" };
    },
    async call(op: string, args: unknown[]) {
      state.calls.push([op, args]);
      if (op === "grant") return [{ writeId: "w1" }, { writeId: "w2" }];
      if (op === "fail") throw { code: "network", message: "down", retryable: true };
      if (op === "loadMedia") {
        const cid = args[0] as string;
        return {
          cid,
          size: 3,
          url: `idb:${cid}`,
          ...(cid === "bafkreimp4" ? {} : { mime: "image/png" }),
        };
      }
      return { op };
    },
    async blob(cid: string) {
      return cid === "bafkreigone" ? null : new Blob([Uint8Array.of(1, 2, 3)]);
    },
    async callJson(op: string, argsJson: string) {
      return JSON.stringify({ op, args: JSON.parse(argsJson) });
    },
    async prepareMedia(bytes: Uint8Array, input: unknown) {
      state.calls.push(["prepareMedia", [bytes, input]]);
      return { cid: "bafkreix" };
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
    await c.forumDiscover("rust");
    expect(state.calls.at(-1)).toEqual(["forumDiscover", ["rust"]]);
    await c.backlinks("at://x");
    expect(state.calls.at(-1)).toEqual(["backlinks", ["at://x"]]);
    expect(await c.grant("at://f", ["did:plc:a", "did:plc:b"])).toEqual({
      writeId: "w1",
    });
    await c.outboxPump();
    await c.liveTail();
    expect(state.calls.map(([op]) => op)).toEqual([
      "getPost",
      "forumDiscover",
      "backlinks",
      "grant",
    ]);
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

  it("sends prepareMedia's bytes as bytes and refuses a file over the wasm window (m4-18)", async () => {
    const { worker, state } = fakeWorker();
    const c = start({ worker });
    expect(await c.prepareMedia(Uint8Array.of(1, 2, 3), { alt: "a" })).toEqual({
      cid: "bafkreix",
    });
    const [op, [bytes, input]] = state.calls.at(-1) as [string, [unknown, unknown]];
    expect(op).toBe("prepareMedia");
    // a Uint8Array, not a JSON number array of one element per byte
    expect(bytes).toBeInstanceOf(Uint8Array);
    expect(Array.from(bytes as Uint8Array)).toEqual([1, 2, 3]);
    expect(input).toEqual({ alt: "a" });
    // limits.md §6: refused on the main thread, before the worker sees a byte
    const calls = state.calls.length;
    await expect(c.prepareMedia(new Uint8Array(64 * 1024 * 1024 + 1))).rejects.toMatchObject({
      code: "too_large",
      detail: { kind: "mediaRejected", limit: 64 * 1024 * 1024 },
    });
    expect(state.calls.length).toBe(calls);
  });

  it("turns loadMedia's verified blob into a typed blob: URL on the main thread (m4-1)", async () => {
    const { worker, state } = fakeWorker();
    const c = start({ worker });
    const view = { index: 0, cid: "bafkreipng", mime: "image/png", author: "did:plc:bob" };
    const loaded = await c.loadMedia("bafkreipng", view as never);
    // the item's view (or the author's DID) goes to the engine, which asks the author's PDS
    expect(state.calls.at(-1)).toEqual(["loadMedia", ["bafkreipng", view]]);
    expect(loaded).toMatchObject({ cid: "bafkreipng", size: 3, mime: "image/png" });
    // not the host's `idb:` locator, which no element can load (CSP: img-src 'self' blob:)
    expect(loaded.url).toMatch(/^blob:/);
    const made = resolveObjectURL(loaded.url);
    expect(made?.type).toBe("image/png");
    expect(Array.from(new Uint8Array((await made?.arrayBuffer()) ?? new ArrayBuffer(0)))).toEqual([
      1, 2, 3,
    ]);
    URL.revokeObjectURL(loaded.url);
    // no display type: the bytes are typed as nothing a page would render inline
    const card = await c.loadMedia("bafkreimp4", "did:plc:bob");
    expect(resolveObjectURL(card.url)?.type).toBe("application/octet-stream");
    URL.revokeObjectURL(card.url);
    await expect(c.loadMedia("bafkreigone")).rejects.toMatchObject({ code: "internal" });
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

  it("exposes no main-thread object to the worker (security review #1)", async () => {
    const { worker, state } = fakeWorker();
    const sent: string[] = [];
    const c = start({
      worker,
      sessions: {
        session: async (did) => ({
          did,
          fetch: async (path) => {
            sent.push(path);
            return Response.json({});
          },
        }),
      },
    });
    await c.ready;
    const port = state.port as MessagePort;
    // what Comlink would send to a proxied AuthBridge: SET namespace, SET along __proto__, APPLY
    port.postMessage({
      id: "x1",
      type: "SET",
      path: ["namespace"],
      value: { type: "RAW", value: "app.bsky.actor" },
    });
    port.postMessage({
      id: "x2",
      type: "SET",
      path: ["__proto__", "__proto__", "pollutedFromWorker"],
      value: { type: "RAW", value: "yes" },
    });
    port.postMessage({ id: "x3", type: "APPLY", path: ["request"], argumentList: [] });
    port.postMessage({ t: "req", id: 1, req: { __proto__: { account: "did:plc:a" } } });
    port.postMessage({ t: "event", json: { not: "a string" } });
    await tick();
    expect(({} as Record<string, unknown>).pollutedFromWorker).toBeUndefined();
    const bridge = state.bridge as AuthBridgePort;
    await expect(
      bridge.request({
        account: "did:plc:a",
        method: "POST",
        path: "/xrpc/com.atproto.repo.putRecord",
        headers: [["content-type", "application/json"]],
        body: new TextEncoder().encode(
          JSON.stringify({ repo: "did:plc:a", collection: "app.bsky.actor.profile", rkey: "self" }),
        ),
        maxBytes: 1024,
        timeoutMs: 1000,
      }),
    ).rejects.toMatchObject({ kind: "blocked" });
    expect(sent).toEqual([]);
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

  it("tries the start again after a failed one (robustness review L6)", async () => {
    const { worker, state } = fakeWorker({ failInits: 1 });
    const c = start({ worker });
    await expect(c.accounts()).rejects.toMatchObject({
      message: expect.stringContaining("blocked"),
    });
    expect(await c.accounts()).toEqual({ op: "accounts" });
    expect(state.inits).toHaveLength(2);
    // the second start got a working bridge port
    const r = await (state.bridge as AuthBridgePort)
      .request({
        account: "did:plc:a",
        method: "GET",
        path: "/xrpc/com.atproto.server.getSession",
        headers: [],
        maxBytes: 1024,
        timeoutMs: 1000,
      })
      .catch((e: unknown) => e);
    expect(r).toMatchObject({ kind: "authRequired" });
  });

  it("closes the worker's engine on terminate", async () => {
    const { worker, state } = fakeWorker();
    const c = startEngine({ worker });
    await c.ready;
    await c.terminate();
    expect(state.closed).toBe(true);
  });
});

describe("OP_ARITY", () => {
  /** `name(params)` of every `EngineApi` method, signatures joined across lines. */
  function signatures(): [string, string][] {
    const src = readFileSync(new URL("./api.ts", import.meta.url), "utf8");
    const start = src.indexOf("export interface EngineApi {");
    const body = src.slice(start, src.indexOf("\n}\n", start));
    const flat = body.replace(/\/\*\*[\s\S]*?\*\//g, "").replace(/\/\/.*$/gm, "");
    return [...flat.matchAll(/\n\s+(\w+)(?:<[^>]*>)?\(([^)]*)\):/g)].map(
      (m) => [m[1] ?? "", (m[2] ?? "").trim()] as [string, string],
    );
  }

  it("pads every operation with optional trailing parameters to its full arity", () => {
    const sigs = signatures();
    expect(sigs.length).toBeGreaterThan(90);
    const optional = sigs
      .filter(([, params]) => /\?:/.test(params))
      .map(([name, params]) => [name, params.split(",").filter((p) => p.trim()).length]);
    for (const [name, arity] of optional) {
      expect(OP_ARITY[name as string], `${name}`).toBe(arity);
    }
    for (const [name, n] of Object.entries(OP_ARITY)) {
      const sig = sigs.find(([m]) => m === name);
      expect(sig, name).toBeDefined();
      expect(sig?.[1].split(",").filter((p) => p.trim()).length, name).toBe(n);
    }
    expect(padArgs("modLog", ["at://f"])).toEqual(["at://f", null, null]);
    expect(padArgs("opinion", ["at://p", { kind: "karma" }])).toEqual([
      "at://p",
      { kind: "karma" },
    ]);
  });
});
