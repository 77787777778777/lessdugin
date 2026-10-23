// SPDX-License-Identifier: AGPL-3.0-or-later
// What the global setup hands to the tests, and the helpers they share: opening the e2e build
// with the devnet options, the OAuth sign-in at the devnet PDS, and a strict watch for CSP and
// Trusted Types violations and page errors.

import { readFileSync } from "node:fs";
import { expect, type Page } from "@playwright/test";

export interface DevnetAccount {
  handle: string;
  did: string;
  password: string;
  appPassword: string;
}

export interface DevnetInfo {
  pds: string;
  plc: string;
  accounts: Record<"alice" | "bob" | "carol", DevnetAccount>;
}

export interface E2EState {
  devnet: DevnetInfo;
  /** The static server of dist-e2e (development CSP). */
  app: string;
  forum: { uri: string; slug: string; owner: string };
  post: { uri: string; rkey: string };
  comment: { uri: string };
}

export function e2eState(): E2EState {
  const path = process.env.LESSDUGIN_SPA_E2E_STATE;
  if (!path) throw new Error("LESSDUGIN_SPA_E2E_STATE is not set (global setup did not run)");
  return JSON.parse(readFileSync(path, "utf8")) as E2EState;
}

/** Hash route of a record or forum uri. */
export function route(uri: string): string {
  const m = /^at:\/\/([^/]+)\/([^/]+)\/(.+)$/.exec(uri);
  if (!m) throw new Error(`not an at-uri: ${uri}`);
  const [, did, coll, rkey] = m;
  if (coll?.endsWith(".forum")) return `#/f/${did}/${rkey}`;
  if (coll?.endsWith(".post")) return `#/p/${did}/${rkey}`;
  if (coll?.endsWith(".comment")) return `#/c/${did}/${rkey}`;
  return `#/at/${uri.slice(5)}`;
}

export interface StrictWatch {
  /** Page errors, console errors and policy violations so far. */
  problems(): Promise<string[]>;
}

/** Records page errors, console errors and `securitypolicyviolation` events. Chromium logs a
 * console error for every HTTP error status a request gets ("Failed to load resource"); the
 * OAuth client expects some (`use_dpop_nonce` 400/401 before its retry), so those network
 * lines are not app errors and are left out. */
export async function watchStrict(page: Page): Promise<StrictWatch> {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
  page.on("console", (m) => {
    if (m.type() !== "error") return;
    const text = m.text();
    if (/^Failed to load resource: the server responded with a status of \d+/.test(text)) return;
    errors.push(`console: ${text}`);
  });
  await page.addInitScript(() => {
    const w = window as unknown as { __cspViolations?: string[] };
    w.__cspViolations = [];
    document.addEventListener("securitypolicyviolation", (e) => {
      w.__cspViolations?.push(`${e.violatedDirective} blocked=${e.blockedURI} sample=${e.sample}`);
    });
  });
  return {
    async problems() {
      const v = await page
        .evaluate(() => (window as unknown as { __cspViolations?: string[] }).__cspViolations ?? [])
        .catch(() => [] as string[]);
      return [...errors, ...v.map((x) => `csp: ${x}`)];
    },
  };
}

/** Passes the devnet (and optionally the parity time) to the e2e build before it loads. */
export async function withDevnet(
  page: Page,
  s: E2EState,
  extra: { parityNowMs?: number } = {},
): Promise<void> {
  await page.addInitScript(
    (o) => {
      (window as unknown as { __LESSDUGIN_E2E__: unknown }).__LESSDUGIN_E2E__ = o;
    },
    { devnet: { plc: s.devnet.plc, pds: s.devnet.pds }, ...extra },
  );
}

/** Signs in and approves at the devnet PDS (its own authorization server); the browser is on
 * the PDS's pages in between and comes back to `origin`. */
export async function approveAtPds(page: Page, password: string, origin: string): Promise<void> {
  await page.locator("input[name=password]").fill(password, { timeout: 30_000 });
  await page.locator("button[type=submit]").click();
  await page.getByRole("button", { name: "Authorize" }).click({ timeout: 30_000 });
  await page.waitForURL((u) => u.origin === origin, { timeout: 30_000 });
}

/** The whole sign-in from the app's login page. */
export async function signIn(
  page: Page,
  s: E2EState,
  who: keyof DevnetInfo["accounts"],
  origin = s.app,
): Promise<void> {
  const acct = s.devnet.accounts[who];
  await page.goto(`${origin}/#/login`);
  await page.getByLabel("Your handle or DID").fill(acct.handle);
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await approveAtPds(page, acct.password, origin);
  await expect(page.getByRole("button", { name: new RegExp(acct.handle) })).toBeVisible();
}
