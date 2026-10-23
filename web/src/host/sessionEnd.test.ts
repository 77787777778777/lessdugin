// SPDX-License-Identifier: AGPL-3.0-or-later
// An ended OAuth session reaches the engine as `authRequired`, never as `network` (robustness
// review H1), and is reported once to the app.
import { AuthBridge } from "@lessdugin/engine/client";
import { describe, expect, it } from "vitest";
import { endAwareSession } from "./sessionEnd.ts";

const DID = "did:plc:alice";
class FakeTokenRefreshError extends Error {}

const create = {
  account: DID,
  method: "POST" as const,
  path: "/xrpc/com.atproto.repo.createRecord",
  headers: [["content-type", "application/json"]] as [string, string][],
  body: new TextEncoder().encode(
    JSON.stringify({ repo: DID, collection: "online.7778777.lessdugin.temp.comment" }),
  ),
  maxBytes: 1 << 20,
  timeoutMs: 5000,
};

function bridgeOver(handler: () => Promise<Response>, ended: string[]): AuthBridge {
  const session = endAwareSession(
    DID,
    handler,
    (e) => e instanceof FakeTokenRefreshError,
    (did) => ended.push(did),
  );
  return new AuthBridge({ session: async () => session });
}

describe("endAwareSession", () => {
  it("turns the library's token errors into authRequired and reports the end", async () => {
    const ended: string[] = [];
    const bridge = bridgeOver(async () => {
      throw new FakeTokenRefreshError("The session was revoked");
    }, ended);
    await expect(bridge.request(create)).rejects.toMatchObject({
      kind: "authRequired",
      message: expect.stringContaining("sign in again"),
    });
    expect(ended).toEqual([DID]);
  });

  it("leaves other failures alone (a failed send stays ambiguous: network)", async () => {
    const ended: string[] = [];
    const bridge = bridgeOver(async () => {
      throw new TypeError("Failed to fetch");
    }, ended);
    await expect(bridge.request(create)).rejects.toMatchObject({ kind: "network" });
    expect(ended).toEqual([]);
  });

  it("passes answers through", async () => {
    const ended: string[] = [];
    const bridge = bridgeOver(async () => Response.json({ uri: "at://x", cid: "c" }), ended);
    expect((await bridge.request(create)).status).toBe(200);
    expect(ended).toEqual([]);
  });
});
