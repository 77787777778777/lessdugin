// SPDX-License-Identifier: AGPL-3.0-or-later
// `lessdugin web` mode: an `outbox:<did>` host lock is the tab's Web Lock and then the native
// lock through `POST /bridge/lock` (conformance review M1: a tab and a native client must not
// pump the same outbox rows).
import { describe, expect, it } from "vitest";
import { MemoryBlobStore } from "./blobs.ts";
import { takeBridgeLock } from "./bridge-lock.ts";
import { createJsHost } from "./host.ts";
import { MemKv } from "./idb.ts";
import { LocalLocks } from "./locks.ts";

const tick = (ms = 0) => new Promise<void>((r) => setTimeout(r, ms));
const enc = new TextEncoder();

interface Call {
  url: string;
  body: { name: string; wait: boolean };
  headers: Record<string, string>;
  signal: AbortSignal;
  /** Ends a held body (the server stopped). */
  end(): void;
}

type Answer = "held" | "busy" | 401 | 500 | "garbage";

/** A `/bridge/lock` server: answers in order, a held body sends heartbeats until aborted. */
function server(answers: Answer[]) {
  const calls: Call[] = [];
  const fetchFn = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const signal = init?.signal as AbortSignal;
    let end = () => {};
    calls.push({
      url: String(input),
      body: JSON.parse(String(init?.body)) as Call["body"],
      headers: init?.headers as Record<string, string>,
      signal,
      end: () => end(),
    });
    const a = answers.shift() ?? "busy";
    if (a === 401 || a === 500) return new Response("{}", { status: a });
    if (a === "busy") return new Response('{"held":false}', { status: 200 });
    if (a === "garbage") return new Response("<html>", { status: 200 });
    const body = new ReadableStream<Uint8Array>({
      start(c) {
        c.enqueue(enc.encode('{"held":true}\n'));
        const beat = setInterval(() => c.enqueue(enc.encode("\n")), 5);
        end = () => {
          clearInterval(beat);
          c.close();
        };
        signal.addEventListener("abort", () => {
          clearInterval(beat);
          c.error(new DOMException("aborted", "AbortError"));
        });
      },
    });
    return new Response(body, { status: 200 });
  }) as typeof fetch;
  return { calls, fetchFn };
}

function host(fetchFn: typeof fetch, locks = new LocalLocks(), onLost?: (n: string) => void) {
  return createJsHost({
    cache: new MemKv(),
    state: new MemKv(),
    fetch: { fetch: fetchFn, policy: { devnet: false } },
    userPds: async () => {
      throw new Error("unused");
    },
    blobs: new MemoryBlobStore(),
    locks,
    bridgeLock: {
      endpoint: "/bridge/lock",
      token: "t".repeat(43),
      fetch: fetchFn,
      retryMs: 1,
      ...(onLost ? { onLost } : {}),
    },
  });
}

describe("the native outbox lock in lessdugin web mode (conformance review M1)", () => {
  it("takes the Web Lock, then the native lock, and releases both", async () => {
    const s = server(["held"]);
    const locks = new LocalLocks();
    const h = host(s.fetchFn, locks);
    const held = await h.lock("outbox:did:plc:a", false);
    expect(held).not.toBeNull();
    expect(s.calls).toHaveLength(1);
    expect(s.calls[0]?.url).toBe("/bridge/lock");
    expect(s.calls[0]?.body).toEqual({ name: "outbox:did:plc:a", wait: false });
    expect(s.calls[0]?.headers.authorization).toBe(`Bearer ${"t".repeat(43)}`);
    expect(locks.names).toEqual(["lessdugin-outbox-did:plc:a"]);
    await tick(20);
    expect(s.calls[0]?.signal.aborted).toBe(false);
    held?.release();
    expect(s.calls[0]?.signal.aborted).toBe(true);
    await tick();
    expect(locks.names).toEqual([]);
  });

  it("answers null and frees the Web Lock when a native client holds the outbox", async () => {
    const s = server(["busy"]);
    const locks = new LocalLocks();
    expect(await host(s.fetchFn, locks).lock("outbox:did:plc:a", false)).toBeNull();
    await tick();
    expect(locks.names).toEqual([]);
  });

  it("does not ask the server while another tab holds the Web Lock", async () => {
    const s = server(["held"]);
    const locks = new LocalLocks();
    const other = await host(server(["held"]).fetchFn, locks).lock("outbox:did:plc:a", false);
    expect(await host(s.fetchFn, locks).lock("outbox:did:plc:a", false)).toBeNull();
    expect(s.calls).toHaveLength(0);
    other?.release();
  });

  it("asks again while waiting until the native lock is free", async () => {
    const s = server(["busy", "busy", "held"]);
    const held = await host(s.fetchFn).lock("outbox:did:plc:a", true);
    expect(held).not.toBeNull();
    expect(s.calls.map((c) => c.body.wait)).toEqual([true, true, true]);
    held?.release();
  });

  it("maps a revoked session to authRequired and an odd answer to storage", async () => {
    for (const [answer, kind] of [
      [401, "authRequired"],
      [500, "storage"],
      ["garbage", "storage"],
    ] as const) {
      const locks = new LocalLocks();
      await expect(
        host(server([answer]).fetchFn, locks).lock("outbox:did:plc:a", true),
      ).rejects.toMatchObject({ kind });
      await tick();
      expect(locks.names).toEqual([]);
    }
  });

  it("reports a held lock whose body ended before the release", async () => {
    const s = server(["held"]);
    const lost: string[] = [];
    const held = await host(s.fetchFn, new LocalLocks(), (n) => lost.push(n)).lock(
      "outbox:did:plc:a",
      false,
    );
    s.calls[0]?.end();
    await tick(10);
    expect(lost).toEqual(["outbox:did:plc:a"]);
    held?.release();
    // a release is not a loss
    const t = server(["held"]);
    const lost2: string[] = [];
    const h2 = await takeBridgeLock(
      { endpoint: "/l", token: "x", fetch: t.fetchFn, onLost: (n) => lost2.push(n) },
      "outbox:did:plc:b",
      false,
    );
    h2?.release();
    await tick(10);
    expect(lost2).toEqual([]);
  });

  it("bridges outbox locks only", async () => {
    const s = server(["held"]);
    const locks = new LocalLocks();
    const held = await host(s.fetchFn, locks).lock("sync-leader", false);
    expect(held).not.toBeNull();
    expect(s.calls).toHaveLength(0);
    held?.release();
  });
});
