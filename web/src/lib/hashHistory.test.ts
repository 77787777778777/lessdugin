// SPDX-License-Identifier: AGPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { routeHrefFromHash } from "./hashHistory.ts";

describe("routeHrefFromHash", () => {
  it("takes the route path and query from the fragment only", () => {
    expect(routeHrefFromHash("#/search?q=fold")).toBe("/search?q=fold");
    expect(routeHrefFromHash("#/f/did:plc:x/s?sort=top&filter=all")).toBe(
      "/f/did:plc:x/s?sort=top&filter=all",
    );
    expect(routeHrefFromHash("")).toBe("/");
    expect(routeHrefFromHash("#")).toBe("/");
    expect(routeHrefFromHash("#/p/a/b#md-h-intro")).toBe("/p/a/b#md-h-intro");
  });
});
