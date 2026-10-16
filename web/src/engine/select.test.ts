// SPDX-License-Identifier: AGPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { chooseEngine, mockAllowed } from "./select.ts";

describe("chooseEngine", () => {
  it("uses the mock for ?mock before or inside the hash, in development and demo builds", () => {
    expect(chooseEngine({ search: "?mock", hash: "#/" }, { DEV: true })).toBe("mock");
    expect(chooseEngine({ search: "", hash: "#/f/x/y?mock" }, { VITE_DEMO: "1" })).toBe("mock");
    expect(chooseEngine({ search: "?mock", hash: "#/" }, { VITE_DEMO: "1" })).toBe("mock");
  });
  it("ignores ?mock in a release build", () => {
    expect(mockAllowed({ DEV: false })).toBe(false);
    expect(chooseEngine({ search: "?mock", hash: "#/" }, { DEV: false })).toBe("wasm");
    expect(chooseEngine({ search: "", hash: "#/f/x/y?mock" }, {})).toBe("wasm");
    expect(chooseEngine({ search: "?mock", hash: "#/" }, { VITE_DEMO: "0" })).toBe("wasm");
  });
  it("honours VITE_ENGINE, then defaults to the mock only in development", () => {
    expect(chooseEngine({ search: "", hash: "" }, { VITE_ENGINE: "mock" })).toBe("mock");
    expect(chooseEngine({ search: "", hash: "" }, { VITE_ENGINE: "wasm", DEV: true })).toBe("wasm");
    expect(chooseEngine({ search: "", hash: "" }, { DEV: true })).toBe("mock");
    expect(chooseEngine({ search: "", hash: "" }, { DEV: false })).toBe("wasm");
    expect(chooseEngine({ search: "", hash: "" }, { VITE_DEMO: "1" })).toBe("wasm");
  });
});
