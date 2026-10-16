// SPDX-License-Identifier: AGPL-3.0-or-later
// Shared e2e helpers: fixture names and a watcher for page errors and CSP / Trusted Types
// violations.

import type { Page } from "@playwright/test";

export const FORUM = "LessDugin Test Forum";
export const WELCOME = "Welcome to LessDugin: a tour of the forum";
/** Routes of the deterministic mock fixture. */
export const FORUM_PATH = "/f/did:plc:5qdku4cwkqmf5f7vo5owwvbz/lessdugin-test";
export const WELCOME_PATH = "/p/did:plc:5qdku4cwkqmf5f7vo5owwvbz/yyxzy3wfando-3mxqo73ht2223";

export interface Watch {
  errors: string[];
  violations: () => Promise<string[]>;
}

/** Collects page errors, console errors and `securitypolicyviolation` events. */
export async function watch(page: Page): Promise<Watch> {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
  page.on("console", (m) => {
    if (m.type() === "error") errors.push(`console: ${m.text()}`);
  });
  await page.addInitScript(() => {
    const w = window as unknown as { __cspViolations: string[] };
    w.__cspViolations = [];
    document.addEventListener("securitypolicyviolation", (e) => {
      w.__cspViolations.push(`${e.violatedDirective} blocked=${e.blockedURI} sample=${e.sample}`);
    });
  });
  return {
    errors,
    violations: () =>
      page.evaluate(() => (window as unknown as { __cspViolations: string[] }).__cspViolations),
  };
}
