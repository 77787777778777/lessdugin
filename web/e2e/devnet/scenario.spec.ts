// SPDX-License-Identifier: AGPL-3.0-or-later
// The M2 parity scenario (docs/plan.md §4.2, text subset; M2 acceptance): three users on three
// clients against the devnet, every client in parity mode with the same fixed `now`
// (engine-api.md §2.1):
//
//   1. alice, CLI (`lessdugin --dev`): creates an approval-mode forum and approves bob and carol;
//   2. bob, SPA (this browser, OAuth at the devnet PDS): joins and posts;
//   3. carol, TUI (crates/tui/examples/parity_driver.rs, the headless TUI over the session the
//      CLI's `login` stored): opens the forum and the post, presses `c`, writes in $EDITOR;
//   6. bob, SPA: edits the post, which creates a revision.
//
// Alice also writes one post straight to her PDS whose TID and `createdAt` are an hour ahead of
// the real clock (the parity canary, conformance review M3): a client that ignored parity mode
// would mark it `futureDated` and rank it by when it first saw it; under the injected `now`
// and `obs` it is neither, in all three clients.
//
// Then each client reads as an anonymous reader on a fresh data directory or browser profile,
// syncing the forum as it does when a forum is opened: the CLI's `ls --json` and
// `read <post> --json`, the TUI's Forum and Thread screens (the PostList and ThreadView they
// hold), and the SPA's forum and post pages (the e2e debug hook's `listPosts` and `getThread`).
// The three MUST be byte-identical:
//
//   - the SPA hook and the TUI driver give the compact dto/1 text (`serde_json::to_string`, the
//     bytes the wasm boundary carries); the TUI driver also gives it pretty-printed exactly as
//     the CLI prints it (`to_string_pretty` and a newline);
//   - so: SPA == TUI compact, CLI == TUI pretty, and CLI with its layout whitespace removed ==
//     SPA (a lexical minify that leaves strings alone).
//
// Every output, the stderr of every process, the timings and (on a mismatch) unified diffs go to
// this test's output directory (test-results/web-e2e-devnet/…/parity), and the diffs are
// attached to the report.
//
// Needs the `lessdugin` binary (LESSDUGIN_BIN, default target/debug/lessdugin) and the TUI
// driver (LESSDUGIN_TUI_DRIVER, default target/debug/examples/parity_driver). `cargo xtask e2e
// --parity` builds everything and runs this test alone (packages/e2e/README.md). Without the
// binaries the test is skipped, except under CI or LESSDUGIN_PARITY_REQUIRED=1, where it fails.

import { execFile, spawnSync } from "node:child_process";
import {
  appendFileSync,
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, type Browser, type Page, type TestInfo, test } from "@playwright/test";
import { login as atpLogin, createPostAt, fk } from "./atp.mjs";
import { e2eState, route, signIn, watchStrict, withDevnet } from "./state.ts";

const s = e2eState();
const ROOT = fileURLToPath(new URL("../../../", import.meta.url));
const BIN = process.env.LESSDUGIN_BIN ?? join(ROOT, "target/debug/lessdugin");
const DRIVER =
  process.env.LESSDUGIN_TUI_DRIVER ?? join(ROOT, "target/debug/examples/parity_driver");
const REQUIRED = Boolean(process.env.CI) || process.env.LESSDUGIN_PARITY_REQUIRED === "1";
const MISSING = [BIN, DRIVER].filter((p) => !existsSync(p));

const FORUM_NAME = "Parity scenario";
const TITLE = "Bob's question";
const BODY = "Does this read the **same** everywhere?";
const EDITED_TITLE = "Bob's question (edited)";
const EDIT_TAIL = "Edited in the browser.";
const REPLY = "Carol answers from the TUI: it should.";
const CANARY_TITLE = "Dated an hour ahead";

interface Run {
  code: number | null;
  stdout: string;
  stderr: string;
  ms: number;
}

/** Runs a binary (stdin `input`), resolving with its exit code and output. */
function run(file: string, args: string[], env: NodeJS.ProcessEnv, input?: string): Promise<Run> {
  const t0 = Date.now();
  return new Promise((ok) => {
    const child = execFile(
      file,
      args,
      { env, maxBuffer: 64 * 1024 * 1024, timeout: 180_000, encoding: "utf8" },
      (err, stdout, stderr) => {
        const code = err ? (typeof err.code === "number" ? err.code : null) : 0;
        ok({ code, stdout, stderr, ms: Date.now() - t0 });
      },
    );
    child.stdin?.end(input ?? "");
  });
}

/** One client user: a data directory and the environment of `lessdugin --dev` (and of the TUI
 * driver, which opens the engine the same way). */
class User {
  readonly env: NodeJS.ProcessEnv;
  constructor(
    readonly name: string,
    readonly dir: string,
    parityNowMs: number,
    private readonly log: (name: string, r: Run) => void,
  ) {
    const env: NodeJS.ProcessEnv = {};
    for (const [k, v] of Object.entries(process.env)) {
      if (k.startsWith("LESSDUGIN_")) continue;
      if (["RUST_LOG", "VISUAL", "EDITOR", "BROWSER"].includes(k)) continue;
      env[k] = v;
    }
    Object.assign(env, {
      LESSDUGIN_DATA_DIR: dir,
      LESSDUGIN_KEYRING: "off",
      LESSDUGIN_PLC_URL: s.devnet.plc,
      LESSDUGIN_PDS_URL: s.devnet.pds,
      LESSDUGIN_SESSION: "parity",
      LESSDUGIN_PARITY_NOW_MS: String(parityNowMs),
      NO_COLOR: "1",
    });
    this.env = env;
  }

  /** `lessdugin --dev <args>`; must succeed. */
  async cli(label: string, args: string[], input?: string): Promise<string> {
    const r = await run(BIN, ["--dev", ...args], this.env, input);
    this.log(`${this.name}.cli.${label}`, r);
    expect(r.code, `${this.name}: lessdugin ${args.join(" ")}\n${r.stderr}`).toBe(0);
    return r.stdout;
  }

  /** The TUI driver; must succeed. */
  async tui(label: string, args: string[], extraEnv: NodeJS.ProcessEnv = {}): Promise<string> {
    const r = await run(DRIVER, args, { ...this.env, ...extraEnv });
    this.log(`${this.name}.tui.${label}`, r);
    expect(r.code, `${this.name}: parity_driver ${args.join(" ")}\n${r.stderr}`).toBe(0);
    return r.stdout;
  }

  async login(): Promise<void> {
    const acct = s.devnet.accounts[this.name as "alice" | "bob" | "carol"];
    await this.cli(
      "login",
      ["--json", "login", acct.handle, "--password-stdin"],
      `${acct.appPassword}\n`,
    );
  }
}

/** A done write's uri (`WriteTicket`, dto/1). */
function doneUri(ticketJson: string): string {
  const t = JSON.parse(ticketJson) as { state: { kind: string; uri?: string } };
  expect(t.state.kind, ticketJson).toBe("done");
  return t.state.uri ?? "";
}

/** serde_json's pretty output with its layout whitespace removed: the compact output, byte for
 * byte (both formatters escape strings the same way; only the pretty one adds whitespace, and
 * only outside strings). */
function minify(text: string): string {
  let out = "";
  let inString = false;
  let escaped = false;
  for (const ch of text) {
    if (inString) {
      out += ch;
      if (escaped) escaped = false;
      else if (ch === "\\") escaped = true;
      else if (ch === '"') inString = false;
    } else if (ch === '"') {
      inString = true;
      out += ch;
    } else if (ch !== " " && ch !== "\n" && ch !== "\r" && ch !== "\t") {
      out += ch;
    }
  }
  return out;
}

/** A dto/1 hook call in the page. */
async function hook(page: Page, op: string, args: unknown[]): Promise<string> {
  const text = await page.evaluate(
    ([o, a]) => window.__lessduginE2E?.callJson(o as string, a as unknown[]),
    [op, args] as const,
  );
  if (typeof text !== "string") throw new Error(`the debug hook is missing (${op})`);
  return text;
}

interface Outputs {
  thread: string;
  list: string;
}

/** Compares the three clients' outputs; on a mismatch writes unified diffs (attached). */
async function compare(
  what: "thread" | "list",
  cli: string,
  tui: { compact: string; pretty: string },
  spa: string,
  dir: string,
  info: TestInfo,
): Promise<string[]> {
  const problems: string[] = [];
  const firstDiff = (a: string, b: string): number => {
    let i = 0;
    while (i < a.length && i < b.length && a[i] === b[i]) i++;
    return i;
  };
  const diff = async (name: string, a: string, b: string, aText: string, bText: string) => {
    const pa = join(dir, `${name}.a.json`);
    const pb = join(dir, `${name}.b.json`);
    writeFileSync(pa, aText);
    writeFileSync(pb, bText);
    const r = spawnSync("diff", ["-u", "--label", a, "--label", b, pa, pb], { encoding: "utf8" });
    const body = r.stdout || r.error?.message || "(no diff output)";
    const path = join(dir, `${name}.diff`);
    writeFileSync(path, body);
    await info.attach(`${name}.diff`, { path, contentType: "text/plain" });
    return path;
  };
  const spaPretty = `${JSON.stringify(JSON.parse(spa), null, 2)}\n`;
  if (cli !== tui.pretty) {
    const at = firstDiff(cli, tui.pretty);
    const p = await diff(`${what}-cli-vs-tui`, "cli", "tui", cli, tui.pretty);
    problems.push(`${what}: CLI and TUI differ at byte ${at} (${p})`);
  }
  if (spa !== tui.compact) {
    const at = firstDiff(spa, tui.compact);
    // the SPA's compact text re-indented for reading (the comparison itself is on the bytes)
    const p = await diff(`${what}-spa-vs-tui`, "spa (re-indented)", "tui", spaPretty, tui.pretty);
    problems.push(`${what}: SPA and TUI differ at byte ${at} of the compact text (${p})`);
  }
  if (minify(cli) !== spa) {
    const at = firstDiff(minify(cli), spa);
    const p = await diff(`${what}-cli-vs-spa`, "cli", "spa (re-indented)", cli, spaPretty);
    problems.push(`${what}: CLI (minified) and SPA differ at byte ${at} (${p})`);
  }
  return problems;
}

test("M2 parity scenario: alice (CLI), bob (SPA) and carol (TUI); the three clients read the same bytes", async ({
  browser,
}, info) => {
  test.skip(
    MISSING.length > 0 && !REQUIRED,
    `missing ${MISSING.join(", ")}: run \`cargo xtask e2e --parity\` (or build them, packages/e2e/README.md)`,
  );
  expect(MISSING, "the binaries the scenario runs (cargo xtask e2e --parity builds them)").toEqual(
    [],
  );
  test.setTimeout(360_000);

  // docs/plan.md §4.2 "Variants: approval and open mode (M3)": the scenario runs once per
  // membership mode, and the three clients must agree byte for byte in both. Approval mode needs
  // the owner's grant before bob and carol can post; open mode lets them join and post at once,
  // and there is no "Approval required" banner to see.
  for (const mode of ["approval", "open"] as const) {
    await runScenario(browser, info, mode);
  }
});

/** One run of the parity scenario in one membership mode. */
async function runScenario(
  browser: Browser,
  info: TestInfo,
  mode: "approval" | "open",
): Promise<void> {
  const approval = mode === "approval";

  // the scenario's start plus a day, the same in every client (engine-api.md §2.1)
  const t = Date.now() + 86_400_000;
  const out = info.outputPath(`parity-${mode}`);
  mkdirSync(out, { recursive: true });
  const work = mkdtempSync(join(tmpdir(), "lessdugin-parity-"));
  const timings: { step: string; ms: number }[] = [];
  const logRun = (name: string, r: Run) => {
    writeFileSync(join(out, `${name}.stderr.txt`), r.stderr);
    timings.push({ step: `  ${name}`, ms: r.ms });
  };
  const step = async <T>(name: string, body: () => Promise<T>): Promise<T> => {
    const t0 = Date.now();
    try {
      return await test.step(name, body);
    } finally {
      timings.push({ step: name, ms: Date.now() - t0 });
    }
  };
  const user = (name: string) => new User(name, join(work, name), t, logRun);
  const alice = user("alice");
  const carol = user("carol");
  // the slug is per variant: the two runs share a devnet, so a reused slug would collide
  const slug = `parity-${mode}-${Date.now().toString(36)}`;
  let forum = "";
  let post = "";
  let reply = "";
  let canary = "";
  // step 4's settled opinion, kept so the report can show what the keys recorded
  let socialVote: unknown = null;
  const total0 = Date.now();

  try {
    // 1. alice, CLI: the forum in this variant's membership mode; in approval mode bob and carol
    // are approved before they can post
    await step(`1. alice (CLI): forum create (${mode}), ${approval ? "approve bob and carol" : "no grant needed"}`, async () => {
      await alice.login();
      forum = doneUri(
        await alice.cli("forum-create", [
          "--json",
          "forum",
          "create",
          slug,
          "--name",
          FORUM_NAME,
          "--mode",
          mode,
          "--frontpage",
        ]),
      );
      const show = JSON.parse(await alice.cli("forum-show", ["--json", "forum", "show", forum]));
      expect(show.forum.membership).toBe(mode);
      if (approval) {
        const approved = JSON.parse(
          await alice.cli("approve", [
            "--json",
            "--forum",
            forum,
            "approve",
            "bob.test",
            "carol.test",
          ]),
        ) as { writes: { state: { kind: string } }[] };
        expect(approved.writes.map((w) => w.state.kind)).toEqual(["done", "done"]);
      }
      // carol signs in with the CLI; the TUI uses that stored session (as `lessdugin tui`)
      await carol.login();
    });

    // the parity canary: alice's post dated an hour ahead of the real clock (well before `t`)
    await step("1b. alice (PDS API): a post dated an hour ahead", async () => {
      const session = await atpLogin(s.devnet.pds, s.devnet.accounts.alice);
      const r = await createPostAt(
        session,
        { uri: forum, fk: fk(forum) },
        {
          title: CANARY_TITLE,
          body: "Its TID and createdAt are an hour ahead.",
          atMs: Date.now() + 3_600_000,
        },
      );
      canary = r.uri;
    });

    // 2. bob, SPA: signs in (OAuth), joins, posts
    const ctx = await browser.newContext();
    const page = await ctx.newPage();
    const w = await watchStrict(page);
    await withDevnet(page, s, { parityNowMs: t });
    await step(`2. bob (SPA): sign in, join, post (${mode})`, async () => {
      await signIn(page, s, "bob");
      await page.goto(`${s.app}/${route(forum)}`);
      await expect(page.getByRole("heading", { level: 1, name: FORUM_NAME })).toBeVisible();
      if (approval) {
        // approval mode: the grant is already there, and the join still asks for the interstitial
        await expect(page.getByText("Approval required")).toBeVisible();
      } else {
        // open mode: no approval gate to announce
        await expect(page.getByText("Approval required")).toHaveCount(0);
      }
      await page.getByRole("button", { name: "Join", exact: true }).click();
      await page
        .getByRole("dialog", { name: "Membership is public" })
        .getByRole("button", { name: "I understand, continue" })
        .click();
      await expect(page.getByText("Joined (your membership is public)")).toBeVisible();
      await page.goto(`${s.app}/${route(forum)}/new`);
      await page.getByLabel("Title").fill(TITLE);
      await page.getByRole("textbox", { name: "Post body" }).click();
      await page.keyboard.type(BODY);
      await page.getByRole("button", { name: "Publish", exact: true }).click();
      await expect(page.getByRole("heading", { level: 1, name: TITLE })).toBeVisible();
      const m = /#\/p\/(did:plc:[a-z2-7]{24})\/([a-z2-7-]+)$/.exec(page.url());
      expect(m?.[1]).toBe(s.devnet.accounts.bob.did);
      post = `at://${m?.[1]}/online.7778777.lessdugin.temp.post/${m?.[2]}`;
      if (!approval) {
        // open mode applies V8: a participant outside S ∪ E is a NEWCOMER, and a newcomer's
        // content is Collapsed(NewUser) by default (social.md §6.5 V8, `display.newUsers`). Bob
        // has just arrived, so the owner's seed upvote is what makes him established
        // (E = S ∪ {seedUp ≥ establishSeeds}, §6.2) and his post visible to every client.
        // Without it the open variant would compare three empty views and prove nothing.
        await alice.cli("seed-vote", ["--json", "vote", post, "up"]);
        await alice.cli("sync", ["--json", "sync", forum]);
      }
    });

    // 3. carol, TUI: opens the forum and the post, `c`, writes the comment in $EDITOR
    await step("3. carol (TUI): reply through $EDITOR", async () => {
      const editor = join(work, "carol-editor.sh");
      const text = join(work, "carol-reply.txt");
      writeFileSync(text, `${REPLY}\n`);
      // the "user" types above the template's scissors line
      writeFileSync(
        editor,
        `#!/bin/sh\nset -e\n{ cat '${text}'; cat "$1"; } > "$1.new"\nmv "$1.new" "$1"\n`,
      );
      chmodSync(editor, 0o755);
      const r = JSON.parse(await carol.tui("reply", ["reply", forum, post], { EDITOR: editor }));
      reply = r.uri;
      expect(reply).toContain(`at://${s.devnet.accounts.carol.did}/`);
    });

    // 4. carol, TUI: votes, agrees and reacts THROUGH THE KEYS (docs/plan.md §4.2 step 4). The
    // driver presses `u`, `a` and `x` Enter and sends each write, so this proves the keys work
    // and that the opinion is settled (not held in the undo window) before the clients compare.
    await step("4. carol (TUI): votes, agrees and reacts", async () => {
      const v = JSON.parse(await carol.tui("votes", ["votes", forum, post]));
      expect(v.subject).toBe(post);
      expect(v.karma, `carol's vote: ${JSON.stringify(v)}`).toBeTruthy();
      expect(v.agreement, `carol's agreement: ${JSON.stringify(v)}`).toBeTruthy();
      // In approval mode a granted member is established, so the opinion counts. In open mode
      // carol is a newcomer too (V8, social.md §6.5), and a newcomer's vote is RECORDED but
      // carries no weight yet - "recorded; will count once you're established". Both are
      // correct, and the three clients must agree on whichever one applies.
      expect(v.not_yet_counted, `counted-ness in ${mode} mode`).toBe(!approval);
      socialVote = v;
      // The react record went through. Whether it forms a COUNTED tally depends on the mode:
      // approval mode establishes carol, so hers counts; in open mode she is a newcomer, and
      // "reacts by voters outside E are never counted" (social.md §8.7) - her position is still
      // recorded, which is what the TUI's uncounted marker shows. Either way the three clients
      // must agree, which the byte comparison below proves.
      const thread = JSON.parse(await carol.cli("read-after-votes", ["--json", "read", post])) as {
        post: { reacts: unknown[] };
      };
      if (approval) {
        expect(
          thread.post.reacts.length,
          `carol's react must form a counted tally: ${JSON.stringify(thread.post.reacts)}`,
        ).toBeGreaterThan(0);
      }
    });

    // 5. the moderator, CLI: hides carol's comment and pins the post (docs/plan.md §4.2 step 5).
    // alice owns the forum, so she is an authority; the hide is a modAction, the pin a forum
    // record update, and both must show up identically in all three clients.
    await step("5. alice (CLI): hides a comment and pins the post", async () => {
      const hidden = JSON.parse(
        await alice.cli("mod-hide", ["--json", "mod", "hide", reply, "--reason", "off topic"]),
      ) as unknown;
      expect(hidden).toBeTruthy();
      const pinned = JSON.parse(
        await alice.cli("mod-pin", [
          "--json",
          "mod",
          "pin",
          post,
          "--scope",
          "forum",
          "--reason",
          "good question",
        ]),
      ) as unknown;
      expect(pinned).toBeTruthy();
      await alice.cli("sync", ["--json", "sync", forum]);
      // the moderator's own view now shows the hide
      const log = JSON.parse(await alice.cli("mod-log", ["--json", "mod", "log", forum])) as {
        entries: { action: { $type?: string } }[];
      };
      expect(log.entries.length).toBeGreaterThan(0);
    });

    // 6. bob, SPA: edits the post (applyWrites [create revision, update post])
    await step("6. bob (SPA): edit the post (a revision)", async () => {
      await page.goto(`${s.app}/${route(post)}`);
      await expect(page.getByRole("heading", { level: 1, name: TITLE })).toBeVisible();
      await page.getByRole("link", { name: "Edit", exact: true }).click();
      await expect(page.getByRole("heading", { name: "Edit post" })).toBeVisible();
      await page.getByLabel("Title").fill(EDITED_TITLE);
      await page.getByRole("textbox", { name: "Post body" }).click();
      await page.keyboard.press("ControlOrMeta+End");
      await page.keyboard.type(`\n\n${EDIT_TAIL}`);
      await page.getByLabel("Edit summary (optional, public)").fill("clarified");
      await page.getByRole("button", { name: "Save edit" }).click();
      await expect(page.getByRole("heading", { level: 1, name: EDITED_TITLE })).toBeVisible();
      await expect(page.getByRole("button", { name: "History (1 revision)" })).toBeVisible();
      const flushed = JSON.parse(await hook(page, "outboxFlush", [])) as {
        failed: number;
        conflicts: number;
      };
      expect([flushed.failed, flushed.conflicts]).toEqual([0, 0]);
      expect(await w.problems()).toEqual([]);
    });
    await ctx.close();

    // the anonymous readers, each on a fresh data directory or profile
    const cli = user("anon-cli");
    const tuiReader = user("anon-tui");
    const cliOut: Outputs = { thread: "", list: "" };
    const tuiOut = { thread: { compact: "", pretty: "" }, list: { compact: "", pretty: "" } };
    const spaOut: Outputs = { thread: "", list: "" };

    await step("read: CLI (ls --json, read --json)", async () => {
      // the DEFAULT view (magic sort, frontpage filter since M3, engine-api.md §3.4), which is
      // what the TUI's Forum screen shows: the three clients must agree on the default, so none
      // of them pins a sort or filter the others do not.
      cliOut.list = await cli.cli("ls", ["--json", "ls", forum]);
      cliOut.thread = await cli.cli("read", ["--json", "read", post]);
    });

    await step("read: TUI (Forum and Thread screens)", async () => {
      const dir = join(out, "tui");
      await tuiReader.tui("read", ["read", forum, post, dir]);
      const f = (n: string) => readFileSync(join(dir, n), "utf8");
      tuiOut.thread = { compact: f("thread.json"), pretty: f("thread.pretty.json") };
      tuiOut.list = { compact: f("list.json"), pretty: f("list.pretty.json") };
    });

    await step("read: SPA (forum page, post page; debug hook)", async () => {
      const actx = await browser.newContext();
      const apage = await actx.newPage();
      const aw = await watchStrict(apage);
      await withDevnet(apage, s, { parityNowMs: t });
      await apage.goto(`${s.app}/${route(forum)}`);
      await expect(apage.getByRole("heading", { level: 1, name: FORUM_NAME })).toBeVisible();
      await apage.getByRole("link", { name: EDITED_TITLE }).click();
      await expect(apage.getByRole("heading", { level: 1, name: EDITED_TITLE })).toBeVisible();
      // carol's reply was HIDDEN by the moderator in step 5, so an anonymous reader must not see
      // its body; what matters here is that the three clients agree (compared below), not the
      // text itself.
      await expect(
        apage.getByText(REPLY),
        "a hidden comment must not show its body to an anonymous reader",
      ).toHaveCount(0);
      // the forum sync the page started has finished before the views are read
      await hook(apage, "syncForum", [forum, { full: false }]);
      spaOut.thread = await hook(apage, "getThread", [post, { sort: "magic", unreadOnly: false }]);
      spaOut.list = await hook(apage, "listPosts", [
        // the same default view as the CLI and the TUI (magic, frontpage)
        { forum, sort: "magic", filter: "frontpage", limit: 30 },
      ]);
      expect(await aw.problems()).toEqual([]);
      await actx.close();
    });

    for (const [name, text] of [
      ["cli.thread.json", cliOut.thread],
      ["cli.list.json", cliOut.list],
      ["spa.thread.json", spaOut.thread],
      ["spa.list.json", spaOut.list],
    ] as const) {
      writeFileSync(join(out, name), text);
    }

    // byte for byte (the diffs are written first, so a failure below still leaves them)
    const problems = [
      ...(await compare("thread", cliOut.thread, tuiOut.thread, spaOut.thread, out, info)),
      ...(await compare("list", cliOut.list, tuiOut.list, spaOut.list, out, info)),
    ];
    writeFileSync(
      join(out, "result.json"),
      `${JSON.stringify({ parityNowMs: t, forum, post, reply, identical: problems.length === 0, problems }, null, 2)}\n`,
    );
    expect(problems, `the clients disagree; diffs in ${out}`).toEqual([]);

    // and what the scenario wrote is in the view (equal outputs are not equally empty)
    const thread = JSON.parse(tuiOut.thread.compact);
    expect(thread.post.post.uri).toBe(post);
    expect(thread.post.post.title).toBe(EDITED_TITLE);
    expect(thread.post.post.author.handle).toBe("bob.test");
    expect(thread.post.body.text).toContain(EDIT_TAIL);
    expect(thread.post.history.revisions).toBe(1);
    // carol's reply was HIDDEN by step 5's mod action, and the fold drops a hidden comment
    // from the anonymous view entirely: it is not listed, and no part of its body survives. This
    // is as much a parity assertion as the byte comparison above - all three clients must agree
    // that the comment is gone.
    expect(
      thread.comments.map((c: { uri: string }) => c.uri),
      "a hidden comment must not be in the anonymous view",
    ).toEqual([]);
    expect(
      JSON.stringify(thread),
      "the hidden comment's body must not survive anywhere in the view",
    ).not.toContain(REPLY);
    // Which repos this thread still needs depends on the mode. In approval mode carol stays a
    // participant whose repo is needed even though her only comment is hidden, so the count is 3
    // (alice, bob, carol). In open mode a newcomer's hidden content takes her out of what the
    // thread needs, so it is 2 (alice, bob). Either way all three clients agree, which the byte
    // comparison above proves.
    expect(thread.completeness).toEqual({
      partial: false,
      reposComplete: approval ? 3 : 2,
      reposTotal: approval ? 3 : 2,
      degraded: false,
    });
    const list = JSON.parse(tuiOut.list.compact) as {
      items: {
        uri: string;
        createdAtMs: number;
        tRankMs: number;
        badges: Record<string, unknown>;
      }[];
    };
    expect(list.items.map((p) => p.uri).sort()).toEqual([post, canary].sort());
    // parity mode reached every client: under the injected `now` and `obs` the canary is not
    // future-dated and ranks by its own date (byte equality above holds it for all three)
    const c = list.items.find((p) => p.uri === canary);
    expect(c?.badges.futureDated).toBeUndefined();
    expect(c?.tRankMs).toBe(c?.createdAtMs);
  } finally {
    timings.push({ step: "total", ms: Date.now() - total0 });
    const table = timings.map((x) => `| ${x.step} | ${x.ms} |`).join("\n");
    writeFileSync(join(out, "timings.json"), `${JSON.stringify(timings, null, 2)}\n`);
    await info.attach("timings", {
      body: JSON.stringify(timings, null, 2),
      contentType: "application/json",
    });
    console.log(
      `parity scenario timings (ms):\n${timings.map((x) => `${x.step}: ${x.ms}`).join("\n")}`,
    );
    const summary = process.env.GITHUB_STEP_SUMMARY;
    if (summary) {
      appendFileSync(summary, `\n### M2 parity scenario\n\n| step | ms |\n|---|---|\n${table}\n`);
    }
    rmSync(work, { recursive: true, force: true });
  }
}
