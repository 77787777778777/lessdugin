// SPDX-License-Identifier: AGPL-3.0-or-later
// Starts the devnet (or reuses `LESSDUGIN_DEVNET_JSON`), seeds alice's forum through the PDS
// API, and serves the e2e build with the development CSP. The state for the tests goes to a
// JSON file named by `LESSDUGIN_SPA_E2E_STATE`; the returned function tears everything down.

import { type ChildProcess, spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { DIST_E2E, startStaticServer } from "../serve.mjs";
import { createComment, createForum, createPost, login } from "./atp.mjs";
import type { DevnetInfo, E2EState } from "./state.ts";

const DEVNET_SCRIPT = fileURLToPath(new URL("../../../packages/e2e/devnet.mjs", import.meta.url));

async function waitForJson(path: string, child: ChildProcess, ms: number): Promise<DevnetInfo> {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) throw new Error(`devnet.mjs exited with ${child.exitCode}`);
    try {
      return JSON.parse(readFileSync(path, "utf8")) as DevnetInfo;
    } catch {
      await new Promise((r) => setTimeout(r, 100));
    }
  }
  throw new Error(`the devnet did not start within ${ms / 1000} s`);
}

export default async function globalSetup(): Promise<() => Promise<void>> {
  if (!existsSync(join(DIST_E2E, "index.html"))) {
    throw new Error(`${DIST_E2E} is missing: run \`VITE_E2E=1 vite build --outDir dist-e2e\``);
  }
  const dir = mkdtempSync(join(tmpdir(), "lessdugin-spa-e2e-"));
  let devnet: ChildProcess | null = null;
  let net: DevnetInfo;
  const reuse = process.env.LESSDUGIN_DEVNET_JSON;
  if (reuse) {
    net = JSON.parse(readFileSync(reuse, "utf8")) as DevnetInfo;
  } else {
    const jsonPath = join(dir, "devnet.json");
    devnet = spawn(process.execPath, [DEVNET_SCRIPT, "--json", jsonPath], {
      stdio: ["ignore", "ignore", "inherit"],
    });
    net = await waitForJson(jsonPath, devnet, 120_000);
  }

  // alice's open forum, a post and a comment, written straight to her PDS.
  const alice = await login(net.pds, net.accounts.alice);
  const slug = `spa-${Date.now().toString(36)}`;
  const forum = await createForum(alice, {
    slug,
    name: "Seeded Forum",
    description: "Written straight to the PDS",
  });
  const post = await createPost(alice, forum, {
    title: "Hello from the PDS API",
    body: "This post was written with **createRecord**.",
  });
  const comment = await createComment(alice, forum, post, "A first comment from alice.");

  const server = await startStaticServer({ root: DIST_E2E, dev: true });
  const state: E2EState = {
    devnet: net,
    app: server.url,
    forum: { uri: forum.uri, slug, owner: alice.did },
    post: { uri: post.uri, rkey: post.rkey },
    comment: { uri: comment.uri },
  };
  const statePath = join(dir, "state.json");
  writeFileSync(statePath, JSON.stringify(state, null, 2));
  process.env.LESSDUGIN_SPA_E2E_STATE = statePath;

  return async () => {
    await server.close();
    if (devnet && devnet.exitCode === null) {
      devnet.kill("SIGTERM");
      await new Promise((r) => devnet?.once("exit", r));
    }
    rmSync(dir, { recursive: true, force: true });
  };
}
