// SPDX-License-Identifier: AGPL-3.0-or-later
// The real-engine adapter with the worker client and the OAuth client replaced by fakes: the
// start-up work before the router mounts (OAuth callback, bridge capability), the login
// providers, and the stand-ins for operations the wasm engine does not implement yet.

import type { AccountInfo, EngineError } from "@lessdugin/engine/types";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const engineCalls: { op: string; args: unknown[] }[] = [];
let addAccountImpl: (info: AccountInfo) => Promise<unknown> = async () => ({});
const started: { init?: unknown }[] = [];

vi.mock("@lessdugin/engine/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("@lessdugin/engine/client")>();
  return {
    ...orig,
    startEngine: (opts: { init?: unknown }) => {
      started.push(opts);
      return new Proxy(
        {},
        {
          get(_t, prop) {
            if (typeof prop !== "string" || prop === "then") return undefined;
            if (prop === "ready")
              return Promise.resolve({ version: "t", namespace: "ns", id: "1" });
            if (prop === "version") return async () => "test-version";
            return async (...args: unknown[]) => {
              engineCalls.push({ op: prop, args });
              if (prop === "addAccount") return addAccountImpl(args[0] as AccountInfo);
              if (prop === "drafts" || prop === "settings" || prop === "setSetting") {
                throw { code: "unsupported", message: "no", retryable: false };
              }
              if (prop === "listPosts") {
                throw { code: "network", message: "down", retryable: true };
              }
              return null;
            };
          },
        },
      );
    },
  };
});

const oauth = {
  finish: vi.fn(),
  begin: vi.fn(),
  revoke: vi.fn(async () => {}),
  session: vi.fn(async () => null),
};
vi.mock("../host/hosted.ts", () => ({
  HostedOAuth: { create: vi.fn(async () => oauth) },
}));

const { takeBootParams } = await import("../lib/bootParams.ts");
const { createRealEngine } = await import("./real.ts");
const { loginNotice } = await import("../host/loginNotice.ts");

function at(url: string): void {
  history.replaceState(null, "", url);
  takeBootParams();
}

beforeEach(() => {
  engineCalls.length = 0;
  started.length = 0;
  loginNotice.set(null);
  oauth.finish.mockReset();
  oauth.begin.mockReset();
  oauth.revoke.mockClear();
  addAccountImpl = async () => ({});
  sessionStorage.clear();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("hosted mode", () => {
  it("starts with IndexedDB state and no callback work", async () => {
    at("/#/f/x/y");
    const h = await createRealEngine({ mode: "hosted" });
    expect(h.kind).toBe("wasm");
    expect(await h.version()).toBe("test-version");
    expect(started[0]).toMatchObject({ init: { config: {} } });
    expect(oauth.finish).not.toHaveBeenCalled();
    expect(location.hash).toBe("#/f/x/y");
  });

  it("finishes an OAuth callback before the router mounts: addAccount, then the saved route", async () => {
    oauth.finish.mockResolvedValue({
      did: "did:plc:aaaaaaaaaaaaaaaaaaaaaaaa",
      pds: "https://pds.example.com",
      issuer: "https://auth.example.com",
      returnTo: "#/f/did:plc:bbbbbbbbbbbbbbbbbbbbbbbb/meta",
      intended: "did:plc:aaaaaaaaaaaaaaaaaaaaaaaa",
    });
    at("/?state=s1&code=c1&iss=https%3A%2F%2Fauth.example.com#/");
    await createRealEngine({ mode: "hosted" });
    expect(oauth.finish).toHaveBeenCalledWith({
      state: "s1",
      code: "c1",
      iss: "https://auth.example.com",
    });
    expect(engineCalls).toContainEqual({
      op: "addAccount",
      args: [
        {
          did: "did:plc:aaaaaaaaaaaaaaaaaaaaaaaa",
          pds: "https://pds.example.com",
          auth: "oauth",
          issuer: "https://auth.example.com",
          intended: "did:plc:aaaaaaaaaaaaaaaaaaaaaaaa",
        },
      ],
    });
    // the callback parameters left the address bar; the route is the one saved at login
    expect(location.search).toBe("");
    expect(location.hash).toBe("#/f/did:plc:bbbbbbbbbbbbbbbbbbbbbbbb/meta");
    expect(loginNotice.get()).toBeNull();
  });

  it("revokes the session and shows the error when the login binding fails", async () => {
    oauth.finish.mockResolvedValue({
      did: "did:plc:aaaaaaaaaaaaaaaaaaaaaaaa",
      pds: "https://pds.example.com",
      issuer: "https://evil.example.com",
      returnTo: null,
      intended: null,
    });
    addAccountImpl = async () => {
      const e: EngineError = { code: "blocked", message: "issuer not listed", retryable: false };
      throw e;
    };
    at("/?state=s&code=c#/");
    await createRealEngine({ mode: "hosted" });
    expect(oauth.revoke).toHaveBeenCalledWith("did:plc:aaaaaaaaaaaaaaaaaaaaaaaa");
    expect(loginNotice.get()).toMatchObject({ kind: "error" });
    expect(location.hash).toBe("#/login");
  });

  it("asks before accepting a changed authorization server", async () => {
    oauth.finish.mockResolvedValue({
      did: "did:plc:aaaaaaaaaaaaaaaaaaaaaaaa",
      pds: "https://pds.example.com",
      issuer: "https://new.example.com",
      returnTo: null,
      intended: null,
    });
    let calls = 0;
    addAccountImpl = async (info) => {
      calls++;
      if (info.confirmIssuerChange) return {};
      const e: EngineError = {
        code: "conflict",
        message: "changed",
        retryable: false,
        detail: {
          kind: "issuerChanged",
          pinned: "https://old.example.com",
          issuer: "https://new.example.com",
        },
      };
      throw e;
    };
    at("/?state=s&code=c#/");
    await createRealEngine({ mode: "hosted" });
    const n = loginNotice.get();
    expect(n).toMatchObject({ kind: "confirmIssuer", pinned: "https://old.example.com" });
    expect(oauth.revoke).not.toHaveBeenCalled();
    if (n?.kind !== "confirmIssuer") throw new Error("no confirmation");
    await n.confirm();
    expect(calls).toBe(2);
    expect(loginNotice.get()).toBeNull();
  });

  it("reports a callback the library refuses (state, error) on the sign-in page", async () => {
    oauth.finish.mockRejectedValue(new Error("Unknown authorization session"));
    at("/?state=s&error=access_denied#/");
    await createRealEngine({ mode: "hosted" });
    expect(engineCalls.find((c) => c.op === "addAccount")).toBeUndefined();
    expect(loginNotice.get()).toMatchObject({
      kind: "error",
      message: expect.stringContaining("Unknown authorization session"),
    });
    expect(location.hash).toBe("#/login");
  });

  it("logs in by redirecting to the authorization URL for the resolved DID", async () => {
    at("/#/");
    oauth.begin.mockResolvedValue(new URL("https://auth.example.com/authorize?x=1"));
    const assign = vi.fn();
    vi.stubGlobal("location", { ...location, assign, hash: "#/", pathname: "/", search: "" });
    const h = await createRealEngine({ mode: "hosted" });
    void h.login.login(
      { did: "did:plc:aaaaaaaaaaaaaaaaaaaaaaaa", handleState: "verified", shortDid: "aaaa" },
      { returnTo: "#/p/x/y" },
    );
    await vi.waitFor(() =>
      expect(assign).toHaveBeenCalledWith("https://auth.example.com/authorize?x=1"),
    );
    expect(oauth.begin).toHaveBeenCalledWith("did:plc:aaaaaaaaaaaaaaaaaaaaaaaa", "#/p/x/y");
  });

  it("signs out by revoking, then removing the account", async () => {
    at("/#/");
    const h = await createRealEngine({ mode: "hosted" });
    await h.login.logout("did:plc:aaaaaaaaaaaaaaaaaaaaaaaa");
    expect(oauth.revoke).toHaveBeenCalledWith("did:plc:aaaaaaaaaaaaaaaaaaaaaaaa");
    expect(engineCalls).toContainEqual({
      op: "removeAccount",
      args: ["did:plc:aaaaaaaaaaaaaaaaaaaaaaaa", false],
    });
  });

  it("answers operations of later milestones with their empty stand-ins, and only those", async () => {
    at("/#/");
    const h = await createRealEngine({ mode: "hosted" });
    await expect(h.api.drafts()).resolves.toEqual([]);
    await expect(h.api.settings()).resolves.toMatchObject({ acknowledgedPublicActions: [] });
    await h.api.setSetting("acknowledgedPublicActions", ["vote"]);
    await expect(h.api.settings()).resolves.toMatchObject({ acknowledgedPublicActions: ["vote"] });
    await expect(
      h.api.listPosts({ forum: "x", sort: "new", filter: "all", limit: 1 }),
    ).rejects.toMatchObject({ code: "network" });
  });
});

describe("lessdugin web mode", () => {
  const CAP = "A".repeat(43);
  const TOKEN = "T".repeat(43);

  it("exchanges the capability before the router mounts and keeps the token in sessionStorage", async () => {
    const fetchMock = vi.fn(async () => new Response(JSON.stringify({ token: TOKEN })));
    vi.stubGlobal("fetch", fetchMock);
    at(`/#lessdugin-cap=${CAP}`);
    expect(location.hash).toBe("#/");
    const h = await createRealEngine({ mode: "bridge" });
    expect(fetchMock).toHaveBeenCalledWith(
      "/bridge/session",
      expect.objectContaining({ method: "POST", body: JSON.stringify({ cap: CAP }) }),
    );
    expect(sessionStorage.getItem("lessdugin-bridge-token")).toBe(TOKEN);
    expect(started[0]).toMatchObject({
      init: { state: { kind: "bridge", endpoint: "/bridge/kv", token: TOKEN } },
    });
    expect(h.notice).toContain("lessdugin web");
    await expect(h.login.logout("did:plc:aaaaaaaaaaaaaaaaaaaaaaaa")).rejects.toMatchObject({
      code: "unsupported",
    });
  });

  it("reuses the tab's token after a reload or a sign-in", async () => {
    sessionStorage.setItem("lessdugin-bridge-token", TOKEN);
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
    at("/#/");
    await createRealEngine({ mode: "bridge" });
    expect(fetchMock).not.toHaveBeenCalled();
    expect(started[0]).toMatchObject({ init: { state: { token: TOKEN } } });
  });

  it("stops with the §7.2 message when the link was used already, or there is no session", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response('{"error":"auth_required"}', { status: 403 })),
    );
    at(`/#lessdugin-cap=${CAP}`);
    await expect(createRealEngine({ mode: "bridge" })).rejects.toMatchObject({
      name: "BridgeLinkError",
      message: expect.stringContaining("already used or has expired"),
    });
    at("/#/");
    await expect(createRealEngine({ mode: "bridge" })).rejects.toMatchObject({
      name: "BridgeLinkError",
    });
  });
});
