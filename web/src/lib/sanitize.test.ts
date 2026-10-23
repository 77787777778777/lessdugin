// SPDX-License-Identifier: AGPL-3.0-or-later
// @vitest-environment jsdom
// DOMPurify needs a spec-compliant HTML parser (namespaces, raw-text elements): jsdom, not
// happy-dom, whose parser loses elements and MathML namespaces.
import { MockEngine } from "@lessdugin/engine/mock";
import { describe, expect, it } from "vitest";
import { attributeAllowed, linkShapeValid, sanitizeLfm, sanitizerMismatch } from "./sanitize.ts";

const ROOT = (inner: string) => `<div class="lfm" dir="auto">\n${inner}</div>\n`;

async function fixtureBodies(): Promise<{ name: string; html: string }[]> {
  const engine = new MockEngine();
  const forum = (await engine.followed())[0]?.uri ?? "";
  const list = await engine.listPosts({ forum, sort: "new", filter: "all", limit: 100 });
  const out: { name: string; html: string }[] = [];
  for (const p of list.items) {
    const t = await engine.getThread(p.uri, { sort: "old", unreadOnly: false });
    const root = t.post;
    if ("body" in root) out.push({ name: `post ${p.title ?? p.uri}`, html: root.body.html });
    for (const c of t.comments) out.push({ name: `comment ${c.uri}`, html: c.body.html });
  }
  return out;
}

describe("LFM sanitiser (DOMPurify with exactly the lfm-1.md §8.2 allowlist)", () => {
  it("leaves allowlisted renderer output unchanged", () => {
    const html = ROOT(
      [
        '<h2 id="md-h-intro" dir="auto">Intro</h2>\n',
        '<p dir="auto">A <strong>b</strong> <em>c</em> <del>d</del> <code>e</code><sup id="md-fnref-1-1"><a data-anchor="md-fn-1">1</a></sup></p>\n',
        '<p dir="auto"><a href="https://example.com/x" rel="nofollow ugc noopener noreferrer">x<span class="link-host">example.com</span></a> ',
        '<a href="#/at/did:plc:abcdefghijklmnopqrstuvwx/online.7778777.lessdugin.temp.post/k" data-at-uri="at://did:plc:abcdefghijklmnopqrstuvwx/online.7778777.lessdugin.temp.post/k" class="at-link">r</a> ',
        '<a href="#/u/did:plc:abcdefghijklmnopqrstuvwx" data-did="did:plc:abcdefghijklmnopqrstuvwx" class="mention">@did:plc:abcdefgh…</a></p>\n',
        '<div class="alert-note">\n<p dir="auto">n</p>\n</div>\n',
        '<details class="spoiler">\n<summary dir="auto">Spoiler</summary>\n<p dir="auto">s</p>\n</details>\n',
        '<ul>\n<li dir="auto"><input type="checkbox" checked="" disabled=""> t</li>\n</ul>\n',
        '<ol start="3">\n<li dir="auto">o</li>\n</ol>\n',
        '<table>\n<thead>\n<tr>\n<th class="ta-r" dir="auto">h</th>\n</tr>\n</thead>\n<tbody>\n<tr>\n<td dir="auto">d</td>\n</tr>\n</tbody>\n</table>\n',
        '<pre><code class="language-rust">fn x() -&gt; u8 {}\n</code></pre>\n',
        '<figure>\n<img data-cid="bafkreiabc" alt="a" width="4" height="3">\n<figcaption dir="auto">c</figcaption>\n</figure>\n',
        '<p dir="auto"><span class="media" data-cid="bafkreiabc">clip</span> <span class="media-missing">m</span></p>\n',
        '<section class="footnotes">\n<ol>\n<li id="md-fn-1" dir="auto"><p dir="auto">f</p>\n<a data-anchor="md-fnref-1-1" class="fn-back">↩</a></li>\n</ol>\n</section>\n',
      ].join(""),
    );
    expect(sanitizerMismatch(html)).toBeNull();
  });

  it("leaves every mock-engine body unchanged", async () => {
    const bodies = await fixtureBodies();
    expect(bodies.length).toBeGreaterThan(100);
    const changed = bodies.filter((b) => sanitizerMismatch(b.html) !== null).map((b) => b.name);
    expect(changed).toEqual([]);
  });

  it.each([
    ["script", '<p dir="auto">x<script>alert(1)</script></p>', "<script"],
    ["event handler", '<img data-cid="bafkreiabc" alt="" onerror="alert(1)">', "onerror"],
    ["src", '<img src="https://evil.example/x.png" alt="">', "src="],
    ["javascript: href", '<a href="javascript:alert(1)">x</a>', "javascript:"],
    ["relative href", '<a href="/steal">x</a>', "href"],
    ["protocol-relative href", '<a href="//evil.example">x</a>', "href"],
    ["style attribute", '<p dir="auto" style="color:red">x</p>', "style"],
    ["iframe", '<iframe src="https://evil.example"></iframe>', "iframe"],
    ["svg", "<svg><script>alert(1)</script></svg>", "svg"],
    ["form", '<form action="https://evil.example"><input name="pw"></form>', "form"],
    [
      "target",
      '<a href="https://example.com" target="_blank" rel="nofollow ugc noopener noreferrer">x</a>',
      "target",
    ],
    ["data attribute", '<p dir="auto" data-foo="1">x</p>', "data-foo"],
    ["aria attribute", '<p dir="auto" aria-hidden="true">x</p>', "aria-hidden"],
    ["unknown class", '<p class="evil" dir="auto">x</p>', "evil"],
    ["id outside the grammar", '<h2 id="user-content" dir="auto">x</h2>', "user-content"],
    ["wrong rel", '<a href="https://example.com" rel="opener">x</a>', "opener"],
    ["dir other than auto", '<p dir="rtl">x</p>', "rtl"],
    ["math href", '<math href="javascript:alert(1)"><mi>x</mi></math>', "javascript"],
    ["mathml style", '<math><mi style="color:red" mathcolor="red">x</mi></math>', "color"],
    ["mathml class outside §7.3", '<math><mrow class="evil">x</mrow></math>', "evil"],
    [
      "mathml math-core font class",
      '<math><mtext class="math-core-serif-font">x</mtext></math>',
      "font",
    ],
    ["mathml unsorted classes", '<math><mtd class="mc-pr0 mc-al">x</mtd></math>', "class"],
    ["mathml duplicate classes", '<math><mtd class="mc-al mc-al">x</mtd></math>', "class"],
    [
      "mathcolor not lower-case hex",
      '<math><mrow mathcolor="#FF0000">x</mrow></math>',
      "mathcolor",
    ],
    ["mathcolor with a url", '<math><mrow mathcolor="url(#x)">x</mrow></math>', "mathcolor"],
    ["mathml id", '<math><mtd id="eq:1">x</mtd></math>', "eq:1"],
    ["mark element", "<mark>x</mark>", "<mark"],
    ["template", "<template><img src=x onerror=alert(1)></template>", "onerror"],
    ["object", '<object data="x.swf"></object>', "object"],
    ["meta refresh", '<meta http-equiv="refresh" content="0;url=https://evil.example">', "meta"],
  ])("strips %s", (_name, dirty, needle) => {
    const out = sanitizeLfm(dirty);
    expect(out.toLowerCase()).not.toContain(needle);
  });

  it("keeps the lfm-1.md §7.3 MathML classes and mathcolor (ADR 0002 decision 2)", () => {
    const html = ROOT(
      [
        '<p dir="auto"><math display="block"><mtable class="mc-w100" displaystyle="true" scriptlevel="0"><mtr>',
        '<mtd class="mc-w50"></mtd><mtd class="mc-al mc-pl1 mc-pr0"><mi>x</mi></mtd>',
        '<mtd class="mc-ar mc-w50"><mtext>(1)</mtext></mtd></mtr></mtable>',
        '<mrow mathcolor="#ff0000"><mi>y</mi></mrow><mspace class="mc-negthin"></mspace>',
        '<mtext class="mc-fs120 mc-tsc">s</mtext>',
        '<menclose notation="updiagonalstrike"><mi>z</mi><mrow class="menclose-updiagonalstrike"></mrow></menclose>',
        "</math></p>\n",
      ].join(""),
    );
    expect(sanitizerMismatch(html)).toBeNull();
    const mtd = document.createElementNS("http://www.w3.org/1998/Math/MathML", "mtd");
    expect(attributeAllowed(mtd, "class", "mc-al mc-pl1 mc-pr0")).toBe(true);
    expect(attributeAllowed(mtd, "class", "mc-pl1 mc-al")).toBe(false);
    expect(attributeAllowed(mtd, "class", "mc-al  mc-pr0")).toBe(false);
    expect(attributeAllowed(mtd, "class", "")).toBe(false);
    expect(attributeAllowed(mtd, "mathcolor", "#0a1b2c")).toBe(true);
    expect(attributeAllowed(mtd, "mathcolor", "red")).toBe(false);
    expect(attributeAllowed(mtd, "style", "width: 50%")).toBe(false);
    expect(attributeAllowed(mtd, "id", "eq:1")).toBe(false);
  });

  it("enforces per-element attribute grammars", () => {
    const el = document.createElement("code");
    expect(attributeAllowed(el, "class", "math-error")).toBe(true);
    expect(attributeAllowed(el, "class", "language-rust")).toBe(false); // only inside <pre>
    const a = document.createElement("a");
    expect(attributeAllowed(a, "data-did", "did:plc:abcdefghijklmnopqrstuvwx")).toBe(true);
    expect(attributeAllowed(a, "data-did", "javascript:alert(1)")).toBe(false);
    expect(attributeAllowed(a, "href", "#/u/did:plc:abcdefghijklmnopqrstuvwx")).toBe(true);
    expect(attributeAllowed(a, "href", "#frag")).toBe(false);
    const p = document.createElement("p");
    expect(attributeAllowed(p, "id", "md-h-x")).toBe(false);
  });

  // Whole-element shapes (lfm-1.md §8.1): attributes that each pass their own grammar can still
  // combine into something the renderer never emits.
  const DID = "did:plc:abcdefghijklmnopqrstuvwx";
  const URI = `at://${DID}/online.7778777.lessdugin.temp.post/3kabc`;
  it.each([
    [
      "external href on a mention",
      `<a href="https://evil.example/" class="mention">@alice.example</a>`,
      `<a>@alice.example</a>`,
    ],
    [
      "external href next to data-at-uri",
      `<a href="https://evil.example/" data-at-uri="${URI}" class="at-link">r</a>`,
      `<a>r</a>`,
    ],
    [
      "record href that disagrees with data-at-uri",
      `<a href="#/at/${DID}/online.7778777.lessdugin.temp.post/other" data-at-uri="${URI}" class="at-link">r</a>`,
      `<a>r</a>`,
    ],
    [
      "profile href that disagrees with data-did",
      `<a href="#/u/did:plc:zzzzzzzzzzzzzzzzzzzzzzzz" data-did="${DID}" class="mention">@x</a>`,
      `<a>@x</a>`,
    ],
    ["external link without rel", `<a href="https://example.com/">x</a>`, `<a>x</a>`],
    [
      "mailto ext-image",
      `<a href="mailto:a@example.com" class="ext-image" rel="nofollow ugc noopener noreferrer">image: x</a>`,
      `<a>image: x</a>`,
    ],
    [
      "record link without its uri",
      `<a href="#/at/${URI.slice(5)}" class="at-link">r</a>`,
      `<a>r</a>`,
    ],
    [
      "data-anchor with an href",
      `<a href="https://x.example/" data-anchor="md-fn-1">1</a>`,
      `<a>1</a>`,
    ],
    ["input without type", `<p dir="auto"><input disabled=""> t</p>`, `<p dir="auto"> t</p>`],
    ["enabled checkbox", `<p dir="auto"><input type="checkbox"> t</p>`, `<p dir="auto"> t</p>`],
    ["img without data-cid", `<p dir="auto"><img alt="a">x</p>`, `<p dir="auto">x</p>`],
    ["img without alt", `<p dir="auto"><img data-cid="bafkreiabc">x</p>`, `<p dir="auto">x</p>`],
    [
      "img with width only",
      `<p dir="auto"><img data-cid="bafkreiabc" alt="" width="3">x</p>`,
      `<p dir="auto">x</p>`,
    ],
    ["span.media without data-cid", `<span class="media">clip</span>`, `<span>clip</span>`],
    ["bare span", `<span>x</span>`, `<span>x</span>`],
    [
      "data-at-uri with an invalid NSID",
      `<a href="#/at/${DID}/not_an_nsid/k" data-at-uri="at://${DID}/not_an_nsid/k" class="at-link">r</a>`,
      `<a>r</a>`,
    ],
    [
      "data-at-uri with an invalid rkey",
      `<a href="#/at/${DID}/online.7778777.lessdugin.temp.post/.." data-at-uri="at://${DID}/online.7778777.lessdugin.temp.post/.." class="at-link">r</a>`,
      `<a>r</a>`,
    ],
    [
      "data-at-uri with an uppercase did:web",
      `<a href="#/at/did:web:Evil.example/online.7778777.lessdugin.temp.post/k" data-at-uri="at://did:web:Evil.example/online.7778777.lessdugin.temp.post/k" class="at-link">r</a>`,
      `<a>r</a>`,
    ],
  ])("rejects the element shape: %s", (_name, dirty, expected) => {
    expect(sanitizeLfm(dirty)).toBe(expected);
  });

  it("accepts every lfm-1.md §8.1 link shape", () => {
    const shapes = [
      `<a href="https://example.com/" rel="nofollow ugc noopener noreferrer">x</a>`,
      `<a href="mailto:a@example.com" rel="nofollow ugc noopener noreferrer">x</a>`,
      `<a href="https://example.com/i.png" class="ext-image" rel="nofollow ugc noopener noreferrer">image: x</a>`,
      `<a href="#/at/${URI.slice(5)}" data-at-uri="${URI}" class="at-link">r</a>`,
      `<a href="#/u/${DID}" data-did="${DID}" class="at-link">p</a>`,
      `<a href="#/u/${DID}" data-did="${DID}" class="mention">@m</a>`,
      `<a data-anchor="md-fn-1">1</a>`,
      `<a data-anchor="md-fnref-1-1" class="fn-back">↩</a>`,
    ];
    for (const html of shapes) expect(sanitizerMismatch(html)).toBeNull();
    const a = document.createElement("a");
    a.setAttribute("data-anchor", "x");
    expect(linkShapeValid(a)).toBe(true);
    a.setAttribute("href", "#/u/x");
    expect(linkShapeValid(a)).toBe(false);
  });

  it("sanitises the content of an element whose shape was rejected", () => {
    const out = sanitizeLfm(
      `<a href="https://evil.example/" class="mention"><img src="x" onerror="alert(1)" alt="">@x<script>alert(2)</script></a>`,
    );
    expect(out).toBe("<a>@x</a>");
  });

  it("does not leak its hooks into the shared DOMPurify instance", async () => {
    const { default: DOMPurify } = await import("dompurify");
    const html = '<a href="https://example.com/" class="mention" data-foo="1">x</a>';
    expect(DOMPurify.sanitize(html)).toBe(
      '<a href="https://example.com/" class="mention" data-foo="1">x</a>',
    );
    expect(sanitizeLfm(html)).toBe("<a>x</a>");
  });

  it("keeps an alt that contains markup-like text (SAFE_FOR_XML off, see sanitize.ts)", () => {
    const html =
      '<p dir="auto"><img data-cid="bafkreiabc" alt="&quot;&gt;&lt;script&gt;alert(1)&lt;/script&gt;"></p>';
    const t = document.createElement("template");
    t.innerHTML = sanitizeLfm(html);
    expect(t.content.querySelector("img")?.getAttribute("alt")).toBe('"><script>alert(1)</script>');
    expect(t.content.querySelector("script")).toBeNull();
  });
});
