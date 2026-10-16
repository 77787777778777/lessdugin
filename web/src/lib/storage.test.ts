// SPDX-License-Identifier: AGPL-3.0-or-later
import { afterEach, describe, expect, it } from "vitest";
import { readLocal, setStorageNamespace, storageKey, writeLocal } from "./storage.ts";

afterEach(() => {
  setStorageNamespace("wasm");
  window.localStorage.clear();
});

describe("per-viewer storage", () => {
  it("keeps the demo engine's state apart from the real engine's", () => {
    setStorageNamespace("mock");
    writeLocal("currentForum", "at://did:plc:mockmockmockmockmockmock/x.y.forum/demo");
    expect(storageKey("currentForum")).toBe("lessdugin.mock.currentForum");
    setStorageNamespace("wasm");
    expect(readLocal("currentForum")).toBeNull();
    expect(window.localStorage.getItem("lessdugin.currentForum")).toBeNull();
  });
});
