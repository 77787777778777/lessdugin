// SPDX-License-Identifier: AGPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { goToSignIn, isAuthRedirect } from "./navigate.ts";

describe("sign-in redirects", () => {
  it("go to https anywhere and to http on loopback hosts only", () => {
    const ok = [
      "https://bsky.social/oauth/authorize?request_uri=x",
      "http://localhost:2583/oauth/authorize",
      "http://127.0.0.1:41234/x",
      "http://[::1]:8080/x",
    ];
    const bad = [
      "javascript:alert(1)",
      "data:text/html,x",
      "http://example.com/oauth/authorize",
      "https://user:pw@example.com/",
      "blob:https://example.com/x",
    ];
    for (const u of ok) expect(isAuthRedirect(new URL(u)), u).toBe(true);
    for (const u of bad) expect(isAuthRedirect(new URL(u)), u).toBe(false);
  });

  it("navigate only after the check", () => {
    const assigned: string[] = [];
    const win = { location: { assign: (u: string) => assigned.push(u) } } as never;
    goToSignIn(new URL("https://pds.example/oauth/authorize?x=1"), win);
    expect(() => goToSignIn(new URL("javascript:alert(1)"), win)).toThrow(/refusing/);
    expect(assigned).toEqual(["https://pds.example/oauth/authorize?x=1"]);
  });
});
