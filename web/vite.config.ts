// SPDX-License-Identifier: AGPL-3.0-or-later
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
  // Relative base so the static build works from any path (static hosts, IPFS, `lessdugin web`).
  base: "./",
  plugins: [react()],
  worker: { format: "es" },
  build: {
    target: "es2023",
    sourcemap: true,
    assetsInlineLimit: 0,
  },
  server: { fs: { allow: [".."] } },
});
