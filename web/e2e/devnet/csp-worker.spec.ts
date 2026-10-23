// SPDX-License-Identifier: AGPL-3.0-or-later
// The CSP watch of every devnet test sees violations inside the engine worker, which does all
// the network I/O and compiles the wasm (security review #3, conformance review M2): the
// document's `securitypolicyviolation` listener never hears of them, so the worker reports each
// one on the console. The e2e build's `cspProbe` makes the worker run one blocked `eval`; under
// the enforced Trusted Types of the §6.2 policy the browser reports it as a
// `require-trusted-types-for` violation of the `eval` sink (without Trusted Types, as
// `script-src` blocking `eval`).

import { expect, test } from "@playwright/test";
import { e2eState, watchStrict, withDevnet } from "./state.ts";

const s = e2eState();

test("a CSP violation inside the engine worker reaches the watcher", async ({ page }) => {
  const w = await watchStrict(page);
  await withDevnet(page, s, { cspProbe: true });
  await page.goto(`${s.app}/#/`);
  await page.evaluate(() => window.__lessduginE2E?.ready());
  await expect
    .poll(() => w.problems(), { timeout: 15_000 })
    .toContainEqual(
      expect.stringMatching(
        /^console: lessdugin engine worker: csp-violation (require-trusted-types-for blocked=trusted-types-sink|script-src blocked=eval)$/,
      ),
    );
});

test("without the probe the same start-up has no violation", async ({ page }) => {
  const w = await watchStrict(page);
  await withDevnet(page, s);
  await page.goto(`${s.app}/#/`);
  await page.evaluate(() => window.__lessduginE2E?.ready());
  // give a late worker report the time the first test needed
  await page.waitForTimeout(1000);
  expect(await w.problems()).toEqual([]);
});
