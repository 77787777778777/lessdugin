#!/usr/bin/env node
// SPDX-License-Identifier: AGPL-3.0-or-later
//
// The release bundle carries no test-only code (threat-model.md SUP-3, SPA-1): no e2e hook or
// devnet switch (src/host/e2e.ts, built only with VITE_E2E=1) and no mock engine with its
// fixture world and fake sign-in (built only in development and demo builds). Vite replaces the
// build flags with literals and drops the dead branches; this check reads the emitted files and
// fails if any marker string of those modules survived, so a refactor that defeats the
// dead-code elimination cannot ship unnoticed.
//
//   node scripts/check-release-bundle.mjs [dist] [--demo]
//
// `--demo` checks a demo build (VITE_DEMO=1), which includes the mock on purpose: only the e2e
// markers count there. Source maps are skipped: they hold the original text of every module
// that went in, dead branches included (the e2e module itself never goes in, so its markers
// would still show up in a map only if the module were bundled).

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

/** Strings found only in src/host/e2e.ts and the e2e branch of src/engine/real.ts. */
export const E2E_MARKERS = ["__lessduginE2E", "__LESSDUGIN_E2E__", "End-to-end test build"];

/** Strings found only in the mock engine (packages/engine/src/mock*) and src/engine/mock.ts. */
export const MOCK_MARKERS = [
  "Demo mode: the in-memory mock engine",
  "Mock sign-in:",
  "resolved from the mock directory",
  "pds.example.test",
];

/** The `build-info.json` modes a release binary may embed (crates/cli/release_markers.rs
 * `RELEASE_MODES`; vite.config.ts `buildMode`). */
export const RELEASE_MODES = ["release", "production", "pages"];

const SCANNED = /\.(js|mjs|html|json|css|wasm|txt)$/;

function files(dir) {
  const out = [];
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) out.push(...files(p));
    else if (SCANNED.test(name)) out.push(p);
  }
  return out;
}

/**
 * Problems in the build at `dir`: `{ file, marker }` for every marker string found in an
 * emitted file (source maps aside), and for a `build-info.json` whose `mode` is not a release
 * mode (`demo` for `--demo`).
 */
export function scanDist(dir, { demo = false } = {}) {
  const markers = demo ? E2E_MARKERS : [...E2E_MARKERS, ...MOCK_MARKERS];
  const problems = [];
  const list = files(dir);
  if (!list.some((f) => f.endsWith(`${sep}index.html`) || f.endsWith("/index.html"))) {
    problems.push({ file: dir, marker: "(no index.html: not a build)" });
  }
  const info = list.find((f) => relative(dir, f) === "build-info.json");
  if (info) {
    let mode;
    try {
      mode = JSON.parse(readFileSync(info, "utf8")).mode;
    } catch {
      mode = "(unreadable)";
    }
    const ok = demo ? mode === "demo" : RELEASE_MODES.includes(mode);
    if (!ok) problems.push({ file: "build-info.json", marker: `mode ${JSON.stringify(mode)}` });
  }
  for (const file of list) {
    // latin1 keeps every byte as one char, so strings inside the wasm match too
    const text = readFileSync(file).toString("latin1");
    for (const marker of markers) {
      if (text.includes(marker)) {
        problems.push({ file: relative(dir, file).split(sep).join("/"), marker });
      }
    }
  }
  return problems;
}

const isMain = process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) {
  const args = process.argv.slice(2);
  const demo = args.includes("--demo");
  const dir = resolve(args.find((a) => !a.startsWith("--")) ?? "dist");
  const problems = scanDist(dir, { demo });
  for (const p of problems) {
    console.error(
      `${p.file}: test-only code in a ${demo ? "demo" : "release"} build: "${p.marker}"`,
    );
  }
  if (problems.length > 0) {
    console.error(`check-release-bundle: ${problems.length} problem(s) in ${dir}`);
    process.exit(1);
  }
  console.log(`check-release-bundle: ok (${relative(process.cwd(), dir) || "."})`);
}
