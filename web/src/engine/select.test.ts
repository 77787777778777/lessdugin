// SPDX-License-Identifier: AGPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { chooseEngine } from "./select.ts";

describe("chooseEngine", () => {
  it("uses the mock for ?mock before or inside the hash", () => {
    expect(chooseEngine({ search: "?mock", hash: "#/" }, {})).toBe("mock");
    expect(chooseEngine({ search: "", hash: "#/f/x/y?mock" }, {})).toBe("mock");
  });
  it("honours VITE_ENGINE, then defaults to the mock only in development", () => {
    expect(chooseEngine({ search: "", hash: "" }, { VITE_ENGINE: "mock" })).toBe("mock");
    expect(chooseEngine({ search: "", hash: "" }, { VITE_ENGINE: "wasm", DEV: true })).toBe("wasm");
    expect(chooseEngine({ search: "", hash: "" }, { DEV: true })).toBe("mock");
    expect(chooseEngine({ search: "", hash: "" }, { DEV: false })).toBe("wasm");
  });
});
