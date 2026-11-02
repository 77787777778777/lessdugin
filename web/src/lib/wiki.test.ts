// SPDX-License-Identifier: AGPL-3.0-or-later
// The wiki editor reads the canonical revision's record, and the history follows social.md §13.2
// "History view": from the canonical revision, the edge to the parent with the greatest depth,
// ties by the smallest uri; an edge whose CID differs is "ancestor altered".
import type { RawView } from "@lessdugin/engine/types";
import { describe, expect, it } from "vitest";
import { loadWikiNodes, type WikiNode, wikiChain, wikiFormOf, wikiNodeOf } from "./wiki.ts";

const node = (uri: string, parents: [string, string?][], available = true): WikiNode => ({
  uri,
  cid: `cid-${uri}`,
  parents: parents.map(([u, c]) => ({ uri: u, cid: c ?? `cid-${u}` })),
  available,
});
const graph = (...nodes: WikiNode[]) => new Map(nodes.map((n) => [n.uri, n]));
const uris = (c: ReturnType<typeof wikiChain>) => c.steps.map((s) => s.node.uri);

describe("wikiFormOf", () => {
  it("reads the fields an edit starts from, isTag defaulting to true", () => {
    expect(
      wikiFormOf({
        name: "Decision theory",
        description: "How to choose",
        body: { text: "# Page", media: [] },
        parentTags: ["rationality", 7],
      }),
    ).toEqual({
      name: "Decision theory",
      description: "How to choose",
      markdown: "# Page",
      media: [],
      isTag: true,
      parentTags: ["rationality"],
    });
    expect(wikiFormOf({ name: "Glossary", isTag: false })?.isTag).toBe(false);
  });

  it("refuses a page whose body spilled into a blob", () => {
    expect(wikiFormOf({ name: "Long", body: { textBlob: { ref: { $link: "b" } } } })).toBeNull();
  });
});

describe("wikiChain (social.md §13.2)", () => {
  it("walks a linear history from the head to the genesis", () => {
    const c = wikiChain("c", graph(node("c", [["b"]]), node("b", [["a"]]), node("a", [])));
    expect(uris(c)).toEqual(["c", "b", "a"]);
    expect(c.parentUnavailable).toBe(false);
    expect(c.steps.some((s) => s.altered)).toBe(false);
  });

  it("follows the deepest parent, ties by the smallest uri", () => {
    // m names x (depth 0) and y (depth 1): y; n names p and q, both depth 0: the smaller uri
    const deep = wikiChain(
      "m",
      graph(node("m", [["x"], ["y"]]), node("x", []), node("y", [["z"]]), node("z", [])),
    );
    expect(uris(deep)).toEqual(["m", "y", "z"]);
    const tie = wikiChain("n", graph(node("n", [["q"], ["p"]]), node("p", []), node("q", [])));
    expect(uris(tie)).toEqual(["n", "p"]);
  });

  it("marks an edge whose stored CID differs as ancestor altered", () => {
    const c = wikiChain("b", graph(node("b", [["a", "an-older-cid"]]), node("a", [])));
    expect(c.steps.map((s) => s.altered)).toEqual([false, true]);
  });

  it("ends at a revision whose parents are all unavailable (a flagged root)", () => {
    const c = wikiChain("b", graph(node("b", [["a"]]), node("a", [], false)));
    expect(uris(c)).toEqual(["b"]);
    expect(c.parentUnavailable).toBe(true);
    // a parent the walk never read is unavailable too
    expect(wikiChain("b", graph(node("b", [["gone"]]))).parentUnavailable).toBe(true);
  });

  it("cuts a cycle", () => {
    const c = wikiChain("a", graph(node("a", [["b"]]), node("b", [["a"]])));
    expect(uris(c)).toEqual(["a", "b"]);
  });
});

describe("loadWikiNodes", () => {
  const raw = (uri: string, parents: string[], extra: Partial<RawView> = {}): RawView => ({
    dto: 1,
    uri,
    cid: `cid-${uri}`,
    level: "pds",
    record: { name: uri, parents: parents.map((p) => ({ uri: p, cid: `cid-${p}` })) },
    validity: [],
    assertions: [],
    ...extra,
  });

  it("reads every revision reachable through parents, at most the cap", async () => {
    const records = new Map([
      ["c", raw("c", ["b"])],
      ["b", raw("b", ["a"])],
      ["a", raw("a", [])],
    ]);
    const read: string[] = [];
    const nodes = await loadWikiNodes(async (u) => {
      read.push(u);
      const r = records.get(u);
      if (!r) throw new Error("not found");
      return r;
    }, "c");
    expect(read).toEqual(["c", "b", "a"]);
    expect(uris(wikiChain("c", nodes))).toEqual(["c", "b", "a"]);
    expect((await loadWikiNodes(async (u) => records.get(u) as RawView, "c", 2)).size).toBe(2);
  });

  it("treats a missing, invalid or hidden revision as unavailable", async () => {
    expect(wikiNodeOf("x", undefined).available).toBe(false);
    expect(wikiNodeOf("x", raw("x", [], { validity: ["bad name"] })).available).toBe(false);
    const hidden = raw("x", [], { state: { kind: "hidden", reason: { kind: "banned" } } });
    expect(wikiNodeOf("x", hidden).available).toBe(false);
    expect(wikiNodeOf("x", raw("x", [])).available).toBe(true);
  });
});
