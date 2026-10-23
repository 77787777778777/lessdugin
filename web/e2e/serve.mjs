// SPDX-License-Identifier: AGPL-3.0-or-later
// A tiny static server for the e2e tests: serves web/dist with the production response headers
// taken from deploy/site/_headers (the `/*` block), so CSP and Trusted Types are enforced
// exactly as on the hosted site. `dev: true` swaps in the §6.2 development policy (the devnet
// e2e harness: `connect-src` also allows `http:` and `ws:`), everything else unchanged.

import { readFile, stat } from "node:fs/promises";
import { createServer } from "node:http";
import { extname, join, normalize, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { DEV_CSP } from "../csp.mjs";

const here = fileURLToPath(new URL(".", import.meta.url));
export const DIST = resolve(here, "../dist");
/** The e2e build (`VITE_E2E=1 vite build --outDir dist-e2e`). */
export const DIST_E2E = resolve(here, "../dist-e2e");
export const HEADERS_FILE = resolve(here, "../../deploy/site/_headers");

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json",
  ".map": "application/json",
  ".wasm": "application/wasm",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".ico": "image/x-icon",
  ".txt": "text/plain; charset=utf-8",
};

/** Parses a Netlify/Cloudflare `_headers` file into `{ pattern: { name: value } }`. */
export function parseHeadersFile(text) {
  const rules = {};
  let current = null;
  for (const raw of text.split("\n")) {
    if (raw.trim() === "" || raw.trimStart().startsWith("#")) continue;
    if (!/^\s/.test(raw)) {
      current = raw.trim();
      rules[current] = rules[current] ?? {};
      continue;
    }
    const i = raw.indexOf(":");
    if (current && i > 0) rules[current][raw.slice(0, i).trim()] = raw.slice(i + 1).trim();
  }
  return rules;
}

export async function productionHeaders() {
  const rules = parseHeadersFile(await readFile(HEADERS_FILE, "utf8"));
  const all = rules["/*"];
  if (!all?.["Content-Security-Policy"]) throw new Error(`no CSP for /* in ${HEADERS_FILE}`);
  return all;
}

/** Starts the server on 127.0.0.1:`port` (0 = any free port). Resolves to `{ url, close }`. */
export async function startStaticServer({ port = 0, root = DIST, dev = false } = {}) {
  const headers = { ...(await productionHeaders()) };
  if (dev) headers["Content-Security-Policy"] = DEV_CSP;
  const server = createServer(async (req, res) => {
    try {
      const path = decodeURIComponent(new URL(req.url ?? "/", "http://x").pathname);
      let file = normalize(join(root, path));
      if (!file.startsWith(root + sep) && file !== root) {
        res.writeHead(403).end();
        return;
      }
      if ((await stat(file).catch(() => null))?.isDirectory()) file = join(file, "index.html");
      const body = await readFile(file);
      res.writeHead(200, {
        ...headers,
        "Content-Type": MIME[extname(file)] ?? "application/octet-stream",
        "Cache-Control": "no-cache",
      });
      res.end(body);
    } catch {
      res.writeHead(404, headers).end("not found");
    }
  });
  await new Promise((ok) => server.listen(port, "127.0.0.1", ok));
  const address = server.address();
  const url = `http://127.0.0.1:${typeof address === "object" && address ? address.port : port}`;
  return { url, headers, close: () => new Promise((ok) => server.close(() => ok())) };
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const port = Number(process.argv[2] ?? 4174);
  startStaticServer({ port }).then(({ url }) => console.log(`serving ${DIST} at ${url}`));
}
