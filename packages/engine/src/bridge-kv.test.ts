// SPDX-License-Identifier: AGPL-3.0-or-later
// The `lessdugin web` state store: the `POST /bridge/kv` wire format of crates/cli/src/web/kv.rs,
// served here by a fake bridge over the in-memory store.
import { describe, expect, it } from "vitest";
import { BridgeKv, fromBase64, toBase64 } from "./bridge-kv.ts";
import { MemKv, OP_DELETE, OP_DELETE_RANGE, OP_PUT } from "./idb.ts";
import { isHostError } from "./protocol.ts";

type Bound = { included: string } | { excluded: string };

/** A fake `/bridge/kv` with the bridge's request shapes (`deny_unknown_fields`). */
function fakeBridge(token: string) {
  const kv = new MemKv();
  const requests: unknown[] = [];
  const bound = (b: Bound): [Uint8Array, boolean] =>
    "included" in b ? [fromBase64(b.included), false] : [fromBase64(b.excluded), true];
  const f = (async (url: string, init: RequestInit) => {
    expect(url).toBe("/bridge/kv");
    expect(init.method).toBe("POST");
    expect(init.credentials).toBe("omit");
    if (new Headers(init.headers).get("authorization") !== `Bearer ${token}`) {
      return new Response("", { status: 401 });
    }
    const req = JSON.parse(String(init.body)) as Record<string, unknown>;
    requests.push(req);
    if (req.op === "get_many") {
      expect(Object.keys(req).sort()).toEqual(["keys", "op"]);
      const values = await kv.getMany((req.keys as string[]).map(fromBase64));
      return Response.json({ values: values.map((v) => (v ? toBase64(v) : null)) });
    }
    if (req.op === "scan") {
      expect(Object.keys(req).sort()).toEqual(["dir", "end", "limit", "op", "start"]);
      const [lo, loOpen] = bound(req.start as Bound);
      const [hi, hiOpen] = bound(req.end as Bound);
      const flat = await kv.scan(
        lo,
        loOpen,
        hi,
        hiOpen,
        req.dir === "reverse",
        req.limit as number,
      );
      const entries: [string, string][] = [];
      for (let i = 0; i < flat.length; i += 2) {
        entries.push([toBase64(flat[i] as Uint8Array), toBase64(flat[i + 1] as Uint8Array)]);
      }
      return Response.json({ entries });
    }
    if (req.op === "write") {
      const ops: unknown[] = [];
      for (const op of req.batch as Record<string, unknown>[]) {
        const [tag] = Object.keys(op);
        if (tag === "put") {
          const [k, v] = op.put as [string, string];
          ops.push(OP_PUT, fromBase64(k), fromBase64(v));
        } else if (tag === "delete") {
          ops.push(OP_DELETE, fromBase64(op.delete as string));
        } else if (tag === "deleteRange") {
          const [s, e] = op.deleteRange as [Bound, Bound];
          const [lo, loOpen] = bound(s);
          const [hi, hiOpen] = bound(e);
          ops.push(OP_DELETE_RANGE, lo, loOpen, hi, hiOpen);
        } else {
          return new Response("bad op", { status: 400 });
        }
      }
      await kv.write(ops);
      return Response.json({});
    }
    return new Response("bad", { status: 400 });
  }) as unknown as typeof fetch;
  return { f, requests };
}

const b = (...xs: number[]) => Uint8Array.from(xs);
const hex = (k: Uint8Array) => Array.from(k, (x) => x.toString(16).padStart(2, "0")).join("");

describe("BridgeKv", () => {
  it("round-trips base64 including the empty and high bytes", () => {
    for (const v of [b(), b(0), b(0xff, 0xfe, 0x80), new Uint8Array(70_000).fill(0xab)]) {
      expect(hex(fromBase64(toBase64(v)))).toBe(hex(v));
    }
  });

  it("speaks the bridge protocol", async () => {
    const { f, requests } = fakeBridge("tok");
    const s = new BridgeKv({ endpoint: "/bridge/kv", token: "tok", fetch: f });
    await s.write([OP_PUT, b(1, 1), b(9), OP_PUT, b(1, 2), b(8), OP_PUT, b(1, 3), b(7)]);
    expect((await s.getMany([b(1, 2), b(5)])).map((v) => (v ? hex(v) : null))).toEqual([
      "08",
      null,
    ]);
    const rev = await s.scan(b(1), true, b(1, 3), true, true, 10);
    expect(rev.map(hex)).toEqual(["0102", "08", "0101", "09"]);
    expect(requests.at(-1)).toEqual({
      op: "scan",
      start: { excluded: "AQ==" },
      end: { excluded: "AQM=" },
      dir: "reverse",
      limit: 10,
    });
    await s.write([OP_DELETE, b(1, 1), OP_DELETE_RANGE, b(1, 2), false, b(1, 3), false]);
    expect(await s.scan(b(1), false, b(2), false, false, 10)).toEqual([]);
  });

  it("sends nothing for empty requests", async () => {
    const { f, requests } = fakeBridge("tok");
    const s = new BridgeKv({ endpoint: "/bridge/kv", token: "tok", fetch: f });
    expect(await s.getMany([])).toEqual([]);
    expect(await s.scan(b(1), false, b(2), false, false, 0)).toEqual([]);
    expect(await s.scan(b(2), false, b(1), false, false, 5)).toEqual([]);
    await s.write([OP_DELETE_RANGE, b(2), false, b(1), false]);
    expect(requests).toEqual([]);
  });

  it("refuses unbounded ranges and reports an expired session", async () => {
    const { f } = fakeBridge("tok");
    const s = new BridgeKv({ endpoint: "/bridge/kv", token: "tok", fetch: f });
    for (const p of [
      s.scan(undefined, false, b(1), false, false, 1),
      s.write([OP_DELETE_RANGE, b(1), false, undefined, false]),
    ]) {
      const e = await p.then(
        () => null,
        (x: unknown) => x,
      );
      expect(isHostError(e) && e.kind).toBe("unsupported");
    }
    const stale = new BridgeKv({ endpoint: "/bridge/kv", token: "old", fetch: f });
    const e = await stale.getMany([b(1)]).then(
      () => null,
      (x: unknown) => x,
    );
    expect(isHostError(e) && e.kind).toBe("authRequired");
  });
});
