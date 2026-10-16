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
  });
});
