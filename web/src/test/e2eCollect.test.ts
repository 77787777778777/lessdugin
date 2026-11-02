// SPDX-License-Identifier: AGPL-3.0-or-later
// The mock Playwright suite (e2e/playwright.config.ts) collects exactly the specs directly in
// e2e/. Its subdirectories hold the devnet and cold-start suites, which have their own configs
// and need their own environment: a spec of theirs collected here fails the mock run before any
// test starts (the cold-start spec throws at import without its fixture).

import { spawnSync } from "node:child_process";
import { readdirSync } from "node:fs";
import { createRequire } from "node:module";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

// vitest runs from web/
const WEB = process.cwd();
const E2E = resolve(WEB, "e2e");

interface ListedSuite {
  file: string;
}
interface Listing {
  suites: ListedSuite[];
  errors: { message?: string }[];
}

describe("the mock e2e suite (e2e/playwright.config.ts)", () => {
  it("collects every top-level spec and nothing from a subdirectory", () => {
    const cli = createRequire(import.meta.url).resolve("@playwright/test/cli");
    // `--list` loads every collected file (an import error is a listing error) and runs no
    // test, no web server and no browser.
    const run = spawnSync(
      process.execPath,
      [cli, "test", "-c", "e2e/playwright.config.ts", "--list", "--reporter=json"],
      { cwd: WEB, encoding: "utf8", env: { ...process.env, CI: "1" }, timeout: 60_000 },
    );
    const listing = JSON.parse(run.stdout) as Listing;
    expect(listing.errors.map((e) => e.message ?? "")).toEqual([]);
    expect(run.status).toBe(0);
    const ownSpecs = readdirSync(E2E, { withFileTypes: true })
      .filter((e) => e.isFile() && e.name.endsWith(".spec.ts"))
      .map((e) => e.name)
      .sort();
    // the suite's files, relative to its testDir (e2e/): a subdirectory's would have a `/`
    const collected = listing.suites.map((s) => s.file).sort();
    expect(collected).toEqual(ownSpecs);
    expect(ownSpecs.length).toBeGreaterThan(3);
  }, 90_000);
});
