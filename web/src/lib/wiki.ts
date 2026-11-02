// SPDX-License-Identifier: AGPL-3.0-or-later
// A wiki page as the editor and the history read it (social.md §13, data-model.md §6.8). An edit
// starts from the canonical revision's own record (`raw(head)`), so its media, parent tags and
// `isTag` are kept unless the author changes them. The engine has no history operation for a
// page, so the history is read revision by revision from the raw records, following `parents`
// as social.md §13.2 "History view" says. Record JSON is untrusted: every field is checked.

import type { MediaRef, RawView } from "@lessdugin/engine/types";
import { isSpilledBody, markdownOf, mediaRefsOf, objectField, stringField } from "./records.ts";

/** The lexicon's `parents` cap (`wikiRevision.parents`, maxLength 8). */
const MAX_PARENTS = 8;
/** How many revisions the history reads at most (each is one `raw`). */
export const WIKI_HISTORY_MAX = 200;

export interface WikiForm {
  name: string;
  description: string;
  markdown: string;
  media: MediaRef[];
  isTag: boolean;
  parentTags: string[];
}

export const EMPTY_WIKI_FORM: WikiForm = {
  name: "",
  description: "",
  markdown: "",
  media: [],
  isTag: true,
  parentTags: [],
};

/** The editable fields of a revision record, or null when its body spilled into a blob (the
 * source is not in the record, and a draft sent without it would drop the page's text). */
export function wikiFormOf(record: unknown): WikiForm | null {
  const body = objectField(record, "body");
  if (body !== undefined && isSpilledBody(body)) return null;
  const isTag = (record as { isTag?: unknown } | null)?.isTag;
  const parents = (record as { parentTags?: unknown } | null)?.parentTags;
  return {
    name: stringField(record, "name") ?? "",
    description: stringField(record, "description") ?? "",
    markdown: markdownOf(body) ?? "",
    media: mediaRefsOf(body),
    // an absent isTag is true (the lexicon's default)
    isTag: isTag !== false,
    parentTags: Array.isArray(parents)
      ? parents.filter((t): t is string => typeof t === "string")
      : [],
  };
}

/** One revision as the history walks it. */
export interface WikiNode {
  uri: string;
  /** The record's CID, to tell an altered ancestor. */
  cid?: string | undefined;
  /** `parents` as written (strong refs). */
  parents: { uri: string; cid?: string | undefined }[];
  /** The revision exists here, is valid and is not hidden: it can be a step of the chain. */
  available: boolean;
  raw?: RawView | undefined;
}

/** A step of the history chain: the revision and whether the edge from the step before it (its
 * child) names it with another CID ("ancestor altered", social.md §13.2). */
export interface WikiStep {
  node: WikiNode;
  altered: boolean;
}

export interface WikiChain {
  /** From the canonical revision back to the oldest one reached. */
  steps: WikiStep[];
  /** The oldest step names parents and none of them is available (a flagged root). */
  parentUnavailable: boolean;
}

/** A raw view as a history node. */
export function wikiNodeOf(uri: string, raw: RawView | undefined): WikiNode {
  if (!raw) return { uri, parents: [], available: false };
  const list = (raw.record as { parents?: unknown } | null)?.parents;
  const parents: WikiNode["parents"] = [];
  for (const p of Array.isArray(list) ? list.slice(0, MAX_PARENTS) : []) {
    const u = stringField(p, "uri");
    if (u !== undefined) parents.push({ uri: u, cid: stringField(p, "cid") });
  }
  const hidden = raw.state?.kind === "hidden" || raw.state?.kind === "tombstone";
  return {
    uri,
    cid: raw.cid,
    parents,
    available: raw.record !== undefined && raw.validity.length === 0 && !hidden,
    raw,
  };
}

/** The history chain from `head` (social.md §13.2): at each step the edge to the available
 * parent with the greatest depth, ties by the smallest uri. A revision's depth is the length of
 * its longest chain of available ancestors; an edge back into the path being measured is cut,
 * so a cycle never loops. */
export function wikiChain(head: string, nodes: ReadonlyMap<string, WikiNode>): WikiChain {
  const depths = new Map<string, number>();
  const onPath = new Set<string>();
  const parentsOf = (n: WikiNode): WikiNode[] => {
    const out: WikiNode[] = [];
    for (const p of n.parents) {
      const q = nodes.get(p.uri);
      if (q?.available && !out.includes(q)) out.push(q);
    }
    return out;
  };
  const depth = (n: WikiNode): number => {
    const known = depths.get(n.uri);
    if (known !== undefined) return known;
    onPath.add(n.uri);
    let d = 0;
    for (const q of parentsOf(n)) {
      if (!onPath.has(q.uri)) d = Math.max(d, depth(q) + 1);
    }
    onPath.delete(n.uri);
    depths.set(n.uri, d);
    return d;
  };
  const start = nodes.get(head);
  if (!start) return { steps: [], parentUnavailable: false };
  const steps: WikiStep[] = [{ node: start, altered: false }];
  const seen = new Set([start.uri]);
  let cur = start;
  for (;;) {
    const next = parentsOf(cur)
      .filter((q) => !seen.has(q.uri))
      .sort((a, b) => depth(b) - depth(a) || (a.uri < b.uri ? -1 : a.uri > b.uri ? 1 : 0))[0];
    if (!next) {
      return { steps, parentUnavailable: cur.parents.length > 0 && parentsOf(cur).length === 0 };
    }
    // any entry naming the parent with another CID marks the edge
    const altered = cur.parents.some((p) => p.uri === next.uri && p.cid !== next.cid);
    steps.push({ node: next, altered });
    seen.add(next.uri);
    cur = next;
  }
}

/** Reads the revisions reachable from `head` through `parents`, at most `max` of them. A
 * revision the engine does not have (`raw` fails) is an unavailable node. */
export async function loadWikiNodes(
  raw: (uri: string) => Promise<RawView>,
  head: string,
  max = WIKI_HISTORY_MAX,
): Promise<Map<string, WikiNode>> {
  const nodes = new Map<string, WikiNode>();
  const queue = [head];
  while (queue.length > 0 && nodes.size < max) {
    const uri = queue.shift() as string;
    if (nodes.has(uri)) continue;
    const view = await raw(uri).catch(() => undefined);
    const node = wikiNodeOf(uri, view);
    nodes.set(uri, node);
    for (const p of node.parents) if (!nodes.has(p.uri)) queue.push(p.uri);
  }
  return nodes;
}
