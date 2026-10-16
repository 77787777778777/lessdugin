// SPDX-License-Identifier: AGPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { splitBootParams } from "./bootParams.ts";

describe("splitBootParams (threat-model SPA-8)", () => {
  it("removes an OAuth callback's parameters and keeps the rest", () => {
    const r = splitBootParams({ search: "?code=abc&state=xyz&iss=https%3A%2F%2Fpds", hash: "#/" });
    expect(r.params.oauth).toEqual({ code: "abc", state: "xyz", iss: "https://pds" });
    expect(r.search).toBe("");
    expect(r.hash).toBe("#/");
    const e = splitBootParams({ search: "?mock&error=access_denied&state=s", hash: "" });
    expect(e.params.oauth).toEqual({ error: "access_denied", state: "s" });
    expect(e.search).toBe("?mock=");
  });

  it("leaves ordinary pages alone", () => {
    const r = splitBootParams({ search: "?mock", hash: "#/search?q=code" });
    expect(r.params).toEqual({ oauth: null, bridgeCapability: null });
    expect(r.search).toBe("?mock");
    expect(r.hash).toBe("#/search?q=code");
  });

  it("takes the bridge capability out of the fragment", () => {
    const cap = "A".repeat(43);
    const r = splitBootParams({ search: "", hash: `#lessdugin-cap=${cap}` });
    expect(r.params.bridgeCapability).toBe(cap);
    expect(r.hash).toBe("#/");
    const bad = splitBootParams({ search: "", hash: "#lessdugin-cap=<script>" });
    expect(bad.params.bridgeCapability).toBeNull();
    expect(bad.hash).toBe("#/");
  });
});
