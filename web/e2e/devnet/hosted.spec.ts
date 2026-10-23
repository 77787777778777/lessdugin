// SPDX-License-Identifier: AGPL-3.0-or-later
// The SPA on the real wasm engine against the devnet, hosted mode (the static site's code path,
// served with the §6.2 development CSP, Trusted Types enforced): anonymous reading of a forum
// written straight to the PDS, OAuth sign-in through the PDS's own authorization server, a
// comment, a new post and an edit with a revision. Every test fails on any CSP or Trusted Types
// violation and on any page or console error.

import { expect, test } from "@playwright/test";
import { DEV_CSP } from "../../csp.mjs";
import { e2eState, route, type StrictWatch, signIn, watchStrict, withDevnet } from "./state.ts";

const s = e2eState();
let w: StrictWatch;

/** The records of `collection` in `did`'s repository, straight from the PDS. */
async function listRecords(
  did: string,
  collection: string,
): Promise<{ uri: string; value: Record<string, unknown> }[]> {
  const r = await fetch(
    `${s.devnet.pds}/xrpc/com.atproto.repo.listRecords?repo=${did}&collection=online.7778777.lessdugin.temp.${collection}&limit=100`,
  );
  return ((await r.json()) as { records: { uri: string; value: Record<string, unknown> }[] })
    .records;
}

test.beforeEach(async ({ page }) => {
  w = await watchStrict(page);
  await withDevnet(page, s);
});

test.afterEach(async ({ page }) => {
  expect(await w.problems()).toEqual([]);
  // Trusted Types are really enforced, so a clean run means something.
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

test("anonymous read: forum, post list, post and its thread", async ({ page }) => {
  const resp = await page.goto(`${s.app}/${route(s.forum.uri)}`);
  expect(resp?.headers()["content-security-policy"]).toBe(DEV_CSP);
  await expect(page.getByRole("heading", { level: 1, name: "Seeded Forum" })).toBeVisible();
  await expect(page.getByText("Written straight to the PDS")).toBeVisible();
  await page.getByRole("link", { name: "Hello from the PDS API" }).click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Hello from the PDS API" }),
  ).toBeVisible();
  // The body went through the renderer and the `lessdugin-md` policy.
  await expect(page.locator("[data-rendered-root] strong").first()).toHaveText("createRecord");
  await expect(page.getByText("A first comment from alice.")).toBeVisible();
  await expect(page.getByText("Sign in to comment.")).toBeVisible();

  // The debug hook (e2e builds only) returns the engine's dto/1 text.
  const thread = await page.evaluate((uri) => window.__lessduginE2E?.getThread(uri), s.post.uri);
  expect(thread?.startsWith('{"dto":1,')).toBe(true);
  const parsed = JSON.parse(thread ?? "{}") as { comments: { uri: string }[] };
  expect(parsed.comments.map((c) => c.uri)).toEqual([s.comment.uri]);
  const list = await page.evaluate(
    (forum) => window.__lessduginE2E?.listPosts({ forum }),
    s.forum.uri,
  );
  expect(JSON.parse(list ?? "{}").items.map((p: { uri: string }) => p.uri)).toEqual([s.post.uri]);
});

test("an OAuth callback that does not match a started sign-in is refused on the sign-in page", async ({
  page,
}) => {
  w = await watchStrict(page);
  await page.goto(
    `${s.app}/?state=not-a-real-state&code=cod-x&iss=${encodeURIComponent(s.devnet.pds)}#/`,
  );
  await expect(page).toHaveURL(`${s.app}/#/login`);
  await expect(page.getByRole("alert")).toContainText("The sign-in did not complete");
  await expect(page.getByRole("link", { name: "Sign in" })).toBeVisible();
});

test("bob signs in with OAuth, joins, comments, publishes a post and edits it (a revision)", async ({
  page,
}) => {
  const bob = s.devnet.accounts.bob;
  await signIn(page, s, "bob");
  // The session is in the OAuth library's IndexedDB with a non-extractable DPoP key (AUTH-6).
  const keyInfo = await page.evaluate(async () => {
    const db = await new Promise<IDBDatabase>((ok, ko) => {
      const r = indexedDB.open("@atproto-oauth-client");
      r.onsuccess = () => ok(r.result);
      r.onerror = () => ko(r.error);
    });
    const found: { extractable: boolean; type: string }[] = [];
    const visit = (v: unknown, depth: number): void => {
      if (depth > 6 || v === null || typeof v !== "object") return;
      if (v instanceof CryptoKey) {
        found.push({ extractable: v.extractable, type: v.type });
        return;
      }
      for (const x of Object.values(v as Record<string, unknown>)) visit(x, depth + 1);
    };
    for (const name of Array.from(db.objectStoreNames)) {
      const all = await new Promise<unknown[]>((ok, ko) => {
        const r = db.transaction(name).objectStore(name).getAll();
        r.onsuccess = () => ok(r.result as unknown[]);
        r.onerror = () => ko(r.error);
      });
      for (const v of all) visit(v, 0);
    }
    db.close();
    return found;
  });
  const priv = keyInfo.filter((k) => k.type === "private");
  expect(priv.length).toBeGreaterThan(0);
  expect(priv.every((k) => !k.extractable)).toBe(true);
  const accounts = JSON.parse(
    (await page.evaluate(() => window.__lessduginE2E?.accounts())) ?? "[]",
  ) as { did: string; authKind: string }[];
  expect(accounts).toEqual([expect.objectContaining({ did: bob.did, authKind: "oauth" })]);

  // Joining: the public-action interstitial, then the membership record.
  await page.goto(`${s.app}/${route(s.forum.uri)}`);
  await expect(page.getByRole("heading", { level: 1, name: "Seeded Forum" })).toBeVisible();
  await page.getByRole("button", { name: "Join", exact: true }).click();
  const ack = page.getByRole("dialog", { name: "Membership is public" });
  await ack.getByRole("button", { name: "I understand, continue" }).click();
  await expect(page.getByText("Joined (your membership is public)")).toBeVisible();
  await expect(page.getByRole("button", { name: "Join", exact: true })).toBeHidden();
  expect(
    (await listRecords(bob.did, "membership")).filter((r) => r.value.forum === s.forum.uri),
  ).toHaveLength(1);

  // A comment on alice's post.
  await page.goto(`${s.app}/${route(s.post.uri)}`);
  await expect(
    page.getByRole("heading", { level: 1, name: "Hello from the PDS API" }),
  ).toBeVisible();
  await page.getByRole("textbox", { name: "New comment" }).click();
  await page.keyboard.type("Hello alice, this is **bob** from the SPA.");
  await page.getByRole("button", { name: "Comment", exact: true }).click();
  const mine = page.getByRole("article").filter({ hasText: "this is bob from the SPA" });
  await expect(mine).toBeVisible();
  await expect(mine.locator("strong")).toHaveText("bob");
  // It reached bob's repository (not only the local cache).
  await expect
    .poll(async () => {
      const records = await listRecords(bob.did, "comment");
      return records.filter((r) => (r.value.root as { uri?: string })?.uri === s.post.uri).length;
    })
    .toBe(1);

  // A new post in alice's forum.
  await page.goto(`${s.app}/${route(s.forum.uri)}/new`);
  await page.getByLabel("Title").fill("Bob's first post");
  await page.getByRole("textbox", { name: "Post body" }).click();
  await page.keyboard.type("Written in the browser, signed by **DPoP**.");
  await page.getByRole("button", { name: "Publish", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Bob's first post" })).toBeVisible();
  await expect(page.locator("[data-rendered-root] strong").first()).toHaveText("DPoP");
  const m = /#\/p\/(did:plc:[a-z2-7]{24})\/([a-z2-7-]+)$/.exec(page.url());
  expect(m?.[1]).toBe(bob.did);
  const postUri = `at://${m?.[1]}/online.7778777.lessdugin.temp.post/${m?.[2]}`;

  // The edit: applyWrites [create revision, update post] (data-model.md §8.3).
  await page.getByRole("link", { name: "Edit", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Edit post" })).toBeVisible();
  await page.getByLabel("Title").fill("Bob's first post (edited)");
  await page.getByLabel("Edit summary (optional, public)").fill("typo");
  await page.getByRole("button", { name: "Save edit" }).click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Bob's first post (edited)" }),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "History (1 revision)" })).toBeVisible();
  const history = JSON.parse(
    (await page.evaluate((uri) => window.__lessduginE2E?.history(uri), postUri)) ?? "{}",
  ) as { versions: { n: number }[] };
  expect(history.versions.length).toBe(2);
  const revisions = await listRecords(bob.did, "revision");
  expect(
    revisions.filter((r) => (r.value.subject as { uri?: string })?.uri === postUri),
  ).toHaveLength(1);
});
