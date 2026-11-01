// SPDX-License-Identifier: AGPL-3.0-or-later
// Focused M6 acceptance configuration. It requires the signed fixture runner, not the ordinary
// three-account devnet, and never starts production or external services.

import { fileURLToPath } from "node:url";
import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: ".",
  testMatch: /cold-start\.spec\.ts$/,
  globalSetup: fileURLToPath(new URL("./cold-start.setup.ts", import.meta.url)),
  fullyParallel: false,
  workers: 1,
  timeout: 120_000,
  expect: { timeout: 12_000 },
  reporter: [["list"]],
  outputDir: fileURLToPath(new URL("../../../test-results/m6-cold-start", import.meta.url)),
  use: {
    trace: "retain-on-failure",
    // Some Nix builders cannot create the conventional /opt/pw-browsers symlink. Keep that
    // PLAYWRIGHT_BROWSERS_PATH contract while selecting the same pinned 1.56.1 browser directly.
    launchOptions: process.env.LESSDUGIN_CHROMIUM
      ? { executablePath: process.env.LESSDUGIN_CHROMIUM }
      : {},
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"], viewport: { width: 1400, height: 1000 } },
    },
  ],
});
