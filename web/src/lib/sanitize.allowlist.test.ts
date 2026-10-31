// SPDX-License-Identifier: AGPL-3.0-or-later
// @vitest-environment jsdom
//
// lfm-1.md §8.5: the DOMPurify configuration is the §8.2/§7.3 allowlist and contains nothing
// else (review finding m4-25). The vectors (sanitize.vectors.test.ts) catch a sanitiser that is
// too strict; these catch one that is too loose. Tags, attributes, class tokens, the URI
// pattern and the options are compared with the Rust tables `allowlist::dompurify_config_json()`
// is built from. The value grammars, which DOMPurify enforces through hooks, are probed
// against the patterns of the §8.2 table itself.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import {
  attributeAllowed,
  HTML_CLASS_TOKENS,
  HTML_RULES,
  LFM_CONFIG,
  LFM_URI_REGEXP,
  MATHML_ATTRS,
} from "./sanitize.ts";

// vitest runs from web/ (jsdom's import.meta.url is not a file: URL).
const CRATE = `${resolve(process.cwd(), "../crates/markdown")}/`;
const SPEC = resolve(process.cwd(), "../docs/spec/lfm-1.md");

// The tables of crates/markdown/src/allowlist.rs and src/math/normalize.rs: a tag, attribute,
// class token, URI scheme or option changed there must change here too, in either direction.
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

  it("sets exactly the options the Rust configuration sets, to its values", () => {
    const src = allowlist();
    const start = src.indexOf("pub fn dompurify_config_json() -> String {");
    expect(start).toBeGreaterThan(-1);
    const body = src.slice(start, src.indexOf("\n}\n", start));
    // the JSON keys of the format string, and the ones it fixes to a boolean
    const keys = [...body.matchAll(/\\"([A-Z_]+)\\":/g)].map((m) => m[1] as string);
    const flags = new Map(
      [...body.matchAll(/\\"([A-Z_]+)\\":(true|false)/g)].map((m) => [
        m[1] as string,
        m[2] === "true",
      ]),
    );
    expect(sorted(flags.keys())).toEqual(["ALLOW_ARIA_ATTR", "ALLOW_DATA_ATTR", "SAFE_FOR_XML"]);
    const config = LFM_CONFIG as Record<string, unknown>;
    for (const [k, v] of flags) expect({ k, v: config[k] }).toEqual({ k, v });
    // `LFM_*` are the class tables the hooks enforce; `RETURN_TRUSTED_TYPE` picks the return
    // type of `sanitize`, not what it keeps
    expect(sorted(Object.keys(config).filter((k) => k !== "RETURN_TRUSTED_TYPE"))).toEqual(
      sorted(keys.filter((k) => !k.startsWith("LFM_"))),
    );
  });
});

// The §8.2 value grammars. Each pattern is quoted from the table (the test checks it is still
// there); every probe must get the same verdict from the sanitiser as from the pattern. The
// probes sit on each grammar's edges: lengths, leading zeros, case, other characters.
describe("the value grammars are the §8.2 patterns", () => {
  const spec = readFileSync(SPEC, "utf8");
  const table = spec.slice(spec.indexOf("### 8.2 Allowlist"), spec.indexOf("### 8.3"));
  const MATHML_NS = "http://www.w3.org/1998/Math/MathML";

  interface Grammar {
    /** The §8.2 cell's pattern, as written there. */
    pattern: string;
    tag: string;
    attr: string;
    parent?: string;
    mathml?: boolean;
    probes: string[];
  }
  const a512 = "a".repeat(512);
  const GRAMMARS: Grammar[] = [
    {
      pattern: "language-[a-z0-9_+-]{1,32}",
      tag: "code",
      parent: "pre",
      attr: "class",
      probes: [
        "language-rust",
        "language-c++",
        "language-objective_c-2",
        "language-",
        `language-${"a".repeat(32)}`,
        `language-${"a".repeat(33)}`,
        "language-Rust",
        "language-c#",
        "language-a b",
        "language-rust ",
        "lang-rust",
        "LANGUAGE-rust",
      ],
    },
    {
      pattern: "md-h-[\\p{L}\\p{N}_-]*",
      tag: "h2",
      attr: "id",
      probes: [
        "md-h-intro",
        "md-h-",
        "md-h-über_1-x",
        "md-h-a b",
        "md-h-a.b",
        "h-intro",
        "md-h-a\u200b",
      ],
    },
    {
      pattern: "md-fn-[1-9][0-9]*",
      tag: "li",
      attr: "id",
      probes: ["md-fn-1", "md-fn-10", "md-fn-0", "md-fn-01", "md-fn-", "md-fn-1a", "md-fn-\u0661"],
    },
    {
      pattern: "md-fnref-[1-9][0-9]*-[1-9][0-9]*",
      tag: "sup",
      attr: "id",
      probes: [
        "md-fnref-1-1",
        "md-fnref-12-3",
        "md-fnref-1-0",
        "md-fnref-0-1",
        "md-fnref-1",
        "md-fnref-1-1-1",
      ],
    },
    {
      pattern: "[1-9][0-9]{0,9}",
      tag: "ol",
      attr: "start",
      probes: ["1", "1234567890", "12345678901", "01", "-1", "", "1e3"],
    },
    {
      pattern: "[1-9][0-9]{0,7}",
      tag: "img",
      attr: "width",
      probes: ["1", "12345678", "123456789", "0", "08", "1.5", " 1"],
    },
    {
      pattern: "b[a-z2-7]{1,127}",
      tag: "img",
      attr: "data-cid",
      probes: ["bafy", "b", "B", "bafy1", `b${"a".repeat(127)}`, `b${"a".repeat(128)}`, "zafy"],
    },
    {
      pattern: "[\\p{L}\\p{N}_-]{1,512}",
      tag: "a",
      attr: "data-anchor",
      probes: ["intro", "md-fn-1", "", a512, `${a512}a`, "a b", "a#b", "ü".repeat(512)],
    },
  ];

  const element = (g: Grammar): Element => {
    if (g.mathml) return document.createElementNS(MATHML_NS, g.tag);
    const el = document.createElement(g.tag);
    if (g.parent) document.createElement(g.parent).append(el);
    return el;
  };

  for (const g of GRAMMARS) {
    it(`${g.tag} ${g.attr}: ${g.pattern}`, () => {
      expect(table).toContain(`\`${g.pattern}\``);
      const re = new RegExp(`^(?:${g.pattern})$`, "u");
      const el = element(g);
      const verdicts = g.probes.map((v) => ({ v, allowed: attributeAllowed(el, g.attr, v) }));
      expect(verdicts).toEqual(g.probes.map((v) => ({ v, allowed: re.test(v) })));
    });
  }

  it("code: language-* only as the child of pre, math-error anywhere", () => {
    expect(table).toContain("(only as the child of `pre`) or `math-error`");
    const code = document.createElement("code");
    document.createElement("p").append(code);
    expect(attributeAllowed(code, "class", "language-rust")).toBe(false);
    expect(attributeAllowed(code, "class", "math-error")).toBe(true);
  });

  it("ol start also takes 0", () => {
    expect(table).toContain("`0` or `[1-9][0-9]{0,9}`");
    expect(attributeAllowed(document.createElement("ol"), "start", "0")).toBe(true);
  });

  it("mathcolor: # and six lower-case hexadecimal digits", () => {
    expect(table).toContain("`mathcolor` (`#` and six lower-case hexadecimal digits)");
    const mi = document.createElementNS(MATHML_NS, "mi");
    const probes = ["#00ff00", "#00FF00", "#0f0", "#00ff00 ", "#00ff0g", "red", "00ff00"];
    expect(probes.map((v) => attributeAllowed(mi, "mathcolor", v))).toEqual(
      probes.map((v) => /^#[0-9a-f]{6}$/.test(v)),
    );
  });

  it("dir: auto only", () => {
    const p = document.createElement("p");
    expect(["auto", "ltr", "rtl", "Auto", ""].map((v) => attributeAllowed(p, "dir", v))).toEqual([
      true,
      false,
      false,
      false,
      false,
    ]);
  });
});
