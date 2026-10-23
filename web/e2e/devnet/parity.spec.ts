// SPDX-License-Identifier: AGPL-3.0-or-later
// Parity mode in the SPA (engine-api.md §2.1; e2e builds only): with the same `parityNowMs`,
// two fresh browser profiles that sync the same records at different times produce the same
// dto/1 bytes for `getThread` and `listPosts` (the debug hook returns the engine's own text,
// as the CLI's `--json` prints it). The cross-client comparison of docs/plan.md §4.2 builds on
// this: scenario.spec.ts.

import { expect, test } from "@playwright/test";
import { e2eState, route, watchStrict, withDevnet } from "./state.ts";

const s = e2eState();

test("parity mode: two fresh profiles at different times read the same thread bytes", async ({
  browser,
}) => {
  const t = Date.now() + 86_400_000;
  const read = async (): Promise<{ thread: string; list: string; notice: string }> => {
    const ctx = await browser.newContext();
    const page = await ctx.newPage();
    const w = await watchStrict(page);
    await withDevnet(page, s, { parityNowMs: t });
    await page.goto(`${s.app}/${route(s.post.uri)}`);
    await expect(
      page.getByRole("heading", { level: 1, name: "Hello from the PDS API" }),
    ).toBeVisible();
    await expect(page.getByText("A first comment from alice.")).toBeVisible();
    const thread =
      (await page.evaluate((uri) => window.__lessduginE2E?.getThread(uri), s.post.uri)) ?? "";
    const list =
      (await page.evaluate((forum) => window.__lessduginE2E?.listPosts({ forum }), s.forum.uri)) ??
      "";
    const notice = await page.getByTestId("engine-banner").innerText();
    expect(await w.problems()).toEqual([]);
    await ctx.close();
    return { thread, list, notice };
  };
  const a = await read();
  await new Promise((r) => setTimeout(r, 1500));
  const b = await read();
  expect(a.notice).toContain(`parity time ${t}`);
  expect(a.thread.startsWith('{"dto":1,')).toBe(true);
  expect(b.thread).toBe(a.thread);
  expect(b.list).toBe(a.list);
  // Handles are verified by the query itself in parity mode, whatever ran in the background.
  expect(a.thread).toContain('"handle":"alice.test"');
});
