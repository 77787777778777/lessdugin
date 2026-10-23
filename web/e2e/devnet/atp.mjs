// SPDX-License-Identifier: AGPL-3.0-or-later
// Minimal atproto helpers for the devnet e2e tests, independent of the engine: XRPC over fetch
// with app-password sessions, record keys (`fk/1`, TIDs; docs/spec/data-model.md §3.1) and the
// record shapes of a forum, a membership, a post and a comment (§6), so the SPA is tested
// against records written straight to the PDS API.

import { createHash } from "node:crypto";

/** The NSID prefix of this build (`.temp` until release 0.1, crates/lexicon `ns::NS`). */
export const NS = "online.7778777.lessdugin.temp";

const B32 = "234567abcdefghijklmnopqrstuvwxyz";

/** `b32s(bytes)[0..n]`: MSB-first 5-bit groups (data-model.md §3.1). */
export function b32s(bytes, n) {
  let out = "";
  let acc = 0;
  let bits = 0;
  for (const b of bytes) {
    acc = (acc << 8) | b;
    bits += 8;
    while (bits >= 5 && out.length < n) {
      bits -= 5;
      out += B32[(acc >> bits) & 31];
    }
    acc &= (1 << bits) - 1;
    if (out.length >= n) break;
  }
  return out;
}

/** `fk(forumUri)` (`fk/1`). */
export function fk(forumUri) {
  const h = createHash("sha256");
  h.update(Buffer.from("lessdugin-fk/1", "utf8"));
  h.update(Buffer.from([0x1f]));
  h.update(Buffer.from(forumUri.normalize("NFC"), "utf8"));
  return b32s(h.digest(), 12);
}

let lastMicros = 0n;

/** A fresh TID (13 characters, monotonic within this process). */
export function tid() {
  let micros = BigInt(Date.now()) * 1000n;
  if (micros <= lastMicros) micros = lastMicros + 1n;
  lastMicros = micros;
  const v = (micros << 10n) | 7n;
  let out = "";
  for (let i = 12; i >= 0; i--) out += B32[Number((v >> BigInt(i * 5)) & 31n)];
  return out;
}

/** The TID of the time `ms` (clock id 9), for records dated on purpose. */
export function tidAt(ms) {
  const v = ((BigInt(ms) * 1000n) << 10n) | 9n;
  let out = "";
  for (let i = 12; i >= 0; i--) out += B32[Number((v >> BigInt(i * 5)) & 31n)];
  return out;
}

/** RFC 3339 UTC with milliseconds (data-model.md §2.4). */
export function now() {
  return new Date().toISOString();
}

/** One XRPC call; throws with the status and body on failure. */
export async function xrpc(pds, nsid, { token, body, query } = {}) {
  const url = new URL(`/xrpc/${nsid}`, pds);
  for (const [k, v] of Object.entries(query ?? {})) url.searchParams.set(k, v);
  const res = await fetch(url, {
    method: body === undefined ? "GET" : "POST",
    headers: {
      ...(body === undefined ? {} : { "content-type": "application/json" }),
      ...(token ? { authorization: `Bearer ${token}` } : {}),
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  const text = await res.text();
  if (!res.ok) throw new Error(`${nsid}: ${res.status} ${text}`);
  return text === "" ? {} : JSON.parse(text);
}

/** An app-password session: `{ did, token, pds }`. */
export async function login(pds, account) {
  const s = await xrpc(pds, "com.atproto.server.createSession", {
    body: { identifier: account.handle, password: account.appPassword },
  });
  return { did: s.did, token: s.accessJwt, pds };
}

/** `createRecord` with an explicit rkey; returns `{ uri, cid }`. */
export async function put(session, collection, rkey, record) {
  return xrpc(session.pds, "com.atproto.repo.createRecord", {
    token: session.token,
    body: {
      repo: session.did,
      collection: `${NS}.${collection}`,
      rkey,
      record: { $type: `${NS}.${collection}`, ...record },
    },
  });
}

export function markdown(text) {
  return { $type: `${NS}.markdown`, text };
}

/** The `open` preset of `forum create` (data-model.md §6.1), every policy field explicit. */
export function openPolicy() {
  return {
    moderation: "mod/1",
    karma: { algorithm: "karma/1", pairCap: 25, establish: 1, establishSeeds: 2 },
    ranking: {
      algorithm: "magic/1",
      offsetHours: 2,
      exponentMilli: 1150,
      curatedBonus: 10,
      frontpageBonus: 0,
      cutoffDays: 90,
    },
    voting: "reacts",
    reactPalette: "default/1",
    gates: {
      reactSecond: 5,
      reactCreate: 10,
      reactAnti: 20,
      authorModPersonal: 50,
      authorModFrontpage: 2000,
      createTag: 10,
      wikiEdit: 5,
      sequence: 1000,
    },
    display: { collapseBelow: -5, newUsers: "collapse" },
    authorModeration: true,
    tagCreation: "open",
  };
}

/** Writes a forum declaration and the owner's membership; returns the forum's uri and fk. */
export async function createForum(session, { slug, name, description, membership = "open" }) {
  const uri = `at://${session.did}/${NS}.forum/${slug}`;
  const key = fk(uri);
  await put(session, "forum", slug, {
    name,
    ...(description ? { description } : {}),
    membership,
    defaultTier: "frontpage",
    policy: openPolicy(),
    createdAt: now(),
  });
  await put(session, "membership", key, { forum: uri, createdAt: now() });
  return { uri, fk: key };
}

/** Writes a post; returns `{ uri, cid, rkey }`. */
export async function createPost(session, forum, { title, body }) {
  const rkey = `${forum.fk}-${tid()}`;
  const r = await put(session, "post", rkey, {
    forum: forum.uri,
    kind: "article",
    title,
    body: markdown(body),
    createdAt: now(),
  });
  return { ...r, rkey };
}

/** Writes a post whose TID and `createdAt` are both the time `atMs` (a future-dated post when
 * `atMs` is ahead of the reader's clock); returns `{ uri, cid, rkey }`. */
export async function createPostAt(session, forum, { title, body, atMs }) {
  const rkey = `${forum.fk}-${tidAt(atMs)}`;
  const r = await put(session, "post", rkey, {
    forum: forum.uri,
    kind: "article",
    title,
    body: markdown(body),
    createdAt: new Date(atMs).toISOString(),
  });
  return { ...r, rkey };
}

/** Writes a top-level comment on `post`; returns `{ uri, cid, rkey }`. */
export async function createComment(session, forum, post, body) {
  const rootTid = post.rkey.split("-")[1];
  const rkey = `${forum.fk}-${rootTid}-${tid()}`;
  const r = await put(session, "comment", rkey, {
    root: { uri: post.uri, cid: post.cid },
    forum: forum.uri,
    body: markdown(body),
    createdAt: now(),
  });
  return { ...r, rkey };
}
