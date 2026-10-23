// SPDX-License-Identifier: AGPL-3.0-or-later
// @vitest-environment node
import { describe, expect, it } from "vitest";
// @ts-expect-error -- a plain .mjs script without type declarations
import { check } from "../../scripts/check-no-raw-href.mjs";

type Problem = { rule: string; line: number };
const rules = (file: string, src: string): string[] =>
  (check(file, src) as Problem[]).map((p) => p.rule);

describe("check-no-raw-href", () => {
  it("flags raw href, inline style, dangerouslySetInnerHTML and HTML sinks", () => {
    const src = [
      "export function X({ s }: { s: string }) {",
      "  document.body.innerHTML = s;",
      '  return <p>Don\'t <a href="https://x">x</a><i style={{}} dangerouslySetInnerHTML={{ __html: s }} /></p>;',
      "}",
    ].join("\n");
    expect(rules("components/X.tsx", src)).toEqual([
      "html-sink",
      "raw-href",
      "inline-style",
      "dsih",
    ]);
  });
  it("ignores strings, comments and the two allowed files", () => {
    expect(rules("components/X.tsx", 'const s = "href={x} style={y}"; // <a href="x">')).toEqual(
      [],
    );
    expect(rules("components/SafeLink.tsx", "export const L = () => <a href={u} />;")).toEqual([]);
    expect(rules("pages/P.tsx", "export const P = () => <SafeLink href={h}>x</SafeLink>;")).toEqual(
      [],
    );
    expect(rules("lib/sanitize.ts", "el.innerHTML = html;")).toEqual([]);
    expect(rules("lib/navigate.ts", "win.location.assign(url.href);")).toEqual([]);
    expect(rules("lib/navigate.ts", "a.href = u;")).toEqual(["dom-url"]);
    expect(rules("lib/navigate.ts", "location.replace(u);")).toEqual(["dom-url"]);
  });
  it("flags DOM writes that set a URL or navigate", () => {
    const lines = [
      "a.href = u;",
      "img.src = u;",
      'el.setAttribute("href", u);',
      'el.setAttribute("SRC", u);',
      'el.setAttribute("style", s);',
      'el.setAttribute("srcdoc", s);',
      'el.setAttribute("onclick", s);',
      "el.setAttribute(name, u);",
      'el.setAttributeNS(null, "xlink:href", u);',
      "location = u;",
      "window.location.href = u;",
      "location.hash = h;",
      "location.assign(u);",
      "document.location.replace(u);",
      "window.open(u);",
      "open(u);",
      'React.createElement("a", { href: u });',
    ];
    for (const line of lines) {
      expect(rules("components/X.tsx", line), line).toEqual(["dom-url"]);
    }
  });

  it("allows non-URL attributes, object URLs in lib/download.ts, and tests", () => {
    expect(rules("components/X.tsx", 'el.setAttribute("tabindex", "0");')).toEqual([]);
    expect(rules("components/X.tsx", 'el.setAttribute("data-sidenote", n);')).toEqual([]);
    expect(rules("lib/download.ts", "a.href = URL.createObjectURL(blob);")).toEqual([]);
    expect(rules("lib/x.test.ts", "a.href = u; el.innerHTML = h;")).toEqual([]);
  });

  it("flags other URL-bearing JSX attributes and spread props on intrinsic elements", () => {
    expect(rules("components/X.tsx", "export const X = () => <img src={u} alt='' />;")).toEqual([
      "raw-href",
    ]);
    expect(rules("components/X.tsx", "export const X = () => <form action={u} />;")).toEqual([
      "raw-href",
    ]);
    expect(
      rules("components/X.tsx", "export const X = () => <svg><use xlinkHref={u} /></svg>;"),
    ).toEqual(["raw-href"]);
    expect(rules("components/X.tsx", "export const X = () => <button formAction={u} />;")).toEqual([
      "raw-href",
    ]);
    expect(rules("components/X.tsx", "export const X = (p: object) => <a {...p} />;")).toEqual([
      "jsx-spread",
    ]);
    // Components take engine values (an `Href`), never raw URLs: spreads on them are fine.
    expect(rules("components/X.tsx", "export const X = (p: P) => <SafeLink {...p} />;")).toEqual(
      [],
    );
  });

  it("flags code from strings", () => {
    expect(rules("components/X.tsx", "eval(s);")).toEqual(["eval"]);
    expect(rules("components/X.tsx", "new Function(s);")).toEqual(["eval"]);
    expect(rules("components/X.tsx", 'setTimeout("alert(1)", 1);')).toEqual(["eval"]);
    expect(rules("components/X.tsx", "setTimeout(() => f(), 1);")).toEqual([]);
  });
});
