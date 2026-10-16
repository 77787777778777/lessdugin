// SPDX-License-Identifier: AGPL-3.0-or-later
import { screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { renderWithApp } from "../test/render.tsx";
import { EXTERNAL_REL, SafeLink } from "./SafeLink.tsx";

const DID = "did:plc:abcdefghijklmnopqrstuvwx";

describe("SafeLink", () => {
  it("renders external links with rel, no referrer and the visible host", async () => {
    renderWithApp(
      <SafeLink href={{ kind: "external", url: "https://example.com/a", host: "example.com" }}>
        Docs
      </SafeLink>,
    );
    const a = await screen.findByRole("link", { name: /Docs/ });
    expect(a.getAttribute("href")).toBe("https://example.com/a");
    expect(a.getAttribute("rel")).toBe(EXTERNAL_REL);
    expect(a.getAttribute("referrerpolicy")).toBe("no-referrer");
    expect(a.textContent).toContain("example.com");
  });

  it("routes record and profile links in-app", async () => {
    const uri = `at://${DID}/online.7778777.lessdugin.temp.post/abc`;
    renderWithApp(
      <>
        <SafeLink href={{ kind: "record", uri }}>record</SafeLink>
        <SafeLink href={{ kind: "profile", did: DID }}>profile</SafeLink>
      </>,
    );
    expect((await screen.findByRole("link", { name: "record" })).getAttribute("href")).toBe(
      `#/at/${uri.slice(5)}`,
    );
    expect(screen.getByRole("link", { name: "profile" }).getAttribute("href")).toBe(`#/u/${DID}`);
  });

  it("refuses unsafe shapes even when handed an Href", async () => {
    renderWithApp(
      <>
        <SafeLink href={{ kind: "external", url: "javascript:alert(1)", host: "" }}>js</SafeLink>
        <SafeLink href={{ kind: "record", uri: "at://alice.test/x/y" }}>handle authority</SafeLink>
        <SafeLink href={{ kind: "profileHandle", handle: "alice.test" }} />
        <SafeLink href={{ kind: "anchor", id: "intro" }}>anchor</SafeLink>
      </>,
    );
    await screen.findByText("js");
    expect(screen.queryAllByRole("link")).toHaveLength(0);
    expect(screen.getByText("@alice.test")).toBeTruthy();
  });

  it("passes raw field URLs through the engine's safeHref", async () => {
    renderWithApp(
      <>
        <SafeLink raw="https://Example.org/x">ok</SafeLink>
        <SafeLink raw="javascript:alert(1)">bad</SafeLink>
      </>,
    );
    const ok = await screen.findByRole("link", { name: /ok/ });
    expect(ok.getAttribute("href")).toBe("https://example.org/x");
    expect(screen.getByText("bad").closest("a")).toBeNull();
  });
});
