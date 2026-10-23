// SPDX-License-Identifier: AGPL-3.0-or-later
// The fixed-schema port between the engine worker and the main thread's AuthBridge: round trips,
// event order, and what a compromised worker can send (security review #1).
import { afterEach, describe, expect, it } from "vitest";
import { connectBridgePort, parsePdsRequest, serveBridgePort } from "./bridge-port.ts";
import { type HttpResult, hostError, type PdsRequest } from "./protocol.ts";

const tick = () => new Promise<void>((r) => setTimeout(r, 10));

const REQ: PdsRequest = {
  account: "did:plc:a",
  method: "POST",
  path: "/xrpc/com.atproto.repo.createRecord",
  headers: [["content-type", "application/json"]],
  body: new Uint8Array([123, 125]),
  maxBytes: 1024,
  timeoutMs: 1000,
};

const closers: (() => void)[] = [];
afterEach(() => {
  for (const c of closers.splice(0)) c();
});

function pair(handler: (req: PdsRequest) => Promise<HttpResult>) {
  const { port1, port2 } = new MessageChannel();
  const seen: PdsRequest[] = [];
  const events: string[] = [];
  const stop = serveBridgePort(port1, {
    request: (req) => {
      seen.push(req);
      return handler(req);
    },
    event: (json) => events.push(json),
  });
  const link = connectBridgePort(port2);
  closers.push(stop, () => link.close());
  return { link, seen, events, workerPort: port2 };
}

describe("parsePdsRequest", () => {
  it("rebuilds a well-formed request field by field", () => {
    const r = parsePdsRequest({ ...REQ, extra: "dropped" });
    expect(r).toEqual(REQ);
    expect(r?.body).not.toBe(REQ.body);
    expect(Object.keys(r ?? {}).sort()).toEqual(
      ["account", "body", "headers", "maxBytes", "method", "path", "timeoutMs"].sort(),
    );
    const get = parsePdsRequest({ ...REQ, method: "GET", body: undefined });
    expect(get && "body" in get).toBe(false);
  });

  it("refuses anything else", () => {
    const bad: unknown[] = [
      null,
      "req",
      [],
      { ...REQ, account: 1 },
      { ...REQ, account: "" },
      { ...REQ, method: "PUT" },
      { ...REQ, path: { toString: () => "/xrpc/x" } },
      { ...REQ, path: "x".repeat(9000) },
      { ...REQ, headers: "content-type: application/json" },
      { ...REQ, headers: [["a"]] },
      { ...REQ, headers: [["a", 1]] },
      { ...REQ, headers: Array.from({ length: 40 }, () => ["a", "b"]) },
      { ...REQ, body: "{}" },
      { ...REQ, body: [1, 2] },
      { ...REQ, maxBytes: -1 },
      { ...REQ, maxBytes: 1.5 },
      { ...REQ, maxBytes: "1024" },
      { ...REQ, timeoutMs: Number.POSITIVE_INFINITY },
      { ...REQ, timeoutMs: 10_000_000 },
    ];
    for (const b of bad) expect(parsePdsRequest(b)).toBeNull();
  });
});

describe("serveBridgePort / connectBridgePort", () => {
  it("round-trips requests (concurrently) and their answers", async () => {
    const { link, seen } = pair(async (req) => ({
      status: 200,
      headers: ["x-path", req.path],
      body: new Uint8Array([1, 2, 3]),
    }));
    const [a, b] = await Promise.all([
      link.bridge.request(REQ),
      link.bridge.request({ ...REQ, path: "/xrpc/com.atproto.repo.putRecord" }),
    ]);
    expect(a).toEqual({
      status: 200,
      headers: ["x-path", "/xrpc/com.atproto.repo.createRecord"],
      body: new Uint8Array([1, 2, 3]),
    });
    expect(b?.headers[1]).toBe("/xrpc/com.atproto.repo.putRecord");
    expect(seen).toHaveLength(2);
  });

  it("passes host errors through as plain values; other throws become `other`", async () => {
    let n = 0;
    const { link } = pair(async () => {
      n++;
      if (n === 1) throw hostError("authRequired", "no session", { status: 401 });
      throw new TypeError("boom");
    });
    await expect(link.bridge.request(REQ)).rejects.toEqual({
      kind: "authRequired",
      message: "no session",
      status: 401,
    });
    await expect(link.bridge.request(REQ)).rejects.toEqual({ kind: "other", message: "boom" });
  });

  it("delivers events in order, strings only", async () => {
    const { link, events, workerPort } = pair(async () => {
      throw new Error("unused");
    });
    link.emit("1");
    workerPort.postMessage({ t: "event", json: 2 });
    link.emit("3");
    await tick();
    expect(events).toEqual(["1", "3"]);
  });

  it("ignores Comlink messages, prototype paths and malformed requests", async () => {
    const { workerPort, seen } = pair(async () => ({
      status: 200,
      headers: [],
      body: new Uint8Array(),
    }));
    const answers: unknown[] = [];
    workerPort.addEventListener("message", (ev) => answers.push(ev.data));
    workerPort.postMessage({ id: "c1", type: "GET", path: ["namespace"] });
    workerPort.postMessage({
      id: "c2",
      type: "SET",
      path: ["__proto__", "__proto__", "pollutedFromWorker"],
      value: { type: "RAW", value: "yes" },
    });
    workerPort.postMessage({ id: "c3", type: "APPLY", path: ["request"], argumentList: [] });
    workerPort.postMessage({ t: "req", id: -1, req: REQ });
    workerPort.postMessage({ t: "req", id: "7", req: REQ });
    workerPort.postMessage({ t: "req", id: 5, req: { ...REQ, method: "DELETE" } });
    await tick();
    expect(({} as Record<string, unknown>).pollutedFromWorker).toBeUndefined();
    expect(seen).toEqual([]);
    // only the malformed request with a valid id gets an answer, a refusal
    expect(answers).toEqual([
      {
        t: "res",
        id: 5,
        ok: false,
        error: { kind: "blocked", message: "malformed bridge request" },
      },
    ]);
  });

  it("rejects pending requests when the worker side closes", async () => {
    const { link } = pair(() => new Promise<HttpResult>(() => {}));
    const p = link.bridge.request(REQ);
    link.close();
    await expect(p).rejects.toMatchObject({ kind: "other" });
    await expect(link.bridge.request(REQ)).rejects.toMatchObject({ kind: "other" });
  });
});
