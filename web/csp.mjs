// SPDX-License-Identifier: AGPL-3.0-or-later
// The SPA's Content Security Policy, from one source (docs/spec/threat-model.md §6):
// - PRODUCTION_CSP is the §6.1 header text, sent by `deploy/site/_headers` (Cloudflare Pages,
//   Netlify) and by `lessdugin web`; src/lib/csp.test.ts asserts it equals the spec text and
//   the `_headers` file exactly;
// - DEV_CSP is the §6.2 development policy (devnet e2e), never served by release builds;
// - META_CSP is the GitHub Pages fallback of §6.3: the §6.1 text minus `frame-ancestors`
//   (ignored in a <meta> policy), injected as the first element of <head> by the `pages`
//   build mode (vite.config.ts, `injectMetaCsp`).

export const PRODUCTION_CSP_DIRECTIVES = Object.freeze([
  "default-src 'none'",
  "script-src 'self' 'wasm-unsafe-eval'",
  "worker-src 'self'",
  "style-src 'self'",
  "font-src 'self'",
  "img-src 'self' blob:",
  "media-src blob:",
  "connect-src 'self' https: wss:",
  "object-src 'none'",
  "base-uri 'none'",
  "form-action 'none'",
  "frame-ancestors 'none'",
  "require-trusted-types-for 'script'",
  "trusted-types lessdugin-md lessdugin-worker dompurify",
]);

export const PRODUCTION_CSP = PRODUCTION_CSP_DIRECTIVES.join("; ");

export const DEV_CSP = PRODUCTION_CSP_DIRECTIVES.map((d) =>
  d.startsWith("connect-src ") ? "connect-src 'self' https: wss: http: ws:" : d,
).join("; ");

export const META_CSP = PRODUCTION_CSP_DIRECTIVES.filter(
  (d) => !d.startsWith("frame-ancestors "),
).join("; ");

/** `index.html` with the META_CSP <meta> as the first child of <head> (GitHub Pages builds). */
export function injectMetaCsp(html) {
  const head = /<head(\s[^>]*)?>/i.exec(html);
  if (!head) throw new Error("injectMetaCsp: index.html has no <head>");
  const content = META_CSP.replaceAll("&", "&amp;").replaceAll('"', "&quot;");
  const meta = `<meta http-equiv="Content-Security-Policy" content="${content}" />`;
  const at = head.index + head[0].length;
  return `${html.slice(0, at)}\n    ${meta}${html.slice(at)}`;
}
