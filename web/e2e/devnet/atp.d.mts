// SPDX-License-Identifier: AGPL-3.0-or-later
export const NS: string;
export function b32s(bytes: Uint8Array, n: number): string;
export function fk(forumUri: string): string;
export function tid(): string;
export function now(): string;
export function xrpc(
  pds: string,
  nsid: string,
  opts?: { token?: string; body?: unknown; query?: Record<string, string> },
): Promise<Record<string, unknown>>;
export interface Session {
  did: string;
  token: string;
  pds: string;
}
export function login(
  pds: string,
  account: { handle: string; appPassword: string },
): Promise<Session>;
export interface Written {
  uri: string;
  cid: string;
}
export function put(
  session: Session,
  collection: string,
  rkey: string,
  record: Record<string, unknown>,
): Promise<Written>;
export function markdown(text: string): { $type: string; text: string };
export function openPolicy(): Record<string, unknown>;
export interface ForumRef {
  uri: string;
  fk: string;
}
export function createForum(
  session: Session,
  opts: { slug: string; name: string; description?: string; membership?: "open" | "approval" },
): Promise<ForumRef>;
export function createPost(
  session: Session,
  forum: ForumRef,
  opts: { title: string; body: string },
): Promise<Written & { rkey: string }>;
export function createComment(
  session: Session,
  forum: ForumRef,
  post: Written & { rkey: string },
  body: string,
): Promise<Written & { rkey: string }>;
