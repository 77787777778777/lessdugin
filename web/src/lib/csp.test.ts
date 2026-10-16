// SPDX-License-Identifier: AGPL-3.0-or-later
// @vitest-environment node
//
// threat-model.md §10 "CSP and Trusted Types": the exact header text in deploy/site/_headers,
// and the GitHub Pages <meta> fallback (§6.3), both against the §6.1 text of the spec.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { DEV_CSP, injectMetaCsp, META_CSP, PRODUCTION_CSP } from "../../csp.mjs";
import { parseHeadersFile } from "../../e2e/serve.mjs";

const REPO = resolve(process.cwd(), "..");
const read = (p: string): string => readFileSync(resolve(REPO, p), "utf8");

/** The first fenced code block after `heading`, its lines joined by single spaces (§6.1:
 * "this text on one line, with each line break above replaced by a single space"). */
function specPolicy(heading: string): string {
  const md = read("docs/spec/threat-model.md");
  const at = md.indexOf(heading);
  expect(at).toBeGreaterThan(-1);
  const block = /```\n([\s\S]*?)\n```/.exec(md.slice(at));
  if (!block?.[1]) throw new Error(`no code block after ${heading}`);
  return block[1].split("\n").join(" ");
}

describe("Content Security Policy", () => {
  it("is exactly the threat-model §6.1 production policy", () => {
    expect(PRODUCTION_CSP).toBe(specPolicy("### 6.1 Production policy"));
  });

  it("is sent verbatim by deploy/site/_headers on every path", () => {
    const rules = parseHeadersFile(read("deploy/site/_headers"));
    expect(rules["/*"]?.["Content-Security-Policy"]).toBe(PRODUCTION_CSP);
    expect(rules["/*"]?.["Referrer-Policy"]).toBe("no-referrer");
    expect(rules["/*"]?.["X-Content-Type-Options"]).toBe("nosniff");
    expect(rules["/*"]?.["Cross-Origin-Opener-Policy"]).toBe("same-origin");
    expect(rules["/*"]?.["Permissions-Policy"]).toBe(
      "camera=(), microphone=(), geolocation=(), payment=()",
    );
  });

  it("marks index.html and the config files no-cache (§6.3)", () => {
    const rules = parseHeadersFile(read("deploy/site/_headers"));
    for (const path of ["/", "/index.html", "/lessdugin-config.json", "/oauth/*"]) {
      expect(rules[path]?.["Cache-Control"]).toBe("no-cache");
    }
  });

  it("has the §6.2 development variant", () => {
    expect(DEV_CSP).toBe(
      PRODUCTION_CSP.replace(
        "connect-src 'self' https: wss:",
        "connect-src 'self' https: wss: http: ws:",
      ),
    );
  });

  it("is the first element of <head> in GitHub Pages builds, minus frame-ancestors (§6.3)", () => {
    expect(META_CSP).toBe(PRODUCTION_CSP.replace(" frame-ancestors 'none';", ""));
    const html = injectMetaCsp(read("web/index.html"));
    const head = /<head>\s*(<[^>]+>)/.exec(html)?.[1];
    expect(head).toBe(`<meta http-equiv="Content-Security-Policy" content="${META_CSP}" />`);
    expect(html.match(/http-equiv="Content-Security-Policy"/g)).toHaveLength(1);
  });
});
