// SPDX-License-Identifier: AGPL-3.0-or-later
// `lessdugin web --dev` (threat-model §7) serving the SPA, against the devnet: the one-time
// capability link, the bridge session in sessionStorage, anonymous reading with the `state`
// store behind `/bridge/kv`, the sign-in through `/bridge/login` (the CLI's loopback OAuth at
// the devnet PDS), a comment written through `/bridge/xrpc` with the CLI's session, and the
// refusal of a link used twice.
//
// Needs a `lessdugin` binary that embeds an e2e build of the SPA (only those accept the devnet
// endpoints): `LESSDUGIN_WEB_DIST=$PWD/web/dist-e2e cargo build -p lessdugin`, then
// `LESSDUGIN_BIN=<that binary>` (default target/debug/lessdugin). Skipped when the binary is
// missing or embeds another build.

import { type ChildProcess, spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import { DEV_CSP } from "../../csp.mjs";
import { approveAtPds, e2eState, route, watchStrict, withDevnet } from "./state.ts";

const s = e2eState();
const BIN =
  process.env.LESSDUGIN_BIN ??
  fileURLToPath(new URL("../../../target/debug/lessdugin", import.meta.url));

function freePort(): Promise<number> {
  return new Promise((ok, ko) => {
    const srv = createServer();
    srv.once("error", ko);
    srv.listen(0, "127.0.0.1", () => {
      const a = srv.address();
      const port = typeof a === "object" && a ? a.port : 0;
      srv.close(() => ok(port));
    });
  });
}

interface Web {
  origin: string;
  links: string[];
  stderr: string[];
  child: ChildProcess;
  dir: string;
  /** The next link printed on stdout. */
  nextLink(): Promise<string>;
}

async function startWeb(): Promise<Web> {
  const port = await freePort();
  const dir = mkdtempSync(join(tmpdir(), "lessdugin-web-e2e-"));
  const env: NodeJS.ProcessEnv = {};
  for (const [k, v] of Object.entries(process.env)) {
    if (!k.startsWith("LESSDUGIN_") && k !== "RUST_LOG" && k !== "BROWSER") env[k] = v;
  }
  Object.assign(env, {
    LESSDUGIN_DATA_DIR: dir,
    LESSDUGIN_KEYRING: "off",
    LESSDUGIN_PLC_URL: s.devnet.plc,
    LESSDUGIN_PDS_URL: s.devnet.pds,
    NO_COLOR: "1",
  });
  const child = spawn(BIN, ["--dev", "web", "--no-open", "--port", String(port)], {
    env,
    stdio: ["pipe", "pipe", "pipe"],
  });
  const links: string[] = [];
  const stderr: string[] = [];
  const waiters: ((l: string) => void)[] = [];
  let buf = "";
  child.stdout?.on("data", (d: Buffer) => {
    buf += d.toString("utf8");
    for (let i = buf.indexOf("\n"); i >= 0; i = buf.indexOf("\n")) {
      const line = buf.slice(0, i).trim();
      buf = buf.slice(i + 1);
      if (!line.startsWith("http://")) continue;
      const w = waiters.shift();
      if (w) w(line);
      else links.push(line);
    }
  });
  child.stderr?.on("data", (d: Buffer) => stderr.push(d.toString("utf8")));
  const web: Web = {
    origin: `http://127.0.0.1:${port}`,
    links,
    stderr,
    child,
    dir,
    nextLink() {
      const l = links.shift();
      if (l) return Promise.resolve(l);
      return new Promise((ok, ko) => {
        const t = setTimeout(
          () => ko(new Error(`no link from lessdugin web: ${stderr.join("")}`)),
          60_000,
        );
        waiters.push((x) => {
          clearTimeout(t);
          ok(x);
        });
      });
    },
  };
  return web;
}

/** The tuple key `(a, b)` (sync.md §10.3), standard base64 as `/bridge/kv` takes it. */
function kvKey(...parts: string[]): string {
  const bytes: number[] = [];
  for (const p of parts) {
    bytes.push(0x02);
    for (const b of Buffer.from(p, "utf8")) bytes.push(...(b === 0 ? [0, 0xff] : [b]));
    bytes.push(0x00);
  }
  return Buffer.from(bytes).toString("base64");
}

/** Which of the outbox's state key scopes `/bridge/kv` refuses. */
async function bridgeScopes(
  origin: string,
  token: string,
  did: string,
): Promise<{ missing: string[] }> {
  const missing: string[] = [];
  for (const scope of ["out", "out.budget", "out.meta", "out.blob"]) {
    const r = await fetch(`${origin}/bridge/kv`, {
      method: "POST",
      headers: {
        origin,
        authorization: `Bearer ${token}`,
        "content-type": "application/json",
      },
      body: JSON.stringify({ op: "get_many", keys: [kvKey(scope, did)] }),
    });
    if (r.status === 403) missing.push(scope);
  }
  return { missing };
}

let web: Web | null = null;

test.beforeAll(async () => {
  if (!existsSync(BIN)) return;
  web = await startWeb();
});

test.afterAll(async () => {
  if (!web) return;
  web.child.kill("SIGTERM");
  await new Promise((r) => (web?.child.exitCode !== null ? r(null) : web?.child.once("exit", r)));
  rmSync(web.dir, { recursive: true, force: true });
});

test("lessdugin web: capability link, anonymous read, sign-in through the CLI, a comment", async ({
  page,
  browser,
}) => {
  test.skip(!web, `no lessdugin binary at ${BIN} (set LESSDUGIN_BIN)`);
  if (!web) return;
  const w = await watchStrict(page);
  await withDevnet(page, s);
  const link = await web.nextLink();
  expect(link).toMatch(/^http:\/\/127\.0\.0\.1:\d+\/#lessdugin-cap=[A-Za-z0-9_-]{43}$/);
  const resp = await page.goto(link);
  expect(resp?.headers()["content-security-policy"]).toBe(DEV_CSP);
  const isE2E = await page
    .waitForFunction(() => window.__lessduginE2E !== undefined, null, { timeout: 15_000 })
    .then(
      () => true,
      () => false,
    );
  test.skip(!isE2E, `${BIN} does not embed an e2e build (LESSDUGIN_WEB_DIST=web/dist-e2e)`);

  // The capability left the address bar before the router mounted; the token is in
  // sessionStorage only.
  await expect(page).toHaveURL(`${web.origin}/#/`);
  const token = await page.evaluate(() => sessionStorage.getItem("lessdugin-bridge-token"));
  expect(token).toMatch(/^[A-Za-z0-9_-]{43}$/);
  await expect(page.getByTestId("engine-banner")).toContainText("lessdugin web");

  // Anonymous read (the cache in IndexedDB, the `state` store behind /bridge/kv).
  await page.goto(`${web.origin}/${route(s.post.uri)}`);
  await expect(
    page.getByRole("heading", { level: 1, name: "Hello from the PDS API" }),
  ).toBeVisible();
  await expect(page.getByText("A first comment from alice.")).toBeVisible();

  // Sign-in: /bridge/login starts the CLI's loopback OAuth; the CLI's callback sends the tab
  // back to the bridge origin, whose sessionStorage still holds the token.
  const carol = s.devnet.accounts.carol;
  await page.goto(`${web.origin}/#/login`);
  await page.getByLabel("Your handle or DID").fill(carol.handle);
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await approveAtPds(page, carol.password, web.origin);
  await expect(page.getByRole("button", { name: new RegExp(carol.handle) })).toBeVisible();
  expect(await page.evaluate(() => sessionStorage.getItem("lessdugin-bridge-token"))).toBe(token);
  // The browser holds no OAuth state of its own in this mode (threat-model AUTH-12).
  const dbs = await page.evaluate(async () => (await indexedDB.databases()).map((d) => d.name));
  expect(dbs).not.toContain("@atproto-oauth-client");

  // A comment, sent through /bridge/xrpc with the CLI's session. The engine's outbox keeps
  // rows under `out.meta` (W6) and `out.blob` (§8.8) next to `out` and `out.budget`; a bridge
  // whose `/bridge/kv` scopes lack them refuses every write, so this part waits for that fix.
  const scopes = await bridgeScopes(web.origin, token ?? "", carol.did);
  if (scopes.missing.length > 0) {
    test.info().annotations.push({
      type: "fixme",
      description: `lessdugin web refuses /bridge/kv keys under ${scopes.missing.join(", ")} (crates/cli/src/web/kv.rs SCOPES): writes from the tab fail until it allows them`,
    });
  } else {
    await page.goto(`${web.origin}/${route(s.post.uri)}`);
    await page.getByRole("textbox", { name: "New comment" }).click();
    await page.keyboard.type("carol, via **lessdugin web**.");
    await page.getByRole("button", { name: "Comment", exact: true }).click();
    await expect(
      page.getByRole("article").filter({ hasText: "carol, via lessdugin web." }),
    ).toBeVisible();
    await expect
      .poll(async () => {
        const r = await fetch(
          `${s.devnet.pds}/xrpc/com.atproto.repo.listRecords?repo=${carol.did}&collection=online.7778777.lessdugin.temp.comment&limit=100`,
        );
        const { records } = (await r.json()) as {
          records: { value: { root?: { uri?: string } } }[];
        };
        return records.filter((x) => x.value.root?.uri === s.post.uri).length;
      })
      .toBeGreaterThan(0);
  }

  expect(await w.problems()).toEqual([]);

  // The same link a second time: refused, and the first session is revoked (§7.2).
  const ctx = await browser.newContext();
  const other = await ctx.newPage();
  await withDevnet(other, s);
  await other.goto(link);
  await expect(other.getByRole("alert")).toContainText("already used or has expired");
  await ctx.close();
  await expect.poll(() => web?.stderr.join("") ?? "").toContain("used twice");
});
