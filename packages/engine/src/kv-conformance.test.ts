// SPDX-License-Identifier: AGPL-3.0-or-later
// The core's KV conformance suites (crates/testkit `kv_conformance` and
// `record_store_conformance`, the ones `MemKv` and `SqliteKv` pass) run in wasm against the
// browser stores, through the same `JsKv` adapter the engine uses (sync.md §12.4).
//
// The module is the test build `cargo xtask wasm --kv-test` writes to pkg/kvtest. Without it the
// test is skipped locally and fails under CI (or with LESSDUGIN_KV_TEST=1).
import { existsSync, readFileSync } from "node:fs";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { describe, expect, it } from "vitest";
import { IdbKv, type KvBackend, MemKv } from "./idb.ts";

const DIR = new URL("../pkg/kvtest/", import.meta.url);
const WASM = new URL("lessdugin_engine_bg.wasm", DIR);
const GLUE = new URL("lessdugin_engine.js", DIR).href;
const present = existsSync(WASM);
const required = process.env.CI !== undefined || process.env.LESSDUGIN_KV_TEST === "1";

interface KvTestModule {
  initSync(input: { module: BufferSource }): unknown;
  kvConformance(store: KvBackend): Promise<void>;
}

let loaded: Promise<KvTestModule> | null = null;
function load(): Promise<KvTestModule> {
  loaded ??= (async () => {
    const m = (await import(/* @vite-ignore */ GLUE)) as KvTestModule;
    m.initSync({ module: readFileSync(WASM) });
    return m;
  })();
  return loaded;
}

/** Runs the suites; a violation panics in wasm (the hook logs it on `console.error`), which
 * fails the test at once instead of leaving the promise pending. */
async function conformance(store: KvBackend): Promise<void> {
  const m = await load();
  const original = console.error;
  let fail: (e: Error) => void = () => {};
  const panicked = new Promise<never>((_, reject) => {
    fail = reject;
  });
  console.error = (...args: unknown[]) => {
    original(...args);
    fail(new Error(args.map(String).join(" ")));
  };
  try {
    await Promise.race([m.kvConformance(store), panicked]);
  } finally {
    console.error = original;
  }
}

describe("core KV conformance (wasm)", () => {
  it.runIf(!present && required)("needs the kv-test module", () => {
    expect.fail(`${WASM.pathname} is missing: run \`cargo xtask wasm --kv-test\` first`);
  });

  it.runIf(present)(
    "IdbKv over IndexedDB passes",
    async () => {
      const idb = { factory: new IDBFactory(), keyRange: IDBKeyRange };
      await conformance(await IdbKv.open("conformance-state", "strict", idb));
      await conformance(await IdbKv.open("conformance-cache", "relaxed", idb));
    },
    120_000,
  );

  it.runIf(present)("MemKv passes", () => conformance(new MemKv()), 120_000);
});
