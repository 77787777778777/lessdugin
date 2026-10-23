// SPDX-License-Identifier: AGPL-3.0-or-later
// The core's KV conformance suites (crates/testkit `kv_conformance` and
// `record_store_conformance`) against the browser stores in real Chromium IndexedDB, not only
// fake-indexeddb (packages/engine/src/kv-conformance.test.ts; conformance review L3): byte-wise
// key order, ranges, limits and atomic batches as the engine relies on them (sync.md §12.4).
//
// The harness page is served from memory (Playwright routes): the kv-test wasm module of
// `cargo xtask wasm --kv-test` (packages/engine/pkg/kvtest) and packages/engine/src/idb.ts with
// its types stripped. Without the module the test is skipped, unless LESSDUGIN_KV_TEST=1 (set
// it where the module is built first, as CI's browser job should: `cargo xtask wasm --kv-test`).

import { existsSync, readFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";

const ENGINE = fileURLToPath(new URL("../../packages/engine/", import.meta.url));
const KVTEST = `${ENGINE}pkg/kvtest/`;
const present = existsSync(`${KVTEST}lessdugin_engine_bg.wasm`);
const required = process.env.LESSDUGIN_KV_TEST === "1";
const ORIGIN = "http://127.0.0.1:47999";

const MAIN = `
import init, { kvConformance } from "./glue.js";
import { IdbKv, MemKv } from "./idb.ts";
const done = [];
let panic = null;
const original = console.error;
// a violated assertion panics in wasm; its hook logs on console.error
console.error = (...a) => { panic = a.map(String).join(" "); original(...a); };
async function run(store) {
  await Promise.race([
    kvConformance(store),
    new Promise((_, reject) => {
      const t = setInterval(() => { if (panic) { clearInterval(t); reject(new Error(panic)); } }, 20);
    }),
  ]);
}
try {
  await init({ module_or_path: new URL("./kv.wasm", import.meta.url) });
  for (const [name, open] of [
    ["memory", async () => new MemKv()],
    ["idb-strict", () => IdbKv.open("conformance-state", "strict")],
    ["idb-relaxed", () => IdbKv.open("conformance-cache", "relaxed")],
  ]) {
    await run(await open());
    done.push(name);
  }
  window.__kv = { ok: true, done };
} catch (e) {
  window.__kv = { ok: false, done, error: String(e && e.message ? e.message : e).slice(0, 4000) };
}
`;

function js(path: string): string {
  return stripTypeScriptTypes(readFileSync(path, "utf8"), { mode: "transform" });
}

test("the core's KV conformance suites pass on Chromium's IndexedDB", async ({ page }) => {
  test.skip(!present && !required, "no kv-test module: run `cargo xtask wasm --kv-test`");
  expect(present, `${KVTEST} is missing: run \`cargo xtask wasm --kv-test\``).toBe(true);
  test.setTimeout(300_000);
  const files: Record<string, { type: string; body: string | Buffer }> = {
    "/": {
      type: "text/html",
      body: '<!doctype html><title>kv</title><script type="module" src="./main.js"></script>',
    },
    "/main.js": { type: "text/javascript", body: MAIN },
    "/idb.ts": { type: "text/javascript", body: js(`${ENGINE}src/idb.ts`) },
    "/protocol.ts": { type: "text/javascript", body: js(`${ENGINE}src/protocol.ts`) },
    "/glue.js": { type: "text/javascript", body: readFileSync(`${KVTEST}lessdugin_engine.js`) },
    "/kv.wasm": {
      type: "application/wasm",
      body: readFileSync(`${KVTEST}lessdugin_engine_bg.wasm`),
    },
  };
  await page.route(`${ORIGIN}/**`, (route) => {
    const f = files[new URL(route.request().url()).pathname];
    return f
      ? route.fulfill({ status: 200, contentType: f.type, body: f.body })
      : route.fulfill({ status: 404, body: "" });
  });
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(`${ORIGIN}/`);
  const result = await page.waitForFunction(
    () => (window as unknown as { __kv?: unknown }).__kv,
    null,
    { timeout: 280_000, polling: 250 },
  );
  expect(await result.jsonValue()).toEqual({
    ok: true,
    done: ["memory", "idb-strict", "idb-relaxed"],
  });
  expect(errors).toEqual([]);
});
