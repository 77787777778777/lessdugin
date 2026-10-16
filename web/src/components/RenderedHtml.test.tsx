// SPDX-License-Identifier: AGPL-3.0-or-later
// @vitest-environment jsdom
// (DOMPurify needs a spec-compliant parser; see lib/sanitize.test.ts.)

import { MockEngine } from "@lessdugin/engine/mock";
import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { qk } from "../engine/queries.ts";
import { renderWithApp } from "../test/render.tsx";
import { ParticipantsProvider } from "./participants.tsx";
import { RenderedHtml } from "./RenderedHtml.tsx";

const SAFE = `<div class="lfm" dir="auto">
<h2 id="md-h-intro" dir="auto">Intro</h2>
<p dir="auto">Hello <strong>world</strong>.<sup id="md-fnref-1-1"><a data-anchor="md-fn-1">1</a></sup> See <a data-anchor="intro">the intro</a>.</p>
<p dir="auto"><a href="https://example.com/" rel="nofollow ugc noopener noreferrer">example</a></p>
<section class="footnotes">
<ol>
<li id="md-fn-1" dir="auto"><p dir="auto">A footnote.</p>
<a data-anchor="md-fnref-1-1" class="fn-back">↩</a></li>
</ol>
</section>
</div>
`;

const EVIL = `<div class="lfm" dir="auto">
<p dir="auto" onclick="alert(1)">x<script>alert(1)</script><img src="x" onerror="alert(2)"><a href="javascript:alert(3)">j</a><iframe src="https://evil.example"></iframe></p>
</div>
`;

function rootOf(container: HTMLElement): HTMLElement {
  const root = container.querySelector("[data-rendered-root]");
  if (!(root instanceof HTMLElement)) throw new Error("no rendered root");
  return root;
}

/** The router renders asynchronously; wait for the rendered root to mount. */
function mountedRoot(container: HTMLElement): Promise<HTMLElement> {
  return waitFor(() => rootOf(container));
}

describe("RenderedHtml", () => {
  it("inserts allowlisted renderer output unchanged", async () => {
    const { container } = renderWithApp(<RenderedHtml html={SAFE} />);
    // The only runtime additions: anchor links become focusable links (lfm-1.md §8.2 allows
    // runtime DOM changes after sanitisation).
    await waitFor(() =>
      expect(rootOf(container).innerHTML.replaceAll(' tabindex="0" role="link"', "")).toBe(SAFE),
    );
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("makes in-content anchor links keyboard-reachable", async () => {
    const scroll = vi.fn();
    Element.prototype.scrollIntoView = scroll;
    const { container } = renderWithApp(<RenderedHtml html={SAFE} />);
    const root = await mountedRoot(container);
    await waitFor(() => expect(root.querySelector("a[data-anchor]")).not.toBeNull());
    const anchors = [...root.querySelectorAll("a[data-anchor]")];
    for (const a of anchors) {
      expect(a.getAttribute("tabindex")).toBe("0");
      expect(a.getAttribute("role")).toBe("link");
    }
    const ref = anchors.find((a) => a.getAttribute("data-anchor") === "md-fn-1");
    if (!(ref instanceof HTMLElement)) throw new Error("no footnote reference");
    ref.focus();
    fireEvent.keyDown(ref, { key: "Enter" });
    const note = root.querySelector("#md-fn-1");
    expect(scroll.mock.contexts.at(-1)).toBe(note);
    expect(document.activeElement).toBe(note);
  });

  it("strips malicious HTML and flags the mismatch in development", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const { container } = renderWithApp(<RenderedHtml html={EVIL} />);
    const root = await mountedRoot(container);
    await waitFor(() => expect(root.querySelector("p")).not.toBeNull());
    const html = root.innerHTML;
    for (const bad of ["<script", "onclick", "onerror", "javascript:", "<iframe", "src="]) {
      expect(html).not.toContain(bad);
    }
    expect(root.textContent).toContain("x");
    expect((await screen.findByRole("alert")).textContent).toContain("Development check failed");
    expect(error).toHaveBeenCalled();
    error.mockRestore();
  });

  it("scrolls data-anchor links inside its own root", async () => {
    const scroll = vi.fn();
    Element.prototype.scrollIntoView = scroll;
    const { container } = renderWithApp(<RenderedHtml html={SAFE} />);
    const root = await mountedRoot(container);
    await waitFor(() => expect(root.querySelector("h2")).not.toBeNull());
    const link = [...root.querySelectorAll("a[data-anchor]")].find(
      (a) => a.textContent === "the intro",
    );
    if (!(link instanceof HTMLElement)) throw new Error("no anchor link");
    fireEvent.click(link);
    expect(scroll).toHaveBeenCalledTimes(1);
    expect(scroll.mock.contexts[0]).toBe(root.querySelector("#md-h-intro"));
  });

  it("adds sidenotes and media placeholders after sanitising", async () => {
    const media = SAFE.replace(
      "</section>\n",
      '</section>\n<div class="gallery">\n<span class="media" data-cid="bafkreiabc">clip.mp4</span>\n</div>\n',
    );
    const { container } = renderWithApp(<RenderedHtml html={media} sidenotes />);
    const root = await mountedRoot(container);
    await waitFor(() => expect(root.querySelector(".lfm-sidenote")).not.toBeNull());
    expect(root.querySelector(".lfm-sidenote")?.textContent).toContain("A footnote.");
    expect(root.querySelector(".lfm-sidenote [id]")).toBeNull();
    expect(screen.getByRole("button", { name: /clip\.mp4 \(click to load\)/ })).toBeTruthy();
  });

  describe("mention resolution (lfm-1.md §5.4, F16)", () => {
    const html = (did: string): string =>
      `<div class="lfm" dir="auto">\n<p dir="auto">Hi <a href="#/u/${did}" data-did="${did}" class="mention">@${did.slice(0, 16)}…</a></p>\n</div>\n`;

    it("resolves participants eagerly and shows their verified handle", async () => {
      const engine = new MockEngine();
      const bob = await engine.resolveActor("bob.test");
      const resolve = vi.spyOn(engine, "resolveActor");
      const { container } = renderWithApp(
        <ParticipantsProvider
          forum={undefined}
          items={[{ author: bob, state: { kind: "visible" } }]}
        >
          <RenderedHtml html={html(bob.did)} />
        </ParticipantsProvider>,
        { engine },
      );
      const root = await mountedRoot(container);
      await waitFor(() => expect(root.querySelector("a.mention")?.textContent).toBe("@bob.test"));
      expect(resolve).toHaveBeenCalledWith(bob.did);
    });

    it("never resolves a non-participant before the reader asks", async () => {
      const engine = new MockEngine();
      const bob = await engine.resolveActor("bob.test");
      const resolve = vi.spyOn(engine, "resolveActor");
      const user = vi.spyOn(engine, "user");
      const { container, queryClient } = renderWithApp(<RenderedHtml html={html(bob.did)} />, {
        engine,
      });
      const root = await mountedRoot(container);
      await waitFor(() => expect(root.querySelector("a.mention")).not.toBeNull());
      await new Promise((r) => setTimeout(r, 50));
      expect(resolve).not.toHaveBeenCalled();
      expect(user).not.toHaveBeenCalled();
      expect(root.querySelector("a.mention")?.textContent).toMatch(/^@did:plc:/);
      // Once the identity is resolved on demand (hover card, profile visit), the mention shows it.
      act(() => queryClient.setQueryData(qk.actor(bob.did), bob));
      await waitFor(() => expect(root.querySelector("a.mention")?.textContent).toBe("@bob.test"));
      expect(resolve).not.toHaveBeenCalled();
    });
  });
});
