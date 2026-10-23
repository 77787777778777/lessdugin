// SPDX-License-Identifier: AGPL-3.0-or-later
// Accessibility: axe (WCAG 2.1 A/AA, colour contrast included) in light and dark mode on the
// main pages and their interactive states, keyboard behaviour that axe cannot see, and the
// small-screen layout (no horizontal page scroll at 360 px).

import AxeBuilder from "@axe-core/playwright";
import { expect, type Page, test } from "@playwright/test";
import { FORUM_PATH, WELCOME, WELCOME_PATH, watch } from "./helpers.ts";

const TAGS = ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"];

async function axe(page: Page, label: string): Promise<string[]> {
  const result = await new AxeBuilder({ page }).withTags(TAGS).analyze();
  return result.violations.map(
    (v) =>
      `${label}: ${v.id} (${v.nodes.length}) ${v.nodes
        .slice(0, 3)
        .map((n) => n.target.join(" "))
        .join(" | ")}`,
  );
}

async function open(page: Page, path: string, ready: () => Promise<void>): Promise<void> {
  await page.goto(`/?mock#${path}`);
  await ready();
}

for (const scheme of ["light", "dark"] as const) {
  test.describe(`axe, ${scheme} mode`, () => {
    test.use({ colorScheme: scheme });

    test("pages", async ({ page }) => {
      const w = await watch(page);
      const found: string[] = [];
      const pages: [string, string][] = [
        ["/", "Your forums"],
        [FORUM_PATH, "LessDugin Test Forum"],
        [`${FORUM_PATH}/members`, "LessDugin Test Forum"],
        [`${FORUM_PATH}/mod`, "LessDugin Test Forum"],
        [`${FORUM_PATH}/settings`, "LessDugin Test Forum"],
        [`${FORUM_PATH}/new`, "LessDugin Test Forum"],
        [WELCOME_PATH, WELCOME],
        ["/notifications", "Notifications"],
        ["/drafts", "Drafts"],
        ["/settings", "Settings"],
        ["/search?q=vote", "Search"],
      ];
      for (const [path, h1] of pages) {
        await open(page, path, () =>
          expect(page.getByRole("heading", { level: 1, name: h1 })).toBeVisible(),
        );
        // Let queries settle (lists, threads) before scanning.
        await page.waitForLoadState("networkidle");
        found.push(...(await axe(page, path)));
      }
      expect(found).toEqual([]);
      expect(w.errors).toEqual([]);
    });

    test("interactive states: dialogs, palette, composer, toasts, forms", async ({ page }) => {
      const w = await watch(page);
      const found: string[] = [];
      await open(page, WELCOME_PATH, () =>
        expect(page.getByRole("heading", { level: 1, name: WELCOME })).toBeVisible(),
      );
      const post = page.getByRole("article", { name: WELCOME });

      await post
        .getByRole("group", { name: "Karma vote" })
        .getByRole("button", { name: "Upvote" })
        .click();
      await expect(page.getByRole("dialog", { name: "Votes are public" })).toBeVisible();
      found.push(...(await axe(page, "vote interstitial")));
      await page.getByRole("button", { name: "I understand, continue" }).click();
      await expect(page.getByRole("button", { name: "Undo" })).toBeVisible();
      found.push(...(await axe(page, "undo toast")));

      await post.getByRole("button", { name: "Add react" }).click();
      await expect(page.getByRole("dialog", { name: "Add a react" })).toBeVisible();
      found.push(...(await axe(page, "react palette")));
      await page.keyboard.press("Escape");

      await post.getByRole("button", { name: "Vote on tags" }).click();
      await expect(page.getByRole("region", { name: "Tag votes" })).toBeVisible();
      found.push(...(await axe(page, "tag votes")));

      const editor = page.getByRole("textbox", { name: "New comment" });
      await editor.click();
      await page.keyboard.type("Some **bold** text with $x^2$ math.");
      await expect(page.getByRole("region", { name: "Preview" }).locator("strong")).toHaveText(
        "bold",
      );
      found.push(...(await axe(page, "composer with preview")));

      await open(page, "/", () =>
        expect(page.getByRole("heading", { level: 1, name: "Your forums" })).toBeVisible(),
      );
      await page.getByRole("button", { name: "Create a forum" }).click();
      await page.getByRole("button", { name: "Create the forum" }).click();
      await expect(page.getByRole("alert")).toBeVisible();
      found.push(...(await axe(page, "forum form with errors")));

      expect(found).toEqual([]);
      expect(w.errors).toEqual([]);
    });
  });
}

test.describe("keyboard", () => {
  test("changing a sort or a filter keeps focus on the control", async ({ page }) => {
    await open(page, FORUM_PATH, () =>
      expect(page.getByRole("heading", { level: 1, name: "LessDugin Test Forum" })).toBeVisible(),
    );
    const sort = page.getByLabel("Sort");
    await sort.focus();
    await page.keyboard.press("ArrowDown");
    await expect(page).toHaveURL(/sort=/);
    await expect(sort).toBeFocused();
    await page.keyboard.press("ArrowDown");
    await expect(sort).toBeFocused();
    const all = page.getByRole("button", { name: "All posts" });
    await all.focus();
    await page.keyboard.press("Enter");
    await expect(page).toHaveURL(/filter=all/);
    await expect(all).toBeFocused();
    // A new page still moves focus to its heading.
    await page.getByRole("link", { name: WELCOME }).click();
    await expect(page.getByRole("heading", { level: 1, name: WELCOME })).toBeFocused();
  });

  test("footnote references and back-links are reachable with Tab and work with Enter", async ({
    page,
  }) => {
    await open(page, WELCOME_PATH, () =>
      expect(page.getByRole("heading", { level: 1, name: WELCOME })).toBeVisible(),
    );
    const body = page.locator("[data-rendered-root]").first();
    const ref = body.locator('sup a[data-anchor="md-fn-2"]');
    await expect(ref).toHaveAttribute("tabindex", "0");
    await expect(ref).toHaveAttribute("role", "link");
    // Tab from the link just before it lands on it.
    await body.locator('sup a[data-anchor="md-fn-1"]').focus();
    await page.keyboard.press("Tab");
    await expect(ref).toBeFocused();
    await page.keyboard.press("Enter");
    // Focus follows to the footnote (or its sidenote copy on wide screens).
    await expect
      .poll(() =>
        page.evaluate(() => {
          const a = document.activeElement;
          return a?.id === "md-fn-2" || a?.getAttribute("data-sidenote") === "2";
        }),
      )
      .toBe(true);
  });

  test("visually hidden inputs in chips and the file picker show keyboard focus", async ({
    page,
  }) => {
    const outlineOfFocusedParent = () =>
      page.evaluate(() => {
        const label = document.activeElement?.closest("label");
        return label ? getComputedStyle(label).outlineStyle : "no label";
      });
    await open(page, `${FORUM_PATH}/new`, () =>
      expect(page.getByRole("radio", { name: "Article" })).toBeVisible(),
    );
    await page.getByRole("radio", { name: "Article" }).focus();
    await page.keyboard.press("ArrowRight");
    await expect(page.getByRole("radio", { name: "Question" })).toBeFocused();
    expect(await outlineOfFocusedParent()).toBe("solid");
    await open(page, "/settings", () =>
      expect(page.getByRole("button", { name: "Export local state" })).toBeVisible(),
    );
    await page.getByRole("button", { name: "Export local state" }).focus();
    await page.keyboard.press("Tab");
    await expect(page.locator('input[type="file"]')).toBeFocused();
    expect(await outlineOfFocusedParent()).toBe("solid");
  });

  test("the markdown editor has an accessible name", async ({ page }) => {
    await open(page, WELCOME_PATH, () =>
      expect(page.getByRole("heading", { level: 1, name: WELCOME })).toBeVisible(),
    );
    await expect(page.getByRole("textbox", { name: "New comment" })).toHaveCount(1);
    await expect(page.getByTestId("composer-editor").first()).toMatchAriaSnapshot(
      '- textbox "New comment"',
    );
  });
});

test.describe("small screens", () => {
  test.use({ viewport: { width: 360, height: 740 }, hasTouch: true, isMobile: true });

  test("no page scrolls horizontally at 360 px", async ({ page }) => {
    const paths = [
      "/",
      FORUM_PATH,
      `${FORUM_PATH}/mod`,
      `${FORUM_PATH}/members`,
      `${FORUM_PATH}/settings`,
      WELCOME_PATH,
      "/settings",
      "/notifications",
      // An unknown comment: the error view shows a long at-uri.
      "/at/did:plc:5qdku4cwkqmf5f7vo5owwvbz/online.7778777.lessdugin.temp.comment/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ];
    const wide: string[] = [];
    for (const path of paths) {
      await page.goto(`/?mock#${path}`);
      await expect(page.getByRole("heading", { level: 1 }).first()).toBeVisible();
      await page.waitForLoadState("networkidle");
      const { sw, cw } = await page.evaluate(() => ({
        sw: document.documentElement.scrollWidth,
        cw: document.documentElement.clientWidth,
      }));
      if (sw > cw) wide.push(`${path}: ${sw} > ${cw}`);
    }
    expect(wide).toEqual([]);
  });
});
