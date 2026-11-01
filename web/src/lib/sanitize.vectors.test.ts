// SPDX-License-Identifier: AGPL-3.0-or-later
// @vitest-environment jsdom
//
// SPA-3 / lfm-1.md §8.5: the sanitiser leaves every renderer output unchanged. The outputs are
// every `expected.html` / `expected.html_temml` in the lfm1, xss and mathml vectors, and the HTML
// the real renderer gives for the input of every render and terminal vector, including the ones
// whose expected values are only sizes, heads, tails, text or styled output: the release engine
// wasm (packages/engine/pkg, `cargo xtask wasm`) renders each input through its `render`
// operation, with the input's media items (`defs#media` values) and the body's `profile`, as a
// stored record is rendered. Only an input that is an encoding error (not a string) is left to
// its blessed html. That part is skipped locally without pkg/ and required under CI (or with
// LESSDUGIN_WASM_TEST=1), as in the engine package's tests.
//
// jsdom's serialiser differs from the pinned browser's in one respect: it does not escape `<` and
// `>` inside attribute values. So this runner asserts `sanitizeLfm(html) === html` with that one
// difference applied to `html` (`jsdomForm`); anything else DOMPurify or the parser changes fails.
// e2e/vectors.spec.ts checks the expected strings byte for byte in Chromium.

import { existsSync, readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { describe, expect, it } from "vitest";
import { MATHML_CLASSES, sanitizeLfm } from "./sanitize.ts";

// vitest runs from web/ (jsdom's import.meta.url is not a file: URL).
const VECTORS = `${resolve(process.cwd(), "../testdata/vectors")}/`;
const CRATE = `${resolve(process.cwd(), "../crates/markdown")}/`;
const PKG = `${resolve(process.cwd(), "../packages/engine/pkg")}/`;
const WASM = `${PKG}lessdugin_engine_bg.wasm`;
const wasmPresent = existsSync(WASM);
const wasmRequired = process.env.CI !== undefined || process.env.LESSDUGIN_WASM_TEST === "1";

interface Vector {
  id: string;
  html: string;
}

/** A vector's render input (`crates/markdown/tests/vectors.rs` `render_input`). */
interface RenderInput {
  text?: string;
  gen?: { prefix?: string; repeat?: string; times?: number; suffix?: string };
  media?: unknown[];
  profile?: string;
  encoding_error?: boolean;
}

interface VectorFile {
  kind?: string;
  vectors?: {
    name: string;
    kind?: string;
    input?: RenderInput;
    expected?: Record<string, unknown>;
  }[];
}

function vectorFiles(dirs: string[]): { id: string; json: VectorFile }[] {
  const out: { id: string; json: VectorFile }[] = [];
  for (const dir of dirs) {
    for (const file of readdirSync(`${VECTORS}${dir}`).filter((f) => f.endsWith(".json"))) {
      const json = JSON.parse(readFileSync(`${VECTORS}${dir}/${file}`, "utf8")) as VectorFile;
      out.push({ id: `${dir}/${file}`, json });
    }
  }
  return out;
}

/** Every expected renderer output (`expected.html` and `expected.html_temml`). */
function loadVectors(): Vector[] {
  const out: Vector[] = [];
  for (const { id, json } of vectorFiles(["lfm1", "xss", "mathml"])) {
    for (const v of json.vectors ?? []) {
      for (const key of ["html", "html_temml"]) {
        const html = v.expected?.[key];
        if (typeof html === "string") out.push({ id: `${id}#${v.name}:${key}`, html });
      }
    }
  }
  return out;
}

/** The text of a render input: `text`, or `gen` = prefix ‖ repeat × times ‖ suffix. */
function inputText(input: RenderInput): string {
  if (typeof input.text === "string") return input.text;
  const g = input.gen ?? {};
  return `${g.prefix ?? ""}${(g.repeat ?? "").repeat(g.times ?? 1)}${g.suffix ?? ""}`;
}

interface RenderCase {
  id: string;
  text: string;
  /** The input's media items, as a record stores them (`defs#media` values). */
  media: unknown[];
  /** The body's `profile`, when the input names one. */
  profile?: string;
  /** `expected.html`, when the vector has one. */
  expected?: string;
}

/** The render and terminal vectors the `render` operation takes (a markdown string with its
 * media items and profile), and the ids of the ones it cannot: an encoding error is bytes that
 * are not a string, so only its blessed html is sanitised (`loadVectors`). */
function renderInputs(): { take: RenderCase[]; encodingErrors: string[] } {
  const take: RenderCase[] = [];
  const encodingErrors: string[] = [];
  for (const { id, json } of vectorFiles(["lfm1", "xss", "terminal"])) {
    for (const v of json.vectors ?? []) {
      const kind = v.kind ?? json.kind;
      if ((kind !== "render" && kind !== "terminal") || !v.input) continue;
      const i = v.input;
      if (i.encoding_error) {
        encodingErrors.push(`${id}#${v.name}`);
        continue;
      }
      const html = v.expected?.html;
      take.push({
        id: `${id}#${v.name}`,
        text: inputText(i),
        media: i.media ?? [],
        ...(i.profile !== undefined ? { profile: i.profile } : {}),
        ...(typeof html === "string" ? { expected: html } : {}),
      });
    }
  }
  return { take, encodingErrors };
}

/** `html` as jsdom serialises it: `<` and `>` unescaped inside attribute values (the renderer, and
 * the pinned browser, write `&lt;` and `&gt;` there). */
function jsdomForm(html: string): string {
  return html.replace(
    /(<[a-zA-Z][^\s/>]*)((?:\s+[^\s=/>]+(?:="[^"]*")?)*)(\s*\/?>)/g,
    (_, tag, attrs, end) => {
      const a = (attrs as string).replace(
        /="([^"]*)"/g,
        (_m: string, v: string) => `="${v.replaceAll("&lt;", "<").replaceAll("&gt;", ">")}"`,
      );
      return `${tag}${a}${end}`;
    },
  );
}

/** Above this many elements an output is not sanitised in jsdom (see the render test). */
const MAX_JSDOM_ELEMENTS = 200_000;

/** The number of start tags in `html` (an upper bound on its elements). */
function elements(html: string): number {
  let n = 0;
  for (let i = html.indexOf("<"); i !== -1; i = html.indexOf("<", i + 1)) {
    if (html.charCodeAt(i + 1) !== 47) n++;
  }
  return n;
}

/** The sanitiser's changes to `vs`, as ids. */
function changedBySanitiser(vs: Vector[]): string[] {
  return vs.filter((v) => sanitizeLfm(v.html) !== jsdomForm(v.html)).map((v) => v.id);
}

interface RenderEngine {
  call(op: string, argsJson: string): Promise<string>;
}

/** The release engine. `render` is a pure operation, so the host has only what constructing
 * the engine reads (its random seed). */
async function engine(): Promise<RenderEngine> {
  const m = (await import(/* @vite-ignore */ pathToFileURL(`${PKG}lessdugin_engine.js`).href)) as {
    initSync(input: { module: BufferSource }): unknown;
    WasmEngine: new (configJson: string, host: object) => RenderEngine;
  };
  m.initSync({ module: readFileSync(WASM) });
  return new m.WasmEngine("{}", {
    now: () => Date.now(),
    random: (n: number) => crypto.getRandomValues(new Uint8Array(n)),
  });
}

describe("LFM vectors through the sanitiser (jsdom)", () => {
  const vectors = loadVectors();

  it("covers the lfm1, xss and mathml corpora", () => {
    expect(vectors.length).toBeGreaterThan(300);
  });

  it("changes no expected renderer output", () => {
    expect(changedBySanitiser(vectors)).toEqual([]);
  });

  it("compares with the renderer's own bytes, not with a re-serialisation", () => {
    // an output the parser would rewrite (here an unquoted attribute) is a change, even when
    // DOMPurify keeps everything in it
    const unquoted = '<div class="lfm" dir=auto><p>a</p></div>';
    expect(changedBySanitiser([{ id: "unquoted", html: unquoted }])).toEqual(["unquoted"]);
    // the one serialiser difference allowed: `<` and `>` inside attribute values
    const v = vectors.find((x) => x.id === "lfm1/serialise.json#attribute-escaping:html");
    expect(v !== undefined && jsdomForm(v.html) !== v.html).toBe(true);
  });

  it.skipIf(!wasmPresent && !wasmRequired)(
    "changes no output of the real renderer for any render or terminal vector input",
    async () => {
      expect(wasmPresent, `${WASM} is missing: run \`cargo xtask wasm\``).toBe(true);
      const e = await engine();
      const { take, encodingErrors } = renderInputs();
      expect(take.length).toBeGreaterThan(500);
      // every render input is taken, media items and profiles included; the one input that is
      // not a string is left to its blessed html, which `loadVectors` sanitises
      expect(take.some((v) => v.media.length > 0)).toBe(true);
      expect(take.some((v) => v.profile !== undefined)).toBe(true);
      expect(encodingErrors).toEqual(["lfm1/limits.json#encoding-error"]);
      for (const id of encodingErrors) {
        expect(
          vectors.some((v) => v.id === `${id}:html`),
          id,
        ).toBe(true);
      }
      const rendered: Vector[] = [];
      const notBlessed: string[] = [];
      const tooLarge: string[] = [];
      for (const v of take) {
        // `[markdown, media, opts]`, as the engine renders a stored body (crates/wasm ops.rs)
        const args = [v.text, v.media, v.profile !== undefined ? { profile: v.profile } : null];
        const body = JSON.parse(await e.call("render", JSON.stringify(args))) as {
          html: string;
        };
        if (v.expected !== undefined && body.html !== v.expected) notBlessed.push(v.id);
        if (elements(body.html) > MAX_JSDOM_ELEMENTS) tooLarge.push(v.id);
        else rendered.push({ id: v.id, html: body.html });
      }
      // it is the renderer the vectors bless (native = wasm, crates/markdown/tests/vectors.rs)
      expect(notBlessed).toEqual([]);
      // the output-budget cases (7.7 to 16.8 MB, 700,000 elements and more) need several GB of
      // jsdom heap each: crates/markdown/tests/vectors.rs checks them (check_html, ammonia)
      expect(
        tooLarge.every((id) => /budget|event-cap/.test(id)),
        tooLarge.join(", "),
      ).toBe(true);
      expect(tooLarge.length).toBeLessThan(8);
      // the vectors without an expected html are among them, the media and profile ones too
      expect(rendered.some((r) => r.id.startsWith("lfm1/limits.json#"))).toBe(true);
      expect(rendered.some((r) => r.id.startsWith("terminal/escapes.json#"))).toBe(true);
      for (const id of [
        "lfm1/images.json#media-alt-amplification-bounded",
        "lfm1/images.json#media-alt-amplification-video-bounded",
        "lfm1/gallery.json#gallery-styled",
        "lfm1/limits.json#flattened-keeps-footnote-reference-and-image",
        "lfm1/limits.json#lfm1-profile",
      ]) {
        expect(
          rendered.some((r) => r.id === id),
          id,
        ).toBe(true);
      }
      expect(changedBySanitiser(rendered)).toEqual([]);
    },
    120_000,
  );
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
