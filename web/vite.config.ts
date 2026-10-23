// SPDX-License-Identifier: AGPL-3.0-or-later
import { createHash } from "node:crypto";
import react from "@vitejs/plugin-react";
import { defineConfig, type Plugin } from "vite";
import { injectMetaCsp } from "./csp.mjs";

/** What kind of build this is, recorded in `build-info.json`: `release` and `pages` are the
 * builds a release `lessdugin` binary may embed (crates/cli/build.rs, `RELEASE_MODES`);
 * `e2e` (`VITE_E2E=1`) and `demo` (`VITE_DEMO=1`, the mock engine) never are (WEB-5, SUP-3). */
export function buildMode(viteMode: string, env: NodeJS.ProcessEnv = process.env): string {
  if (env.VITE_E2E === "1") return "e2e";
  if (env.VITE_DEMO === "1") return "demo";
  if (viteMode === "pages") return "pages";
  return viteMode === "production" ? "release" : viteMode;
}

/**
 * Emits `build-info.json` with the build's `mode` (first), the SHA-256 of every emitted file
 * and an aggregate hash, so Settings → About can show which build is running (threat-model.md
 * SUP-4). The hash of a file cannot be embedded in that file, hence a separate asset fetched
 * at runtime.
 */
function buildInfo(mode: string): Plugin {
  return {
    name: "lessdugin-build-info",
    apply: "build",
    enforce: "post",
    generateBundle(_options, bundle) {
      const total = createHash("sha256");
      const files: Record<string, string> = {};
      for (const name of Object.keys(bundle).sort()) {
        const item = bundle[name];
        if (!item) continue;
        const data = item.type === "chunk" ? item.code : item.source;
        const digest = createHash("sha256").update(data).digest("hex");
        files[name] = digest;
        total.update(`${name}\0${digest}\n`);
      }
      this.emitFile({
        type: "asset",
        fileName: "build-info.json",
        source: `${JSON.stringify({ mode, sha256: total.digest("hex"), files }, null, 2)}\n`,
      });
    },
  };
}

/**
 * GitHub Pages cannot send headers, so `vite build --mode pages` delivers the CSP as the first
 * element of <head> (threat-model.md §6.3; the text comes from csp.mjs, like `_headers`).
 */
function pagesMetaCsp(): Plugin {
  return {
    name: "lessdugin-pages-meta-csp",
    apply: "build",
    transformIndexHtml: { order: "post", handler: (html) => injectMetaCsp(html) },
  };
}

export default defineConfig(({ mode }) => ({
  // Relative base so the static build works from any path (static hosts, IPFS, `lessdugin web`).
  base: "./",
  plugins: [react(), buildInfo(buildMode(mode)), ...(mode === "pages" ? [pagesMetaCsp()] : [])],
  worker: { format: "es" },
  build: {
    target: "es2023",
    sourcemap: true,
    assetsInlineLimit: 0,
    // React, TanStack and DOMPurify (~160 kB gzipped); CodeMirror is split into its own chunk.
    chunkSizeWarningLimit: 600,
  },
  server: { fs: { allow: [".."] } },
  preview: { port: 4173, strictPort: true },
}));
