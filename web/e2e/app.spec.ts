// SPDX-License-Identifier: AGPL-3.0-or-later
// End-to-end tests of the built SPA with the mock engine (`?mock`).

import { expect, type Locator, type Page, test } from "@playwright/test";
import { PRODUCTION_CSP } from "../csp.mjs";
import { FORUM, WELCOME, watch } from "./helpers.ts";
import { startStaticServer } from "./serve.mjs";

async function scoreOf(group: Locator): Promise<number> {
  const text = await group.locator("span", { hasText: "score" }).first().innerText();
  const m = /(-?\d+)\s*$/.exec(text);
  if (!m?.[1]) throw new Error(`no score in "${text}"`);
  return Number(m[1]);
}

async function openWelcomePost(page: Page, base = ""): Promise<void> {
  await page.goto(`${base}/?mock#/`);
  await expect(page.getByRole("heading", { level: 1, name: "Your forums" })).toBeVisible();
  await page.getByRole("link", { name: FORUM }).first().click();
  await expect(page.getByRole("heading", { level: 1, name: FORUM })).toBeVisible();
  await page.getByRole("link", { name: WELCOME }).click();
  await expect(page.getByRole("heading", { level: 1, name: WELCOME })).toBeVisible();
}

/** The main flow, shared by the vite-preview run and the production-CSP run. */
async function mainFlow(page: Page, base = ""): Promise<void> {
  await openWelcomePost(page, base);

  // Renderer output reached the DOM through the sanitiser intact.
  const body = page.locator("[data-rendered-root]").first();
  await expect(body.locator("table")).toBeVisible();
  await expect(body.locator("math")).toHaveCount(2);
  await expect(body.locator("a.mention")).toHaveText("@bob.test");
  await expect(body.locator("details.spoiler > summary")).toHaveText("Spoiler");

  // Expand a comment the fold collapsed (new user).
  const dan = page.getByRole("article", { name: /^Comment by Dan Newcomer/ });
  await expect(dan.getByText("Collapsed: from a new user.")).toBeVisible();
  await dan.getByRole("button", { name: "Expand comment" }).click();
  await expect(dan.getByText("Hi all, first comment here!")).toBeVisible();

  // Vote: first-use interstitial, optimistic score, undo inside the 4 s window.
  const post = page.getByRole("article", { name: WELCOME });
  const karma = post.getByRole("group", { name: "Karma vote" });
  const before = await scoreOf(karma);
  await karma.getByRole("button", { name: "Upvote" }).click();
  const dialog = page.getByRole("dialog", { name: "Votes are public" });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "I understand, continue" }).click();
  await expect.poll(() => scoreOf(karma)).toBe(before + 1);
  await expect(karma.getByRole("button", { name: /^Upvote/ })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await page.getByRole("button", { name: "Undo" }).click();
  await expect(page.getByText("Vote undone")).toBeVisible();
  await expect.poll(() => scoreOf(karma)).toBe(before);
  await expect(karma.getByRole("button", { name: /^Upvote/ })).toHaveAttribute(
    "aria-pressed",
    "false",
  );

  // Vote again (no interstitial): held, sent, then confirmed by the engine's re-query.
  await karma.getByRole("button", { name: "Upvote" }).click();
  await expect(dialog).toBeHidden();
  await expect.poll(() => scoreOf(karma)).toBe(before + 1);
  await expect(page.getByRole("status").filter({ hasText: "Saving…" })).toBeVisible();
  await expect(page.getByRole("status").filter({ hasText: "Saving…" })).toBeHidden({
    timeout: 15_000,
  });
  await expect.poll(() => scoreOf(karma)).toBe(before + 1);

  // Reply to a comment; the engine's `changed` event brings the reply and the new count.
  const count = page.getByTestId("comment-count");
  const countBefore = Number(/\d+/.exec(await count.innerText())?.[0]);
  const carol = page
    .getByRole("article", { name: /^Comment by Carol Bayes/ })
    .filter({ hasText: "This is a great introduction" });
  await carol.getByRole("button", { name: "Reply", exact: true }).click();
  await carol.locator(".cm-content").click();
  await page.keyboard.type("Agreed, and **public** votes keep the fold auditable.");
  await expect(carol.getByRole("region", { name: "Preview" }).locator("strong")).toHaveText(
    "public",
  );
  await carol.getByRole("button", { name: "Publish reply" }).click();
  await expect(page.getByText("Reply published")).toBeVisible();
  const reply = page
    .getByRole("article", { name: /^Comment by You \(mock account\)/ })
    .filter({ hasText: "keep the fold auditable" });
  await expect(reply).toBeVisible();
  await expect(reply.locator("strong")).toHaveText("public");
  await expect(count).toHaveText(`${countBefore + 1} comments`);
}

test("home → forum → post → expand → vote and undo → reply → live update", async ({ page }) => {
  const w = await watch(page);
  await mainFlow(page);
  expect(w.errors).toEqual([]);
});

test("forum filters, sorts and search keep their query in the hash", async ({ page }) => {
  const w = await watch(page);
  await page.goto("/?mock#/");
  await page.getByRole("link", { name: FORUM }).first().click();
  // the M3 default view is the frontpage, magic first (src/lib/listDefaults.ts):
  // personal-tier posts, like the quick takes, are hidden until "All posts"
  await expect(page.getByRole("button", { name: "Frontpage" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await expect(page.getByText(/Quick take: the cheapest way/)).toHaveCount(0);
  await page.getByRole("button", { name: "All posts" }).click();
  await expect(page.getByRole("link", { name: /Quick take: the cheapest way/ })).toBeVisible();
  await expect(page).toHaveURL(/\?mock#\/f\/[^?]+\?filter=all$/);
  await page.getByLabel("Sort").selectOption("new");
  await expect(page).toHaveURL(/filter=all/);
  await expect(page).toHaveURL(/sort=new/);
  await expect(page.getByRole("button", { name: "All posts" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  const search = page.getByRole("searchbox", { name: "Search" });
  await search.fill("confusion");
  await search.press("Enter");
  await expect(page.getByRole("heading", { level: 1, name: "Search" })).toBeVisible();
  await expect(page.locator("mark").first()).toHaveText(/confusion/i);
  expect(w.errors).toEqual([]);
});

test("keyboard navigation through a thread", async ({ page }) => {
  const w = await watch(page);
  await openWelcomePost(page);
  await page.getByRole("heading", { level: 1, name: WELCOME }).click();
  const row = (i: number) => page.locator(`[data-row-index="${i}"]`);
  await page.keyboard.press("j");
  await expect(row(0)).toBeFocused();
  await page.keyboard.press("j");
  await expect(row(1)).toBeFocused();
  await page.keyboard.press("j");
  await expect(row(2)).toBeFocused();
  await page.keyboard.press("k");
  await expect(row(1)).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(row(1).getByRole("button", { name: "Expand comment" })).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(row(1).getByRole("button", { name: "Collapse comment" })).toBeVisible();
  // Indentation is capped: deeper replies continue on their own page.
  const more = page.getByRole("link", { name: /Continue this thread/ });
  await expect(more).toBeVisible();
  await more.click();
  await expect(page.getByRole("heading", { level: 1, name: "Comment thread" })).toBeVisible();
  await expect(page.getByText("Depth 7: this reply is past the indentation cap")).toBeVisible();
  expect(w.errors).toEqual([]);
});

test("long threads are virtualised", async ({ page }) => {
  const w = await watch(page);
  await page.goto("/?mock#/");
  await page.getByRole("link", { name: FORUM }).first().click();
  await page.getByRole("link", { name: "A very long thread" }).click();
  await expect(page.getByTestId("comment-count")).toHaveText("160 comments");
  await expect(page.locator("[data-virtualized]")).toBeVisible();
  const rendered = await page.locator("[data-virtualized] [data-row-index]").count();
  expect(rendered).toBeGreaterThan(0);
  expect(rendered).toBeLessThan(80);
  await page.getByRole("heading", { level: 1, name: "A very long thread" }).click();
  for (let i = 0; i < 12; i++) await page.keyboard.press("j");
  await expect(page.locator(":focus")).toHaveAttribute("data-row-index", /\d+/);
  expect(w.errors).toEqual([]);
});

test.describe("production headers", () => {
  let server: Awaited<ReturnType<typeof startStaticServer>>;
  test.beforeAll(async () => {
    server = await startStaticServer();
  });
  test.afterAll(async () => {
    await server.close();
  });

  test("no CSP or Trusted Types violation under the production CSP", async ({ page }) => {
    const w = await watch(page);
    const response = await page.goto(`${server.url}/?mock#/`);
    // Exactly the threat-model §6.1 text (csp.mjs; src/lib/csp.test.ts checks it against the spec).
    expect(response?.headers()["content-security-policy"]).toBe(PRODUCTION_CSP);
    expect(response?.headers()["referrer-policy"]).toBe("no-referrer");

    await mainFlow(page, server.url);

    // Hover preview of an internal record link (rendered through the same policy).
    const policyLink = page.locator("[data-rendered-root] a.at-link", {
      hasText: "the moderation policy",
    });
    await policyLink.first().hover();
    await expect(page.getByRole("dialog", { name: "Link preview" })).toContainText(
      "Moderation policy",
    );
    await page.mouse.move(0, 0);

    // React palette, the long (virtualised) thread, settings with the build hash.
    const post = page.getByRole("article", { name: WELCOME });
    await post.getByRole("button", { name: "Add react" }).click();
    await expect(page.getByRole("dialog", { name: "Add a react" })).toBeVisible();
    await page.keyboard.press("Escape");
    await page.goto(`${server.url}/?mock#/settings`);
    await expect(page.getByTestId("build-hash")).toHaveText(/^[0-9a-f]{64}$/);
    await page.goto(`${server.url}/?mock#/`);
    await page.getByRole("link", { name: FORUM }).first().click();
    await page.getByRole("link", { name: "A very long thread" }).click();
    await expect(page.locator("[data-virtualized] [data-row-index]").first()).toBeVisible();

    expect(await w.violations()).toEqual([]);
    expect(w.errors).toEqual([]);

    // Trusted Types are really enforced, so the clean run above is meaningful.
    const sink = await page.evaluate(() => {
      try {
        document.createElement("div").innerHTML = "<b>x</b>";
        return "assigned";
      } catch {
        return "blocked";
      }
    });
    expect(sink).toBe("blocked");
  });

  test("the real engine (wasm in a worker) starts under the production CSP", async ({ page }) => {
    const w = await watch(page);
    // No `?mock`: the demo build runs the wasm engine then, as a release build always does.
    // Nothing may leave the machine; the settings page needs no network.
    await page.route(/^(?!http:\/\/127\.0\.0\.1[:/])/, (r) => r.abort());
    await page.goto(`${server.url}/#/settings`);
    // The version comes from the wasm, through the worker created via the `lessdugin-worker`
    // Trusted Types policy.
    await expect(page.getByText(/^wasm · \d+\.\d+\.\d+/)).toBeVisible({ timeout: 30_000 });
    await expect(page.getByTestId("mock-banner")).toHaveCount(0);
    expect(await w.violations()).toEqual([]);
    expect(w.errors).toEqual([]);
  });
});
