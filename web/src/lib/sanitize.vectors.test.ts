// SPDX-License-Identifier: AGPL-3.0-or-later
// @vitest-environment jsdom
//
// SPA-3 / lfm-1.md §8.5: the sanitiser leaves every renderer output in the shared vectors
// unchanged. jsdom's serialiser differs from the pinned browser's in one respect (it does not
// escape `<` and `>` inside attribute values), so this runner compares the sanitiser's output
// with jsdom's own serialisation of the same HTML, i.e. it asserts that DOMPurify removed or
// changed nothing. The exact string equality `sanitize(html) === html` runs in Chromium
// (e2e/vectors.spec.ts).

import { readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { MATHML_CLASSES, sanitizeLfm } from "./sanitize.ts";

// vitest runs from web/ (jsdom's import.meta.url is not a file: URL).
const VECTORS = `${resolve(process.cwd(), "../testdata/vectors")}/`;
const CRATE = `${resolve(process.cwd(), "../crates/markdown")}/`;

interface Vector {
  id: string;
  html: string;
}

function loadVectors(): Vector[] {
  const out: Vector[] = [];
  for (const dir of ["lfm1", "xss", "mathml"]) {
    for (const file of readdirSync(`${VECTORS}${dir}`).filter((f) => f.endsWith(".json"))) {
      const json = JSON.parse(readFileSync(`${VECTORS}${dir}/${file}`, "utf8")) as {
        vectors?: { name: string; expected?: Record<string, unknown> }[];
      };
      for (const v of json.vectors ?? []) {
        for (const key of ["html", "html_temml"]) {
          const html = v.expected?.[key];
          if (typeof html === "string") out.push({ id: `${dir}/${file}#${v.name}:${key}`, html });
        }
      }
    }
  }
  return out;
}

/** jsdom's serialisation of `html` (what an unchanged DOMPurify pass returns here). */
function reserialised(html: string): string {
  const t = document.createElement("template");
  t.innerHTML = html;
  return t.innerHTML;
}

describe("LFM vectors through the sanitiser (jsdom)", () => {
  const vectors = loadVectors();

  it("covers the lfm1, xss and mathml corpora", () => {
    expect(vectors.length).toBeGreaterThan(300);
  });

  it("changes no renderer output", () => {
    const changed = vectors
      .filter((v) => sanitizeLfm(v.html) !== reserialised(v.html))
      .map((v) => v.id);
    expect(changed).toEqual([]);
  });
});

// lfm-1.md §7.3 / ADR 0002 decision 2: the MathML class tokens are one closed list, kept in step
// between the renderer (`MATHML_CLASSES` in crates/markdown/src/math/normalize.rs), this
// sanitiser and the stylesheet the SPA imports (src/styles/mathml.css).
describe("MathML classes", () => {
  const rustList = (): string[] => {
    const src = readFileSync(`${CRATE}src/math/normalize.rs`, "utf8");
    const block = /pub const MATHML_CLASSES: &\[&str\] = &\[([^\]]*)\];/.exec(src)?.[1] ?? "";
    return [...block.matchAll(/"([^"]+)"/g)].map((m) => m[1] as string);
  };
  const css = () =>
    ["mathmlfixes.css", "mathml.css"]
      .map((f) => readFileSync(`${CRATE}assets/${f}`, "utf8"))
      .join("\n");

  it("are the renderer's list", () => {
    const rust = rustList();
    expect(rust.length).toBeGreaterThan(40);
    expect([...MATHML_CLASSES]).toEqual(rust);
  });

  it("each have a rule in the stylesheet the SPA imports", () => {
    const sheet = css();
    const missing = [...MATHML_CLASSES].filter((c) => !new RegExp(`\\.${c}[\\s.,{]`).test(sheet));
    expect(missing).toEqual([]);
    const entry = readFileSync(resolve(process.cwd(), "src/styles/mathml.css"), "utf8");
    expect(entry).toContain('@import "../../../crates/markdown/assets/mathmlfixes.css";');
    expect(entry).toContain('@import "../../../crates/markdown/assets/mathml.css";');
    expect(sheet).not.toMatch(/style=|!important;/);
  });

  it("cover every class the vectors use", () => {
    const used = new Set<string>();
    for (const v of loadVectors()) {
      for (const m of v.html.matchAll(/<m[a-z]* [^>]*class="([^"]*)"/g)) {
        for (const t of (m[1] as string).split(" ")) used.add(t);
      }
    }
    expect([...used].filter((t) => !MATHML_CLASSES.has(t))).toEqual([]);
    // the corpus exercises every token (testdata/vectors/mathml/gen_mathml.py)
    expect([...MATHML_CLASSES].filter((t) => !used.has(t))).toEqual([]);
  });
});
