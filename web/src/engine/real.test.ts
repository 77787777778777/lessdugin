// SPDX-License-Identifier: AGPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { createRealEngine } from "./real.ts";

describe("real engine adapter", () => {
  it("rejects every operation with a clear unsupported error until the wasm engine has it", async () => {
    const handle = createRealEngine();
    await expect(
      handle.api.listPosts({ forum: "x", sort: "magic", filter: "all", limit: 1 }),
    ).rejects.toMatchObject({
      code: "unsupported",
      message: expect.stringContaining("engine not yet available"),
    });
    await expect(
      handle.login.login({ did: "did:plc:x", handleState: "verified", shortDid: "x" }),
    ).rejects.toMatchObject({
      code: "unsupported",
    });
    expect(typeof handle.api.subscribe(() => {})).toBe("function");
  });
});
