// SPDX-License-Identifier: AGPL-3.0-or-later
// @vitest-environment jsdom
// (DOMPurify needs a spec-compliant parser; see lib/sanitize.test.ts.)

import { fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { renderWithApp } from "../test/render.tsx";
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
    await waitFor(() => expect(rootOf(container).innerHTML).toBe(SAFE));
    expect(screen.queryByRole("alert")).toBeNull();
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
});
