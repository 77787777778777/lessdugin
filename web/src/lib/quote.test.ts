// SPDX-License-Identifier: AGPL-3.0-or-later
// @vitest-environment jsdom
// (Ranges over a parsed body: jsdom implements Range.intersectsNode.)

import { afterEach, describe, expect, it } from "vitest";
import { normaliseQuote, projectedText, quoteFor, quoteFromSelection } from "./quote.ts";

/** A rendered body as the SPA decorates it: a footnote reference, a sidenote copy, a link with
 * its `.link-host`, a media button and math. */
const BODY = `<div class="lfm" dir="auto">
<p dir="auto">Votes are public in the same view.<sup id="md-fnref-2-1"><a data-anchor="md-fn-2">2</a></sup><span class="lfm-sidenote" role="note"><span class="lfm-sidenote-n">2</span>See the policy.</span> Read <a href="https://example.com/" rel="nofollow ugc noopener noreferrer">the moderation policy<span class="link-host">example.com</span></a> first.</p>
<p dir="auto">Energy is <math><mi>m</mi><msup><mi>c</mi><mn>2</mn></msup></math> here.</p>
<p dir="auto"><button type="button" class="lfm-media-load">🖼 cat (click to load)</button><img data-cid="bafkreiabc" alt="cat"> A cat.</p>
</div>`;

/** P of BODY (lfm-1.md §10.1): footnote numbers, hosts and SPA additions are not in it; math
 * contributes its TeX source. */
const P =
  "Votes are public in the same view. Read the moderation policy first.\nEnergy is mc^2 here.\ncat A cat.";

function mount(html: string): HTMLElement {
  const host = document.createElement("div");
  host.innerHTML = html;
  document.body.append(host);
  return host;
}

function textNode(root: Element, contains: string): Text {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    if ((n as Text).data.includes(contains)) return n as Text;
  }
  throw new Error(`no text node with ${contains}`);
}

function select(root: Element, from: string, to: string): Range {
  const a = textNode(root, from);
  const b = textNode(root, to);
  const range = document.createRange();
  range.setStart(a, a.data.indexOf(from));
  range.setEnd(b, b.data.indexOf(to) + to.length);
  const sel = window.getSelection();
  sel?.removeAllRanges();
  sel?.addRange(range);
  return range;
}

afterEach(() => {
  window.getSelection()?.removeAllRanges();
  document.body.replaceChildren();
});

describe("quote anchoring (lfm-1.md §11 Writers)", () => {
  it("projects a selection the way P sees it, skipping presentation", () => {
    const root = mount(BODY);
    const range = select(root, "same view.", "the moderation policy");
    // Selection.toString() would read "same view.2See the policy. Read the moderation policy".
    expect(normaliseQuote(projectedText(root, range))).toBe(
      "same view. Read the moderation policy",
    );
  });

  it("anchors such a selection in P, with context and a code-point start", () => {
    const root = mount(BODY);
    select(root, "same view.", "the moderation policy");
    const q = quoteFromSelection(root, P);
    expect(q).toEqual({
      kind: "anchored",
      quote: {
        exact: "same view. Read the moderation policy",
        start: P.indexOf("same view."),
        prefix: "Votes are public in the ",
        // 32 graphemes of context (lfm-1.md §11).
        suffix: " first.\nEnergy is mc^2 here.\ncat",
      },
    });
  });

  it("reports a selection that is not verbatim in P (here: across math) as unmatched", () => {
    const root = mount(BODY);
    select(root, "Energy is", "here.");
    expect(quoteFromSelection(root, P)).toEqual({ kind: "unmatched", exact: "Energy is here." });
  });

  it("skips media buttons and images", () => {
    const root = mount(BODY);
    const range = select(root, "cat (click", "A cat.");
    expect(normaliseQuote(projectedText(root, range))).toBe("A cat.");
  });

  it("ignores empty selections and selections outside the body", () => {
    const root = mount(BODY);
    const other = mount("<p>elsewhere text</p>");
    expect(quoteFromSelection(root, P)).toBeNull();
    select(other, "elsewhere", "text");
    expect(quoteFromSelection(root, P)).toBeNull();
  });

  it("normalises whitespace and NFC, and limits the quote's size", () => {
    expect(normaliseQuote("  a \n\tb  ")).toBe("a b");
    expect(normaliseQuote("é")).toBe("é");
    expect(quoteFor("abc", "   ")).toBeNull();
    expect(quoteFor("x".repeat(3000), "x".repeat(2001))).toBeNull();
    const q = quoteFor("🙂 hello world", "world");
    expect(q?.kind === "anchored" && q.quote.start).toBe(8);
  });
});
