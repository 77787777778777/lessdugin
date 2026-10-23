// SPDX-License-Identifier: AGPL-3.0-or-later
// The AuthBridge allowlist (threat-model AUTH-4), session binding (AUTH-8) and the two session
// kinds the SPA plugs in.
import { describe, expect, it } from "vitest";
import {
  AuthBridge,
  bridgeSession,
  checkRequest,
  NS,
  oauthSession,
  type PdsSession,
  RECORD_COLLECTIONS,
  type SessionFetchInit,
} from "./auth-bridge.ts";
import { isHostError, type PdsRequest } from "./protocol.ts";

const ALICE = "did:plc:alice";
const BOB = "did:plc:bob";
const enc = (v: unknown): Uint8Array => new TextEncoder().encode(JSON.stringify(v));
const JSON_CT: [string, string][] = [["content-type", "application/json"]];

function req(over: Partial<PdsRequest> & { path: string }): PdsRequest {
  return {
    account: ALICE,
    method: "POST",
    headers: [],
    maxBytes: 1 << 20,
    timeoutMs: 5000,
    ...over,
  };
}

function write(nsid: string, body: unknown, over: Partial<PdsRequest> = {}): PdsRequest {
  return req({ path: `/xrpc/${nsid}`, headers: JSON_CT, body: enc(body), ...over });
}

function blocked(f: () => unknown): string {
  try {
    f();
  } catch (e) {
    if (isHostError(e) && e.kind === "blocked") return e.message;
    throw e;
  }
  throw new Error("not refused");
}

const POST = `${NS}.post`;

describe("checkRequest allows", () => {
  it("getSession as a bodiless GET", () => {
    const c = checkRequest(
      req({ method: "GET", path: "/xrpc/com.atproto.server.getSession", headers: [] }),
    );
    expect(c).toEqual({
      nsid: "com.atproto.server.getSession",
      method: "GET",
      path: "/xrpc/com.atproto.server.getSession",
    });
  });

  it("record writes to every NS collection of the account's repo", () => {
    for (const col of RECORD_COLLECTIONS) {
      for (const m of ["createRecord", "putRecord", "deleteRecord"]) {
        const nsid = `com.atproto.repo.${m}`;
        const c = checkRequest(write(nsid, { repo: ALICE, collection: `${NS}.${col}`, rkey: "x" }));
        expect(c.path).toBe(`/xrpc/${nsid}`);
        expect(c.contentType).toBe("application/json");
      }
    }
  });

  it("applyWrites whose every op targets an NS collection", () => {
    const body = {
      repo: ALICE,
      writes: [
        { $type: "com.atproto.repo.applyWrites#create", collection: POST, value: {} },
        { $type: "com.atproto.repo.applyWrites#update", collection: `${NS}.forum`, rkey: "a" },
        { $type: "com.atproto.repo.applyWrites#delete", collection: `${NS}.comment`, rkey: "b" },
      ],
    };
    expect(checkRequest(write("com.atproto.repo.applyWrites", body)).nsid).toBe(
      "com.atproto.repo.applyWrites",
    );
  });

  it("uploadBlob of a declared type, with parameters stripped", () => {
    const c = checkRequest(
      req({
        path: "/xrpc/com.atproto.repo.uploadBlob",
        headers: [["Content-Type", "image/PNG; charset=binary"]],
        body: new Uint8Array([1, 2, 3]),
      }),
    );
    expect(c.contentType).toBe("image/png");
    expect(Array.from(c.body ?? [])).toEqual([1, 2, 3]);
  });

  it("re-serialises the checked JSON (no duplicate keys reach the PDS)", () => {
    const raw = `{"repo":"${ALICE}","collection":"app.bsky.feed.post","collection":"${POST}","rkey":"r"}`;
    const c = checkRequest(
      req({
        path: "/xrpc/com.atproto.repo.createRecord",
        headers: JSON_CT,
        body: new TextEncoder().encode(raw),
      }),
    );
    expect(new TextDecoder().decode(c.body)).toBe(
      JSON.stringify({ repo: ALICE, collection: POST, rkey: "r" }),
    );
  });
});

describe("checkRequest refuses", () => {
  it("methods off the allowlist and odd paths", () => {
    for (const path of [
      "/xrpc/com.atproto.repo.listRecords",
      "/xrpc/com.atproto.server.createSession",
      "/xrpc/com.atproto.identity.updateHandle",
      "/xrpc/app.bsky.feed.post",
      "/xrpc/com.atproto.server.getSession?x=1",
      "/xrpc/com.atproto.repo.createRecord?repo=did:plc:bob",
      "/xrpc/../oauth/token",
      "/oauth/token",
      "https://evil.example/xrpc/com.atproto.repo.createRecord",
      "",
    ]) {
      blocked(() => checkRequest(write(path.replace(/^\/xrpc\//, ""), {}, { path })));
    }
  });

  it("wrong HTTP methods", () => {
    blocked(() =>
      checkRequest(req({ method: "POST", path: "/xrpc/com.atproto.server.getSession" })),
    );
    blocked(() =>
      checkRequest(
        req({ method: "GET", path: "/xrpc/com.atproto.server.getSession", body: enc({}) }),
      ),
    );
    blocked(() =>
      checkRequest(
        write(
          "com.atproto.repo.createRecord",
          { repo: ALICE, collection: POST },
          { method: "GET" },
        ),
      ),
    );
  });

  it("writes outside the namespace or to another repo", () => {
    for (const collection of [
      "app.bsky.feed.post",
      NS,
      `${NS}.`,
      `${NS}.unknown`,
      `${NS}.post.extra`,
      `${NS}x.post`,
      "online.7778777.lessdugin.post",
      42,
      undefined,
    ]) {
      blocked(() =>
        checkRequest(write("com.atproto.repo.putRecord", { repo: ALICE, collection, rkey: "x" })),
      );
    }
    blocked(() =>
      checkRequest(write("com.atproto.repo.createRecord", { repo: BOB, collection: POST })),
    );
    blocked(() => checkRequest(write("com.atproto.repo.createRecord", { collection: POST })));
  });

  it("applyWrites with no ops, a foreign op or an unknown op type", () => {
    const op = (collection: string, t = "create") => ({
      $type: `com.atproto.repo.applyWrites#${t}`,
      collection,
    });
    for (const writes of [
      [],
      undefined,
      [op(POST), op("app.bsky.feed.like")],
      [op(POST, "upsert")],
      [{ collection: POST }],
      [null],
    ]) {
      blocked(() => checkRequest(write("com.atproto.repo.applyWrites", { repo: ALICE, writes })));
    }
  });

  it("bodies that are not a JSON object, or not declared JSON", () => {
    const path = "/xrpc/com.atproto.repo.createRecord";
    for (const body of ["[]", "null", '"x"', "{", ""]) {
      blocked(() =>
        checkRequest(req({ path, headers: JSON_CT, body: new TextEncoder().encode(body) })),
      );
    }
    blocked(() =>
      checkRequest(req({ path, headers: JSON_CT, body: new Uint8Array([0x7b, 0xff, 0x7d]) })),
    );
    blocked(() =>
      checkRequest(
        req({
          path,
          headers: [["content-type", "text/plain"]],
          body: enc({ repo: ALICE, collection: POST }),
        }),
      ),
    );
  });

  it("uploads of an undeclared type", () => {
    for (const ct of ["text/html", "image/svg+xml", "application/octet-stream", ""]) {
      blocked(() =>
        checkRequest(
          req({
            path: "/xrpc/com.atproto.repo.uploadBlob",
            headers: ct ? [["content-type", ct]] : [],
          }),
        ),
      );
    }
  });
});

const create = write("com.atproto.repo.createRecord", {
  repo: ALICE,
  collection: POST,
  record: { text: "hi" },
});

interface Sent {
  path: string;
  init: SessionFetchInit;
}

function fakeSession(
  did: string,
  answer: () => Response | Promise<Response>,
): PdsSession & { sent: Sent[] } {
  const sent: Sent[] = [];
  return {
    did,
    sent,
    async fetch(path, init) {
      sent.push({ path, init });
      return answer();
    },
  };
}

const sessions = (...list: PdsSession[]) => ({
  session: async (did: string) => list.find((s) => s.did === did) ?? null,
});

async function fails(p: Promise<unknown>): Promise<{ kind: string; message: string }> {
  try {
    await p;
  } catch (e) {
    if (isHostError(e)) return e;
    throw e;
  }
  throw new Error("did not fail");
}

describe("AuthBridge", () => {
  it("sends the checked request through the account's session", async () => {
    const s = fakeSession(ALICE, () =>
      Response.json({ uri: "at://x", cid: "c" }, { headers: { "x-test": "1" } }),
    );
    const bridge = new AuthBridge(sessions(s));
    const r = await bridge.request({
      ...create,
      headers: [...JSON_CT, ["authorization", "Bearer stolen"], ["accept", "application/json"]],
    });
    expect(r.status).toBe(200);
    expect(JSON.parse(new TextDecoder().decode(r.body))).toEqual({ uri: "at://x", cid: "c" });
    expect(r.headers).toContain("x-test");
    expect(s.sent).toHaveLength(1);
    const sent = s.sent[0] as Sent;
    expect(sent.path).toBe("/xrpc/com.atproto.repo.createRecord");
    expect(sent.init.method).toBe("POST");
    // only content-type and accept are passed on; the session adds its own credentials
    expect([...sent.init.headers.keys()].sort()).toEqual(["accept", "content-type"]);
  });

  it("refuses before asking for a session", async () => {
    const s = fakeSession(ALICE, () => new Response("{}"));
    const bridge = new AuthBridge(sessions(s));
    const e = await fails(
      bridge.request(write("com.atproto.repo.createRecord", { repo: ALICE, collection: "x.y.z" })),
    );
    expect(e.kind).toBe("blocked");
    expect(s.sent).toHaveLength(0);
  });

  it("needs a session for the account, bound to that account (AUTH-8)", async () => {
    const bob = fakeSession(BOB, () => new Response("{}"));
    expect((await fails(new AuthBridge(sessions()).request(create))).kind).toBe("authRequired");
    const misbound = new AuthBridge({ session: async () => bob });
    expect((await fails(misbound.request(create))).kind).toBe("blocked");
    expect(bob.sent).toHaveLength(0);
  });

  it("never follows a redirect and caps the answer", async () => {
    const redirect = fakeSession(
      ALICE,
      () => new Response(null, { status: 302, headers: { location: "https://evil.example/" } }),
    );
    expect((await fails(new AuthBridge(sessions(redirect)).request(create))).kind).toBe("blocked");
    const big = fakeSession(ALICE, () => new Response(new Uint8Array(2048)));
    const e = await fails(new AuthBridge(sessions(big)).request({ ...create, maxBytes: 1024 }));
    expect(e.kind).toBe("tooLarge");
  });

  it("passes error statuses through for the engine to classify", async () => {
    const s = fakeSession(ALICE, () =>
      Response.json({ error: "InvalidRequest", message: "no" }, { status: 400 }),
    );
    const r = await new AuthBridge(sessions(s)).request(create);
    expect(r.status).toBe(400);
  });

  it("reports a failed send as network and a stalled one as timeout", async () => {
    const down = fakeSession(ALICE, () => {
      throw new TypeError("Failed to fetch");
    });
    expect((await fails(new AuthBridge(sessions(down)).request(create))).kind).toBe("network");
    const stall: PdsSession = {
      did: ALICE,
      fetch: (_path, init) =>
        new Promise((_, reject) => {
          init.signal.addEventListener("abort", () =>
            reject(new DOMException("aborted", "AbortError")),
          );
        }),
    };
    const e = await fails(new AuthBridge(sessions(stall)).request({ ...create, timeoutMs: 10 }));
    expect(e.kind).toBe("timeout");
  });

  it("checks against this build's namespace only, which nothing can change", async () => {
    const s = fakeSession(ALICE, () => new Response("{}"));
    const bridge = new AuthBridge(sessions(s));
    const profile = write("com.atproto.repo.putRecord", {
      repo: ALICE,
      collection: "app.bsky.actor.profile",
      rkey: "self",
    });
    expect((await fails(bridge.request(profile))).kind).toBe("blocked");
    // no property of the bridge widens the allowlist (security review #1)
    const b = bridge as unknown as Record<string, unknown>;
    b.namespace = "app.bsky.actor";
    expect((await fails(bridge.request(profile))).kind).toBe("blocked");
    expect(s.sent).toHaveLength(0);
    // the pure check takes another prefix (the final namespace of release 0.1)
    const finalNs = write("com.atproto.repo.createRecord", {
      repo: ALICE,
      collection: "online.7778777.lessdugin.post",
    });
    expect(checkRequest(finalNs, "online.7778777.lessdugin").nsid).toBe(
      "com.atproto.repo.createRecord",
    );
    expect(blocked(() => checkRequest(finalNs))).toContain("collection");
  });
});

describe("sessions", () => {
  it("bridgeSession posts to /bridge/xrpc/<nsid>?account= with the bearer token", async () => {
    const calls: { url: string; init: RequestInit }[] = [];
    const f = (async (url: string, init: RequestInit) => {
      calls.push({ url, init });
      return new Response("{}");
    }) as typeof fetch;
    const s = bridgeSession({ did: ALICE, token: "tok", fetch: f });
    const bridge = new AuthBridge(sessions(s));
    await bridge.request(create);
    await bridge.request(
      req({ method: "GET", path: "/xrpc/com.atproto.server.getSession", headers: [] }),
    );
    expect(calls.map((c) => c.url)).toEqual([
      `/bridge/xrpc/com.atproto.repo.createRecord?account=${encodeURIComponent(ALICE)}`,
      `/bridge/xrpc/com.atproto.server.getSession?account=${encodeURIComponent(ALICE)}`,
    ]);
    for (const c of calls) {
      const h = new Headers(c.init.headers);
      expect(c.init.method).toBe("POST");
      expect(h.get("authorization")).toBe("Bearer tok");
      expect(h.get("content-type")).toBe("application/json");
      expect(c.init.credentials).toBe("omit");
      expect(c.init.cache).toBe("no-store");
      expect(c.init.redirect).toBe("error");
    }
    // getSession goes as POST {} (the bridge takes POST only)
    expect(new TextDecoder().decode(calls[1]?.init.body as Uint8Array)).toBe("{}");
  });

  it("bridgeSession returns PDS answers and rejects the bridge's own errors", async () => {
    const answers: Response[] = [
      // forwarded from the PDS: returned as it is, even a 401
      new Response('{"error":"InvalidToken"}', {
        status: 401,
        headers: { "x-lessdugin-upstream": "pds", "content-type": "application/json" },
      }),
      // the bridge's own answers
      new Response('{"error":"auth_required","message":"no session"}', { status: 401 }),
      new Response('{"error":"auth_required","message":"not logged in"}', { status: 403 }),
      new Response('{"error":"blocked","message":"collection"}', { status: 403 }),
      new Response('{"error":"network","message":"down"}', { status: 502 }),
      new Response('{"error":"network","message":"slow"}', { status: 504 }),
      new Response("not json", { status: 413 }),
    ];
    const f = (async () => {
      const r = answers.shift();
      if (!r) throw new Error("no more answers");
      return r;
    }) as typeof fetch;
    const s = bridgeSession({ did: ALICE, token: "tok", fetch: f });
    const bridge = new AuthBridge(sessions(s));
    const first = await bridge.request(create);
    expect(first.status).toBe(401);
    const kinds: string[] = [];
    for (let i = 0; i < 6; i++) {
      kinds.push(
        await bridge.request(create).then(
          () => "ok",
          (e: { kind: string }) => e.kind,
        ),
      );
    }
    expect(kinds).toEqual([
      "authRequired",
      "authRequired",
      "blocked",
      "network",
      "timeout",
      "tooLarge",
    ]);
  });

  it("oauthSession hands the checked request to the OAuth fetch handler", async () => {
    const calls: { path: string; init: RequestInit }[] = [];
    const s = oauthSession(ALICE, async (path, init) => {
      calls.push({ path, init });
      return new Response("{}");
    });
    await new AuthBridge(sessions(s)).request(create);
    expect(calls).toHaveLength(1);
    expect(calls[0]?.path).toBe("/xrpc/com.atproto.repo.createRecord");
    expect(calls[0]?.init.method).toBe("POST");
    expect(new Headers(calls[0]?.init.headers).get("authorization")).toBeNull();
    // AUTH-8: never follow a redirect; sync.md §12.1: no HTTP cache
    expect(calls[0]?.init.redirect).toBe("manual");
    expect(calls[0]?.init.cache).toBe("no-store");
    expect(calls[0]?.init.credentials).toBe("omit");
  });

  it("oauthSession: a redirect reaches the bridge as an opaque answer and is refused", async () => {
    // what fetch returns for a 307 under `redirect: "manual"`
    const opaque = { type: "opaqueredirect", status: 0, body: null, headers: new Headers() };
    const s = oauthSession(ALICE, async (_path, init) => {
      expect(init.redirect).toBe("manual");
      return opaque as unknown as Response;
    });
    const e = await fails(new AuthBridge(sessions(s)).request(create));
    expect(e.kind).toBe("blocked");
  });
});
