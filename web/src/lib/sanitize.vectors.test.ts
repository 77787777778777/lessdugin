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
import {
  HTML_CLASS_TOKENS,
  HTML_RULES,
  LFM_CONFIG,
  LFM_URI_REGEXP,
  MATHML_ATTRS,
  MATHML_CLASSES,
  sanitizeLfm,
} from "./sanitize.ts";

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

// lfm-1.md §8.5: the DOMPurify configuration is the §8.2/§7.3 allowlist and nothing else, the
// tables `allowlist::dompurify_config_json()` is built from (crates/markdown/src/allowlist.rs,
// src/math/normalize.rs). The vectors above catch a sanitiser that is too strict; these catch one
// that is too loose: a tag, attribute, class token or URI scheme removed from the Rust tables
// must be removed here too (review finding m4-25).
describe("the DOMPurify configuration is the Rust allowlist", () => {
  const allowlist = (): string => readFileSync(`${CRATE}src/allowlist.rs`, "utf8");
  const normalize = (): string => readFileSync(`${CRATE}src/math/normalize.rs`, "utf8");
  const strings = (text: string): string[] =>
    [...text.matchAll(/"([^"]*)"/g)].map((m) => m[1] as string);
  /** A `&[(&str, &[&str])]` table as tag → strings. */
  const pairs = (src: string, name: string): Map<string, string[]> => {
    const start = src.indexOf(`pub const ${name}: &[(&str, &[&str])] = &[`);
    const end = src.indexOf("\n];", start);
    expect(start).toBeGreaterThan(-1);
    const body = src.slice(start, end);
    const out = new Map<string, string[]>();
    for (const m of body.matchAll(/\(\s*"([a-z0-9-]+)",\s*&\[([^\]]*)\],?\s*\)/g)) {
      out.set(m[1] as string, strings(m[2] as string));
    }
    return out;
  };
  const list = (src: string, decl: string): string[] => {
    const start = src.indexOf(decl);
    expect(start).toBeGreaterThan(-1);
    return strings(src.slice(start, src.indexOf("];", start)));
  };
  const sorted = (xs: Iterable<string>): string[] => [...new Set(xs)].sort();

  const html = pairs(allowlist(), "HTML_ALLOWLIST");
  const mathTags = list(normalize(), "const ALLOWED: &[&str] = &[");
  const mathAttrs = list(normalize(), "pub const MATHML_ATTRIBUTES: &[&str] = &[");

  it("allows exactly the Rust tags and attributes", () => {
    expect(html.size).toBeGreaterThan(30);
    expect(sorted(LFM_CONFIG.ALLOWED_TAGS)).toEqual(sorted([...html.keys(), ...mathTags]));
    const attrs = sorted([...[...html.values()].flat(), ...mathAttrs]);
    expect(sorted(LFM_CONFIG.ALLOWED_ATTR)).toEqual(attrs);
    expect(sorted(LFM_CONFIG.ADD_URI_SAFE_ATTR)).toEqual(attrs.filter((a) => a !== "href"));
    expect(sorted(MATHML_ATTRS)).toEqual(sorted(mathAttrs));
  });

  it("gives each HTML element exactly its Rust attributes", () => {
    for (const [tag, attrs] of html) {
      expect({ tag, attrs: sorted(Object.keys(HTML_RULES[tag] ?? {})) }).toEqual({
        tag,
        attrs: sorted(attrs),
      });
    }
    // an element without attributes has no rules
    const withAttrs = [...html].filter(([, attrs]) => attrs.length > 0).map(([tag]) => tag);
    expect(sorted(Object.keys(HTML_RULES))).toEqual(sorted(withAttrs));
  });

  it("allows exactly the Rust class tokens", () => {
    const classes = pairs(allowlist(), "CLASS_ALLOWLIST");
    expect(classes.size).toBeGreaterThan(5);
    expect(
      Object.fromEntries(Object.entries(HTML_CLASS_TOKENS).map(([t, c]) => [t, sorted(c)])),
    ).toEqual(Object.fromEntries([...classes].map(([t, c]) => [t, sorted(c)])));
  });

  it("uses the Rust URI pattern", () => {
    const rust = /pub const ALLOWED_URI_REGEXP: &str = "([^"]*)";/.exec(allowlist())?.[1];
    expect(rust).toBeDefined();
    expect(LFM_URI_REGEXP.source).toBe(new RegExp(rust ?? "").source);
  });
});
