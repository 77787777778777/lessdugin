// SPDX-License-Identifier: AGPL-3.0-or-later
// End-to-end tests of the built SPA against the mock engine (`?mock`). The app is served by
// `vite preview`; the CSP test starts its own static server with the production headers.
// Uses the preinstalled Chromium (PLAYWRIGHT_BROWSERS_PATH, see the web `e2e` script).

import { fileURLToPath } from "node:url";
import { defineConfig, devices } from "@playwright/test";

const webDir = fileURLToPath(new URL("..", import.meta.url));
const e2eDir = fileURLToPath(new URL(".", import.meta.url));
const PORT = 4173;

/** The mock suite is the specs directly in e2e/. Every subdirectory holds a suite with its own
 * config and environment (devnet/, cold-start/), so none of their files is collected here
 * (checked by src/test/e2eCollect.test.ts). Playwright matches against the absolute path. */
export const MOCK_SPECS = new RegExp(
  `^${e2eDir.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}[^/]+\\.spec\\.ts$`,
);

export default defineConfig({
  testDir: ".",
  testMatch: MOCK_SPECS,
  fullyParallel: false,
  workers: 1,
  timeout: 90_000,
  expect: { timeout: 10_000 },
  reporter: [["list"]],
  outputDir: fileURLToPath(new URL("../../test-results/web-e2e", import.meta.url)),
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    trace: "retain-on-failure",
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"], viewport: { width: 1400, height: 1000 } },
    },
  ],
  webServer: {
    command: `pnpm exec vite preview --host 127.0.0.1 --port ${PORT} --strictPort`,
    cwd: webDir,
    url: `http://127.0.0.1:${PORT}/`,
    reuseExistingServer: false,
    timeout: 60_000,
  },
});
