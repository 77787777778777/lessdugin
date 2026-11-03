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
  const NS = "online.7778777.lessdugin.temp";
  const FORUM = `at://did:plc:abcdefghijklmnopqrstuvwx/${NS}.forum/3kforum`;
  const PAGE = { forum: FORUM, slug: "decision-theory" };
  const rev = (rkey: string) => `at://did:plc:bcdefghijklmnopqrstuvwxy/${NS}.wikiRevision/${rkey}`;
  const [C, B, A] = [rev("3kccc"), rev("3kbbb"), rev("3kaaa")];
  /** A revision of `PAGE` (or of the page `at` names) naming `parents`. */
  const raw = (
    uri: string,
    parents: string[],
    extra: Partial<RawView> = {},
    at: { forum?: string; slug?: string; $type?: string } = {},
  ): RawView => ({
    dto: 1,
    uri,
    cid: `cid-${uri}`,
    level: "pds",
    record: {
      $type: at.$type ?? `${NS}.wikiRevision`,
      forum: at.forum ?? FORUM,
      slug: at.slug ?? PAGE.slug,
      name: uri,
      parents: parents.map((p) => ({ uri: p, cid: `cid-${p}` })),
    },
    validity: [],
    assertions: [],
    ...extra,
  });
  const reader = (records: Map<string, RawView>, read: string[] = []) => {
    return async (u: string) => {
      read.push(u);
      const r = records.get(u);
      if (!r) throw new Error("not found");
      return r;
    };
  };

  it("reads every revision reachable through parents, at most the cap", async () => {
    const records = new Map([
      [C, raw(C, [B])],
      [B, raw(B, [A])],
      [A, raw(A, [])],
    ]);
    const read: string[] = [];
    const nodes = await loadWikiNodes(reader(records, read), C, PAGE);
    expect(read).toEqual([C, B, A]);
    expect(uris(wikiChain(C, nodes))).toEqual([C, B, A]);
    expect((await loadWikiNodes(reader(records), C, PAGE, 2)).size).toBe(2);
  });

  it("treats a missing, invalid or hidden revision as unavailable", async () => {
    const on = { ...PAGE, collection: `${NS}.wikiRevision` };
    expect(wikiNodeOf(A, undefined, on).available).toBe(false);
    expect(wikiNodeOf(A, raw(A, [], { validity: ["bad name"] }), on).available).toBe(false);
    const hidden = raw(A, [], { state: { kind: "hidden", reason: { kind: "banned" } } });
    expect(wikiNodeOf(A, hidden, on).available).toBe(false);
    expect(wikiNodeOf(A, raw(A, []), on).available).toBe(true);
  });

  // social.md §13.2: par(r) = parents ∩ uris(page), so a parent of another (forum, slug), or a
  // record of another collection, gives no edge, however long its own chain is
  it("follows only the page's own revisions, never another page's chain", async () => {
    const [X1, X2, X3] = [rev("3kxx1"), rev("3kxx2"), rev("3kxx3")];
    const OTHER_FORUM = `at://did:plc:cdefghijklmnopqrstuvwxyz/${NS}.forum/3kother`;
    const POST = `at://did:plc:bcdefghijklmnopqrstuvwxy/${NS}.post/3kpost`;
    const records = new Map([
      // the head names its own genesis, a deep chain of another slug, a revision of the same
      // slug in another forum and a post
      [C, raw(C, [A, X1, X3, POST])],
      [A, raw(A, [])],
      [X1, raw(X1, [X2], {}, { slug: "other-page" })],
      [X2, raw(X2, [], {}, { slug: "other-page" })],
      [X3, raw(X3, [], {}, { forum: OTHER_FORUM })],
      [POST, raw(POST, [], {}, { $type: `${NS}.post` })],
    ]);
    const read: string[] = [];
    const nodes = await loadWikiNodes(reader(records, read), C, PAGE);
    // a foreign record's parents are not read
    expect(read).not.toContain(X2);
    for (const u of [X1, X3, POST]) expect(nodes.get(u)?.available).toBe(false);
    const chain = wikiChain(C, nodes);
    expect(uris(chain)).toEqual([C, A]);
    expect(chain.parentUnavailable).toBe(false);
  });

  it("flags a root whose parents are all of other pages", async () => {
    const X = rev("3kxxx");
    const records = new Map([
      [C, raw(C, [X])],
      [X, raw(X, [], {}, { slug: "other-page" })],
    ]);
    const chain = wikiChain(C, await loadWikiNodes(reader(records), C, PAGE));
    expect(uris(chain)).toEqual([C]);
    expect(chain.parentUnavailable).toBe(true);
  });
});
