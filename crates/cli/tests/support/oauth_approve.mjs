// SPDX-License-Identifier: AGPL-3.0-or-later
// Completes an OAuth authorization on the devnet PDS in headless Chromium (the e2e tests of
// `lessdugin login`). Usage:
//
//   node oauth_approve.mjs <authorize-url> <password> capture|follow
//
// - capture: the redirect to http://127.0.0.1:<port>/callback is recorded and printed (the
//   `--no-browser` paste-back flow; nothing listens on that port, so the page fails to load).
// - follow: the browser follows the redirect to the CLI's loopback listener; the final URL and
//   the page text are printed.
//
// Prints `REDIRECT <url>` on success. Playwright is loaded from PLAYWRIGHT_MODULE (default
// `playwright`); browsers from PLAYWRIGHT_BROWSERS_PATH.

import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || "playwright");

const [url, password, mode] = process.argv.slice(2);
if (!url || !password || !["capture", "follow"].includes(mode)) {
  console.error("usage: oauth_approve.mjs <authorize-url> <password> capture|follow");
  process.exit(2);
}

const callback = /^http:\/\/127\.0\.0\.1:\d+\/callback/;
const browser = await chromium.launch();
try {
  const page = await browser.newPage();
  let captured = null;
  if (mode === "capture") {
    // The callback is reached by a redirect, which `page.route` does not intercept; the
    // request event fires for it, and the navigation then fails (nothing listens).
    page.on("request", (request) => {
      if (callback.test(request.url())) captured = request.url();
    });
  }
  await page.goto(url);
  // Sign in (the identifier is prefilled from login_hint).
  await page.locator("input[name=password]").fill(password, { timeout: 30000 });
  await page.locator("button[type=submit]").click();
  // Consent.
  await page.getByRole("button", { name: "Authorize" }).click({ timeout: 30000 });
  if (mode === "capture") {
    const deadline = Date.now() + 30000;
    while (!captured && Date.now() < deadline) {
      await page.waitForTimeout(100);
    }
    if (!captured) throw new Error("no redirect to the callback was seen");
    console.log(`REDIRECT ${captured}`);
  } else {
    await page.waitForURL(callback, { timeout: 30000 });
    await page.waitForLoadState("load");
    console.log(`REDIRECT ${page.url()}`);
    console.log(
      `PAGE ${(await page.evaluate(() => document.body.innerText)).replace(/\s+/g, " ")}`,
    );
  }
} finally {
  await browser.close();
}
