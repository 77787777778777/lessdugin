// SPDX-License-Identifier: AGPL-3.0-or-later
// docs/plan.md M6: a fresh SPA context cold-paints a proof-backed title from a synthetic forum
// with 2,000 actual signed participant DIDs in under two seconds. No mock engine or PDS shortcut.

import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { performance } from "node:perf_hooks";
import { expect, test } from "@playwright/test";
import { route, watchStrict } from "../devnet/state.ts";

interface Fixture {
  pds: string;
  plc: string;
  indexer: string;
  forum: string;
  post: string;
  title: string;
  ownerDid: string;
  participants: number;
  participantDids: string[];
  pdsCount: number;
  seedMs: number;
  preindexMs: number;
  preindex: Record<string, unknown>;
}

interface RequestRow {
  atMs: number;
  method: string;
  url: string;
}

const fixturePath = process.env.LESSDUGIN_COLD_START_JSON;
if (!fixturePath) throw new Error("LESSDUGIN_COLD_START_JSON is required");
const fixture = JSON.parse(readFileSync(fixturePath, "utf8")) as Fixture;
const app = process.env.LESSDUGIN_COLD_START_APP;
if (!app) throw new Error("LESSDUGIN_COLD_START_APP is required (global setup did not run)");
const resultsPath =
  process.env.LESSDUGIN_COLD_START_RESULTS ?? "test-results/m6-cold-start/results.json";

function xrpc(url: string): string {
  const u = new URL(url);
  return u.pathname.startsWith("/xrpc/") ? u.pathname.slice(5) : u.pathname;
}

function requestSummary(requests: RequestRow[]) {
  const byMethod: Record<string, number> = {};
  for (const r of requests) byMethod[xrpc(r.url)] = (byMethod[xrpc(r.url)] ?? 0) + 1;
  return { count: requests.length, byMethod, requests };
}

test("M6: three fresh Chromium contexts cold-paint the signed post title under 2 s", async ({
  browser,
}) => {
  expect(fixture.participants).toBe(2_000);
  expect(fixture.participantDids).toHaveLength(2_000);
  expect(new Set(fixture.participantDids).size).toBe(2_000);
  expect(fixture.preindex.participants).toBe(2_000);
  expect(fixture.preindex.signedAccounts).toBe(2_000);

  const runs: Record<string, unknown>[] = [];
  for (let n = 1; n <= 3; n++) {
    const context = await browser.newContext();
    const page = await context.newPage();
    const strict = await watchStrict(page);
    const requests: RequestRow[] = [];
    const proofResponses: Record<string, unknown>[] = [];
    const listResponses: Record<string, unknown>[] = [];
    const responseTasks: Promise<void>[] = [];
    let started = 0;
    page.on("request", (request) => {
      if (!started) return;
      requests.push({
        atMs: Math.round((performance.now() - started) * 1000) / 1000,
        method: request.method(),
        url: request.url(),
      });
    });
    page.on("response", (response) => {
      const url = response.url();
      if (!url.includes(".getRecordProofs") && !url.includes(".listPosts")) return;
      const task = (async () => {
        try {
          const body = await response.body();
          const row = {
            url,
            status: response.status(),
            bytes: body.length,
            sha256: createHash("sha256").update(body).digest("hex"),
            contentType: response.headers()["content-type"],
          };
          if (url.includes(".getRecordProofs")) proofResponses.push(row);
          else listResponses.push({ ...row, body: body.toString("utf8") });
        } catch (error) {
          proofResponses.push({ url, bodyError: String(error) });
        }
      })();
      responseTasks.push(task);
    });

    const result: Record<string, unknown> = { run: n, freshContext: true };
    try {
      await page.addInitScript(
        (options) => {
          window.__LESSDUGIN_E2E__ = options;
        },
        {
          devnet: { plc: fixture.plc, pds: fixture.pds },
          sources: "assisted",
          relays: [],
          jetstream: [],
          constellation: null,
          slingshot: null,
          indexers: [fixture.indexer],
        },
      );
      const url = `${app}/${route(fixture.forum)}?sort=new&filter=all`;
      started = performance.now();
      await page.goto(url, { waitUntil: "domcontentloaded" });
      await page.getByText(fixture.title, { exact: true }).waitFor({ state: "visible" });
      result.durationMs = Math.round((performance.now() - started) * 1000) / 1000;
      result.titleVisible = true;
      await Promise.all(responseTasks);

      const atTitle = [...requests];
      const headChecks = atTitle.filter(
        (r) =>
          r.url.includes("com.atproto.sync.getLatestCommit") &&
          decodeURIComponent(r.url).includes(fixture.ownerDid),
      );
      const postPointFetches = atTitle.filter((r) => {
        const decoded = decodeURIComponent(r.url);
        return (
          decoded.includes("com.atproto.repo.getRecord") &&
          decoded.includes("lessdugin.temp.post") &&
          decoded.includes(fixture.post.split("/").at(-1) ?? "")
        );
      });
      const ownerDocuments = atTitle.filter(
        (r) => r.url.startsWith(fixture.plc) && r.url.endsWith(`/${fixture.ownerDid}`),
      );
      result.networkAtTitle = requestSummary(atTitle);
      result.evidenceAtTitle = {
        headChecks,
        ownerDocuments,
        postPointFetches,
        proofResponses,
        indexerListPosts: listResponses.map(({ body: _body, ...row }) => row),
      };

      // This query reads the same locally admitted record. It must carry the proof trust badge;
      // indexer JSON itself is checked below to ensure it never supplied the title text.
      const dtoText = await page.evaluate(
        (forum) =>
          window.__lessduginE2E?.listPosts({ forum, sort: "new", filter: "all", limit: 20 }),
        fixture.forum,
      );
      const dto = JSON.parse(dtoText ?? "{}") as {
        items?: { uri?: string; title?: string; badges?: { verified?: boolean } }[];
      };
      const item = dto.items?.find((p) => p.uri === fixture.post);
      result.localDto = { item, items: dto.items?.length ?? 0 };
      result.strictProblems = await strict.problems();

      expect(headChecks.length, "fresh owner PDS head check").toBeGreaterThan(0);
      expect(ownerDocuments.length, "fresh owner PLC document").toBeGreaterThan(0);
      expect(postPointFetches, "proof hydration must not fall back to a post point fetch").toEqual(
        [],
      );
      expect(proofResponses.length, "real getRecordProofs CAR response").toBeGreaterThan(0);
      for (const proof of proofResponses) {
        expect(proof.status).toBe(200);
        expect(proof.bytes as number).toBeGreaterThan(0);
      }
      expect(listResponses.length, "indexer ordering request").toBeGreaterThan(0);
      for (const list of listResponses) {
        expect(list.status).toBe(200);
        expect(list.url as string).toContain("hydrate=false");
        expect(list.body as string).not.toContain(fixture.title);
      }
      expect(item?.title).toBe(fixture.title);
      expect(item?.badges?.verified).toBe(true);
      expect(result.strictProblems).toEqual([]);
    } catch (error) {
      result.error = String(error);
      if (result.durationMs === undefined) {
        result.durationMs = Math.round((performance.now() - started) * 1000) / 1000;
      }
      result.requestsOnFailure = requestSummary([...requests]);
      result.strictProblems = await strict.problems();
    } finally {
      await context.close();
    }
    runs.push(result);
    console.log(`M6 cold start run ${n}: ${JSON.stringify(result)}`);
  }

  const output = {
    schema: 1,
    acceptance: "docs/plan.md M6 cold SPA title from 2,000 signed participants in under 2 s",
    measuredAt: new Date().toISOString(),
    targetMs: 2_000,
    command: process.env.LESSDUGIN_COLD_START_COMMAND ?? null,
    pds: fixture.pds,
    plc: fixture.plc,
    indexer: fixture.indexer,
    participantDids: fixture.participantDids,
    fixture: {
      participants: fixture.participants,
      pdsCount: fixture.pdsCount,
      seedMs: fixture.seedMs,
      preindexMs: fixture.preindexMs,
      preindex: fixture.preindex,
      forum: fixture.forum,
      post: fixture.post,
      title: fixture.title,
    },
    runs,
  };
  mkdirSync(dirname(resultsPath), { recursive: true });
  writeFileSync(resultsPath, `${JSON.stringify(output, null, 2)}\n`);
  for (const [i, run] of runs.entries()) {
    expect(run.error, `run ${i + 1} failed`).toBeUndefined();
    expect(run.durationMs as number, `run ${i + 1} cold-paint time`).toBeLessThan(2_000);
  }
});
