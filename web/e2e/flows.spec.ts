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

test("leave a forum after confirming (engine-api.md §3.2 leave)", async ({ page }) => {
  await go(page, FORUM_PATH, FORUM);
  await page.getByRole("button", { name: "Leave", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Leave this forum?" });
  await expect(dialog).toContainText("Your membership record is deleted");
  await dialog.getByRole("button", { name: "Cancel" }).click();
  await expect(dialog).toBeHidden();
  await page.getByRole("button", { name: "Leave", exact: true }).click();
  await dialog.getByRole("button", { name: "Leave" }).click();
  await expect(page.getByText("Left the forum")).toBeVisible();
});

test("a spilled about is loaded before the forum form is shown (robustness-2)", async ({
  page,
}) => {
  await page.goto(`${server.url}/?mock&spilled#/`);
  await page.getByRole("button", { name: "Create a forum" }).click();
  const form = page.getByRole("region", { name: "Create a forum" });
  await form.getByLabel("Address").fill("spilled-about");
  await form.getByLabel("Name").fill("Spilled about");
  await form.getByRole("textbox", { name: /^About/ }).fill("Source behind the blob");
  await form.getByRole("button", { name: "Create the forum" }).click();
  await page.getByRole("link", { name: "About" }).click();
  await page.getByRole("button", { name: "Edit the forum" }).click();
  const about = page.getByRole("textbox", { name: /^About/ });
  await expect(about).toHaveValue("Source behind the blob");
  await expect(about).toBeEnabled();
  await expect(page.getByRole("button", { name: "Save the declaration" })).toBeEnabled();
  await expect(page.getByTestId("about-unavailable")).toHaveCount(0);
  await about.fill("Source behind the blob, edited");
  await page.getByRole("button", { name: "Save the declaration" }).click();
  await expect(page.getByRole("region", { name: "About this forum" })).toContainText(
    "Source behind the blob, edited",
  );
});

test("a spilled post opens its normal editor with the stored source (robustness-2)", async ({
  page,
}) => {
  const source = "Testing quick takes from the mock account. Everything here lives in memory.";
  await page.goto(`${server.url}/?mock&spilled#${FORUM_PATH}/new`);
  await page.getByText("Quick take", { exact: true }).click();
  const body = page.getByRole("textbox", { name: "Post body" });
  await body.fill(source);
  await page.getByRole("button", { name: "Publish" }).click();
  await page.getByRole("link", { name: "Edit", exact: true }).click();
  await expect(body).toHaveText(source);
  await body.fill(`${source}\n\nA browser edit.`);
  await page.getByRole("button", { name: "Save edit" }).click();
  await expect(page.getByText("A browser edit.")).toBeVisible();
});

test("a spilled post edit preserves its media and link-card draft fields", async ({ page }) => {
  await page.goto(`${server.url}/?mock&spilled#${FORUM_PATH}/new`);
  await page.getByLabel("Title", { exact: true }).fill("Preserved media and card");
  await page.getByText("Link post", { exact: true }).click();
  await page.getByRole("textbox", { name: "Link", exact: true }).fill("https://example.test/paper");
  const body = page.getByRole("textbox", { name: "Post body" });
  await body.fill("Source behind the blob");
  await page.getByTestId("attachment-input").setInputFiles({
    name: "figure.png",
    mimeType: "image/png",
    buffer: Buffer.from(
      "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
      "base64",
    ),
  });
  await expect(page.getByTestId("attachments")).toContainText("figure.png");
  await page.getByLabel("Alt text").fill("Preserved diagram");
  await page.getByRole("button", { name: "Publish" }).click();
  await page.getByRole("link", { name: "Edit", exact: true }).click();
  await expect(body).toHaveText("Source behind the blob");
  await expect(page.getByRole("textbox", { name: "Link", exact: true })).toHaveValue(
    "https://example.test/paper",
  );
  await expect(page.getByLabel("Alt text")).toHaveValue("Preserved diagram");
  await body.fill("Edited source");
  await page.getByRole("button", { name: "Save edit" }).click();
  await page.getByRole("link", { name: "Edit", exact: true }).click();
  await expect(body).toHaveText("Edited source");
  await expect(page.getByLabel("Alt text")).toHaveValue("Preserved diagram");
  await expect(page.getByRole("textbox", { name: "Link", exact: true })).toHaveValue(
    "https://example.test/paper",
  );
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

test("edit a wiki page against its head and read its history (engine-api.md §3.5)", async ({
  page,
}) => {
  await go(page, `${FORUM_PATH}/t/decision-theory`, FORUM);
  await page.getByRole("link", { name: "Edit page" }).click();
  const body = page.getByRole("textbox", { name: /^Page/ });
  await expect(body).toHaveValue(/## Classic problems/);
  await body.fill("Decision theory studies choice.\n\n## Classic problems\n\n- Newcomb's problem");
  await page.getByRole("textbox", { name: /^Edit summary/ }).fill("Trim the page");
  await page.getByRole("button", { name: "Save page" }).click();
  await expect(page.getByText("Page saved (a public revision)")).toBeVisible();
  await expect(page.getByText("Decision theory studies choice.")).toBeVisible();
  await page.getByRole("link", { name: "History", exact: true }).click();
  const revisions = page.getByRole("list", { name: "Revisions" }).getByRole("listitem");
  await expect(revisions).toHaveCount(3);
  await expect(revisions.first()).toContainText("(current)");
  await expect(revisions.first()).toContainText("Trim the page");

  // a new page, started from the tags list
  await page.getByRole("link", { name: "Tags", exact: true }).click();
  await page.getByLabel("New page address").fill("calibration");
  await page.getByRole("button", { name: "Start page" }).click();
  // the engine opens only tag pages: a page here that is not a tag would be replaced
  await expect(page.getByText(/^Only pages that are tags open here\./)).toBeVisible();
  await page.getByRole("textbox", { name: "Name", exact: true }).fill("Calibration");
  await page.getByRole("button", { name: "Create page" }).click();
  await expect(page.getByRole("heading", { level: 2, name: "Calibration" })).toBeVisible();
});

test("a wiki save that meets a newer head keeps the text and is redone after a reload (O-2)", async ({
  page,
}) => {
  // `?wikirace`: another participant's revision lands while the editor is open
  await page.goto(`${server.url}/?mock&wikirace#${FORUM_PATH}/t/decision-theory/edit`);
  const body = page.getByRole("textbox", { name: /^Page/ });
  await expect(body).toHaveValue(/## Classic problems/);
  const mine = "Decision theory, rewritten by me.";
  await body.fill(mine);
  await page.getByRole("button", { name: "Save page" }).click();
  const alert = page.getByRole("alert");
  await expect(alert).toContainText("This page changed since you opened it.");
  await expect(body).toHaveValue(mine);
  await alert.getByRole("button", { name: "Reload the page" }).click();
  const kept = page.getByRole("region", { name: "Your unsaved edit" });
  await expect(kept.getByRole("textbox", { name: "Your unsaved text" })).toHaveValue(mine);
  await expect(body).toHaveValue(/An edit made elsewhere/);
  await kept.getByRole("button", { name: "Use my text" }).click();
  await expect(body).toHaveValue(mine);
  await page.getByRole("button", { name: "Save page" }).click();
  await expect(page.getByText("Page saved (a public revision)")).toBeVisible();
  await expect(page.getByText(mine)).toBeVisible();
  await page.getByRole("link", { name: "History", exact: true }).click();
  const revisions = page.getByRole("list", { name: "Revisions" }).getByRole("listitem");
  // the genesis, carol's edit, the concurrent edit and the redone one, in one chain
  await expect(revisions).toHaveCount(4);
  await expect(revisions.nth(1)).toContainText("A concurrent edit");
});

test("create a sequence, then reorder and remove its entries (engine-api.md §3.5)", async ({
  page,
}) => {
  await go(page, `${FORUM_PATH}/seqs`, FORUM);
  await page.getByRole("link", { name: "New sequence" }).click();
  await page.getByRole("textbox", { name: "Title", exact: true }).fill("Starter pack");
  const add = page.getByRole("textbox", { name: "Add a post to chapter 1" });
  await add.fill(`#${WELCOME_PATH}`);
  await page.getByRole("button", { name: "Add", exact: true }).click();
  await expect(page.getByRole("list", { name: "Posts of chapter 1" })).toContainText(WELCOME);
  await page.getByRole("button", { name: "Create sequence" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Starter pack" })).toBeVisible();
  await expect(page.getByRole("article", { name: WELCOME })).toBeVisible();

  await page.getByRole("button", { name: /^Remove Welcome to LessDugin/ }).click();
  await expect(page.getByText("Removed from the sequence")).toBeVisible();
  await expect(page.getByRole("article", { name: WELCOME })).toHaveCount(0);

  await page.getByRole("link", { name: "Edit sequence" }).click();
  await page.getByRole("textbox", { name: "Title", exact: true }).fill("Starter pack, revised");
  await page.getByRole("button", { name: "Save sequence" }).click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Starter pack, revised" }),
  ).toBeVisible();
});
