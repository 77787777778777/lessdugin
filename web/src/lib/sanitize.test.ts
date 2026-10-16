// SPDX-License-Identifier: AGPL-3.0-or-later
// @vitest-environment jsdom
// DOMPurify needs a spec-compliant HTML parser (namespaces, raw-text elements): jsdom, not
// happy-dom, whose parser loses elements and MathML namespaces.
import { MockEngine } from "@lessdugin/engine/mock";
import { describe, expect, it } from "vitest";
import { attributeAllowed, sanitizeLfm, sanitizerMismatch } from "./sanitize.ts";

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
    ["mark element", "<mark>x</mark>", "<mark"],
    ["template", "<template><img src=x onerror=alert(1)></template>", "onerror"],
    ["object", '<object data="x.swf"></object>', "object"],
    ["meta refresh", '<meta http-equiv="refresh" content="0;url=https://evil.example">', "meta"],
  ])("strips %s", (_name, dirty, needle) => {
    const out = sanitizeLfm(dirty);
    expect(out.toLowerCase()).not.toContain(needle);
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
});
