// SPDX-License-Identifier: AGPL-3.0-or-later
// The engine worker against the real release wasm (pkg/, `cargo xtask wasm`) in node: the JS host
// (IndexedDB over fake-indexeddb, fetch, locks, timers), the dto/1 boundary, events, the leader
// election between two "tabs" and the change relay. Skipped locally without pkg/; required
// under CI (or with LESSDUGIN_WASM_TEST=1).
import { existsSync, readFileSync } from "node:fs";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { afterEach, describe, expect, it } from "vitest";
import { NS } from "./auth-bridge.ts";
import type { ChannelLike } from "./leader.ts";
import { LocalLocks } from "./locks.ts";
import type { AuthBridgePort, PdsRequest, WorkerInit } from "./protocol.ts";
import type { EngineError } from "./types.ts";
import { type WasmModule, WorkerEngine, type WorkerEnv } from "./worker-core.ts";

const PKG = new URL("../pkg/", import.meta.url);
const WASM = new URL("lessdugin_engine_bg.wasm", PKG);
const present = existsSync(WASM);
const required = process.env.CI !== undefined || process.env.LESSDUGIN_WASM_TEST === "1";

let loaded: Promise<WasmModule> | null = null;
function load(): Promise<WasmModule> {
  loaded ??= (async () => {
    const glue = new URL("lessdugin_engine.js", PKG).href;
    const m = (await import(/* @vite-ignore */ glue)) as WasmModule & {
      initSync(input: { module: BufferSource }): unknown;
    };
    m.initSync({ module: readFileSync(WASM) });
    return m;
  })();
  return loaded;
}

const tick = (ms = 0) => new Promise<void>((r) => setTimeout(r, ms));

/** An in-memory BroadcastChannel hub. */
function hub(): () => ChannelLike {
  const ends = new Set<(data: unknown) => void>();
  return () => {
    const listeners: ((ev: MessageEvent) => void)[] = [];
    const deliver = (data: unknown) => {
      for (const l of listeners) l({ data } as MessageEvent);
    };
    ends.add(deliver);
    return {
      postMessage(m: unknown) {
        const copy = structuredClone(m);
        for (const d of ends) if (d !== deliver) d(copy);
      },
      addEventListener: (_t: "message", l: (ev: MessageEvent) => void) => {
        listeners.push(l);
      },
      close: () => {
        ends.delete(deliver);
      },
    };
  };
}

interface Tab {
  engine: WorkerEngine;
  events: unknown[];
  warnings: string[];
  fetched: string[];
  bridged: PdsRequest[];
}

const open: WorkerEngine[] = [];
afterEach(async () => {
  for (const e of open.splice(0)) await e.close();
});

/** A shared origin: one IndexedDB, one lock manager, one change channel. */
function origin() {
  return {
    idb: { factory: new IDBFactory(), keyRange: IDBKeyRange },
    locks: new LocalLocks(),
    channel: hub(),
  };
}

async function tab(
  o: ReturnType<typeof origin>,
  init: Partial<WorkerInit> = {},
  fetchImpl?: (url: string) => Response | Promise<Response>,
): Promise<Tab> {
  const wasm = await load();
  const t: Tab = {
    engine: undefined as unknown as WorkerEngine,
    events: [],
    warnings: [],
    fetched: [],
    bridged: [],
  };
  const env: WorkerEnv = {
    fetch: (async (input: RequestInfo | URL) => {
      const url = String(input);
      t.fetched.push(url);
      if (fetchImpl) return fetchImpl(url);
      throw new TypeError("offline");
    }) as typeof fetch,
    locks: o.locks,
    idb: o.idb,
    onLine: () => true,
    channel: o.channel,
    warn: (m) => t.warnings.push(m),
  };
  const bridge: AuthBridgePort = {
    async request(req) {
      t.bridged.push(req);
      throw { kind: "authRequired", message: "no session in this test" };
    },
  };
  t.engine = new WorkerEngine(wasm, env);
  open.push(t.engine);
  await t.engine.init({ config: {}, namespace: NS, ...init }, bridge, (json) =>
    t.events.push(JSON.parse(json)),
  );
  return t;
}

/** An 8×4 baseline JPEG (left column red, the rest green) without EXIF, as Pillow writes it. */
const JPEG_8X4 =
  "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDABALDA4MChAODQ4SERATGCgaGBYWGDEjJR0oOjM9PDkzODdASFxOQERXRTc4UG1RV19iZ2hnPk1xeXBkeFxlZ2P/2wBDARESEhgVGC8aGi9jQjhCY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2P/wAARCAAEAAgDASIAAhEBAxEB/8QAFQABAQAAAAAAAAAAAAAAAAAAAAP/xAAaEAEAAQUAAAAAAAAAAAAAAAAAAQMENHKx/8QAFAEBAAAAAAAAAAAAAAAAAAAABv/EABsRAAIBBQAAAAAAAAAAAAAAAAABAgQxM3Gx/9oADAMBAAIRAxEAPwC1ti0tI4AMyuwpU5p7fT//2Q==";

/** The CIDv1 (raw, sha2-256, base32) of `bytes`. */
async function rawCid(bytes: Uint8Array): Promise<string> {
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes as BufferSource));
  const raw = [0x01, 0x55, 0x12, 0x20, ...digest];
  const b32 = "abcdefghijklmnopqrstuvwxyz234567";
  let out = "b";
  let bits = 0;
  let value = 0;
  for (const byte of raw) {
    value = (value << 8) | byte;
    bits += 8;
    while (bits >= 5) {
      bits -= 5;
      out += b32[(value >> bits) & 31];
    }
    value &= (1 << bits) - 1;
  }
  if (bits > 0) out += b32[(value << (5 - bits)) & 31];
  return out;
}

async function rejects(p: Promise<unknown>): Promise<EngineError> {
  try {
    await p;
  } catch (e) {
    return e as EngineError;
  }
  throw new Error("did not reject");
}

describe.runIf(!present && required)("the engine wasm", () => {
  it("is built", () => {
    expect.fail(`${WASM.pathname} is missing: run \`cargo xtask wasm\` first`);
  });
});

describe.runIf(present)("WorkerEngine over the real wasm", () => {
  it("starts and answers local operations as dto/1 JSON", async () => {
    const { engine } = await tab(origin(), { elect: false });
    expect(engine.version()).toMatch(/^\d+\.\d+\.\d+/);
    expect(await engine.callJson("accounts", "[]")).toBe("[]");
    expect(await engine.callJson("activeAccount", "[]")).toBe("null");
    expect(await engine.callJson("writes", "[]")).toBe("[]");
    expect(await engine.call("parseRef", ["#/u/did:plc:ewvi7nxzyoun6zhxrhs64oiz", null])).toEqual({
      kind: "actor",
      did: "did:plc:ewvi7nxzyoun6zhxrhs64oiz",
    });
    const body = (await engine.call("render", ["Hello *world*", [], null])) as {
      html: string;
      wordCount: number;
    };
    expect(body.html).toContain("<em>world</em>");
    expect(body.wordCount).toBe(2);
    expect(await engine.call("safeHref", ["javascript:alert(1)", "markdown"])).toBeNull();
  });

  it("answers the M3 local operations and checks the social writes' arguments", async () => {
    const { engine } = await tab(origin(), { elect: false });
    // without an account: nothing to notify, nothing acknowledged
    expect(await engine.callJson("notifications", JSON.stringify([{ limit: 5 }]))).toBe(
      '{"dto":1,"items":[],"unread":0}',
    );
    expect(await engine.callJson("unreadCounts", "[]")).toBe(
      '{"dto":1,"notifications":0,"forums":[]}',
    );
    expect(await engine.call("acknowledgedPublicActions", [])).toEqual([]);
    expect(await engine.call("karmaNotifications", [])).toBe("daily");
    await engine.call("setKarmaNotifications", ["weekly"]);
    expect(await engine.call("karmaNotifications", [])).toBe("weekly");
    expect(await engine.call("mutes", [])).toEqual([]);
    expect(await engine.call("bookmarks", [])).toEqual([]);
    expect(await engine.call("subscriptions", [])).toEqual([]);
    expect(await engine.call("outboxExpedite", [])).toBe(0);
    const post = `at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/${NS}.post/3l2s5xxv2zaa2`;
    for (const [op, args] of [
      ["opinion", [post, { kind: "react", id: "insightful", stance: "for" }]],
      [
        "modAction",
        [{ kind: "remove", forum: post, scope: "forum", subject: post, actionKey: "hide" }],
      ],
      ["acknowledgePublicAction", ["vote"]],
    ] as const) {
      expect((await rejects(engine.call(op, [...args]))).code, op).toBe("auth_required");
    }
    expect((await rejects(engine.call("opinion", [post, { kind: "vote" }]))).code).toBe(
      "invalid_input",
    );
  });

  it("rejects with EngineError objects", async () => {
    const { engine } = await tab(origin(), { elect: false });
    const search = await rejects(engine.call("search", [{}]));
    expect(search.code).toBe("unsupported");
    expect(search.detail).toEqual({ milestone: "M5" });
    expect((await rejects(engine.call("noSuchOp", []))).code).toBe("unsupported");
    expect((await rejects(engine.callJson("accounts", "[1]"))).code).toBe("invalid_input");
    const join = await rejects(
      engine.call("join", ["at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/x.y.forum/f", null]),
    );
    expect(join.code).toBe("auth_required");
  });

  it("resolves an actor through the JS fetch (PLC directory, DNS over HTTPS)", async () => {
    const did = "did:plc:ewvi7nxzyoun6zhxrhs64oiz";
    const doc = {
      id: did,
      alsoKnownAs: ["at://alice.example.com"],
      verificationMethod: [
        {
          id: "#atproto",
          type: "Multikey",
          controller: did,
          publicKeyMultibase: "zQ3shXjHeiBuRCKmM36cuYnm7YEMzhGnCmCyW92sRJ9pribSF",
        },
      ],
      service: [
        {
          id: "#atproto_pds",
          type: "AtprotoPersonalDataServer",
          serviceEndpoint: "https://pds.example.com",
        },
      ],
    };
    const t = await tab(origin(), { elect: false }, (url) => {
      const u = new URL(url);
      if (u.host === "plc.directory" && u.pathname === `/${did}`) return Response.json(doc);
      if (u.searchParams.get("name") === "_atproto.alice.example.com") {
        return new Response(
          JSON.stringify({ Status: 0, Answer: [{ type: 16, data: `"did=${did}"` }] }),
          { headers: { "content-type": "application/dns-json" } },
        );
      }
      return new Response("not found", { status: 404 });
    });
    const byHandle = (await t.engine.call("resolveActor", ["alice.example.com"])) as Record<
      string,
      unknown
    >;
    expect(byHandle.did).toBe(did);
    expect(byHandle.handle).toBe("alice.example.com");
    const byDid = (await t.engine.call("resolveActor", [did])) as Record<string, unknown>;
    expect(byDid.did).toBe(did);
    // every request was anonymous and went through the JS fetch
    expect(t.bridged).toEqual([]);
    expect(t.fetched.some((u) => u.startsWith(`https://plc.directory/${did}`))).toBe(true);
  });

  it("orients a photo in the SPA's prepareMedia, not only strips its EXIF (m4-6)", async () => {
    // an 8×4 JPEG (no EXIF), and the same with an APP1 whose IFD0 says orientation 6
    const plain = Uint8Array.from(atob(JPEG_8X4), (c) => c.charCodeAt(0));
    const tiff = [0x49, 0x49, 0x2a, 0, 8, 0, 0, 0, 1, 0, 0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0];
    const app1 = [...new TextEncoder().encode("Exif\0\0"), ...tiff, 0, 0, 0, 0];
    const seg = [0xff, 0xe1, (app1.length + 2) >> 8, (app1.length + 2) & 0xff, ...app1];
    const photo = Uint8Array.from([...plain.subarray(0, 2), ...seg, ...plain.subarray(2)]);
    const { engine } = await tab(origin(), { elect: false });
    const ref = (await engine.call("prepareMedia", [
      Array.from(photo),
      { declaredMime: "image/jpeg" },
    ])) as { cid: string; mime: string; sizeBytes: number };
    expect(ref.mime).toBe("image/jpeg");
    // what stripping alone gives: SOI, then everything after the JFIF APP0 (18 bytes) - the
    // pixels unrotated. The orientation pass re-encodes, so the prepared file is another one.
    const stripped = Uint8Array.from([...plain.subarray(0, 2), ...plain.subarray(20)]);
    expect(ref.cid).not.toBe(await rawCid(stripped));
    expect(ref.sizeBytes).not.toBe(stripped.length);
  });

  it("refuses a mismatched namespace, a bad configuration and a second init", async () => {
    const o = origin();
    const wasm = await load();
    const env: WorkerEnv = { fetch, locks: o.locks, idb: null, warn: () => {} };
    const bridge = { request: async () => ({ status: 500, headers: [], body: new Uint8Array() }) };
    const e1 = new WorkerEngine(wasm, env);
    const ns = await rejects(e1.init({ config: {}, namespace: "x.y" }, bridge, () => {}));
    expect(ns.message).toMatch(/mismatched builds/);
    const e2 = new WorkerEngine(wasm, env);
    const cfg = await rejects(
      e2.init({ config: { noSuchKey: 1 } as never, namespace: NS }, bridge, () => {}),
    );
    expect(cfg.code).toBe("invalid_input");
    const { engine } = await tab(o, { elect: false });
    expect((await rejects(engine.init({ config: {}, namespace: NS }, bridge, () => {}))).code).toBe(
      "conflict",
    );
  });

  it("elects one leader per origin and hands over when it closes", async () => {
    const o = origin();
    const a = await tab(o);
    const b = await tab(o);
    await tick(10);
    expect([a.engine.isLeader, b.engine.isLeader]).toEqual([true, false]);
    await a.engine.close();
    await tick(10);
    expect(b.engine.isLeader).toBe(true);
  });

  it("applies another tab's change and forwards it", async () => {
    const o = origin();
    const a = await tab(o, { elect: false });
    const b = await tab(o, { elect: false });
    const raw = o.channel();
    const changed = {
      kind: "changed",
      topics: [{ kind: "forum", uri: "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/x.y.forum/f" }],
    };
    raw.postMessage({ type: "event", from: "another-tab", event: changed });
    await tick(10);
    expect(a.events).toEqual([changed]);
    expect(b.events).toEqual([changed]);
    expect([...a.warnings, ...b.warnings]).toEqual([]);
  });

  it("keeps the state store in IndexedDB across workers", async () => {
    const o = origin();
    const a = await tab(o, { elect: false, dbPrefix: "p" });
    await a.engine.close();
    const names = (await o.idb.factory.databases()).map((d) => d.name).sort();
    expect(names).toEqual(["p-blobs", "p-cache", "p-state"]);
    const mem = await tab(origin(), { elect: false, state: { kind: "memory" }, dbPrefix: "m" });
    expect(await mem.engine.callJson("accounts", "[]")).toBe("[]");
  });
});
