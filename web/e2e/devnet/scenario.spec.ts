// SPDX-License-Identifier: AGPL-3.0-or-later
// The M2 parity scenario (docs/plan.md §4.2, text subset; M2 acceptance): three users on three
// clients against the devnet, every client in parity mode with the same fixed `now`
// (engine-api.md §2.1):
//
//   1. alice, CLI (`lessdugin --dev`): creates an approval-mode forum and approves bob and carol;
//   2. bob, SPA (this browser, OAuth at the devnet PDS): joins and posts;
//   3. carol, TUI (crates/tui/examples/parity_driver.rs, the headless TUI over the session the
//      CLI's `login` stored): opens the forum and the post, presses `c`, writes in $EDITOR;
//   4. carol, TUI: votes, agrees and reacts through the keys;
//   5. alice, CLI: hides carol's comment and pins the post;
//   6. bob, SPA: edits the post, which creates a revision;
//   6c. alice, SPA: applies the forum's wiki-backed tag to the post;
//   6b. alice, CLI: pins the edited post again (a pin is bound to the version it was made on,
//      social.md §4.5, so the edit suspended step 5's pin), so the compared views carry it;
//   7. carol, TUI: tags the post too (`:vote up --tag`); her vote counts in approval mode only.
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
// Sources (docs/plan.md §4.2 variants, §4.1; sync.md §2.2-§2.4, §4.4): both variants configure
// every client (config.toml, the SPA's source options) with the devnet's CORE+ services
// (crates/testkit/examples/devnet_services.rs: a mini-Jetstream serving both wires, with
// identity and account noise for unrelated DIDs, a Constellation and a relay whose pages are
// short and still carry a cursor, all over the devnet PDS's firehose). Only `sources` differs.
// Every reader must contact them in `assisted` and no client may contact them in `direct`
// (sync.md §2.4). A newcomer (a fresh account no seed references) joins and comments: in open
// mode only CORE+ can introduce it (sync.md §2.2, §4.4), so the assisted readers show the
// comment and the direct ones do not; in approval mode an applicant's content is never shown.
//
// Indexer (M6, plan §4.2 "indexer on and off"; decision U-D2): in the assisted variants
// `lessdugin-indexer --poll` follows the devnet from step 1 on. After the readers above, three
// more fresh readers (CLI, TUI, SPA) are configured with that indexer (configuration consent,
// sync.md §2.4) and no prior sync: the TUI and the SPA hydrate their cold forum page through
// the indexer's ordering and proof bundles. The SPA's cold page is read while every range
// listing of a repo other than the forum authority's is held (sync.md §14.3: the indexer, head
// checks, DID documents and the authority sync only), and must list the indexer-off list's
// posts in its order, a proof-admitted one at the same CID. Then the listings are released and
// each reader's ordinary forum sync (after its change-feed poll) runs; those converged views
// must equal the indexer-off views byte for byte once the provenance badges (verified, hint,
// stale) are removed.
//
// Needs the `lessdugin` binary (LESSDUGIN_BIN, default target/debug/lessdugin), the TUI
// driver (LESSDUGIN_TUI_DRIVER, default target/debug/examples/parity_driver), the devnet
// services (LESSDUGIN_DEVNET_SERVICES, default target/debug/examples/devnet_services) and the
// indexer (LESSDUGIN_INDEXER_BIN, default target/debug/lessdugin-indexer). `cargo xtask e2e
// --parity` builds everything and runs this test alone (packages/e2e/README.md). Without the
// binaries the test is skipped, except under CI or LESSDUGIN_PARITY_REQUIRED=1, where it fails.

import { type ChildProcess, execFile, spawn, spawnSync } from "node:child_process";
import {
  appendFileSync,
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { createServer as createHttpServer, request as httpRequest } from "node:http";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { type Browser, expect, type Page, type TestInfo, test } from "@playwright/test";
import {
  login as atpLogin,
  createComment,
  createPostAt,
  fk,
  NS,
  now,
  put,
  type Session,
  xrpc,
} from "./atp.mjs";
import { e2eState, route, signIn, watchStrict, withDevnet } from "./state.ts";

const s = e2eState();
const ROOT = fileURLToPath(new URL("../../../", import.meta.url));
const BIN = process.env.LESSDUGIN_BIN ?? join(ROOT, "target/debug/lessdugin");
const DRIVER =
  process.env.LESSDUGIN_TUI_DRIVER ?? join(ROOT, "target/debug/examples/parity_driver");
const SERVICES =
  process.env.LESSDUGIN_DEVNET_SERVICES ?? join(ROOT, "target/debug/examples/devnet_services");
const INDEXER = process.env.LESSDUGIN_INDEXER_BIN ?? join(ROOT, "target/debug/lessdugin-indexer");
const REQUIRED = Boolean(process.env.CI) || process.env.LESSDUGIN_PARITY_REQUIRED === "1";
const MISSING = [BIN, DRIVER, SERVICES, INDEXER].filter((p) => !existsSync(p));

const FORUM_NAME = "Parity scenario";
const TITLE = "Bob's question";
const BODY = "Does this read the **same** everywhere?";
// A real 8x8 PNG, attached in step 2 so the media subset (M4) goes through the engine.
const FIXTURE_PNG = fileURLToPath(new URL("../fixtures/pixel.png", import.meta.url));
const EDITED_TITLE = "Bob's question (edited)";
const EDIT_TAIL = "Edited in the browser.";
const REPLY = "Carol answers from the TUI: it should.";
const CANARY_TITLE = "Dated an hour ahead";
const TAG_SLUG = "parity";
const TAG_NAME = "Parity evidence";
const WIKI_BODY = "A real wiki page created for the M5 cross-client parity scenario.";
const NEWCOMER_REPLY = "A newcomer's comment: only CORE+ discovery can find it.";
/** The unrelated DIDs of the services' identity and account noise
 * (crates/testkit/src/devnet_services.rs, `NOISE_DIDS`). */
const NOISE_DIDS = [
  "did:plc:noiseaaaaaaaaaaaaaaaaaab",
  "did:plc:noiseaaaaaaaaaaaaaaaaaac",
  "did:plc:noiseaaaaaaaaaaaaaaaaaad",
];
/** The provenance badges decision U-D2 lets differ with and without the indexer. */
const PROVENANCE_BADGES = ["verified", "hint", "stale"];
/** An ordinary repo sync's collection and range listings at the PDS (sync.md §5.2, §5.3). */
const RANGE_LISTINGS = [
  "/xrpc/com.atproto.repo.describeRepo",
  "/xrpc/com.atproto.repo.listRecords",
];

type SourceMode = "direct" | "assisted";

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

/** Waits for the JSON file a child process writes once it is up. */
async function waitForJson<T>(path: string, child: ChildProcess, what: string): Promise<T> {
  const deadline = Date.now() + 60_000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) throw new Error(`${what} exited with ${child.exitCode}`);
    try {
      return JSON.parse(readFileSync(path, "utf8")) as T;
    } catch {
      await new Promise((r) => setTimeout(r, 50));
    }
  }
  throw new Error(`${what} did not start within 60 s`);
}

async function stop(child: ChildProcess): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return;
  const exited = new Promise((r) => child.once("exit", r));
  child.kill("SIGTERM");
  await exited;
}

/** The loopback URLs of the devnet's CORE+ services. */
interface CorePlus {
  jetstream: string;
  constellation: string;
  relay: string;
  control: string;
}

/** What the services answered (`/_devnet/log`). */
interface ServicesLog {
  http: { service: string; nsid: string; url: string; status: number }[];
  jetstream: {
    url: string;
    wire: "v1" | "v2";
    options: unknown;
    frames: number;
    noiseFrames: number;
  }[];
  mirror: { events: number; upstreamSeq: number; skipped: number; noise: number };
}

/** The CORE+ traffic between two logs. */
function since(a: ServicesLog, b: ServicesLog): Pick<ServicesLog, "http" | "jetstream"> {
  return { http: b.http.slice(a.http.length), jetstream: b.jetstream.slice(a.jetstream.length) };
}

/** `devnet_services --pds <pds>`: the CORE+ services of one variant. */
class Services {
  private constructor(
    readonly urls: CorePlus,
    private readonly child: ChildProcess,
  ) {}

  static async start(dir: string, pds: string): Promise<Services> {
    const json = join(dir, "services.json");
    const child = spawn(SERVICES, ["--pds", pds, "--json", json], {
      stdio: ["ignore", "ignore", "inherit"],
    });
    return new Services(await waitForJson<CorePlus>(json, child, "devnet_services"), child);
  }

  async log(): Promise<ServicesLog> {
    const res = await fetch(`${this.urls.control}/_devnet/log`);
    expect(res.status).toBe(200);
    return (await res.json()) as ServicesLog;
  }

  /** Waits until the firehose mirror holds `uri`, so every reader sees the same hints. */
  async waitForRecord(uri: string): Promise<void> {
    const deadline = Date.now() + 30_000;
    while (Date.now() < deadline) {
      const res = await fetch(`${this.urls.control}/_devnet/record?uri=${encodeURIComponent(uri)}`);
      if (res.status === 200) return;
      await new Promise((r) => setTimeout(r, 50));
    }
    throw new Error(`the services' firehose mirror never saw ${uri}`);
  }

  stop(): Promise<void> {
    return stop(this.child);
  }
}

async function freePort(): Promise<number> {
  return await new Promise((ok, fail) => {
    const server = createServer();
    server.once("error", fail);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      server.close(() => ok(typeof address === "object" && address ? address.port : 0));
    });
  });
}

/** `lessdugin-indexer --poll <pds> --forum <forum>` (M6): follows the devnet PDS. */
class Indexer {
  private constructor(
    readonly url: string,
    private readonly child: ChildProcess,
  ) {}

  static async start(dir: string, forum: string, out: string): Promise<Indexer> {
    mkdirSync(dir, { recursive: true });
    writeFileSync(
      join(dir, "config.toml"),
      [
        "doh = []",
        "",
        "[network]",
        'mode = "devnet"',
        `plc = ${JSON.stringify(s.devnet.plc)}`,
        `pds = ${JSON.stringify(s.devnet.pds)}`,
        "",
      ].join("\n"),
      { mode: 0o600 },
    );
    const port = await freePort();
    const url = `http://127.0.0.1:${port}`;
    const log = openSync(join(out, "indexer.log"), "w");
    const child = spawn(
      INDEXER,
      [
        "--listen",
        `127.0.0.1:${port}`,
        "--data-dir",
        dir,
        "--poll",
        s.devnet.pds,
        "--poll-interval-ms",
        "500",
        "--forum",
        forum,
      ],
      { stdio: ["ignore", log, log], env: { ...process.env, RUST_LOG: "info" } },
    );
    const deadline = Date.now() + 60_000;
    for (;;) {
      if (child.exitCode !== null)
        throw new Error(`lessdugin-indexer exited with ${child.exitCode}`);
      const up = await fetch(`${url}/_health`).then(
        (r) => r.ok,
        () => false,
      );
      if (up) return new Indexer(url, child);
      if (Date.now() > deadline) throw new Error("lessdugin-indexer did not start within 60 s");
      await new Promise((r) => setTimeout(r, 200));
    }
  }

  /** `lessdugin_indexer_http_requests_total` (this request included). */
  async requests(): Promise<number> {
    const text = await (await fetch(`${this.url}/metrics`)).text();
    const m = /^lessdugin_indexer_http_requests_total (\d+)$/m.exec(text);
    if (!m) throw new Error(`no request counter in the indexer metrics:\n${text}`);
    return Number(m[1]);
  }

  /** Waits until the indexer lists `required` among the forum's participants and its heads
   * of every participant it lists are the devnet PDS's (it converged). */
  async converge(forum: string, required: string[]): Promise<void> {
    const deadline = Date.now() + 60_000;
    let last = "";
    while (Date.now() < deadline) {
      const listed = await fetch(
        `${this.url}/xrpc/${NS}.index.listParticipants?${new URLSearchParams({ forum, limit: "100" })}`,
      ).then(
        async (r) =>
          r.ok
            ? ((await r.json()) as { participants: { did: string }[] }).participants.map(
                (p) => p.did,
              )
            : [],
        () => [] as string[],
      );
      const dids = [...new Set([...required, ...listed])].sort();
      const pds = await Promise.all(
        dids.map(
          async (did) =>
            (
              (await xrpc(s.devnet.pds, "com.atproto.sync.getLatestCommit", {
                query: { did },
              })) as { rev: string }
            ).rev,
        ),
      );
      const q = new URLSearchParams(dids.map((d) => ["dids", d]));
      const res = await fetch(`${this.url}/xrpc/${NS}.index.getRepoHeads?${q}`);
      const heads = res.ok
        ? ((await res.json()) as { heads: { did: string; rev?: string }[] }).heads
        : [];
      const indexed = dids.map((d) => heads.find((h) => h.did === d)?.rev ?? null);
      last = JSON.stringify({ dids, listed, pds, indexed });
      if (required.every((d) => listed.includes(d)) && indexed.every((rev, i) => rev === pds[i]))
        return;
      // the indexer allows 20 requests per second per address; stay well below
      await new Promise((r) => setTimeout(r, 500));
    }
    throw new Error(`the indexer did not converge on the devnet heads: ${last}`);
  }

  stop(): Promise<void> {
    return stop(this.child);
  }
}

/** A loopback proxy in front of `target` (the indexer) that records the path of every request,
 * so the scenario can tell which indexer methods one native reader used. */
class RecordingProxy {
  readonly paths: string[] = [];
  private constructor(
    readonly url: string,
    private readonly server: ReturnType<typeof createHttpServer>,
  ) {}

  static async start(target: string): Promise<RecordingProxy> {
    let proxy: RecordingProxy | null = null;
    const server = createHttpServer((req, res) => {
      const url = new URL(req.url ?? "/", target);
      proxy?.paths.push(url.pathname);
      const upstream = httpRequest(
        url,
        { method: req.method, headers: { ...req.headers, host: url.host } },
        (r) => {
          res.writeHead(r.statusCode ?? 502, r.headers);
          r.pipe(res);
        },
      );
      upstream.on("error", () => {
        res.writeHead(502);
        res.end();
      });
      req.pipe(upstream);
    });
    await new Promise<void>((ok) => server.listen(0, "127.0.0.1", ok));
    const address = server.address();
    const port = typeof address === "object" && address ? address.port : 0;
    proxy = new RecordingProxy(`http://127.0.0.1:${port}`, server);
    return proxy;
  }

  close(): Promise<void> {
    return new Promise((ok) => this.server.close(() => ok()));
  }
}

/** `text` (dto/1 JSON) without the provenance badges, re-serialised compactly or as
 * `serde_json::to_string_pretty` plus a newline. */
function withoutProvenance(text: string, pretty: boolean): string {
  const strip = (v: unknown): unknown => {
    if (Array.isArray(v)) return v.map(strip);
    if (typeof v !== "object" || v === null) return v;
    const out: Record<string, unknown> = {};
    for (const [k, x] of Object.entries(v)) {
      if (k === "badges" && typeof x === "object" && x !== null && !Array.isArray(x)) {
        out[k] = Object.fromEntries(
          Object.entries(x).filter(([b]) => !PROVENANCE_BADGES.includes(b)),
        );
      } else {
        out[k] = strip(x);
      }
    }
    return out;
  };
  const value = strip(JSON.parse(text));
  return pretty ? `${JSON.stringify(value, null, 2)}\n` : JSON.stringify(value);
}

/** True if a provenance badge is set anywhere in `text`. */
function hasProvenance(text: string, badge: string): boolean {
  let found = false;
  const walk = (v: unknown) => {
    if (Array.isArray(v)) v.forEach(walk);
    else if (typeof v === "object" && v !== null) {
      for (const [k, x] of Object.entries(v)) {
        if (k === "badges" && (x as Record<string, unknown>)?.[badge] === true) found = true;
        walk(x);
      }
    }
  };
  walk(JSON.parse(text));
  return found;
}

/** How a cold page (`{items: [{uri, cid, badges}]}`: the posts a page hydrated from the
 * indexer listed before any participant's repo was listed, in its order, with their local
 * views; "" when those could not be read) departs from the converged indexer-off list: other
 * posts or another order, a proof-admitted post at another version, or no proof-admitted
 * post at all. Its comment counts and scores still lack what only the participants' repos
 * hold, so they are not compared here (the converged views are, byte for byte). */
function coldPageProblems(cold: string, off: string): string[] {
  type Item = { uri: string; cid: string; badges?: Record<string, boolean> };
  if (cold === "") {
    return ["cold page: its posts' local views could not be read while the listings were held"];
  }
  const coldItems = (JSON.parse(cold) as { items: Item[] }).items;
  const offItems = (JSON.parse(off) as { items: Item[] }).items;
  const problems: string[] = [];
  const uris = (items: Item[]) => items.map((i) => i.uri).join(", ");
  if (uris(coldItems) !== uris(offItems)) {
    problems.push(
      `cold page: the indexer-hydrated posts [${uris(coldItems)}] are not the indexer-off list [${uris(offItems)}]`,
    );
  }
  const offCid = new Map(offItems.map((i) => [i.uri, i.cid]));
  const verified = coldItems.filter((i) => i.badges?.verified === true);
  for (const i of verified) {
    if (offCid.get(i.uri) !== i.cid) {
      problems.push(
        `cold page: ${i.uri} was admitted by proof at ${i.cid}, not ${offCid.get(i.uri)}`,
      );
    }
  }
  if (verified.length === 0) problems.push("cold page: no post was admitted by proof");
  return problems;
}

/** One client user: a data directory and the environment of `lessdugin --dev` (and of the TUI
 * driver, which opens the engine the same way). */
class User {
  readonly env: NodeJS.ProcessEnv;
  constructor(
    readonly name: string,
    readonly dir: string,
    parityNowMs: number,
    readonly sources: SourceMode,
    core: CorePlus,
    private readonly log: (name: string, r: Run) => void,
    indexer: string | null = null,
  ) {
    mkdirSync(dir, { recursive: true });
    // TOML has no null: false maps an optional service to None. Every endpoint is local, in
    // BOTH variants: the devnet's CORE+ services (the relay, the Constellation, and the
    // Jetstream of the devnet profile), and no Slingshot. Only `sources` differs, so direct
    // mode is what keeps a client from contacting them (sync.md §2.4). An explicit indexer URL
    // is the user's configuration consent (sync.md §2.4); every other client uses none.
    writeFileSync(
      join(dir, "config.toml"),
      [
        `sources = "${sources}"`,
        "doh = []",
        'plc = "http://127.0.0.1:9"',
        `relays = [${JSON.stringify(core.relay)}]`,
        "jetstream = []",
        `constellation = ${JSON.stringify(core.constellation)}`,
        "slingshot = false",
        indexer ? `indexer = [${JSON.stringify(indexer)}]` : 'indexer = "none"',
        "",
        "[network]",
        'mode = "devnet"',
        `plc = ${JSON.stringify(s.devnet.plc)}`,
        `pds = ${JSON.stringify(s.devnet.pds)}`,
        `jetstream = ${JSON.stringify(core.jetstream)}`,
        "",
      ].join("\n"),
      { mode: 0o600 },
    );
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

  /** Effective engine settings, not merely the file the harness wrote. */
  async verifySources(): Promise<void> {
    const settings = JSON.parse(
      await this.cli("config-sources", ["--json", "config", "get", "sources"]),
    ) as { settings: { sources: string }; pinned: string[] };
    expect(settings.settings.sources).toBe(this.sources);
    expect(settings.pinned).toContain("sources");
  }

  async login(): Promise<void> {
    const acct = s.devnet.accounts[this.name as "alice" | "bob" | "carol"];
    await this.cli(
      "login",
      ["--json", "login", acct.handle, "--password-stdin"],
      `${acct.appPassword}\n`,
    );
    await this.verifySources();
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

/** The fields of a dto/1 `MediaView` the scenario checks. */
interface MediaItem {
  cid: string;
  mime: string;
  author: string;
}

/** Compares the three clients' outputs; on a mismatch writes unified diffs (attached). */
async function compare(
  what: string,
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

test("M5 full parity scenario: approval/open × direct/assisted; CLI, SPA and TUI read the same bytes", async ({
  browser,
}, info) => {
  test.skip(
    MISSING.length > 0 && !REQUIRED,
    `missing ${MISSING.join(", ")}: run \`cargo xtask e2e --parity\` (or build them, packages/e2e/README.md)`,
  );
  expect(MISSING, "the binaries the scenario runs (cargo xtask e2e --parity builds them)").toEqual(
    [],
  );
  test.setTimeout(900_000);

  // docs/plan.md §4.2 "Variants: approval and open mode (M3)": the scenario runs once per
  // membership mode and source mode. Approval mode needs the owner's grant before bob and carol
  // can post; open mode lets them join and post at once. Both source variants use the same local
  // PDS/PLC and the same local CORE+ services; only `sources` differs. The assisted variants
  // also read with and without the indexer (M6).
  for (const mode of ["approval", "open"] as const) {
    for (const sources of ["direct", "assisted"] as const) {
      await runScenario(browser, info, mode, sources);
    }
  }
});

/** One run of the parity scenario in one membership mode. */
async function runScenario(
  browser: Browser,
  info: TestInfo,
  mode: "approval" | "open",
  sources: SourceMode,
): Promise<void> {
  const approval = mode === "approval";

  // the scenario's start plus a day, the same in every client (engine-api.md §2.1)
  const t = Date.now() + 86_400_000;
  const out = info.outputPath(`parity-${mode}-${sources}`);
  mkdirSync(out, { recursive: true });
  const work = mkdtempSync(join(tmpdir(), "lessdugin-parity-"));
  const timings: { step: string; ms: number }[] = [];
  const logRun = (name: string, r: Run) => {
    writeFileSync(join(out, `${name}.stdout.txt`), r.stdout);
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
  // this variant's CORE+ services; every client is configured with them (see User)
  const services = await Services.start(work, s.devnet.pds);
  let indexer: Indexer | null = null;
  const user = (name: string, indexerUrl: string | null = null) =>
    new User(name, join(work, name), t, sources, services.urls, logRun, indexerUrl);
  const alice = user("alice");
  const carol = user("carol");
  // the slug is per variant: all four runs share a devnet, so a reused slug would collide
  const slug = `parity-${mode}-${sources}-${Date.now().toString(36)}`;
  let forum = "";
  let wiki = "";
  let post = "";
  let reply = "";
  let canary = "";
  // the newcomer's comment (step 6d), which only CORE+ discovery can introduce in open mode
  let newcomerComment = "";
  let newcomer = "";
  // step 4's settled opinion, kept so the report can show what the keys recorded
  let socialVote: unknown = null;
  let cachedRender: { ms: number; blockedRequests: string[] } | null = null;
  // (read below, in the parity report)
  const total0 = Date.now();
  const spaSourceOptions = {
    sources,
    relays: [services.urls.relay],
    jetstream: [services.urls.jetstream],
    constellation: services.urls.constellation,
    slingshot: null,
  };
  // the CORE+ requests each reader made (the services' log, before and after its step)
  const traffic: Record<string, Pick<ServicesLog, "http" | "jetstream">> = {};
  const watched = async (client: string, body: () => Promise<void>): Promise<void> => {
    const before = await services.log();
    await body();
    traffic[client] = since(before, await services.log());
  };
  const spaRequests: Record<string, string[]> = {};
  const recordRequests = (name: string, page: Page) => {
    const seen: string[] = [];
    spaRequests[name] = seen;
    page.on("request", (r) => seen.push(`${r.method()} ${r.url()}`));
  };
  const expectLoopbackOnly = (name: string) => {
    const external = (spaRequests[name] ?? []).filter((line) => {
      const url = new URL(line.slice(line.indexOf(" ") + 1));
      return !["localhost", "127.0.0.1", "[::1]"].includes(url.hostname);
    });
    expect(external, `${name}: every request stays on loopback`).toEqual([]);
  };

  try {
    // 1. alice, CLI: the forum in this variant's membership mode; in approval mode bob and carol
    // are approved before they can post
    await step(
      `1. alice (CLI): forum create (${mode}), ${approval ? "approve bob and carol" : "no grant needed"}`,
      async () => {
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
        wiki = doneUri(
          await alice.cli(
            "wiki-create",
            [
              "--json",
              "wiki",
              "edit",
              TAG_SLUG,
              "--name",
              TAG_NAME,
              "--description",
              "Cross-client organisation evidence",
              "--forum",
              forum,
              "--file",
              "-",
            ],
            `${WIKI_BODY}\n`,
          ),
        );
        expect(wiki).toContain(`at://${s.devnet.accounts.alice.did}/`);
        const page = JSON.parse(
          await alice.cli("wiki-read", ["--json", "tag", TAG_SLUG, "--forum", forum]),
        ) as { head: string | null; page: { text: string } | null; tag: { slug: string } };
        expect(page.tag.slug).toBe(TAG_SLUG);
        expect(page.head).toBe(wiki);
        expect(page.page?.text).toContain(WIKI_BODY);
        await alice.cli("sync-after-wiki", ["--json", "sync", forum]);
        if (approval) {
          const approved = JSON.parse(
            await alice.cli("approve", [
              "--json",
              "--forum",
              forum,
              "approve",
              "bob.test",
              "carol.test",
              "--yes",
            ]),
          ) as { writes: { state: { kind: string } }[] };
          expect(approved.writes.map((w) => w.state.kind)).toEqual(["done", "done"]);
        }
        // carol signs in with the CLI; the TUI uses that stored session (as `lessdugin tui`)
        await carol.login();
      },
    );

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

    // M6: in the assisted variants the indexer follows the devnet from here on (poll mode,
    // seeded with this forum), as a deployed one follows the network
    if (sources === "assisted") {
      await step("1c. lessdugin-indexer --poll follows the forum", async () => {
        indexer = await Indexer.start(join(work, "indexer"), forum, out);
      });
    }

    // 2. bob, SPA: signs in (OAuth), joins, posts
    const ctx = await browser.newContext();
    const page = await ctx.newPage();
    const w = await watchStrict(page);
    recordRequests("bob", page);
    await withDevnet(page, s, { parityNowMs: t, ...spaSourceOptions });
    await step(`2. bob (SPA): sign in, join, post (${mode})`, async () => {
      await signIn(page, s, "bob");
      const effective = JSON.parse(await hook(page, "settings", [])) as { sources: string };
      expect(effective.sources, "the SPA worker's effective source mode").toBe(sources);
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
      // docs/plan.md §4.2 step 2, the MEDIA subset (M4): the post carries an image, which
      // runs the engine's whole path - sniff the type, strip the metadata, apply the
      // orientation, enforce the 413 policy - on the same bytes every client then reads.
      await page.getByTestId("attachment-input").setInputFiles(FIXTURE_PNG);
      await expect(page.getByTestId("attachment-list")).toContainText("ready");
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
        await alice.cli("seed-vote", ["--json", "vote", post, "up", "--yes"]);
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
        await alice.cli("mod-hide", [
          "--json",
          "mod",
          "hide",
          reply,
          "--reason",
          "off topic",
          "--yes",
        ]),
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
    // 6c. Alice, the owner and therefore a seed in both membership modes, applies the wiki-backed
    // tag to Bob's post through the real SPA action. Her vote resolves independently of Bob's
    // open-mode newcomer status; in approval mode his grant already includes him in S.
    await step("6c. alice (SPA): applies the wiki-backed tag to bob's post", async () => {
      const tagCtx = await browser.newContext();
      const tagPage = await tagCtx.newPage();
      const tw = await watchStrict(tagPage);
      recordRequests("alice", tagPage);
      await withDevnet(tagPage, s, { parityNowMs: t, ...spaSourceOptions });
      await signIn(tagPage, s, "alice");
      await tagPage.goto(`${s.app}/${route(post)}`);
      await expect(tagPage.getByRole("heading", { level: 1, name: EDITED_TITLE })).toBeVisible();
      await tagPage.getByRole("button", { name: "Vote on tags" }).click();
      const votes = tagPage.getByRole("region", { name: "Tag votes" });
      await votes.getByLabel("Apply another tag").selectOption(TAG_SLUG);
      await votes.getByRole("button", { name: "Vote to apply" }).click();
      await tagPage
        .getByRole("dialog", { name: "Tag votes are public" })
        .getByRole("button", { name: "I understand, continue" })
        .click();
      await expect(votes.getByRole("button", { name: "Applies", exact: true })).toHaveAttribute(
        "aria-pressed",
        "true",
      );
      const flushed = JSON.parse(await hook(tagPage, "outboxFlush", [])) as {
        failed: number;
        conflicts: number;
      };
      expect([flushed.failed, flushed.conflicts]).toEqual([0, 0]);
      const opinions = (await xrpc(s.devnet.pds, "com.atproto.repo.listRecords", {
        query: { repo: s.devnet.accounts.alice.did, collection: `${NS}.opinion`, limit: "100" },
      })) as {
        records: {
          uri: string;
          value: { subject?: { uri?: string }; tags?: { tag: string; value: string }[] };
        }[];
      };
      writeFileSync(join(out, "alice-opinions.json"), `${JSON.stringify(opinions, null, 2)}\n`);
      const tagOpinion = opinions.records.find(
        (r) =>
          r.value.subject?.uri === post &&
          r.value.tags?.some((v) => v.tag === TAG_SLUG && v.value === "up"),
      );
      expect(
        tagOpinion,
        "alice's settled public opinion contains the applicable tag vote",
      ).toBeTruthy();
      writeFileSync(
        join(out, "alice-tag-opinion-raw.json"),
        await hook(tagPage, "raw", [tagOpinion?.uri ?? ""]),
      );
      await hook(tagPage, "syncForum", [forum, { full: false }]);
      const ownSpaList = await hook(tagPage, "listPosts", [
        { forum, sort: "magic", filter: "frontpage", limit: 30 },
      ]);
      writeFileSync(join(out, "alice-spa-list.json"), ownSpaList);
      const ownItems = (JSON.parse(ownSpaList) as { items: { uri: string; tags: string[] }[] })
        .items;
      expect(ownItems.find((item) => item.uri === post)?.tags).toContain(TAG_SLUG);
      expect(await tw.problems()).toEqual([]);
      expectLoopbackOnly("alice");
      await tagCtx.close();
    });
    expectLoopbackOnly("bob");
    await ctx.close();

    // 6b. step 5's pin is bound to the CID it was made on (social.md §4.5): the edit suspended
    // it "until an authority re-asserts it on the new version", which alice does here, so the
    // compared views show a pin (and the moderator's own view shows the suspension first)
    await step("6b. alice (CLI): pins the edited post again", async () => {
      await alice.cli("sync-after-edit", ["--json", "sync", forum]);
      const own = JSON.parse(await alice.cli("read-after-edit", ["--json", "read", post])) as {
        post: { post: { title?: string; pinned: boolean } };
      };
      expect(own.post.post.title).toBe(EDITED_TITLE);
      expect(own.post.post.pinned, "the edit suspends the pin of the earlier version").toBe(false);
      await alice.cli("mod-repin", [
        "--json",
        "mod",
        "pin",
        post,
        "--scope",
        "forum",
        "--reason",
        "still a good question",
      ]);
      await alice.cli("sync-after-repin", ["--json", "sync", forum]);
      const ownList = JSON.parse(await alice.cli("ls-after-tag", ["--json", "ls", forum])) as {
        items: { uri: string; tags: string[] }[];
      };
      expect(ownList.items.find((p) => p.uri === post)?.tags).toEqual([TAG_SLUG]);
    });

    // 7. carol, TUI: tags the post (docs/plan.md §4.2 step 7). The driver types
    // `:vote up --tag <slug>` past the public-action notice and sends the write. A tag vote
    // counts only from a voter in E (social.md §13.3): approval mode establishes carol, so
    // rel(p, slug) grows by her power (1); in open mode she is a newcomer and the vote is
    // recorded without weight (§6.5). Alice's vote (6c) applies the tag either way, and the
    // compared views below carry whichever holds.
    await step("7. carol (TUI): tags the post; relevance applies", async () => {
      type Tagged = { slug: string; relevance: number; applies: boolean; mine?: string };
      const tagOf = async (label: string): Promise<Tagged | undefined> => {
        await carol.cli(`sync-${label}`, ["--json", "sync", forum]);
        const thread = JSON.parse(await carol.cli(`read-${label}`, ["--json", "read", post])) as {
          post: { tagRelevance?: Tagged[] };
        };
        return thread.post.tagRelevance?.find((t) => t.slug === TAG_SLUG);
      };
      const before = await tagOf("before-tag");
      expect(before?.applies, `alice's tag vote applies: ${JSON.stringify(before)}`).toBe(true);
      expect(before?.mine).toBeUndefined();
      const r = JSON.parse(await carol.tui("tag", ["tag", forum, post, TAG_SLUG]));
      expect(r).toEqual({ subject: post, tag: TAG_SLUG });
      const opinions = (await xrpc(s.devnet.pds, "com.atproto.repo.listRecords", {
        query: { repo: s.devnet.accounts.carol.did, collection: `${NS}.opinion`, limit: "100" },
      })) as {
        records: {
          uri: string;
          value: { subject?: { uri?: string }; tags?: { tag: string; value: string }[] };
        }[];
      };
      writeFileSync(join(out, "carol-opinions.json"), `${JSON.stringify(opinions, null, 2)}\n`);
      expect(
        opinions.records.find(
          (o) =>
            o.value.subject?.uri === post &&
            o.value.tags?.some((v) => v.tag === TAG_SLUG && v.value === "up"),
        ),
        "carol's public opinion carries the tag vote",
      ).toBeTruthy();
      const after = await tagOf("after-tag");
      expect(after?.mine, `carol's own tag vote: ${JSON.stringify(after)}`).toBe("up");
      expect(after?.applies).toBe(true);
      expect(after?.relevance, `rel(p, ${TAG_SLUG}) in ${mode} mode`).toBe(
        (before?.relevance ?? 0) + (approval ? 1 : 0),
      );
    });

    // 6d. a newcomer, PDS API: a fresh account that no seed references joins and comments on
    // bob's post. In open mode only CORE+ can introduce it (sync.md §2.2, §4.4: Constellation
    // backlinks, the relay, Jetstream); the closure (§4.3) never reaches it. In approval mode it
    // is an applicant, whose content no reader is shown.
    await step("6d. a newcomer (PDS API): joins and comments", async () => {
      const handle = `nc${Date.now().toString(36)}.test`;
      const acct = (await xrpc(s.devnet.pds, "com.atproto.server.createAccount", {
        body: { handle, email: `${handle}@example.test`, password: `${handle}-password` },
      })) as { did: string; accessJwt: string };
      newcomer = acct.did;
      const session: Session = { did: acct.did, token: acct.accessJwt, pds: s.devnet.pds };
      const ref = { uri: forum, fk: fk(forum) };
      await put(session, "membership", ref.fk, { forum, createdAt: now() });
      const rkey = post.split("/").pop() ?? "";
      const current = (await xrpc(s.devnet.pds, "com.atproto.repo.getRecord", {
        query: { repo: s.devnet.accounts.bob.did, collection: `${NS}.post`, rkey },
      })) as { uri: string; cid: string };
      const c = await createComment(
        session,
        ref,
        { uri: current.uri, cid: current.cid, rkey },
        NEWCOMER_REPLY,
      );
      newcomerComment = c.uri;
      // every reader below is given the same hints
      await services.waitForRecord(newcomerComment);
    });

    // the blob the SPA prepared and uploaded in step 2, as bob's post record names it (the edit
    // keeps the blob object verbatim, data-model.md §8.8)
    const rkey = post.split("/").pop() ?? "";
    const record = (await xrpc(s.devnet.pds, "com.atproto.repo.getRecord", {
      query: { repo: s.devnet.accounts.bob.did, collection: `${NS}.post`, rkey },
    })) as { value: { body: { media?: { blob: { ref: { $link: string } } }[] } } };
    expect(record.value.body.media?.length, "bob's post carries the attached image").toBe(1);
    const mediaCid = record.value.body.media?.[0]?.blob.ref.$link ?? "";

    // the anonymous readers, each on a fresh data directory or profile
    const cli = user("anon-cli");
    const tuiReader = user("anon-tui");
    const cliOut: Outputs = { thread: "", list: "" };
    const tuiOut = { thread: { compact: "", pretty: "" }, list: { compact: "", pretty: "" } };
    const spaOut: Outputs = { thread: "", list: "" };

    await step("read: CLI (ls --json, read --json)", () =>
      watched("cli", async () => {
        await cli.verifySources();
        // the DEFAULT view (magic sort, frontpage filter since M3, engine-api.md §3.4), which is
        // what the TUI's Forum screen shows: the three clients must agree on the default, so
        // none of them pins a sort or filter the others do not.
        cliOut.list = await cli.cli("ls", ["--json", "ls", forum]);
        cliOut.thread = await cli.cli("read", ["--json", "read", post]);
      }),
    );

    await step("read: TUI (Forum and Thread screens)", () =>
      watched("tui", async () => {
        const dir = join(out, "tui");
        await tuiReader.tui("read", ["read", forum, post, dir]);
        const f = (n: string) => readFileSync(join(dir, n), "utf8");
        tuiOut.thread = { compact: f("thread.json"), pretty: f("thread.pretty.json") };
        tuiOut.list = { compact: f("list.json"), pretty: f("list.pretty.json") };
        const effective = JSON.parse(f("settings.json")) as { sources: string };
        expect(effective.sources, "the TUI engine's effective source mode").toBe(sources);
      }),
    );

    await step("read: SPA (forum page, post page; debug hook)", () =>
      watched("spa", async () => {
        const actx = await browser.newContext();
        const apage = await actx.newPage();
        const aw = await watchStrict(apage);
        recordRequests("anonymous", apage);
        await withDevnet(apage, s, { parityNowMs: t, ...spaSourceOptions });
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
        spaOut.thread = await hook(apage, "getThread", [
          post,
          { sort: "magic", unreadOnly: false },
        ]);
        spaOut.list = await hook(apage, "listPosts", [
          // the same default view as the CLI and the TUI (magic, frontpage)
          { forum, sort: "magic", filter: "frontpage", limit: 30 },
        ]);
        expect(await aw.problems()).toEqual([]);

        // M5 cached forum rendering: return to the forum while the devnet is live, then reload the
        // real SPA/worker against the IndexedDB this page just warmed. During the reload, only the
        // forum's own declaration refresh stays live; every other PLC/PDS request gets 503, and so
        // does every request to the CORE+ services (which carry no record text anyway). Thus the
        // non-empty post list can only come from the real cache, without charging the render budget
        // for the declaration fetcher's offline retry schedule. This is a warm page/worker restart,
        // not a cold browser process start or a claim that the whole network was offline.
        await apage.goto(`${s.app}/${route(forum)}`);
        await expect(apage.getByRole("heading", { level: 1, name: FORUM_NAME })).toBeVisible();
        const blockedRequests: string[] = [];
        const forumParts = forum.split("/");
        const corePlusOrigins = [services.urls.constellation, services.urls.relay].map(
          (u) => new URL(u).origin,
        );
        await apage.route("**/*", async (r) => {
          const url = r.request().url();
          const parsed = new URL(url);
          const origin = parsed.origin;
          const declaration =
            origin === s.devnet.pds &&
            parsed.pathname === "/xrpc/com.atproto.repo.getRecord" &&
            parsed.searchParams.get("repo") === forumParts[2] &&
            parsed.searchParams.get("collection") === forumParts[3] &&
            parsed.searchParams.get("rkey") === forumParts[4];
          const network = origin === s.devnet.pds || origin === s.devnet.plc;
          if ((network && !declaration) || corePlusOrigins.includes(origin)) {
            blockedRequests.push(url);
            await r.fulfill({ status: 503, contentType: "text/plain", body: "cache-only reload" });
          } else {
            await r.continue();
          }
        });
        const cache0 = Date.now();
        await apage.reload();
        await expect(apage.getByRole("heading", { level: 1, name: FORUM_NAME })).toBeVisible();
        await expect(apage.getByRole("link", { name: EDITED_TITLE })).toBeVisible();
        await expect(apage.getByRole("link", { name: CANARY_TITLE })).toBeVisible();
        const ms = Date.now() - cache0;
        // The reloaded page's sync must have been refused. In direct mode its first requests
        // (the live tail's head polls) come at once; in assisted mode Jetstream replaces those
        // polls and the forum sync asks the CORE+ services before the PDS, so the refusals may
        // follow the paint. The render time above is unaffected by this wait.
        await expect
          .poll(() => blockedRequests.length, {
            message: "the cached reload really refused repository/sync traffic",
            timeout: 15_000,
          })
          .toBeGreaterThan(0);
        cachedRender = { ms, blockedRequests };
        writeFileSync(
          join(out, "cached-render.json"),
          `${JSON.stringify({ ...cachedRender, budgetMs: 1500, coldBrowserRestart: false, declarationRefresh: "loopback only; all other PLC/PDS and CORE+ HTTP requests returned 503" }, null, 2)}\n`,
        );
        expect(ms, `warm IndexedDB forum render (${ms} ms); see cached-render.json`).toBeLessThan(
          1500,
        );
        expectLoopbackOnly("anonymous");
        await actx.close();
      }),
    );

    // M6, the indexer on: three more fresh readers, configured with the indexer that followed
    // the scenario (configuration consent), and no sync before they open the forum. The TUI's
    // Forum screen and the SPA's forum page hydrate the cold page from the indexer's ordering
    // and proof bundles (engine-api.md: list_posts_page, hydrate_indexer_page); the SPA's is
    // read before any participant's repo is listed. Every reader's forum sync polls the
    // indexer's change feed and then runs the ordinary PDS sync, and they read exactly as the
    // readers above.
    const idx = {
      cli: { thread: "", list: "" } as Outputs,
      tui: { thread: { compact: "", pretty: "" }, list: { compact: "", pretty: "" } },
      spa: { thread: "", list: "" } as Outputs,
      /** The SPA's cold page, read while the participants' range listings were held. */
      spaCold: "",
      /** Those held listings (the page's ordinary sync, released after the cold page). */
      spaHeld: [] as string[],
      requests: {} as Record<string, number>,
      spaPaths: [] as string[],
      cliPaths: [] as string[],
      tuiPaths: [] as string[],
    };
    const runIndexer = indexer as Indexer | null;
    if (runIndexer) {
      const idxUrl = runIndexer.url;
      // the native readers reach the indexer through a recording proxy each, so the methods
      // each one used are known (the SPA's requests are seen by the page)
      const cliProxy = await RecordingProxy.start(idxUrl);
      const tuiProxy = await RecordingProxy.start(idxUrl);
      idx.cliPaths = cliProxy.paths;
      idx.tuiPaths = tuiProxy.paths;
      await step("indexer: converged on the devnet heads", async () => {
        const required = [s.devnet.accounts.alice.did, s.devnet.accounts.bob.did];
        if (approval) required.push(s.devnet.accounts.carol.did);
        await runIndexer.converge(forum, required);
      });
      // the indexer's request counter around a reader (its own two /metrics reads excluded)
      const counted = async (client: string, body: () => Promise<void>) => {
        const before = await runIndexer.requests();
        await body();
        idx.requests[client] = (await runIndexer.requests()) - before - 1;
      };
      const idxCli = user("idx-cli", cliProxy.url);
      const idxTui = user("idx-tui", tuiProxy.url);
      await step("read with the indexer: CLI", () =>
        counted("cli", async () => {
          idx.cli.list = await idxCli.cli("ls", ["--json", "ls", forum]);
          idx.cli.thread = await idxCli.cli("read", ["--json", "read", post]);
        }),
      );
      await step("read with the indexer: TUI", () =>
        counted("tui", async () => {
          const dir = join(out, "tui-indexer");
          await idxTui.tui("read", ["read", forum, post, dir]);
          const f = (n: string) => readFileSync(join(dir, n), "utf8");
          idx.tui.thread = { compact: f("thread.json"), pretty: f("thread.pretty.json") };
          idx.tui.list = { compact: f("list.json"), pretty: f("list.pretty.json") };
        }),
      );
      await step("read with the indexer: SPA", () =>
        counted("spa", async () => {
          const ictx = await browser.newContext();
          const ipage = await ictx.newPage();
          const iw = await watchStrict(ipage);
          recordRequests("indexer-reader", ipage);
          ipage.on("request", (r) => {
            if (r.url().startsWith(idxUrl)) idx.spaPaths.push(new URL(r.url()).pathname);
          });
          await withDevnet(ipage, s, { parityNowMs: t, ...spaSourceOptions, indexers: [idxUrl] });
          // The cold page as the indexer path leaves it (sync.md §14.3): until it has been read,
          // every range listing of a repo other than the forum authority's waits (held, not
          // refused), so the page's ordinary forum sync cannot add, reorder or repair anything
          // on it. The indexer, head checks, DID documents, the declaration, point fetches of
          // stale proofs and the authority's P0 sync (§14.3 step 5) go through.
          const authority = forum.split("/")[2];
          let release = () => {};
          const released = new Promise<void>((ok) => {
            release = ok;
          });
          const participantListing = (u: URL) =>
            u.origin === s.devnet.pds &&
            RANGE_LISTINGS.includes(u.pathname) &&
            u.searchParams.get("repo") !== authority;
          await ipage.route(participantListing, async (r) => {
            idx.spaHeld.push(r.request().url());
            await released;
            // (a request the page gave up on while it was held cannot be continued)
            await r.continue().catch(() => {});
          });
          await ipage.goto(`${s.app}/${route(forum)}`);
          await expect(ipage.getByRole("heading", { level: 1, name: FORUM_NAME })).toBeVisible();
          // the cold page is painted: both posts, or what the indexer path alone shows within
          // 20 s (sync.md §14.3 expects 1-2 s); a post missing then is reported below, by
          // coldPageProblems, with every output written
          await Promise.all(
            [EDITED_TITLE, CANARY_TITLE].map((name) =>
              ipage
                .getByRole("link", { name })
                .waitFor({ timeout: 20_000 })
                .catch(() => {}),
            ),
          );
          // Read without hydrating again: the posts the page lists, in its order, and each one's
          // local view (getPost reads the cache), so what is compared is what the page showed,
          // not a second hydration.
          const hrefs = await ipage
            .getByRole("region", { name: "Posts" })
            .getByRole("article")
            .evaluateAll((articles) =>
              articles.map((a) => a.querySelector("h3 a")?.getAttribute("href") ?? ""),
            );
          const local = Promise.all(
            hrefs.map(async (href) => {
              const m = /#\/p\/([^/]+)\/([^/?#]+)$/.exec(href);
              // (a hidden or removed post is listed as its reason only, without a link)
              if (!m) return { uri: href || "(a row without a link)", cid: "", badges: {} };
              const uri = `at://${decodeURIComponent(m[1] ?? "")}/${NS}.post/${m[2]}`;
              const view = JSON.parse(await hook(ipage, "getPost", [uri])) as {
                post: { uri: string; cid: string; badges: Record<string, boolean> };
              };
              return { uri: view.post.uri, cid: view.post.cid, badges: view.post.badges };
            }),
          );
          const items = await Promise.race([
            local,
            new Promise<null>((ok) => setTimeout(() => ok(null), 15_000)),
          ]);
          idx.spaCold = items ? JSON.stringify({ items }) : "";
          // the page's ordinary sync did ask for the participants' listings meanwhile
          await expect
            .poll(() => idx.spaHeld.length, {
              message: "the forum page's sync listed a participant's repo (held)",
              timeout: 15_000,
            })
            .toBeGreaterThan(0);
          release();
          await ipage.unrouteAll({ behavior: "wait" });
          await local;
          // a hold that outlasted the page's sync budget (8 s) cut that sync short; its repos are
          // requeued and converge in the background (sync.md §5.2), and an explicit forum sync
          // has that done before the page is read like the readers above
          await hook(ipage, "syncForum", [forum, { full: false }]);
          await ipage.getByRole("link", { name: EDITED_TITLE }).click();
          await expect(ipage.getByRole("heading", { level: 1, name: EDITED_TITLE })).toBeVisible();
          await hook(ipage, "syncForum", [forum, { full: false }]);
          idx.spa.thread = await hook(ipage, "getThread", [
            post,
            { sort: "magic", unreadOnly: false },
          ]);
          idx.spa.list = await hook(ipage, "listPosts", [
            { forum, sort: "magic", filter: "frontpage", limit: 30 },
          ]);
          expect(await iw.problems()).toEqual([]);
          expectLoopbackOnly("indexer-reader");
          await ictx.close();
        }),
      );
      await cliProxy.close();
      await tuiProxy.close();
    }

    for (const [name, text] of [
      ["cli.thread.json", cliOut.thread],
      ["cli.list.json", cliOut.list],
      ["spa.thread.json", spaOut.thread],
      ["spa.list.json", spaOut.list],
      ...(runIndexer
        ? ([
            ["indexer.cli.thread.json", idx.cli.thread],
            ["indexer.cli.list.json", idx.cli.list],
            ["indexer.spa.thread.json", idx.spa.thread],
            ["indexer.spa.list.json", idx.spa.list],
            ["indexer.spa.cold-list.json", idx.spaCold],
            ["indexer.spa.held-listings.json", `${JSON.stringify(idx.spaHeld, null, 2)}\n`],
          ] as const)
        : []),
    ] as const) {
      writeFileSync(join(out, name), text);
    }

    // byte for byte (the diffs are written first, so a failure below still leaves them)
    const problems = [
      ...(await compare("thread", cliOut.thread, tuiOut.thread, spaOut.thread, out, info)),
      ...(await compare("list", cliOut.list, tuiOut.list, spaOut.list, out, info)),
    ];
    if (runIndexer) {
      // U-D2: with the indexer on, every client's view equals the indexer-off view once the
      // provenance badges are removed: content, scores, states and order byte for byte
      const bare = (o: Outputs, tui: typeof tuiOut) => ({
        cli: { thread: withoutProvenance(o.thread, true), list: withoutProvenance(o.list, true) },
        tui: {
          thread: {
            compact: withoutProvenance(tui.thread.compact, false),
            pretty: withoutProvenance(tui.thread.pretty, true),
          },
          list: {
            compact: withoutProvenance(tui.list.compact, false),
            pretty: withoutProvenance(tui.list.pretty, true),
          },
        },
      });
      const on = bare(idx.cli, idx.tui);
      const onSpa = {
        thread: withoutProvenance(idx.spa.thread, false),
        list: withoutProvenance(idx.spa.list, false),
      };
      // the SPA's cold page as the indexer path left it, before the participants' listings
      problems.push(...coldPageProblems(idx.spaCold, spaOut.list));
      problems.push(
        ...(await compare("thread-indexer", on.cli.thread, on.tui.thread, onSpa.thread, out, info)),
        ...(await compare("list-indexer", on.cli.list, on.tui.list, onSpa.list, out, info)),
      );
      const off = bare(cliOut, tuiOut);
      for (const what of ["thread", "list"] as const) {
        if (on.tui[what].compact !== off.tui[what].compact) {
          const a = join(out, `${what}-indexer-off.json`);
          const b = join(out, `${what}-indexer-on.json`);
          writeFileSync(a, off.tui[what].pretty);
          writeFileSync(b, on.tui[what].pretty);
          const r = spawnSync(
            "diff",
            ["-u", "--label", "indexer off", "--label", "indexer on", a, b],
            {
              encoding: "utf8",
            },
          );
          const path = join(out, `${what}-indexer-off-vs-on.diff`);
          writeFileSync(path, r.stdout || r.error?.message || "(no diff output)");
          await info.attach(`${what}-indexer-off-vs-on.diff`, { path, contentType: "text/plain" });
          problems.push(
            `${what}: the indexer-hydrated view differs from the indexer-off view (${path})`,
          );
        }
      }
    }
    writeFileSync(join(out, "native-config.toml"), readFileSync(join(alice.dir, "config.toml")));
    writeFileSync(join(out, "spa-requests.json"), `${JSON.stringify(spaRequests, null, 2)}\n`);
    const servicesLog = await services.log();
    writeFileSync(
      join(out, "core-plus.json"),
      `${JSON.stringify({ urls: services.urls, readers: traffic, log: servicesLog }, null, 2)}\n`,
    );
    const corePlus = Object.fromEntries(
      Object.entries(traffic).map(([client, d]) => [
        client,
        {
          http: d.http.map((r) => `${r.service} ${r.nsid} ${r.status}`),
          jetstream: d.jetstream.map((j) => `${j.wire} ${j.url.split("?")[0]}`),
        },
      ]),
    );
    writeFileSync(
      join(out, "result.json"),
      `${JSON.stringify({ parityNowMs: t, mode, sources, forum, wiki, tag: TAG_SLUG, post, reply, newcomer, newcomerComment, mediaCid, socialVote, cachedRender, corePlus, indexer: runIndexer ? { url: runIndexer.url, requests: idx.requests, cliPaths: idx.cliPaths, tuiPaths: idx.tuiPaths, spaPaths: idx.spaPaths, spaHeldListings: idx.spaHeld.length } : null, identical: problems.length === 0, problems }, null, 2)}\n`,
    );
    expect(problems, `the clients disagree; diffs in ${out}`).toEqual([]);

    // The sources dimension (sync.md §2.4, decision O-3): every client was configured with the
    // same CORE+ services. In assisted mode each reader contacted them; in direct mode no client
    // (writer or reader, native or browser) contacted any of them during the whole variant.
    if (sources === "assisted") {
      for (const client of ["cli", "tui", "spa"]) {
        const d = traffic[client];
        expect(
          (d?.http.length ?? 0) + (d?.jetstream.length ?? 0),
          `${client}: CORE+ requests in assisted mode (core-plus.json)`,
        ).toBeGreaterThan(0);
        expect(
          d?.http.every((r) => r.status === 200),
          `${client}: every CORE+ request was answered`,
        ).toBe(true);
      }
      // the readers followed the short pages to the end (each page carries a cursor)
      expect(
        servicesLog.http.filter((r) => r.service === "constellation" && /[?&]cursor=/.test(r.url))
          .length,
        "Constellation pages followed by cursor",
      ).toBeGreaterThan(0);
      // identity and account noise for unrelated DIDs reached the clients (a subscription
      // without a DID filter, as the browser's before it knows the participants, gets it all)
      // and was dropped: no view below names a noise DID
      expect(servicesLog.mirror.noise, "the Jetstream log carries noise").toBeGreaterThan(0);
      expect(
        servicesLog.jetstream.reduce((n, j) => n + j.noiseFrames, 0),
        "noise frames delivered to the clients' Jetstream connections",
      ).toBeGreaterThan(0);
    } else {
      expect(servicesLog.http, "direct: nothing asked the Constellation or the relay").toEqual([]);
      expect(servicesLog.jetstream, "direct: nothing connected to Jetstream").toEqual([]);
    }
    // the noise of unrelated DIDs never becomes forum content
    for (const text of [cliOut.thread, cliOut.list, spaOut.thread, spaOut.list]) {
      for (const did of NOISE_DIDS) expect(text).not.toContain(did);
    }
    if (runIndexer) {
      // the indexer path was taken: every indexer-on reader asked the indexer; the TUI's and
      // the SPA's cold forum page was hydrated from its ordering and a proof bundle
      // (hydrate_indexer_page, list_posts_page), and every reader's forum sync polled its
      // change feed (poll_indexer_changes) before its ordinary PDS sync
      for (const client of ["cli", "tui", "spa"]) {
        expect(idx.requests[client], `${client}: requests to the indexer`).toBeGreaterThan(0);
      }
      const hydration = [`/xrpc/${NS}.index.listPosts`, `/xrpc/${NS}.index.getRecordProofs`];
      const changes = `/xrpc/${NS}.index.getChanges`;
      expect(idx.tuiPaths, "the TUI's cold page hydration and change feed").toEqual(
        expect.arrayContaining([...hydration, changes]),
      );
      expect(idx.spaPaths, "the SPA's cold page hydration and change feed").toEqual(
        expect.arrayContaining([...hydration, changes]),
      );
      expect(idx.cliPaths, "the CLI's change feed").toContain(changes);
      // and their proof bundles admitted records at level proven (the badge U-D2 lets differ)
      for (const [client, text] of [
        ["TUI", idx.tui.list.compact],
        ["SPA", idx.spa.list],
      ] as const) {
        expect(hasProvenance(text, "verified"), `${client}: a proof-admitted post`).toBe(true);
      }
    }

    // and what the scenario wrote is in the view (equal outputs are not equally empty)
    const thread = JSON.parse(tuiOut.thread.compact);
    expect(thread.post.post.uri).toBe(post);
    expect(thread.post.post.title).toBe(EDITED_TITLE);
    expect(thread.post.post.author.handle).toBe("bob.test");
    expect(thread.post.body.text).toContain(EDIT_TAIL);
    expect(thread.post.history.revisions).toBe(1);
    // step 2's media subset (M4): every compared thread view holds the one image bob attached,
    // by the CID his record names, and its body places it (the gallery's placeholder)
    for (const [client, text] of [
      ["CLI", cliOut.thread],
      ["TUI", tuiOut.thread.compact],
      ["SPA", spaOut.thread],
    ] as const) {
      const body = (JSON.parse(text) as { post: { body: { html: string; media: MediaItem[] } } })
        .post.body;
      expect(
        body.media.map((m) => [m.cid, m.mime, m.author]),
        `${client}: the attached image`,
      ).toEqual([[mediaCid, "image/png", s.devnet.accounts.bob.did]]);
      expect(body.html, `${client}: the image in the body`).toContain(`data-cid="${mediaCid}"`);
    }
    // steps 4 and 5 (the social subset, M3): the counted opinions and the pin. In approval mode
    // carol's vote is the one weighted vote, with her agreement. In open mode alice's seed upvote
    // is weighted, while carol's newcomer vote carries no weight; it is absent from the direct
    // readers' input (reposTotal 2 below) and, found through CORE+, an unweighted vote in the
    // assisted readers' (sync.md §4.3: "K, scores and the visibility of other users are the same
    // with or without CORE+ discovery. `unweighted` counts and the collapsed content of newcomers
    // do differ"). No vote counts as cast on an earlier version: every update of an opinion sets
    // its subject.cid to the subject's current CID (data-model.md §8.5), and both voters updated
    // theirs after bob's edit, alice with her tag action (6c) and carol with her tag vote (7).
    const introduced = !approval && sources === "assisted";
    const score = thread.post.post.score as {
      score: number;
      agreement?: number;
      unweighted: number;
      earlierVersionVotes: number;
      weightedVotes?: number;
    };
    const shown = JSON.stringify(score);
    expect(score.score, `the counted upvote: ${shown}`).toBeGreaterThan(0);
    expect([score.weightedVotes, score.earlierVersionVotes, score.unweighted], shown).toEqual([
      1,
      0,
      introduced ? 1 : 0,
    ]);
    expect(score.agreement, `carol's agreement counts only when she is established: ${shown}`).toBe(
      approval ? 1 : 0,
    );
    expect(thread.post.post.pinned, "step 6b's pin of the edited post").toBe(true);
    // carol's reply was HIDDEN by step 5's mod action, and the fold drops a hidden comment
    // from the anonymous view entirely: it is not listed, and no part of its body survives. This
    // is as much a parity assertion as the byte comparison above - all three clients must agree
    // that the comment is gone.
    // The newcomer's comment (step 6d) is the one other comment: it is in the view exactly when
    // CORE+ can introduce it (open mode, assisted; sync.md §2.2, §4.4), collapsed as a new
    // user's (social.md §6.5 V8), and never in approval mode, where its author is an applicant.
    expect(
      thread.comments.map((c: { uri: string; state: unknown }) => [c.uri, c.state]),
      "a hidden comment must not be in the anonymous view; the newcomer's only through CORE+",
    ).toEqual(introduced ? [[newcomerComment, { kind: "collapsed", reason: "newUser" }]] : []);
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
        pinned: boolean;
        createdAtMs: number;
        tRankMs: number;
        badges: Record<string, unknown>;
        tags: string[];
      }[];
    };
    // the pinned post comes first (social.md §12), whatever the canary's rank
    expect(list.items.map((p) => [p.uri, p.pinned])).toEqual([
      [post, true],
      [canary, false],
    ]);
    // M5 organisation parity: the wiki-backed tag resolved from Alice's real, seed-weighted
    // opinion is present, in the same PostSummary order, in every client's DTO. Equal empty lists
    // cannot pass.
    for (const [client, text] of [
      ["CLI", cliOut.list],
      ["TUI", tuiOut.list.compact],
      ["SPA", spaOut.list],
    ] as const) {
      const view = JSON.parse(text) as { items: { uri: string; tags: string[] }[] };
      expect(view.items.length, `${client}: a non-empty PostSummary list`).toBe(2);
      expect(
        view.items.find((p) => p.uri === post)?.tags,
        `${client}: resolved tags on the tagged post`,
      ).toEqual([TAG_SLUG]);
      expect(view.items.find((p) => p.uri === canary)?.tags).toEqual([]);
    }
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
    await (indexer as Indexer | null)?.stop();
    await services.stop();
    rmSync(work, { recursive: true, force: true });
  }
}
