// SPDX-License-Identifier: AGPL-3.0-or-later
// Completes a `lessdugin web` browser sign-in on the devnet PDS in headless Chromium
// (`web_e2e_devnet.rs`). Usage:
//
//   node web_e2e_approve.mjs <authorize-url> <password> <bridge-origin>
//
// Signs in, approves, follows the redirect to the `web` process's callback listener and its
// 303 back to the bridge, and prints `FINAL <url>` once the tab is on <bridge-origin>, and
// `VIA <url>` for every request to a 127.0.0.1 /callback. Playwright is loaded from
// PLAYWRIGHT_MODULE (default `playwright`); browsers from PLAYWRIGHT_BROWSERS_PATH.

import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || "playwright");

const [url, password, origin] = process.argv.slice(2);
if (!url || !password || !origin) {
  console.error("usage: web_e2e_approve.mjs <authorize-url> <password> <bridge-origin>");
  process.exit(2);
}

const callback = /^http:\/\/127\.0\.0\.1:\d+\/callback/;
const browser = await chromium.launch();
try {
  const page = await browser.newPage();
  page.on("request", (request) => {
    if (callback.test(request.url())) console.log(`VIA ${request.url().split("?")[0]}`);
  });
  await page.goto(url);
  await page.locator("input[name=password]").fill(password, { timeout: 30000 });
  await page.locator("button[type=submit]").click();
  await page.getByRole("button", { name: "Authorize" }).click({ timeout: 30000 });
  await page.waitForURL((u) => u.toString().startsWith(`${origin}/`), { timeout: 30000 });
  console.log(`FINAL ${page.url()}`);
} finally {
  await browser.close();
}
