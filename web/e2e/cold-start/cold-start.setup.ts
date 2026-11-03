// SPDX-License-Identifier: AGPL-3.0-or-later

import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { DIST_E2E, startStaticServer } from "../serve.mjs";

export default async function globalSetup(): Promise<() => Promise<void>> {
  if (!existsSync(join(DIST_E2E, "index.html"))) {
    throw new Error(`${DIST_E2E} is missing: run \`VITE_E2E=1 vite build --outDir dist-e2e\``);
  }
  const fixturePath = process.env.LESSDUGIN_COLD_START_JSON;
  if (!fixturePath) throw new Error("LESSDUGIN_COLD_START_JSON must name the running fixture JSON");
  const fixture = JSON.parse(readFileSync(fixturePath, "utf8")) as {
    participants?: number;
    indexer?: string;
    posts?: unknown[];
  };
  if (fixture.participants !== 2_000) {
    throw new Error(`M6 acceptance needs exactly 2,000 participants, got ${fixture.participants}`);
  }
  if ((fixture.posts?.length ?? 0) < 26) {
    throw new Error("the fixture has no first page of posts by distinct participants");
  }
  if (!fixture.indexer) throw new Error("the fixture has no indexer endpoint");
  const server = await startStaticServer({ root: DIST_E2E, dev: true });
  process.env.LESSDUGIN_COLD_START_APP = server.url;
  return server.close;
}
