// SPDX-License-Identifier: AGPL-3.0-or-later
// @vitest-environment node
// The release build contains neither the e2e hooks (devnet endpoints, parity time, the debug
// hook) nor the mock engine: three real `vite build`s (release, e2e, demo) into temporary
// directories, scanned by scripts/check-release-bundle.mjs. The e2e and demo builds must show
// every marker, so a marker that no longer matches the code fails here rather than letting the
// release check pass vacuously.

import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, describe, expect, it } from "vitest";
// @ts-expect-error -- a plain .mjs script without type declarations
import { E2E_MARKERS, MOCK_MARKERS, scanDist } from "../../scripts/check-release-bundle.mjs";

type Problem = { file: string; marker: string };

const WEB = fileURLToPath(new URL("../..", import.meta.url));
const VITE = join(WEB, "node_modules", "vite", "bin", "vite.js");
const dirs: string[] = [];

afterAll(() => {
  for (const d of dirs) rmSync(d, { recursive: true, force: true });
});

/** `vite build` in a child process with only the given build flags (the test runner's own
 * NODE_ENV and VITE_* would otherwise make it a development build). */
function build(flags: Record<string, string>): string {
  const out = mkdtempSync(join(tmpdir(), "lessdugin-web-build-"));
  dirs.push(out);
  const env: NodeJS.ProcessEnv = {};
  for (const [k, v] of Object.entries(process.env)) {
    if (!/^(VITE_|VITEST|NODE_ENV$|MODE$|DEV$|PROD$|SSR$|BASE_URL$)/.test(k)) env[k] = v;
  }
  const r = spawnSync(
    process.execPath,
    [VITE, "build", "--outDir", out, "--emptyOutDir", "--logLevel", "error"],
    { cwd: WEB, env: { ...env, ...flags }, encoding: "utf8", timeout: 180_000 },
  );
  if (r.status !== 0) throw new Error(`vite build failed: ${r.stderr || r.stdout}`);
  return out;
}

const markersOf = (problems: Problem[]) => [...new Set(problems.map((p) => p.marker))].sort();

describe.runIf(existsSync(VITE))("release bundle", () => {
  it("has no e2e or mock code; the e2e and demo builds show every marker", () => {
    const release = build({});
    expect(scanDist(release)).toEqual([]);

    const e2e = build({ VITE_E2E: "1" });
    expect(markersOf(scanDist(e2e))).toEqual([...E2E_MARKERS].sort());

    const demo = build({ VITE_DEMO: "1" });
    expect(scanDist(demo, { demo: true })).toEqual([]);
    expect(markersOf(scanDist(demo))).toEqual([...MOCK_MARKERS].sort());
  }, 300_000);
});
