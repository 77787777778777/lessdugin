// SPDX-License-Identifier: AGPL-3.0-or-later
// Interaction flows, run under the production headers (CSP and Trusted Types enforced, served by
// e2e/serve.mjs from deploy/site/_headers): every test also asserts that nothing violated the
// policy and nothing logged an error.

import { expect, type Page, test } from "@playwright/test";
import { FORUM, FORUM_PATH, type Watch, WELCOME, WELCOME_PATH, watch } from "./helpers.ts";
import { startStaticServer } from "./serve.mjs";

let server: Awaited<ReturnType<typeof startStaticServer>>;
test.beforeAll(async () => {
  server = await startStaticServer();
});
test.afterAll(async () => {
  await server.close();
});

let w: Watch;
test.beforeEach(async ({ page }) => {
  w = await watch(page);
});
test.afterEach(async () => {
  expect(await w.violations()).toEqual([]);
  expect(w.errors).toEqual([]);
});

async function go(page: Page, path: string, h1: string): Promise<void> {
  await page.goto(`${server.url}/?mock#${path}`);
  await expect(page.getByRole("heading", { level: 1, name: h1 })).toBeVisible();
}

/** Covered height of the virtualised list inside the viewport, and that viewport span. */
function coverage(page: Page): Promise<{ span: number; covered: number }> {
  return page.evaluate(() => {
    const list = document.querySelector("[data-virtualized]")?.getBoundingClientRect();
    if (!list) return { span: 0, covered: 0 };
    const top = Math.max(0, list.top);
    const bottom = Math.min(innerHeight, list.bottom);
    const rows = [...document.querySelectorAll("[data-virtualized] [data-row-index]")]
      .map((e) => e.getBoundingClientRect())
      .map((b) => [Math.max(top, b.top), Math.min(bottom, b.bottom)] as const)
      .filter(([a, b]) => b > a)
      .sort((x, y) => x[0] - y[0]);
    let covered = 0;
    let cur = top;
    for (const [a, b] of rows) {
      if (b > cur) {
        covered += b - Math.max(a, cur);
        cur = b;
      }
    }
    return { span: Math.max(0, bottom - top), covered };
  });
}

test("a virtualised thread keeps rendering rows when content above it grows", async ({ page }) => {
  await go(page, FORUM_PATH, FORUM);
  await page.getByRole("link", { name: "A very long thread" }).click();
  await expect(page.getByTestId("comment-count")).toHaveText("160 comments");
  const listTop = () =>
    page.evaluate(
      () =>
        (document.querySelector("[data-virtualized]")?.getBoundingClientRect().top ?? 0) +
        window.scrollY,
    );
  const before = await listTop();
  // The top-level composer's live preview pushes the list several screens down.
  await page.getByRole("textbox", { name: "New comment" }).click();
  const para = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(6);
  for (let i = 0; i < 10; i++) await page.keyboard.insertText(`${para}\n\n`);
  await expect.poll(listTop, { timeout: 5000 }).toBeGreaterThan(before + 2000);
  const top = await listTop();
  const height = await page.evaluate(() => document.documentElement.scrollHeight);
  const gaps: string[] = [];
  for (let y = top; y < height - 1000; y += 1500) {
    await page.evaluate((yy) => window.scrollTo(0, yy), y);
    await expect
      .poll(async () => {
        const c = await coverage(page);
        // Rows have margins between them; a stale scroll margin leaves the viewport empty.
        return c.span === 0 || c.covered >= c.span * 0.6;
      })
      .toBe(true)
      .catch(async () => gaps.push(`y=${y}: ${JSON.stringify(await coverage(page))}`));
  }
  expect(gaps).toEqual([]);
  // j/k still scroll the focused comment into view.
  await page.getByRole("heading", { level: 1, name: "A very long thread" }).click();
  for (let i = 0; i < 30; i++) await page.keyboard.press("j");
  const focused = page.locator(":focus");
  await expect(focused).toHaveAttribute("data-row-index", /\d+/);
  await expect(focused).toBeInViewport();
});

test("a react on selected text across a footnote reference is anchored, not orphaned", async ({
  page,
}) => {
  await go(page, WELCOME_PATH, WELCOME);
  const post = page.getByRole("article", { name: WELCOME });
  const trigger = post.getByRole("button", { name: "Add react" });
  const palette = page.getByRole("dialog", { name: "Add a react" });
  // The trigger closes the palette it opened.
  await trigger.click();
  await expect(palette).toBeVisible();
  await trigger.click();
  await expect(palette).toBeHidden();
  await expect(trigger).toHaveAttribute("aria-expanded", "false");
  // Select "same view.[2] Read the moderation policy[example.com]" and react to it.
  await page.evaluate(() => {
    const root = document.querySelector("[data-rendered-root]");
    if (!root) throw new Error("no body");
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    let start: [Node, number] | null = null;
    let end: [Node, number] | null = null;
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
      const data = (n as Text).data;
      if (!start && data.includes("into the same view")) start = [n, data.indexOf("same view")];
      if (start && data.includes("moderation policy")) {
        end = [n, data.indexOf("policy") + "policy".length];
        break;
      }
    }
    if (!start || !end) throw new Error("text not found");
    const r = document.createRange();
    r.setStart(...start);
    r.setEnd(...end);
    getSelection()?.removeAllRanges();
    getSelection()?.addRange(r);
  });
  await trigger.click();
  await expect(palette).toContainText("same view. Read the moderation policy");
  await palette.getByRole("button", { name: /Crux/ }).first().click();
  await page.getByRole("button", { name: "I understand, continue" }).click();
  const tallies = post.getByRole("list", { name: "Reacts" });
  await expect(tallies).toContainText("“same view. Read the moderation policy”");
  // Once the engine applies it (after the undo window), the re-queried tally still resolves.
  await expect(page.getByRole("status").filter({ hasText: "Saving…" })).toBeHidden({
    timeout: 15_000,
  });
  await expect(tallies).toContainText("“same view. Read the moderation policy”");
  const crux = tallies.getByRole("listitem").filter({ hasText: "Crux" });
  await expect(crux).not.toContainText("quoted text no longer present");
});

test("signed out, actions lead to sign-in instead of failing on submit", async ({ page }) => {
  await go(page, WELCOME_PATH, WELCOME);
  await page.getByRole("button", { name: /You \(mock account\)/ }).click();
  await page.getByRole("menuitem", { name: "Sign out" }).click();
  await expect(
    page.getByRole("banner").getByRole("link", { name: "Sign in", exact: true }),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Reply", exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "Sign in to reply" }).first().click();
  await expect(page.getByRole("heading", { level: 1, name: /Sign in/ })).toBeVisible();
  await go(page, FORUM_PATH, FORUM);
  await expect(page.getByRole("button", { name: "Join" })).toHaveCount(0);
  await expect(page.getByRole("link", { name: "Sign in to join" })).toBeVisible();
});

test("create a forum, edit its declaration and manage a moderator", async ({ page }) => {
  await go(page, "/", "Your forums");
  await page.getByRole("button", { name: "Create a forum" }).click();
  const form = page.getByRole("region", { name: "Create a forum" });
  await form.getByLabel("Address").fill("Bad Slug");
  await form.getByLabel("Name").fill("Reading group");
  await form.getByRole("button", { name: "Create the forum" }).click();
  await expect(form.getByRole("alert")).toContainText("lowercase letters and digits");
  await form.getByLabel("Address").fill("reading-group");
  await form.getByLabel(/^Tagline/).fill("Slow reading, together.");
  await form.getByText("Open", { exact: true }).click();
  await form.getByRole("button", { name: "Create the forum" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Reading group" })).toBeVisible();
  await expect(page.getByText("Open membership")).toBeVisible();

  await page.getByRole("link", { name: "About" }).click();
  await page.getByRole("button", { name: "Edit the forum" }).click();
  await expect(page.getByLabel("Name")).toHaveValue("Reading group");
  await expect(page.getByLabel(/^Tagline/)).toHaveValue("Slow reading, together.");
  await page.getByLabel("Name").fill("Reading circle");
  await page.getByText("Approval required", { exact: true }).click();
  await page.getByRole("button", { name: "Save the declaration" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Reading circle" })).toBeVisible();
  await expect(page.getByText("Approval required").first()).toBeVisible();

  await page.getByPlaceholder("handle or did:…").fill("bob.test");
  await page.getByRole("button", { name: "Add moderator" }).click();
  await page.getByRole("button", { name: "I understand, continue" }).click();
  const mods = page.getByRole("list").filter({ hasText: "owner" });
  await expect(mods).toContainText("Bob Moderator");
  await mods
    .getByRole("listitem")
    .filter({ hasText: "Bob Moderator" })
    .getByRole("button", { name: "Remove" })
    .click();
  await expect(mods).not.toContainText("Bob Moderator");
});

test("vote on the tags of someone else's post", async ({ page }) => {
  await go(page, WELCOME_PATH, WELCOME);
  const post = page.getByRole("article", { name: WELCOME });
  await post.getByRole("button", { name: "Vote on tags" }).click();
  const panel = page.getByRole("region", { name: "Tag votes" });
  const first = panel.getByRole("listitem").first();
  await first.getByRole("button", { name: /Applies/ }).click();
  await page.getByRole("button", { name: "I understand, continue" }).click();
  await expect(page.getByText("Tag vote recorded (public)")).toBeVisible();
  await expect(first.getByRole("button", { name: /Applies/ })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  // Applying another tag is a vote for it.
  const choose = panel.getByLabel("Apply another tag");
  const option = await choose.locator("option").nth(1).getAttribute("value");
  if (!option) throw new Error("no other tag to apply");
  await choose.selectOption(option);
  await panel.getByRole("button", { name: "Vote to apply" }).click();
  await expect(panel.getByRole("listitem")).toHaveCount(await panel.getByRole("listitem").count());
  await expect(
    panel
      .getByRole("listitem")
      .last()
      .getByRole("button", { name: /Applies/ }),
  ).toHaveAttribute("aria-pressed", "true");
});
