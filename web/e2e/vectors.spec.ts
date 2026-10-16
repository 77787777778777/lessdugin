// SPDX-License-Identifier: AGPL-3.0-or-later
// SPA-3 / lfm-1.md §8.3, §8.5: `DOMPurify.sanitize(html, LFM_CONFIG) === html` for every
// renderer output in the shared vectors (lfm1, xss, mathml: `expected.html` and `html_temml`),
// in the pinned browser. jsdom serialises attribute values differently (it does not escape `<`
// and `>`), so this exact-string check runs here; src/lib/sanitize.vectors.test.ts is the jsdom
// counterpart. The sanitiser module is bundled on its own (the app does not expose it).

import { readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import { build } from "vite";

const webDir = fileURLToPath(new URL("..", import.meta.url));
const VECTORS = resolve(webDir, "../testdata/vectors");
const OUT = fileURLToPath(new URL("../../test-results/web-e2e-sanitizer", import.meta.url));

interface Vector {
  id: string;
  html: string;
}

function loadVectors(): Vector[] {
  const out: Vector[] = [];
  for (const dir of ["lfm1", "xss", "mathml"]) {
    for (const file of readdirSync(resolve(VECTORS, dir)).filter((f) => f.endsWith(".json"))) {
      const json = JSON.parse(readFileSync(resolve(VECTORS, dir, file), "utf8")) as {
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

test.beforeAll(async () => {
  await build({
    configFile: false,
    root: webDir,
    logLevel: "warn",
    build: {
      outDir: OUT,
      emptyOutDir: true,
      minify: false,
      write: true,
      lib: {
        entry: resolve(webDir, "src/lib/sanitize.ts"),
        formats: ["iife"],
        name: "LessduginSanitize",
        fileName: () => "sanitize.js",
      },
    },
  });
});

test("the sanitiser leaves every vector's renderer output byte-identical", async ({ page }) => {
  const vectors = loadVectors();
  expect(vectors.length).toBeGreaterThan(300);
  await page.setContent("<!doctype html><html><head></head><body></body></html>");
  await page.addScriptTag({ path: resolve(OUT, "sanitize.js") });
  const changed = await page.evaluate((vs) => {
    const s = (window as unknown as { LessduginSanitize: { sanitizeLfm: (h: string) => string } })
      .LessduginSanitize;
    return vs
      .filter((v) => s.sanitizeLfm(v.html) !== v.html)
      .map((v) => ({ id: v.id, out: s.sanitizeLfm(v.html) }));
  }, vectors);
  expect(changed).toEqual([]);
});
