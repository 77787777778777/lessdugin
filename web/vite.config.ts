// SPDX-License-Identifier: AGPL-3.0-or-later
import { createHash } from "node:crypto";
import react from "@vitejs/plugin-react";
import { defineConfig, type Plugin } from "vite";

/**
 * Emits `build-info.json` with the SHA-256 of every emitted file and an aggregate hash, so
 * Settings → About can show which build is running (threat-model.md SUP-4). The hash of a
 * file cannot be embedded in that file, hence a separate asset fetched at runtime.
 */
function buildInfo(): Plugin {
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
        source: `${JSON.stringify({ sha256: total.digest("hex"), files }, null, 2)}\n`,
      });
    },
  };
}

export default defineConfig({
  // Relative base so the static build works from any path (static hosts, IPFS, `lessdugin web`).
  base: "./",
  plugins: [react(), buildInfo()],
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
});
