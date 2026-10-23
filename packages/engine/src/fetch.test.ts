// SPDX-License-Identifier: AGPL-3.0-or-later
// Anonymous HTTP (sync.md §12.1, threat-model §5.6.2, INFRA-3) and blob fetching with the CID
// check (§5.6.1).
import { createHash } from "node:crypto";
import { describe, expect, it } from "vitest";
import { fetchBlob, MemoryBlobStore, rawSha256Digest } from "./blobs.ts";
import { checkUrl, fetchAnon, readCapped } from "./fetch.ts";
import { type AnonRequest, isHostError } from "./protocol.ts";

async function kind(p: Promise<unknown> | (() => unknown)): Promise<string> {
  try {
    await (typeof p === "function" ? p() : p);
  } catch (e) {
    if (isHostError(e)) return e.kind;
    throw e;
  }
  return "ok";
}

describe("checkUrl", () => {
  const prod = { devnet: false };
  const dev = { devnet: true };
  it("allows public https URLs", () => {
    for (const u of [
      "https://plc.directory/did:plc:x",
      "https://bsky.social/xrpc/x",
      "https://pds.example.com:8443/",
    ]) {
      expect(checkUrl(u, prod).href).toBe(new URL(u).href);
    }
  });

  it("refuses local, literal, userinfo and non-https URLs", async () => {
    for (const u of [
      "http://plc.directory/",
      "ftp://example.com/",
      "javascript:alert(1)",
      "https://user:pw@example.com/",
      "https://127.0.0.1/",
      "https://0x7f.1/",
      "https://[::1]/",
      "https://10.0.0.1/",
      "https://localhost/",
      "https://printer.local/",
      "https://x.localhost/",
      "https://svc.internal/",
      "https://nas.home.arpa/",
      "https://intranet/",
      "not a url",
    ]) {
      expect(await kind(() => checkUrl(u, prod)), u).toBe("blocked");
    }
  });

  it("allows http and loopback in devnet mode only", async () => {
    expect(checkUrl("http://localhost:2582/", dev).host).toBe("localhost:2582");
    expect(checkUrl("http://127.0.0.1:2582/", dev).host).toBe("127.0.0.1:2582");
    expect(await kind(() => checkUrl("http://10.0.0.1/", dev))).toBe("blocked");
    expect(await kind(() => checkUrl("http://localhost:2582/", prod))).toBe("blocked");
  });
});

function stream(chunks: number[]): ReadableStream<Uint8Array> {
  return new ReadableStream({
    start(c) {
      for (const n of chunks) c.enqueue(new Uint8Array(n));
      c.close();
    },
  });
}

describe("readCapped", () => {
  it("reads up to the cap and fails past it", async () => {
    expect((await readCapped(stream([3, 4]), 7)).length).toBe(7);
    expect((await readCapped(null, 0)).length).toBe(0);
    expect(await kind(readCapped(stream([3, 4, 1]), 7))).toBe("tooLarge");
  });
});

interface Call {
  url: string;
  init: RequestInit;
}

function fakeFetch(answer: (url: string, init: RequestInit) => Response | Promise<Response>) {
  const calls: Call[] = [];
  const f = (async (input: RequestInfo | URL, init: RequestInit = {}) => {
    calls.push({ url: String(input), init });
    return answer(String(input), init);
  }) as typeof fetch;
  return { calls, f };
}

const anon = (over: Partial<AnonRequest> = {}): AnonRequest => ({
  method: "GET",
  url: "https://pds.example.com/xrpc/com.atproto.repo.listRecords?repo=did:plc:a",
  headers: [["accept", "application/json"]],
  maxBytes: 1024,
  timeoutMs: 5000,
  redirects: 0,
  ...over,
});

describe("fetchAnon", () => {
  it("sends no credentials, no cache, no referrer and only safelisted headers", async () => {
    const { calls, f } = fakeFetch(() => Response.json({ ok: 1 }, { headers: { "X-A": "b" } }));
    const r = await fetchAnon(
      anon({
        headers: [
          ["accept", "application/json"],
          ["authorization", "Bearer x"],
          ["cookie", "c"],
        ],
      }),
      { fetch: f, policy: { devnet: false } },
    );
    expect(r.status).toBe(200);
    expect(r.headers).toEqual(expect.arrayContaining(["x-a", "b"]));
    const init = calls[0]?.init as RequestInit;
    expect(init.cache).toBe("no-store");
    expect(init.credentials).toBe("omit");
    expect(init.referrerPolicy).toBe("no-referrer");
    expect(init.redirect).toBe("manual");
    expect([...new Headers(init.headers).keys()]).toEqual(["accept"]);
  });

  it("enforces the byte cap while streaming", async () => {
    const { f } = fakeFetch(() => new Response(stream([600, 600])));
    expect(await kind(fetchAnon(anon(), { fetch: f, policy: { devnet: false } }))).toBe("tooLarge");
  });

  it("answers an unfollowed redirect as a bare 302 and re-checks a followed one", async () => {
    const opaque = { type: "opaqueredirect", status: 0, headers: new Headers(), body: null };
    const { f } = fakeFetch(() => opaque as unknown as Response);
    expect(await fetchAnon(anon(), { fetch: f, policy: { devnet: false } })).toEqual({
      status: 302,
      headers: [],
      body: new Uint8Array(0),
    });
    const followed = new Response("x");
    Object.defineProperty(followed, "redirected", { value: true });
    Object.defineProperty(followed, "url", { value: "https://192.168.1.1/" });
    const g = fakeFetch(() => followed);
    expect(
      await kind(fetchAnon(anon({ redirects: 3 }), { fetch: g.f, policy: { devnet: false } })),
    ).toBe("blocked");
    expect(g.calls[0]?.init.redirect).toBe("follow");
  });

  it("classifies failures: offline, cors (probe reachable), network, timeout", async () => {
    const policy = { devnet: false };
    const down = fakeFetch(() => {
      throw new TypeError("Failed to fetch");
    });
    expect(await kind(fetchAnon(anon(), { fetch: down.f, policy, onLine: () => false }))).toBe(
      "network",
    );
    expect(down.calls).toHaveLength(1);
    expect(await kind(fetchAnon(anon(), { fetch: down.f, policy }))).toBe("network");
    expect(down.calls.at(-1)?.init.mode).toBe("no-cors");
    const corsOnly = fakeFetch((_u, init) => {
      if (init.mode === "no-cors") return new Response(null, { status: 200 });
      throw new TypeError("Failed to fetch");
    });
    expect(await kind(fetchAnon(anon(), { fetch: corsOnly.f, policy }))).toBe("cors");
    const post = fakeFetch(() => {
      throw new TypeError("Failed to fetch");
    });
    expect(
      await kind(
        fetchAnon(anon({ method: "POST", body: new Uint8Array([1]) }), { fetch: post.f, policy }),
      ),
    ).toBe("cors");
    expect(post.calls).toHaveLength(1);
    const stall = fakeFetch(
      (_u, init) =>
        new Promise((_, reject) => {
          init.signal?.addEventListener("abort", () =>
            reject(new DOMException("aborted", "AbortError")),
          );
        }),
    );
    expect(await kind(fetchAnon(anon({ timeoutMs: 10 }), { fetch: stall.f, policy }))).toBe(
      "timeout",
    );
  });

  it("refuses a blocked URL without fetching", async () => {
    const { calls, f } = fakeFetch(() => new Response("x"));
    expect(
      await kind(
        fetchAnon(anon({ url: "https://localhost/x" }), { fetch: f, policy: { devnet: false } }),
      ),
    ).toBe("blocked");
    expect(calls).toHaveLength(0);
  });
});

// ---------------------------------------------------------------------------------------------
// Blobs
// ---------------------------------------------------------------------------------------------

function base32(bytes: Uint8Array): string {
  const A = "abcdefghijklmnopqrstuvwxyz234567";
  let bits = 0;
  let value = 0;
  let out = "";
  for (const b of bytes) {
    value = (value << 8) | b;
    bits += 8;
    while (bits >= 5) {
      out += A[(value >>> (bits - 5)) & 31];
      bits -= 5;
    }
  }
  if (bits > 0) out += A[(value << (5 - bits)) & 31];
  return out;
}

function rawCid(data: Uint8Array): string {
  const digest = createHash("sha256").update(data).digest();
  return `b${base32(Uint8Array.from([0x01, 0x55, 0x12, 0x20, ...digest]))}`;
}

describe("blobs", () => {
  const data = new TextEncoder().encode("hello blob");
  const cid = rawCid(data);
  const did = "did:plc:a";

  it("reads the sha2-256 digest of a raw CIDv1 only", () => {
    expect(Array.from(rawSha256Digest(cid) ?? [])).toEqual(
      Array.from(createHash("sha256").update(data).digest()),
    );
    // dag-cbor codec, CIDv0, bad base32, truncated
    expect(
      rawSha256Digest(
        `b${base32(Uint8Array.from([0x01, 0x71, 0x12, 0x20, ...new Uint8Array(32)]))}`,
      ),
    ).toBeNull();
    expect(rawSha256Digest("QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG")).toBeNull();
    expect(rawSha256Digest("b!!!")).toBeNull();
    expect(rawSha256Digest(cid.slice(0, -4))).toBeNull();
  });

  it("fetches without redirects, checks the CID and stores the blob", async () => {
    const store = new MemoryBlobStore();
    const { calls, f } = fakeFetch(() => new Response(data));
    const deps = { fetch: f, policy: { devnet: false }, store };
    const h = await fetchBlob(deps, "https://pds.example.com", did, cid, 1024);
    expect(h).toEqual({ cid, size: data.length, locator: `mem:${cid}` });
    const u = new URL(calls[0]?.url ?? "");
    expect(u.pathname).toBe("/xrpc/com.atproto.sync.getBlob");
    expect(u.searchParams.get("did")).toBe(did);
    expect(u.searchParams.get("cid")).toBe(cid);
    expect(calls[0]?.init.redirect).toBe("manual");
    // stored: no second request
    await fetchBlob(deps, "https://pds.example.com", did, cid, 1024);
    expect(calls).toHaveLength(1);
  });

  it("refuses mismatched bytes, oversize blobs, redirects and HTTP errors", async () => {
    const policy = { devnet: false };
    const pds = "https://pds.example.com";
    const other = fakeFetch(() => new Response(new TextEncoder().encode("other bytes")));
    const store = new MemoryBlobStore();
    expect(await kind(fetchBlob({ fetch: other.f, policy, store }, pds, did, cid, 1024))).toBe(
      "cidMismatch",
    );
    expect(await store.get(cid)).toBeUndefined();
    const big = fakeFetch(() => new Response(data));
    expect(await kind(fetchBlob({ fetch: big.f, policy, store }, pds, did, cid, 4))).toBe(
      "tooLarge",
    );
    const opaque = { type: "opaqueredirect", status: 0, headers: new Headers(), body: null };
    const redirect = fakeFetch(() => opaque as unknown as Response);
    expect(await kind(fetchBlob({ fetch: redirect.f, policy, store }, pds, did, cid, 1024))).toBe(
      "blocked",
    );
    const missing = fakeFetch(() => Response.json({ error: "BlobNotFound" }, { status: 400 }));
    try {
      await fetchBlob({ fetch: missing.f, policy, store }, pds, did, cid, 1024);
      expect.fail("no error");
    } catch (e) {
      expect(e).toMatchObject({ kind: "httpStatus", status: 400, name: "BlobNotFound" });
    }
    expect(await kind(fetchBlob({ fetch: missing.f, policy, store }, pds, did, "bafy", 1))).toBe(
      "unsupported",
    );
  });

  /** A body that sends `parts` with `gapMs` between them, then (optionally) stalls. */
  function trickle(
    parts: Uint8Array[],
    gapMs: number,
    stall: boolean,
    signal?: AbortSignal | null,
  ) {
    let i = 0;
    return new ReadableStream<Uint8Array>({
      async pull(c) {
        if (i < parts.length) {
          await new Promise((r) => setTimeout(r, gapMs));
          c.enqueue(parts[i++] as Uint8Array);
          return;
        }
        if (!stall) return c.close();
        await new Promise<void>((_, reject) =>
          signal?.addEventListener("abort", () =>
            reject(new DOMException("aborted", "AbortError")),
          ),
        );
      },
    });
  }

  it("aborts a download after the idle timeout without a byte, but has no total timeout (limits.md §7)", async () => {
    const policy = { devnet: false };
    const pds = "https://pds.example.com";
    // no answer at all
    const silent = fakeFetch(
      (_u, init) =>
        new Promise<Response>((_, reject) =>
          init.signal?.addEventListener("abort", () =>
            reject(new DOMException("aborted", "AbortError")),
          ),
        ),
    );
    const store = new MemoryBlobStore();
    expect(
      await kind(fetchBlob({ fetch: silent.f, policy, store, idleMs: 30 }, pds, did, cid, 1024)),
    ).toBe("timeout");
    // an answer whose body stops half-way
    const half = [data.slice(0, 4)];
    const stalls = fakeFetch((_u, init) => new Response(trickle(half, 0, true, init.signal)));
    expect(
      await kind(fetchBlob({ fetch: stalls.f, policy, store, idleMs: 30 }, pds, did, cid, 1024)),
    ).toBe("timeout");
    // slow but steady: longer in total than the idle timeout, every gap shorter
    const parts = Array.from(data, (b) => Uint8Array.of(b));
    const steady = fakeFetch(() => new Response(trickle(parts, 10, false)));
    const h = await fetchBlob({ fetch: steady.f, policy, store, idleMs: 60 }, pds, did, cid, 1024);
    expect(h.size).toBe(data.length);
    expect(await (await store.get(cid))?.text()).toBe("hello blob");
  });
});
