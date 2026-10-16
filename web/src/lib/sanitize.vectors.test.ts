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
import { sanitizeLfm } from "./sanitize.ts";

// vitest runs from web/ (jsdom's import.meta.url is not a file: URL).
const VECTORS = `${resolve(process.cwd(), "../testdata/vectors")}/`;

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
